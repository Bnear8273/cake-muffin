# New bash-compat features: C-style for loops, file-test operators
# (-nt -ot -ef -O -G -N -S -b -c -p -u -g -k -h), and arithmetic
# postfix ++/--. Filesystem work happens in a per-run temp dir.

rm -rf /tmp/muffin-corpus-new-$$
mkdir -p /tmp/muffin-corpus-new-$$
cd /tmp/muffin-corpus-new-$$

# --- C-style for loop ---
sum=0
for ((i=1; i<=5; i++)); do
  sum=$((sum+i))
done
echo "sum=$sum"

# prefix increment and multiple vars
out=""
for ((j=0; j<3; ++j)); do
  out="$out $j"
done
echo "pre=$out"

# decrement step
out=""
for ((k=10; k>0; k-=4)); do
  out="$out $k"
done
echo "step=$out"

# empty condition = infinite; broken via break
n=0
for ((;;)); do
  n=$((n+1))
  if [ $n -ge 3 ]; then
    break
  fi
done
echo "inf=$n"

# continue skips to increment
out=""
for ((m=0; m<5; m++)); do
  if [ $((m % 2)) -eq 0 ]; then
    continue
  fi
  out="$out $m"
done
echo "cont=$out"

# --- postfix ++/-- in arithmetic ---
a=5
((a++))
echo "post-a=$a"
((a--))
echo "post-b=$a"

# --- file test operators ---
touch older.txt newer.txt
touch -t 202001010000 older.txt
touch -t 202101010000 newer.txt
# -nt / -ot
if [ newer.txt -nt older.txt ]; then echo nt-yes; else echo nt-no; fi
if [ older.txt -ot newer.txt ]; then echo ot-yes; else echo ot-no; fi
# -ef (hard link identity)
ln newer.txt hardlink.txt
if [ newer.txt -ef hardlink.txt ]; then echo ef-yes; else echo ef-no; fi
if [ newer.txt -ef older.txt ]; then echo ef-bad; else echo ef-ok; fi
# -O / -G ownership
if [ -O newer.txt ]; then echo O-yes; else echo O-no; fi
if [ -G newer.txt ]; then echo G-yes; else echo G-no; fi
# -N (modified since read) / -u -g -k
if [ -N newer.txt ]; then echo N-yes; else echo N-no; fi
# -h symlink
ln -s newer.txt sym.txt
if [ -h sym.txt ]; then echo h-yes; else echo h-no; fi
if [ -L sym.txt ]; then echo L-yes; else echo L-no; fi
# -b -c -p -S device/pipe/socket types (expect -p true on a fifo)
mkfifo pipe.fifo
if [ -p pipe.fifo ]; then echo p-yes; else echo p-no; fi
# [[ ]] forms too
[[ newer.txt -nt older.txt ]] && echo db-nt || echo db-nt-no
[[ -O newer.txt ]] && echo db-O || echo db-O-no

# cleanup
cd /
rm -rf /tmp/muffin-corpus-new-$$
