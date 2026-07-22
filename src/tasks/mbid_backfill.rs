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
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::database::queries;
use crate::error::AppResult;
use crate::library::mbid;
use crate::services::scrobble::ScrobbleService;
use crate::services::scrobble::providers::listenbrainz::{
    self, ListenBrainzError, LookupQuery, MAX_LOOKUPS_PER_POST,
};
use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Fallback wait when a 429 gives no `X-RateLimit-Reset-In`.
const DEFAULT_BACKOFF_SECS: u64 = 30;
/// Cap on a single rate-limit wait so a misbehaving header can't park the task
/// for minutes; a longer real window just costs one extra retry.
const MAX_BACKOFF_SECS: u64 = 300;
/// Gentle pause between successful batches — `ListenBrainz` is load-sensitive, so
/// pace lookups rather than sprint into a 429.
const BATCH_PAUSE: Duration = Duration::from_millis(300);

/// Spawn the backfill loop on the shared task lifecycle so shutdown waits for an
/// in-flight batch's DB commit.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let state = state.clone();
    let service = state.scrobble.clone();
    let mut lib_rx = state.library_changed_tx.subscribe();

    spawner.spawn_cancellable(move |shutdown| async move {
        let mut attempted: HashSet<i64> = HashSet::new();

        // Boot sweep: tags an already-enabled library (and is a no-op otherwise).
        run_sweep(&state, &service, &shutdown, &mut attempted).await;

        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                changed = lib_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = lib_rx.borrow_and_update();
                    // A real scan/import: resolve only the not-yet-attempted rows.
                    run_sweep(&state, &service, &shutdown, &mut attempted).await;
                }
                () = service.mbid_kicked() => {
                    // Enable toggle / manual button: force a full re-sweep.
                    attempted.clear();
                    run_sweep(&state, &service, &shutdown, &mut attempted).await;
                }
            }
        }
        log::info!("MBID backfill task stopped");
    });

    log::info!("MBID backfill task started");
}

/// Gate on the enabled+connected shadow, then sweep. A no-op when auto-tagging is
/// off or `ListenBrainz` isn't connected.
async fn run_sweep(
    state: &AppState,
    service: &ScrobbleService,
    shutdown: &CancellationToken,
    attempted: &mut HashSet<i64>,
) {
    let Some(token) = service.mbid_lookup_token() else {
        return;
    };
    if let Err(e) = backfill(state, &token, shutdown, attempted).await {
        log::warn!("MBID backfill failed: {e}");
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
) -> AppResult<()> {
    let pending: Vec<_> = queries::track::get_tracks_missing_mbid(&state.db)
        .await?
        .into_iter()
        .filter(|(id, ..)| !attempted.contains(id))
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    log::info!("MBID backfill: {} track(s) to look up", pending.len());

    let client = state.http_client().clone();
    let mut idx = 0;
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
                for (id, ..) in chunk {
                    attempted.insert(*id);
                }
                if !resolved.is_empty() {
                    match mbid::write_resolved_mbids(state, &resolved).await {
                        Ok(n) => written += n,
                        Err(e) => log::warn!("MBID backfill write failed: {e}"),
                    }
                }
                idx = end;
                if shutdown
                    .run_until_cancelled(tokio::time::sleep(BATCH_PAUSE))
                    .await
                    .is_none()
                {
                    break;
                }
            }
            Err(ListenBrainzError::RateLimited { reset_in_secs }) => {
                let secs = reset_in_secs.unwrap_or(DEFAULT_BACKOFF_SECS).min(MAX_BACKOFF_SECS);
                log::info!("MBID backfill rate-limited; waiting {secs}s");
                if shutdown
                    .run_until_cancelled(tokio::time::sleep(Duration::from_secs(secs)))
                    .await
                    .is_none()
                {
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
                idx = end;
            }
        }
    }

    if written > 0 {
        log::info!("MBID backfill: tagged {written} track(s)");
    }
    Ok(())
}
