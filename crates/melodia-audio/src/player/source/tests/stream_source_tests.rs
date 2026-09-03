use std::time::Duration;

use reqwest::Url;

use crate::player::source::stream_source::{
    first_stream_url, is_playlist_content_type, is_playlist_url, reconnect_delay,
};

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

#[test]
fn the_backoff_grows_and_then_gives_up() {
    let delays: Vec<Option<Duration>> = (0..8).map(reconnect_delay).collect();

    assert_eq!(delays.first(), Some(&Some(Duration::from_secs(1))));

    let attempted = delays.iter().take_while(|d| d.is_some()).count();
    assert!(attempted > 1, "a dropped stream must be retried more than once");
    assert!(
        delays[attempted..].iter().all(Option::is_none),
        "the budget must stay spent once it runs out"
    );

    let mut previous = Duration::ZERO;
    let mut grew = false;
    for delay in delays.iter().take(attempted).flatten() {
        assert!(*delay >= previous, "the backoff must never shrink");
        assert!(*delay <= Duration::from_mins(1), "the backoff must stay capped");
        grew |= *delay > previous;
        previous = *delay;
    }
    assert!(grew, "the backoff must actually back off");
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
