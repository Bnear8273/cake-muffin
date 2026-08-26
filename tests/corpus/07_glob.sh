# cake:xfail
cd /tmp
touch cake_glob_aaa cake_glob_bbb
echo cake_glob_*
rm -f cake_glob_aaa cake_glob_bbb
echo ??.txt
