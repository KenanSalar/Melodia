#!/usr/bin/env bash
# Regenerate the `test-manifest.*` + `test-pubkey-manifest.b64` fixtures
# used by `minisign_tests::manifest_*` tests.
#
# Same EPHEMERAL contract as `regen-versioned.sh`: keypair generated
# fresh, no password, secret discarded at the end. Not signed by the
# production minisign key.
#
# Run from the repo root:
#   bash src/services/updater/tests/fixtures/regen-manifest.sh

set -euo pipefail

FIX="$(dirname "$0")"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
cd "$TMP"

minisign -G -W -p manifest.pub -s manifest.key >/dev/null

# A plausible-shaped latest.json body; the test only cares about the
# bytes signed, not the JSON content. Keep it small so the fixture is
# easy to inspect.
cat > test-manifest.json <<'JSON'
{"manifest_schema_version":1,"version":"0.2.0","critical":false,"platforms":{}}
JSON

# -H prehashed; -W no password; manifest=true tag distinguishes the
# signature from an artifact .minisig (which carries version+target+file).
minisign -SHW -t "version=0.2.0 manifest=true" \
  -s manifest.key -m test-manifest.json >/dev/null

cp test-manifest.json "$FIX/test-manifest.json"
cp test-manifest.json.minisig "$FIX/test-manifest.minisig"
cp manifest.pub "$FIX/test-pubkey-manifest.b64"

echo "regenerated: test-manifest.json, test-manifest.minisig, test-pubkey-manifest.b64"
echo "trusted comment: $(sed -n 's/^trusted comment: //p' "$FIX/test-manifest.minisig")"
