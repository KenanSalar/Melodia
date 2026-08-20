//! What the two station-list formats are easy to get wrong.
//!
//! A parser miss here is not an error the user sees — the import simply reports fewer stations
//! than the file held, or none at all, and there is nothing on screen naming which line was
//! dropped. So every shape a real file arrives in is spelled out rather than trusted.

use crate::entities::radio::RadioStation;

use super::{StationEntry, indexed_key, parse, serialize};

/// One entry, spelled the way the assertions read.
fn entry(name: Option<&str>, url: &str) -> StationEntry {
    StationEntry {
        name: name.map(str::to_owned),
        url: url.to_owned(),
    }
}

/// A stored station with only the two fields the writer touches.
fn station(name: &str, stream_url: &str) -> RadioStation {
    RadioStation {
        id: 1,
        station_uuid: None,
        name: name.to_owned(),
        stream_url: stream_url.to_owned(),
        homepage: None,
        favicon_url: None,
        artwork_path: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
        is_favorite: true,
        sort_key: name.to_lowercase(),
        date_added: "2026-08-21T00:00:00.000+00:00".to_owned(),
        last_played: None,
        play_count: 0,
    }
}

/// The format Melodia writes, read back by the parser it ships. A round trip is the one assertion
/// that catches a writer and a reader drifting apart, which is the failure neither can see alone.
#[test]
fn what_the_writer_emits_is_what_the_parser_reads_back() {
    let stations = [
        station("Radio One", "https://example.test/one"),
        station("Radio Two", "http://example.test/two.mp3"),
    ];
    let text = serialize(&stations);

    assert!(text.starts_with("#EXTM3U\n"), "an Extended-M3U8 file leads with its header");
    assert_eq!(
        parse(&text),
        vec![
            entry(Some("Radio One"), "https://example.test/one"),
            entry(Some("Radio Two"), "http://example.test/two.mp3"),
        ]
    );
}

/// A name carrying a newline would otherwise close the `#EXTINF` line early and leave the rest of
/// it parsed as a URL — a station named out of the directory's free-form fields can hold one.
#[test]
fn a_name_with_a_line_break_cannot_split_its_own_tag() {
    let text = serialize(&[station("Two\nLines", "https://example.test/one")]);
    assert_eq!(parse(&text), vec![entry(Some("Two Lines"), "https://example.test/one")]);
}

/// The shape a hand-written or foreign `.m3u` arrives in: no `#EXTINF` at all, unknown comments,
/// blank lines, and a BOM from whatever wrote it.
#[test]
fn a_bare_m3u_needs_no_tags_at_all() {
    let text = "\u{feff}#EXTM3U\n\
                # exported by something else\n\
                \n\
                https://example.test/one\n\
                https://example.test/two\n";
    assert_eq!(
        parse(text),
        vec![
            entry(None, "https://example.test/one"),
            entry(None, "https://example.test/two")
        ]
    );
}

/// `.pls` is what most stations are handed out as, and its titles are **not** required to follow
/// their files — real files put every `File` first and every `Title` after. Pairing by position
/// rather than by index would name both stations wrongly and look right on a one-entry file.
#[test]
fn a_pls_pairs_its_titles_by_index_and_not_by_order() {
    let text = "[playlist]\n\
                NumberOfEntries=2\n\
                File1=https://example.test/one\n\
                File2=https://example.test/two\n\
                Title1=Station One\n\
                Title2=Station Two\n\
                Length1=-1\n\
                Version=2\n";
    assert_eq!(
        parse(text),
        vec![
            entry(Some("Station One"), "https://example.test/one"),
            entry(Some("Station Two"), "https://example.test/two"),
        ]
    );
}

/// The `.pls` keys are case-insensitive in the wild, and `Length`/`Version`/`NumberOfEntries` all
/// parse as `key=value` too — a key filter that missed the case, or one that swallowed every
/// `key=value` line, would drop the stations or import the metadata as one.
#[test]
fn pls_keys_are_case_insensitive_and_the_rest_are_ignored() {
    let text = "[Playlist]\nnumberofentries=1\nfile1=https://example.test/one\ntitle1=Named\n";
    assert_eq!(parse(text), vec![entry(Some("Named"), "https://example.test/one")]);

    assert_eq!(indexed_key("File12", "File"), Some(12));
    assert_eq!(indexed_key("file1", "File"), Some(1));
    assert_eq!(indexed_key("Fil", "File"), None, "a key shorter than the prefix is not one");
    assert_eq!(indexed_key("Filename", "File"), None, "the tail has to be a number");
    assert_eq!(indexed_key("Title1", "File"), None);
}

/// A bare `.m3u` URL routinely carries a session token after a `?`, so its line holds an `=` and
/// reads as a `.pls` key/value pair under the rule above. Falling through to the whole-line
/// reading is what recovers it — and it is the only reason both formats can share one pass.
#[test]
fn an_m3u_url_with_a_query_string_is_not_read_as_a_pls_key() {
    let text = "#EXTM3U\n#EXTINF:-1,Tokened\nhttps://example.test/live?token=abc&x=1\n";
    assert_eq!(
        parse(text),
        vec![entry(
            Some("Tokened"),
            "https://example.test/live?token=abc&x=1"
        )]
    );
}

/// The scheme check is the whole filter, and it is a positive one: a playlist somebody sent you
/// can name a local path, and a station list is not a reason to reach for one.
#[test]
fn only_http_entries_are_taken() {
    let text = "#EXTM3U\n\
                file:///home/someone/track.mp3\n\
                /home/someone/track.mp3\n\
                ftp://example.test/stream\n\
                HTTPS://example.test/one\n";
    assert_eq!(
        parse(text),
        vec![entry(None, "HTTPS://example.test/one")],
        "the scheme is matched case-insensitively but the URL is kept verbatim"
    );
}

/// An `#EXTINF` with no title, and one whose station never arrives. The pending name must not
/// carry across to whatever URL comes next, or a nameless station inherits the one above it.
#[test]
fn a_dangling_extinf_names_nothing_below_it() {
    let text = "#EXTM3U\n\
                #EXTINF:-1,Orphan\n\
                #EXTINF:-1\n\
                https://example.test/one\n";
    assert_eq!(
        parse(text),
        vec![entry(None, "https://example.test/one")],
        "the second `#EXTINF` carries no title and must clear the first one's"
    );
}
