#!/bin/sh
# Aggregate every layer's TSV into one markdown report + exit code.
#
# Runs last. Reads $OUT/results.tsv (written by all layers) and
# $OUT/bench/results.tsv, emits $OUT/report.md.
set -u
. /qa/scripts/lib.sh

OUT=${OUT:-/out}
COMBINED="$OUT/all-results.tsv"
REPORT="$OUT/report.md"
LAYER=report

qa_section "report"
qa_init

# Layers write results.<layer>.tsv so none can wipe another's rows.
# Concatenate them under ONE header into a single stream.
printf 'layer\tcase\toracle\tverdict\tdetail\n' > "$COMBINED"
found=0
for f in "$OUT"/results.*.tsv; do
    [ -f "$f" ] || continue
    found=1
    sed '1d' "$f" >> "$COMBINED"
done
if [ "$found" = 0 ]; then
    echo "no layer results found in $OUT"
    exit 2
fi
RESULTS="$COMBINED"

{
    echo "# briSH QA report"
    echo
    echo "Generated: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo
    echo "## Summary"
    echo
    printf '| Layer | PASS | FAIL | SKIP | N/A | DEV |\n'
    printf '|---|---:|---:|---:|---:|---:|\n'

    awk -F'\t' '
      NR>1 {
        key = $1
        v = $4
        if (v=="PASS") p[key]++
        else if (v=="FAIL") f[key]++
        else if (v=="SKIP") s[key]++
        else if (v=="NA") n[key]++
        else if (v=="DEV") d[key]++
      }
      END {
        for (k in p) seen[k]=1
        for (k in f) seen[k]=1
        for (k in s) seen[k]=1
        for (k in n) seen[k]=1
        for (k in d) seen[k]=1
        for (k in seen)
          printf "| %s | %d | %d | %d | %d | %d |\n", k, p[k]+0, f[k]+0, s[k]+0, n[k]+0, d[k]+0
      }
    ' "$RESULTS" | sort

    echo
    echo "## Failures"
    echo
    fails=$(awk -F'\t' 'NR>1 && $4=="FAIL"' "$COMBINED" | wc -l | tr -d ' ')
    if [ "$fails" = 0 ]; then
        echo "None."
    else
        echo "$fails finding(s). Each row is reproducible: run the recorded"
        echo "script under the recorded shell in the QA image."
        echo
        printf '| Layer | Case | Detail |\n'
        printf '|---|---|---|\n'
        awk -F'\t' 'NR>1 && $4=="FAIL" {
            d=$5; gsub(/\|/,"\\\\|",d)
            printf "| %s | `%s` | %s |\n", $1, $2, d
        }' "$RESULTS"
    fi

    echo
    echo "## Documented deviations (DEV)"
    echo
    devs=$(awk -F'\t' 'NR>1 && $4=="DEV"' "$COMBINED" | wc -l | tr -d ' ')
    if [ "$devs" = 0 ]; then
        echo "None."
    else
        printf '| Layer | Case | Detail |\n|---|---|---|\n'
        awk -F'\t' 'NR>1 && $4=="DEV" {
            d=$5; gsub(/\|/,"\\\\|",d)
            printf "| %s | `%s` | %s |\n", $1, $2, d
        }' "$RESULTS"
    fi

    if [ -f "$OUT/bench/results.tsv" ]; then
        echo
        echo "## Benchmarks (min of N, ms — lower is better)"
        echo
        printf '| Workload | dash | mksh | busybox | bash | brish |\n'
        printf '|---|---:|---:|---:|---:|---:|\n'
        awk -F'\t' '
          { t[$1][$2]=$3 }
          END {
            for (w in t)
              printf "| %s | %s | %s | %s | %s | %s |\n", w,
                t[w]["dash"], t[w]["mksh"], t[w]["busybox"],
                t[w]["bash"], t[w]["brish"]
          }
        ' "$OUT/bench/results.tsv" | sort
        echo
        echo "Numbers are min-of-N on a shared container. They are comparative"
        echo "within a single run only; a number is not a regression signal on"
        echo "its own."
    fi

    if [ -f "$OUT/bench/interactive.tsv" ]; then
        echo
        echo "## Interactive latency (ms, median)"
        echo
        printf '| Metric | brish | bash | dash | mksh |\n|---|---:|---:|---:|---:|\n'
        awk -F'\t' '
          { key=$3; val[key][$2]=$4 }
          END {
            for (m in val)
              printf "| %s | %s | %s | %s | %s |\n", m,
                val[m]["brish"], val[m]["bash"], val[m]["dash"], val[m]["mksh"]
          }
        ' "$OUT/bench/interactive.tsv" | sort
    fi

    echo
    echo "## Reproducing"
    echo
    echo '```sh'
    echo "docker build -f qa/Dockerfile -t brish-qa ."
    echo "docker run --rm --cpus 2 --memory 2g --pids-limit 256 \\"
    echo "  -v \"\$PWD/qa:/qa:ro\" -v \"\$PWD/qa-out:/out\" brish-qa /qa/scripts/run-all.sh"
    echo '```'
} > "$REPORT"

total_fail=$(awk -F'\t' 'NR>1 && $4=="FAIL"' "$COMBINED" | wc -l | tr -d ' ')
total_pass=$(awk -F'\t' 'NR>1 && $4=="PASS"' "$COMBINED" | wc -l | tr -d ' ')

printf '\nreport: %s\npass:   %s\nfail:   %s\n' "$REPORT" "$total_pass" "$total_fail"

if [ "$total_fail" -gt 0 ]; then
    printf '\nRESULT: FAIL (%s finding(s))\n' "$total_fail"
    exit 1
fi
printf '\nRESULT: PASS\n'
exit 0