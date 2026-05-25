//! Streaming minisign verification.
//!
//! Two traps to avoid in this module (both encoded as tests in
//! `tests/minisign_tests.rs`):
//!
//! 1. `PublicKey::from_base64()` only accepts the bare base64 portion
//!    and returns `Error::InvalidEncoding` on the full multi-line file
//!    `minisign -G` produces. The embedded pubkey at
//!    `assets/updater-pubkey.b64` is the full file. We parse it via
//!    [`PublicKey::decode`].
//! 2. [`PublicKey::verify_stream`] only accepts prehashed signatures
//!    (`signature_algorithm` bytes `0x45 0x44`). CI must sign with
//!    `minisign -SH …` — without `-H`, the client would have to buffer
//!    the entire 80–150 MiB artifact into RAM to call the non-streaming
//!    `verify()`. We reject non-prehashed signatures up-front with a
//!    clear error so a release accidentally signed without `-H` can't
//!    silently break the streaming path.

use std::io::Read;

use minisign_verify::{Error as MinisignVerifyError, PublicKey, Signature};

/// The compiled-in public key the updater verifies every downloaded
/// artifact against. Full multi-line minisign file (`untrusted comment:`
/// header + base64 key). Paired with the secret stored in the
/// `MINISIGN_SECRET_KEY` GitHub Secret (and unlocked at sign time with
/// `MINISIGN_PASSWORD`); `release.yml` signs every artifact with
/// `minisign -SH` (prehashed — see [`verify_stream`]) using that secret.
///
/// Rotation is **one-way and disruptive**: shipping a release signed
/// with a new key makes every in-flight installed client (still
/// verifying against the old key) reject the new artifact as a signature
/// mismatch. To rotate:
///   1. `minisign -G -p new.pub -s new.key` — generate a fresh keypair.
///   2. Replace `assets/updater-pubkey.b64` with the contents of `new.pub`.
///   3. Update the `MINISIGN_SECRET_KEY` GitHub Secret (base64-encoded
///      contents of `new.key`) and `MINISIGN_PASSWORD`.
///   4. Bump `Cargo.toml`'s version and let `release.yml` ship the first
///      release signed with the new key. Clients on the rotation
///      release-or-newer pick up updates normally; clients on prior
///      releases stay stuck on their installed version until the user
///      manually downloads + installs from the GitHub release page.
const EMBEDDED_PUBKEY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/updater-pubkey.b64"
));

#[derive(Debug, thiserror::Error)]
pub enum MinisignError {
    #[error("failed to parse public key: {0}")]
    PubkeyDecode(String),
    #[error("failed to parse signature: {0}")]
    SignatureDecode(String),
    #[error("verification setup failed (likely a non-prehashed signature): {0}")]
    StreamSetup(String),
    #[error("signature did not verify: {0}")]
    Verify(String),
    #[error("trusted-comment version cross-check failed: {0}")]
    VersionMismatch(String),
    #[error("I/O error while reading data to verify: {0}")]
    Io(#[from] std::io::Error),
}

/// Parse the embedded pubkey. Cheap, but called once per verify (no
/// caching) so a swapped `assets/updater-pubkey.b64` always takes
/// effect on rebuild.
pub fn embedded_pubkey() -> Result<PublicKey, MinisignError> {
    PublicKey::decode(EMBEDDED_PUBKEY)
        .map_err(|e| MinisignError::PubkeyDecode(e.to_string()))
}

/// Stream-verify `reader`'s contents against `sig_text` (the full
/// multi-line `.minisig` text) using `pubkey`. The signature **must**
/// be prehashed (CI signed with `-H`); non-prehashed signatures are
/// rejected up-front by [`PublicKey::verify_stream`] before any byte
/// of the data stream is read.
///
/// When `expected_version` is `Some`, after the cryptographic
/// verification succeeds the trusted comment is parsed for a
/// `version=<X.Y.Z>` field and asserted equal. The trusted comment is
/// covered by the signature (minisign's "global" signature hashes both
/// the prehash and the trusted-comment bytes), so a tampered comment
/// would have already failed verification — but cross-checking here
/// blocks an attacker who controls the CDN/manifest from substituting
/// one legitimately-signed-by-us artifact for another at a different
/// version. CI must sign with `minisign -SH -t "version=$VERSION …"`
/// for this check to find a `version=` field; older releases (and
/// existing test fixtures without `-t`) won't carry one and will be
/// rejected with [`MinisignError::VersionMismatch`].
///
/// Pass `None` to skip the cross-check (existing tests + the rare
/// caller verifying a signature whose version we don't know).
///
/// Production callers pass [`embedded_pubkey`]; the parameter exists
/// so tests can inject their own keypair (the production pubkey is
/// keyed to a destroyed-private-key placeholder, so test fixtures
/// can't match its key id).
pub fn verify_stream<R: Read>(
    mut reader: R,
    sig_text: &str,
    pubkey: &PublicKey,
    expected_version: Option<&str>,
) -> Result<(), MinisignError> {
    let sig = Signature::decode(sig_text)
        .map_err(|e| MinisignError::SignatureDecode(e.to_string()))?;
    // Capture the trusted comment up front. After `pubkey.verify_stream(&sig)`
    // moves `sig` into the verifier we can't reach it from here; reading
    // before `finalize()` is safe because the global signature covers the
    // trusted-comment bytes, so a tampered comment would fail
    // verification below before this string is acted on.
    let trusted_comment = sig.trusted_comment().to_string();
    let mut verifier = pubkey
        .verify_stream(&sig)
        .map_err(|e| match e {
            MinisignVerifyError::UnsupportedLegacyMode => MinisignError::StreamSetup(
                "signature is not prehashed (CI must sign with `minisign -SH …`)".into(),
            ),
            other => MinisignError::StreamSetup(other.to_string()),
        })?;
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        verifier.update(&buf[..n]);
    }
    verifier
        .finalize()
        .map_err(|e| MinisignError::Verify(e.to_string()))?;

    if let Some(expected) = expected_version {
        match parse_trusted_comment_version(&trusted_comment) {
            Some(found) if found == expected => Ok(()),
            Some(found) => Err(MinisignError::VersionMismatch(format!(
                "expected version {expected}, signature names {found} (trusted comment: {trusted_comment})"
            ))),
            None => Err(MinisignError::VersionMismatch(format!(
                "trusted comment carries no `version=` field — release must sign with \
                 `minisign -SH -t \"version=$VERSION …\"` (trusted comment: {trusted_comment})"
            ))),
        }
    } else {
        Ok(())
    }
}

/// Extracts the `version=` value from a minisign trusted-comment line.
/// The comment is a free-form space-separated bag of `key=value` tokens
/// (everything after `"trusted comment: "`); we look for the token whose
/// key is `version` and return its value.
///
/// Stops at the next whitespace, so `version=0.42.0 target=…` returns
/// `Some("0.42.0")`. Returns `None` if no `version=` token is present.
pub(crate) fn parse_trusted_comment_version(comment: &str) -> Option<&str> {
    parse_trusted_comment_field(comment, "version")
}

/// Generic key= lookup in a trusted-comment line. Used by
/// [`parse_trusted_comment_version`] and the manifest-tag check in
/// [`verify_manifest_bytes`]. Returns the value as a `&str` slice into
/// `comment`; the comment is a free-form space-separated bag of
/// `key=value` tokens.
pub(crate) fn parse_trusted_comment_field<'a>(comment: &'a str, key: &str) -> Option<&'a str> {
    for token in comment.split_whitespace() {
        if let Some(value) = token.strip_prefix(key)
            && let Some(value) = value.strip_prefix('=')
        {
            return Some(value);
        }
    }
    None
}

/// Verify a small, fully-buffered payload (the `latest.json` manifest
/// body) against `sig_text` (the contents of `latest.json.minisig`)
/// using `pubkey`. Mirrors [`verify_stream`] but takes a `&[u8]`
/// directly — the manifest is ~1 KiB, so buffering is fine and avoids
/// a `Read`-over-bytes adapter.
///
/// When `expected_manifest_tag` is `Some("true")` (the production
/// caller), asserts the trusted comment carries `manifest=true`. This
/// prevents an attacker from cross-pasting an artifact's `.minisig`
/// (which carries `version=$VERSION target=$KIND file=$BASENAME`) into
/// this verification slot — the artifact sigs are valid minisign blobs
/// signed with the same key, so the cryptographic verify would pass
/// without this domain-separation check.
///
/// `expected_version`, when `Some`, is cross-checked against the
/// trusted comment's `version=` field, same as [`verify_stream`]'s
/// version check. Production callers pass `None` here because the
/// manifest *is* the source of the version string — there's nothing
/// to cross-check against until after we parse it.
pub fn verify_manifest_bytes(
    body: &[u8],
    sig_text: &str,
    pubkey: &PublicKey,
    expected_version: Option<&str>,
    expected_manifest_tag: Option<&str>,
) -> Result<(), MinisignError> {
    let sig = Signature::decode(sig_text)
        .map_err(|e| MinisignError::SignatureDecode(e.to_string()))?;
    let trusted_comment = sig.trusted_comment().to_string();
    let mut verifier = pubkey
        .verify_stream(&sig)
        .map_err(|e| match e {
            MinisignVerifyError::UnsupportedLegacyMode => MinisignError::StreamSetup(
                "manifest signature is not prehashed (CI must sign with `minisign -SH …`)".into(),
            ),
            other => MinisignError::StreamSetup(other.to_string()),
        })?;
    verifier.update(body);
    verifier
        .finalize()
        .map_err(|e| MinisignError::Verify(e.to_string()))?;

    if let Some(expected_tag) = expected_manifest_tag {
        match parse_trusted_comment_field(&trusted_comment, "manifest") {
            Some(found) if found == expected_tag => {}
            Some(found) => {
                return Err(MinisignError::VersionMismatch(format!(
                    "manifest-tag mismatch: expected manifest={expected_tag}, signature names \
                     manifest={found} (trusted comment: {trusted_comment})"
                )));
            }
            None => {
                return Err(MinisignError::VersionMismatch(format!(
                    "trusted comment carries no `manifest=` field — the signature may belong to \
                     an artifact rather than the manifest. Release must sign with \
                     `minisign -SH -t \"version=$VERSION manifest=true\"` (trusted comment: \
                     {trusted_comment})"
                )));
            }
        }
    }

    if let Some(expected) = expected_version {
        match parse_trusted_comment_version(&trusted_comment) {
            Some(found) if found == expected => {}
            Some(found) => {
                return Err(MinisignError::VersionMismatch(format!(
                    "expected version {expected}, manifest signature names {found} (trusted \
                     comment: {trusted_comment})"
                )));
            }
            None => {
                return Err(MinisignError::VersionMismatch(format!(
                    "trusted comment carries no `version=` field (trusted comment: \
                     {trusted_comment})"
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "tests/minisign_tests.rs"]
mod tests;
