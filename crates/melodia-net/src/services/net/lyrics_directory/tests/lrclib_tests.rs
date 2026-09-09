//! Which row of an index page is believed, and how long a refusal holds.
//!
//! The index is a keyword search, so it answers with rows that are not this recording at all, and
//! the choice between the ones that are is made on a duration the service reports in fractional
//! seconds. Both halves are silent when wrong: the panel draws whatever came back.

use super::*;

use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};

const TRACK: &str = "Song";
const ARTIST: &str = "Band";
const ALBUM: &str = "Record";

/// The playing track's length, spelled in both units the comparison crosses.
const TRACK_MS: i64 = 180_000;
const TRACK_SECONDS: f64 = 180.0;

fn ours() -> Recording {
    Recording::new(TRACK, ARTIST)
}

/// A timed row for the track above, `off_by` seconds from its length, filed under `album`.
fn row(album: &str, off_by: f64) -> ApiLyrics {
    ApiLyrics {
        synced_lyrics: Some("[00:01.00]words".to_owned()),
        plain_lyrics: None,
        instrumental: false,
        duration: TRACK_SECONDS + off_by,
        track_name: TRACK.to_owned(),
        artist_name: ARTIST.to_owned(),
        album_name: album.to_owned(),
    }
}

/// The album of whichever row was picked, which is what the cases below tell rows apart by.
fn picked(rows: Vec<ApiLyrics>, album: &str) -> Option<String> {
    pick_timed(rows, &ours(), album, TRACK_MS).map(|row| row.album_name)
}

#[test]
fn an_empty_index_page_is_no_answer() {
    assert_eq!(picked(Vec::new(), ALBUM), None);
}

#[test]
fn a_row_carrying_no_timings_is_refused_however_well_it_matches() {
    // The index is only ever asked because the signature left us with words nothing can follow, so
    // a plain row here is a row that answers nothing.
    let mut plain = row(ALBUM, 0.0);
    plain.synced_lyrics = None;
    assert_eq!(picked(vec![plain], ALBUM), None, "absent");

    let mut blank = row(ALBUM, 0.0);
    blank.synced_lyrics = Some("   ".to_owned());
    assert_eq!(picked(vec![blank], ALBUM), None, "present but empty");
}

#[test]
fn a_row_inside_the_duration_tolerance_is_taken() {
    let on_it = DURATION_TOLERANCE_MS / 1000.0;
    assert_eq!(picked(vec![row(ALBUM, on_it)], ALBUM), Some(ALBUM.to_owned()), "on the edge");
    assert_eq!(
        picked(vec![row(ALBUM, on_it - 0.001)], ALBUM),
        Some(ALBUM.to_owned()),
        "the step inside"
    );
}

#[test]
fn a_row_past_the_duration_tolerance_is_refused() {
    // A different recording of the same song by the same band is what this window is for, and it
    // is the one case the title and artist check cannot see.
    let past = DURATION_TOLERANCE_MS / 1000.0 + 0.001;
    assert_eq!(picked(vec![row(ALBUM, past)], ALBUM), None);
}

#[test]
fn the_nearer_duration_wins() {
    let rows = vec![row("Far", 1.5), row("Near", 0.5)];
    assert_eq!(picked(rows, ""), Some("Near".to_owned()));
}

#[test]
fn two_rows_the_same_distance_out_are_settled_by_the_album_in_hand() {
    // The tie-break and nothing more: at this point both rows are the same recording twice, and
    // the release it was ripped from is all there is left to prefer.
    let rows = vec![row("Somebody Else's Compilation", 0.5), row(ALBUM, 0.5)];
    assert_eq!(picked(rows, ALBUM), Some(ALBUM.to_owned()));
}

#[test]
fn with_no_album_in_hand_the_index_order_stands() {
    // An untagged album must not skew the ranking, so every row ranks alike and the first of them
    // is kept.
    let rows = vec![row("First", 0.5), row("Second", 0.5)];
    assert_eq!(picked(rows, ""), Some("First".to_owned()));
}

#[test]
fn another_artists_row_is_dropped_at_an_exact_duration() {
    // The duration filter is cheap and runs first, which is exactly why it cannot be the only one:
    // two songs of the same name run the same length often enough.
    let mut stranger = row(ALBUM, 0.0);
    stranger.artist_name = "Some Other Band".to_owned();
    assert_eq!(picked(vec![stranger], ALBUM), None);
}

#[test]
fn a_row_arrives_with_every_field_missing() -> Result<(), serde_json::Error> {
    // Nothing in the response is required, so a shape change upstream has to read as an empty row
    // rather than as a failed lookup for the whole library.
    let row: ApiLyrics = serde_json::from_str("{}")?;
    assert!(!row.is_timed());
    assert!(!row.instrumental);
    assert_eq!(row.album_name, "");
    Ok(())
}

#[test]
fn the_wire_names_are_the_camel_cased_ones() -> Result<(), serde_json::Error> {
    let json = r#"{"syncedLyrics":"[00:01.00]a","plainLyrics":"a","instrumental":false,
        "duration":180.5,"trackName":"Song","artistName":"Band","albumName":"Record"}"#;
    let row: ApiLyrics = serde_json::from_str(json)?;
    assert!(row.is_timed());
    assert_eq!(row.track_name, TRACK);
    assert!(row.recording().matches(&ours()));
    Ok(())
}

#[test]
fn an_answer_carries_both_texts_and_the_flag() {
    let mut instrumental = row(ALBUM, 0.0);
    instrumental.plain_lyrics = Some("words".to_owned());
    instrumental.instrumental = true;

    let answer = instrumental.into_answer();
    assert_eq!(answer.synced.as_deref(), Some("[00:01.00]words"));
    assert_eq!(answer.plain.as_deref(), Some("words"));
    assert!(answer.instrumental);
}

#[test]
fn a_refusal_with_no_header_names_no_delay() {
    assert_eq!(retry_after(&HeaderMap::new()), None);
}

#[test]
fn a_delay_in_seconds_is_read() {
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));
    assert_eq!(retry_after(&headers), Some(Duration::from_mins(2)));
}

#[test]
fn a_delay_longer_than_a_session_would_sit_out_is_clamped() {
    // Honoured as stated, an hour-long refusal turns into an hour of a running app answering
    // nothing, where the service is reachable again long before that.
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("99999"));
    assert_eq!(retry_after(&headers), Some(MAX_BACKOFF));
}

#[test]
fn a_delay_this_cannot_read_is_no_delay_rather_than_a_wrong_one() {
    // The header's other spelling is an HTTP-date. Falling through leaves the caller's own default
    // to arm the stop, which is the same answer an absent header gets.
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
    assert_eq!(retry_after(&headers), None, "an http-date");

    headers.insert(RETRY_AFTER, HeaderValue::from_static("soon"));
    assert_eq!(retry_after(&headers), None, "a word");
}
