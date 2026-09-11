# set -e / set -u / pipefail / shopt / trap
# Each `set -e` scenario runs in a subshell so the file continues.

( set -e; true && false; echo no1 )
echo a1
( set -e; false && true; echo ok2 )
echo a2
( set -e; if false; then :; fi; echo ok3 )
echo a3
( set -e; ! true; echo ok4 )
echo a4
( set -e; false; echo no5 )
echo a5
( set -e; f() { false; }; f && echo ok6 )
echo a6
( set -e; g() { false; }; g && echo ok7 )
echo a7
( set -e; true | false; echo no8 )
echo a8
( set -e; false | true; echo ok9 )
echo a9

( set -u; echo $unset_var )
echo b1
( set -u; echo ${def-} )
echo b2
( set -u; echo $@ )
echo b3
( set -u; echo ${#unset_var} )
echo b4
( set -u; arr=(x); echo ${arr[9]} )
echo b5

set -o pipefail
false | true
echo "p1=$?"
set +o pipefail
false | true
echo "p2=$?"

mkdir -p /tmp/muffin-corpus-glob-$$
cd /tmp/muffin-corpus-glob-$$
rm -f .hidden plain.txt
touch .hidden plain.txt
echo "g1=$(echo *)"
shopt -s dotglob
echo "g2=$(echo *)"
shopt -u dotglob
echo "g3=$(echo *)"
shopt -s nullglob
echo "g4=[$(echo *.zzz)]"
shopt -u nullglob
echo "g5=[$(echo *.zzz)]"
cd /tmp
rm -rf /tmp/muffin-corpus-glob-$$

trap 'echo bye' EXIT
echo before-exit
