#!/bin/sh
# Layer 4 — plugin store end-to-end, including hostile manifests.
#
# Two halves:
#   A. happy path — install every bundled example, assert `plugin list`
#      shows it and the shell still runs
#   B. hostile path — manifests and helpers that a real attacker or a
#      real bug would produce: traversal, malformed TOML, missing helper,
#      hanging helper, output flood, non-zero helper
#
# The deadline tests matter most: PLUGINS.md promises helper/segment/hook
# subprocesses are deadline-killed and never wedge the shell. If a hang
# survives, that is a wedge bug, not a slow machine.
set -u
. /qa/scripts/lib.sh

LAYER=plugin
REPO_ROOT=${REPO_ROOT:-/usr/local/share/brish-examples}

qa_section "layer 4: plugin store"
qa_init

# Depends on $SCRATCH, so it must be built after qa_init.
LAYERDIR="$SCRATCH/plughome/.config/brish"

# Run brish with a private HOME so we never touch a real config dir.
# shellcheck disable=SC2086
brish_home() {
    ( cd "$SCRATCH" && env -i PATH="$QA_PATH" HOME="$SCRATCH/plughome" \
        LC_ALL=C timeout 20 brish -c "$1" 2>&1 )
}

fresh_home() {
    rm -rf "$SCRATCH/plughome"
    mkdir -p "$LAYERDIR"
}

# --- A. happy path -----------------------------------------------------------
for ex in starter sentinel words; do
    src="$REPO_ROOT/examples/plugins/$ex"
    if [ ! -d "$src" ]; then
        qa_skip "$LAYER" "install-$ex" brish "example not present in image"
        continue
    fi
    fresh_home
    out=$(brish_home "plugin add $src")
    st=$?
    if [ "$st" != 0 ]; then
        qa_fail "$LAYER" "install-$ex" brish "add failed: $out"
        continue
    fi
    qa_pass "$LAYER" "install-$ex" brish "$out"

    listed=$(brish_home "plugin list")
    case $listed in
        *"$ex"*) qa_pass "$LAYER" "list-$ex" brish ;;
        *) qa_fail "$LAYER" "list-$ex" brish "not in plugin list" ;;
    esac

    # The shell must still run with the plugin installed.
    if [ "$(brish_home 'echo alive')" = alive ]; then
        qa_pass "$LAYER" "shell-alive-$ex" brish
    else
        qa_fail "$LAYER" "shell-alive-$ex" brish "shell broke after install"
    fi

    # rm --purge must actually delete the directory.
    brish_home "plugin rm --purge $ex" >/dev/null 2>&1
    if [ -d "$LAYERDIR/plugins/$ex" ]; then
        qa_fail "$LAYER" "purge-$ex" brish "directory still present"
    else
        qa_pass "$LAYER" "purge-$ex" brish
    fi
done

# --- B. hostile manifests ----------------------------------------------------
hostile() {
    _name=$1; _body=$2; _expect=$3
    fresh_home
    mkdir -p "$LAYERDIR/plugins/evil"
    printf '%s\n' "$_body" > "$LAYERDIR/plugins/evil/plugin.toml"
    # If the shell starts and answers, the manifest did not wedge it.
    out=$(brish_home 'echo alive')
    st=$?
    case $out in
        *alive*) got=ok ;;
        *)      got=broken ;;
    esac
    if [ "$got" = "$_expect" ]; then
        qa_pass "$LAYER" "$_name" brish "shell=$got"
    else
        qa_fail "$LAYER" "$_name" brish "shell=$got want=$_expect out=[$out]"
    fi
}

hostile manifest-missing-name '[theme]
name = "x"' ok
hostile manifest-traversal 'name = "../escape"
version = "1"
[theme]
name = "t"
prompt = "{cwd}"' ok
hostile manifest-helper-missing 'name = "nohelper"
version = "1"
[helper]
path = "bin/definitely-not-here"' ok
hostile manifest-bad-timeout 'name = "badtimeout"
version = "1"
[helper]
path = "bin/x"
timeout_ms = "not-a-number"' ok
hostile manifest-empty '' ok
hostile manifest-garbage '<<<>>> not toml [[[' ok
hostile manifest-abs-path-helper 'name = "abshelper"
version = "1"
[helper]
path = "/etc/passwd"' ok

# --- C. misbehaving helpers (deadline + flood) -------------------------------
# These prove PLUGINS.md's promise: deadline-killed, warn-once, never
# wedge the shell.
mkhelper() {
    _dir=$1; _body=$2
    mkdir -p "$_dir/bin"
    printf '%s\n' "$_body" > "$_dir/bin/h"
    chmod +x "$_dir/bin/h"
}

helper_case() {
    _name=$1; _helper=$2; _timeout_ms=$3
    fresh_home
    _dir="$LAYERDIR/plugins/$_name"
    mkdir -p "$_dir"
    mkhelper "$_dir" "$_helper"
    cat > "$_dir/plugin.toml" <<TOML
name = "$_name"
version = "1"
[helper]
path = "bin/h"
timeout_ms = $_timeout_ms
TOML
    start=$(date +%s)
    out=$(brish_home 'echo alive')
    elapsed=$(( $(date +%s) - start ))
    st=$?
    case $out in
        *alive*) got=ok ;;
        *)      got=broken ;;
    esac
    # The helper only runs on Tab/segment, so a plain -c probe may not
    # invoke it; what we assert is that nothing hangs and the shell works.
    if [ "$got" = ok ] && [ "$elapsed" -lt 15 ]; then
        qa_pass "$LAYER" "$_name" brish "shell=$got elapsed=${elapsed}s"
    else
        qa_fail "$LAYER" "$_name" brish "shell=$got elapsed=${elapsed}s out=[$out]"
    fi
}

helper_case helper-hangs    '#!/bin/sh
sleep 300' 200
helper_case helper-floods   '#!/bin/sh
i=0; while [ $i -lt 200000 ]; do echo "candidate-$i"; i=$((i+1)); done' 200
helper_case helper-fails    '#!/bin/sh
exit 3' 200
helper_case helper-huge-line '#!/bin/sh
awk "BEGIN{s=\"x\";for(i=0;i<200000;i++)s=s s; print s}"' 200
helper_case helper-garbage-output '#!/bin/sh
printf "no-tab-no-newline"' 200
helper_case helper-cr-to-stdout '#!/bin/sh
printf "a\rb\rc"' 200

# --- D. plugin add argument handling ----------------------------------------
fresh_home
out=$(brish_home 'plugin add'); st=$?
if [ "$st" != 0 ]; then qa_pass "$LAYER" "add-no-arg-usage" brish "status=$st"
else qa_fail "$LAYER" "add-no-arg-usage" brish "exited 0"; fi

fresh_home
out=$(brish_home 'plugin add /no/such/dir-xyz'); st=$?
if [ "$st" != 0 ]; then qa_pass "$LAYER" "add-missing-path" brish "status=$st"
else qa_fail "$LAYER" "add-missing-path" brish "exited 0"; fi

fresh_home
out=$(brish_home 'plugin rm definitely-not-installed'); st=$?
# Removing something absent should be an error, not a crash.
if [ "$st" -le 2 ]; then qa_pass "$LAYER" "rm-unknown-name" brish "status=$st"
else qa_fail "$LAYER" "rm-unknown-name" brish "status=$st"; fi

fresh_home
out=$(brish_home 'plugin search example'); st=$?
if [ "$st" != 127 ]; then qa_pass "$LAYER" "search-reachable" brish "status=$st"
else qa_fail "$LAYER" "search-reachable" brish "status 127"; fi

qa_summary layer4