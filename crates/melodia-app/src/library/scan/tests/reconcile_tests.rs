//! Which folders a pass reads, and how a request merges into the pass already running.

use parking_lot::Mutex;

use super::{PassClaim, PassState, Reach};
use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;

fn folder(is_enabled: bool, last_scanned: Option<&str>) -> Folder {
    Folder {
        id: 1,
        path: "music".to_owned(),
        is_enabled,
        last_scanned: last_scanned.map(str::to_owned),
        added_at: String::new(),
    }
}

/// Watching off scans only the imports a quit cut short, and `last_scanned` is stamped by a
/// completed scan alone, so it is what tells one of those from a finished folder.
#[test]
fn a_pass_reads_enabled_folders_and_the_narrow_one_only_unfinished_imports() {
    let finished = Some("2026-10-01T00:00:00Z");
    let cases = [
        (Reach::Library, true, finished, true),
        (Reach::Library, true, None, true),
        (Reach::Library, false, None, false),
        (Reach::UnfinishedImports, true, None, true),
        (Reach::UnfinishedImports, true, finished, false),
        (Reach::UnfinishedImports, false, None, false),
    ];

    for (reach, is_enabled, last_scanned, covered) in cases {
        assert_eq!(
            reach.covers(&folder(is_enabled, last_scanned)),
            covered,
            "{reach:?} over an enabled={is_enabled} folder last scanned {last_scanned:?}"
        );
    }
}

fn idle() -> Mutex<PassState> {
    Mutex::new(PassState::Idle)
}

/// A claim on an idle lock, which nothing refuses.
fn claim(pass: &Mutex<PassState>, reach: Reach) -> Result<PassClaim<'_>, AppError> {
    PassClaim::take(pass, reach)
        .ok_or_else(|| AppError::Validation("an idle pass refused its first claim".into()))
}

/// A second pass beside the first would walk and stat every file again, and the two would
/// clobber the one progress bar.
#[test]
fn a_request_while_a_pass_runs_merges_into_it() -> Result<(), AppError> {
    let pass = idle();
    let _running = claim(&pass, Reach::Library)?;

    assert!(PassClaim::take(&pass, Reach::Library).is_none());
    Ok(())
}

/// A narrower round doesn't read the folders a library request is for, so dropping the request
/// would leave changes on disk unread until the next launch.
#[test]
fn a_library_request_during_a_narrower_pass_is_owed_a_round() -> Result<(), AppError> {
    let pass = idle();
    let mut running = claim(&pass, Reach::UnfinishedImports)?;
    let _merged = PassClaim::take(&pass, Reach::Library);

    assert_eq!(running.next_round(Reach::UnfinishedImports), Some(Reach::Library));
    Ok(())
}

/// The repair widens a narrow round to the library once it clears a cover, and that round already
/// read what the request asked for.
#[test]
fn a_round_widened_to_the_library_settles_what_it_was_owed() -> Result<(), AppError> {
    let pass = idle();
    let mut running = claim(&pass, Reach::UnfinishedImports)?;
    let _merged = PassClaim::take(&pass, Reach::Library);

    assert_eq!(running.next_round(Reach::Library), None);
    Ok(())
}

/// The owed round runs under the same claim, so a request arriving during it merges rather than
/// starting a pass beside it.
#[test]
fn an_owed_round_still_holds_the_pass() -> Result<(), AppError> {
    let pass = idle();
    let mut running = claim(&pass, Reach::UnfinishedImports)?;
    let _merged = PassClaim::take(&pass, Reach::Library);
    running.next_round(Reach::UnfinishedImports);

    assert!(PassClaim::take(&pass, Reach::Library).is_none());
    Ok(())
}

/// Let go under the same lock that found nothing owed, which is what keeps a request landing
/// then from merging into a pass that has already ended.
#[test]
fn a_pass_owed_nothing_lets_the_next_request_start_one() -> Result<(), AppError> {
    let pass = idle();
    let mut running = claim(&pass, Reach::Library)?;
    running.next_round(Reach::Library);

    assert!(PassClaim::take(&pass, Reach::Library).is_some());
    Ok(())
}

/// A pass stopped by a cancel, or by a panic, leaves without asking for its next round.
#[test]
fn a_pass_that_ends_early_lets_go_of_the_lock() -> Result<(), AppError> {
    let pass = idle();
    let running = claim(&pass, Reach::Library)?;

    drop(running);

    assert!(PassClaim::take(&pass, Reach::Library).is_some());
    Ok(())
}
