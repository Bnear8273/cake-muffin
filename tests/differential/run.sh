#!/usr/bin/env bash
# Differential runner: run every corpus script through both `bash` and `cake`
# and report agreement on stdout + exit code.
#
# Markers in a corpus file (first-line comments):
#   # cake:xfail   expected to differ (feature not yet implemented)
#   # cake:skip    don't run this file
#
# Usage: tests/differential/run.sh [path-to-cake] [path-to-bash]
set -u

CAKE="${1:-target/debug/cake}"
BASH="${2:-bash}"
CORPUS_DIR="$(cd "$(dirname "$0")/../corpus" && pwd)"

pass=0
fail=0
xfail=0
skip=0
declare -a failures=()

for f in "$CORPUS_DIR"/*.sh; do
    name="$(basename "$f")"
    marker="$(grep -m1 -oE '# cake:(xfail|skip)' "$f" || true)"

    if [ "$marker" = "# cake:skip" ]; then
        skip=$((skip+1))
        printf 'SKIP  %s\n' "$name"
        continue
    fi

    src="$(cat "$f")"

    bash_out="$("$BASH" --posix -c "$src" 2>/dev/null)"; bash_code=$?
    cake_out="$("$CAKE" -c "$src" 2>/dev/null)"; cake_code=$?

    if [ "$bash_out" = "$cake_out" ] && [ "$bash_code" = "$cake_code" ]; then
        if [ "$marker" = "# cake:xfail" ]; then
            # Implemented something that was expected to fail: promote to pass.
            xfail=$((xfail+1))
            printf 'XFIX %s (now passing)\n' "$name"
        else
            pass=$((pass+1))
            printf 'PASS %s\n' "$name"
        fi
    else
        if [ "$marker" = "# cake:xfail" ]; then
            xfail=$((xfail+1))
            printf 'XFAIL %s\n' "$name"
        else
            fail=$((fail+1))
            failures+=("$name")
            printf 'FAIL %s\n' "$name"
        fi
    fi
done

printf '\n==== summary ====\n'
printf 'PASS  %d\n' "$pass"
printf 'XFAIL %d\n' "$xfail"
printf 'FAIL  %d\n' "$fail"
printf 'SKIP  %d\n' "$skip"
if [ "${#failures[@]}" -gt 0 ]; then
    printf 'Failing: %s\n' "${failures[*]}"
fi
