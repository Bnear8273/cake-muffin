# cake:xfail
echo file_{1,2,3}.txt
echo {a..c}
mkdir -p /tmp/cake_brace && cd /tmp/cake_brace
touch x{1..3}
echo x{1..3}
cd / && rmdir /tmp/cake_brace
