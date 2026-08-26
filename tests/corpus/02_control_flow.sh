for i in 1 2 3; do
  if [ $((i % 2)) -eq 0 ]; then
    echo "even: $i"
  else
    echo "odd: $i"
  fi
done
n=0
while [ $n -lt 3 ]; do
  echo "n=$n"
  n=$((n+1))
done
