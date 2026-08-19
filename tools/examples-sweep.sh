#!/usr/bin/env bash
# Run every example under both tiers and compare their output byte-for-byte.
#
#   tools/examples-sweep.sh [path-to-aura-binary]
#
# Default binary: target/debug/aura. To sweep the x86-64 JIT from another
# host arch, pass the cross-built binary — the qemu runner in
# .cargo/config.toml applies to `cargo run`, so the simplest is:
#   cargo build --target x86_64-unknown-linux-gnu -p aura-cli
#   tools/examples-sweep.sh "qemu-x86_64 -L /usr/x86_64-linux-gnu target/x86_64-unknown-linux-gnu/debug/aura"
#
# Exits non-zero if any example fails to run or the tiers disagree.
set -u
cd "$(dirname "$0")/.."

AURA=${1:-target/debug/aura}
pass=0 fail=0

check() {
    local name=$1; shift
    local interp jit
    interp=$($AURA run "$@" </dev/null 2>&1); local ri=$?
    jit=$($AURA run --jit "$@" </dev/null 2>&1); local rj=$?
    if [ $ri -ne 0 ] || [ $rj -ne 0 ]; then
        echo "FAIL $name (exit interp=$ri jit=$rj)"
        echo "$interp" | head -3 | sed 's/^/    /'
        fail=$((fail + 1))
    elif [ "$interp" != "$jit" ]; then
        echo "FAIL $name (tier outputs differ)"
        fail=$((fail + 1))
    else
        pass=$((pass + 1))
    fi
}

for f in examples/*.aura; do
    check "$f" "$f"
done
# The multifile example is a directory of modules compiled together.
if [ -d examples/multifile ]; then
    check "examples/multifile" examples/multifile/*.aura
fi

echo "examples sweep: $pass passed, $fail failed"
[ $fail -eq 0 ]
