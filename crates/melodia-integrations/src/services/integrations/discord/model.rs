//! The pure Rich Presence projection: the player's view model into a [`Presence`] card, deduped
//! so a `watch` republishing on every volume tap doesn't become an IPC write per tap. `now_ts`
//! (UNIX **seconds**) is an input, as in [`crate::services::integrations::scrobble::detector`].

use melodia_core::entities::integrations::DiscordFlags;
use melodia_engine::player::engine::now_playing::{SourceId, SourceSummary};
use melodia_engine::player::engine::state::PlayerViewModelLight;

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

/// Owned, so it can cross the worker channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    /// Card title (Discord `details`) — the song, or a station's name until it announces one.
    pub details: String,
    /// Second line (Discord `state`) — the artist, or the station once the line above is its
    /// song; `None` when there is nothing to add.
    pub state: Option<String>,
    /// Large-image tooltip — the album, falling back to the app name. A station fills it with its
    /// own name once the lines above are a song and an artist.
    pub large_text: Option<String>,
    /// An `https://` cover, `None` showing the app logo. The task fills it, the lookup being I/O.
    pub large_image: Option<String>,
    pub paused: bool,
    /// Progress-bar anchor `now - elapsed`, UNIX **seconds**. `None` while paused
    /// or with an unknown duration (no bar).
    pub start_ts: Option<u64>,
    /// Track end, UNIX seconds — `start_ts + duration`. Paired with `start_ts`.
    pub end_ts: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Set(Presence),
    Clear,
}

/// What the card is *about*, at the granularity a republish has to notice. A station is its
/// stream URL **and the lines it announces**: the URL never changes in a session, so keyed on it
/// alone every song after the first would dedupe away and the card would name whatever was
/// playing at tune-in.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CardSource {
    Track(i64),
    Station { stream_url: String, title: String, secondary: Option<String> },
}

impl From<&SourceSummary<'_>> for CardSource {
    fn from(source: &SourceSummary<'_>) -> Self {
        match source.id {
            SourceId::Track(id) => Self::Track(id),
            SourceId::Station(stream_url) => Self::Station {
                stream_url: stream_url.to_owned(),
                title: source.title.to_owned(),
                secondary: source.secondary.map(str::to_owned),
            },
        }
    }
}

/// The dedupe key: what a republish has to move before it costs a write.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Identity {
    /// Also the initial state, Discord showing nothing.
    Cleared,
    /// `start_ts` matches within [`ANCHOR_TOLERANCE_SECS`], so only a real seek re-anchors.
    Playing { source: CardSource, start_ts: u64 },
    /// No timestamps, so a seek while paused changes nothing.
    Paused { source: CardSource },
}

impl Identity {
    fn matches(&self, other: &Identity) -> bool {
        match (self, other) {
            (Identity::Cleared, Identity::Cleared) => true,
            (Identity::Paused { source: a }, Identity::Paused { source: b }) => a == b,
            (
                Identity::Playing { source: a, start_ts: sa },
                Identity::Playing { source: b, start_ts: sb },
            ) => a == b && sa.abs_diff(*sb) <= ANCHOR_TOLERANCE_SECS,
            _ => false,
        }
    }
}

/// The last card emitted, held by the task between view models.
#[derive(Debug)]
pub struct PresenceState {
    last: Identity,
}

impl Default for PresenceState {
    fn default() -> Self {
        Self { last: Identity::Cleared }
    }
}

impl PresenceState {
    pub fn new() -> Self {
        Self::default()
    }

    /// For the off-to-on edge: the card was cleared while off, but the dedupe still remembers it.
    pub fn reset(&mut self) {
        self.last = Identity::Cleared;
    }

    /// `None` when nothing is to be sent: the card is current, or a `loading` transition holds it.
    pub fn on_view_model(
        &mut self,
        vm: Option<&PlayerViewModelLight>,
        now_ts: i64,
        flags: &DiscordFlags,
    ) -> Option<Update> {
        let (identity, decision) = classify(vm, now_ts, flags)?;
        if self.last.matches(&identity) {
            return None;
        }
        self.last = identity;
        let update = match decision {
            Decision::Clear => Update::Clear,
            Decision::Set { source, paused, anchor } => {
                Update::Set(build_presence(&source, paused, anchor))
            }
        };
        Some(update)
    }
}

/// A decision before its card is built, so a deduped republish never pays for a [`Presence`].
enum Decision<'a> {
    Clear,
    Set { source: SourceSummary<'a>, paused: bool, anchor: u64 },
}

fn classify<'a>(
    vm: Option<&'a PlayerViewModelLight>,
    now_ts: i64,
    flags: &DiscordFlags,
) -> Option<(Identity, Decision<'a>)> {
    let Some(vm) = vm else {
        return Some((Identity::Cleared, Decision::Clear));
    };
    match vm.status {
        // Every track change passes through `loading`, and a clear here would flash the card off
        // and back on, spending an update window.
        "loading" => None,
        "stopped" => Some((Identity::Cleared, Decision::Clear)),
        _ => {
            let Some(source) = vm.source() else {
                return Some((Identity::Cleared, Decision::Clear));
            };
            let paused = vm.status == "paused";
            if paused && flags.discord_rpc_hide_when_paused {
                return Some((Identity::Cleared, Decision::Clear));
            }
            let now_secs = u64::try_from(now_ts).unwrap_or(0);
            let anchor = now_secs.saturating_sub(vm.position_ms / 1000);
            let card = CardSource::from(&source);
            let identity = if paused {
                Identity::Paused { source: card }
            } else {
                Identity::Playing { source: card, start_ts: anchor }
            };
            Some((identity, Decision::Set { source, paused, anchor }))
        }
    }
}

fn build_presence(source: &SourceSummary<'_>, paused: bool, anchor: u64) -> Presence {
    let details = clamp_field(source.title).unwrap_or_else(|| APP_NAME.to_owned());
    let state = source.secondary.and_then(clamp_field);
    let large_text =
        Some(source.album.and_then(clamp_field).unwrap_or_else(|| APP_NAME.to_owned()));
    // A live source has no duration, so no bar: right for a stream with no end, and no arm of
    // its own.
    let duration_ms = source.duration_ms.unwrap_or(0);
    let (start_ts, end_ts) = if !paused && duration_ms > 0 {
        (Some(anchor), Some(anchor + duration_ms / 1000))
    } else {
        (None, None)
    };
    Presence { details, state, large_text, large_image: None, paused, start_ts, end_ts }
}

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
