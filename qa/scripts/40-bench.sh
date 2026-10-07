#!/bin/sh
# Benchmark orchestrator. Generates workloads, runs them under every
# oracle, aggregates medians, applies the loose gate.
#
# NOT imported by the other layers; run standalone:
#   sh /qa/scripts/40-bench.sh
set -u
. /qa/scripts/lib.sh

BENCH_OUT="$OUT/bench"
LAYER=bench

# Shells benchmarked. busybox is included deliberately: it and mksh are
# the lean implementations, so comparing against them separates "brish is
# slow" from "Rust is fast" (dash/bash are C shells carrying a decade of
# accumulated extensions).
BENCH_SHELLS="dash mksh busybox bash brish"
REPEATS=${BENCH_REPEATS:-15}
WARMUP=${BENCH_WARMUP:-3}

qa_section "benchmarks"
qa_init
mkdir -p "$BENCH_OUT"

# Pin to one CPU so the numbers mean something on a shared box. If
# taskset is unavailable we still run, just unpinned (recorded in output).
PIN=""
if command -v taskset >/dev/null 2>&1; then
    PIN="taskset -c 0"
fi

# time_ms <shell> <script> -> milliseconds, best of N
# Uses date +%s%N (GNU date). Reports the MINIMUM across repeats, which
# is the least noisy statistic on a shared machine — p50/p95 of a noisy
# sample mostly measures the neighbours.
time_ms() {
    _sh=$1; _script=$2
    _cmd=$(qa_shell_cmd "$_sh")
    _best=""
    _r=0
    while [ "$_r" -lt "$REPEATS" ]; do
        _wd="$SCRATCH/t"; rm -rf "$_wd"; mkdir -p "$_wd"
        _start=$(date +%s%N)
        ( cd "$_wd" && env -i PATH="$QA_PATH" HOME="$_wd" LC_ALL=C \
            $PIN timeout 60 sh -c "$_cmd \"\$1\"" qa "$_script" \
            >/dev/null 2>&1 </dev/null )
        _end=$(date +%s%N)
        _ms=$(( (_end - _start) / 1000000 ))
        if [ -z "$_best" ] || [ "$_ms" -lt "$_best" ]; then
            _best=$_ms
        fi
        _r=$((_r + 1))
    done
    echo "$_best"
}

# bench <name> <script-file>
# Emits one TSV row per shell into $BENCH_OUT/results.tsv
bench() {
    _name=$1; _script=$2
    _row=""
    for sh in $BENCH_SHELLS; do
        # warmup
        _w=0
        while [ "$_w" -lt "$WARMUP" ]; do
            _cmd=$(qa_shell_cmd "$sh")
            _wd="$SCRATCH/w"; rm -rf "$_wd"; mkdir -p "$_wd"
            ( cd "$_wd" && env -i PATH="$QA_PATH" HOME="$_wd" LC_ALL=C \
                $PIN timeout 60 sh -c "$_cmd \"\$1\"" qa "$_script" \
                >/dev/null 2>&1 </dev/null )
            _w=$((_w + 1))
        done
        ms=$(time_ms "$sh" "$_script")
        _row="$_row $sh=$ms"
        printf '%s\t%s\t%s\n' "$_name" "$sh" "$ms" >> "$BENCH_OUT/results.tsv"
    done
    printf '  %-26s%s\n' "$_name" "$_row"
}

# ---- workload generation ----------------------------------------------------
printf 'shells: %s\nrepeats: %s (min of N)\npinned: %s\n\n' \
    "$BENCH_SHELLS" "$REPEATS" "${PIN:-no}"

# run_wl <name> <command...> — the command writes the workload script to
# stdout. Workloads are generated rather than checked in so their size can
# be tuned without editing a corpus file.
run_wl() {
    _n=$1
    shift
    _f="$BENCH_OUT/$_n.sh"
    "$@" > "$_f"
    bench "$_n" "$_f"
}

run_wl startup    printf ':\n'
run_wl small      printf 'x=1\necho $x\n'
run_wl builtins   printf 'i=0\nwhile [ $i -lt 2000 ]; do i=$((i+1)); done\n'
run_wl expansion  printf 'i=0\nwhile [ $i -lt 500 ]; do x="a b c d"; set -- $x; y="${x#a}${x%c}${#x}"; i=$((i+1)); done\n'
run_wl arith      printf 'i=0\nr=0\nwhile [ $i -lt 500 ]; do r=$((r+i*2-1)); i=$((i+1)); done\necho $r\n'
run_wl pipeline   printf 'i=0\nwhile [ $i -lt 60 ]; do printf "x\\n" | cat | wc -l >/dev/null; i=$((i+1)); done\n'
run_wl subshell   printf 'i=0\nwhile [ $i -lt 300 ]; do (x=$i); i=$((i+1)); done\n'
run_wl function   printf 'f() { :; }\ni=0\nwhile [ $i -lt 2000 ]; do f; i=$((i+1)); done\n'
run_wl glob       printf 'i=0\nwhile [ $i -lt 300 ]; do : > "f$i"; set -- f*; i=$((i+1)); done\nrm -f f*\n'
run_wl fork       printf 'i=0\nwhile [ $i -lt 200 ]; do /bin/true; i=$((i+1)); done\n'
run_wl case       printf 'i=0\nwhile [ $i -lt 500 ]; do case $i in 1) :;; *) :;; esac; i=$((i+1)); done\n'
run_wl parse_big  sh -c 'for i in $(seq 1 2000); do echo "x$i=1"; done'

# ---- loose gate -------------------------------------------------------------
# Fail only when brish is more than GATE x slower than the SLOWEST oracle
# on a workload. Tuning for "no slower than dash" is not realistic for a
# shell with autosuggest/highlighting and would make the gate noise.
GATE=${BENCH_GATE:-2}
printf '\n--- gate: brish median-min must be <= %sx slowest oracle ---\n' "$GATE"

gate_fail=0
awk -F'\t' -v gate="$GATE" '
  { t[$1][$2] = $3 }
  END {
    for (w in t) {
      brish = t[w]["brish"] + 0
      slowest = 0
      for (s in t[w]) {
        if (s == "brish") continue
        v = t[w][s] + 0
        if (v > slowest) slowest = v
      }
      if (brish > gate * slowest) {
        printf "GATE-FAIL\t%s\tbrish=%dms slowest-oracle=%dms\n", w, brish, slowest
      }
    }
  }
' "$BENCH_OUT/results.tsv" | tee "$BENCH_OUT/gate.txt"

if [ -s "$BENCH_OUT/gate.txt" ]; then
    gate_fail=1
fi
printf '\nbench results: %s\n' "$BENCH_OUT/results.tsv"
exit "$gate_fail"