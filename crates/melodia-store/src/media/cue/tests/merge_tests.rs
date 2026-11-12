//! A sheet track's row: what it takes from the sheet, what from the image's tags for the whole
//! disc, what from neither, and which tracks fit inside the image at all.

use std::path::Path;

use super::*;
use crate::database::queries::fixtures::make_test_metadata;
use crate::media::cue::layout::lay_out;
use crate::media::cue::parse::{Files, parse};
use melodia_core::entities::album::ReleaseTagRow;
use melodia_core::entities::genre::GenreList;

/// The image's length, past every track the sheets here place unless a test shortens it.
const IMAGE_MS: i64 = 600_000;

fn image() -> ExtractedMetadata {
    let mut disc = make_test_metadata("Whole Disc");
    disc.duration_ms = IMAGE_MS;
    disc
}

fn sheet(lines: &[&str]) -> Sheet {
    parse(&lines.join("\n"), Path::new("album.cue"), Files::Named)
}

fn carve(lines: &[&str], disc: &ExtractedMetadata) -> Option<Vec<CueRow>> {
    let sheet = sheet(lines);
    let layout = lay_out(&sheet, Path::new("album.cue")).into_iter().next()?;
    rows(&sheet, &layout, Path::new("album.flac"), disc)
}

fn each<T>(lines: &[&str], disc: &ExtractedMetadata, field: impl Fn(&CueRow) -> T) -> Vec<T> {
    carve(lines, disc).unwrap_or_default().iter().map(field).collect()
}

const PLAIN: [&str; 6] = [
    "FILE \"album.wav\" WAVE",
    "TRACK 01 AUDIO",
    "TITLE \"One\"",
    "INDEX 01 00:00:00",
    "TRACK 02 AUDIO",
    "INDEX 01 04:00:00",
];

#[test]
fn a_track_untitled_by_its_sheet_is_named_for_the_image_and_its_number() {
    let titles = each(&PLAIN, &image(), |row| row.meta.title.clone());
    assert_eq!(titles, ["One", "album 02"], "the image's own title names the disc, not a track");
}

#[test]
fn the_hidden_track_is_numbered_zero_and_named_for_it() {
    let lines = [
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "INDEX 00 00:00:00",
        "INDEX 01 00:30:00",
        "TRACK 02 AUDIO",
        "INDEX 01 04:00:00",
    ];

    let rows = each(&lines, &image(), |row| (row.meta.track_number, row.meta.title.clone()));

    assert_eq!(rows.first(), Some(&(Some(0), "album 00".to_owned())));
}

#[test]
fn a_track_is_numbered_by_its_sheet_and_counted_among_its_image_s_audio_tracks() {
    let lines = [
        "FILE \"album.bin\" BINARY",
        "TRACK 01 AUDIO",
        "INDEX 01 00:00:00",
        "TRACK 02 MODE1/2352",
        "INDEX 01 03:00:00",
        "TRACK 03 AUDIO",
        "INDEX 01 04:00:00",
    ];

    let numbers = each(&lines, &image(), |row| (row.meta.track_number, row.meta.track_total));

    assert_eq!(numbers, [(Some(1), Some(2)), (Some(3), Some(2))]);
}

#[test]
fn a_track_s_artist_is_its_own_then_the_sheet_s_then_the_image_s() {
    let with_sheet_performer = [
        "PERFORMER \"Sheet Artist\"",
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "PERFORMER \"Track Artist\"",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 01 04:00:00",
    ];
    let artist = |row: &CueRow| row.meta.artist.line().map(str::to_owned);

    let stated = each(&with_sheet_performer, &image(), artist);
    let unstated = each(&PLAIN, &image(), artist);

    assert_eq!(stated, [Some("Track Artist".to_owned()), Some("Sheet Artist".to_owned())]);
    assert_eq!(unstated, [Some("Test Artist".to_owned()), Some("Test Artist".to_owned())]);
}

#[test]
fn the_album_and_album_artist_are_the_sheet_s_else_the_image_s() {
    let mut disc = image();
    disc.album_artist = ArtistCredit::from_name("Image Album Artist");
    let stated =
        ["TITLE \"Sheet Album\"", "PERFORMER \"Sheet Album Artist\"", "FILE \"a.wav\" WAVE"];
    let lines = [&stated[..], &PLAIN[1..]].concat();
    let album =
        |row: &CueRow| (row.meta.album.clone(), row.meta.album_artist.line().map(str::to_owned));

    let from_sheet = each(&lines, &disc, album);
    let from_image = each(&PLAIN, &disc, album);

    let sheet_album = (Some("Sheet Album".to_owned()), Some("Sheet Album Artist".to_owned()));
    let image_album = (Some("Test Album".to_owned()), Some("Image Album Artist".to_owned()));
    assert_eq!((from_sheet.first(), from_image.first()), (Some(&sheet_album), Some(&image_album)));
}

#[test]
fn a_performer_restating_the_image_s_credit_keeps_its_names_ids_and_sort() {
    let mut disc = image();
    disc.artist = ArtistCredit::from_tags("A feat. B", &["A".to_owned(), "B".to_owned()]);
    disc.artist_mbids = vec!["mbid-a".to_owned(), "mbid-b".to_owned()];
    disc.sort.artist = Some("A, The".to_owned());
    let lines = [
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "PERFORMER \"A feat. B\"",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "PERFORMER \"C\"",
        "INDEX 01 04:00:00",
    ];

    let credits = each(&lines, &disc, |row| {
        let names: Vec<String> = row.meta.artist.artists().iter().map(|a| a.name.clone()).collect();
        (names, row.meta.artist_mbids.len(), row.meta.sort.artist.clone())
    });

    assert_eq!(
        credits,
        [
            (vec!["A".to_owned(), "B".to_owned()], 2, Some("A, The".to_owned())),
            (vec!["C".to_owned()], 0, None),
        ]
    );
}

#[test]
fn a_sheet_album_artist_restating_the_image_s_keeps_its_ids() {
    let mut disc = image();
    disc.album_artist = ArtistCredit::from_name("Disc Artist");
    disc.album_artist_mbids = vec!["mbid-disc".to_owned()];
    let lines = [&["PERFORMER \"Disc Artist\""][..], &PLAIN].concat();

    let ids = each(&lines, &disc, |row| row.meta.album_artist_mbids.clone());

    assert_eq!(ids.first(), Some(&vec!["mbid-disc".to_owned()]));
}

fn composer(name: &str) -> RoleCredit {
    RoleCredit { role: CreditRole::Composer, name: name.to_owned(), detail: String::new() }
}

fn credits_of(row: &CueRow) -> Vec<(CreditRole, String)> {
    row.meta.credits.all().iter().map(|credit| (credit.role, credit.name.clone())).collect()
}

#[test]
fn a_track_s_composers_come_from_its_own_lines_then_the_sheet_s_every_other_role_kept() {
    let mut disc = image();
    disc.credits = RoleCredits::new(vec![
        composer("Image Composer"),
        RoleCredit {
            role: CreditRole::Producer,
            name: "Producer".to_owned(),
            detail: String::new(),
        },
    ]);
    let lines = [
        "SONGWRITER \"Sheet Composer\"",
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "SONGWRITER \"X; Y\"",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "REM COMPOSER \"Z\"",
        "INDEX 01 03:00:00",
        "TRACK 03 AUDIO",
        "INDEX 01 06:00:00",
    ];

    let credits = each(&lines, &disc, credits_of);

    let producer = (CreditRole::Producer, "Producer".to_owned());
    let composer = |name: &str| (CreditRole::Composer, name.to_owned());
    assert_eq!(
        credits,
        [
            vec![composer("X"), composer("Y"), producer.clone()],
            vec![composer("Z"), producer.clone()],
            vec![composer("Sheet Composer"), producer],
        ]
    );
}

#[test]
fn a_sheet_naming_no_composer_keeps_the_image_s() {
    let mut disc = image();
    disc.credits = RoleCredits::new(vec![composer("Image Composer")]);

    let credits = each(&PLAIN, &disc, credits_of);

    assert_eq!(credits.first(), Some(&vec![(CreditRole::Composer, "Image Composer".to_owned())]));
}

#[test]
fn genre_and_date_are_the_track_s_then_the_sheet_s() {
    let lines = [
        "REM GENRE \"Sheet Genre\"",
        "REM DATE 1999",
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "REM GENRE \"Track Genre\"",
        "REM DATE 2001-05-04",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 01 04:00:00",
    ];

    let fields = each(&lines, &image(), |row| {
        (row.meta.genres.names().to_vec(), row.meta.year, row.meta.release_date.clone())
    });

    assert_eq!(
        fields,
        [
            (vec!["Track Genre".to_owned()], Some(2001), Some("2001-05-04".to_owned())),
            (vec!["Sheet Genre".to_owned()], Some(1999), Some("1999".to_owned())),
        ]
    );
}

#[test]
fn a_date_that_does_not_read_keeps_the_image_s_year() {
    let lines = [&["REM DATE someday"][..], &PLAIN].concat();
    assert_eq!(each(&lines, &image(), |row| row.meta.year).first(), Some(&Some(2024)));
}

#[test]
fn disc_numbers_are_the_sheet_s_else_the_image_s() {
    let lines = [&["REM DISCNUMBER 2", "REM TOTALDISCS 3"][..], &PLAIN].concat();
    let discs = |row: &CueRow| (row.meta.disc_number, row.meta.disc_total);

    let stated = each(&lines, &image(), discs);
    let unstated = each(&PLAIN, &image(), discs);

    assert_eq!(
        (stated.first(), unstated.first()),
        (Some(&(Some(2), Some(3))), Some(&(Some(1), None)))
    );
}

#[test]
fn isrc_is_the_track_s_own_or_none() {
    let mut disc = image();
    disc.isrc = Some("IMAGE0000001".to_owned());
    let lines = [
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "ISRC USRC17607839",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 01 04:00:00",
    ];

    let isrcs = each(&lines, &disc, |row| row.meta.isrc.clone());

    assert_eq!(isrcs, [Some("USRC17607839".to_owned()), None]);
}

#[test]
fn the_barcode_is_the_sheet_s_catalog_else_the_image_s() {
    let mut disc = image();
    disc.release.barcode = Some("IMAGE-BARCODE".to_owned());
    let lines = [&["CATALOG 0724384960650"][..], &PLAIN].concat();
    let barcode = |row: &CueRow| row.meta.release.barcode.clone();

    let stated = each(&lines, &disc, barcode);
    let unstated = each(&PLAIN, &disc, barcode);

    assert_eq!(
        (stated.first(), unstated.first()),
        (Some(&Some("0724384960650".to_owned())), Some(&Some("IMAGE-BARCODE".to_owned())))
    );
}

#[test]
fn what_describes_one_recording_is_no_track_s() {
    let mut disc = image();
    disc.bpm = Some(120.0);
    disc.mood = Some("Happy".to_owned());
    disc.rating = Some(80);
    disc.musicbrainz_track_id = Some("recording".to_owned());
    disc.sort.title = Some("Disc, The".to_owned());

    let fields = each(&PLAIN, &disc, |row| {
        let m = &row.meta;
        (m.bpm, m.mood.clone(), m.rating, m.musicbrainz_track_id.clone(), m.sort.title.clone())
    });

    assert_eq!(fields.first(), Some(&(None, None, None, None, None)));
}

#[test]
fn what_describes_the_image_s_audio_stays() {
    let mut disc = image();
    disc.artwork_path = Some("cover.jpg".to_owned());

    let audio = each(&PLAIN, &disc, |row| {
        (row.meta.codec.clone(), row.meta.sample_rate, row.meta.artwork_path.clone())
    });

    assert_eq!(
        audio.first(),
        Some(&(Some("Mpeg".to_owned()), Some(44_100), Some("cover.jpg".to_owned())))
    );
}

#[test]
fn a_row_carries_no_sheet_of_its_own() {
    let mut disc = image();
    disc.embedded_cue = EmbeddedCue::Unread;

    let carried = each(&PLAIN, &disc, |row| row.meta.embedded_cue.clone());

    assert_eq!(carried, [EmbeddedCue::Absent, EmbeddedCue::Absent]);
}

/// Within 0.001, gains in dB and linear peaks alike: each is parsed from text, never computed.
fn is_near(value: Option<f64>, expected: f64) -> bool {
    value.is_some_and(|value| (value - expected).abs() < 1e-3)
}

#[test]
fn the_album_gain_is_the_sheet_s_over_the_image_s() {
    let mut disc = image();
    disc.replaygain_album_gain = Some(-5.0);
    let lines =
        [&["REM REPLAYGAIN_ALBUM_GAIN -6.50 dB", "REM REPLAYGAIN_ALBUM_PEAK 0.9"][..], &PLAIN]
            .concat();

    let album =
        each(&lines, &disc, |row| (row.meta.replaygain_album_gain, row.meta.replaygain_album_peak));

    let (gain, peak) = album.first().copied().unwrap_or_default();
    assert!(is_near(gain, -6.5) && is_near(peak, 0.9), "{album:?}");
}

#[test]
fn the_image_s_album_gain_stands_where_the_sheet_states_none() {
    let mut disc = image();
    disc.replaygain_album_gain = Some(-5.0);
    disc.replaygain_track_gain = Some(-4.0);

    let gains = each(&PLAIN, &disc, |row| row.meta.replaygain_album_gain);

    assert!(gains.iter().all(|&gain| is_near(gain, -5.0)), "{gains:?}");
}

#[test]
fn the_image_s_track_gain_is_the_album_s_where_it_states_no_album_gain() {
    let mut disc = image();
    disc.replaygain_track_gain = Some(-4.0);
    disc.replaygain_track_peak = Some(0.8);

    let album =
        each(&PLAIN, &disc, |row| (row.meta.replaygain_album_gain, row.meta.replaygain_album_peak));

    let (gain, peak) = album.first().copied().unwrap_or_default();
    assert!(is_near(gain, -4.0) && is_near(peak, 0.8), "{album:?}");
}

#[test]
fn the_track_gain_is_the_track_s_own_or_none() {
    let mut disc = image();
    disc.replaygain_track_gain = Some(-4.0);
    let lines = [
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "REM REPLAYGAIN_TRACK_GAIN -3.25 dB",
        "INDEX 01 00:00:00",
        "TRACK 02 AUDIO",
        "INDEX 01 04:00:00",
    ];

    let gains = each(&lines, &disc, |row| row.meta.replaygain_track_gain);

    assert!(matches!(gains.as_slice(), [first, None] if is_near(*first, -3.25)), "{gains:?}");
}

#[test]
fn a_track_lasts_its_span_and_the_last_lasts_to_the_end_of_the_image() {
    let durations = each(&PLAIN, &image(), |row| row.meta.duration_ms);
    assert_eq!(durations, [240_000, IMAGE_MS - 240_000]);
}

#[test]
fn a_track_starting_at_or_past_the_end_of_the_image_is_dropped() {
    let rows = [("09:59:74", vec![1, 2, 3]), ("10:00:00", vec![1, 2]), ("12:00:00", vec![1, 2])];
    for (third, kept) in rows {
        let index = format!("INDEX 01 {third}");
        let lines = [&PLAIN[..], &["TRACK 03 AUDIO", &index]].concat();
        let numbers = each(&lines, &image(), |row| row.track.number());
        assert_eq!(numbers, kept, "track 3 at {third}");
    }
}

#[test]
fn a_track_ending_past_the_end_of_the_image_plays_to_the_end_of_the_file() {
    let lines = [&PLAIN[..], &["TRACK 03 AUDIO", "INDEX 01 12:00:00"]].concat();

    let second = each(&lines, &image(), |row| (row.span.end(), row.meta.duration_ms)).pop();

    assert_eq!(second, Some((None, IMAGE_MS - 240_000)));
}

#[test]
fn an_image_left_with_fewer_than_two_tracks_is_not_split() {
    let mut disc = image();
    disc.duration_ms = 200_000;

    assert!(carve(&PLAIN, &disc).is_none());
}

#[test]
fn an_image_of_unknown_length_drops_nothing() {
    let mut disc = image();
    disc.duration_ms = 0;
    let lines = [&PLAIN[..], &["TRACK 03 AUDIO", "INDEX 01 99:00:00"]].concat();

    let numbers = each(&lines, &disc, |row| (row.track.number(), row.meta.duration_ms));

    assert_eq!(numbers, [(1, 240_000), (2, 5_700_000), (3, 0)]);
}

/// The image's tags as Edit Tags reads them for a file filed outside the library.
fn image_tags() -> FileTags {
    FileTags {
        lyrics: Some("The disc's lyrics".to_owned()),
        credits: (
            ArtistCredit::from_name("Image Artist"),
            ArtistCredit::from_name("Image Album Artist"),
        ),
        roles: RoleCredits::new(vec![composer("Image Composer")]),
        genres: GenreList::from_name("Image Genre"),
        release: ReleaseTagRow { barcode: "IMAGE-BARCODE".to_owned(), ..ReleaseTagRow::default() },
    }
}

#[test]
fn a_track_s_tags_read_over_its_image_s_by_the_scan_s_rules() {
    let stated = sheet(&[
        "PERFORMER \"Sheet Artist\"",
        "REM GENRE \"Sheet Genre\"",
        "CATALOG 0724384960650",
        "FILE \"album.wav\" WAVE",
        "TRACK 01 AUDIO",
        "PERFORMER \"Track Artist\"",
        "SONGWRITER \"Track Composer\"",
        "INDEX 01 00:00:00",
    ]);
    let track = CueTrack::try_from(1).unwrap_or(CueTrack::HIDDEN);

    let tags = track_tags(&stated, track, image_tags());

    let expected = FileTags {
        lyrics: None,
        credits: (ArtistCredit::from_name("Track Artist"), ArtistCredit::from_name("Sheet Artist")),
        roles: RoleCredits::new(vec![composer("Track Composer")]),
        genres: GenreList::from_name("Sheet Genre"),
        release: ReleaseTagRow { barcode: "0724384960650".to_owned(), ..ReleaseTagRow::default() },
    };
    assert_eq!(tags, expected);
}

#[test]
fn a_track_the_sheet_says_nothing_of_reads_its_image_s_tags_less_the_lyrics() {
    let stated = sheet(&PLAIN);

    let tags = track_tags(&stated, CueTrack::HIDDEN, image_tags());

    assert_eq!(tags, FileTags { lyrics: None, ..image_tags() });
}
