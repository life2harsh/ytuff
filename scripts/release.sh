#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

VERSION=$(cargo pkgid | cut -d# -f2)
BINARY="ytuff"
BUILD_DIR="target/release"

echo "=== YTuff Release Builder ==="
echo "Version: $VERSION"
echo "Project: $PROJECT_DIR"
echo ""

# Clean and build
echo "[1/5] Cleaning..."
cargo clean

echo "[2/5] Building release..."
cargo build --release

echo "[3/5] Stripping binary..."
strip $BUILD_DIR/$BINARY
ls -lh $BUILD_DIR/$BINARY

echo "[4/5] Creating tarball..."
mkdir -p dist
tar -czf dist/${BINARY}-${VERSION}-linux-x86_64.tar.gz \
	-C $BUILD_DIR $BINARY \
	-C $PROJECT_DIR ytuff.desktop README.md LICENSE

echo "[5/5] Creating Debian package structure..."
rm -rf dist/${BINARY}_deb
mkdir -p dist/${BINARY}_deb/usr/local/bin
mkdir -p dist/${BINARY}_deb/usr/local/share/applications
mkdir -p dist/${BINARY}_deb/DEBIAN

cp $BUILD_DIR/$BINARY dist/${BINARY}_deb/usr/local/bin/
cp ytuff.desktop dist/${BINARY}_deb/usr/local/share/applications/

cat > dist/${BINARY}_deb/DEBIAN/control <<EOF
Package: $BINARY
Version: $VERSION
Section: sound
Priority: optional
Architecture: amd64
Depends: libgtk-3-0, libwebkit2gtk-4.0-37, libssl3
Maintainer: YTuff Team
Description: A fast and lightweight music player built with Rust
 YTuff is a terminal music player with YouTube streaming,
 playlist management, and a daemon-based architecture.
EOF

echo ""
echo "=== Build Complete ==="
echo "Files created in dist/:"
ls -lh dist/
echo ""
echo "To create Debian package:"
echo "  dpkg-deb --build dist/${BINARY}_deb dist/${BINARY}-${VERSION}.deb"
