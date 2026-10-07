#!/bin/sh
# briSH QA suite entrypoint.
#
#   docker build -f qa/Dockerfile -t brish-qa .
#   docker run --rm --cpus 2 --memory 2g --pids-limit 256 \
#     -v "$PWD/qa:/qa:ro" -v "$PWD/qa-out:/out" brish-qa /qa/scripts/run-all.sh
#
# Runs every layer, each writing its own TSV into $OUT, then aggregates.
# Layers are independent: one failing does not stop the rest, because a
# harness bug in layer 3 must not hide a real bug in layer 5.
#
#   qa/scripts/run-all.sh              everything
#   qa/scripts/run-all.sh conformance  only layers whose name matches
set -u

SCRIPTS=/qa/scripts
OUT=${OUT:-/out}
FILTER=${1:-}

mkdir -p "$OUT"

LAYERS="00-env
10-conformance
11-parse-sweep
20-features
21-config-matrix
22-plugin-store
23-interactive
30-adversarial
40-bench
43-bench-interactive"

overall=0
for l in $LAYERS; do
    case $l in
        *$FILTER*) ;;
        *) continue ;;
    esac
    printf '\n##################################################\n'
    printf '## %s\n' "$l"
    printf '##################################################\n'
    # A failing layer must not stop the suite: a harness bug in one
    # layer should never hide a real bug in another.
    sh "$SCRIPTS/$l.sh" || true

done

printf '\n##################################################\n'
printf '## aggregating\n'
printf '##################################################\n'
sh "$SCRIPTS/60-report.sh"
rc=$?
[ "$rc" -ne 0 ] && overall=1

printf '\n==================================================\n'
if [ "$overall" -eq 0 ]; then
    printf 'SUITE RESULT: PASS\n'
else
    printf 'SUITE RESULT: FAIL (see %s/report.md)\n' "$OUT"
fi
printf '==================================================\n'
exit "$overall"