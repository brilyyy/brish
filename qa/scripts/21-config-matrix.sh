#!/bin/sh
# Layer 3 — config.toml matrix.
#
# The documented contract (docs/CONFIGURATION.md) is: "Unknown keys/values
# warn and fall back to defaults; the shell never refuses to start over
# config." Every row here drives one key to default / valid / malformed /
# unknown and asserts the shell still starts and still runs.
#
# A config parser that exits non-zero, hangs, or silently applies a
# malformed value is a finding even though the shell "starts".
set -u
. /qa/scripts/lib.sh

LAYER=config

qa_section "layer 3: config matrix"
qa_init

# Depends on $SCRATCH, so it must be built after qa_init.
CFGDIR="$SCRATCH/cfghome/.config/brish"

# write_cfg <body> -> installs a config.toml, returns cwd to run from
write_cfg() {
    mkdir -p "$CFGDIR"
    printf '%s\n' "$1" > "$CFGDIR/config.toml"
}

# probe <name> <cfg-body> <probe-script>
# Asserts: shell starts (exit 0 on a trivial probe) and the probe works.
probe() {
    _name=$1; _cfg=$2; _script=$3
    write_cfg "$_cfg"
    f="$SCRATCH/cfg.$$.sh"
    printf '%s\n' "$_script" > "$f"
    wd="$SCRATCH/cfgwd.$$"
    rm -rf "$wd"; mkdir -p "$wd"
    # HOME points at the scratch config tree so brish reads our config.
    ( cd "$wd" && env -i PATH="$QA_PATH" HOME="$SCRATCH/cfghome" \
        LC_ALL=C timeout 15 brish "$f" >"$wd/out" 2>"$wd/err" </dev/null )
    echo $? > "$wd/st"
    st=$(cat "$wd/st")
    if [ "$st" = 0 ]; then
        qa_pass "$LAYER" "$_name" brish
    else
        qa_fail "$LAYER" "$_name" brish \
            "status=$st err=$(head -2 "$wd/err" | tr '\n' ' ')"
    fi
    rm -rf "$CFGDIR"
}

PROBE='echo alive'

# --- baseline: no config file at all -----------------------------------------
rm -rf "$SCRATCH/cfghome"
probe no-config-file "" "$PROBE"

# --- theme -------------------------------------------------------------------
probe theme-valid '[theme]
name = "briiish-plain"' "$PROBE"
probe theme-unknown '[theme]
name = "no-such-theme-xyz"' "$PROBE"
probe theme-wrong-type '[theme]
name = 42' "$PROBE"

# --- errors.style ------------------------------------------------------------
probe errors-valid '[errors]
style = "plain"' "$PROBE"
probe errors-unknown '[errors]
style = "rainbow"' "$PROBE"

# --- completion --------------------------------------------------------------
probe completion-valid '[completion]
algorithm = "fuzzy"
sort = false
match_description = true' "$PROBE"
probe completion-bad-algorithm '[completion]
algorithm = "telepathic"' "$PROBE"

# --- prompt ------------------------------------------------------------------
probe prompt-valid '[prompt]
indicator = "$ "
vi_normal = "n"
multiline = "m "
completion_description = "cyan"' "$PROBE"
probe prompt-empty-indicator '[prompt]
indicator = ""' "$PROBE"

# --- plugins gate ------------------------------------------------------------
probe plugins-disabled '[plugins]
disabled = ["brish-git", "brish-completion"]' "$PROBE"
probe plugins-enabled-list '[plugins]
enabled = ["brish-themes"]' "$PROBE"
probe plugins-unknown-name '[plugins]
enabled = ["brish-does-not-exist"]' "$PROBE"

# --- store -------------------------------------------------------------------
probe store-valid '[store]
index = "https://example.invalid/repo.git"' "$PROBE"

# --- highlight / cd / ls / output --------------------------------------------
probe highlight-dynamic-off '[highlight]
dynamic = false' "$PROBE"
probe cd-zoxide-auto '[cd]
zoxide = "auto"' "$PROBE"
probe ls-backend-builtin '[ls]
backend = "builtin"
icons = false' "$PROBE"
probe output-table-off '[output]
table = "off"' "$PROBE"

# --- hooks -------------------------------------------------------------------
probe hooks-cnf '[hooks]
command_not_found = "echo advice"' "$PROBE"

# --- structurally malformed configs (must still start) -----------------------
probe cfg-malformed-toml 'this is not toml at all [[[' "$PROBE"
probe cfg-unclosed-bracket '[theme
name = "x"' "$PROBE"
probe cfg-wrong-type-section 'theme = "not-a-table"' "$PROBE"
probe cfg-empty '' "$PROBE"
probe cfg-only-comments '# just a comment' "$PROBE"
probe cfg-unknown-top-key 'totally_unknown_key = 1' "$PROBE"
probe cfg-duplicate-key '[theme]
name = "a"
name = "b"' "$PROBE"

qa_summary layer3