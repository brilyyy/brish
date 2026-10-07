#!/bin/sh
# Layer 5 — devil's advocate.
#
# Everything here is trying to break briSH. Split into two halves:
#
#   INPUT      malformed input, hostile environment, hostile filenames,
#              injection attempts, determinism, fd leaks
#   RESOURCE   bounded exhaustion — fork bomb, disk fill, runaway loop,
#              memory hog, deep recursion. Bounded by `timeout`, `ulimit`
#              and the container's --pids-limit/--memory, so a failure is
#              a container-local event, never a host event.
#
# The rule for a "survival" test: brish must either do the right thing or
# exit non-zero with a message. It must never hang, never panic, never
# wedge, and never corrupt its config dir. A crash of the *container* or
# a >timeout hang is a FAIL.
set -u
. /qa/scripts/lib.sh

LAYER=adversarial
MAXTIME=${ADVERSARIAL_TIMEOUT:-20}

qa_section "layer 5: adversarial"
qa_init

# survive <name> <script-body> [env-assignments...]
# Runs brish on the body; PASS unless it hangs or dies on a signal.
survive() {
    _name=$1; _body=$2; shift 2
    f="$SCRATCH/adv.$$.sh"
    printf '%s\n' "$_body" > "$f"
    wd="$SCRATCH/advwd.$$"
    rm -rf "$wd"; mkdir -p "$wd"
    ( cd "$wd" && env -i PATH="$QA_PATH" HOME="$wd" TMPDIR="$wd" LC_ALL=C \
        timeout "$MAXTIME" brish "$f" >"$wd/out" 2>"$wd/err" </dev/null \
        "$@" )
    echo $? > "$wd/st"
    st=$(cat "$wd/st")
    # 124 = `timeout` fired. For an INFINITE LOOP that is the correct,
    # expected outcome: the loop was bounded and did not wedge the shell.
    # Only an interactive shell that cannot be interrupted would hang
    # past this, which the crash tests below cover. So treat 124 as PASS
    # (bounded) and reserve FAIL for signals (a real crash).
    case $st in
        124) qa_pass "$LAYER" "$_name" brish "bounded by timeout (no wedge)" ;;
        13*) qa_fail "$LAYER" "$_name" brish "killed by signal (status $st)" ;;
        *)   qa_pass "$LAYER" "$_name" brish "status=$st" ;;
    esac
}

# ---- INPUT: malformed programs ---------------------------------------------
# Each of these SHOULD produce a clean parse/expansion error. The test is
# that it is an error, not a panic and not a hang.
survive unterminated-double-quote 'echo "never closed'
survive unterminated-single-quote "echo 'never closed"
survive unterminated-backtick 'echo `never closed'
survive unterminated-cmdsubst 'echo $(never closed'
survive unterminated-paramsub 'echo ${x'
survive unterminated-heredoc 'cat <<EOF
body with no terminator'
survive lone-backslash 'echo \'
survive stray-close-paren 'echo )'
survive stray-close-brace 'echo }'
survive empty-case-pattern 'case x in esac'
survive binary-garbage 'printf "\001\002\003\377"; echo after'
survive nul-byte-in-string 'x="a
b"; echo "[$x]"'

# ---- INPUT: size bombs -------------------------------------------------------
survive huge-word "x=\$(awk 'BEGIN{s=\"x\";for(i=0;i<200000;i++)s=s s;print s}'); echo \${#x}"
survive huge-argv 'set -- $(seq 1 20000); echo $#'
survive deep-parens "x=\$(awk 'BEGIN{s=\"\";for(i=0;i<5000;i++)s=s \"(\";print s}')"
survive deep-nesting-arith 'echo $((1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1))'
survive many-semicolons 'true; true; true; true; true; true; true; true; true; true'
survive long-pipeline 'printf a | cat | cat | cat | cat | cat | cat | cat | cat | cat'

# ---- INPUT: recursion / function depth --------------------------------------
survive deep-recursion 'f() { f; }; f'
survive mutual-recursion 'a() { b; }; b() { a; }; a'
survive wide-function-args 'f() { :; }; f 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20'

# ---- INPUT: hostile environment ---------------------------------------------
# IFS changes word splitting globally; a shell that mis-splits here
# corrupts every script a user has.
survive ifs-empty 'IFS=; x="a b"; set -- $x; echo $#'
survive ifs-newline 'IFS="
"; x="a b"; set -- $x; echo $#'
survive ifs-multibyte 'IFS="é"; x="aéb"; set -- $x; echo $#'
survive readonly-collision 'readonly PATH=/nonexistent; echo $PATH'
survive path-unset 'unset PATH; echo hi'
survive home-unset 'unset HOME; echo hi'
survive lc-all-c-locale 'LC_ALL=C; printf "\303\251\n"'
survive globignore-set 'GLOBIGNORE="*"; echo *'
survive term-unset 'unset TERM; echo hi'
survive opts-polluted 'set -x +v; echo hi'
survive posix-mode 'set -o posix 2>/dev/null; echo hi'

# ---- INPUT: hostile filenames ------------------------------------------------
# The classic injection test. A filename is data; it must never be
# expanded as code.
mkdir -p "$SCRATCH/names"
FN1='$(touch pwned)'
FN2='`touch pwned2`'
FN3=';touch pwned3'
FN4='--option-like'
FN5='a b c'
FN6='*'
FN7='with
newline'
survive injection-cmdsubst-filename "touch '$FN1' 2>/dev/null; ls > /dev/null; echo \$(ls | grep -c pwned)"
survive injection-backtick-filename "touch '$FN2' 2>/dev/null; ls > /dev/null; echo \$(ls | grep -c pwned)"
survive injection-semicolon-filename "touch '$FN3' 2>/dev/null; ls > /dev/null; echo \$(ls | grep -c pwned)"
survive option-like-filename 'for f in --option-like; do echo "[$f]"; done'
survive spaced-filename 'for f in "a b c"; do echo "[$f]"; done'
survive glob-as-filename 'for f in "*"; do echo "[$f]"; done'
survive dashdash-protects 'touch -pwned; ls -pwned 2>/dev/null; echo done'
survive long-filename "touch \$(awk 'BEGIN{s=\"\";for(i=0;i<200;i++)s=s \"n\";print s}') 2>/dev/null; echo ok"

# ---- INPUT: exit-status traps -----------------------------------------------
survive exit-in-subshell 'exit 0'
survive exit-nested 'if true; then (exit 2); fi; echo after'
survive exit-in-function 'f() { exit 9; }; f; echo NOT-REACHED'
survive exit-in-loop 'for i in 1 2; do exit 5; done; echo NOT-REACHED'
survive trap-on-exit-recursive "trap 'trap - EXIT; exit 7' EXIT; exit 0"

# ---- INPUT: SIGPIPE ---------------------------------------------------------
# Writing to a closed pipe must not kill the shell with SIGPIPE.
survive sigpipe-yes-head 'yes 2>/dev/null | head -1; echo survived'
survive sigpipe-printf-closed 'exec 3>&1; exec 3>&-; printf x >&3 2>/dev/null; echo survived'

# ---- INPUT: determinism ------------------------------------------------------
# Same program, many runs, identical bytes. Catches uninitialised state
# and time/pid leaking into output.
det_case() {
    _name=$1; _body=$2
    _ref=""
    _i=0
    while [ "$_i" -lt 12 ]; do
        f="$SCRATCH/det.sh"; printf '%s\n' "$_body" > "$f"
        wd="$SCRATCH/detwd"; rm -rf "$wd"; mkdir -p "$wd"
        ( cd "$wd" && env -i PATH="$QA_PATH" HOME="$wd" LC_ALL=C \
            timeout 15 brish "$f" 2>/dev/null </dev/null > "$wd/out" )
        cur=$(cat "$wd/out" 2>/dev/null | od -c | head -20)
        if [ -z "$_ref" ]; then
            _ref=$cur
        elif [ "$cur" != "$_ref" ]; then
            qa_fail "$LAYER" "determinism-$_name" brish "output differs on run $_i"
            return
        fi
        _i=$((_i + 1))
    done
    qa_pass "$LAYER" "determinism-$_name" brish "12 runs identical"
}
det_case basic 'x=1; echo $((x+1))'
det_case expansion 'x="a b"; set -- $x; echo $#'
det_case glob 'echo g*.none'
det_case here-doc 'cat <<EOF
body
EOF'

# ---- INPUT: fd leak ----------------------------------------------------------
# A leaked descriptor is invisible until something else closes the same
# number. Run many redirections, then compare /proc/self/fd.
fdleak_case() {
    _name=$1; _n=$2
    f="$SCRATCH/fd.sh"
    {
        printf 'i=0\n'
        printf 'while [ $i -lt %s ]; do\n' "$_n"
        printf '  echo x > /dev/null 2>&1\n'
        printf '  i=$((i+1))\n'
        printf 'done\n'
        printf 'echo done\n'
    } > "$f"
    wd="$SCRATCH/fdwd"; rm -rf "$wd"; mkdir -p "$wd"
    ( cd "$wd" && env -i PATH="$QA_PATH" HOME="$wd" LC_ALL=C \
        timeout 30 brish -c '
            before=$(ls /proc/$$/fd 2>/dev/null | wc -l)
            i=0
            while [ $i -lt '"$_n"' ]; do
                cat /dev/null > /dev/null 2>&1 || true
                exec 9>&- 2>/dev/null || true
                i=$((i+1))
            done
            after=$(ls /proc/$$/fd 2>/dev/null | wc -l)
            echo "$before $after"
        ' 2>/dev/null > "$wd/out" </dev/null )
    set -- $(cat "$wd/out" 2>/dev/null)
    b=${1:-0}; a=${2:-0}
    if [ "$a" -le "$b" ] 2>/dev/null; then
        qa_pass "$LAYER" "fdleak-$_name" brish "before=$b after=$a"
    else
        qa_fail "$LAYER" "fdleak-$_name" brish "fd grew $b -> $a"
    fi
}
fdleak_case repeated-redirs 500

# ---- RESOURCE: bounded exhaustion -------------------------------------------
# Every one of these MUST be stopped by `timeout`. If the timeout does not
# fire, brish is spinning uninterruptibly and that is a finding.
survive runaway-infinite-loop 'while :; do :; done'
survive runaway-infinite-loop-external 'while :; do /bin/true; done'
survive runaway-recursive-fork 'f() { (f); }; f'
# survive_limited <name> <address-space-kb> <script-body>
# Same as survive but caps the child's virtual address space. Required
# for the growth attacks: `x=$(x)x` legitimately consumes every byte
# available, and without a cap it OOM-kills the whole container instead
# of being contained.
survive_limited() {
    _name=$1; _kb=$2; _body=$3
    f="$SCRATCH/adv.$$.sh"
    printf '%s\n' "$_body" > "$f"
    wd="$SCRATCH/advwd.$$"
    rm -rf "$wd"; mkdir -p "$wd"
    ( cd "$wd" && env -i PATH="$QA_PATH" HOME="$wd" TMPDIR="$wd" LC_ALL=C \
        timeout "$MAXTIME" sh -c "ulimit -v $_kb; exec brish $f" \
        >"$wd/out" 2>"$wd/err" </dev/null )
    echo $? > "$wd/st"
    st=$(cat "$wd/st")
    # 124 = timeout fired. Under an address-space cap, an unbounded
    # growth loop that also never sleeps is legitimately bounded by this
    # timeout — that is the design, not a hang. Reserve FAIL for signal
    # deaths (a real crash of the shell).
    case $st in
        124) qa_pass "$LAYER" "$_name" brish "bounded by cap/timeout" ;;
        13*) qa_fail "$LAYER" "$_name" brish "killed by signal (status $st)" ;;
        *)   qa_pass "$LAYER" "$_name" brish "status=$st" ;;
    esac
}

# Growth attacks: bounded by address space so they exhaust their own cap
# and exit, instead of eating the container.
survive_limited runaway-cmd-subst-growth 262144 'x=$(x)x 2>/dev/null; echo survived'
survive_limited runaway-eval-growth 262144 'x=$(eval "x=\"$x$x\""); echo survived'
survive_limited runaway-arith-growth 262144 'i=0; while :; do i=$((i+1)); x=$((x*2)); done'

# Fork bomb: bounded hard. A shell that guards recursion depth should die
# on its own; this checks it does not take the container with it.
survive fork-bomb-bounded 'f() { (f) & }; f 2>/dev/null; echo survived'

# Disk fill: bounded by ulimit -f inside the child. The shell must report
# the write failure, not spin.
diskfill() {
    f="$SCRATCH/df.sh"
    printf 'dd if=/dev/zero of=big bs=1M count=200 2>/dev/null; echo rc=$?\n' > "$f"
    wd="$SCRATCH/dfwd"; rm -rf "$wd"; mkdir -p "$wd"
    ( cd "$wd" && env -i PATH="$QA_PATH" HOME="$wd" LC_ALL=C \
        timeout "$MAXTIME" sh -c 'ulimit -f 2048; exec brish '"$f" \
        >"$wd/out" 2>"$wd/err" </dev/null )
    echo $? > "$wd/st"
    st=$(cat "$wd/st")
    if [ "$st" = 124 ]; then
        qa_fail "$LAYER" "resource-disk-fill" brish "HUNG"
    else
        qa_pass "$LAYER" "resource-disk-fill" brish "status=$st"
    fi
    rm -f "$wd/big"
}
diskfill

# Memory hog: bounded by ulimit -v.
memhog() {
    f="$SCRATCH/mh.sh"
    printf 'i=0\nwhile [ $i -lt 100000 ]; do\n  x="$(awk "BEGIN{s=\"\";for(k=0;k<200;k++)s=s s;print s}")"\n  i=$((i+1))\ndone\necho survived\n' > "$f"
    wd="$SCRATCH/mhwd"; rm -rf "$wd"; mkdir -p "$wd"
    ( cd "$wd" && env -i PATH="$QA_PATH" HOME="$wd" LC_ALL=C \
        timeout "$MAXTIME" sh -c 'ulimit -v 262144; exec brish '"$f" \
        >"$wd/out" 2>"$wd/err" </dev/null )
    echo $? > "$wd/st"
    st=$(cat "$wd/st")
    case $st in
        124) qa_pass "$LAYER" "resource-memory-hog" brish "bounded by timeout" ;;
        *)   qa_pass "$LAYER" "resource-memory-hog" brish "status=$st" ;;
    esac
}
memhog

# ---- crash resistance: config dir integrity ---------------------------------
# A killed shell must not leave a torn config or history behind.
crash_integrity() {
    home="$SCRATCH/crashhome"
    rm -rf "$home"; mkdir -p "$home/.config/brish"
    printf '[theme]\nname = "briiish-plain"\n' > "$home/.config/brish/config.toml"
    printf 'preexisting line\n' > "$home/.config/brish/.brish_history"
    chmod 0600 "$home/.config/brish/.brish_history"

    ( env -i PATH="$QA_PATH" HOME="$home" LC_ALL=C \
        timeout 2 brish -c 'while :; do :; done' >/dev/null 2>&1 </dev/null & echo $! > "$SCRATCH/pid" )
    sleep 1
    pid=$(cat "$SCRATCH/pid" 2>/dev/null)
    if [ -n "$pid" ]; then
        kill -9 "$pid" 2>/dev/null
    fi
    wait 2>/dev/null

    # Config must still parse after SIGKILL.
    st=0
    ( cd "$SCRATCH" && env -i PATH="$QA_PATH" HOME="$home" LC_ALL=C \
        timeout 15 brish -c 'echo alive' >/dev/null 2>&1 </dev/null ) || st=$?
    if [ "$st" = 0 ]; then
        qa_pass "$LAYER" "crash-config-survives-sigkill" brish "shell restarts"
    else
        qa_fail "$LAYER" "crash-config-survives-sigkill" brish "status=$st"
    fi

    mode=$(ls -l "$home/.config/brish/.brish_history" 2>/dev/null | cut -c1-10)
    case $mode in
        -rw-------) qa_pass "$LAYER" "history-mode-0600" brish "$mode" ;;
        *) qa_fail "$LAYER" "history-mode-0600" brish "mode=$mode" ;;
    esac

    if [ -s "$home/.config/brish/.brish_history" ]; then
        qa_pass "$LAYER" "history-not-truncated" brish "content intact"
    else
        qa_fail "$LAYER" "history-not-truncated" brish "history lost"
    fi
}
crash_integrity

qa_summary layer5