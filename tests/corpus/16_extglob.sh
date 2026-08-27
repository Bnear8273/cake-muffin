# extglob: ?(p) *(p) +(p) @(p) !(p)
# Bare extglob words are lexed as syntax errors by `bash -c` (extglob is a
# parse-time state), so glob scenarios run through `eval`; `[[ ]]` patterns
# are exempt and work directly.

shopt -s extglob
mkdir -p /tmp/cake-corpus-eg-$$
cd /tmp/cake-corpus-eg-$$
touch foo.txt bar.txt baz.log backup.txt

eval 'echo @(foo|bar).txt'
eval 'echo ?(foo|baz).txt'
eval 'echo *(foo|baz).txt'
eval 'echo +(foo|bar).txt'
eval 'echo !(foo)*.txt'
eval 'echo @(foo|bar).z*'

[[ foobar == @(foo|bar)* ]] && echo dbl-yes
[[ x == !(a) ]] && echo dbl-neg
[[ foo == *(f|b)oo ]] && echo dbl-star
[[ foobar != @(foo|bar)* ]] && echo dbl-no
[[ abc == a?c ]] && echo dbl-q

eval 'case foobar in @(foo|bar)*) echo case-yes;; esac'
eval 'case bar in !(a|b)) echo case-no;; *) echo case-neg;; esac'

cd /tmp
rm -rf /tmp/cake-corpus-eg-$$

echo "x=$(< /etc/hostname)" 2>/dev/null || echo no-hostname
