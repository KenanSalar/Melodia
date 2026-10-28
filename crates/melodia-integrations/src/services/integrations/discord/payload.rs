//! The serde mirror of Discord's activity object, and the frame bodies [`super::ipc`] ships.
//! Kept apart from [`super::model`] so the projection and the wire format change independently.

use serde::Serialize;

use super::model::Presence;

// Art Asset keys, uploaded under these names to the Discord application.
const ASSET_LOGO: &str = "melodia";
const ASSET_PAUSED: &str = "paused";
const ASSET_PLAYING: &str = "playing";
const PAUSED_TEXT: &str = "Paused";
const PLAYING_TEXT: &str = "Playing";

/// Fixed rather than the album's link, so it has no per-track state and can never point somewhere
/// wrong. English-only, like the tray labels. Discord hides it from the profile's owner.
const MELODIA_BUTTON: ButtonDto =
    ButtonDto { label: "Get Melodia", url: "https://github.com/KenanSalar/Melodia" };

/// Activity type 2 = "Listening" — the only value that renders "Listening to …"
/// and permits an `end` timestamp (the progress bar).
const ACTIVITY_LISTENING: u8 = 2;
/// `status_display_type` 2 = Details, so the member-list line shows the song
/// title rather than the application name.
const STATUS_DISPLAY_DETAILS: u8 = 2;

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
    // Paused → paused badge; a cover in the large slot → a play badge over it;
    // otherwise the large slot already is the logo, so no small badge.
    let (small_image, small_text) = if p.paused {
        (Some(ASSET_PAUSED), Some(PAUSED_TEXT))
    } else if has_cover {
        (Some(ASSET_PLAYING), Some(PLAYING_TEXT))
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

pub fn handshake_json(client_id: &str) -> Vec<u8> {
    serde_json::to_vec(&HandshakeDto { v: 1, client_id }).unwrap_or_default()
}

pub fn set_activity_json(presence: &Presence, pid: u32, nonce: &str) -> Vec<u8> {
    serde_json::to_vec(&SetActivityDto {
        cmd: "SET_ACTIVITY",
        args: SetActivityArgs { pid, activity: Some(activity_dto(presence)) },
        nonce,
    })
    .unwrap_or_default()
}

pub fn clear_activity_json(pid: u32, nonce: &str) -> Vec<u8> {
    serde_json::to_vec(&SetActivityDto {
        cmd: "SET_ACTIVITY",
        args: SetActivityArgs { pid, activity: None },
        nonce,
    })
    .unwrap_or_default()
}

#[cfg(test)]
#[path = "tests/payload_tests.rs"]
mod tests;
