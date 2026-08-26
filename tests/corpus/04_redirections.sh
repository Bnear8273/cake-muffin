cat <<'MARKER'
heredoc line 1
heredoc line 2
MARKER
cat <<< herestring
printf 'to err\n' >&2
echo "out only"
