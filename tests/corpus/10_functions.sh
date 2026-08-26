add() {
  echo $(( $1 + $2 ))
}
add 3 4
greet() {
  echo "Hello, $1!"
}
greet World
fib() {
  a=0
  b=1
  i=0
  while [ $i -lt $1 ]; do
    echo -n "$a "
    c=$((a + b))
    a=$b
    b=$c
    i=$((i + 1))
  done
  echo
}
fib 6
