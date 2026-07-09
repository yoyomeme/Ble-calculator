#!/usr/bin/env bash
#
# Double-click this file (Finder → build-mac-arm64.command) to build a LOCAL
# release of Evolve Calc for this Apple Silicon (arm64) Mac only.
#
# Unlike release-all.command (which triggers the multi-platform GitHub Actions
# workflow), this runs entirely on your machine: it compiles the Rust BLE core
# for aarch64-apple-darwin, bundles the app, and produces the macOS installer
# under dist/. Nothing is pushed or published.
#
# Requirements: Node.js + npm, and the Rust toolchain (rustup) with the
# aarch64-apple-darwin target. The script installs the Rust target for you if
# it is missing. Pass --skip-native to bundle with the TypeScript mock instead
# of compiling the native module.

set -euo pipefail

# Run from the repo root regardless of where the file is double-clicked from.
cd "$(dirname "$0")/.."

pause_and_exit() {
  echo ""
  read -r -p "Press Enter to close this window..." _ || true
  exit "${1:-0}"
}

echo "=================================================="
echo "  Evolve Calc — Local build (macOS arm64)"
echo "=================================================="
echo ""

if [ "$(uname -s)" != "Darwin" ]; then
  echo "✗ This script only builds on macOS."
  pause_and_exit 1
fi

if [ "$(uname -m)" != "arm64" ]; then
  echo "⚠ This Mac reports arch '$(uname -m)', not arm64 (Apple Silicon)."
  echo "  The build will still target arm64, but it cannot run here natively."
  echo ""
fi

if ! command -v node >/dev/null 2>&1; then
  echo "✗ Node.js is not installed. Install it from https://nodejs.org then retry."
  pause_and_exit 1
fi

# Install JS dependencies on first run so double-clicking works from a clean checkout.
if [ ! -d node_modules ]; then
  echo "→ Installing npm dependencies (first run)..."
  npm install
  echo ""
fi

echo "→ Building macOS arm64 package (target: mac-arm64)..."
echo ""

# Forward any extra flags (e.g. --skip-native, --dry-run) straight through.
npm run package -- mac-arm64 "$@"

echo ""
echo "✓ Done. Find the installer and app under:  dist/"
ls -1 dist 2>/dev/null || true

pause_and_exit 0
