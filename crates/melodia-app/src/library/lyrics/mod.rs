//! Lyrics for the playing track, and the one door onto them.
//!
//! A sidecar, the lyrics tag, a store of past directory answers and the directory itself resolve
//! through [`for_track`], so each of two switches has one place to guard: the feature's in front
//! of all four, the lookup's in front of the directory. **`is_enabled` and `online_lookup_enabled`
//! may not move**, each being the one reader of its flag (`crates/melodia/tests/lyrics_switch.rs`).
//!
//! **The order is sidecar, tag, store, lookup, and a timed sheet cuts across it** wherever the
//! sidecar hasn't spoken (`docs/adr/0038-lyrics-display-reads-the-file.md`). The store sits on the
//! local side of the switch: turning the lookup off buys no traffic, not no lyrics.
//!
//! Reachable with lyrics off: `forget_all`, so a retag leaves no stale answer waiting for the
//! feature to come back, and the tag editor's `resident_text` and `write_to_tag`.

mod embedded;
mod lrc;
mod online;
mod romanize;
mod sidecar;
mod store;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::entities::lyrics::{Lyrics, LyricsOutcome};
use melodia_core::entities::tags::{FieldEdit, TagEdit};
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;
use melodia_net::services::net::pacer::RequestPacer;

/// What this track has, from whichever source has it, ready to draw, in at most two blocking hops:
/// the local reads together, then the romanization.
pub async fn for_track(state: &AppState, track: &TrackSummary) -> Result<LyricsOutcome, AppError> {
    ensure_enabled(state)?;
    romanized(state, resolve(state, track).await?).await
}

/// Whether lyrics run at all. Now Playing's reseed and close ask this too, so the view asking and
/// this door answering can't disagree.
pub fn is_enabled(state: &AppState) -> bool {
    state.lyrics_enabled.get()
}

/// A refusal rather than [`LyricsOutcome::Absent`]: the panel is unmounted while lyrics are off,
/// so a call still arriving is a stale edge worth a log line.
fn ensure_enabled(state: &AppState) -> Result<(), AppError> {
    if is_enabled(state) {
        return Ok(());
    }
    Err(AppError::settings_msg("Lyrics are switched off"))
}

/// The romanization drawn under each line, filled off the UI thread whatever the panel's toggle
/// says: the toggle is a display filter, and filling lazily would put the pass on the thread that
/// draws. Here rather than in each of [`resolve`]'s four arms.
async fn romanized(state: &AppState, outcome: LyricsOutcome) -> Result<LyricsOutcome, AppError> {
    let LyricsOutcome::Sheet(mut sheet) = outcome else {
        return Ok(outcome);
    };
    if sheet.lines.iter().all(|line| line.text.is_ascii()) {
        return Ok(LyricsOutcome::Sheet(sheet));
    }

    let sheet = state
        .runtime
        .spawn_blocking(move || {
            romanize::apply(&mut sheet.lines);
            sheet
        })
        .await
        .map_err(AppError::io_source)?;
    Ok(LyricsOutcome::Sheet(sheet))
}

/// Which of the four sources answers for this track.
async fn resolve(state: &AppState, track: &TrackSummary) -> Result<LyricsOutcome, AppError> {
    let path = PathBuf::from(&track.file_path);
    let paths = Arc::clone(&state.paths);
    let track_path = track.file_path.clone();

    let local = state
        .runtime
        .spawn_blocking(move || read_local(&path, &paths.lyrics_dir, &track_path))
        .await
        .map_err(AppError::io_source)??;

    let Local { own, is_sidecar, stored } = local;
    let own = match own {
        // A sidecar is the user's own, and a timed sheet is the best a lookup could answer with.
        Some(own) if is_sidecar || own.is_synced() => return Ok(LyricsOutcome::Sheet(own)),
        own => own,
    };
    // The directory has answered before, and asking again would fetch the same answer.
    if let Some(stored) = stored {
        return Ok(timed_first(stored, own));
    }
    if !online_lookup_enabled(state) {
        return Ok(own.map_or(LyricsOutcome::Absent, LyricsOutcome::Sheet));
    }

    let fetched = online::look_up(state, track).await?;
    Ok(timed_first(fetched, own))
}

/// The sheet this track already has, as raw text for the tag editor to write into the file.
/// **Local only**, so a dialog over a selection spends no request per track; raw, since
/// re-serializing a parsed sheet would drop its `[offset:]` tag and gloss separator.
pub async fn resident_text(state: &AppState, track_path: &str) -> Result<Option<String>, AppError> {
    let path = PathBuf::from(track_path);
    let paths = Arc::clone(&state.paths);
    let track_path = track_path.to_owned();

    state
        .runtime
        .spawn_blocking(move || read_local_text(&path, &paths.lyrics_dir, &track_path))
        .await
        .map_err(AppError::io_source)?
}

/// [`read_local`]'s three homes as text, **in [`resolve`]'s order**, so the editor is offered the
/// sheet the panel shows. Blocking.
fn read_local_text(
    path: &Path,
    lyrics_dir: &Path,
    track_path: &str,
) -> Result<Option<String>, AppError> {
    if let Some(text) = sidecar::read_text(path)? {
        return Ok(Some(text));
    }
    let own = crate::library::tags::read_lyrics(path)?;
    let stored = store::read_text(lyrics_dir, track_path);

    if own.as_deref().is_some_and(lrc::is_timed) {
        return Ok(own);
    }
    if stored.as_deref().is_some_and(lrc::is_timed) {
        return Ok(stored);
    }
    Ok(own.or(stored))
}

/// What the local sources had: all three answers rather than the first, since a sidecar ends the
/// question where a plain lyrics tag only floors it.
struct Local {
    /// The sidecar where there is one, otherwise the lyrics tag.
    own: Option<Lyrics>,
    is_sidecar: bool,
    /// What the directory last said, where it has been asked.
    stored: Option<LyricsOutcome>,
}

/// The better of what the directory had and what the file carries: a timed sheet whatever wrote
/// it, then the file's own, so a directory calling the recording instrumental loses to a tag with
/// words in it.
fn timed_first(fetched: LyricsOutcome, own: Option<Lyrics>) -> LyricsOutcome {
    let Some(own) = own else {
        return fetched;
    };
    match &fetched {
        LyricsOutcome::Sheet(sheet) if sheet.is_synced() => fetched,
        _ => LyricsOutcome::Sheet(own),
    }
}

/// The three sources that need no network, in the order a user's own file wins. Blocking.
fn read_local(path: &Path, lyrics_dir: &Path, track_path: &str) -> Result<Local, AppError> {
    if let Some(lyrics) = sidecar::read(path)? {
        return Ok(Local { own: Some(lyrics), is_sidecar: true, stored: None });
    }
    let own = embedded::read(path)?;
    // Nothing stored could better a timed tag.
    let stored = match &own {
        Some(lyrics) if lyrics.is_synced() => None,
        _ => store::read(lyrics_dir, track_path),
    };
    Ok(Local { own, is_sidecar: false, stored })
}

/// Whether a track with no sheet of its own may be looked up online. `false` ends in
/// [`LyricsOutcome::Absent`] rather than a refusal, being the ordinary state of most libraries.
fn online_lookup_enabled(state: &AppState) -> bool {
    state.lyrics_online_enabled.get()
}

/// Forget what the directory said about this track, so the next lookup asks again: the whole of
/// what a refresh is. Only the store; a sidecar and a lyrics tag are the user's own files.
pub async fn forget(state: &AppState, track_path: &str) -> Result<(), AppError> {
    forget_all(state, vec![track_path.to_owned()]).await
}

/// [`forget`] over a retagged batch, in one hop. Every path is attempted, one failure being no
/// reason to leave the rest stale, and the first error comes back.
pub async fn forget_all(state: &AppState, track_paths: Vec<String>) -> Result<(), AppError> {
    let paths = Arc::clone(&state.paths);

    state
        .runtime
        .spawn_blocking(move || {
            let mut first_error = None;
            for track_path in &track_paths {
                if let Err(e) = store::forget(&paths.lyrics_dir, track_path) {
                    first_error = first_error.or(Some(e));
                }
            }
            first_error.map_or(Ok(()), Err)
        })
        .await
        .map_err(AppError::io_source)?
}

/// Write `text` into the track's own lyrics tag, as an ordinary tag edit so the scan pipeline
/// re-reads the file. One spelling for the Edit Tags dialog and the Now Playing menu.
pub async fn write_to_tag(state: &AppState, track_id: i64, text: &str) -> Result<(), AppError> {
    let edit = TagEdit { lyrics: FieldEdit::Set(text.to_owned()), ..TagEdit::default() };
    let report = crate::library::tags::apply_tag_edit(state, vec![track_id], edit, None).await?;

    // A per-file refusal comes back inside the `Ok`, and would otherwise read as a save.
    if let Some((_, reason)) = report.failures.first() {
        return Err(AppError::io_other(reason.clone()));
    }
    if report.updated == 0 {
        return Err(AppError::io_other("The lyrics could not be written to the file"));
    }
    Ok(())
}

/// A fresh directory pacer for `AppState` to hold, built here so the directory client is named
/// only behind this door.
#[must_use]
pub fn pacer() -> RequestPacer {
    melodia_net::services::net::lyrics_directory::pacer()
}

/// Hold the on-disk store to its bounds. Blocking; `tasks::lyrics_cache` owns the hop.
pub fn prune_store(paths: &Paths) -> Result<u32, AppError> {
    store::prune(&paths.lyrics_dir)
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
