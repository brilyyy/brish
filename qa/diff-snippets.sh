#!/bin/sh
# Layer 1b — exit-status differential on hermetic snippets.
#
# Whole real scripts print paths, dates and pids, so their *stdout* is
# never comparable. What IS comparable is the exit status of a
# self-contained snippet: same input, same environment, no clock, no
# randomness, no filesystem outside a scratch dir.
#
# Every snippet runs under dash, bash and brish with an identical
# scrubbed environment. stdout AND status are compared separately —
# the bugs this QA has found were all status-only or silent, so a
# combined comparison would have hidden them.
#
# Status travels through a file, never through stdout: a snippet may
# print newlines, which would corrupt an inline separator.
#
# Run inside the QA image:  sh qa/diff-snippets.sh <cases-file> <outdir>
set -u

CASES=${1:?usage: diff-snippets.sh cases-file outdir}
OUT=${2:?usage: diff-snippets.sh cases-file outdir}
mkdir -p "$OUT"
: > "$OUT/results.tsv"
: > "$OUT/failures.tsv"

SCRATCH=$(mktemp -d)
trap 'rm -rf "$SCRATCH"' EXIT
seq_no=0

# Fixed environment. PATH must include /usr/local/bin: brish lives
# there, and a missing oracle would silently read as "127 = bug".
QPATH=/usr/local/bin:/usr/bin:/bin

# run_one <shell> <snippet-dir> -> stdout in <dir>/out, status in <dir>/st
run_one() {
  _shell=$1
  _d=$2
  printf '%s\n' "$SNIPPET" > "$_d/s.sh"
  ( cd "$_d" && env -i \
      PATH="$QPATH" \
      HOME="$_d" \
      TMPDIR="$_d" \
      LC_ALL=C \
      timeout 10 "$_shell" "$_d/s.sh" >"$_d/out" 2>"$_d/err" </dev/null )
  echo $? > "$_d/st"
}

total=0
pass=0
fail=0
while IFS= read -r SNIPPET; do
  [ -n "$SNIPPET" ] || continue
  case $SNIPPET in '#'*) continue ;; esac
  total=$((total + 1))
  seq_no=$((seq_no + 1))

  for sh in dash bash brish; do
    mkdir -p "$SCRATCH/$seq_no.$sh"
    run_one "$sh" "$SCRATCH/$seq_no.$sh"
  done

  d_st=$(cat "$SCRATCH/$seq_no.dash/st")
  b_st=$(cat "$SCRATCH/$seq_no.bash/st")
  r_st=$(cat "$SCRATCH/$seq_no.brish/st")
  # Compare stdout with the trailing newline trimmed: `echo` differences
  # there are noise, content differences are signal.
  d_out=$(cat "$SCRATCH/$seq_no.dash/out")
  r_out=$(cat "$SCRATCH/$seq_no.brish/out")

  status_ok=no; [ "$d_st" = "$r_st" ] && status_ok=yes
  stdout_ok=no; [ "$d_out" = "$r_out" ] && stdout_ok=yes
  bash_status=no; [ "$b_st" = "$r_st" ] && bash_status=yes

  verdict=PASS
  [ "$status_ok" = yes ] || verdict=STATUS
  if [ "$stdout_ok" = no ] && [ "$verdict" = PASS ]; then verdict=STDOUT; fi

  if [ "$verdict" = PASS ]; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
    printf '%s\t%s\tdash=%s brish=%s bash=%s\tdash_out=[%s] brish_out=[%s]\n' \
      "$verdict" "$SNIPPET" "$d_st" "$r_st" "$b_st" \
      "$d_out" "$r_out" >> "$OUT/failures.tsv"
  fi
  printf '%s\t%s\tdash=%s\tbrish=%s\tbash=%s\n' \
    "$verdict" "$SNIPPET" "$d_st" "$r_st" "$b_st" >> "$OUT/results.tsv"
done < "$CASES"

printf 'snippets: %s\npass:     %s\nfail:     %s\nwrote:    %s\n' \
  "$total" "$pass" "$fail" "$OUT/results.tsv"
exit 0