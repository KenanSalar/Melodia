use melodia_core::error::{AppError, describe};

use super::{FailureKind, UpdateFailure};

/// What `?` hands a failure raised below the updater. The kind is read off the variant and never
/// the text, so a network error whose message happens to say "signature" still reads as one.
#[test]
fn a_failure_no_updater_site_named_keeps_its_layers_category() {
    let network = UpdateFailure::from(AppError::network_msg("fetch latest.json returned HTTP 503"));
    assert_eq!(network.kind, FailureKind::Network);

    let io = UpdateFailure::from(AppError::io_other("no space left on device"));
    assert_eq!(io.kind, FailureKind::Io);

    let misleading = UpdateFailure::from(AppError::network_msg("signature server unreachable"));
    assert_eq!(misleading.kind, FailureKind::Network);
}

/// A join error, an installer's exit code, a rolled-back swap: none of them is the server's or the
/// disk's, so each takes the generic toast unless its site named something else.
#[test]
fn the_updaters_own_errors_take_the_generic_toast() {
    let join = UpdateFailure::from(AppError::updater_msg("update install task join error"));
    assert_eq!(join.kind, FailureKind::Other);
}

/// The panel's reason is `describe` over the failure, so a failure that stopped at its own text
/// would tell the user what failed and never why.
#[test]
fn a_failure_describes_as_its_error_does() {
    let failure = UpdateFailure::signature(AppError::updater(
        "update signature verification failed",
        std::io::Error::other("signature did not verify"),
    ));

    assert_eq!(
        describe(&failure),
        "Updater error: update signature verification failed: signature did not verify"
    );
}

/// Read by the `Settings.update-failed-reason(kind)` switch in `settings.slint`; a value
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
