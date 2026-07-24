use std::sync::Arc;

use serde_json::Value;

use super::{Presence, PresenceState, Update, clamp_field, clear_activity_json, set_activity_json};
use crate::entities::track::TrackSummary;
use crate::player::state::PlayerViewModelLight;
use crate::services::settings::DiscordFlags;

/// A fixed UNIX-seconds "now" for anchor arithmetic.
const NOW: i64 = 1_700_000_000;

fn track(
    id: i64,
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    duration_ms: i64,
) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id,
        file_path: String::new(),
        file_name: String::new(),
        title: title.to_owned(),
        artist: artist.map(str::to_owned),
        album: album.map(str::to_owned),
        duration_ms,
        artwork_path: None,
        track_number: None,
        disc_number: None,
        last_position: 0,
        is_favorite: false,
        rating: 0,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
    })
}

fn vm(
    status: &'static str,
    current_track: Option<Arc<TrackSummary>>,
    position_ms: u64,
    duration_ms: u64,
) -> PlayerViewModelLight {
    PlayerViewModelLight {
        status,
        current_track,
        position_ms,
        duration_ms,
        progress_percent: 0.0,
        volume: 100,
        is_muted: false,
        playback_speed: 1.0,
        gapless_enabled: false,
        sleep_at_track_end: false,
        has_next: false,
        has_previous: false,
    }
}

fn flags(hide_when_paused: bool) -> DiscordFlags {
    DiscordFlags {
        discord_rpc_enabled: true,
        discord_rpc_artwork: true,
        discord_rpc_hide_when_paused: hide_when_paused,
    }
}

fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice::<Value>(bytes).unwrap_or(Value::Null)
}

#[test]
fn first_play_sets_the_card() {
    let mut ps = PresenceState::new();
    let song = track(1, "Song", Some("Artist"), Some("Album"), 200_000);
    let update = ps.on_view_model(
        Some(&vm("playing", Some(song), 30_000, 200_000)),
        NOW,
        &flags(false),
    );
    assert!(matches!(update, Some(Update::Set(_))));
}

#[test]
fn volume_republish_sends_nothing() {
    let mut ps = PresenceState::new();
    let song = track(1, "Song", Some("Artist"), Some("Album"), 200_000);
    let first = ps.on_view_model(
        Some(&vm("playing", Some(song.clone()), 30_000, 200_000)),
        NOW,
        &flags(false),
    );
    assert!(matches!(first, Some(Update::Set(_))));

    // 5 s later playback has advanced 5 s (only volume actually changed) — the
    // anchor `now - position` is invariant, so nothing new should be sent.
    let second = ps.on_view_model(
        Some(&vm("playing", Some(song), 35_000, 200_000)),
        NOW + 5,
        &flags(false),
    );
    assert!(second.is_none());
}

#[test]
fn seek_re_anchors() {
    let mut ps = PresenceState::new();
    let song = track(1, "Song", Some("Artist"), Some("Album"), 200_000);
    ps.on_view_model(
        Some(&vm("playing", Some(song.clone()), 30_000, 200_000)),
        NOW,
        &flags(false),
    );

    // Same instant, position jumps forward — the anchor moves well past the 2 s
    // tolerance, so this is a fresh update.
    let after_seek = ps.on_view_model(
        Some(&vm("playing", Some(song), 120_000, 200_000)),
        NOW,
        &flags(false),
    );
    assert!(matches!(after_seek, Some(Update::Set(_))));
}

#[test]
fn pause_drops_timestamps_then_resume_re_anchors() {
    let mut ps = PresenceState::new();
    let song = track(1, "Song", Some("Artist"), Some("Album"), 200_000);
    ps.on_view_model(
        Some(&vm("playing", Some(song.clone()), 30_000, 200_000)),
        NOW,
        &flags(false),
    );

    let paused = ps.on_view_model(
        Some(&vm("paused", Some(song.clone()), 30_000, 200_000)),
        NOW + 1,
        &flags(false),
    );
    let expected_paused = Presence {
        details: "Song".to_owned(),
        state: Some("Artist".to_owned()),
        large_text: Some("Album".to_owned()),
        large_image: None,
        paused: true,
        start_ts: None,
        end_ts: None,
    };
    assert_eq!(paused, Some(Update::Set(expected_paused)));

    let resumed = ps.on_view_model(
        Some(&vm("playing", Some(song), 30_000, 200_000)),
        NOW + 10,
        &flags(false),
    );
    assert!(matches!(resumed, Some(Update::Set(_))));
}

#[test]
fn seek_while_paused_sends_nothing() {
    let mut ps = PresenceState::new();
    let song = track(1, "Song", Some("Artist"), Some("Album"), 200_000);
    ps.on_view_model(
        Some(&vm("paused", Some(song.clone()), 30_000, 200_000)),
        NOW,
        &flags(false),
    );
    // Paused identity ignores position, so a paused seek changes nothing.
    let after = ps.on_view_model(
        Some(&vm("paused", Some(song), 90_000, 200_000)),
        NOW,
        &flags(false),
    );
    assert!(after.is_none());
}

#[test]
fn stop_clears_once() {
    let mut ps = PresenceState::new();
    let song = track(1, "Song", Some("Artist"), Some("Album"), 200_000);
    ps.on_view_model(
        Some(&vm("playing", Some(song), 30_000, 200_000)),
        NOW,
        &flags(false),
    );

    let stopped = ps.on_view_model(Some(&vm("stopped", None, 0, 0)), NOW, &flags(false));
    assert_eq!(stopped, Some(Update::Clear));

    // Idempotent — a second stop is already cleared.
    let again = ps.on_view_model(Some(&vm("stopped", None, 0, 0)), NOW, &flags(false));
    assert!(again.is_none());
}

#[test]
fn loading_holds_previous_card() {
    let mut ps = PresenceState::new();
    let song = track(1, "Song", Some("Artist"), Some("Album"), 200_000);
    ps.on_view_model(
        Some(&vm("playing", Some(song.clone()), 30_000, 200_000)),
        NOW,
        &flags(false),
    );

    let loading = ps.on_view_model(Some(&vm("loading", Some(song), 0, 0)), NOW + 1, &flags(false));
    assert!(loading.is_none());
}

#[test]
fn hide_when_paused_clears() {
    let mut ps = PresenceState::new();
    let song = track(1, "Song", Some("Artist"), Some("Album"), 200_000);
    ps.on_view_model(
        Some(&vm("playing", Some(song.clone()), 30_000, 200_000)),
        NOW,
        &flags(true),
    );

    let paused = ps.on_view_model(
        Some(&vm("paused", Some(song), 30_000, 200_000)),
        NOW + 1,
        &flags(true),
    );
    assert_eq!(paused, Some(Update::Clear));
}

#[test]
fn reset_forces_repaint() {
    let mut ps = PresenceState::new();
    let song = track(1, "Song", Some("Artist"), Some("Album"), 200_000);
    ps.on_view_model(
        Some(&vm("playing", Some(song.clone()), 30_000, 200_000)),
        NOW,
        &flags(false),
    );

    // Without a reset the identical view-model dedupes; after a reset it repaints
    // (the disabled→enabled edge the task uses).
    let deduped = ps.on_view_model(
        Some(&vm("playing", Some(song.clone()), 30_000, 200_000)),
        NOW,
        &flags(false),
    );
    assert!(deduped.is_none());

    ps.reset();
    let repainted = ps.on_view_model(
        Some(&vm("playing", Some(song), 30_000, 200_000)),
        NOW,
        &flags(false),
    );
    assert!(matches!(repainted, Some(Update::Set(_))));
}

#[test]
fn clamp_field_enforces_bounds() {
    assert!(clamp_field("").is_none());
    assert!(clamp_field("   ").is_none());
    // A lone character is padded to Discord's 2-char minimum.
    assert_eq!(clamp_field("a").as_deref(), Some("a "));
    assert_eq!(clamp_field("  hi  ").as_deref(), Some("hi"));

    // 200 two-byte chars truncate to 128 chars on a char boundary (never mid-byte).
    let long = "é".repeat(200);
    let clamped = clamp_field(&long).unwrap_or_default();
    assert_eq!(clamped.chars().count(), 128);
    assert!(clamped.chars().all(|c| c == 'é'));
}

/// A playing card with an anchor of `NOW - 30 s` and a 200 s track.
fn playing_card() -> Presence {
    let anchor = 1_699_999_970; // NOW - 30
    Presence {
        details: "Song".to_owned(),
        state: Some("Artist".to_owned()),
        large_text: Some("Album".to_owned()),
        large_image: None,
        paused: false,
        start_ts: Some(anchor),
        end_ts: Some(anchor + 200),
    }
}

#[test]
fn playing_activity_json_carries_type_and_timestamps() {
    let value = parse(&set_activity_json(&playing_card(), 4242, "4242-1"));
    assert_eq!(value["cmd"], "SET_ACTIVITY");
    assert_eq!(value["nonce"], "4242-1");
    assert_eq!(value["args"]["pid"], 4242);

    let activity = &value["args"]["activity"];
    assert_eq!(activity["type"], 2);
    assert_eq!(activity["status_display_type"], 2);
    assert_eq!(activity["details"], "Song");
    assert_eq!(activity["state"], "Artist");
    assert_eq!(activity["assets"]["large_text"], "Album");
    assert_eq!(activity["assets"]["large_image"], "melodia");
    assert!(activity["timestamps"]["start"].is_number());
    assert!(activity["timestamps"]["end"].is_number());
}

#[test]
fn paused_activity_json_omits_timestamps_and_shows_badge() {
    let paused = Presence {
        paused: true,
        start_ts: None,
        end_ts: None,
        ..playing_card()
    };
    let value = parse(&set_activity_json(&paused, 1, "1-1"));
    let activity = &value["args"]["activity"];
    assert_eq!(activity["type"], 2);
    assert!(activity["timestamps"].is_null());
    assert_eq!(activity["assets"]["small_image"], "paused");
    assert_eq!(activity["assets"]["small_text"], "Paused");
}

#[test]
fn untagged_track_omits_state_and_falls_back_to_app_name() {
    let untagged = Presence {
        details: "OnlyTitle".to_owned(),
        state: None,
        large_text: Some("Melodia".to_owned()),
        large_image: None,
        paused: false,
        start_ts: None,
        end_ts: None,
    };

    let value = parse(&set_activity_json(&untagged, 1, "1-1"));
    let activity = &value["args"]["activity"];
    assert!(activity["state"].is_null());
    assert_eq!(activity["assets"]["large_text"], "Melodia");
    assert_eq!(activity["assets"]["large_image"], "melodia");
    assert!(activity["timestamps"].is_null());
}

#[test]
fn clear_activity_json_nulls_the_activity() {
    let value = parse(&clear_activity_json(7, "7-1"));
    assert_eq!(value["cmd"], "SET_ACTIVITY");
    assert_eq!(value["args"]["pid"], 7);
    assert!(value["args"]["activity"].is_null());
}
