//! Auto-tag backfill: resolve the `MusicBrainz` Recording ID for tracks that lack
//! one (via `ListenBrainz`'s `metadata/lookup`) and write it into the file + DB, so
//! `ListenBrainz` loves — which key on that id — work on an untagged library.
//!
//! Inert until the user enables "Add `MusicBrainz` IDs" **and** `ListenBrainz` is
//! connected (the lookup endpoint needs the token). Triggers: a boot/enable
//! sweep, a `library_changed_tx` subscription (new imports get resolved), and a
//! manual kick from the Settings button. No `ui::*` imports.
//!
//! An in-memory `attempted` set keeps unmatched tracks from being re-looked-up on
//! every subsequent `library_changed` bump — they stay NULL in the DB but are
//! skipped until the next full sweep (a manual kick clears the set). The writer
//! deliberately doesn't bump `library_changed_tx`, so this task never wakes
//! itself.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::database::queries;
use crate::error::AppResult;
use crate::library::mbid;
use crate::services::scrobble::ScrobbleService;
use crate::services::scrobble::providers::listenbrainz::{
    self, ListenBrainzError, LookupQuery, MAX_LOOKUPS_PER_POST,
};
use crate::services::toast::{self, ToastKind};
use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Gentle pause between successful batches — `ListenBrainz` is load-sensitive, so
/// pace lookups rather than sprint into a 429.
const BATCH_PAUSE: Duration = Duration::from_millis(300);

/// What one sweep accomplished, for logging + the user-facing toast.
struct SweepOutcome {
    /// Tracks whose lookup completed this sweep (matched or not).
    looked_up: usize,
    /// Tracks that got a Recording ID written.
    tagged: usize,
}

/// Spawn the backfill loop on the shared task lifecycle so shutdown waits for an
/// in-flight batch's DB commit.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let state = state.clone();
    let service = state.scrobble.clone();
    let mut lib_rx = state.library_changed_tx.subscribe();

    spawner.spawn_cancellable(move |shutdown| async move {
        // Persisted across restarts so an unmatched track isn't re-looked-up on
        // every launch — only new/not-yet-attempted rows are. A manual kick clears
        // it (below) for a genuine full re-sweep.
        let state_path = state.paths.scrobble_mbid_state_path.clone();
        let mut attempted: HashSet<i64> = load_attempted(&state_path);

        // Boot sweep: tags an already-enabled library (and is a no-op otherwise).
        // Silent — the user didn't ask for it just now.
        run_sweep(&state, &service, &shutdown, &mut attempted, false).await;

        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                changed = lib_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = lib_rx.borrow_and_update();
                    // A real scan/import: resolve only the not-yet-attempted rows,
                    // silently (a toast on every import would be noise).
                    run_sweep(&state, &service, &shutdown, &mut attempted, false).await;
                }
                () = service.mbid_kicked() => {
                    // Enable toggle / manual button: force a full re-sweep and
                    // report the result — this one the user triggered on purpose.
                    // Clear the persisted set first so a crash mid-sweep re-sweeps
                    // rather than resurrecting the stale skip-list.
                    attempted.clear();
                    persist_attempted(&state_path, &attempted).await;
                    run_sweep(&state, &service, &shutdown, &mut attempted, true).await;
                }
            }
        }
        log::info!("MBID backfill task stopped");
    });

    log::info!("MBID backfill task started");
}

/// Gate on the enabled+connected shadow, then sweep. A no-op when auto-tagging is
/// off or `ListenBrainz` isn't connected. `announce` toasts the result — set only
/// for user-triggered sweeps (enable toggle / "Look up missing IDs" button).
async fn run_sweep(
    state: &AppState,
    service: &ScrobbleService,
    shutdown: &CancellationToken,
    attempted: &mut HashSet<i64>,
    announce: bool,
) {
    let Some(token) = service.mbid_lookup_token() else {
        return;
    };
    match backfill(state, &token, shutdown, attempted).await {
        Ok(outcome) => {
            // Always log the outcome — a zero-match sweep used to be silent, which
            // read as "nothing happened / stuck".
            log::info!("MBID backfill: looked up {}, tagged {}", outcome.looked_up, outcome.tagged);
            // Anything looked up grew the attempted set (only not-yet-attempted
            // rows are queried) — persist so the next launch skips them.
            if outcome.looked_up > 0 {
                persist_attempted(&state.paths.scrobble_mbid_state_path, attempted).await;
            }
            if announce {
                toast::notify(ToastKind::MbidTagging, summarize(&outcome));
            }
        }
        Err(e) => {
            log::warn!("MBID backfill failed: {e}");
            if announce {
                toast::notify(ToastKind::MbidTagging, "Couldn't look up MusicBrainz IDs");
            }
        }
    }
}

/// A one-line, human summary of a sweep for the toast body. English-only, like
/// every other dynamic toast detail (the localized part is the title).
fn summarize(outcome: &SweepOutcome) -> String {
    match outcome {
        SweepOutcome { looked_up: 0, .. } => {
            "All eligible tracks already have a MusicBrainz ID".to_owned()
        }
        SweepOutcome {
            tagged: 0,
            looked_up,
        } => format!("No matches — {looked_up} track(s) had tags MusicBrainz couldn't identify"),
        SweepOutcome { tagged, looked_up } => {
            format!("Tagged {tagged} of {looked_up} track(s)")
        }
    }
}

/// Resolve + write in ≤[`MAX_LOOKUPS_PER_POST`] batches, honoring rate limits and
/// shutdown. Marks every processed id (matched or not) as attempted so unmatched
/// tracks aren't retried until the next full sweep.
async fn backfill(
    state: &AppState,
    token: &str,
    shutdown: &CancellationToken,
    attempted: &mut HashSet<i64>,
) -> AppResult<SweepOutcome> {
    let pending: Vec<_> = queries::track::get_tracks_missing_mbid(&state.db)
        .await?
        .into_iter()
        .filter(|(id, ..)| !attempted.contains(id))
        .collect();
    if pending.is_empty() {
        return Ok(SweepOutcome {
            looked_up: 0,
            tagged: 0,
        });
    }
    log::info!("MBID backfill: {} track(s) to look up", pending.len());

    let client = state.http_client().clone();
    let mut idx = 0;
    let mut looked_up = 0usize;
    let mut written = 0usize;

    while idx < pending.len() && !shutdown.is_cancelled() {
        let end = (idx + MAX_LOOKUPS_PER_POST).min(pending.len());
        let chunk = &pending[idx..end];
        let lookups: Vec<LookupQuery> = chunk
            .iter()
            .map(|(_id, _path, artist, title, album)| LookupQuery {
                artist,
                title,
                release: album.as_deref(),
            })
            .collect();

        let Some(result) = shutdown
            .run_until_cancelled(listenbrainz::lookup_recording_mbids_bulk(
                &client, token, &lookups,
            ))
            .await
        else {
            break; // cancelled mid-request
        };

        match result {
            Ok(matches) => {
                let resolved: Vec<mbid::ResolvedMbid> = chunk
                    .iter()
                    .zip(matches)
                    .filter_map(|((id, path, ..), matched)| {
                        matched.map(|m| (*id, path.clone(), m.recording_mbid))
                    })
                    .collect();
                log::debug!(
                    "MBID backfill batch: {} looked up, {} matched",
                    chunk.len(),
                    resolved.len()
                );
                for (id, ..) in chunk {
                    attempted.insert(*id);
                }
                if !resolved.is_empty() {
                    match mbid::write_resolved_mbids(state, &resolved).await {
                        Ok(n) => written += n,
                        Err(e) => log::warn!("MBID backfill write failed: {e}"),
                    }
                }
                looked_up += chunk.len();
                idx = end;
                if shutdown.run_until_cancelled(tokio::time::sleep(BATCH_PAUSE)).await.is_none() {
                    break;
                }
            }
            Err(ListenBrainzError::RateLimited { reset_in_secs }) => {
                let backoff = listenbrainz::rate_limit_backoff(reset_in_secs);
                log::info!("MBID backfill rate-limited; waiting {}s", backoff.as_secs());
                if shutdown.run_until_cancelled(tokio::time::sleep(backoff)).await.is_none() {
                    break;
                }
                // Retry the same chunk (idx unchanged).
            }
            Err(ListenBrainzError::InvalidToken) => {
                log::warn!("MBID backfill: ListenBrainz token rejected; stopping sweep");
                break;
            }
            Err(e) => {
                // Transient/server error: skip this batch rather than spin on it.
                log::warn!("MBID backfill lookup error: {e}");
                for (id, ..) in chunk {
                    attempted.insert(*id);
                }
                looked_up += chunk.len();
                idx = end;
            }
        }
    }

    Ok(SweepOutcome {
        looked_up,
        tagged: written,
    })
}

/// Load the persisted attempted-id set, defaulting to empty on a missing or
/// unreadable file (a fresh sweep is the safe fallback). Stored as a plain id
/// array for stable file contents.
fn load_attempted(path: &Path) -> HashSet<i64> {
    crate::services::load_json_or_default_sync::<Vec<i64>>(path)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Persist the attempted-id set off the async runtime (blocking `std::fs`).
/// Best-effort: a failed write just means the next launch re-looks-up those ids.
async fn persist_attempted(path: &Path, attempted: &HashSet<i64>) {
    let path = path.to_path_buf();
    let ids: Vec<i64> = attempted.iter().copied().collect();
    match tokio::task::spawn_blocking(move || crate::services::write_json_atomic_sync(&path, &ids))
        .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::warn!("MBID backfill: failed to persist attempted set: {e}"),
        Err(e) => log::warn!("MBID backfill: attempted-set persist task panicked: {e}"),
    }
}
