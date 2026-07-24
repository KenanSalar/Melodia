//! Pure Discord Rich Presence projection: turn the player's published
//! view-model into a Discord "activity" payload, plus the dedupe rule that keeps
//! a state-change-only `watch` from becoming an IPC write per volume tap.
//!
//! No I/O, no locks, no clock reads — `now_ts` (UNIX **seconds**) is an input,
//! the same shape as [`crate::services::scrobble::detector`] and
//! `player::handlers::evaluate_playing_tick`. The impure task in
//! [`crate::tasks::discord_presence`] drives it; [`super::ipc`] ships the bytes
//! the `*_json` builders here produce.

use serde::Serialize;

use crate::entities::track::TrackSummary;
use crate::player::state::PlayerViewModelLight;
use crate::services::settings::DiscordFlags;

/// Art-asset key for the app logo (uploaded under this name in the Discord
/// application's Art Assets). The large image when there's no cover, and the
/// small corner badge when a cover fills the large slot.
const ASSET_LOGO: &str = "melodia";
/// Art-asset key for the paused badge.
const ASSET_PAUSED: &str = "paused";
/// Fallback album line / large-image caption when a track is untagged.
const APP_NAME: &str = "Melodia";
/// Small-image caption while paused.
const PAUSED_TEXT: &str = "Paused";

/// Discord truncates `details`/`state` past 128 characters and rejects a value
/// shorter than 2, so we clamp both before the string leaves the model.
const MAX_FIELD_CHARS: usize = 128;
const MIN_FIELD_CHARS: usize = 2;

/// The single fixed profile button. Deliberately not the resolved album link —
/// a fixed target has no per-track state and can never point somewhere wrong.
/// English-only, like the tray labels. (Discord hides a button from the owner
/// and shows it to everyone else viewing the profile.)
const MELODIA_BUTTON: ButtonDto = ButtonDto {
    label: "Get Melodia",
    url: "https://github.com/KenanSalar/Melodia",
};

/// Activity type 2 = "Listening" — the only value that renders "Listening to …"
/// and permits an `end` timestamp (the progress bar).
const ACTIVITY_LISTENING: u8 = 2;
/// `status_display_type` 2 = Details, so the member-list line shows the song
/// title rather than the application name.
const STATUS_DISPLAY_DETAILS: u8 = 2;

/// How far the anchored start may drift (seconds) and still count as the same
/// play. Absorbs the ~500 ms-stale monitor position and second truncation, so a
/// volume republish (same anchor) dedupes while a real seek re-anchors.
const ANCHOR_TOLERANCE_SECS: u64 = 2;

/// A fully-resolved presence card, owned so it can cross the worker channel.
/// Built by [`PresenceState`] from the view-model; serialized to the Discord
/// activity object by [`set_activity_json`].
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
        let (identity, update) = classify(vm, now_ts, flags)?;
        if self.last.matches(identity) {
            return None;
        }
        self.last = identity;
        Some(update)
    }
}

/// The decision, with its dedupe identity. `None` means "hold the current card"
/// (only the `loading` transition), distinct from a `Clear`.
fn classify(
    vm: Option<&PlayerViewModelLight>,
    now_ts: i64,
    flags: &DiscordFlags,
) -> Option<(Identity, Update)> {
    let Some(vm) = vm else {
        return Some((Identity::Cleared, Update::Clear));
    };
    match vm.status {
        // Every track change passes through `loading`; mapping it to a clear
        // would flash the card off and back on (and spend an update window).
        "loading" => None,
        "stopped" => Some((Identity::Cleared, Update::Clear)),
        _ => {
            let Some(track) = vm.current_track.as_ref() else {
                return Some((Identity::Cleared, Update::Clear));
            };
            let paused = vm.status == "paused";
            if paused && flags.discord_rpc_hide_when_paused {
                return Some((Identity::Cleared, Update::Clear));
            }
            let now_secs = u64::try_from(now_ts).unwrap_or(0);
            let anchor = now_secs.saturating_sub(vm.position_ms / 1000);
            let presence = build_presence(track, paused, anchor, vm.duration_ms);
            let identity = if paused {
                Identity::Paused { track_id: track.id }
            } else {
                Identity::Playing {
                    track_id: track.id,
                    start_ts: anchor,
                }
            };
            Some((identity, Update::Set(presence)))
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

// ── Wire payloads (serialize mirrors, never hand-rolled impls) ──────────────

#[derive(Serialize)]
struct HandshakeDto<'a> {
    v: u8,
    client_id: &'a str,
}

#[derive(Serialize)]
struct SetActivityDto<'a> {
    cmd: &'static str,
    args: SetActivityArgs<'a>,
    nonce: &'a str,
}

#[derive(Serialize)]
struct SetActivityArgs<'a> {
    pid: u32,
    /// `null` clears the presence; omitting it would instead leave it unchanged.
    activity: Option<ActivityDto<'a>>,
}

#[derive(Serialize)]
struct ActivityDto<'a> {
    #[serde(rename = "type")]
    activity_type: u8,
    status_display_type: u8,
    details: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamps: Option<TimestampsDto>,
    assets: AssetsDto<'a>,
    /// One fixed profile button — always present on a set (never on a clear,
    /// which sends `activity: null`).
    buttons: [ButtonDto; 1],
}

#[derive(Serialize)]
struct ButtonDto {
    label: &'static str,
    url: &'static str,
}

#[derive(Serialize)]
struct TimestampsDto {
    start: u64,
    end: u64,
}

#[derive(Serialize)]
struct AssetsDto<'a> {
    large_image: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    large_text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    small_image: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    small_text: Option<&'a str>,
}

fn activity_dto(p: &Presence) -> ActivityDto<'_> {
    let large_image = p.large_image.as_deref().unwrap_or(ASSET_LOGO);
    let has_cover = p.large_image.is_some();
    // Paused → paused badge; a cover in the large slot → app logo as the corner
    // badge; otherwise the large slot already is the logo, so no small badge.
    let (small_image, small_text) = if p.paused {
        (Some(ASSET_PAUSED), Some(PAUSED_TEXT))
    } else if has_cover {
        (Some(ASSET_LOGO), None)
    } else {
        (None, None)
    };
    let timestamps = match (p.start_ts, p.end_ts) {
        (Some(start), Some(end)) => Some(TimestampsDto { start, end }),
        _ => None,
    };
    ActivityDto {
        activity_type: ACTIVITY_LISTENING,
        status_display_type: STATUS_DISPLAY_DETAILS,
        details: &p.details,
        state: p.state.as_deref(),
        timestamps,
        assets: AssetsDto {
            large_image,
            large_text: p.large_text.as_deref(),
            small_image,
            small_text,
        },
        buttons: [MELODIA_BUTTON],
    }
}

/// The handshake payload — `{"v":1,"client_id":…}`, opcode 0.
pub fn handshake_json(client_id: &str) -> Vec<u8> {
    serde_json::to_vec(&HandshakeDto { v: 1, client_id }).unwrap_or_default()
}

/// A `SET_ACTIVITY` frame body that sets `presence`.
pub fn set_activity_json(presence: &Presence, pid: u32, nonce: &str) -> Vec<u8> {
    serde_json::to_vec(&SetActivityDto {
        cmd: "SET_ACTIVITY",
        args: SetActivityArgs {
            pid,
            activity: Some(activity_dto(presence)),
        },
        nonce,
    })
    .unwrap_or_default()
}

/// A `SET_ACTIVITY` frame body that clears the presence (`activity: null`).
pub fn clear_activity_json(pid: u32, nonce: &str) -> Vec<u8> {
    serde_json::to_vec(&SetActivityDto {
        cmd: "SET_ACTIVITY",
        args: SetActivityArgs {
            pid,
            activity: None,
        },
        nonce,
    })
    .unwrap_or_default()
}

#[cfg(test)]
#[path = "tests/model_tests.rs"]
mod tests;
