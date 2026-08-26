x=5
y="hello world"
echo "$x $y"
z="$x$y"
echo "${z}${x}"
unset x
echo "x is [${x}]"
a=one
b="two three"
echo "$a $b"
echo "quoted '$a'"
