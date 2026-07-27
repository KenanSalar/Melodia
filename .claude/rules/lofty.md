---
paths:
  - src/media/**/*.rs
  - src/library/tags.rs
---

# Lofty Best Practices

## Reading Metadata

- Use `read_from_path()` when the file extension is known — fastest path, skips format guessing
- Use `Probe::open().guess_file_type()` only when file type is uncertain
- `ItemKey` implements `Copy` — passed **by value** (not reference): `tag.get_string(ItemKey::Bpm)`
- `tag.year()` removed — use `tag.date().map(|ts| ts.year as i32)` (`Timestamp { year: u16, month: Option<u8>, ... }`)
- `ItemKey::Lyrics` removed from ID3v2 — use `ItemKey::UnsyncLyrics` for unsynchronized lyrics
- `ItemKey::Unknown` removed — use format-specific concrete tag types for non-standard items

## Bulk Scanning Performance

- Use `ParseOptions::new().read_cover_art(false)` to skip embedded artwork — significant speedup for library scans (trade-off: embedded-only artwork won't be extracted)
- Optionally add `.read_properties(false)` if only tag metadata is needed (skips duration, bitrate, etc.)
- Combine with Rayon's `par_iter` for parallel file scanning
- Handle errors per-file gracefully — don't let one corrupt file abort the entire scan

## Tag Priority

- Lofty returns tags in format-specific priority order (e.g., ID3v2 before ID3v1 for MP3)
- Use `tagged_file.primary_tag()` for the most relevant tag, `tag()` for a specific tag type
- Fall back through tag types: `primary_tag().or(first_tag())`

## Writing Metadata

- Always save tags back with `tagged_file.save()` or `tag.save_to_path()`
- Be aware that saving may rewrite the entire file for some formats
- Back up or verify before bulk tag writes

## Error Handling

- Lofty errors are non-fatal for scanning — log and skip unreadable files
- Common issues: unsupported format, corrupt headers, missing tags — all should be handled gracefully

## Supported Tag Access

- `ItemKey::TrackTitle`, `ItemKey::TrackArtist`, `ItemKey::AlbumTitle`, `ItemKey::AlbumArtist` — common keys
- `ItemKey::TrackNumber`, `ItemKey::DiscNumber` — returned as strings; parse to `u32` manually
- `tag.get_string(ItemKey::Comment)` — comment field; may contain multiple values in some formats
- `tagged_file.properties()` — returns `&FileProperties` with `duration()`, `overall_bitrate()`, `sample_rate()`, `channels()`

## Format Notes

- **MP3** (ID3v2): supports most `ItemKey` variants; ID3v2.4 preferred over 2.3 for writing
- **FLAC** (VorbisComments): all tags stored as UTF-8 key=value pairs; multi-value fields supported
- **M4A/AAC** (MP4 atoms): uses `ItemKey::Mp4Atom("©nam")` style keys for non-standard fields
- **OGG Vorbis**: uses same VorbisComments format as FLAC
- Writing ID3v1 is deprecated in Lofty — always write ID3v2

## Artwork Access

- `tag.pictures()` — returns `&[Picture]`; filter by `PictureType::CoverFront` for album art
- `Picture::data()` — raw bytes of the image (JPEG, PNG, etc.)
- `Picture::mime_type()` — `MimeType::Jpeg`, `MimeType::Png`, etc.
- `Picture::new_unchecked()` removed in 0.23 — use `Picture::unchecked()` (builder pattern)
- Skip artwork during scans with `ParseOptions::new().read_cover_art(false)` — significant speedup but skips embedded art
