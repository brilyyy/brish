# Deeply nested compound commands + redirection scoping.
i=0
for a in 1 2; do
  for b in x y; do
    if [ "$a" = 1 ]; then
      while [ $i -lt 2 ]; do
        case $b in
          x) i=$((i+1)); echo "ax$i" ;;
          y) i=$((i+1)); echo "ay$i" ;;
        esac
      done
    else
      echo "skip$a$b"
    fi
  done
done
