//! Streaming minisign verification, pinned by `tests/minisign_tests.rs` against two traps.
//! `PublicKey::from_base64` rejects the full key file `minisign -G` writes, so the embedded key
//! goes through [`PublicKey::decode`]. [`PublicKey::verify_stream`] takes only prehashed
//! signatures, so CI signs with `minisign -SH` and a legacy one is refused up front.

use std::io::Read;

use minisign_verify::{Error as MinisignVerifyError, PublicKey, Signature};

/// The key every artifact and manifest is verified against, paired with the `MINISIGN_SECRET_KEY`
/// secret. **Rotating it strands every installed client**: each rejects the first release signed
/// with the new key, and moves past it only by a manual install.
///
/// Relative rather than through `MELODIA_REPO_ROOT`, which cargo defines only for a build run from
/// inside the repo, so a `cargo install --git` would stop compiling.
const EMBEDDED_PUBKEY: &str = include_str!("../../../../../assets/updater-pubkey.b64");

#[derive(Debug, thiserror::Error)]
pub enum MinisignError {
    #[error("failed to parse public key: {0}")]
    PubkeyDecode(#[source] MinisignVerifyError),
    #[error("failed to parse signature: {0}")]
    SignatureDecode(#[source] MinisignVerifyError),
    #[error("signature is not prehashed (CI must sign with `minisign -SH …`)")]
    NotPrehashed,
    #[error("verification setup failed: {0}")]
    StreamSetup(#[source] MinisignVerifyError),
    #[error("signature did not verify: {0}")]
    Verify(#[source] MinisignVerifyError),
    #[error("trusted-comment version cross-check failed: {0}")]
    VersionMismatch(String),
    #[error("I/O error while reading data to verify: {0}")]
    Io(#[from] std::io::Error),
}

pub fn embedded_pubkey() -> Result<PublicKey, MinisignError> {
    PublicKey::decode(EMBEDDED_PUBKEY).map_err(MinisignError::PubkeyDecode)
}

/// Stream-verifies `reader` against the full `.minisig` text, which must be prehashed.
///
/// With `expected_version`, the signed trusted comment must also carry `version=<it>`: integrity
/// is already proven, so this catches a validly signed artifact from another release. `pubkey` is
/// a parameter for tests, the embedded key matching no fixture.
pub fn verify_stream<R: Read>(
    reader: R,
    sig_text: &str,
    pubkey: &PublicKey,
    expected_version: Option<&str>,
) -> Result<(), MinisignError> {
    let trusted_comment = verified_comment(reader, sig_text, pubkey)?;

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

pub(crate) fn parse_trusted_comment_version(comment: &str) -> Option<&str> {
    parse_trusted_comment_field(comment, "version")
}

/// `key`'s value in a trusted comment, which is a space-separated bag of `key=value` tokens.
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

/// Verifies `latest.json` and requires `manifest=<expected_manifest_tag>` in its trusted comment.
/// That is **domain separation**: an artifact's `.minisig` carries the same key and would verify
/// here too. No version is cross-checked, the manifest being where the version comes from.
pub fn verify_manifest_bytes(
    body: &[u8],
    sig_text: &str,
    pubkey: &PublicKey,
    expected_manifest_tag: &str,
) -> Result<(), MinisignError> {
    let trusted_comment = verified_comment(body, sig_text, pubkey)?;

    match parse_trusted_comment_field(&trusted_comment, "manifest") {
        Some(found) if found == expected_manifest_tag => Ok(()),
        Some(found) => Err(MinisignError::VersionMismatch(format!(
            "manifest-tag mismatch: expected manifest={expected_manifest_tag}, signature names \
             manifest={found} (trusted comment: {trusted_comment})"
        ))),
        None => Err(MinisignError::VersionMismatch(format!(
            "trusted comment carries no `manifest=` field — the signature may belong to \
             an artifact rather than the manifest. Release must sign with \
             `minisign -SH -t \"version=$VERSION manifest=true\"` (trusted comment: \
             {trusted_comment})"
        ))),
    }
}

/// `sig_text`'s trusted comment, once `reader`'s contents verify against it. The signature
/// covers the comment as well, so it is unauthenticated text until `finalize` passes.
fn verified_comment(
    mut reader: impl Read,
    sig_text: &str,
    pubkey: &PublicKey,
) -> Result<String, MinisignError> {
    let sig = Signature::decode(sig_text).map_err(MinisignError::SignatureDecode)?;
    let mut verifier = pubkey.verify_stream(&sig).map_err(setup_failure)?;
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        verifier.update(&buf[..n]);
    }
    verifier.finalize().map_err(MinisignError::Verify)?;
    Ok(sig.trusted_comment().to_owned())
}

/// A verifier that wouldn't start, the legacy (non-prehashed) mode named on its own since it is
/// the one a release can be cut with by mistake.
fn setup_failure(e: MinisignVerifyError) -> MinisignError {
    match e {
        MinisignVerifyError::UnsupportedLegacyMode => MinisignError::NotPrehashed,
        other => MinisignError::StreamSetup(other),
    }
}

#[cfg(test)]
#[path = "tests/minisign_tests.rs"]
mod tests;
