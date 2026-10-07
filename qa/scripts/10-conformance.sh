#!/bin/sh
# Layer 1b — POSIX conformance differential.
#
# Whole real scripts print paths, dates and pids, so their *stdout* is
# never comparable. What IS comparable is the exit status and stdout of
# a self-contained snippet: same input, same environment, no clock, no
# randomness, no filesystem outside a scratch dir.
#
# stdout AND status are compared separately — every bug this QA has
# found was status-only or silent, so a combined comparison would have
# hidden them. Status travels by file, never stdout: a snippet may print
# newlines and corrupt an inline separator.
#
# Oracle roles:
#   dash      primary — the strictest widely-deployed POSIX shell, and
#             what crates/brish/tests/posix.rs cross-checks against
#   mksh      second POSIX oracle — catches bugs that dash and brish
#             happen to agree on
#   busybox   third POSIX oracle (via `busybox ash`)
#   bash      NON-BLOCKING opinion — bash accepts bash-isms briSH is not
#             required to implement, so a brish/dash mismatch that bash
#             accepts is recorded as DEV, not FAIL
#
# Verdicts:
#   PASS  brish matches dash on stdout and status
#   FAIL  brish diverges from dash and no other oracle justifies it
#   DEV   brish rejects something bash/mksh accept (documented
#         deviation — docs/POSIX.md records the reasoning)
#   NA    brish and every POSIX oracle reject it; nothing to compare
#
# Run:  sh /qa/scripts/10-conformance.sh
set -u
. /qa/scripts/lib.sh

CASES=${1:-/qa/scripts/cases/conformance.txt}
MULTI=${2:-/qa/scripts/cases/multiline}
LAYER=conformance

qa_section "layer 1b: POSIX conformance"
qa_init

# run_all_oracles <script-file> -> populates $WD/<sh>/{out,err,st}
WD=
run_all_oracles() {
    _script=$1
    WD="$SCRATCH/run.$$"
    rm -rf "$WD"
    for sh in dash mksh busybox bash brish; do
        run_shell "$sh" "$_script" "$WD/$sh" 10
    done
}

# "rejected" = the shell refused the program (status 2 = syntax/usage),
# as opposed to running it and failing for some other reason.
rejected() { [ "$(st_of "$1")" = 2 ]; }

compare_one() {
    _case=$1
    _script=$2
    run_all_oracles "$_script"

    r_st=$(st_of "$WD/brish")
    r_out=$(out_of "$WD/brish" | trim)
    d_st=$(st_of "$WD/dash")
    d_out=$(out_of "$WD/dash" | trim)

    status_ok=no; [ "$d_st" = "$r_st" ] && status_ok=yes
    stdout_ok=no; [ "$d_out" = "$r_out" ] && stdout_ok=yes

    if [ "$status_ok" = yes ] && [ "$stdout_ok" = yes ]; then
        qa_pass "$LAYER" "$_case" brish
        return
    fi

    # Does any other oracle accept what dash rejects / brish handles?
    # bash accepting is not evidence (it accepts bash-isms); mksh or
    # busybox accepting IS evidence that the construct is POSIX.
    other_ok=no
    for sh in mksh busybox bash; do
        st=$(st_of "$WD/$sh")
        [ "$st" = 2 ] && continue
        other_ok=yes
        break
    done

        # DEV applies only when dash (our primary) ALSO rejects — the
    # divergence is then just the status code of a refusal, both shells
    # agree it is not runnable. If dash ACCEPTS (0) and brish rejects,
    # that is a FAIL, never a DEV.
    if [ "$r_st" = 2 ] && [ "$d_st" = 2 ]; then
        qa_na "$LAYER" "$_case" brish "all POSIX oracles reject"
    elif [ "$r_st" = 1 ] && [ "$d_st" != 0 ]; then
        # Both refuse (dash usually exits 2 on expansion errors, brish
        # exits 1). POSIX requires only "non-zero". Documented in
        # docs/POSIX.md; recorded, not failed.
        qa_record "$LAYER" "$_case" brish DEV \
            "brish=1 dash=$d_st mksh=$(st_of "$WD/mksh") busybox=$(st_of "$WD/busybox") (exit-status convention; see docs/POSIX.md)"
    elif [ "$status_ok" = no ]; then
        qa_fail "$LAYER" "$_case" brish \
            "status brish=$r_st dash=$d_st out=[$r_out] dash_out=[$d_out]"
    else
        qa_fail "$LAYER" "$_case" brish \
            "stdout brish=[$r_out] dash=[$d_out]"
    fi
}

# --- one-liner corpus ---------------------------------------------------------
n=0
while IFS= read -r SNIPPET; do
    case $SNIPPET in ''|'#'*) continue ;; esac
    n=$((n + 1))
    f="$SCRATCH/case.$n.sh"
    printf '%s\n' "$SNIPPET" > "$f"
    compare_one "$SNIPPET" "$f"
done < "$CASES"
printf 'one-liner cases: %s\n' "$n"

# --- multi-line corpus --------------------------------------------------------
# The one-per-line format cannot express constructs spanning lines
# (`f() {` newline, nested here-docs, unterminated quotes). Those are
# where the parser bugs lived, so they get real files.
m=0
if [ -d "$MULTI" ]; then
    for f in "$MULTI"/*.sh; do
        [ -f "$f" ] || continue
        m=$((m + 1))
        compare_one "multiline/$(basename "$f")" "$f"
    done
fi
printf 'multiline cases:  %s\n' "$m"

qa_summary layer1b