#!/bin/sh
# Layer 2 — feature + use-case matrix.
#
# Every documented behaviour gets a row: builtin, expansion, redirection,
# control flow, function, trap, history, plugin, config gate, error style,
# CLI flag. A feature with no row here is a feature nobody is checking.
#
# Case format (TAB-separated, 4 fields):
#   name <TAB> expected_status <TAB> expected_stdout_substring <TAB> script
#
# expected_stdout_substring of "." means "don't check stdout" — for
# features whose output is inherently variable (paths, jobs, history).
#
# These are NOT differential: they assert brish against what
# docs/CONFIGURATION.md and docs/PLUGINS.md say it does. Differential
# POSIX checking is layer 1b's job; this layer catches brish being
# *self-inconsistent* with its own documentation.
set -u
. /qa/scripts/lib.sh

CASES=${1:-/qa/scripts/cases/features.txt}
LAYER=features
FIELD=$(printf '\t')

qa_section "layer 2: feature matrix"
qa_init

n=0; npass=0
while IFS="$FIELD" read -r name want_st want_out script; do
    case "${name:-}" in ''|'#'*) continue ;; esac
    n=$((n + 1))
    f="$SCRATCH/feat.$n.sh"
    printf '%s\n' "$script" > "$f"
    wd="$SCRATCH/feat.$n"
    run_shell brish "$f" "$wd" 15
    got_st=$(st_of "$wd")
    got_out=$(out_of "$wd")

    if [ "$got_st" != "$want_st" ]; then
        qa_fail "$LAYER" "$name" brish \
            "status want=$want_st got=$got_st err=$(err_of "$wd" | head -1)"
        continue
    fi
    if [ "$want_out" != "." ] && [ -n "$want_out" ]; then
        case $got_out in
            *"$want_out"*) ;;
            *) qa_fail "$LAYER" "$name" brish \
                   "stdout missing [$want_out] got=[$got_out]"; continue ;;
        esac
    fi
    qa_pass "$LAYER" "$name" brish
    npass=$((npass + 1))
done < "$CASES"

printf 'feature cases: %s\n' "$n"
qa_summary layer2