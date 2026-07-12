use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use tempfile::TempDir;

use super::*;
use crate::database::queries::scan::ExistingTrackSummary;
use crate::error::AppError;
use crate::media::AUDIO_EXTENSIONS;
use crate::media::artwork::CoverCache;

fn create_test_files(dir: &Path, names: &[&str]) -> Result<(), AppError> {
    for name in names {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, b"fake audio")?;
    }
    Ok(())
}

#[test]
fn collects_audio_files() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    create_test_files(
        tmp.path(),
        &["song.mp3", "track.flac", "audio.m4a", "clip.aac", "voice.ogg", "pcm.wav"],
    )?;
    let files = collect_media_files(tmp.path());
    assert_eq!(files.len(), 6);
    Ok(())
}

#[test]
fn ignores_non_audio_files() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    create_test_files(tmp.path(), &["song.mp3", "readme.txt", "cover.jpg", "doc.pdf"])?;
    let files = collect_media_files(tmp.path());
    assert_eq!(files.len(), 1);
    assert!(files[0].to_string_lossy().ends_with("song.mp3"));
    Ok(())
}

#[test]
fn handles_empty_directory() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let files = collect_media_files(tmp.path());
    assert!(files.is_empty());
    Ok(())
}

#[test]
fn follows_nested_directories() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    create_test_files(tmp.path(), &["a/b/deep.flac", "top.mp3", "sub/track.ogg"])?;
    let files = collect_media_files(tmp.path());
    assert_eq!(files.len(), 3);
    Ok(())
}

#[test]
fn collects_all_supported_extensions() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio_files: Vec<String> = AUDIO_EXTENSIONS
        .iter()
        .map(|ext| format!("file.{ext}"))
        .collect();
    let names: Vec<&str> = audio_files.iter().map(std::string::String::as_str).collect();
    create_test_files(tmp.path(), &names)?;
    let files = collect_media_files(tmp.path());
    assert_eq!(files.len(), AUDIO_EXTENSIONS.len());
    Ok(())
}

/// The extension match is case-folded, so a library ripped on a system that
/// wrote `.FLAC` is collected, and a `.JPG` cover is still not.
#[test]
fn extension_match_is_case_insensitive() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    create_test_files(
        tmp.path(),
        &["Track.FLAC", "Song.Mp3", "clip.AAC", "cover.JPG", "notes.TXT"],
    )?;
    let mut names: Vec<String> = collect_media_files(tmp.path())
        .iter()
        .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
        .collect();
    names.sort();
    assert_eq!(names, ["Song.Mp3", "Track.FLAC", "clip.AAC"]);
    Ok(())
}

// ── scan_files_parallel ──

/// Creates a minimal valid WAV file for scanner tests.
fn create_minimal_wav(path: &std::path::Path) -> Result<(), AppError> {
    let sample_rate: u32 = 44_100;
    let data_size: u32 = 4;
    let byte_rate = sample_rate * 2; // mono 16-bit
    let file_size = 36 + data_size;

    let mut buf = Vec::with_capacity(48);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    buf.extend_from_slice(&[0u8; 4]);

    fs::write(path, &buf)?;
    Ok(())
}

fn test_cover_cache() -> CoverCache {
    crate::media::artwork::new_cover_cache()
}

#[test]
fn scan_files_parallel_empty_returns_empty() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let artwork_dir = tmp.path().join("artwork");
    fs::create_dir(&artwork_dir)?;

    let result = scan_files_parallel(&[], &artwork_dir, &test_cover_cache(), &|_, _| {});
    assert!(result.is_empty());
    Ok(())
}

#[test]
fn scan_files_parallel_skips_unreadable_files() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let artwork_dir = tmp.path().join("artwork");
    fs::create_dir(&artwork_dir)?;

    // Create a file with invalid audio content
    let bad_file = tmp.path().join("bad.mp3");
    fs::write(&bad_file, b"not valid audio")?;

    let files = vec![bad_file];
    let result = scan_files_parallel(&files, &artwork_dir, &test_cover_cache(), &|_, _| {});
    assert!(result.is_empty());
    Ok(())
}

#[test]
fn scan_files_parallel_calls_progress_callback() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let artwork_dir = tmp.path().join("artwork");
    fs::create_dir(&artwork_dir)?;

    // Create 10 valid WAV files to trigger the progress callback (fires every 10)
    let mut files = Vec::new();
    for i in 0..10 {
        let path = tmp.path().join(format!("track_{i}.wav"));
        create_minimal_wav(&path)?;
        files.push(path);
    }

    let callback_count = Arc::new(AtomicU32::new(0));
    let counter = callback_count.clone();

    scan_files_parallel(
        &files,
        &artwork_dir,
        &test_cover_cache(),
        &move |_, _| {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
    );

    // With 10 files, callback fires at file 10 (every 10 files)
    assert!(callback_count.load(std::sync::atomic::Ordering::Relaxed) >= 1);
    Ok(())
}

// ── track_is_current (incremental-scan filter) ──

/// Build an `ExistingTrackSummary` map with one entry whose size + mtime
/// match `path` on disk.
fn existing_for(path: &Path) -> Result<HashMap<String, ExistingTrackSummary>, AppError> {
    let meta = std::fs::metadata(path)?;
    let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    let mtime = crate::media::metadata::extract_date_modified(path);
    let mut map = HashMap::new();
    map.insert(
        path.to_string_lossy().into_owned(),
        ExistingTrackSummary { file_size: Some(size), date_modified: mtime },
    );
    Ok(map)
}

#[test]
fn track_is_current_false_when_no_row() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let f = tmp.path().join("a.wav");
    create_minimal_wav(&f)?;
    assert!(!track_is_current(&f, &HashMap::new()));
    Ok(())
}

#[test]
fn track_is_current_true_when_size_and_mtime_match() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let f = tmp.path().join("a.wav");
    create_minimal_wav(&f)?;
    assert!(track_is_current(&f, &existing_for(&f)?));
    Ok(())
}

#[test]
fn track_is_current_false_when_size_differs() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let f = tmp.path().join("a.wav");
    create_minimal_wav(&f)?;
    let mut existing = existing_for(&f)?;
    // Simulate a content edit: stored size no longer matches the file.
    if let Some(row) = existing.get_mut(&f.to_string_lossy().into_owned()) {
        row.file_size = row.file_size.map(|s| s + 1);
    }
    assert!(!track_is_current(&f, &existing));
    Ok(())
}

#[test]
fn track_is_current_false_when_mtime_differs() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let f = tmp.path().join("a.wav");
    create_minimal_wav(&f)?;
    let mut existing = existing_for(&f)?;
    // Stored mtime no longer matches — e.g. tags were rewritten in place.
    if let Some(row) = existing.get_mut(&f.to_string_lossy().into_owned()) {
        row.date_modified = Some("1999-01-01T00:00:00Z".to_owned());
    }
    assert!(!track_is_current(&f, &existing));
    Ok(())
}
