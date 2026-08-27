echo "date: $(date +%Y)"
files=$(ls /etc | head -1)
echo "first: $files"
out=$(printf 'a\nb\n')
echo "[$out]"
