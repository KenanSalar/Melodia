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
