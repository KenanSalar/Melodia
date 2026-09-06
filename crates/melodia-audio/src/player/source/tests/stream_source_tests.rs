use std::time::Duration;

use reqwest::Url;

use crate::player::source::prebuffer::{PrebufferSource, StreamShared};
use crate::player::source::stream_source::{
    ABANDON_POLL, PREFETCH_FALLBACK_BYTES, PREFETCH_MAX_BYTES, PREFETCH_MIN_BYTES,
    PREFETCH_SECONDS, RECONNECT_ATTEMPTS, RECONNECT_MAX_DELAY, first_stream_url,
    is_playlist_content_type, is_playlist_url, prefetch_bytes, reconnect_delay,
    sleep_unless_abandoned,
};
use crate::player::source::tests::helpers::shape;

/// `Some(verdict)` when `raw` parsed, `None` when it didn't — so a typo in a test URL fails the
/// assertion instead of quietly satisfying a negative one.
fn playlist_verdict(raw: &str) -> Option<bool> {
    Url::parse(raw).ok().as_ref().map(is_playlist_url)
}

#[test]
fn playlist_extensions_are_recognised_case_insensitively() {
    for raw in [
        "http://example.test/stream.pls",
        "http://example.test/stream.M3U",
        "https://example.test/live.m3u8",
        "http://example.test/a.asx",
    ] {
        assert_eq!(playlist_verdict(raw), Some(true), "{raw} should read as a playlist");
    }
}

/// A mount routinely carries a session parameter after its name, so the query must not be part of
/// what the extension is read off.
#[test]
fn a_query_string_does_not_hide_the_extension() {
    assert_eq!(playlist_verdict("http://example.test/stream.pls?token=abc"), Some(true));
    assert_eq!(playlist_verdict("http://example.test/stream.mp3?file=x.pls"), Some(false));
}

#[test]
fn an_audio_mount_is_not_a_playlist() {
    for raw in [
        "http://example.test/stream.mp3",
        "http://example.test/stream",
        "http://example.test/",
        "http://example.test/live/aac",
    ] {
        assert_eq!(playlist_verdict(raw), Some(false), "{raw} should not read as a playlist");
    }
}

#[test]
fn playlist_content_types_are_recognised_with_and_without_parameters() {
    assert!(is_playlist_content_type("audio/x-scpls"));
    assert!(is_playlist_content_type("AUDIO/X-MPEGURL"));
    assert!(is_playlist_content_type("application/vnd.apple.mpegurl; charset=utf-8"));
    assert!(!is_playlist_content_type("audio/mpeg"));
    assert!(!is_playlist_content_type("application/ogg"));
}

#[test]
fn a_pls_body_yields_its_first_entry() {
    let body = "[playlist]\r\n\
                NumberOfEntries=2\r\n\
                File1=http://example.test/live.mp3\r\n\
                Title1=Example FM\r\n\
                File2=http://backup.test/live.mp3\r\n";

    assert_eq!(first_stream_url(body).as_deref(), Some("http://example.test/live.mp3"));
}

#[test]
fn an_m3u_body_skips_comments_and_blank_lines() {
    let body = "#EXTM3U\n\n#EXTINF:-1,Example FM\nhttp://example.test/live.mp3\n";

    assert_eq!(first_stream_url(body).as_deref(), Some("http://example.test/live.mp3"));
}

#[test]
fn an_asx_body_yields_the_first_href() {
    let body = "<ASX version=\"3.0\">\n  <Entry>\n    \
                <Ref href=\"http://example.test/live.mp3\" />\n  </Entry>\n</ASX>";

    assert_eq!(first_stream_url(body).as_deref(), Some("http://example.test/live.mp3"));
}

#[test]
fn a_single_quoted_href_works_too() {
    let body = "<Ref href='https://example.test/live.aac'/>";

    assert_eq!(first_stream_url(body).as_deref(), Some("https://example.test/live.aac"));
}

/// A query string puts an `=` in a line that is already a whole URL, which reads as a `.pls`
/// key/value pair. Falling through to the whole line is what recovers it.
#[test]
fn a_bare_url_carrying_a_query_string_is_not_read_as_a_key_value_pair() {
    let body = "#EXTM3U\nhttp://example.test/live.mp3?token=abc123\n";

    assert_eq!(
        first_stream_url(body).as_deref(),
        Some("http://example.test/live.mp3?token=abc123")
    );
}

/// And the `.pls` reading still wins where it should: `split_once` cuts at the *first* `=`, so a
/// station URL with its own query survives on the right-hand side.
#[test]
fn a_pls_entry_keeps_its_own_query_string() {
    let body = "[playlist]\nFile1=http://example.test/live.mp3?token=abc123\n";

    assert_eq!(
        first_stream_url(body).as_deref(),
        Some("http://example.test/live.mp3?token=abc123")
    );
}

/// Foreign exporters write a BOM, and the first line is exactly the one that has to survive it.
#[test]
fn a_leading_byte_order_mark_is_stripped() {
    let body = "\u{FEFF}http://example.test/live.mp3\n";

    assert_eq!(first_stream_url(body).as_deref(), Some("http://example.test/live.mp3"));
}

#[test]
fn a_playlist_naming_nothing_playable_yields_none() {
    assert_eq!(first_stream_url(""), None);
    assert_eq!(first_stream_url("#EXTM3U\n#EXTINF:-1,Nothing\n"), None);
    assert_eq!(first_stream_url("[playlist]\nNumberOfEntries=0\n"), None);
    // A relative path is a track playlist, not a station one, and this player cannot open it.
    assert_eq!(first_stream_url("../music/track.mp3\n"), None);
}

/// The whole budget, written out, and then past it. The last rung lands exactly on
/// [`RECONNECT_MAX_DELAY`], so the cap clamps nothing today and only starts doing work if the
/// attempt count rises — which is why both constants are pinned here rather than the shape they
/// happen to make.
#[test]
fn the_backoff_grows_to_its_cap_and_then_gives_up() {
    let ladder: Vec<Option<u64>> = (0..RECONNECT_ATTEMPTS)
        .map(|attempt| reconnect_delay(attempt).map(|delay| delay.as_secs()))
        .collect();

    assert_eq!(ladder, vec![Some(1), Some(2), Some(4), Some(8), Some(16)], "the whole budget");
    assert_eq!(
        reconnect_delay(RECONNECT_ATTEMPTS - 1),
        Some(RECONNECT_MAX_DELAY),
        "and its last rung is the cap",
    );
    assert_eq!(reconnect_delay(RECONNECT_ATTEMPTS), None, "a station gone for good is left alone");
    assert_eq!(reconnect_delay(u32::MAX), None, "and stays left alone");
}

/// A ring nobody is reading is a socket nobody is listening to. The delay is served in
/// [`ABANDON_POLL`] slices so a dropped source is noticed within one of them rather than after the
/// whole wait, which at the top of the ladder is sixteen seconds of holding a connection open for
/// a station the user has already left.
#[test]
fn a_reconnect_wait_gives_up_as_soon_as_the_source_is_dropped() {
    let shared = StreamShared::new();
    let (source, _writer) = PrebufferSource::new(shared.clone(), shape(2, 48_000));
    drop(source);
    let wait = ABANDON_POLL * 40;

    let started = std::time::Instant::now();
    let carry_on = sleep_unless_abandoned(&shared, wait);

    assert!(!carry_on, "the source is gone, so there is nothing to reconnect for");
    // Half the wait rather than a slice of it: what this has to tell apart is a poll from serving
    // the whole delay, and the unpolled version takes all of it.
    assert!(started.elapsed() < wait / 2, "and it did not serve the wait out first");
}

/// Zero is the arm the loop never enters, so the trailing check is the only thing that can answer
/// it — and that check is also what catches a source dropped during the final slice, which no
/// amount of polling ahead of the sleep would see.
#[test]
fn a_wait_with_nothing_left_to_serve_still_reports_the_source() {
    let live = StreamShared::new();
    assert!(sleep_unless_abandoned(&live, Duration::ZERO));

    let abandoned = StreamShared::new();
    let (source, _writer) = PrebufferSource::new(abandoned.clone(), shape(2, 48_000));
    drop(source);
    assert!(!sleep_unless_abandoned(&abandoned, Duration::ZERO));
}

/// Building the decoder probes the container by *reading*, and `StreamDownload`'s reader parks the
/// calling thread until its downloader task delivers bytes. That task needs a worker, and the
/// runtime has two, so a probe on one takes half the runtime hostage and two stations take all of
/// it. Nothing reports it: the connect simply never returns, so it is pinned by reading the source
/// rather than by a test that would have to hang to fail.
#[test]
fn the_decoder_probe_is_built_on_the_blocking_pool() {
    const SOURCE: &str = include_str!("../stream_source.rs");
    const OPEN: &str = "StreamDecoder::open";
    const HANDOFF: &str = "spawn_blocking";

    let sites: Vec<usize> = SOURCE.match_indices(OPEN).map(|(at, _)| at).collect();
    assert!(!sites.is_empty(), "`{OPEN}` left stream_source.rs, so this pin reads nothing");

    for at in sites {
        let ahead = SOURCE.get(..at).unwrap_or_default();
        // The nearest `spawn_blocking` behind it, with nothing between them that could have ended
        // the statement it would have to be an argument of.
        let handed_off = ahead.rfind(HANDOFF).is_some_and(|from| !ahead[from..].contains(';'));
        assert!(
            handed_off,
            "the probe at byte {at} is not inside a `{HANDOFF}` closure, so it runs on a worker \
             and deadlocks against the download that would satisfy it"
        );
    }
}

/// A station that states no usable bitrate gets the fallback rather than a figure derived from a
/// zero, which the directory reports for a large share of live stations.
#[test]
fn a_station_with_no_stated_bitrate_prefetches_the_fallback() {
    assert_eq!(prefetch_bytes(None), PREFETCH_FALLBACK_BYTES);
    assert_eq!(prefetch_bytes(Some(0)), PREFETCH_FALLBACK_BYTES);
}

/// Both ends of the derived figure, worked against the constants rather than round numbers: the
/// floor is what stops a thin stream stuttering through its first seconds, the ceiling what stops
/// a fat one taking noticeably long to start, and between them the answer is
/// [`PREFETCH_SECONDS`] of audio at the stated rate.
#[test]
fn the_prefetch_is_two_seconds_of_audio_between_its_floor_and_its_ceiling() {
    // kbps → bytes, stepping either side of both clamps.
    let cases = [
        (131_u32, PREFETCH_MIN_BYTES),
        (132, 33_000),
        (320, 80_000),
        (524, 131_000),
        (525, PREFETCH_MAX_BYTES),
    ];
    for (kbps, expected) in cases {
        assert_eq!(prefetch_bytes(Some(kbps)), expected, "at {kbps} kbps");
    }

    // The unclamped middle really is the stated number of seconds.
    assert_eq!(prefetch_bytes(Some(320)), 320 * 1_000 / 8 * PREFETCH_SECONDS);
}
