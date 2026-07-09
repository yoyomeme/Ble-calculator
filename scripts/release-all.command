#!/usr/bin/env bash
#
# Double-click this file (Finder → release-all.command) to build release
# installers for ALL desktop platforms — macOS, Linux, and Windows.
#
# A single machine cannot produce real native builds for other operating
# systems (the Rust BLE core needs each platform's own toolchain), so this
# launcher triggers the "Release" GitHub Actions workflow, which builds every
# platform on its own native runner and (optionally) publishes a GitHub Release.
#
# Requirements: GitHub CLI (`gh`) installed and authenticated (`gh auth login`).

set -euo pipefail

# Run from the repo root regardless of where the file is double-clicked from.
cd "$(dirname "$0")/.."

WORKFLOW="release.yml"

pause_and_exit() {
  echo ""
  read -r -p "Press Enter to close this window..." _ || true
  exit "${1:-0}"
}

echo "=================================================="
echo "  Evolve Calc — Release all platforms"
echo "=================================================="
echo ""

if ! command -v gh >/dev/null 2>&1; then
  echo "✗ GitHub CLI (gh) is not installed."
  echo "  Install it from https://cli.github.com then run this again."
  pause_and_exit 1
fi

if ! gh auth status >/dev/null 2>&1; then
  echo "✗ GitHub CLI is not authenticated."
  echo "  Run this in a terminal first:  gh auth login"
  pause_and_exit 1
fi

BRANCH="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo main)"
echo "Repository branch: ${BRANCH}"
echo ""

# Auto-increment the version from the tags already on GitHub.
LATEST="$(node scripts/next-version.mjs --latest 2>/dev/null || echo none)"
NEXT="$(node scripts/next-version.mjs patch 2>/dev/null || echo "")"

echo "Latest published tag: ${LATEST}"
echo ""
echo "Choose a version to publish:"
echo "  • press Enter        → auto-increment to ${NEXT:-v0.1.0}"
echo "  • minor / major      → auto-bump that level instead"
echo "  • vX.Y.Z             → a specific version"
echo "  • none               → just build installers, don't publish a Release"
read -r -p "Version [${NEXT:-v0.1.0}]: " INPUT || INPUT=""

case "${INPUT}" in
  "")                VERSION="${NEXT:-v0.1.0}" ;;
  none|NONE|skip)    VERSION="" ;;
  patch|minor|major) VERSION="$(node scripts/next-version.mjs "${INPUT}")" ;;
  v[0-9]*)           VERSION="${INPUT}" ;;
  [0-9]*)            VERSION="v${INPUT}" ;;   # accept "1.2.3" and add the v
  *)                 echo "  Unrecognized input; using ${NEXT:-v0.1.0}"; VERSION="${NEXT:-v0.1.0}" ;;
esac

if [ -n "${VERSION}" ]; then
  echo ""
  echo "→ Will publish Release ${VERSION}"
fi

echo ""
echo "→ Triggering the Release workflow on '${BRANCH}'..."
if [ -n "${VERSION}" ]; then
  gh workflow run "${WORKFLOW}" --ref "${BRANCH}" -f version="${VERSION}"
else
  gh workflow run "${WORKFLOW}" --ref "${BRANCH}"
fi

echo "  Triggered. Locating the run..."
# Give GitHub a moment to register the new run before we look it up.
sleep 5

RUN_ID="$(gh run list --workflow="${WORKFLOW}" --limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || echo "")"

if [ -n "${RUN_ID}" ]; then
  echo "  Opening run #${RUN_ID} in your browser..."
  gh run view "${RUN_ID}" --web >/dev/null 2>&1 || true
  echo "  Watching progress here (Ctrl-C to stop watching — the build keeps running):"
  echo ""
  gh run watch "${RUN_ID}" --exit-status || true
else
  echo "  Could not auto-find the run. Opening the Actions tab..."
  gh repo view --web >/dev/null 2>&1 || true
fi

echo ""
echo "Note: workflow_dispatch only works once release.yml exists on the"
echo "repository's DEFAULT branch (main). If the trigger was rejected, merge"
echo "this branch to main first, then double-click again."

pause_and_exit 0
