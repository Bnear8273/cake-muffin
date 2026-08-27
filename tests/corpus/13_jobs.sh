# job control basics: jobs / wait / $! / fg / bg
# Note: no raw pids in output (they differ between shells).

sleep 0.2 & jobs
sleep 0.1 & sleep 0.15; jobs
false & p=$!; wait $p
echo "w1=$?"
sleep 0.1 & wait
echo "w2=$?"
sleep 0.1 & fg
echo "f=$?"
sleep 0.1 & bg
echo "b=$?"
