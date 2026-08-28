//! Streaming minisign verification.
//!
//! Two traps to avoid in this module, both pinned by `tests/minisign_tests.rs`:
//!
//! 1. `PublicKey::from_base64()` only accepts the bare base64 portion and returns
//!    `Error::InvalidEncoding` on the full multi-line file `minisign -G` produces. The embedded
//!    pubkey at `assets/updater-pubkey.b64` is the full file, so it is parsed via
//!    [`PublicKey::decode`].
//! 2. [`PublicKey::verify_stream`] only accepts prehashed signatures, so CI must sign with
//!    `minisign -SH …` — without `-H` the client would have to buffer the whole artifact into RAM
//!    for the non-streaming `verify()`. Non-prehashed signatures are rejected up-front with a
//!    clear error, so a release accidentally signed without `-H` can't silently break the
//!    streaming path.

use std::io::Read;

use minisign_verify::{Error as MinisignVerifyError, PublicKey, Signature};

/// The compiled-in public key the updater verifies every downloaded artifact against. Full
/// multi-line minisign file, paired with the `MINISIGN_SECRET_KEY` GitHub Secret; `release.yml`
/// signs every artifact with `minisign -SH` (prehashed — see [`verify_stream`]).
///
/// Rotation is **one-way and disruptive**: shipping a release signed with a new key makes every
/// installed client, still verifying against the old key, reject the new artifact as a signature
/// mismatch. To rotate:
///   1. `minisign -G -p new.pub -s new.key` — generate a fresh keypair.
///   2. Replace `assets/updater-pubkey.b64` with the contents of `new.pub`.
///   3. Update the `MINISIGN_SECRET_KEY` GitHub Secret (base64-encoded `new.key`) and
///      `MINISIGN_PASSWORD`.
///   4. Bump `Cargo.toml`'s version and push the matching tag; `release.yml` ships the first
///      release signed with the new key. Clients on prior releases stay stuck on their installed
///      version until the user manually downloads and installs from the GitHub release page.
const EMBEDDED_PUBKEY: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/updater-pubkey.b64"));

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

/// Parse the embedded pubkey. Cheap, but called once per verify with no caching, so a swapped
/// `assets/updater-pubkey.b64` always takes effect on rebuild.
pub fn embedded_pubkey() -> Result<PublicKey, MinisignError> {
    PublicKey::decode(EMBEDDED_PUBKEY).map_err(|e| MinisignError::PubkeyDecode(e.to_string()))
}

/// Stream-verify `reader`'s contents against `sig_text` (the full multi-line `.minisig` text)
/// using `pubkey`. The signature **must** be prehashed; non-prehashed ones are rejected up-front
/// by [`PublicKey::verify_stream`] before any byte of the data stream is read.
///
/// When `expected_version` is `Some`, the trusted comment is parsed for a `version=<X.Y.Z>` field
/// and asserted equal once the cryptographic verification succeeds — the comment is covered by the
/// signature, so this is a substitution check rather than an integrity one. CI must sign with
/// `minisign -SH -t "version=$VERSION …"` for it to find a `version=` field; older releases and
/// fixtures without `-t` are rejected with [`MinisignError::VersionMismatch`].
///
/// Pass `None` to skip the cross-check.
///
/// Production callers pass [`embedded_pubkey`]; the parameter exists so tests can inject their own
/// keypair, the production pubkey being keyed to a placeholder no fixture can match.
pub fn verify_stream<R: Read>(
    mut reader: R,
    sig_text: &str,
    pubkey: &PublicKey,
    expected_version: Option<&str>,
) -> Result<(), MinisignError> {
    let sig =
        Signature::decode(sig_text).map_err(|e| MinisignError::SignatureDecode(e.to_string()))?;
    // Capture the trusted comment up front: `pubkey.verify_stream(&sig)` moves `sig` into the
    // verifier. Reading before `finalize()` is safe because the global signature covers the
    // trusted-comment bytes, so a tampered comment fails verification before this is acted on.
    let trusted_comment = sig.trusted_comment().to_string();
    let mut verifier = pubkey.verify_stream(&sig).map_err(|e| match e {
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
    verifier.finalize().map_err(|e| MinisignError::Verify(e.to_string()))?;

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

/// The `version=` value from a minisign trusted-comment line. Stops at the next whitespace, so
/// `version=0.42.0 target=…` returns `Some("0.42.0")`.
pub(crate) fn parse_trusted_comment_version(comment: &str) -> Option<&str> {
    parse_trusted_comment_field(comment, "version")
}

/// Generic `key=` lookup in a trusted-comment line, which is a free-form space-separated bag of
/// `key=value` tokens. The value is a slice into `comment`.
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

/// Verify a small, fully-buffered payload (the `latest.json` manifest body) against `sig_text`
/// using `pubkey`. Mirrors [`verify_stream`] but takes a `&[u8]` directly — the manifest is ~1 KiB,
/// so buffering avoids a `Read`-over-bytes adapter.
///
/// `expected_manifest_tag` of `Some("true")`, which is what production passes, asserts the trusted
/// comment carries `manifest=true`. That is **domain separation**: an artifact's `.minisig` is a
/// valid minisign blob signed with the same key, so the cryptographic verify would pass on one
/// cross-pasted into this slot.
///
/// `expected_version` is cross-checked as in [`verify_stream`]. Production passes `None` here, the
/// manifest *being* the source of the version string.
pub fn verify_manifest_bytes(
    body: &[u8],
    sig_text: &str,
    pubkey: &PublicKey,
    expected_version: Option<&str>,
    expected_manifest_tag: Option<&str>,
) -> Result<(), MinisignError> {
    let sig =
        Signature::decode(sig_text).map_err(|e| MinisignError::SignatureDecode(e.to_string()))?;
    let trusted_comment = sig.trusted_comment().to_string();
    let mut verifier = pubkey.verify_stream(&sig).map_err(|e| match e {
        MinisignVerifyError::UnsupportedLegacyMode => MinisignError::StreamSetup(
            "manifest signature is not prehashed (CI must sign with `minisign -SH …`)".into(),
        ),
        other => MinisignError::StreamSetup(other.to_string()),
    })?;
    verifier.update(body);
    verifier.finalize().map_err(|e| MinisignError::Verify(e.to_string()))?;

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
