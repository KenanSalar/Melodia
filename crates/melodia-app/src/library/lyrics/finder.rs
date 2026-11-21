//! The lyrics finder: every sheet the directory has for a track, asked with a query a person can
//! edit, and the one they pick stored as the track's online answer.
//!
//! Behind the feature's and the lookup's switches, not the automatic one, and writing the store a
//! lookup writes, so a pick is what later plays read back, as an automatic answer would be.

use std::sync::Arc;
use std::time::Duration;

use super::online::credit_for;
use super::store::{self, Fetched, LyricsKey};
use super::{Local, ensure_enabled, local, lrc, online_lookup_enabled};
use crate::state::AppState;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::lyrics::{LyricsCandidate, LyricsSource, Timing};
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;
use melodia_core::utils::text::filled;
use melodia_net::services::net::lyrics_directory::{self as directory, LookupError};

/// What a candidate carries, which is what a person tells candidates apart by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceKind {
    Words,
    Lines,
    Plain,
    Instrumental,
}

/// One candidate, read once so the dialog draws it without parsing a sheet.
#[derive(Debug, Clone)]
pub struct Choice {
    pub candidate: LyricsCandidate,
    pub kind: ChoiceKind,
    /// Its lines, without stamps or glosses.
    pub preview: String,
}

/// Why a search has nothing to show. A refusal for a period stands apart, nothing but waiting
/// fixing it.
#[derive(Debug)]
pub enum SearchFailure {
    RateLimited { retry_after: Option<Duration> },
    Failed(AppError),
}

/// What the panel shows ahead of a stored pick, which ranks below the track's own lyrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outranking {
    Nothing,
    Sidecar,
    TimedTag,
    PlainTag,
}

impl Outranking {
    /// Whether a pick of this kind stays out of sight, on `resolve`'s terms: only a timed pick
    /// shows past a plain tag.
    #[must_use]
    pub fn hides(self, kind: ChoiceKind) -> bool {
        match self {
            Self::Sidecar | Self::TimedTag => true,
            Self::PlainTag => matches!(kind, ChoiceKind::Plain | ChoiceKind::Instrumental),
            Self::Nothing => false,
        }
    }
}

/// The query a search for this track opens on.
pub async fn default_query(state: &AppState, track: &TrackSummary) -> String {
    directory::query_for(&track.title, &credit_of(state, track).await)
}

/// Every candidate the directory has for `query`, best first for this track.
pub async fn search(
    state: &AppState,
    track: &TrackSummary,
    query: &str,
) -> Result<Vec<Choice>, SearchFailure> {
    ensure_enabled(state).map_err(SearchFailure::Failed)?;
    if !online_lookup_enabled(state) {
        return Err(SearchFailure::Failed(AppError::settings_msg(
            "Looking lyrics up online is switched off",
        )));
    }

    let credit = credit_of(state, track).await;
    let candidates = directory::search(
        state.http_client(),
        &state.lyrics_pacer,
        query,
        &track.title,
        &credit,
        track.duration_ms,
    )
    .await?;
    Ok(candidates.into_iter().filter_map(Choice::read).collect())
}

/// Stores the pick as the track's online answer, in place of whatever was stored before.
pub async fn choose(
    state: &AppState,
    track: LyricsKey,
    candidate: LyricsCandidate,
) -> Result<(), AppError> {
    ensure_enabled(state)?;
    let paths = Arc::clone(&state.paths);

    state
        .runtime
        .spawn_blocking(move || {
            // An old sheet would outlast a pick of "instrumental", being read first.
            store::forget(&paths.lyrics_dir, &track)?;
            store::write(&paths.lyrics_dir, &track, Fetched::of(&candidate.answer))
        })
        .await
        .map_err(AppError::io_source)?
}

/// Which of the track's own lyrics would be shown ahead of a pick.
pub async fn outranking(state: &AppState, track: LyricsKey) -> Result<Outranking, AppError> {
    ensure_enabled(state)?;
    Ok(match local(state, track).await? {
        Local { is_sidecar: true, .. } => Outranking::Sidecar,
        Local { own: Some(own), .. } if own.is_synced() => Outranking::TimedTag,
        Local { own: Some(_), .. } => Outranking::PlainTag,
        Local { own: None, .. } => Outranking::Nothing,
    })
}

async fn credit_of(state: &AppState, track: &TrackSummary) -> ArtistCredit {
    let printed = filled(track.artist.as_deref()).unwrap_or_default();
    credit_for(&state.db, track.id, printed).await
}

impl Choice {
    /// `None` for a candidate with neither words that parse nor an instrumental mark: nothing to
    /// pick.
    fn read(candidate: LyricsCandidate) -> Option<Self> {
        let Some(text) = candidate.answer.text() else {
            let instrumental = candidate.answer.instrumental;
            return instrumental.then(|| Self {
                candidate,
                kind: ChoiceKind::Instrumental,
                preview: String::new(),
            });
        };

        let lyrics = lrc::parse(text, LyricsSource::Online)?;
        let kind = match lyrics.timing() {
            Timing::Words => ChoiceKind::Words,
            Timing::Lines => ChoiceKind::Lines,
            Timing::Plain => ChoiceKind::Plain,
        };
        let preview =
            lyrics.lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n");
        Some(Self { candidate, kind, preview })
    }
}

impl From<LookupError> for SearchFailure {
    fn from(error: LookupError) -> Self {
        match error {
            LookupError::RateLimited { retry_after } => Self::RateLimited { retry_after },
            LookupError::Failed(e) => Self::Failed(e),
        }
    }
}

#[cfg(test)]
#[path = "tests/finder_tests.rs"]
mod tests;
