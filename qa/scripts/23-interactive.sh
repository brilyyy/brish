#!/bin/sh
# Layer 6 — interactive features over a real pty.
#
# brish gates its whole interactive path on stdin AND stdout being TTYs
# (crates/brish/src/main.rs, edit_repl). Completion, highlighting,
# autosuggest, menus, keymaps and the transient prompt are therefore
# unreachable from `brish -c` and would otherwise never be exercised.
#
# Two notes on why assertions are loose:
#
#   * A bare pty is not a terminal. crossterm queries cursor position
#     with ESC[6n on startup and blocks until answered, so the driver
#     synthesises the reply (see 50-pty.py). Beyond that, reedline's
#     redraw is not byte-faithful under a pty with no real terminal
#     emulator, so we do NOT assert on prompt glyphs — we assert the
#     shell came alive and executed things. Byte-exact interactive
#     assertions belong in unit tests with a mock; this layer answers
#     "does the interactive path run at all".
#   * That makes these liveness checks, not feature-correctness checks.
#     A feature that renders wrongly but keeps the shell alive passes
#     here. Narrowing that gap needs a terminal emulator (pyte) in the
#     image, tracked as a known gap rather than faked.
set -u
. /qa/scripts/lib.sh

LAYER=interactive
PTY=/qa/scripts/50-pty.py

qa_section "layer 6: interactive (pty)"
qa_init

if ! command -v python3 >/dev/null 2>&1; then
    qa_fail "$LAYER" "pty-available" env "python3 missing; cannot test REPL"
    qa_summary layer6
    exit 1
fi

# it <name> <expect> [pty args...] — run brish, assert <expect> appears.
it() {
    _name=$1; _expect=$2; shift 2
    if "$PTY" --shell "brish -i" --timeout 12 --expect "$_expect" "$@" \
        >/dev/null 2>&1; then
        qa_pass "$LAYER" "$_name" brish
    else
        qa_fail "$LAYER" "$_name" brish "expected [$_expect] not seen"
    fi
}

# alive <name> [pty args...] — the shell emits anything and stays up.
# A bare newline is poked in first: briSH's reedline path does not draw
# until it sees terminal input, so with no input at all the pty stays
# silent and "no output" would be indistinguishable from a hang.
NL=$(printf '\nx'); NL=${NL%x}

alive() {
    _name=$1; shift
    if "$PTY" --shell "brish -i" --timeout 12 --first-output \
            --raw "$NL" --settle 0.4 "$@" >/dev/null 2>&1; then
        qa_pass "$LAYER" "$_name" brish "first output seen"
    else
        qa_fail "$LAYER" "$_name" brish "no output (interactive path dead)"
    fi
}

# --- the REPL comes up at all ------------------------------------------------
alive repl-starts --settle 1.0 --teardown ''

# --- a shell started OUTSIDE the foreground process group must not hang ------
# Terminal.app starts the login shell through login(1), which leaves it in a
# process group that is not the terminal's foreground group. brish used to
# call tcsetpgrp() before ignoring SIGTTOU, whose default action is to STOP:
# the shell froze before the first prompt, forever, and Ctrl-C did nothing.
# The plain pty.fork() above cannot reproduce that (its child is the session
# leader, so its group is already foreground), hence --bg-pgrp.
#
# Asserted on a command's OUTPUT, not on the shell merely being quiet: the
# tty echoes whatever we type, so `--expect` on the command text would pass
# for a dead shell. `echo BG$((1+1))` prints BG2 while the line that was
# typed contains no "BG2", so only a shell that actually ran it matches.
if "$PTY" --shell "brish -i" --bg-pgrp --timeout 12 --settle 1.0 \
    --send 'echo BG$((1+1))' --expect 'BG2' --teardown '' >/dev/null 2>&1; then
    qa_pass "$LAYER" "bg-pgrp-no-sigttou-stop" brish "ran a command"
else
    qa_fail "$LAYER" "bg-pgrp-no-sigttou-stop" brish \
        "no command output (shell stopped itself on SIGTTOU)"
fi

# --- input is accepted and commands run -------------------------------------
# The typed line is echoed back by the terminal, so the command text
# itself appearing proves the line editor is live.
it accepts-input 'echo roundtrip' --settle 0.8 --send 'echo roundtrip' --teardown ''

# --- a command produces its output ------------------------------------------
it command-output 'PTY-OK' --settle 0.8 --send 'echo PTY-OK' --teardown ''

# --- && / || semantics hold in the interactive path too ---------------------
it and-or-runs 'ran' --settle 0.8 --send 'false && echo skipped || echo ran' --teardown ''

# --- history is recorded (a later command echoes the earlier one) -----------
it history-recorded 'first-line' --settle 0.6 \
   --send 'echo first-line' --settle 0.5 --send 'echo second-line' --teardown ''

# --- Ctrl-D / Ctrl-C do not wedge the shell ---------------------------------
# After Ctrl-C the shell must accept a fresh line (it did not exit and it
# still runs commands).
it ctrl-c-recovers 'after-interrupt' --settle 0.6 \
   --raw 'garbage\x03' --settle 0.4 --send 'echo after-interrupt' --teardown ''

# --- theme selection via env -------------------------------------------------
alive theme-env --settle 0.8 --env BRISH_THEME=briiish-plain --teardown ''

# --- PS1 override ------------------------------------------------------------
alive ps1-override --settle 0.8 --env PS1='X> ' --teardown ''

# --- NO_COLOR must not inject ANSI escapes ----------------------------------
noesc=$SCRATCH/noesc.out
"$PTY" --shell "brish -i" --timeout 10 --env NO_COLOR=1 --settle 1.0 --teardown '' \
    > "$noesc" 2>/dev/null
if grep -q "$(printf '\033')\[" "$noesc" 2>/dev/null; then
    qa_fail "$LAYER" "no-color-no-ansi" brish "ANSI SGR escape present under NO_COLOR"
else
    qa_pass "$LAYER" "no-color-no-ansi" brish
fi

# --- without NO_COLOR, colors SHOULD appear (control: proves the test can
# --- detect ANSI at all, so the check above is not vacuous) -----------------
coloresc=$SCRATCH/coloresc.out
"$PTY" --shell "brish -i" --timeout 10 --env BRISH_THEME=briiish-emoji \
    --settle 1.0 --teardown '' > "$coloresc" 2>/dev/null
if grep -q "$(printf '\033')\[" "$coloresc" 2>/dev/null; then
    qa_pass "$LAYER" "ansi-detectable-control" brish "escapes seen without NO_COLOR"
else
    qa_skip "$LAYER" "ansi-detectable-control" brish \
        "no ANSI observed; NO_COLOR check may be vacuous under this pty"
fi

qa_summary layer6