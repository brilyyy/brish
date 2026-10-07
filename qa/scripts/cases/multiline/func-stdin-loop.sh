# Real-world Debian shape: loop reading stdin, the single most common
# construct in /etc/init.d and update-* scripts.
printf 'one\ntwo\nthree\n' | while IFS= read -r line; do
  printf '[%s]\n' "$line"
done
