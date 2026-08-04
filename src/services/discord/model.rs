//! Pure Discord Rich Presence projection: turn the player's published
//! view-model into a [`Presence`] card, plus the dedupe rule that keeps
//! a state-change-only `watch` from becoming an IPC write per volume tap.
//!
//! No I/O, no locks, no clock reads — `now_ts` (UNIX **seconds**) is an input,
//! the same shape as [`crate::services::scrobble::detector`] and
//! `player::handlers::evaluate_playing_tick`. The impure task in
//! [`crate::tasks::discord_presence`] drives it; [`super::payload`] serializes
//! the [`Presence`] it produces and [`super::ipc`] ships the bytes.

use crate::entities::track::TrackSummary;
use crate::player::state::PlayerViewModelLight;
use crate::services::settings::DiscordFlags;

/// Fallback album line / large-image caption when a track is untagged.
const APP_NAME: &str = "Melodia";

/// Discord truncates `details`/`state` past 128 characters and rejects a value
/// shorter than 2, so we clamp both before the string leaves the model.
const MAX_FIELD_CHARS: usize = 128;
const MIN_FIELD_CHARS: usize = 2;

/// How far the anchored start may drift (seconds) and still count as the same
/// play. Absorbs the ~500 ms-stale monitor position and second truncation, so a
/// volume republish (same anchor) dedupes while a real seek re-anchors.
const ANCHOR_TOLERANCE_SECS: u64 = 2;

/// A fully-resolved presence card, owned so it can cross the worker channel.
/// Built by [`PresenceState`] from the view-model; serialized to the Discord
/// activity object by [`super::payload::set_activity_json`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    /// Card title (Discord `details`) — the song title.
    pub details: String,
    /// Second line (Discord `state`) — the artist; `None` when untagged.
    pub state: Option<String>,
    /// Large-image tooltip — the album, falling back to the app name.
    pub large_text: Option<String>,
    /// External `https://` cover URL for the large image; `None` uses the app
    /// logo asset. Populated by the detector task on a track change (the pure
    /// model leaves it `None` — the lookup is I/O).
    pub large_image: Option<String>,
    pub paused: bool,
    /// Progress-bar anchor `now - elapsed`, UNIX **seconds**. `None` while paused
    /// or with an unknown duration (no bar).
    pub start_ts: Option<u64>,
    /// Track end, UNIX seconds — `start_ts + duration`. Paired with `start_ts`.
    pub end_ts: Option<u64>,
}

/// What the detector decided to push. `Clear` removes the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Set(Presence),
    Clear,
}

/// Dedupe identity for the currently-shown card. `view_model` republishes on
/// *every* state change (volume included), so the task compares this against the
/// last emitted one and skips a write when nothing Discord-visible moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identity {
    /// Nothing on the card (also the initial state — Discord shows nothing).
    Cleared,
    /// Playing: track + anchored start (compared with a tolerance).
    Playing { track_id: i64, start_ts: u64 },
    /// Paused: no timestamps, so a seek-while-paused changes nothing.
    Paused { track_id: i64 },
}

impl Identity {
    /// Same card? `Playing` allows a small anchor drift; everything else is exact.
    fn matches(self, other: Identity) -> bool {
        match (self, other) {
            (Identity::Cleared, Identity::Cleared) => true,
            (Identity::Paused { track_id: a }, Identity::Paused { track_id: b }) => a == b,
            (
                Identity::Playing {
                    track_id: a,
                    start_ts: sa,
                },
                Identity::Playing {
                    track_id: b,
                    start_ts: sb,
                },
            ) => a == b && sa.abs_diff(sb) <= ANCHOR_TOLERANCE_SECS,
            _ => false,
        }
    }
}

/// Tracks the last card the task emitted so republishes that change nothing
/// visible don't become IPC writes. Pure — the task owns one and feeds it the
/// view-model.
#[derive(Debug)]
pub struct PresenceState {
    last: Identity,
}

impl Default for PresenceState {
    fn default() -> Self {
        Self {
            last: Identity::Cleared,
        }
    }
}

impl PresenceState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the last emitted card so the next evaluation always paints. Called
    /// by the task on the disabled→enabled edge (the shown card was cleared while
    /// off, but our dedupe state still remembers it).
    pub fn reset(&mut self) {
        self.last = Identity::Cleared;
    }

    /// Project the view-model into an [`Update`], or `None` when nothing should
    /// be sent — either the card is already current (dedupe) or we're in a
    /// `loading` transition and hold the previous card.
    pub fn on_view_model(
        &mut self,
        vm: Option<&PlayerViewModelLight>,
        now_ts: i64,
        flags: &DiscordFlags,
    ) -> Option<Update> {
        let (identity, decision) = classify(vm, now_ts, flags)?;
        if self.last.matches(identity) {
            return None;
        }
        self.last = identity;
        // Build the (allocating) card only now that this is a genuine change — a
        // deduped republish never pays for a presence it would immediately discard.
        let update = match decision {
            Decision::Clear => Update::Clear,
            Decision::Set {
                track,
                paused,
                anchor,
                duration_ms,
            } => Update::Set(build_presence(track, paused, anchor, duration_ms)),
        };
        Some(update)
    }
}

/// A decision before its (allocating) card is built. Holds only cheap ingredients
/// — a borrow plus copies — so an evaluation can be classified and deduped without
/// building a [`Presence`] that a republish would discard.
enum Decision<'a> {
    /// Remove the card.
    Clear,
    /// Show a card built from these inputs by [`build_presence`].
    Set {
        track: &'a TrackSummary,
        paused: bool,
        anchor: u64,
        duration_ms: u64,
    },
}

/// The decision, with its dedupe identity. `None` means "hold the current card"
/// (only the `loading` transition), distinct from a `Clear`.
fn classify<'a>(
    vm: Option<&'a PlayerViewModelLight>,
    now_ts: i64,
    flags: &DiscordFlags,
) -> Option<(Identity, Decision<'a>)> {
    let Some(vm) = vm else {
        return Some((Identity::Cleared, Decision::Clear));
    };
    match vm.status {
        // Every track change passes through `loading`; mapping it to a clear
        // would flash the card off and back on (and spend an update window).
        "loading" => None,
        "stopped" => Some((Identity::Cleared, Decision::Clear)),
        _ => {
            let Some(track) = vm.current_track.as_deref() else {
                return Some((Identity::Cleared, Decision::Clear));
            };
            let paused = vm.status == "paused";
            if paused && flags.discord_rpc_hide_when_paused {
                return Some((Identity::Cleared, Decision::Clear));
            }
            let now_secs = u64::try_from(now_ts).unwrap_or(0);
            let anchor = now_secs.saturating_sub(vm.position_ms / 1000);
            let identity = if paused {
                Identity::Paused { track_id: track.id }
            } else {
                Identity::Playing {
                    track_id: track.id,
                    start_ts: anchor,
                }
            };
            Some((
                identity,
                Decision::Set {
                    track,
                    paused,
                    anchor,
                    duration_ms: vm.duration_ms,
                },
            ))
        }
    }
}

fn build_presence(track: &TrackSummary, paused: bool, anchor: u64, duration_ms: u64) -> Presence {
    let details = clamp_field(&track.title).unwrap_or_else(|| APP_NAME.to_owned());
    let state = track.artist.as_deref().and_then(clamp_field);
    let large_text = Some(
        track
            .album
            .as_deref()
            .and_then(clamp_field)
            .unwrap_or_else(|| APP_NAME.to_owned()),
    );
    let (start_ts, end_ts) = if !paused && duration_ms > 0 {
        (Some(anchor), Some(anchor + duration_ms / 1000))
    } else {
        (None, None)
    };
    Presence {
        details,
        state,
        large_text,
        large_image: None,
        paused,
        start_ts,
        end_ts,
    }
}

/// Clamp a field to Discord's limits: trim, drop when empty, pad a lone
/// character to two (Discord rejects a 1-char field), and truncate to 128
/// characters on a char boundary.
fn clamp_field(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let count = trimmed.chars().count();
    if count < MIN_FIELD_CHARS {
        return Some(format!("{trimmed} "));
    }
    if count <= MAX_FIELD_CHARS {
        return Some(trimmed.to_owned());
    }
    Some(trimmed.chars().take(MAX_FIELD_CHARS).collect())
}

#[cfg(test)]
#[path = "tests/model_tests.rs"]
mod tests;
