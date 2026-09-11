echo file_{1,2,3}.txt
echo {a..c}
mkdir -p /tmp/muffin_brace && cd /tmp/muffin_brace
touch x{1..3}
echo x{1..3}
cd / && rmdir /tmp/muffin_brace
