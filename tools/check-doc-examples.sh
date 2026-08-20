#!/usr/bin/env bash
# Verify the documentation's code samples against the real toolchain.
#
#   tools/check-doc-examples.sh [path-to-aura-binary]
#
# Every ```aura fenced block in docs/**/*.md is extracted and classified:
#   * contains `static void Main`  -> must compile AND run cleanly
#   * contains a `// ERROR` line   -> must FAIL to compile (the docs promise
#                                     a compile error; hold them to it)
#   * anything else                -> fragment, skipped
#
# This keeps the guide from drifting: a language change that breaks a
# documented sample fails this script, which runs alongside the test
# battery.
set -u
cd "$(dirname "$0")/.."

AURA=${1:-target/debug/aura}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

pass=0 fail=0 skipped=0

check_block() {
    local src=$1 label=$2
    if grep -q '// ERROR' "$src"; then
        if $AURA compile "$src" >/dev/null 2>&1; then
            echo "FAIL $label: documented compile error compiles cleanly"
            fail=$((fail + 1))
        else
            pass=$((pass + 1))
        fi
    elif grep -q 'static void Main' "$src"; then
        if out=$($AURA run "$src" </dev/null 2>&1); then
            pass=$((pass + 1))
        else
            echo "FAIL $label:"
            echo "$out" | head -4 | sed 's/^/    /'
            fail=$((fail + 1))
        fi
    else
        skipped=$((skipped + 1))
    fi
}

n=0
for md in $(find docs -name '*.md' | sort); do
    block="" in_block=0 start=0 lineno=0
    while IFS= read -r line; do
        lineno=$((lineno + 1))
        if [ $in_block -eq 0 ] && [ "$line" = '```aura' ]; then
            in_block=1 start=$lineno block=""
            continue
        fi
        if [ $in_block -eq 1 ] && [ "$line" = '```' ]; then
            in_block=0
            n=$((n + 1))
            printf '%s\n' "$block" > "$TMP/snippet_$n.aura"
            check_block "$TMP/snippet_$n.aura" "$md:$start"
            continue
        fi
        if [ $in_block -eq 1 ]; then
            block="$block$line
"
        fi
    done < "$md"
done

echo "doc examples: $pass verified, $skipped fragments skipped, $fail failed"
[ $fail -eq 0 ]
