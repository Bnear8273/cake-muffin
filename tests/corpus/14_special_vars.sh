# special dynamic variables: RANDOM / LINENO / SECONDS / PWD / OLDPWD

r=$RANDOM
[ $r -ge 0 ] && [ $r -le 32767 ] && echo random-ok
r2=$RANDOM
[ $r -ne $r2 ] && echo random-changes
echo $LINENO
echo $LINENO
echo $LINENO
sleep 1
[ $SECONDS -ge 1 ] && echo seconds-ok
cd /tmp
echo $PWD
echo $OLDPWD
