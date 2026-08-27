echo hello # trailing comment
x=1 # comment after assignment
echo $x
if [ -d /tmp ]; then echo dir-ok; fi
[ 3 -lt 5 ] && echo lt || echo ge
test -n "hello" && echo nonempty
test -z "" && echo empty
for i in 1 2 3 4 5; do
  [ $i -eq 3 ] && break
  echo "b$i"
done
for i in 1 2 3; do
  [ $i -eq 2 ] && continue
  echo "c$i"
done
sum() { echo "$1 + $2 = $(( $1 + $2 ))"; }
sum 3 4
arr=(alpha beta gamma)
echo "${arr[0]} ${arr[1]} ${arr[2]}"
echo "len=${#arr[@]}"
echo "all=${arr[*]}"
arr[1]=BETA
echo "${arr[@]}"
