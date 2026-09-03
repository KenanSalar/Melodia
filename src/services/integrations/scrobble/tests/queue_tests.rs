use std::collections::VecDeque;

use super::{LoveItem, MAX_QUEUED, QueuedItem, ScrobbleQueue};
use crate::services::integrations::scrobble::model::ScrobbleTrack;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sample_item(title: &str, timestamp: i64) -> QueuedItem {
    QueuedItem {
        track: ScrobbleTrack {
            artist: "Artist".to_owned(),
            track: title.to_owned(),
            album: Some("Album".to_owned()),
            album_artist: None,
            duration_secs: Some(200),
            track_number: Some(3),
            recording_mbid: None,
            release_mbid: None,
        },
        timestamp,
        lastfm_remaining: true,
        listenbrainz_remaining: true,
    }
}

fn sample_love(title: &str, loved: bool) -> LoveItem {
    LoveItem {
        track: ScrobbleTrack {
            artist: "Artist".to_owned(),
            track: title.to_owned(),
            album: None,
            album_artist: None,
            duration_secs: None,
            track_number: None,
            recording_mbid: None,
            release_mbid: None,
        },
        loved,
        lastfm_remaining: true,
        listenbrainz_remaining: false,
    }
}

#[test]
fn round_trips_through_disk_preserving_timestamp() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("scrobble_queue.json");

    let mut queue = ScrobbleQueue::default();
    queue.push(sample_item("First", 1_700_000_000));
    queue.push(sample_item("Second", 1_700_000_123));
    queue.save(&path)?;

    let loaded = ScrobbleQueue::load(&path)?;
    assert_eq!(loaded, queue);
    assert_eq!(loaded.items[0].timestamp, 1_700_000_000);
    assert_eq!(loaded.items[1].timestamp, 1_700_000_123);
    Ok(())
}

#[test]
fn load_of_missing_file_is_empty() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("does_not_exist.json");
    assert_eq!(ScrobbleQueue::load(&path)?, ScrobbleQueue::default());
    Ok(())
}

#[test]
fn push_beyond_cap_drops_oldest() {
    let mut queue = ScrobbleQueue::default();
    for i in 0..MAX_QUEUED {
        queue.push(sample_item(&format!("t{i}"), 0));
    }
    assert_eq!(queue.items.len(), MAX_QUEUED);

    queue.push(sample_item("newest", 0));
    assert_eq!(queue.items.len(), MAX_QUEUED);
    // The oldest entry (t0) was evicted; the newest sits at the back.
    assert_eq!(queue.items.front().map(|it| it.track.track.as_str()), Some("t1"));
    assert_eq!(queue.items.back().map(|it| it.track.track.as_str()), Some("newest"));
}

#[test]
fn retain_pending_drops_only_fully_submitted() {
    let mut one_flag = sample_item("one-flag", 1);
    one_flag.lastfm_remaining = false; // ListenBrainz still pending → kept
    let mut done = sample_item("done", 2);
    done.lastfm_remaining = false;
    done.listenbrainz_remaining = false; // both cleared → dropped
    let pending = sample_item("pending", 3); // both true → kept

    let mut queue = ScrobbleQueue {
        items: VecDeque::from(vec![one_flag, done, pending]),
        loves: VecDeque::new(),
    };
    queue.retain_pending();

    let titles: Vec<&str> = queue.items.iter().map(|it| it.track.track.as_str()).collect();
    assert_eq!(titles, vec!["one-flag", "pending"]);
}

#[test]
fn push_love_coalesces_same_track() {
    let mut queue = ScrobbleQueue::default();
    queue.push_love(sample_love("Song", true));
    queue.push_love(sample_love("Other", true));

    // Re-toggling the same track folds into the existing entry (no third love),
    // takes the newer `loved`, and re-arms provider flags.
    let mut retoggle = sample_love("Song", false);
    retoggle.listenbrainz_remaining = true;
    queue.push_love(retoggle);

    assert_eq!(queue.loves.len(), 2);
    let song = queue.loves.iter().find(|l| l.track.track == "Song");
    assert!(matches!(
        song,
        Some(l) if !l.loved && l.lastfm_remaining && l.listenbrainz_remaining
    ));
}

#[test]
fn push_love_beyond_cap_drops_oldest() {
    let mut queue = ScrobbleQueue::default();
    for i in 0..MAX_QUEUED {
        queue.push_love(sample_love(&format!("t{i}"), true));
    }
    assert_eq!(queue.loves.len(), MAX_QUEUED);

    queue.push_love(sample_love("newest", true));
    assert_eq!(queue.loves.len(), MAX_QUEUED);
    assert_eq!(queue.loves.front().map(|it| it.track.track.as_str()), Some("t1"));
    assert_eq!(queue.loves.back().map(|it| it.track.track.as_str()), Some("newest"));
}

#[test]
fn retain_pending_drops_fully_submitted_loves() {
    let pending = sample_love("pending", true); // lastfm_remaining true → kept
    let mut done = sample_love("done", true);
    done.lastfm_remaining = false;
    done.listenbrainz_remaining = false; // both false → dropped

    let mut queue = ScrobbleQueue {
        items: VecDeque::new(),
        loves: VecDeque::from(vec![pending, done]),
    };
    queue.retain_pending();

    let titles: Vec<&str> = queue.loves.iter().map(|it| it.track.track.as_str()).collect();
    assert_eq!(titles, vec!["pending"]);
}

#[test]
fn legacy_queue_without_loves_field_loads_empty() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("scrobble_queue.json");
    // A queue file written before love-sync carries only `items`.
    std::fs::write(&path, r#"{"items":[]}"#)?;

    let loaded = ScrobbleQueue::load(&path)?;
    assert!(loaded.items.is_empty());
    assert!(loaded.loves.is_empty());
    Ok(())
}
