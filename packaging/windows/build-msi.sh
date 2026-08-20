#!/usr/bin/env bash
# Build the Aura Windows x64 installer: target/Aura-<version>-x64.msi
#
# Cross-builds from Linux (any host arch) — no Windows machine needed.
# Prerequisites (Debian/Ubuntu package names):
#   rustup target add x86_64-pc-windows-gnu
#   apt install gcc-mingw-w64-x86-64 wixl
# The linker for the Windows target is configured in .cargo/config.toml.
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
TARGET=x86_64-pc-windows-gnu
STAGE=target/msi-stage

echo "Building aura.exe $VERSION for $TARGET..."
cargo build --release --target "$TARGET" -p aura-cli

echo "Staging install tree..."
rm -rf "$STAGE"
mkdir -p "$STAGE/bin"
cp "target/$TARGET/release/aura.exe" "$STAGE/bin/"
x86_64-w64-mingw32-strip "$STAGE/bin/aura.exe"
cp LICENSE README.md "$STAGE/"
cp -r examples "$STAGE/examples"
cp -r docs "$STAGE/docs"
rm -rf "$STAGE/docs/research"   # internal design studies; not user docs

# Harvest everything except aura.exe (which has an explicit component in
# aura.wxs, since it anchors the PATH entry and key paths).
echo "Harvesting file components..."
find "$STAGE" -type f ! -path "$STAGE/bin/aura.exe" |
    wixl-heat -p "$STAGE/" \
        --component-group CG.Files \
        --var var.StageDir \
        --directory-ref INSTALLDIR \
        --win64 > target/msi-harvest.wxs

MSI="target/Aura-$VERSION-x64.msi"
echo "Building $MSI..."
wixl -a x64 \
    -D Version="$VERSION" \
    -D StageDir="$STAGE" \
    -D Win64=yes \
    -o "$MSI" \
    packaging/windows/aura.wxs target/msi-harvest.wxs

echo "Done: $MSI"
