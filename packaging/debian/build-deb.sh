#!/usr/bin/env bash
# Build the Aura Debian package: target/aura_<version>_<arch>.deb
#
# Builds natively for the host architecture (arm64 on an ARM machine,
# amd64 on x86-64) — run it on the architecture you are packaging for.
# Prerequisites: a Rust toolchain and dpkg-deb (present on any Debian/Ubuntu).
#
# Layout:
#   /usr/bin/aura                       the CLI
#   /usr/share/aura/examples/           example programs
#   /usr/share/aura/docs/               documentation
#   /usr/share/doc/aura/                copyright, README, changelog
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
ARCH=$(dpkg --print-architecture)
STAGE=target/deb-stage
DEB="target/aura_${VERSION}_${ARCH}.deb"

echo "Building aura $VERSION for $ARCH..."
cargo build --release -p aura-cli

echo "Staging package tree..."
rm -rf "$STAGE"
mkdir -p "$STAGE/DEBIAN" "$STAGE/usr/bin" \
         "$STAGE/usr/share/aura" "$STAGE/usr/share/doc/aura"
install -m 755 target/release/aura "$STAGE/usr/bin/aura"
strip "$STAGE/usr/bin/aura"
cp -r examples "$STAGE/usr/share/aura/examples"
cp -r docs "$STAGE/usr/share/aura/docs"
cp README.md "$STAGE/usr/share/doc/aura/"
cp LICENSE "$STAGE/usr/share/doc/aura/copyright"
gzip -9 -n -c packaging/debian/changelog > "$STAGE/usr/share/doc/aura/changelog.gz"

# Dependencies: computed from the stripped binary so they track reality
# (glibc version bounds etc.) instead of being guessed.
echo "Computing shared-library dependencies..."
mkdir -p "$STAGE/debian"
touch "$STAGE/debian/control"   # dpkg-shlibdeps insists on this file existing
DEPS=$(cd "$STAGE" && dpkg-shlibdeps -O usr/bin/aura 2>/dev/null | sed 's/^shlibs:Depends=//')
rm -rf "$STAGE/debian"

INSTALLED_SIZE=$(du -sk "$STAGE" --exclude=DEBIAN | cut -f1)
sed -e "s/@VERSION@/$VERSION/" \
    -e "s/@ARCH@/$ARCH/" \
    -e "s/@DEPS@/$DEPS/" \
    -e "s/@SIZE@/$INSTALLED_SIZE/" \
    packaging/debian/control.in > "$STAGE/DEBIAN/control"

echo "Building $DEB..."
dpkg-deb --build --root-owner-group "$STAGE" "$DEB"
echo "Done: $DEB"
