use tempfile::TempDir;

use super::*;
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::error::AppError;

/// Creates a minimal valid WAV file (44-byte header + 4 bytes PCM data).
/// This is the smallest file that Symphonia/Lofty can parse.
fn create_minimal_wav(path: &std::path::Path) -> Result<(), AppError> {
    let sample_rate: u32 = 44_100;
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let data_size: u32 = 4; // 2 samples of 16-bit mono
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
    let block_align = channels * bits_per_sample / 8;
    let file_size = 36 + data_size;

    let mut buf = Vec::with_capacity(44 + data_size as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    buf.extend_from_slice(&[0u8; 4]); // silent samples

    std::fs::write(path, &buf)?;
    Ok(())
}

fn test_cover_cache() -> CoverCache {
    melodia_artwork::media::image::artwork::new_cover_cache()
}

#[test]
fn parse_gain_standard() {
    assert_eq!(parse_replaygain_gain("-6.50 dB"), Some(-6.5));
}

#[test]
fn parse_gain_positive() {
    assert_eq!(parse_replaygain_gain("+3.21 dB"), Some(3.21));
}

#[test]
fn parse_gain_zero() {
    assert_eq!(parse_replaygain_gain("0.00 dB"), Some(0.0));
}

#[test]
fn parse_gain_no_db_suffix() {
    assert_eq!(parse_replaygain_gain("-6.50"), Some(-6.5));
}

#[test]
fn parse_gain_extra_whitespace() {
    assert_eq!(parse_replaygain_gain("  -6.50 dB  "), Some(-6.5));
}

#[test]
fn parse_gain_invalid() {
    assert_eq!(parse_replaygain_gain("not a number"), None);
}

#[test]
fn parse_gain_empty() {
    assert_eq!(parse_replaygain_gain(""), None);
}

#[test]
fn parse_gain_rejects_non_finite() {
    // Rust's float parser accepts "nan"/"inf"; a non-finite gain baked into the
    // audio source would render the track as silence, so it must map to None.
    assert_eq!(parse_replaygain_gain("nan dB"), None);
    assert_eq!(parse_replaygain_gain("inf dB"), None);
    assert_eq!(parse_replaygain_gain("-inf"), None);
}

#[test]
fn parse_peak_standard() {
    assert_eq!(parse_replaygain_peak("0.988553"), Some(0.988_553));
}

#[test]
fn parse_peak_whitespace() {
    assert_eq!(parse_replaygain_peak("  1.0  "), Some(1.0));
}

#[test]
fn parse_peak_invalid() {
    assert_eq!(parse_replaygain_peak("abc"), None);
}

#[test]
fn parse_peak_rejects_non_finite() {
    // Same guard as the gain parser — a non-finite peak breaks the clip clamp.
    assert_eq!(parse_replaygain_peak("nan"), None);
    assert_eq!(parse_replaygain_peak("inf"), None);
}

// ── extract_metadata ──

#[test]
fn extract_metadata_wav_basic_properties() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let wav_path = tmp.path().join("test.wav");
    create_minimal_wav(&wav_path)?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;

    let meta = extract_metadata(&wav_path, &artwork_dir, &test_cover_cache(), false)?;

    let sample_rate =
        meta.sample_rate.ok_or_else(|| AppError::Validation("missing sample_rate".into()))?;
    assert_eq!(sample_rate, 44_100);
    let channels = meta.channels.ok_or_else(|| AppError::Validation("missing channels".into()))?;
    assert_eq!(channels, 1);
    assert!(meta.codec.is_some());
    assert!(meta.file_size > 0);
    Ok(())
}

#[test]
fn extract_metadata_file_not_found_returns_error() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;

    let result = extract_metadata(
        &tmp.path().join("nonexistent.mp3"),
        &artwork_dir,
        &test_cover_cache(),
        false,
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
fn extract_metadata_non_audio_file_returns_error() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let txt_path = tmp.path().join("notes.txt");
    std::fs::write(&txt_path, "not audio data")?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;

    let result = extract_metadata(&txt_path, &artwork_dir, &test_cover_cache(), false);
    assert!(result.is_err());
    Ok(())
}

#[test]
fn extract_metadata_title_falls_back_to_filename() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let wav_path = tmp.path().join("My Song.wav");
    create_minimal_wav(&wav_path)?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;

    let meta = extract_metadata(&wav_path, &artwork_dir, &test_cover_cache(), false)?;

    // WAV without tags should fall back to file stem as title
    assert_eq!(meta.title, "My Song");
    Ok(())
}

#[test]
fn extract_metadata_skip_artwork_flag() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let wav_path = tmp.path().join("test.wav");
    create_minimal_wav(&wav_path)?;
    // Put a cover art file in the directory
    std::fs::write(tmp.path().join("cover.jpg"), b"fake image")?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;

    let meta = extract_metadata(&wav_path, &artwork_dir, &test_cover_cache(), true)?;

    // With skip_artwork=true, artwork_path should be None
    assert!(meta.artwork_path.is_none());
    Ok(())
}

#[test]
fn extract_metadata_file_size_recorded() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let wav_path = tmp.path().join("test.wav");
    create_minimal_wav(&wav_path)?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;

    let actual_size = i64::try_from(std::fs::metadata(&wav_path)?.len())
        .map_err(|_| AppError::Validation("file size exceeds i64".into()))?;
    let meta = extract_metadata(&wav_path, &artwork_dir, &test_cover_cache(), false)?;

    assert_eq!(meta.file_size, actual_size);
    Ok(())
}

// ── the containers the extension list gained ──

fn assets_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(melodia_testkit::ASSETS_DIR)
}

/// Copy a checked-in fixture into `tmp` under `name`, so a rename is free and the
/// artwork lookup can't see `test-assets/cover.jpg` sitting beside the original.
fn stage_as(tmp: &TempDir, fixture: &str, name: &str) -> Result<std::path::PathBuf, AppError> {
    let dst = tmp.path().join(name);
    std::fs::copy(assets_dir().join(fixture), &dst)?;
    Ok(dst)
}

/// `.oga` is the reason `read_tags` consults the header at all: lofty's extension map
/// stops at `.ogg`, so this file is anonymous by name and fully readable by content.
#[test]
fn extract_metadata_reads_an_oga_by_its_header() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let oga = stage_as(&tmp, "silence.ogg", "quiet.oga")?;

    let meta = extract_metadata(&oga, &artwork_dir, &test_cover_cache(), false)?;

    assert_eq!(meta.codec.as_deref(), Some("Vorbis"));
    assert_eq!(meta.sample_rate, Some(44_100));
    assert!(meta.duration_ms > 0, "an identified Ogg should carry a duration");
    Ok(())
}

/// `.aif` and `.m4b` are the containers lofty already reads under their longer names.
/// Only the extension list stood between them and the library.
#[test]
fn extract_metadata_reads_the_alias_extensions() -> Result<(), AppError> {
    for (fixture, alias, codec) in
        [("silence.aiff", "quiet.aif", "Aiff"), ("silence.m4a", "quiet.m4b", "Mp4")]
    {
        let tmp = TempDir::new()?;
        let artwork_dir = tmp.path().join("artwork");
        std::fs::create_dir(&artwork_dir)?;
        let path = stage_as(&tmp, fixture, alias)?;

        let meta = extract_metadata(&path, &artwork_dir, &test_cover_cache(), false)?;

        assert_eq!(meta.codec.as_deref(), Some(codec), "{alias} read as the wrong container");
        assert!(meta.duration_ms > 0, "{alias} carries no duration");
    }
    Ok(())
}

/// AIFF-C is a distinct RIFF form from AIFF, and symphonia parses only a fixed set of
/// its compression types, so this needs a real `AIFC` fixture rather than a renamed one.
#[test]
fn extract_metadata_reads_an_aifc() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let aifc = stage_as(&tmp, "silence.aifc", "quiet.aifc")?;

    let meta = extract_metadata(&aifc, &artwork_dir, &test_cover_cache(), false)?;

    assert_eq!(meta.codec.as_deref(), Some("Aiff"));
    assert_eq!(meta.sample_rate, Some(44_100));
    assert!(meta.duration_ms > 0);
    Ok(())
}

/// Matroska and CAF decode but have no lofty reader, so they exist in the library only
/// through the fallback. The duration is the decoder's answer, not lofty's.
#[test]
fn containers_with_no_tag_reader_become_filename_rows() -> Result<(), AppError> {
    for (fixture, name) in [("silence.mka", "quiet.mka"), ("silence.caf", "quiet.caf")] {
        let tmp = TempDir::new()?;
        let artwork_dir = tmp.path().join("artwork");
        std::fs::create_dir(&artwork_dir)?;
        let path = stage_as(&tmp, fixture, name)?;

        assert!(
            extract_metadata(&path, &artwork_dir, &test_cover_cache(), false).is_err(),
            "{fixture} has no lofty reader, so the strict path must report that"
        );

        let meta = extract_or_filename_row(&path, &artwork_dir, &test_cover_cache(), false)?;
        assert_eq!(meta.title, "quiet");
        assert_eq!(meta.codec, None);
        assert!(meta.duration_ms > 0, "{fixture} should get a duration from the decoder");
    }
    Ok(())
}

/// Pins `sniff_file_type` to `FileType::from_buffer` over `Probe::guess_file_type`.
///
/// The latter falls through to scanning the first kilobyte for an MPEG frame sync, and
/// Matroska's payload contains that byte pair: this fixture came back labelled AAC at
/// 24 kHz lasting two seconds, none of which is true of a one-second 44.1 kHz FLAC.
/// Wrong metadata is worse than none, because nothing downstream can tell.
#[test]
fn an_unreadable_container_is_never_guessed_from_its_payload() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    let mka = stage_as(&tmp, "silence.mka", "quiet.mka")?;

    let meta = extract_or_filename_row(&mka, &artwork_dir, &test_cover_cache(), false)?;

    assert_eq!(meta.codec, None, "a container lofty can't read must not acquire a codec");
    assert_eq!(meta.sample_rate, None);
    assert_eq!(meta.channels, None);
    assert_eq!(meta.bitrate, None);
    Ok(())
}

// === What a tag's multi-value fields read as ===

use lofty::tag::{ItemValue, Tag, TagItem, TagType};

/// A Vorbis tag holding exactly the values given, repeats included.
fn tagged(values: &[(ItemKey, &str)]) -> Tag {
    let mut tag = Tag::new(TagType::VorbisComments);
    for (key, value) in values {
        tag.push(TagItem::new(*key, ItemValue::Text((*value).to_owned())));
    }
    tag
}

fn credit_names(tag: &Tag) -> Vec<String> {
    credits_from_tag(Some(tag)).0.artists().iter().map(|a| a.name.clone()).collect()
}

#[test]
fn a_whitespace_only_value_is_not_a_value() {
    let tag = tagged(&[
        (ItemKey::Composer, "Alice"),
        (ItemKey::Composer, "   "),
        (ItemKey::Composer, ""),
    ]);

    assert_eq!(trimmed_values(&tag, ItemKey::Composer), ["Alice"]);
    assert!(trimmed_values(&tag, ItemKey::Mood).is_empty());
}

/// **A repeated `GENRE` is the multi-value form and needs no splitting.** A single value is split
/// on `;` and nothing else — `ID3v1`'s own list contains `Pop/Funk`, so a slash is genuinely
/// ambiguous where a semicolon is not, and a comma or ampersand lives inside names like
/// `Drum & Bass`.
#[test]
fn a_single_genre_value_splits_on_semicolons_and_on_nothing_else() {
    let table = [
        ("Rock; Metal", vec!["Rock", "Metal"]),
        ("Rock;Metal", vec!["Rock", "Metal"]),
        ("Pop/Funk", vec!["Pop/Funk"]),
        ("Drum & Bass", vec!["Drum & Bass"]),
        ("Chanson, Francaise", vec!["Chanson, Francaise"]),
        ("Rock; ; Metal", vec!["Rock", "Metal"]),
    ];

    for (value, expected) in table {
        let genres = read_genres(Some(&tagged(&[(ItemKey::Genre, value)])));

        assert_eq!(genres.names(), expected, "{value} split wrong");
    }
}

#[test]
fn a_repeated_genre_is_taken_as_the_list_it_already_is() {
    let tag = tagged(&[(ItemKey::Genre, "Rock"), (ItemKey::Genre, "Drum & Bass")]);

    assert_eq!(read_genres(Some(&tag)).names(), ["Rock", "Drum & Bass"]);
}

#[test]
fn a_tag_with_no_genre_at_all_reads_as_no_genres() {
    assert!(read_genres(None).is_empty());
    assert!(read_genres(Some(&tagged(&[]))).is_empty());
}

/// **A single `ARTIST` value is one artist whatever delimiters it contains** — that is what the
/// list tag exists for, and the exceptions list that splitting on punctuation would need is not
/// one anybody can finish.
#[test]
fn a_lone_artist_value_is_one_name_however_it_is_punctuated() {
    for printed in ["AC/DC", "Earth, Wind & Fire", "Alice feat. Bob"] {
        let tag = tagged(&[(ItemKey::TrackArtist, printed)]);

        assert_eq!(credit_names(&tag), [printed]);
    }
}

#[test]
fn the_list_tag_wins_over_the_printed_one() {
    let tag = tagged(&[
        (ItemKey::TrackArtist, "Alice feat. Bob"),
        (ItemKey::TrackArtists, "Alice"),
        (ItemKey::TrackArtists, "Bob"),
    ]);

    assert_eq!(credit_names(&tag), ["Alice", "Bob"]);
    assert_eq!(credits_from_tag(Some(&tag)).0.line(), Some("Alice feat. Bob"));
}

/// **`ID3v2.3` has no multi-value frame**, so Picard flattens `ARTISTS` to `"Alice; Bob"` — which
/// is most MP3s in the wild, and left whole it reads as one artist named that.
#[test]
fn a_flattened_list_tag_splits_back_into_its_names() {
    let tag =
        tagged(&[(ItemKey::TrackArtist, "Alice & Bob"), (ItemKey::TrackArtists, "Alice; Bob")]);

    assert_eq!(credit_names(&tag), ["Alice", "Bob"]);
}

/// The separator carries its space, and the split runs on the list key only — so a bare `;` inside
/// one name stays inside it.
#[test]
fn a_semicolon_with_no_space_stays_inside_the_name() {
    let tag = tagged(&[(ItemKey::TrackArtists, "Alice;Bob")]);

    assert_eq!(credit_names(&tag), ["Alice;Bob"]);
}

#[test]
fn the_album_artist_reads_through_its_own_pair_of_keys() {
    let tag = tagged(&[
        (ItemKey::AlbumArtist, "Alice & Bob"),
        (ItemKey::AlbumArtists, "Alice"),
        (ItemKey::AlbumArtists, "Bob"),
        (ItemKey::TrackArtist, "Carol"),
    ]);

    let (track, album) = credits_from_tag(Some(&tag));

    assert_eq!(track.primary_name(), "Carol");
    assert_eq!(album.artists().len(), 2);
    assert_eq!(album.primary_name(), "Alice");
}

/// **All of them or none.** Picard writes one id per credited artist in the same order, so a file
/// whose counts disagree has no alignment left to trust — and a misaligned id stamps the wrong
/// `MBID` onto a real artist, which nothing downstream can detect or undo.
#[test]
fn artist_ids_are_kept_only_where_they_line_up_with_the_names() {
    let two_names = [
        (ItemKey::TrackArtist, "Alice & Bob"),
        (ItemKey::TrackArtists, "Alice"),
        (ItemKey::TrackArtists, "Bob"),
    ];
    let aligned = tagged(&[
        two_names[0],
        two_names[1],
        two_names[2],
        (ItemKey::MusicBrainzArtistId, "id-alice"),
        (ItemKey::MusicBrainzArtistId, "id-bob"),
    ]);
    assert_eq!(
        read_credit_mbids(Some(&aligned), ItemKey::MusicBrainzArtistId, 2),
        ["id-alice", "id-bob"]
    );

    let too_few = tagged(&[
        two_names[0],
        two_names[1],
        two_names[2],
        (ItemKey::MusicBrainzArtistId, "id-alice"),
    ]);
    assert!(read_credit_mbids(Some(&too_few), ItemKey::MusicBrainzArtistId, 2).is_empty());

    let too_many = tagged(&[
        two_names[0],
        two_names[1],
        two_names[2],
        (ItemKey::MusicBrainzArtistId, "id-alice"),
        (ItemKey::MusicBrainzArtistId, "id-bob"),
        (ItemKey::MusicBrainzArtistId, "id-carol"),
    ]);
    assert!(read_credit_mbids(Some(&too_many), ItemKey::MusicBrainzArtistId, 2).is_empty());
}

#[test]
fn a_solo_credit_keeps_its_only_id() {
    let tag =
        tagged(&[(ItemKey::TrackArtist, "Alice"), (ItemKey::MusicBrainzArtistId, "id-alice")]);

    assert_eq!(read_credit_mbids(Some(&tag), ItemKey::MusicBrainzArtistId, 1), ["id-alice"]);
}

/// The read side of the same precedence `tag_writer::apply_year` clears the whole list for:
/// `ReleaseDate` is the explicit answer, `RecordingDate` the one Picard writes, `Year` a
/// Vorbis-only spelling some rippers still emit alone.
#[test]
fn a_release_date_is_taken_from_the_first_key_that_carries_one() {
    let all_three = tagged(&[
        (ItemKey::ReleaseDate, "1959"),
        (ItemKey::RecordingDate, "1958"),
        (ItemKey::Year, "1957"),
    ]);
    assert_eq!(release_timestamp(Some(&all_three)).map(|ts| ts.year), Some(1959));

    let no_release = tagged(&[(ItemKey::RecordingDate, "1958"), (ItemKey::Year, "1957")]);
    assert_eq!(release_timestamp(Some(&no_release)).map(|ts| ts.year), Some(1958));

    let year_alone = tagged(&[(ItemKey::Year, "1957")]);
    assert_eq!(release_timestamp(Some(&year_alone)).map(|ts| ts.year), Some(1957));

    assert!(release_timestamp(Some(&tagged(&[]))).is_none());
}

/// One key per field, which is the shape a copy-paste slip gets silently wrong — and every one of
/// these lands on `albums`, where a mis-mapped value describes the whole release.
#[test]
fn each_release_field_reads_from_its_own_key() {
    let tag = tagged(&[
        (ItemKey::Label, "ECM"),
        (ItemKey::CatalogNumber, "ECM 1064"),
        (ItemKey::Barcode, "042281100420"),
        (ItemKey::OriginalMediaType, "CD"),
        (ItemKey::MusicBrainzReleaseType, "Album"),
        (ItemKey::ReleaseCountry, "DE"),
        (ItemKey::MusicBrainzReleaseGroupId, "rg-1"),
        (ItemKey::FlagCompilation, "1"),
    ]);

    let release = read_release_tags(Some(&tag));

    assert_eq!(release.label.as_deref(), Some("ECM"));
    assert_eq!(release.catalog_number.as_deref(), Some("ECM 1064"));
    assert_eq!(release.barcode.as_deref(), Some("042281100420"));
    assert_eq!(release.media.as_deref(), Some("CD"));
    assert_eq!(release.release_type.as_deref(), Some("Album"));
    assert_eq!(release.release_country.as_deref(), Some("DE"));
    assert_eq!(release.musicbrainz_release_group_id.as_deref(), Some("rg-1"));
    assert!(release.is_compilation);
}

/// A file that says nothing is not a compilation, which is why the flag is a `bool` and not an
/// `Option<bool>`.
#[test]
fn a_file_that_says_nothing_carries_no_release_tags() {
    let release = read_release_tags(Some(&tagged(&[])));

    assert_eq!(release.label, None);
    assert!(!release.is_compilation);
}
