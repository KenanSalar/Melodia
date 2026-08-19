use tempfile::TempDir;

use super::*;
use crate::error::AppError;
use crate::media::artwork::CoverCache;

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
    crate::media::artwork::new_cover_cache()
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
    std::path::PathBuf::from(crate::test_support::ASSETS_DIR)
}

/// Copy a checked-in fixture into `tmp` under `name`, so a rename is free and the
/// artwork lookup can't see `tests/assets/cover.jpg` sitting beside the original.
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
    for (fixture, alias, codec) in [
        ("silence.aiff", "quiet.aif", "Aiff"),
        ("silence.m4a", "quiet.m4b", "Mp4"),
    ] {
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
