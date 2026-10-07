#!/bin/sh
# Layer 1a — parse sweep: can briSH parse the shell scripts a real Debian
# install ships?
#
# Parse-only gate: each script is prefixed with `set -n` (noexec) and
# run. briSH parses the whole file before executing anything, so a syntax
# error surfaces as exit 2 + a stderr message while a clean script exits 0
# with no side effects. That is what makes sweeping real scripts safe:
# `apt` config hooks and `update-*` scripts would otherwise reconfigure
# the container.
#
# A parse failure is a *finding*, not automatically a bug — bash-isms and
# zsh-isms are supposed to be rejected. The classifier in report-time
# buckets them.
#
# Run inside the QA image:  sh qa/sweep-parse.sh <outdir>
set -u

OUT=${1:-/tmp/qa-parse}
mkdir -p "$OUT"
: > "$OUT/parse-fail.tsv"
: > "$OUT/clean.tsv"
: > "$OUT/skip.tsv"

# Candidate corpus: executable text files carrying a shebang.
find /usr/bin /usr/sbin /bin /sbin /etc/profile.d /etc/init.d \
     -type f -perm -u+x 2>/dev/null \
  | while IFS= read -r f; do
      head -1 "$f" 2>/dev/null | grep -q '^#!' && printf '%s\n' "$f"
    done \
  | sort -u > "$OUT/candidates.list"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

total=0
clean=0
failed=0
while IFS= read -r f; do
  total=$((total + 1))
  sb=$(head -1 "$f" 2>/dev/null | tr -d '\r')
  # Only #!/bin/sh and #!/bin/bash are POSIX-or-close targets. #!/bin/zsh,
  # #!/usr/bin/perl and friends are out of scope by construction.
  case $sb in
    *'/bin/sh'*|*'/bin/bash'*|*'/bin/dash'*) ;;
    *) continue ;;
  esac

  { printf 'set -n\n'; cat "$f"; } > "$TMP/probe.sh" 2>/dev/null || continue

  # Reference first: if dash cannot parse the file either, the script is
  # simply not a parse-gate candidate (truncated quote, deliberate syntax
  # error, non-shell content). Reporting briSH for those is noise.
  if ! dash -n "$f" >/dev/null 2>&1; then
    printf '%s\t%s\tdash-cannot-parse-either\n' "$f" "$sb" >> "$OUT/skip.tsv"
    continue
  fi

  if err=$(cd "$TMP" && timeout 10 brish "$TMP/probe.sh" 2>&1 >/dev/null); then
    clean=$((clean + 1))
    printf '%s\t%s\n' "$f" "$sb" >> "$OUT/clean.tsv"
  else
    st=$?
    reason=$(printf '%s' "$err" | grep -i 'parse error\|incomplete\|unexpected\|expected' \
             | head -1 | cut -c1-100)
    [ -n "$reason" ] || reason=$(printf '%s' "$err" | head -1 | cut -c1-100)
    failed=$((failed + 1))
    printf '%s\t%s\t%s\t%s\n' "$f" "$sb" "$st" "$reason" >> "$OUT/parse-fail.tsv"
  fi
done < "$OUT/candidates.list"

skipped=$(wc -l < "$OUT/skip.tsv" | tr -d ' ')
printf 'sh/bash scripts seen:  %s\n' "$total"
printf 'skipped (dash also fails): %s\n' "$skipped"
printf 'parsed clean:        %s\n' "$clean"
printf 'parse failures:      %s\n' "$failed"
printf 'wrote:               %s\n' "$OUT"