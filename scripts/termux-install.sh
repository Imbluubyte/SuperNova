#!/data/data/com.termux/files/usr/bin/bash
# Installs the SuperNova compiler (snc) in Termux and verifies it works.
# Usage (from the repository root):   ./scripts/termux-install.sh
set -e

if [ -z "$PREFIX" ] || [ ! -d "$PREFIX/bin" ]; then
    echo "This script is meant for Termux. On other systems see README.md." >&2
    exit 1
fi

echo "==> Installing build tools (rust, clang, git)"
pkg update -y
pkg install -y rust clang git

# Termux sets TMPDIR; make sure it exists and is writable.
export TMPDIR="${TMPDIR:-$PREFIX/tmp}"
mkdir -p "$TMPDIR"

echo "==> Building the compiler (first build takes a few minutes)"
cd "$(dirname "$0")/../compiler"
cargo build --release

echo "==> Installing snc to $PREFIX/bin"
cp target/release/snc "$PREFIX/bin/snc"
chmod +x "$PREFIX/bin/snc"

echo "==> Verifying the installation"
snc doctor
echo
echo "Done. Try:  snc run examples/hello.sn"
