# process substitution <(cmd) / >(cmd)

cat <(echo hello world)
diff <(echo a; echo b) <(echo a; echo c)
grep o <(echo fox; echo dog)
cat <(echo $(echo nested))
cat < <(echo redir-target)
diff <(echo a) <(echo a) && echo same
echo "x<(/bin/echo literal)"
