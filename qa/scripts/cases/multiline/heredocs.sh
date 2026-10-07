# Here-doc forms that cannot be expressed one-per-line.
# `<<-` strips leading TABS (not spaces) from body and delimiter.
cat <<A
plain body
A

cat <<'B'
literal $notexpanded `cmd`
B

cat <<"C"
double quoted: $also_literal
C

printf 'tab-stripped\n' > hd.txt
cat <<-D >> hd.txt
	indented with tab
	D
cat hd.txt
rm -f hd.txt

cat <<E > out.txt
redirected here-doc
E
cat out.txt
rm -f out.txt

# here-doc inside a compound command
if true; then
  cat <<F
in-if
F
fi

# here-doc body containing shell metacharacters
cat <<'G'
$(echo not-run) `echo not-run` ; | & > < * ? [a-z]
G