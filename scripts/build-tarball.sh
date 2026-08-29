#!/usr/bin/env bash
# Stage the per-user Linux bundle and tar it. Extract anywhere, run
# `./install-linux.sh`, land under ~/.local/share/Melodia/ — user-writable, so
# the updater needs no polkit.
#
# Usage:
#   ./scripts/build-tarball.sh                          # melodia-v<ver>-<uname -m>-linux.tar.gz
#   ./scripts/build-tarball.sh /tmp/custom.tar.gz       # specific output path
#   ARCH=aarch64 ./scripts/build-tarball.sh             # aarch64
#
# The `-<arch>-linux.tar.gz` suffix is not cosmetic: build-latest-json.py's
# PLATFORM_PATTERNS matches on it, and an artifact it cannot classify aborts the
# manifest build.

set -euo pipefail

REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

BINARY="${BINARY:-$REPO_ROOT/target/release/Melodia}"
[[ -f "$BINARY" ]] || { echo "ERROR: $BINARY not found. Run 'cargo build --release' first."; exit 1; }

# Both packages inherit the version from `[workspace.package]`, so `[package]`
# reads `version.workspace = true` and carries no literal. Anchor on the table
# rather than taking the file's first `version = ` line.
VERSION="$(awk -F'"' '
  /^\[/                  { in_ws = ($0 == "[workspace.package]") }
  in_ws && /^version = / { print $2; exit }
' Cargo.toml)"
[[ -n "$VERSION" ]] || { echo "ERROR: no version in Cargo.toml's [workspace.package]"; exit 1; }
ARCH="${ARCH:-$(uname -m)}"
case "$ARCH" in
  x86_64|aarch64) ;;
  *) echo "ERROR: unsupported ARCH=$ARCH (expected x86_64 or aarch64)"; exit 1 ;;
esac
OUTPUT="${1:-melodia-v${VERSION}-${ARCH}-linux.tar.gz}"

STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT
BUNDLE="$STAGING/melodia"
mkdir -p "$BUNDLE"

cp "$BINARY"                                       "$BUNDLE/Melodia"
cp "$REPO_ROOT/LICENSE"                            "$BUNDLE/"
cp -r "$REPO_ROOT/licenses"                        "$BUNDLE/licenses"
cp "$REPO_ROOT/assets/icons/logo-with-background.svg" "$BUNDLE/melodia.svg"
cp "$REPO_ROOT/scripts/Melodia.desktop"            "$BUNDLE/com.github.kenansalar.melodia.desktop"
cp "$REPO_ROOT/scripts/install-linux.sh"           "$BUNDLE/install-linux.sh"
cp "$REPO_ROOT/scripts/uninstall-linux.sh"         "$BUNDLE/uninstall-linux.sh"
chmod +x "$BUNDLE/install-linux.sh" "$BUNDLE/uninstall-linux.sh"

tar -C "$STAGING" -czf "$OUTPUT" melodia
echo "Built $OUTPUT"
