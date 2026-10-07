#!/bin/sh
# Benchmark: interactive REPL latency.
#
# The only layer that needs a pty. Two numbers matter and neither is
# reachable any other way:
#
#   first-prompt   time from process spawn to the prompt appearing
#   render         time from a keystroke to the redrawn line
#
# Measured through 50-pty.py's --measure, which timestamps the first
# appearance of a sentinel in the pty stream. Reported as medians of
# BENCH_REPEATS runs. No gate: interactive latency inside a container is
# dominated by the container, not the shell. The value is comparative
# across shells measured in the same run.
set -u
. /qa/scripts/lib.sh

LAYER=bench-interactive
BENCH_OUT="$OUT/bench"
REPEATS=${BENCH_REPEATS:-7}

qa_section "benchmarks: interactive"
qa_init
mkdir -p "$BENCH_OUT"

PTY=/qa/scripts/50-pty.py
if ! command -v python3 >/dev/null 2>&1; then
    qa_skip "$LAYER" "pty" env "no python3"
    qa_summary layer-bench-interactive
    exit 0
fi

SHELLS="brish bash dash mksh"

median_of() {
    sort -n | awk '{v[NR]=$1} END {if(NR==0){print "-";exit} print v[int((NR+1)/2)]}'
}

# latency_firstout <shell> — median seconds until the shell emits its
# first output byte. Deliberately not "time until the prompt glyphs
# appear": prompt rendering under a bare pty is not faithful, but the
# shell becoming live is exactly the signal we want, and it is
# comparable across shells measured in the same run.
latency_firstout() {
    _shell=$1
    _vals=""
    _r=0
    while [ "$_r" -lt "$REPEATS" ]; do
        _v=$("$PTY" --shell "$_shell" --settle 0.25 --timeout 8 --first-output \
                 --teardown "" 2>&1 >/dev/null \
             | sed -n 's/^LATENCY=//p')
        [ "$_v" = "none" ] && _v=""
        [ -n "$_v" ] && _vals="$_vals$_v
"
        _r=$((_r + 1))
    done
    printf '%s' "$_vals" | median_of
}

printf '\n%-34s' "metric"
for sh in $SHELLS; do printf '%10s' "$sh"; done
printf '   (ms, median of %s)\n' "$REPEATS"

# seconds -> ms for the table
to_ms() { awk -v s="$1" 'BEGIN{printf "%d", (s=="-"?0:s*1000)}'; }

printf '%-34s' "first-output (spawn -> live)"
for sh in $SHELLS; do
    s=$(latency_firstout "$sh")
    m=$(to_ms "$s")
    printf '%10s' "${s:--}"
    printf '%s\t%s\tfirst-output\t%s\n' "$LAYER" "$sh" "$m" \
        >> "$BENCH_OUT/interactive.tsv"
done
printf '\n'

printf '\ninteractive results: %s\n' "$BENCH_OUT/interactive.tsv"
qa_pass "$LAYER" "interactive-measurements" env "collected (no gate; comparative only)"
qa_summary layer-bench-interactive