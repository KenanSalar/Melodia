//! Tests for the skip-list the backfill keeps between sweeps.
//!
//! The set is what stops a library full of loosely-tagged tracks being re-queried against
//! `ListenBrainz` on every `library_changed` bump and on every launch, so the two directions it
//! can fail in are opposite: forget it and the task hammers the endpoint, strand it and tracks
//! the user has since retagged are skipped with no way back but the manual kick. Both halves
//! were prose in the module doc and nothing else until these.

use tempfile::TempDir;

use super::*;
use melodia_core::error::AppError;

/// The file the task keeps this in, under whatever root `Paths` resolved.
fn state_path(tmp: &TempDir) -> std::path::PathBuf {
    tmp.path().join("scrobble_mbid_attempted.json")
}

/// A miss has to still be a miss after a restart. That is the whole reason the set is on disk
/// rather than in memory, and the module doc and the rule describing it once disagreed about
/// exactly that.
#[tokio::test]
async fn a_persisted_attempted_set_survives_a_restart() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = state_path(&tmp);

    let attempted: HashSet<i64> = [7, 11, 13].into_iter().collect();
    persist_attempted(&path, &attempted).await;

    assert!(path.exists(), "the next launch has nothing to skip without a file");
    assert_eq!(
        load_attempted(&path),
        attempted,
        "compared as a set: the ids leave a HashSet, so the array order on disk is not ours"
    );
    Ok(())
}

#[test]
fn an_absent_attempted_file_loads_as_an_empty_set() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    assert!(load_attempted(&state_path(&tmp)).is_empty(), "a first launch skips nothing");
    Ok(())
}

/// Every way of failing to read the file folds to the same answer, and it has to be the empty
/// one: a fresh sweep costs a round of lookups, where a half-read set would silently skip tracks
/// nobody can name and the button that clears it would look like it had done nothing.
#[test]
fn an_unreadable_attempted_file_loads_as_an_empty_set() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = state_path(&tmp);

    for corrupt in ["{ not json", r#"{"ids": [1, 2]}"#, r#"["1", "2"]"#] {
        std::fs::write(&path, corrupt)?;
        assert!(
            load_attempted(&path).is_empty(),
            "{corrupt:?} must fall back to a re-sweep, not to a skip-list nobody can clear"
        );
    }
    Ok(())
}

/// The kick's two lines, in the order the loop spells them. Clearing only the in-memory set
/// leaves the file to resurrect it on the next launch, which is the failure the button exists
/// to prevent.
#[tokio::test]
async fn the_manual_kick_clears_the_file_as_well_as_the_set() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = state_path(&tmp);

    let mut attempted: HashSet<i64> = [1, 2, 3].into_iter().collect();
    persist_attempted(&path, &attempted).await;

    attempted.clear();
    persist_attempted(&path, &attempted).await;

    assert!(path.exists(), "the kick rewrites the file, it does not remove it");
    assert!(
        load_attempted(&path).is_empty(),
        "the next launch must re-look-up everything rather than reload what the kick cleared"
    );
    Ok(())
}

/// Three outcomes the user can tell apart, asserted on the fragment that distinguishes each arm
/// rather than on the whole sentence: the arm is the behaviour, the rest is copy.
#[test]
fn summarize_tells_the_three_outcomes_apart() {
    let nothing_to_do = summarize(&SweepOutcome {
        looked_up: 0,
        tagged: 0,
    });
    let all_missed = summarize(&SweepOutcome {
        looked_up: 5,
        tagged: 0,
    });
    let some_tagged = summarize(&SweepOutcome {
        looked_up: 5,
        tagged: 3,
    });

    assert!(nothing_to_do.contains("already have"), "got {nothing_to_do:?}");
    assert!(all_missed.contains("No matches"), "got {all_missed:?}");
    assert!(
        all_missed.contains("5 track(s)"),
        "the count is what makes a miss actionable: {all_missed:?}"
    );
    assert!(some_tagged.contains("Tagged 3 of 5"), "got {some_tagged:?}");
}

/// The task subscribes to `library_changed`, so a bump anywhere on the path it writes through is
/// a loop that re-wakes the sweep for the rest of the session. `write_resolved_mbids` takes an
/// `&AppState` and so has the channel in reach; nothing but this notices one being added.
#[test]
fn the_backfill_never_bumps_the_channel_it_subscribes_to() {
    const SOURCES: [(&str, &str, &str); 2] = [
        ("tasks/mbid_backfill.rs", include_str!("../mbid_backfill.rs"), "async fn run_sweep"),
        (
            "library/mbid.rs",
            include_str!("../../library/mbid.rs"),
            "pub async fn write_resolved_mbids",
        ),
    ];

    for (name, source, anchor) in SOURCES {
        let code = melodia_testkit::strip_line_comments(source);
        assert!(
            code.contains(anchor),
            "{name} no longer defines `{anchor}`, so this is reading a file that moved out from \
             under it rather than the write path it names"
        );
        assert!(
            !code.contains("bump("),
            "{name} bumps a signal, and the backfill subscribes to `library_changed`: a bump on \
             its own write path wakes the sweep again and it never settles"
        );
    }
}

// --- What a refused batch costs ---------------------------------------------
//
// The skip-list above is what the sweep reads; this is what writes it. Four outcomes, and the two
// that decide anything are the refusals that disagree about retiring the chunk: a 429 has to keep
// its ids, because the lookup it was refused never happened, and a 500 has to give them up,
// because the alternative is a sweep that spends its one pass per launch on the same bad batch.
// They sit one `matches!` arm apart.

/// The transient arm, standing in for anything the endpoint answers that is neither a 401 nor a
/// 429. `Transport` is the other member of the same partition and takes the same row.
fn server_error() -> Result<Vec<Option<listenbrainz::MbidMatch>>, ListenBrainzError> {
    Err(ListenBrainzError::Server {
        status: 503,
        message: "unavailable".to_owned(),
    })
}

/// A rate limit is the one refusal that must not retire its chunk: those tracks were never asked
/// about, and an id in the attempted set is out of reach of every later sweep until the user
/// finds the manual button.
#[test]
fn only_a_rate_limit_keeps_the_chunk_it_was_refused_over() {
    let rows: Vec<(&str, bool)> = vec![
        ("answered", BatchStep::for_outcome(&Ok(vec![None])).retires_chunk()),
        (
            "rate limited",
            BatchStep::for_outcome(&Err(ListenBrainzError::RateLimited {
                reset_in_secs: Some(9),
            }))
            .retires_chunk(),
        ),
        ("server error", BatchStep::for_outcome(&server_error()).retires_chunk()),
    ];

    assert_eq!(
        rows,
        vec![
            ("answered", true),
            ("rate limited", false),
            ("server error", true)
        ]
    );
}

/// A rejected token ends the sweep. Answering it like a server error would ask the endpoint for
/// every remaining chunk with a credential it has already refused.
#[test]
fn a_rejected_token_ends_the_sweep_where_a_server_error_does_not() {
    let rejected = BatchStep::for_outcome(&Err(ListenBrainzError::InvalidToken));

    assert_eq!(rejected.wait_before_next(), None);
    assert!(BatchStep::for_outcome(&server_error()).wait_before_next().is_some());
}

/// The wait comes from the header the server sent rather than from a constant of ours, and
/// `rate_limit_backoff` is what bounds it. A fixed sleep here would either hammer a server that
/// asked for a minute or park for a minute over one that asked for a second.
#[test]
fn the_throttled_wait_is_the_one_the_server_asked_for() {
    let step = BatchStep::for_outcome(&Err(ListenBrainzError::RateLimited {
        reset_in_secs: Some(7),
    }));

    assert_eq!(step, BatchStep::Throttled(std::time::Duration::from_secs(7)));
    assert_eq!(step.wait_before_next(), Some(listenbrainz::rate_limit_backoff(Some(7))));
}

/// An answered batch pauses and a skipped one does not, because the pause is there for a server
/// that answered: a refusal already cost whatever the server spent refusing it.
#[test]
fn only_an_answered_batch_pays_the_pacing_pause() {
    assert_eq!(BatchStep::for_outcome(&Ok(vec![])).wait_before_next(), Some(BATCH_PAUSE));
    assert_eq!(BatchStep::for_outcome(&server_error()).wait_before_next(), Some(Duration::ZERO));
}
