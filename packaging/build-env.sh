# shellcheck shell=bash
# The build scripts source this for REPO_ROOT, made the working directory, VERSION and ARCH;
# each check's `exit` ends the build that sourced it.

REPO_ROOT="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
cd "$REPO_ROOT" || exit

# Every member inherits the version from `[workspace.package]`, so `[package]`
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
