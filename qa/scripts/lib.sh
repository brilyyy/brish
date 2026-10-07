#!/bin/sh
# Shared harness primitives for the briSH QA suite.
#
# Sourced by every qa/scripts/*.sh. POSIX sh (dash) only — these scripts
# run under /bin/sh inside the QA image, and one of the oracles under
# test is dash. A harness bug must never be mistaken for a brish bug,
# so everything here stays boring.
#
# Convention:
#   * every layer writes TSV rows to $OUT/results.tsv
#   * verdict is one of PASS / FAIL / SKIP / N-A
#   * a N-A row means "this oracle cannot express it", not "brish lost"
#
# Environment knobs:
#   OUT         output dir (default /out)
#   QA_STRICT   1 = exit non-zero on any FAIL (default 1)

set -u

OUT=${OUT:-/out}
mkdir -p "$OUT"

# Each layer writes its OWN results file. Sharing one file meant
# qa_init's truncate wiped every previous layer's findings, leaving only
# the last layer in the report. 60-report.sh concatenates them.
RESULTS=""
FAILURES=""

# PATH used for every child process. Must include /usr/local/bin (brish
# lives there) and /usr/bin (mksh). A missing oracle silently reads as
# exit 127 = "bug"; that cost us a whole debugging round once.
QA_PATH=/usr/local/bin:/usr/bin:/bin

QA_PASS=0
QA_FAIL=0
QA_SKIP=0
QA_NA=0

# oracles: name -> command prefix. `busybox` needs the subcommand.
qa_oracles() {
    printf '%s\n' dash bash mksh busybox brish
}

# Full command for a shell name, as a shell word list.
qa_shell_cmd() {
    case "$1" in
        busybox) printf 'busybox ash' ;;
        *)       printf '%s' "$1" ;;
    esac
}

qa_init() {
    _l=${LAYER:-unknown}
    RESULTS="$OUT/results.$_l.tsv"
    FAILURES="$OUT/failures.$_l.tsv"
    : > "$RESULTS"
    : > "$FAILURES"
    SCRATCH=$(mktemp -d)
    trap 'qa_cleanup' EXIT INT TERM
    printf 'layer\tcase\toracle\tverdict\tdetail\n' >> "$RESULTS"
}

qa_cleanup() {
    [ -n "${SCRATCH:-}" ] && rm -rf "$SCRATCH"
}

qa_section() {
    printf '\n=== %s ===\n' "$1"
}

# qa_record <layer> <case> <oracle> <verdict> [detail]
qa_record() {
    _layer=$1; _case=$2; _oracle=$3; _verdict=$4; _detail=${5:-}
    printf '%s\t%s\t%s\t%s\t%s\n' \
        "$_layer" "$_case" "$_oracle" "$_verdict" "$_detail" >> "$RESULTS"
    case $_verdict in
        PASS) QA_PASS=$((QA_PASS + 1)) ;;
        FAIL) QA_FAIL=$((QA_FAIL + 1))
              printf '%s\t%s\t%s\t%s\n' "$_layer" "$_case" "$_detail" \
                  >> "$FAILURES" ;;
        SKIP) QA_SKIP=$((QA_SKIP + 1)) ;;
        NA)   QA_NA=$((QA_NA + 1)) ;;
    esac
}

qa_pass() { qa_record "$1" "$2" "$3" PASS "${4:-}"; }
qa_fail() { qa_record "$1" "$2" "$3" FAIL "${4:-}"; }
qa_skip() { qa_record "$1" "$2" "$3" SKIP "${4:-}"; }
qa_na()   { qa_record "$1" "$2" "$3" NA "${4:-}"; }

qa_summary() {
    _name=${1:-layer}
    printf '\n--- %s: pass=%s fail=%s skip=%s n/a=%s ---\n' \
        "$_name" "$QA_PASS" "$QA_FAIL" "$QA_SKIP" "$QA_NA"
    [ "$QA_FAIL" -eq 0 ]
}

# run_shell <shell-name> <script-file> <workdir> [timeout-secs]
# Runs the named shell on the script with a scrubbed, identical
# environment. stdout -> <workdir>/out, stderr -> <workdir>/err,
# exit status -> <workdir>/st.
#
# Status travels by file, never stdout: a script that prints newlines
# would corrupt an inline separator.
run_shell() {
    _sh=$1; _script=$2; _wd=$3; _to=${4:-10}
    _cmd=$(qa_shell_cmd "$_sh")
    mkdir -p "$_wd"
    ( cd "$_wd" && env -i \
        PATH="$QA_PATH" \
        HOME="$_wd" \
        TMPDIR="$_wd" \
        LC_ALL=C \
        timeout "$_to" sh -c "$_cmd \"\$1\"" qa "$_script" \
        >"$_wd/out" 2>"$_wd/err" </dev/null )
    echo $? > "$_wd/st"
}

st_of()  { cat "$1/st" 2>/dev/null || echo 999; }
out_of() { cat "$1/out" 2>/dev/null; }
err_of() { cat "$1/err" 2>/dev/null; }

# Trim trailing newlines so `echo` newline differences read as noise
# while content differences remain signal.
trim() { sed -e 's/[[:space:]]*$//'; }

# qa_expect <layer> <case> <shell> <script-file>
# Runs one script under one shell and asserts status 0 + empty stderr
# unless the case name says otherwise. Kept simple on purpose; per-case
# assertions live in the case file format instead.
qa_expect_ok() {
    _layer=$1; _case=$2; _sh=$3; _script=$4
    _wd="$SCRATCH/exp.$$"
    rm -rf "$_wd"
    run_shell "$_sh" "$_script" "$_wd" 15
    _st=$(st_of "$_wd")
    if [ "$_st" = 0 ]; then
        qa_pass "$_layer" "$_case" "$_sh"
    else
        qa_fail "$_layer" "$_case" "$_sh" \
            "status=$_st stderr=$(err_of "$_wd" | head -2 | tr '\n' ' ')"
    fi
}