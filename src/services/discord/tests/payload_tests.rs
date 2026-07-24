use serde_json::Value;

use super::{clear_activity_json, set_activity_json};
use crate::services::discord::model::Presence;

fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice::<Value>(bytes).unwrap_or(Value::Null)
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

    // The fixed link button rides on every set.
    assert_eq!(activity["buttons"][0]["label"], "Get Melodia");
    assert_eq!(activity["buttons"][0]["url"], "https://github.com/KenanSalar/Melodia");
}

#[test]
fn cover_url_fills_large_image_and_swaps_logo_to_badge() {
    let with_cover = Presence {
        large_image: Some("https://cdn.example/cover.jpg".to_owned()),
        ..playing_card()
    };
    let value = parse(&set_activity_json(&with_cover, 1, "1-1"));
    let activity = &value["args"]["activity"];
    // The cover fills the large slot; the logo drops to the corner badge.
    assert_eq!(activity["assets"]["large_image"], "https://cdn.example/cover.jpg");
    assert_eq!(activity["assets"]["small_image"], "melodia");
    assert!(activity["assets"]["small_text"].is_null());
    assert_eq!(activity["buttons"][0]["label"], "Get Melodia");
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
