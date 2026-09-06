use melodia_core::entities::radio::RadioStation;
use melodia_core::error::AppError;

use super::*;

fn json_err(e: &serde_json::Error) -> AppError {
    AppError::Validation(format!("json error: {e}"))
}

#[test]
fn playback_status_as_str_all_variants() {
    assert_eq!(PlaybackStatus::Stopped.as_str(), "stopped");
    assert_eq!(PlaybackStatus::Playing.as_str(), "playing");
    assert_eq!(PlaybackStatus::Paused.as_str(), "paused");
    assert_eq!(PlaybackStatus::Loading.as_str(), "loading");
}

#[test]
fn playback_status_serde_roundtrip() -> Result<(), AppError> {
    for status in [
        PlaybackStatus::Stopped,
        PlaybackStatus::Playing,
        PlaybackStatus::Paused,
        PlaybackStatus::Loading,
    ] {
        let json = serde_json::to_string(&status).map_err(|e| json_err(&e))?;
        let deserialized: PlaybackStatus = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
        assert_eq!(status, deserialized);
    }
    Ok(())
}

#[test]
fn repeat_mode_serde_roundtrip() -> Result<(), AppError> {
    for mode in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
        let json = serde_json::to_string(&mode).map_err(|e| json_err(&e))?;
        let deserialized: RepeatMode = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
        assert_eq!(mode, deserialized);
    }
    Ok(())
}

#[test]
fn persistable_queue_serde_roundtrip() -> Result<(), AppError> {
    let queue = PersistableQueue {
        track_ids: vec![10, 20, 30],
        current_index: 1,
    };
    let json = serde_json::to_string(&queue).map_err(|e| json_err(&e))?;
    let deserialized: PersistableQueue = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(queue, deserialized);
    Ok(())
}

#[test]
fn persistable_queue_empty() -> Result<(), AppError> {
    let queue = PersistableQueue {
        track_ids: vec![],
        current_index: -1,
    };
    let json = serde_json::to_string(&queue).map_err(|e| json_err(&e))?;
    let deserialized: PersistableQueue = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(queue, deserialized);
    Ok(())
}

#[test]
fn playback_status_equality() {
    assert_eq!(PlaybackStatus::Playing, PlaybackStatus::Playing);
    assert_ne!(PlaybackStatus::Playing, PlaybackStatus::Paused);
    assert_ne!(PlaybackStatus::Stopped, PlaybackStatus::Loading);
}

#[test]
fn persisted_playback_serde_roundtrip_with_and_without_a_station() -> Result<(), AppError> {
    for station_id in [Some(42_i64), None] {
        let persisted = PersistedPlayback {
            queue: PersistableQueue {
                track_ids: vec![1, 2, 3],
                current_index: 1,
            },
            station_id,
        };

        let json = serde_json::to_string(&persisted).map_err(|e| json_err(&e))?;
        let back: PersistedPlayback = serde_json::from_str(&json).map_err(|e| json_err(&e))?;

        assert_eq!(back, persisted);
    }
    Ok(())
}

/// The flatten is what keeps an already-shipped `queue.json` readable: a file written before the
/// station rode along carries the queue's two fields at the top level and no `station_id` at all.
/// Nesting the queue, or dropping the `default`, turns every such install into a lost queue on the
/// first launch after an update.
#[test]
fn a_queue_file_written_before_stations_still_parses() -> Result<(), AppError> {
    let shipped = r#"{"track_ids":[7,8],"current_index":1}"#;

    let back: PersistedPlayback = serde_json::from_str(shipped).map_err(|e| json_err(&e))?;

    assert_eq!(back.queue.track_ids, vec![7, 8]);
    assert_eq!(back.queue.current_index, 1);
    assert_eq!(back.station_id, None, "an old file tunes no station");
    Ok(())
}

// ---- what a stored station looks like to the player ----

/// A directory station carrying no overrides. The `local_*` columns are what each case sets.
fn directory_station() -> RadioStation {
    RadioStation {
        id: 42,
        station_uuid: Some("uuid-1".to_owned()),
        name: "Radio Paradise".to_owned(),
        stream_url: "https://stream.example/rp".to_owned(),
        homepage: Some("https://directory.example/rp".to_owned()),
        local_homepage: None,
        favicon_url: Some("https://directory.example/rp.png".to_owned()),
        local_favicon_url: None,
        local_tags: None,
        local_country: None,
        artwork_path: Some("radio-logos/rp.png".to_owned()),
        tags: "eclectic,rock".to_owned(),
        country: "The United States Of America".to_owned(),
        country_code: "US".to_owned(),
        language: "english".to_owned(),
        codec: "MP3".to_owned(),
        bitrate: 320,
        hls: false,
        is_favorite: true,
        sort_key: "radio paradise".to_owned(),
        date_added: "2026-01-01T00:00:00Z".to_owned(),
        last_played: None,
        play_count: 7,
    }
}

/// The three descriptive fields go through `RadioStation`'s override accessors rather than the
/// same-named columns, which is the only reason the Now-Playing bar and the station's own page
/// cannot disagree. Reading the columns instead compiles, and the user's own edits then show on
/// the page they were typed into and nowhere else.
#[test]
fn the_playing_station_states_the_users_overrides_and_not_the_directorys() {
    let station = RadioStation {
        local_country: Some("Somewhere Else".to_owned()),
        local_tags: Some("ambient".to_owned()),
        local_homepage: Some("https://mine.example".to_owned()),
        ..directory_station()
    };

    let playing = RadioNowPlaying::from(&station);

    assert_eq!(playing.country.as_deref(), Some("Somewhere Else"));
    assert_eq!(playing.tags.as_deref(), Some("ambient"));
    assert_eq!(playing.homepage.as_deref(), Some("https://mine.example"));
}

/// With nothing overridden the same three fall back to the directory's, which is the half that
/// would still pass if the accessors were bypassed.
#[test]
fn a_station_with_no_overrides_states_what_the_directory_said() {
    let playing = RadioNowPlaying::from(&directory_station());

    assert_eq!(playing.country.as_deref(), Some("The United States Of America"));
    assert_eq!(playing.tags.as_deref(), Some("eclectic,rock"));
    assert_eq!(playing.homepage.as_deref(), Some("https://directory.example/rp"));
}

/// `codec` is a `String` on the row and an `Option` here, so the empty one the directory sends
/// for a station it knows nothing about has to become `None`. `Some("")` would paint an empty
/// codec chip rather than no chip.
#[test]
fn a_station_with_no_codec_carries_none_rather_than_an_empty_one() {
    let station = RadioStation {
        codec: String::new(),
        ..directory_station()
    };

    assert_eq!(RadioNowPlaying::from(&station).codec, None);
}

/// The identity and playback fields, which is the half a transposition would silently swap: the
/// stream URL and the logo path are both `Option<String>`-ish strings that render nowhere near
/// each other, and `live_title` and `buffering` describe a stream nobody has opened yet.
#[test]
fn a_freshly_tuned_station_carries_the_row_and_no_stream_state() {
    let playing = RadioNowPlaying::from(&directory_station());

    assert_eq!(playing.station_id, 42);
    assert_eq!(playing.station_uuid.as_deref(), Some("uuid-1"));
    assert_eq!(playing.name, "Radio Paradise");
    assert_eq!(playing.stream_url, "https://stream.example/rp");
    assert_eq!(playing.artwork_path.as_deref(), Some("radio-logos/rp.png"));
    assert_eq!(playing.bitrate, 320);
    assert_eq!(playing.play_count, 7);
    assert_eq!(playing.live_title, None);
    assert!(!playing.buffering, "nothing has opened the stream yet");
}
