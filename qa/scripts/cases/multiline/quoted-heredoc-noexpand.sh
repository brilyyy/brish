# Quoted delimiter (<<"EOF" or <<'EOF') must NOT expand (POSIX 2.7.4).
# QA finding: briSH expanded $var inside these, dash does not.
echo begin
cat <<"DOUBLE"
$var_should_not_expand
DOUBLE
cat <<'SINGLE'
$var_should_not_expand
SINGLE
echo end
