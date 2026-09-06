use melodia_core::error::AppError;

use super::FailureKind;

/// The two modules whose `Validation` prose `classify` keys on. Pulled in so the table below
/// classifies the messages the tree actually produces: a reword that drops "signature" would
/// otherwise leave the table passing against a string nothing sends any more.
const VERIFY_SRC: &str = include_str!("../install/verify.rs");
const GITHUB_SRC: &str = include_str!("../github.rs");

/// Message prefixes paired with the source they are built in. Each is the literal ahead of the
/// `{e}` interpolation, which is all `classify` can see.
const PRODUCED_MESSAGES: &[(&str, &str, FailureKind)] = &[
    ("update signature verification failed: ", VERIFY_SRC, FailureKind::Signature),
    ("manifest signature verification failed: ", GITHUB_SRC, FailureKind::Signature),
    ("manifest signature missing or unreachable: HTTP ", GITHUB_SRC, FailureKind::Signature),
    // The near-miss: same modules, same variant, no "signature" in the prose, so it reads as a
    // parse failure. Deliberate — a broken embedded key is not an adversarial artifact.
    ("embedded updater pubkey is invalid: ", VERIFY_SRC, FailureKind::Parse),
    ("parse latest.json failed: ", GITHUB_SRC, FailureKind::Parse),
    // A rolled-back swap also reads as Parse. The classifier is this coarse on purpose; the raw
    // error was logged at the send site and the toast is a one-liner.
    ("post-swap launch verification timed out: ", VERIFY_SRC, FailureKind::Parse),
];

#[test]
fn every_validation_message_lands_in_the_bucket_its_module_meant() {
    for (message, _, expected) in PRODUCED_MESSAGES {
        let err = AppError::Validation(format!("{message}oh no"));
        assert_eq!(FailureKind::classify(&err), *expected, "classify disagrees about {message:?}");
    }
}

/// Without this the table above is a copy that drifts: `classify` recognises a signature failure
/// by an unanchored substring of prose written three modules away, and the wrong toast on the one
/// failure the threat model treats as adversarial is invisible in review.
#[test]
fn every_classified_message_is_still_the_one_its_module_sends() {
    let missing: Vec<&str> = PRODUCED_MESSAGES
        .iter()
        .filter(|(message, source, _)| !source.contains(message))
        .map(|(message, _, _)| *message)
        .collect();

    assert!(
        missing.is_empty(),
        "{missing:?} no longer appear in the module that built them — reword and reclassify together"
    );
}

/// The two variants that are matched ahead of the `Validation` substring check, so a network or
/// I/O failure whose message happens to say "signature" still reads as what it is.
#[test]
fn transport_and_disk_are_classified_before_the_substring_check() {
    let network = AppError::network_msg("fetch latest.json returned HTTP 503");
    assert_eq!(FailureKind::classify(&network), FailureKind::Network);

    let io = AppError::io_other("no space left on device");
    assert_eq!(FailureKind::classify(&io), FailureKind::Io);

    let misleading = AppError::network_msg("signature server unreachable");
    assert_eq!(FailureKind::classify(&misleading), FailureKind::Network);
}

/// `install/mod.rs` wraps both `spawn_blocking` join failures as `Settings`, which is the only
/// non-`Validation` error the updater raises that isn't transport or disk.
#[test]
fn anything_else_falls_through_to_other() {
    let join = AppError::Settings("update install task join error: panicked".into());
    assert_eq!(FailureKind::classify(&join), FailureKind::Other);
}

/// Read by the `Settings.update-failed-reason(kind)` switch in `globals/updater.slint`; a value
/// changed on one side alone silently picks the fallback message.
#[test]
fn the_kind_discriminators_are_the_ones_slint_branches_on() {
    let pairs = [
        (FailureKind::Network, "network"),
        (FailureKind::Signature, "signature"),
        (FailureKind::Parse, "parse"),
        (FailureKind::Io, "io"),
        (FailureKind::Other, "other"),
    ];
    for (kind, expected) in pairs {
        assert_eq!(kind.as_kind_str(), expected);
    }
}
