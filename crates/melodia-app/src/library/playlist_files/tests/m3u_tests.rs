use super::{parse, serialize};
use melodia_core::entities::track::PlaylistExportRow;

fn row(
    path: &str,
    hash: Option<&str>,
    title: &str,
    artist: Option<&str>,
    ms: i64,
) -> PlaylistExportRow {
    PlaylistExportRow {
        file_path: path.to_owned(),
        file_hash: hash.map(ToOwned::to_owned),
        title: title.to_owned(),
        artist: artist.map(ToOwned::to_owned),
        duration_ms: ms,
    }
}

#[test]
fn serialize_emits_header_name_and_tags() {
    let tracks = [row(
        "/music/Song.flac",
        Some("abc123"),
        "Get Lucky",
        Some("Daft Punk"),
        248_000,
    )];
    let text = serialize("Roadtrip", &tracks);

    assert!(text.starts_with("#EXTM3U\n"));
    assert!(text.contains("#PLAYLIST:Roadtrip\n"));
    assert!(text.contains("#EXTINF:248,Daft Punk - Get Lucky\n"));
    assert!(text.contains("#MELODIA-HASH:abc123\n"));
    assert!(text.contains("/music/Song.flac\n"));
    assert!(text.ends_with('\n'));
}

#[test]
fn serialize_omits_artist_and_hash_when_absent() {
    let tracks = [row("/music/Untitled.wav", None, "Untitled", None, 0)];
    let text = serialize("X", &tracks);

    // No artist => title only; unknown duration => -1; no hash line.
    assert!(text.contains("#EXTINF:-1,Untitled\n"));
    assert!(!text.contains("#MELODIA-HASH"));
    assert!(!text.contains(" - "));
}

#[test]
fn round_trip_preserves_path_hash_duration_and_name() {
    let tracks = [
        row("/m/a.flac", Some("HASHA"), "A", Some("Artist A"), 200_000),
        row("/m/b.flac", None, "B", None, 0),
    ];
    let parsed = parse(&serialize("My List", &tracks));

    assert_eq!(parsed.playlist_name.as_deref(), Some("My List"));
    assert_eq!(parsed.entries.len(), 2);

    assert_eq!(parsed.entries[0].path, "/m/a.flac");
    assert_eq!(parsed.entries[0].hash.as_deref(), Some("hasha")); // lowercased
    assert_eq!(parsed.entries[0].duration_secs, Some(200));
    assert_eq!(parsed.entries[0].display.as_deref(), Some("Artist A - A"));

    assert_eq!(parsed.entries[1].path, "/m/b.flac");
    assert_eq!(parsed.entries[1].hash, None);
    assert_eq!(parsed.entries[1].duration_secs, None); // -1 => None
}

#[test]
fn parse_tolerates_foreign_m3u_with_unknown_comments() {
    // VLC/foobar-style: no #PLAYLIST, no #MELODIA-HASH, extra comments.
    let text = "#EXTM3U\n\
                #EXTINF:181,Some Artist - Some Title\n\
                #EXTVLCOPT:network-caching=1000\n\
                /home/u/Music/song1.mp3\n\
                # a bare comment\n\
                /home/u/Music/song2.mp3\n";
    let parsed = parse(text);

    assert_eq!(parsed.playlist_name, None);
    assert_eq!(parsed.entries.len(), 2);
    assert_eq!(parsed.entries[0].path, "/home/u/Music/song1.mp3");
    assert_eq!(parsed.entries[0].duration_secs, Some(181));
    assert_eq!(parsed.entries[0].hash, None);
    assert_eq!(parsed.entries[1].path, "/home/u/Music/song2.mp3");
    // The bare comment between the two paths must not attach to song2.
    assert_eq!(parsed.entries[1].display, None);
}

#[test]
fn parse_strips_leading_bom() {
    let text = "\u{FEFF}#EXTM3U\n#PLAYLIST:BOMmy\n/m/a.mp3\n";
    let parsed = parse(text);
    assert_eq!(parsed.playlist_name.as_deref(), Some("BOMmy"));
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].path, "/m/a.mp3");
}

#[test]
fn parse_handles_crlf_line_endings() {
    let text = "#EXTM3U\r\n#PLAYLIST:CRLF\r\n#EXTINF:90,T\r\n/m/a.mp3\r\n";
    let parsed = parse(text);
    assert_eq!(parsed.playlist_name.as_deref(), Some("CRLF"));
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].path, "/m/a.mp3");
    assert_eq!(parsed.entries[0].duration_secs, Some(90));
}

#[test]
fn parse_empty_and_header_only_yield_no_entries() {
    assert!(parse("").entries.is_empty());
    assert!(parse("#EXTM3U\n").entries.is_empty());
    assert!(parse("\n\n   \n").entries.is_empty());
}

#[test]
fn parse_extinf_with_float_and_attributes() {
    let text = "#EXTM3U\n#EXTINF:213.7 tvg-id=\"x\",Artist - Title\n/m/a.mp3\n";
    let parsed = parse(text);
    assert_eq!(parsed.entries.len(), 1);
    // Leading integer part is taken; float fraction and attributes ignored.
    assert_eq!(parsed.entries[0].duration_secs, Some(213));
    assert_eq!(parsed.entries[0].display.as_deref(), Some("Artist - Title"));
}

#[test]
fn parse_garbage_extinf_duration_is_none_not_panic() {
    let text = "#EXTM3U\n#EXTINF:notanumber,T\n/m/a.mp3\n";
    let parsed = parse(text);
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].duration_secs, None);
    assert_eq!(parsed.entries[0].display.as_deref(), Some("T"));
}

#[test]
fn serialize_neutralizes_newlines_in_title() {
    let tracks = [row(
        "/m/a.mp3",
        None,
        "Line1\nLine2",
        Some("Art\rist"),
        1000,
    )];
    let text = serialize("Na\nme", &tracks);
    // The #PLAYLIST and #EXTINF lines must each stay single-line.
    assert!(text.contains("#PLAYLIST:Na me\n"));
    assert!(text.contains("#EXTINF:1,Art ist - Line1 Line2\n"));
}
