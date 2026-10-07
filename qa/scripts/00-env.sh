#!/bin/sh
# Layer 0 — environment sanity.
#
# Everything downstream trusts this layer. If an oracle is missing or
# broken, every comparison silently becomes "brish: command not found"
# (127) and the suite reports a wall of false failures that look like
# real bugs. Assert liveness up front instead of debugging it later.
set -u
. /qa/scripts/lib.sh

qa_section "layer 0: environment"
qa_init

# --- oracles actually execute -------------------------------------------------
for sh in $(qa_oracles); do
    cmd=$(qa_shell_cmd "$sh")
    # shellcheck disable=SC2086
    if out=$(env -i PATH="$QA_PATH" HOME=/tmp LC_ALL=C \
             timeout 10 sh -c "$cmd -c 'echo alive'" 2>/dev/null); then
        if [ "$out" = alive ]; then
            qa_pass env "$sh-executes" "$sh"
        else
            qa_fail env "$sh-executes" "$sh" "unexpected output: $out"
        fi
    else
        qa_fail env "$sh-executes" "$sh" "oracle did not run"
    fi
done

# --- brish is the build under test -------------------------------------------
ver=$(env -i PATH="$QA_PATH" HOME=/tmp brish --version 2>/dev/null | head -1)
if [ -n "$ver" ]; then
    qa_pass env "brish-version" brish "$ver"
else
    qa_fail env "brish-version" brish "no --version output"
fi

# --- we must NOT be root ------------------------------------------------------
# uid 0 makes test -r/-w/-x always true, which would hide the exact bug
# class brish shipped a fix for.
uid=$(id -u)
if [ "$uid" = 0 ]; then
    qa_fail env "not-root" env "running as uid 0; -r/-w/-x tables are meaningless"
else
    qa_pass env "not-root" env "uid=$uid"
fi

# --- scratch space ------------------------------------------------------------
if d=$(mktemp -d) && touch "$d/probe" && rm -rf "$d"; then
    qa_pass env "scratch-writable" env "$SCRATCH"
else
    qa_fail env "scratch-writable" env "cannot create temp dirs"
fi

# --- benchmark prerequisites (soft: missing tool = SKIP, not FAIL) ------------
for tool in taskset; do
    if command -v "$tool" >/dev/null 2>&1; then
        qa_pass env "has-$tool" env "present"
    else
        qa_skip env "has-$tool" env "absent; benchmarks will be unpinned"
    fi
done
if env -i PATH="$QA_PATH" python3 -c 'import pty,os,sys' 2>/dev/null; then
    qa_pass env "pty-driver" env "python3 pty module present"
else
    qa_fail env "pty-driver" env "python3 cannot import pty; interactive layer cannot run"
fi

qa_summary layer0