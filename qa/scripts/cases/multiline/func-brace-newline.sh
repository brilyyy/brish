# POSIX no_newlines forbids `{` on the next line, but it is the dominant
# Debian house style and both oracles accept it. Bug 3 in qa/report.md.
show_version()
{
  echo v1
}
show_version
