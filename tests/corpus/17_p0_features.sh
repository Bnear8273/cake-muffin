# P0 features: local, declare/typeset, ${!name}, ${var@Q}, [[ =~ ]],
# pushd/popd/dirs, getopts, ** globstar, [[:alpha:]] POSIX char classes.
# All filesystem work happens inside a per-run temp dir; we `cd` in so that
# stdout never contains the `$$`-suffixed path (which differs between runs).

# --- local ---
local_test() {
    local x=5
    echo "local-x=$x"
    local -r r=3
    echo "local-r=$r"
    x=99
    echo "local-mutated=$x"
}
local_test

# --- declare / typeset ---
declare -r ro_var=42
echo "ro=$ro_var"
typeset -i num=7
echo "num=$num"
num=8
echo "num2=$num"
declare -x exported_var=hello
echo "exported=$exported_var"
declare -i intvar=100
declare -p intvar
declare -ir rigid=50
declare -p rigid
declare -ix xport=200
declare -p xport

# --- ${!name} indirect expansion ---
a=10
b=a
echo "indirect=${!b}"

# --- ${var@Q} quote escaping ---
s="hello world"
echo "q1=${s@Q}"
t='it'"'"'s'
echo "q2=${t@Q}"

# --- [[ =~ ]] regex ---
[[ "hello world" =~ ^h.*d$ ]] && echo re-yes || echo re-no
[[ "abc123" =~ [0-9]+ ]] && echo re-digit || echo re-nodigit
[[ "abc" =~ ^x ]] && echo re-bad || echo re-good
v="pattern123"
[[ "item123" =~ $v ]] && echo re-var || echo re-novar

# --- pushd / popd / dirs ---
rm -rf /tmp/muffin_corpus_p0
mkdir -p /tmp/muffin_corpus_p0/a /tmp/muffin_corpus_p0/b
cd /tmp/muffin_corpus_p0
pushd a >/dev/null
pwd
popd >/dev/null
pushd b >/dev/null
pwd
pushd a >/dev/null
pwd
dirs
popd >/dev/null
pwd
popd >/dev/null
pwd
cd /
rm -rf /tmp/muffin_corpus_p0

# --- getopts ---
while getopts "ab:" opt -a -b val1; do
    case $opt in
        a) echo "got-a" ;;
        b) echo "got-b=$OPTARG" ;;
        ?) echo "got-unknown" ;;
    esac
done

# --- globstar ** ---
cd /tmp/muffin-corpus-p0-$$
mkdir -p glob/sub/deep
touch glob/top.txt glob/sub/mid.txt glob/sub/deep/bot.txt
shopt -s globstar
echo "gs=$(echo glob/**/*.txt | sort | tr ' ' ' ')"
shopt -u globstar
echo "gs-off=$(echo glob/**/*.txt)"

# --- [[:alpha:]] POSIX char classes ---
touch class_a.txt class_1.txt CLASS_B.txt
echo "alpha=$(echo [[:alpha:]]*.txt | sort | tr ' ' ' ')"
echo "digit=$(echo [[:digit:]]*.txt | sort | tr ' ' ' ')"

cd /
rm -rf /tmp/muffin-corpus-p0-$$
