# && / || lists broken across lines, with continuations.
false &&
  echo skipped ||
  echo ran
true ||
  echo skipped &&
  echo also
if false &&
   echo no; then
  echo unreachable
fi
