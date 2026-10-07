# Nested command substitution and backticks.
a=$(echo $(echo $(echo deep)))
echo "$a"
b=`echo \`echo tick\``
echo "$b"
echo $( (echo paren) )
v=$(printf 'l1\nl2\n')
echo "$(echo "$v" | wc -l | tr -d ' ')"
