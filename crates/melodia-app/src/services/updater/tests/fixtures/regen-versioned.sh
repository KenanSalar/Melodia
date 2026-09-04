#!/usr/bin/env bash
# Regenerate the `test-data-versioned.*` + `test-pubkey-versioned.b64`
# fixtures used by `minisign_tests::versioned_*` tests.
#
# These are EPHEMERAL test-only artifacts — the keypair is generated
# fresh, password-less, and the secret key is discarded at the end.
# They are intentionally NOT signed by the production minisign key
# (`assets/updater-pubkey.b64`); production rotation must never depend
# on test fixtures, and test fixtures must never carry secrets that
# could be misused if leaked.
#
# Run from the repo root: `bash crates/melodia-app/src/services/updater/tests/fixtures/regen-versioned.sh`.

set -euo pipefail

FIX="$(dirname "$0")"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
cd "$TMP"

# -W: no password on the secret key. Pair with -W at sign time.
minisign -G -W -p versioned.pub -s versioned.key >/dev/null

printf 'melodia-versioned-fixture-payload\n' > test-data-versioned.bin

# -H prehashed (matches CI sign mode); -W no password;
# -t carries the version=… trusted comment that verify_stream cross-checks.
minisign -SHW -t "version=0.42.0 target=linux-x86_64-tarball file=test-data-versioned.bin" \
  -s versioned.key -m test-data-versioned.bin >/dev/null

cp test-data-versioned.bin "$FIX/test-data-versioned.bin"
cp test-data-versioned.bin.minisig "$FIX/test-data-versioned.minisig"
cp versioned.pub "$FIX/test-pubkey-versioned.b64"

echo "regenerated: test-data-versioned.bin, test-data-versioned.minisig, test-pubkey-versioned.b64"
echo "trusted comment: $(sed -n 's/^trusted comment: //p' "$FIX/test-data-versioned.minisig")"
