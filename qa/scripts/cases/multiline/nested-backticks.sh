# Nested command substitution. QA finding: briSH did not evaluate the
# inner backtick, leaving literal text; dash evaluates both.
echo 1: $(echo $(echo nested))
echo 2: `echo outer`
x=`echo \`echo inner\``
echo 3: $x
echo 4: $( (echo paren) )
