//! Reading the two playlist shapes, and telling both from the pointer `.m3u` most stations serve.
//!
//! Every fixture here is text a station really sends. What makes the parsing worth pinning is that
//! its mistakes are quiet: a pointer read as a segment playlist plays its first segment and stops,
//! a segment playlist read as a pointer does the same thing from the other direction, and a master
//! whose rungs are mis-sorted plays perfectly while spending megabits a second on a picture.

use super::*;

/// The playlist's own address, which every relative URI in these fixtures is joined against.
fn base() -> Url {
    let Ok(url) = Url::parse("https://example.invalid/live/master.m3u8") else {
        unreachable!("the fixture base is a literal absolute URL");
    };
    url
}

fn media(body: &str) -> Result<MediaPlaylist, AppError> {
    match parse(body, &base())? {
        Playlist::Media(playlist) => Ok(playlist),
        Playlist::Master(_) => unreachable!("this fixture declares no variants"),
    }
}

fn variants(body: &str) -> Result<Vec<Variant>, AppError> {
    match parse(body, &base())? {
        Playlist::Master(variants) => Ok(variants),
        Playlist::Media(_) => unreachable!("this fixture declares variants"),
    }
}

fn variant(url: &str, bandwidth: u64, has_video: bool) -> Variant {
    let Ok(url) = base().join(url) else {
        unreachable!("the fixture URLs are joinable against the fixture base");
    };
    Variant {
        url,
        bandwidth,
        has_video,
    }
}

const MEDIA_PLAYLIST: &str = "\
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:6
#EXT-X-MEDIA-SEQUENCE:42
#EXTINF:6.0,
seg-42.aac
#EXTINF:6.0,
seg-43.aac
";

const MASTER_PLAYLIST: &str = "\
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.2\"
audio-64.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=128000,CODECS=\"mp4a.40.2\"
audio-128.m3u8
";

/// The pointer both shapes have to be told apart from: a valid Extended M3U naming one mount.
const POINTER_PLAYLIST: &str = "\
#EXTM3U
#EXTINF:-1,Example Radio
https://example.invalid/stream
";

/// Keyed on the two tags the spec makes mandatory, one per playlist type.
///
/// The pointer is the case that matters. Read as a segment playlist its one line is a segment, so
/// the station opens, plays a few seconds of whatever that URL answers with, and stops.
#[test]
fn a_pointer_playlist_is_not_hls_and_both_real_shapes_are() {
    assert!(is_hls(MEDIA_PLAYLIST));
    assert!(is_hls(MASTER_PLAYLIST));
    assert!(!is_hls(POINTER_PLAYLIST));
    assert!(!is_hls("[playlist]\nFile1=https://example.invalid/stream\n"));
}

#[test]
fn a_media_playlist_resolves_its_segments_against_its_own_address() -> Result<(), AppError> {
    let playlist = media(MEDIA_PLAYLIST)?;

    assert_eq!(playlist.target_duration, Duration::from_secs(6));
    assert_eq!(playlist.media_sequence, 42);
    assert!(!playlist.ended);
    assert_eq!(playlist.init_segment, None);

    let urls: Vec<&str> = playlist.segments.iter().map(Url::as_str).collect();
    assert_eq!(
        urls,
        [
            "https://example.invalid/live/seg-42.aac",
            "https://example.invalid/live/seg-43.aac"
        ],
        "a relative segment name resolved against anything else reaches nothing"
    );
    Ok(())
}

/// A byte-order mark in front of `#EXTM3U`, which enough servers send that the first tag would
/// otherwise be unrecognisable and the playlist parse as having no duration at all.
#[test]
fn a_leading_byte_order_mark_is_not_part_of_the_first_tag() -> Result<(), AppError> {
    let playlist = media(&format!("\u{FEFF}{MEDIA_PLAYLIST}"))?;
    assert_eq!(playlist.target_duration, Duration::from_secs(6));
    Ok(())
}

/// The reload period is bounded at both ends, and `"inf"` is why the finite check is not
/// belt-and-braces: it parses, and `Duration::from_secs_f32` panics on it.
#[test]
fn the_reload_period_is_clamped_and_a_nonsense_one_is_refused() {
    let with_duration = |value: &str| {
        media(&format!("#EXTM3U\n#EXT-X-TARGETDURATION:{value}\nseg.aac\n"))
            .ok()
            .map(|playlist| playlist.target_duration)
    };

    assert_eq!(with_duration("0.05"), Some(Duration::from_secs(1)));
    assert_eq!(with_duration("600"), Some(Duration::from_secs(30)));
    assert_eq!(with_duration("6"), Some(Duration::from_secs(6)));

    for refused in ["inf", "0", "-4", "soon", ""] {
        assert_eq!(
            with_duration(refused),
            None,
            "`{refused}` was taken as a reload period rather than refused"
        );
    }
}

/// Encrypted segments are refused with a reason, and only where a key is really named.
///
/// A playlist that spells `METHOD=NONE` rather than omitting the tag is unencrypted, and refusing
/// it would drop stations that play.
#[test]
fn an_encrypted_playlist_is_refused_and_a_keyless_one_is_not() {
    let with_key = |method: &str| {
        media(&format!("#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-KEY:METHOD={method}\nseg.aac\n"))
    };

    assert!(with_key("AES-128,URI=\"key.bin\"").is_err());
    assert!(with_key("SAMPLE-AES").is_err());
    assert!(with_key("NONE").is_ok());
    assert!(with_key("none").is_ok(), "the method is spelled either way and means the same");
}

/// `EXT-X-MAP` is the header a fragmented stream's segments are meaningless without, so it has to
/// come out of the attribute list rather than off a line of its own, and resolve like a segment.
#[test]
fn a_fragmented_playlist_carries_its_init_segment() -> Result<(), AppError> {
    let playlist = media(
        "#EXTM3U\n\
         #EXT-X-TARGETDURATION:4\n\
         #EXT-X-MAP:URI=\"init.mp4\"\n\
         #EXTINF:4.0,\n\
         seg-1.m4s\n",
    )?;

    let Some(init) = &playlist.init_segment else {
        unreachable!("the fixture declares an `EXT-X-MAP`");
    };
    assert_eq!(init.as_str(), "https://example.invalid/live/init.mp4");
    assert_eq!(
        playlist.segments.len(),
        1,
        "the init is sent once, ahead of the first segment, and is not one"
    );
    Ok(())
}

/// A URI with a query string is the case a naive attribute split gets wrong: the value carries an
/// `=` of its own, and a comma inside the quotes is not a separator.
#[test]
fn an_init_segment_keeps_a_query_string_and_a_quoted_comma() -> Result<(), AppError> {
    let playlist = media(
        "#EXTM3U\n\
         #EXT-X-TARGETDURATION:4\n\
         #EXT-X-MAP:URI=\"init.mp4?token=a,b\",BYTERANGE=\"718@0\"\n\
         seg-1.m4s\n",
    )?;

    let Some(init) = &playlist.init_segment else {
        unreachable!("the fixture declares an `EXT-X-MAP`");
    };
    assert_eq!(init.as_str(), "https://example.invalid/live/init.mp4?token=a,b");
    Ok(())
}

#[test]
fn an_endlist_marks_the_playlist_finished() -> Result<(), AppError> {
    assert!(!media(MEDIA_PLAYLIST)?.ended);
    assert!(media(&format!("{MEDIA_PLAYLIST}#EXT-X-ENDLIST\n"))?.ended);
    Ok(())
}

#[test]
fn a_master_playlist_resolves_its_renditions() -> Result<(), AppError> {
    let variants = variants(MASTER_PLAYLIST)?;
    let urls: Vec<&str> = variants.iter().map(|variant| variant.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://example.invalid/live/audio-64.m3u8",
            "https://example.invalid/live/audio-128.m3u8"
        ]
    );
    assert!(variants.iter().all(|variant| !variant.has_video));
    Ok(())
}

/// `CODECS` is only a *should*, so the two attributes that come with a picture stand in for it.
///
/// Read from `CODECS` alone a simulcast ladder looks like a row of audio streams, and the pick
/// takes its richest rung: several Mbps of video, decoded and thrown away, on every station whose
/// master happens to omit one optional attribute.
#[test]
fn a_rendition_is_video_by_any_of_the_three_things_that_say_so() -> Result<(), AppError> {
    let declared = |attrs: &str| -> Result<bool, AppError> {
        let body = format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,{attrs}\nv.m3u8\n");
        let variants = variants(&body)?;
        let Some(variant) = variants.into_iter().next() else {
            unreachable!("the fixture declares one rendition");
        };
        Ok(variant.has_video)
    };

    assert!(declared("CODECS=\"mp4a.40.2,avc1.4d401f\"")?, "a quoted comma is not a separator");
    assert!(declared("RESOLUTION=1920x1080")?);
    assert!(declared("VIDEO=\"hi\"")?);
    assert!(declared("CODECS=\"av01.0.05M.08\"")?);
    assert!(!declared("CODECS=\"mp4a.40.2\"")?);
    assert!(!declared("AVERAGE-BANDWIDTH=700000")?, "a master that says nothing claims no picture");
    Ok(())
}

/// An audio rendition group is the playlist a simulcast names its single audio track in, and the
/// only thing in such a master worth opening.
///
/// Its line begins with `#`, so a parser reading only `EXT-X-STREAM-INF` skips it and falls
/// through to a video rung with the same audio muxed in.
#[test]
fn an_audio_rendition_group_is_a_rendition_and_the_other_kinds_are_not() -> Result<(), AppError> {
    let body = "\
#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"English\",DEFAULT=YES,URI=\"audio.m3u8\"
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"s\",NAME=\"English\",URI=\"subs.m3u8\"
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"b\",NAME=\"Muxed\"
#EXT-X-STREAM-INF:BANDWIDTH=2400000,CODECS=\"mp4a.40.2,avc1.4d401f\",AUDIO=\"a\"
video.m3u8
";
    let variants = variants(body)?;
    let audio: Vec<&str> = variants
        .iter()
        .filter(|variant| !variant.has_video)
        .map(|variant| variant.url.as_str())
        .collect();

    assert_eq!(
        audio,
        ["https://example.invalid/live/audio.m3u8"],
        "subtitles are not audio, and an audio group with no `URI` is muxed into the rungs"
    );

    let Some(picked) = pick_variant(variants) else {
        unreachable!("the fixture declares renditions");
    };
    assert_eq!(picked.url.as_str(), "https://example.invalid/live/audio.m3u8");
    Ok(())
}

/// The pick, in all four shapes a master arrives in.
#[test]
fn the_pick_takes_the_richest_audio_rung_and_the_poorest_video_one() {
    assert!(pick_variant(Vec::new()).is_none());

    let picked = |variants: Vec<Variant>| -> String {
        let Some(variant) = pick_variant(variants) else {
            unreachable!("these fixtures all declare renditions");
        };
        variant.url.as_str().to_owned()
    };

    assert_eq!(
        picked(vec![
            variant("audio-64.m3u8", 64_000, false),
            variant("audio-128.m3u8", 128_000, false),
            variant("video.m3u8", 2_400_000, true),
        ]),
        "https://example.invalid/live/audio-128.m3u8",
        "a picture is never worth reaching for, however much bandwidth it was given"
    );

    assert_eq!(
        picked(vec![
            variant("hi.m3u8", 4_000_000, true),
            variant("lo.m3u8", 400_000, true),
            variant("mid.m3u8", 1_200_000, true),
        ]),
        "https://example.invalid/live/lo.m3u8",
        "one audio track runs the whole ladder, so the cheapest rung carries it"
    );

    assert_eq!(
        picked(vec![
            variant("unstated.m3u8", 0, true),
            variant("lo.m3u8", 400_000, true),
        ]),
        "https://example.invalid/live/lo.m3u8",
        "`BANDWIDTH` is mandatory, so a rung reading zero named none and is not the cheapest"
    );

    assert_eq!(
        picked(vec![variant("a.m3u8", 0, true), variant("b.m3u8", 0, true)]),
        "https://example.invalid/live/a.m3u8",
        "a master where every rung omits it still has to resolve to one of them"
    );
}

/// A segment playlist is a valid Extended M3U, so the shapes are told apart by the tags each is
/// required to carry rather than by which parser errors first.
#[test]
fn a_body_carrying_no_variants_is_read_as_a_media_playlist() -> Result<(), AppError> {
    assert!(matches!(parse(MEDIA_PLAYLIST, &base())?, Playlist::Media(_)));
    assert!(matches!(parse(MASTER_PLAYLIST, &base())?, Playlist::Master(_)));
    Ok(())
}
