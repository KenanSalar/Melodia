# Tag Editing — "Edit Track Information"

## Context

Melodia can read every tag it cares about but can never write one back. Every other
serious player (MusicBee, Strawberry, Clementine, foobar2000, Apple Music) ships an
**Edit Track Information** dialog reachable from the track-row right-click menu, and its
absence means a user who spots a typo in an album name has to leave the app for Kid3 or
Picard, retag there, and wait for a rescan.

This adds that dialog, modelled on Strawberry/Clementine: a cover-art panel on the left
and **Tags / Lyrics / Summary** tabs on the right, opened by right-clicking one *or many*
selected rows. Batch editing is the point of the feature — fixing a whole album's
`Album Artist` in one pass is the chore people actually have — so fields that differ
across the selection show a `‹multiple values›` placeholder and **only the fields the
user actually changes are written**.

The design leans hard on machinery that already exists. Writing tags is genuinely new
(lofty is currently read-only here), but *everything after the write* is the scan
pipeline: re-extract the file, resolve the artist/album/genre ids, call
`update_track_metadata`. That gives us a recomputed `file_hash` / `file_size` /
`date_modified` / `sort_key` / `duration_ms` for free, keeps the DB byte-consistent with
the file, and lets the existing `tracks_fts_update` and `tracks_stats_update` triggers do
the FTS reindex and the artist/album/genre rollups with no new SQL.

> **Status.** Validated twice against the lofty 0.24.0 source (`gen_map!` in
> `src/tag/item.rs`, `tag/mod.rs`, `tag/accessor.rs`, `file/{tagged_file,file_type}.rs`,
> `mp4/ilst/`, `picture.rs`, `config/{parse,write,global}_options.rs`), the Slint 1.16.1
> compiler's own widget sources, and every Melodia path it cites. lofty 0.24.0 is the
> current release; Slint's is 1.17.1, but `textedit-base.slint` is byte-identical to
> 1.16.1's and 1.17 adds nothing this dialog wants, so the pin stays.
>
> The tag-writer section below is the *corrected* design. An earlier draft would have
> silently dropped BPM on MP3, written lyrics under a key no other player reads, lost half
> an edit on an ID3v1-only file, **silently reverted every artwork change on M4A/ALAC**
> (`Ilst` flattens `pic_type` to `Other` — see Artwork below), and **hard-errored the save
> if the user picked a TIFF cover for an M4A**. The per-format key tables are the
> load-bearing part; don't simplify them away.
>
> The one thing the second pass *removed* is a worry, not a feature: WAV and AIFF do **not**
> land in `RIFF_INFO_MAP` / `AIFF_TEXT_MAP`. Their primary tag type is ID3v2, so they get a
> full-fidelity tag like everything else. See "Every field maps" below.

---

## Decisions taken

**Lyrics are stored in the file, not in the DB.** Settled — no `tracks.lyrics` column.

- The Lyrics tab needs exactly two things: read the lyrics tag from the file when the
  dialog opens (single-selection only), and write it back on save. Both happen inside the
  tag writer, which already has the file open. **Zero DB involvement.**
- A column would cost: an additive migration, a new `ExtractedMetadata.lyrics` field, and the
  lockstep column contract that `scan/mutations.rs:16-19` warns about (`TRACK_INSERT_COLUMNS` /
  `bind_track_columns` / the multi-row bind) — plus `update_track_metadata`, which shares
  `bind_track_columns` and so is coupled to the same SET-list order, plus the `Track` entity.
  (That source comment is itself stale and should be fixed in passing: it names a
  `push_row_values` function that **does not exist** — the real multi-row bind is the inline
  `qb.push_values(…)` closure inside `insert_tracks_batch` at `mutations.rs:179-181`.)
- The real cost is memory. `scan_files_parallel` (`src/media/scanner.rs:51-90`) `collect()`s
  **every** `ScannedFile` into one `Vec` before its caller chunks them for ingest
  (`library/settings/folders.rs:343-410`), so lyrics text for the whole library would be resident
  during a scan. Lyrics are the one unbounded tag. This project exists *because* of memory
  regressions.
- Nothing in this feature reads lyrics from the DB. YAGNI.

The column becomes worth adding the moment a lyrics *display* or *search* feature lands —
at which point it gets a migration plus a `retroactive_hash`-style backfill task, which is
the established pattern for exactly this.

Also settled: batch editing (single + multi), self-write suppression set, full
Strawberry-style dialog in one branch, and the `Dialog.closed` fix (below) landing first as
its own commit on this branch.

---

## Backend

### 1. `src/media/tag_writer.rs` (new) — the lofty write

The value type is a per-field tri-state. A nested `Option<Option<T>>` would be unreadable;
an explicit enum carries the intent:

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub enum FieldEdit<T> {
    #[default]
    Keep,        // user never touched it — leave the file's tag alone
    Clear,       // user emptied it   — remove the tag key entirely
    Set(T),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ArtworkEdit { #[default] Keep, Remove, Replace(PathBuf) }

#[derive(Debug, Clone, Default)]
pub struct TagEdit {
    pub title: FieldEdit<String>,
    pub artist: FieldEdit<String>,
    pub album_artist: FieldEdit<String>,
    pub album: FieldEdit<String>,
    pub genre: FieldEdit<String>,
    pub year: FieldEdit<u16>,          // lofty's `Timestamp.year` is u16, not i32
    pub original_year: FieldEdit<u16>,
    pub track_number: FieldEdit<u32>,
    pub disc_number: FieldEdit<u32>,
    pub composer: FieldEdit<String>,
    pub comment: FieldEdit<String>,
    pub bpm: FieldEdit<f64>,
    pub lyrics: FieldEdit<String>,
    pub artwork: ArtworkEdit,
}
```

Not every field survives every container, so the writer reports what it couldn't do rather
than pretending it worked:

```rust
/// Fields the file's tag format has no key for. Never an error — the rest of
/// the edit still lands — but the user is told, so "BPM didn't save" is a
/// message and not a mystery.
pub struct UnsupportedFields(pub Vec<&'static str>);
```

Two functions — the pure core is what gets unit-tested:

```rust
/// Pure. Apply `edit` to an in-memory tag. **No I/O at all** — the `Replace`
/// image is decoded by the caller and handed in already built.
fn apply_edit(
    tag: &mut lofty::tag::Tag,
    edit: &TagEdit,
    picture: Option<&lofty::picture::Picture>,
) -> UnsupportedFields;

/// Blocking. Read-modify-write `path`'s tags in place.
pub fn apply_to_file(
    path: &Path,
    edit: &TagEdit,
    picture: Option<&lofty::picture::Picture>,
) -> Result<UnsupportedFields, AppError>;
```

`apply_to_file` is:

```rust
use lofty::prelude::*;   // TaggedFileExt (primary_tag_*, insert_tag), AudioFile (save_to_path)

let mut tagged = lofty::probe::read_from_path(path)?;   // default ParseOptions
let tag_type = tagged.primary_tag_type();

// Honest pre-flight. `insert_tag` NO-OPS and returns `None` when the FileType
// doesn't support the TagType (`file/tagged_file.rs:398`), so without this the
// unsupported case would only surface as a confusing "no writable tag" below.
if !tagged.tag_support(tag_type).is_writable() {
    return Err(AppError::metadata_msg(format!(
        "{tag_type:?} tags are read-only for {}", path.display()
    )));
}

// Always target the PRIMARY tag type — never `first_tag_mut()`. See below.
if tagged.primary_tag_mut().is_none() {
    tagged.insert_tag(lofty::tag::Tag::new(tag_type));
}
let Some(tag) = tagged.primary_tag_mut() else {
    return Err(AppError::metadata_msg(format!("no writable tag for {}", path.display())));
};

let unsupported = apply_edit(tag, edit, picture);
tagged.save_to_path(path, WriteOptions::default())?;    // AudioFile::save_to_path
Ok(unsupported)
```

Load-bearing details, each verified against the lofty 0.24.0 source:

- **Read with cover art on.** `ParseOptions::default()` has `read_cover_art: true`
  (`config/parse_options.rs:58`), and `read_from_path` uses it (`probe.rs:579-584` →
  `Probe::read` → `options.unwrap_or_default()`). `save_to_path` rewrites the tag from the
  parsed `Tag`, so reading with `read_cover_art(false)` — which is what `extract_metadata`'s
  `skip_artwork` branch does (`metadata.rs:127-134`) — **would silently delete every
  embedded picture on save**. Never reuse that branch here.

  The mechanism is a full replace, not a merge, and it's worth knowing exactly why the
  companion tag doesn't save you: `read_cover_art: false` **skips APIC frames at parse**
  (`id3/v2/frame/read.rs:60-63`, and the equivalents for FLAC/OGG/MP4/APE), and pictures live
  in `Tag.pictures`, not in the companion. Skipped at read ⇒ absent from `tag_frames` ⇒ gone
  from disk.

- **`TaggedFile::save_to` writes *every* tag in the file, not just the one we edited.**
  `file/tagged_file.rs:440-451` loops `for tag in &self.tags`, skips the non-writable ones, and
  re-serializes each from its in-memory `Tag`. So an MP3 with a companion ID3v1, or a WAV with a
  RIFF INFO chunk, keeps that tag — rewritten unchanged, and therefore now **stale** relative to
  the ID3v2 we just edited. That is the honest trade (see Known limits), and it is also why
  `WriteOptions::default()`'s `remove_others: false` must stay: flipping it would strip those
  companion tags outright, which is a bigger behavioural change than a stale ID3v1.

- **`WriteOptions::default()` is the right call — here's why, so nobody "improves" it.**
  `preferred_padding: Some(1024)`, `remove_others: false` (above), `respect_read_only: true`,
  `uppercase_id3v2_chunk: true`, `use_id3v23: false` → we write **ID3v2.4**, which is what
  `.claude/rules/lofty.md` prescribes. `lossy_text_encoding: true` replaces non-representable
  characters with `'?'`, but *only* on Latin-1-restricted formats (ID3v1); every primary tag we
  target is Unicode-capable, so a Turkish or Japanese album name is safe.
  (Known trade, not a bug: some car stereos and old Windows Explorer read only ID3v2.3. If that
  ever becomes a complaint, `use_id3v23(true)` is the one-line answer.)

- **Target the primary tag type, never `first_tag_mut()`.** `primary_tag_type()`
  (`file/tagged_file.rs:65` → `file/file_type.rs:103-116`) is the format's canonical tag:

  ```rust
  FileType::Aac | FileType::Aiff | FileType::Mpeg | FileType::Wav => TagType::Id3v2,
  FileType::Flac | FileType::Opus | FileType::Vorbis | FileType::Speex => TagType::VorbisComments,
  FileType::Mp4 => TagType::Mp4Ilst,
  ```

  Falling back to `first_tag_mut()` means an MP3 carrying **only an ID3v1 tag** gets the edit
  applied *to that ID3v1 tag*, whose whole key set is eight items (`id3/v1/constants.rs:200-210`)
  — `AlbumArtist`, `Composer`, `Bpm`, `UnsyncLyrics` and `OriginalReleaseDate` have no mapping at
  all, so half the user's edit vanishes without a word. Creating a fresh primary tag instead also
  keeps the writer aligned with the reader, which is `primary_tag().or(first_tag())`
  (`metadata.rs:158`), so the next `extract_metadata` reads back exactly what we wrote.
  Do **not** try to be clever and `re_map` the old tag into the new type: `Tag::re_map`
  explicitly **discards the companion tag** (`tag/mod.rs:242-250`) — see the `Keep` bullet below.

- **Every field maps. `UnsupportedFields` will be empty in practice — and that is the point of
  targeting the primary tag.**

  It is tempting to read `RIFF_INFO_MAP` (19 keys, no album-artist / disc / BPM / lyrics) and
  `AIFF_TEXT_MAP` (**four** keys — title, artist, copyright, comment) and conclude that WAV and
  AIFF edits will be mostly-unsupported. **They won't**, and the reason is the bullet above:
  `primary_tag_type()` for both is **`TagType::Id3v2`**, and `Id3v2Tag` is
  `#[tag(supported_formats(Aac, Aiff, Mpeg, Wav, …))]` (`id3/v2/tag.rs:108-111`) — i.e. *writable*
  in both, stored in an `ID3 ` chunk. The sparse maps are only reachable by explicitly targeting
  `TagType::RiffInfo`/`AiffText`, or by the `first_tag_mut()` fallback this design forbids.

  So across the seven containers Melodia scans, the primary tags are exactly three — ID3v2
  (MP3/WAV/AIFF/AAC), VorbisComments (FLAC/OGG), `Ilst` (M4A/ALAC) — and, checked key by key
  against `ID3V2_MAP` / `VORBIS_MAP` / `ILST_MAP`, **all three map every field this dialog
  exposes**. `UnsupportedFields` is a safety net, not a routine outcome. Keep it (it is one bool
  check and it is what makes the BPM strategy below correct), but do **not** build UI or tests
  around it being populated.

- **`Tag::insert` returns `bool`, and `false` means the item was silently dropped.**
  `insert` re-maps the `ItemKey` to the target `TagType` and returns `false` when no mapping
  exists (`tag/mod.rs:367-374`); `insert_text` is a thin alias over it. **Every write goes
  through one helper that checks that bool** and pushes the field name into
  `UnsupportedFields`:

  ```rust
  fn set_text(tag: &mut Tag, key: ItemKey, value: String, field: &'static str, out: &mut Vec<&'static str>) {
      if !tag.insert_text(key, value) { out.push(field); }
  }
  ```

  This is what makes the BPM and lyrics fallbacks below expressible — those are the only two
  fields where a key legitimately fails to map on a container we ship, and the bool is how the
  writer knows to try the sibling key. (If you'd rather know *before* mutating,
  `TagItem::new_checked(tag_type, key, value) -> Option<TagItem>` at `item.rs:1055-1064` is a
  direct "does this key map?" probe.)

  **Corollary: do not use the `Accessor` setters** (`set_title` / `set_artist` / `set_album`
  / `set_genre` / `set_comment` / …). They call `insert_text` and **throw the bool away** — the
  discard is in the `impl_accessor!` macro body at `tag/mod.rs:44-46` (`:131-137` is just the
  invocation), and the same goes for the hand-written `set_track` / `set_disk` / `set_date`.
  Worse, the `Accessor` trait's default method bodies are **empty no-ops**
  (`tag/accessor.rs:106-109`), so a non-overriding impl silently discards the whole write. Go
  through `insert_text(ItemKey::…)` uniformly.

- **BPM: the key differs per format, and `ItemKey::Bpm` does not exist on ID3v2.**

  | Format | `ItemKey::Bpm` | `ItemKey::IntegerBpm` |
  |---|---|---|
  | Vorbis (FLAC/OGG) | `BPM` ✓ (`item.rs:435`) | — |
  | ID3v2 (MP3/WAV/AIFF) | **absent** | `TBPM` ✓ (`:237`) |
  | MP4 (`Ilst`) | freeform `----:com.apple.iTunes:BPM` (`:318`) | `tmpo` ✓ (`:317`) |
  | APE | — | — |
  | RIFF INFO / AIFF TEXT | — | — |

  (`APE_MAP` at `item.rs:90-161` has **neither** key. Melodia doesn't scan APE, but the table
  is the load-bearing part of this doc, so it should be right.)

  So `insert_text(ItemKey::Bpm, "128.5")` on an MP3 is a **no-op returning `false`**, and
  `insert_text(ItemKey::IntegerBpm, "128")` on a FLAC is the same. Write `IntegerBpm` (rounded,
  `u16`-ish) **always**, and additionally `Bpm` (the decimal) where it maps — only report BPM as
  unsupported when *both* come back `false`. This is the one field where the `set_text` bool is
  genuinely load-bearing on a format we ship. `Clear` removes both keys.

- **Lyrics: `ItemKey::Lyrics` still exists — it's ID3v2 that lacks the mapping.**

  | Format | `ItemKey::Lyrics` | `ItemKey::UnsyncLyrics` |
  |---|---|---|
  | Vorbis | `LYRICS` ✓ | `UNSYNCEDLYRICS` ✓ |
  | ID3v2 | **absent** (lofty explains why at `item.rs:246-250`: `Lyrics` is overloaded across SYLT/USLT) | `USLT` ✓ |
  | MP4 | `©lyr` ✓ | `©lyr` ✓ |

  `LYRICS` is the key Picard / foobar2000 / MusicBee actually write in Vorbis comments.
  Writing only `UnsyncLyrics` would put our FLAC lyrics under `UNSYNCEDLYRICS`, where **no
  other player looks** — and theirs would be invisible to us. So: **read**
  `get_string(Lyrics).or_else(|| get_string(UnsyncLyrics))`; **write** keyed by tag type
  (`TagType::VorbisComments => ItemKey::Lyrics`, everything else `ItemKey::UnsyncLyrics`);
  **clear** removes *both* keys.

- **`year` has no setter, and `Timestamp.year` is `u16`.** lofty's `Accessor` exposes
  `date: Timestamp`, not `year` (`tag/accessor.rs:135-141`), and `Timestamp` is
  `{ year: u16, month/day/hour/minute/second: Option<u8> }` (`tag/items/timestamp.rs:24-37`),
  deriving `Copy` + `Default`. Its `Display` writes `{:04}` for the year and appends
  `-MM-DD` **only when those parts are present** (`timestamp.rs:57-82`), so a year-only edit
  renders as `"2024"`.
  Do by hand what `Accessor::set_date` does (`tag/mod.rs:197-200`) so the bool is visible:
  `remove_key(ItemKey::Year)` + `insert_text(ItemKey::RecordingDate, ts.to_string())`.
  Read the current `tag.date()` first so editing only the year **preserves an existing
  month/day** (`Timestamp { year, ..existing.unwrap_or_default() }`).
  `Clear` → `remove_date()` (which removes both `Year` and `RecordingDate`).
  Parse the form's year string with `u16::try_from` / a range check — `as u16` trips
  `clippy::cast_possible_truncation` under the pedantic gate, and `unwrap` is denied.

- **`original_year`** is not on `Accessor` — write `ItemKey::OriginalReleaseDate` as the
  4-digit string (`extract_metadata` reads it with `s.get(..4)`, `metadata.rs:201-202`).
  It maps on **all four** relevant tag types: Vorbis `ORIGINALDATE`/`ORIGINALYEAR`
  (`item.rs:416`), ID3v2 `TDOR` (`:200`), MP4 freeform `----:com.apple.iTunes:ORIGINALDATE`
  (`:302`, TagLib-v2.0 compatible) and APE `ORIGINALYEAR` (`:130`). Nothing special to do.

- `track`/`disk` → `ItemKey::TrackNumber` / `ItemKey::DiscNumber` (+ `remove_key`).

- `album_artist` / `composer` / `comment` → `ItemKey::{AlbumArtist, Composer, Comment}`.

- **Artwork — ⚠ the two subtlest traps in the whole writer, both on M4A/ALAC.**

  Build the `Picture` **once, in the orchestrator, before the Rayon fan-out** —
  `Picture::from_reader` reads the whole file into memory (`picture.rs:632-651`), so doing it
  per-track re-reads the image N times. It sniffs the mime from the first 8 bytes and defaults
  `pic_type` to `Other`, so `set_pic_type(PictureType::CoverFront)` before handing it in.

  **Trap 1: `Ilst` flattens every `pic_type` to `Other`, so keying removal on `CoverFront`
  silently reverts the edit.** MP4 has no picture-type concept — lofty documents this at
  `mp4/ilst/mod.rs:122` ("their `PictureType` will be overwritten with `PictureType::Other`")
  and enforces it at `:367` / `:859`. The naive `remove_picture_type(CoverFront)` +
  `push_picture(new)` therefore plays out on an M4A as:

  1. `remove_picture_type(CoverFront)` matches nothing — the existing cover is `Other`. **No-op.**
  2. The tag now holds `[old(Other), new(CoverFront)]`; both are written into `covr`.
  3. On the next read both come back as `Other`, and our own reader — `CoverFront > CoverBack >
     first available` (`artwork.rs:179-188`) — falls through to *first available* and picks up
     **the old cover**.

  Replace silently reverts; Remove does nothing at all. Clear **both** types:

  ```rust
  fn clear_front_cover(tag: &mut Tag) {
      tag.remove_picture_type(PictureType::CoverFront);
      // MP4 flattens every picture to `Other` on read (`Ilst` coerces `pic_type`), so keying
      // the removal on `CoverFront` alone leaves the existing cover in place — and
      // `find_and_cache_artwork`'s "first available" fallback picks the stale one right back up.
      tag.remove_picture_type(PictureType::Other);
  }
  ```

  There is no clear-all `Tag::remove_pictures()` in lofty 0.24 (only `remove_picture_type` /
  `remove_picture(index)` / `set_picture(index, _)`), so the two-call form *is* the idiom.
  `CoverBack` / booklet / artist pictures survive. In `apply_edit`: `Replace` →
  `clear_front_cover(tag)` + `tag.push_picture(pic.clone())`; `Remove` → `clear_front_cover(tag)`.

  **Trap 2: the picker's accepted formats are constrained from two directions at once, and they
  disagree.** Two facts, both verified against the 0.24.0 source:

  - **lofty rejects anything it can't sniff — there is no `Unknown` fallthrough.**
    `Picture::mimetype_from_bin` (`picture.rs:956-966`) recognises exactly PNG / JPEG / GIF / BMP /
    TIFF and otherwise returns `NotAPicture`. So a **WebP hard-errors at `from_reader`**. (An
    earlier draft of this doc claimed it would come back as `MimeType::Unknown`. It does not —
    `Unknown(String)` exists on the enum but `from_reader` can never produce it.)
  - **MP4's `covr` writer** (`mp4/ilst/write.rs:737-751`) accepts only **Gif / Jpeg / Png / Bmp**
    and returns `FileEncodingError` on anything else — so a **TIFF**, which lofty *does* sniff
    happily, still hard-errors the save, but only on M4A/ALAC. MP3 / FLAC / OGG write the mime as a
    free string and don't care. No single picker filter can express that difference.

  **Normalize, don't filter.** `image` is already a direct dependency — but with
  `default-features = false, features = ["jpeg", "png", "webp"]` (`Cargo.toml:165`), so it **cannot
  decode TIFF / BMP / GIF today**. (An earlier draft of this doc assumed it could and specified a
  broad `jpg jpeg png webp gif bmp tiff` picker; that is not implementable without adding three
  decoder features.) So:

  1. Offer the picker `jpg jpeg png webp` — exactly what `image` can decode with the features we
     already build.
  2. **Decode it with `image`.** This is the validation step — `from_reader` only sniffs 8 bytes
     and never decodes, so a truncated JPEG would otherwise embed happily into N files and only
     blow up later at thumbnail time. A corrupt pick now fails the whole edit up front with a
     clear toast.
  3. If the format is **JPEG or PNG**, embed the original bytes untouched (no lossy
     re-compression). Otherwise — today that means **WebP** — **re-encode to JPEG** and build the
     `Picture` from those bytes.

  WebP is the case that makes the normalizer load-bearing rather than decorative: lofty refuses it
  outright, so it can only be embedded at all by being re-encoded, and M4A is the target that
  proves the round-trip. Consequence worth stating plainly: **the TIFF-hard-errors-M4A trap is now
  unreachable rather than fixed** — a TIFF can no longer be picked. Widening the picker back out
  means adding `gif`, `bmp`, `tiff` to `image`'s features, at which point step 3's "everything else
  re-encodes to JPEG" already covers them.

  Implemented as `tag_writer::cover_picture_from_path` — pure, and beside the writer rather than in
  the orchestrator, since it has no `AppState`.

- **Never touch what the user didn't edit — and by default that holds.**
  `GlobalOptions::preserve_format_specific_items` defaults to **`true`**
  (`config/global_options.rs:50`): converting a concrete `Id3v2Tag` to the generic `Tag`
  stashes a companion tag with the frames that have no `ItemKey`, and merges them back on
  save. So ReplayGain, MusicBrainz ids, `label`, POPM, and everything else the form doesn't
  show survives a read-modify-write untouched, and `Keep` genuinely means keep.
  Two footnotes: `GLOBAL_OPTIONS` is a **`thread_local!`**, so if anyone ever calls
  `apply_global_options` it must be done on *every* Rayon worker, not once in `main`; and
  `Tag::re_map` **drops the companion** on purpose (it logs `"Discarding format-specific
  items due to remap"`), which is why the writer never re-maps.

- **Empty ≠ clear.** An empty string in the form must `remove_key`, not write an empty tag;
  `extract_metadata` already filters whitespace-only tags to `None` (`metadata.rs:177-186`),
  so writing `""` would produce a ghost tag our reader ignores but other players show.

- **Bound the write fan-out — the MP4 save path clones the picture bytes.**
  `tag/utils.rs:43-47` does `Into::<Ilst>::into(tag.clone())` on save: a full clone of the tag,
  **embedded cover included**. Under a default-width Rayon `par_iter` that is `num_cpus ×
  (image + its clone)` resident at once — a 5 MB cover on a 12-core box is ~120 MB transient, in
  a project that exists *because* of memory regressions. Cap the pool (a small
  `ThreadPoolBuilder::new().num_threads(4)`, or `.with_max_len()`); tag writing is I/O-bound, so
  the extra width buys nothing anyway.

- Blocking — every caller goes through `spawn_blocking`.

### 2. `src/media/self_writes.rs` (new) — watcher suppression

Our own `save_to_path` rewrites file bytes → inotify `ModifyKind::Data` → `FileEvent::Modified`
→ (2 s notify debounce + 500 ms batch) → a **full BLAKE3 re-hash + lofty re-parse + artwork
re-extract + redundant `update_track_metadata` + a second `library_changed_tx` bump**, per
edited file. It doesn't loop (a DB write generates no fs event) but a 50-track batch pays
for itself twice.

```rust
pub const SELF_WRITE_TTL: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct SelfWrites { inner: parking_lot::Mutex<HashMap<PathBuf, Instant>> }

impl SelfWrites {
    pub fn mark(&self, path: &Path);
    pub fn unmark(&self, paths: &[PathBuf]);
    /// Consume the entry and return true if we wrote `path` within the TTL.
    /// Sweeps expired entries on the way through, so a write that errored
    /// (and therefore never fired an event) can't leak.
    pub fn take_recent(&self, path: &Path) -> bool;
}
```

- Lives on `AppState` as `self_writes: Arc<SelfWrites>` (struct field + the one construction
  site at `state/mod.rs:192-216`).
- **`mark` happens per file, immediately before that file's write** — not once up front for
  the whole batch. A 500-file batch marked up front would start the TTL clock on file 500
  long before it is written; per-file marking keeps the TTL relative to the event it is
  meant to catch. 30 s comfortably covers 2 s + 500 ms + write time.
- Consulted in `src/tasks/file_event_processor/mod.rs`, **after `deduplicate_events`
  (`mod.rs:48`) and after the `RescanNeeded` short-circuit (`mod.rs:60-67`), before
  `process_batch` (`mod.rs:73`)** — so the expensive `extract_metadata_batch` never runs. The
  ordering against the rescan branch is load-bearing: the "if the batch empties, skip
  `process_batch`" `continue` must not be able to swallow a rescan. That is also the only place
  the filter *can* go: `process_batch` takes `(&state.db, &state.paths, &state.cover_cache)`, not
  `&AppState`, so filtering inside `reconcile` would mean widening its signature. In `mod.rs` the
  `state` is already in scope.
  Filter `FileEvent::Modified(p)` only (a tag write can't create, remove or rename — lofty
  writes in place, `save_to_path` opens the file `read(true).write(true)`, `audio_file.rs:51-56`).
  If the batch empties, skip `process_batch` entirely so `library_changed_tx` isn't bumped.
- **A debouncer overflow bypasses the set, harmlessly.** When notify's queue overflows, the batch
  arrives as `RescanNeeded` and takes the short-circuit above — never reaching the filter. That is
  correct, not a hole: a rescan re-derives everything from disk, and `track_is_current` sees the
  new mtime and re-parses. Worth a line in the module header so nobody later reads it as a leak.
- **Safety valve:** if the DB transaction fails after the files are already written,
  `unmark()` the paths so the watcher *does* re-ingest them. Without that, suppression
  would leave the DB permanently stale until the next boot reconcile.
- **`take_recent` consumes the entry, and that's the right way to fail.** If notify ever
  emits two `Modified` batches for a single write, the second one isn't suppressed and
  degrades into a redundant re-ingest — wasted work, correct result. The alternative
  (non-consuming, TTL-only) would swallow strictly more genuine external edits.
- Accepted trade: an *external* edit to the same file inside the TTL window is swallowed.
  The boot-time `reconcile_watched_folders` catches it. Documented in the module header.

### 3. `src/library/tags.rs` (new) — the orchestrator

```rust
pub struct TagEditReport {
    pub updated: usize,
    pub failures: Vec<(String, String)>,             // (file, error)
    pub unsupported: Vec<(String, Vec<&'static str>)>, // (file, fields the format can't store)
}

pub async fn apply_tag_edit(
    state: &AppState,
    ids: Vec<i64>,
    edit: TagEdit,
) -> Result<TagEditReport, AppError>;
```

1. Read `(id, file_path)` for `ids` off the read pool.
2. If `ArtworkEdit::Replace(p)`: **decode + normalize the picked image once** (see the Artwork
   bullet — `image` decode-validates; JPEG/PNG pass through, everything else re-encodes to JPEG),
   build the `Picture` from those bytes (`Picture::from_reader` + `set_pic_type(CoverFront)`), and
   cache it **once** for the whole batch via
   `artwork::cache_image_file(&p, &state.paths.artwork_dir)` — it is content-addressed
   (`{blake3_16}.{ext}`), so the same image reused across N tracks is one file on disk.
   (`cache_image_file` is `pub(crate)`, `artwork.rs:143`.) A decode failure aborts the whole edit
   *before* any file is touched — that is the point of doing it here.
3. One `spawn_blocking` wrapping a **width-capped** Rayon `par_iter` over the files (the shape of
   `scanner::scan_files_parallel`, `src/media/scanner.rs:51-90` — but see the fan-out bullet: the
   MP4 save clones the cover bytes, so this pool is bounded, unlike the scanner's). Per file:
   `self_writes.mark(path)` → `tag_writer::apply_to_file(path, &edit, picture.as_ref())` →
   `metadata::extract_metadata(path, &paths.artwork_dir, &cover_cache, /* skip_artwork */ false)`.
   **Note the third parameter** — `extract_metadata` takes `&artwork::CoverCache`
   (`metadata.rs:98-103`). A per-file failure (read-only file, unsupported container) is
   **collected, not fatal** — partial success must be reported, never rolled back; likewise
   each file's `UnsupportedFields`.
4. One write transaction for the whole batch. Per successful file: resolve ids, then
   `queries::scan::update_track_metadata(tx, path_str, &fresh_meta, &ids)` — it lives in
   `database/queries/scan/mutations.rs:287` and keys on **`file_path`**, not `track_id`.
   For the id resolution, reuse `reconcile::resolve_track_context`
   (`tasks/file_event_processor/reconcile.rs:242`) — it already does the artist/album/genre
   upserts + library-folder resolution and returns `Option<ResolvedIds>`. It is **private**
   and its signature is `(tx, path, path_str, meta, context)`, so hoisting it to a shared home
   (`queries::scan`, or a small `library::ingest`) means updating reconcile's call site too.
5. Bump `library_changed_tx`, sync the player (below), return the report.

**Artwork needs an explicit write, because `update_track_metadata` can't do it.** Two
reasons: its `artwork_path = COALESCE(?, artwork_path)` (`mutations.rs:305`) can never
*null* a path (so `Remove` is impossible through it), and `find_and_cache_artwork` **prefers
an external `cover.jpg`/`folder.jpg` in the directory over embedded art**
(`artwork.rs:237-258`) — so after replacing an embedded cover, the re-extract would hand back
the *external* cover path and the UI would show no change at all. So when, and only when, the
user touched artwork, follow `update_track_metadata` with an authoritative write in the same tx:

- new `queries::track::set_track_artwork(tx, ids, Option<&str>)` — plain `UPDATE tracks SET
  artwork_path = ?`, no COALESCE. `Replace` passes the cached path; `Remove` passes the
  re-extracted value (which correctly falls back to an external cover if one exists, else
  `NULL`).
- also `queries::album::set_album_artwork(tx, album_ids, Option<&str>)` for the affected
  albums — `update_album_artwork_from_tracks` only fills rows `WHERE (artwork_path IS NULL
  OR artwork_path = '')` (`mutations.rs:334`), so it will never *replace* an album cover and
  the Albums grid card would keep the old art forever.

### 4. Player + queue resync — `src/player/state.rs`

`PlayerState.current_track`, `QueueState.tracks` and `QueueState.direct_play_track` all hold
`Arc<TrackSummary>` snapshots carrying title/artist/album. After a retag they are stale in the
Now-Playing bar, the Queue Sheet and Up Next. `sync_current_track_if_in` (`state.rs:683`) only
mutates `current_track` (`state.rs:699-703`) — it never walks the queue — so it gets a sibling:

```rust
/// Overwrite every queued / currently-playing `TrackSummary` whose id appears in
/// `fresh`. Mirrors `sync_current_track_if_in`, but also walks `queue.tracks`
/// (`player/queue.rs:25`) and `queue.direct_play_track` (`queue.rs:37`) — a tag
/// edit changes exactly the fields the Queue Sheet and Up Next render.
pub fn sync_track_summaries(
    state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    fresh: &HashMap<i64, TrackSummary>,
);
```

Pre-check membership outside the lock (as `sync_current_track_if_in` does) so an edit that
touches nothing in the queue doesn't force a pointless emit; then mutate inside
`with_state_emit` via `Arc::make_mut` — the shape `src/ui/queue_sheet/callbacks.rs:137,143`
already uses to patch queue entries in place. (`tasks/queue_prune.rs` is *not* the precedent:
it removes entries via `queue.prune_missing()` and never touches a summary's fields.)

### 5. Queries

- **`get_tracks_by_ids` is `#[cfg(test)]`-only** (`queries/track.rs:109`) — there is no
  production by-id multi-fetch. Rather than un-gating a 41-column `Track` read, add a
  purpose-built projection: `TagEditRow` (id, file_path, title, artist, album_artist, album,
  genre, year, original_year, track_number, disc_number, composer, comment, bpm, artwork_path
  + the technical columns the Summary tab shows: codec, bitrate, sample_rate, bit_depth,
  channels, file_size, date_modified, duration_ms, file_hash). `TrackListRow` carries neither
  composer, comment nor bpm, so it can't serve the dialog. This follows the "pick the slimmest
  projection" convention in CLAUDE.md.
- `get_track_paths_by_ids(&DbPool, &[i64]) -> Vec<(i64, String)>` for the write pass.
- `get_track_summaries_by_ids` already exists (`queries/track.rs:177`) for the player resync —
  reuse it.
- New ones use `crate::database::placeholders(n)` (`pub(crate)`, `database/mod.rs:22`) +
  `chunked_in_query` (`database/mod.rs:39`) + `sqlx::AssertSqlSafe`, per the existing
  `set_rating` / `batch_update_hashes` shape. Note `set_rating` takes `&DbPool` (writing via
  `db.write()`), whereas `set_track_artwork` / `set_album_artwork` are **tx-scoped** — they must
  land in the same transaction as `update_track_metadata`. Copy its *SQL* shape, not its
  connection shape.

---

## Pre-existing bugs to fix

These are all live today, independent of tag editing, and all three are in its blast radius.

### P1. ✅ DONE — `Dialog.closed`'s Slint teardown never runs (memory leak)

`ui/globals.slint:1219-1259` declares `callback closed();` **with a default body** that resets
`kind`, `target-id`, `input-text{,-2}`, `mosaic-*`, `pending-track-ids`, `playlist-pick-rows`,
`export-pick-rows`, the pick counters and the chrome strings. Slint installs that body as the
callback's handler during `InnerDialog::new()` (inside `AppWindow::new()`). Then
`src/ui/callbacks/playlists/dialog.rs:62` calls `ui.global::<Dialog>().on_closed(...)`, which is
`Callback::set_handler` — whose own doc comment reads *"There can only be one single handler per
callback"* and whose body is an unconditional `self.handler.set(Some(…))` over a
`Cell<Option<Box<…>>>` (`i-slint-core-1.16.1/callbacks.rs:52-57`) — on the same callback, and it
runs *later*, via `install_views`. **The Slint body is dead code.**

This is not inference. The generated `app-window.rs` shows `InnerDialog`'s init calling
`set_callback_handler(… FIELD_OFFSETS.closed() …)` with the `.slint` body, and the generated
`on_closed` is a plain `set_handler` on the same slot. Rust necessarily runs after
`AppWindow::new()`, so it wins. (`grep -rn on_closed src/ ui/` → exactly one hit, so nothing else
is competing for the slot today.)

That is a real leak, not a theoretical one, because `dialog.slint:35-38` deliberately never
unmounts the overlay ("We always render the overlay (don't gate it via `if Dialog.open`)") and
its body branches mount on `kind == "…"`:

- `kind` is never cleared → **the last-used dialog body stays instantiated forever**;
- `playlist-pick-rows` / `export-pick-rows` are never cleared → each row's
  `Image { source: Playlists.request-row-cover(…) }` binding keeps a row-tier
  `SharedPixelBuffer` Arc pinned — precisely the leak the Slint comment says it prevents.

It's masked today only because every opener defensively re-sets its own chrome before opening
(`app-window.slint:616-618`, `track-list/track-list-row.slint:743-746`,
`views/playlists-view.slint:99-102` all clear `input-text` first).

**Fix:** keep the teardown declarative, but move it to a **`public function`**, not a second
callback. Declare `public function closed-teardown()` in `globals.slint` carrying the old body,
and have the **one** Rust `on_closed` handler `invoke_closed_teardown()` first, then release the
`image`-typed properties (which have no Slint default literal) and `heap_trim::trim`. The tag
editor's cover release is then one more line in that single handler instead of a second,
clobbering registration.

**A `public function`, not a `callback`, and the distinction is the whole point.** A callback has
exactly one clobberable handler slot — which is the bug we are fixing. A `public function` has no
slot at all, so a future contributor *cannot* `on_closed_teardown(…)` the teardown away and
silently reintroduce this. Slint 1.16 generates an `invoke_*` for public functions on globals, and
the project already uses the construct (`ui/components/search-bar.slint:74,81`). Same call site,
same one-liner, no residual footgun.

Related trap, worth knowing before anyone refactors this further: `Callback::call`
(`i-slint-core-1.16.1/callbacks.rs:34-42`) `take()`s the handler for the duration of the call and
asserts `"Callback Handler set while called"` — so you **cannot** re-register `closed` from inside
the `closed` handler. Invoking a *different* item (as above) is safe; re-registering is a panic.

Also correct the two comments that assert the false belief (`playlists/dialog.rs:49-53`, and
`globals.slint:1210-1213` — the wrong sentence is "The default handler below tears down all
scalar / list state in pure Slint"; `:1214-1218` is the *correct* note about the Rust image
reset) and the CLAUDE.md "Dialog-close releases global Image properties" bullet.

### P2. ✅ DONE — `upsert_album` never updates `albums.year`

`scan/upserts.rs:39-48` is `ON CONFLICT(name, artist_id) DO UPDATE SET name = excluded.name` —
it binds `year` **only on INSERT**, so `albums.year` never updates on a re-ingest. Editing a
track's year is precisely when that bites. Extend the `DO UPDATE SET` to carry
`year = COALESCE(excluded.year, albums.year)`. (`upsert_artist` and `upsert_genre` have the same
no-op-update shape but nothing else to update, so they're fine as-is.)

### P3. ✅ DONE — BPM is never read from MP3 or M4A

`metadata.rs:197` read only `tag.get_string(ItemKey::Bpm)`, which **has no ID3v2 mapping and no
non-freeform MP4 mapping** (see the BPM table above) — so Melodia had never shown a BPM for an
MP3, and a BPM edit would have landed in `TBPM` correctly and then vanished on the next scan.
Fixed: `get_string(ItemKey::Bpm).or_else(|| get_string(ItemKey::IntegerBpm))`, mirroring what
the writer emits, plus the `is_finite()` filter the ReplayGain parsers use at `metadata.rs:89,95`.
Pinned by `an_mp3_bpm_edit_is_read_back_by_extract_metadata` — a writer↔reader round-trip, since
neither half is meaningful alone.

---

## UI

### Slint

- **`ui/globals.slint` → `export global TagEditor`.** Rust-owned. Per field, two properties:
  `title` … `bpm` (13 `in-out string`s — numbers cross the boundary as strings, Rust parses
  and validates) and `title-placeholder` … (13 `in string`s, set to the `‹multiple values›`
  sentinel when the selection disagrees, else the normal hint). Plus `track-count: int`,
  `active-tab: int`, `cover: image`, `has-cover: bool`, `lyrics-enabled: bool` (single
  selection only), the read-only Summary strings, and callbacks
  `request-edit([int])`, `pick-artwork()`, `remove-artwork()`, `commit()`.

  **Touched-tracking is a Rust-side diff, not a Slint flag.** Every input two-way-binds
  (`text <=> TagEditor.title`, exactly as the create-playlist dialog does with
  `Dialog.input-text` — `<=>` is immune to the "component writing its own in-out property
  orphans a one-way binding" pitfall). Rust snapshots the original value per field at
  populate time, and at commit derives the tri-state by diffing:
  `value == original → Keep`, `value == "" && original != "" → Clear`, else `Set(value)`.
  No touched flags, no controlled inputs, no change to `LabeledInput`.

  Known consequence, and it matches every reference player: in **multi** mode a disagreeing
  field starts empty (with the placeholder), so there is no way to *clear it across the
  whole selection* — an empty box is indistinguishable from an untouched one. Single-track
  mode clears normally.

- **`ui/components/dialog/tag-editor-body.slint` (new).** `HorizontalLayout`: left column =
  `ArtworkImage` (160 px, `components/artwork-image.slint:7`) + `PillButton { text:
  @tr("Replace…") }` + `PillButton { text: @tr("Remove"); danger: true }`; right column = tab
  bar + body.

  **The tab bar is `ChipGroup`** (`ui/components/settings/chip-group.slint:43`) — a segmented
  pill selector with `options: [string]`, `selected-index`, `selected(int)`, a `wrap-at: int`
  and a `manual:` controlled mode (when true the click emits `selected(i)` without
  self-writing `selected-index`, so the host stays the source of truth). It is already exactly
  a tab bar; no new component. It is pill-styled and 32 px tall — that's the look we get.
  (std-widgets' `TabWidget` is not an option for the same reason `TextEdit` isn't — see below.)
  Bodies mount under `if active-tab == N` — never `visible: false` (slint#7377: a hidden child
  still claims layout space; mechanically, `passes/visible.rs` lowers `visible` to a `Clip`
  *after* `lower_layouts` has already run, and the core layout solver never reads the property).

  **In multi-select, don't *disable* the Lyrics and Summary tabs — don't mount them.** Both are
  single-selection-only, and `LabeledInput` has no `enabled` property (the new `MultilineInput`
  would have to grow one just for this). Instead drive `ChipGroup.options` off `track-count`. The
  `@tr()`-only-on-literals rule means the ternary picks between two **inline literal arrays** —
  exactly the pattern `settings.slint` already uses for the Theme Variant list, so no Rust seeding
  and no new component prop:

  ```slint
  options: root.track-count == 1
      ? [@tr("Tags"), @tr("Lyrics"), @tr("Summary")]
      : [@tr("Tags")];
  ```

  - **Tags** — `LabeledInput` rows (`components/labeled-input.slint:14`, which already exposes an
    `in-out property <string> text` at `:17` for the `<=>` bind) in a `ScrollView` whose
    `viewport-width` is locked to `visible-width` (otherwise the stretchy inputs collapse to ~0 —
    the pitfall `smart-playlist-editor-body.slint:225-231` already hit and documents). Year /
    Track / Disc share one `HorizontalLayout`.
  - **Lyrics** — needs a multi-line text box, and **there is none**: every `TextInput` in the
    tree is `single-line: true` (`labeled-input.slint:73`, `search-bar.slint:180`,
    `smart-playlist-editor-body.slint:47`), and `std-widgets` is only imported for
    `ListView`/`ScrollView`. New `ui/components/multiline-input.slint`.

    **std-widgets' `TextEdit` is not a shortcut here** — it forwards **zero** appearance
    properties. Its chrome is hardcoded Fluent (`widgets/fluent/textedit.slint:77-98`: 4 px
    radius, `FluentPalette.control-background`, a blue OS-accent focus underline), `FluentPalette`'s
    brushes are `out` with hardcoded literals driven by the OS accent rather than Melodia's
    `Theme`, and the themeable `TextEditBase` underneath is not re-exported. It would drop a grey
    Fluent box into a Catppuccin dialog with no restyling path.

    So copy Slint's own recipe by hand, from
    `i-slint-compiler-1.16.1/widgets/common/textedit-base.slint:99-142`:

    ```slint
    scroll-view := ScrollView {
        viewport-width: self.visible-width;                             // word-wrap ⇒ lock to visible
        viewport-height: max(self.visible-height, ti.preferred-height); // ← without this there is NO vertical travel
        ti := TextInput {
            single-line: false;
            wrap: word-wrap;
            page-height: scroll-view.visible-height;
            cursor-position-changed(cpos) => { /* nudge viewport-y to keep the caret on screen */ }
        }
    }
    ```

    Three things that are easy to get wrong:
    - **`viewport-height` is not optional.** Without `max(visible-height, preferred-height)` the
      viewport never grows, so there is nothing for the caret-nudge handler to scroll — the text
      just runs off the bottom. This is the line the first draft of this doc missed.
    - **`TextInput` has no `placeholder-text`.** `textedit-base` paints the placeholder as a
      *separate sibling `Text` overlay* (`:146-160`). `MultilineInput` must do the same.
    - Upstream's handler nudges both axes and hard-codes the font height as `20px` behind its own
      `// FIXME` (`:138`). With `word-wrap` the x-branch is inert, so only the y-branch is needed —
      and use `TextInput`'s `out property <FontMetrics> font-metrics` (or a `Theme` token) rather
      than copying the magic number.

    Wrap it in the project's `OverlayScrollbar` (ScrollView's own bar policies `always-off`) and
    theme it like `LabeledInput`. Not mounted at all when `track-count > 1` (above).
  - **Summary** — read-only label/value rows (path, codec, bitrate, sample rate, bit depth,
    channels, size, duration, modified, BLAKE3). Not mounted when `track-count > 1` (above).

- **`ui/components/dialog/dialog.slint`** — one `if DialogGlobal.kind == "edit-tags":
  TagEditorBody {}` branch (alongside the existing `EqualizerBody` / `ReplayGainBody` /
  `SmartPlaylistEditorBody` branches at `:264-270`), one `max-w` ternary arm (~720 px, the
  chain at `:103-108`), one `button-enabled` clause (`:303-315`).
- **`ui/globals.slint` `Dialog.accepted`** — one `else if (root.kind == "edit-tags") {
  TagEditor.commit(); }` in the dispatcher chain (`:1145-1206`), per its documented contract.
- **`ui/components/track-list/track-list-row.slint`** — one `MenuItem { label: @tr("Edit
  Tags…"); icon: "edit"; clicked => { /* set Dialog kind/title/labels inline, as the
  Add-to-Playlist item does at :764-779 */ TagEditor.request-edit(root.effective-ids); } }`.
  Use the **direct-global route** (as `Nav.reveal-in-folder` at `:811` and
  `Playlists.request-add-to-playlist` do) — bubbling a new callback up through
  `TrackListRowItem → TrackList → 9 per-view globals` would be pure tax. `effective-ids`
  (`:659-660`) is already the multi-select-aware `[int]`.
  **Slint sets the chrome; Rust flips `open`** — the same split `SmartEditor` uses (the
  `.slint` call site sets `Dialog.kind`, Rust's `request_edit` populates the globals and sets
  `open` on a fresh tick). Don't set `Dialog.open` in Slint here.
  No font work needed: `edit`, `image` and `info` are already in `scripts/icons.txt`. If any
  *new* glyph creeps in, it must be added there and `scripts/subset-icon-fonts.sh` re-run
  (`check-icons.py` fails the build on drift) or it renders as tofu.
- New global + any new struct must be added to **both** `app-window.slint`'s import list
  **and** its `export {}` block, or Slint prunes them from the Rust API.

### Rust — `src/ui/callbacks/tags.rs` (new)

It needs `Rc<NotificationsUi>` for the completion toast, so it wires from `main.rs` alongside
`wire_playlist_files` (`main.rs:269-272` — same constraint, same place: the notifications stack
only exists after `install_views`).

- `on_request_edit(ids)` — fetch `TagEditRow`s on the runtime, then populate the globals and
  `set_open(true)` **from a fresh event-loop tick** (`upgrade_in_event_loop`). A synchronous
  `Dialog.open = true` from inside a click callback trips Slint's property-recursion guard;
  this is exactly the `SmartEditor::request_edit` shape (`playlists/smart.rs:118-142`). For a
  single selection it also `spawn_blocking`s a lofty read for the lyrics (`Lyrics` then
  `UnsyncLyrics`, per the table above).
- `on_pick_artwork` — rfd on the UI thread via `slint::spawn_local(Compat::new(..))`,
  `.set_parent(&ui.window().window_handle())` first (CLAUDE.md: else it z-orders behind the
  window on Win/macOS). Mirror `library_settings.rs:48` (`rfd::AsyncFileDialog`) — there is no
  existing *image* picker to copy, since playlist artwork is chosen from a mosaic of existing
  covers rather than a file. Filter **broadly** — `jpg`, `jpeg`, `png`, `webp`, `gif`, `bmp`,
  `tiff` — because the orchestrator normalizes (see Artwork): the picker is *not* the place
  encoding constraints get enforced, since lofty's accepted set and MP4's accepted set differ and
  no single filter can express that. Stash the chosen `PathBuf` and preview it into
  `TagEditor.cover`.
- `on_commit` — build the `TagEdit` by diffing against the snapshot, then
  `spawn_logged_toast!` the `library::tags::apply_tag_edit` call. This is textbook
  `spawn_logged_toast!` territory: user-initiated, and silent failure is confusing.
  **Short-circuit the no-op.** If every `FieldEdit` came back `Keep` and `ArtworkEdit::Keep`,
  return without touching disk. lofty rewrites the tag regardless of whether anything changed, so
  a reflexive open-then-Save on a 200-track album would otherwise rewrite 200 files — and, via the
  watcher, risk re-ingesting them — for nothing.
- **Post-commit refresh is a full re-fetch, not an optimistic row patch.** The
  `wire_row_flag!` / `patch_track_row_by_id` pattern used by favourites and ratings works
  because those never change list *membership*. A tag edit does: retitle a track and the
  Album Detail view it is in may no longer contain it, a genre change moves it out of a
  Genre Detail list, a Search result stops matching. The `library_changed_tx` bump the
  orchestrator already sends drives the existing visibility-gated refreshers — let it.
- Toast strings go through `pure callback`s on `Settings` wrapping `@tr(...)` literals (the
  only way a translated string reaches Rust), reporting `updated` / `failures` /
  `unsupported` from the report — a partial batch failure, or a field the container can't
  store, must be visible rather than swallowed.
- The tag dialog pins a cover image, so its release goes in the **single** `Dialog.on_closed`
  handler established by P1 — never a second `on_closed` registration.

---

## Order of work

0. ✅ **DONE — P1** — the `Dialog.closed` teardown fix (`public function closed-teardown()` + the
   one Rust handler). Confirmed against the generated `app-window.rs`: `InnerDialog`'s init installs
   the `.slint` body via `set_callback_handler`, and Rust's `on_closed` (a plain `set_handler` on
   the same slot, running later) replaced it — the body really was dead code.
1. ✅ **DONE — `tag_writer.rs`** + 23 unit tests, all green. Fixtures live in **`tests/assets/`**
   (NOT `tests/fixtures/`, which `headless.rs` scans and asserts `scanned == 1` on). The M4A
   artwork test was confirmed to **fail** against a plain `remove_picture_type(CoverFront)` — two
   pictures come back, the old `Other` one beside the new `CoverFront` — and to pass once
   `clear_front_cover` clears both types. `ArtworkEdit::Replace` is a **unit variant**: the
   `Picture` is built once by `cover_picture_from_path` and passed alongside, so the orchestrator
   owns the picked `PathBuf` (it needs it for `cache_image_file` anyway) — and because the
   `Picture` therefore travels *beside* the edit rather than inside it, `apply_edit` clears the
   old cover **only** with a replacement in hand (plus a `debug_assert!`), so a caller that
   forgets to thread it through can't silently turn a Replace into a Remove across a whole batch.
   All seven containers are covered: MP3 / FLAC / M4A / WAV, plus **OGG and AIFF** — a tag type
   says nothing about the container writer wrapped around it (OGG rewrites pages, AIFF writes an
   ID3 chunk), and those are separate code paths in lofty.
2. ✅ **DONE — `self_writes.rs`** (`SelfWrites` on `AppState`, TTL 30 s) + the
   `file_event_processor` filter, with 11 unit tests (7 in `self_writes_tests.rs` + 4
   `suppress_self_writes` tests in `file_event_processor_tests.rs`). The consumer half is complete; the
   **producer** (`mark` / `unmark`) arrives with the step-4 orchestrator, which is the only thing
   that writes files. Two notes for whoever picks that up: the time-taking `mark` / `take_recent`
   are thin wrappers over private `mark_at(path, at)` / `take_recent_at(path, now)` so the TTL
   sweep is testable without a 30 s sleep (and `Instant` **subtraction** is a clippy error under
   the pedantic gate, whose suggested `checked_sub().unwrap()` is denied — the tests age an entry
   by moving the *lookup* forward instead). The filter itself is
   `suppress_self_writes(&mut batch, &SelfWrites)` beside the loop rather than an inline closure,
   so it is testable without an `AppState`.
3. ✅ **DONE — Queries + P2.** `TagEditRow` projection + `track_tag_edit_columns()`
   (`entities/track.rs`, no joins — artist/album/genre are denormalized on `tracks`; `bpm` is the
   `REAL`/`f64` gotcha); `get_tag_edit_rows_by_ids` + `get_track_paths_by_ids` + the tx-scoped,
   COALESCE-free `set_track_artwork` (`queries/track.rs`); `set_album_artwork` (`queries/album.rs`,
   overwrites an existing cover, unlike `update_album_artwork_from_tracks`'s NULL-only roll-up).
   All mirror the `get_track_summaries_by_ids` / `set_rating` shapes; the two artwork setters differ
   only by executing against `&mut **tx`. **P2** — `upsert_album`'s `DO UPDATE` now carries
   `year = COALESCE(excluded.year, albums.year)` (references already-bound columns, so no new
   bind). Tests: `track_tests.rs` (projection + input-order + artwork-null-proves-no-COALESCE),
   `album_tests.rs` (`set_album_artwork_replaces_existing_cover`), `scan_tests.rs`
   (`upsert_album_updates_year_on_conflict` — new year updates, `None` preserves). **P3** (the BPM
   reader fallback) was already ✅ **DONE** — it shipped with the writer, since a `TBPM` the reader
   can't see is a BPM edit that vanishes.
4. ✅ **DONE — `library/tags.rs` orchestrator + `sync_track_summaries`.** Split into a testable
   `write_tag_edit` core (no `AppState`: `db` / `artwork_dir` / `cover_cache` / `self_writes` in,
   `(TagEditReport, updated_ids)` out — since no library-layer test builds an `AppState`) and the
   thin `apply_tag_edit(&AppState, ids, edit, artwork_source)` wrapper that adds the player resync +
   the `library_changed_tx` bump. `resolve_track_context` was **hoisted** out of `reconcile.rs` into
   `queries::scan` (`pub(crate)`, both reconcile call sites updated) so `library/` doesn't import
   from `tasks/`. Artwork: `Replace` decodes/normalizes once up front (fails the whole edit on a bad
   pick) then overwrites track + album art in-tx; `Remove` writes the re-extracted per-track value
   and leaves album art alone. The fan-out is a 4-thread capped Rayon pool inside `spawn_blocking`
   (the MP4 save clones the cover); `self_writes.mark` keys on the DB `file_path`, with an
   `unmark`-on-tx-failure safety valve. `sync_track_summaries` walks `current_track` + `queue.tracks`
   + `direct_play_track` and **bumps `queue.version`** on a queue patch so `with_state_emit`
   republishes the queue VM. Tests: 3 `tags_tests.rs` cases (single-edit preserves
   play_count/rating/favorite + hash changes; batch reports one failure + commits the rest; album
   rename repoints `album_id`) targeting the core against temp fixtures, plus a `sync_track_summaries`
   unit test in `state_tests.rs`. `ArtworkEdit::Replace` is a **unit variant**, so the picked path
   rides as a separate `apply_tag_edit` parameter — the UI (step 6) supplies it.

   ⚠ **The orchestrator owns the whole row, not just the tag columns.** Step 2's suppression
   removes the watcher event that would have run `update_track_metadata` — and that statement
   rewrites `file_hash`, `file_size` and `date_modified` alongside the tags. None of those three
   are tag columns, and **all three change on every tag write**, because rewriting a file's tags
   rewrites its bytes.

   The `extract_metadata` call in step 3 above is what covers them: it `stat`s the file, BLAKE3-
   hashes it (`metadata.rs:114-125` — `ExtractedMetadata.file_hash` is a non-optional `String`)
   and re-parses the tags, and step 4 hands that whole struct to `update_track_metadata`. So
   suppression isn't skipping that work — it's skipping the watcher's **second, redundant copy**
   of it (and a second `library_changed_tx` bump) a few seconds later, which is exactly what §2
   above says.

   What must never happen is "optimizing" the orchestrator into a hand-built UPDATE from the tag
   values it already knows — dropping `extract_metadata` to save a re-parse. It would still have
   to `stat` for `date_modified`, and a fresh `date_modified` beside a stale `file_hash` is the
   one state the boot reconcile cannot repair: `scanner::track_is_current` compares the stored
   size and mtime and would read the row as current forever, leaving a permanently wrong hash
   under moved-file detection and M3U8 playlist re-matching, which `retroactive_hash` won't fix
   either (it backfills *missing* hashes, not stale ones). It would also break the artwork
   `Remove` path, which writes back the *re-extracted* value.

   And `SelfWrites::mark` must be fed the **DB `file_path`** (what `get_track_paths_by_ids`
   returns) — the set keys on exact `PathBuf` equality, so a picker-supplied path silently
   suppresses nothing.
5. ✅ **DONE** — Slint: `TagEditor` global, `tag-editor-body.slint`, `multiline-input.slint`, the
   `dialog.slint` branch (mount + `max-w` + `button-enabled`), the `Dialog.accepted` dispatch arm,
   and the track-row context-menu item.
6. ✅ **DONE — `ui/callbacks/tags.rs` + wiring.** `wire_tags(ui, state, notifications)` implements the
   four `TagEditor` callbacks; all three async handlers use `slint::spawn_local(Compat::new(…))` (not
   `runtime.spawn`) so the per-open `Rc<RefCell<TagSession>>` snapshot and the `Rc<NotificationsUi>`
   completion toast never cross a thread — the `wire_playlist_files` pattern. `request-edit` fetches
   `TagEditRow`s, reads lyrics via a new `tag_writer::read_lyrics` + decodes the cover preview off-thread
   (single selection), then populates + opens on a fresh tick; a disagreeing field across a batch shows
   the `‹multiple values›` sentinel and starts empty. `commit` diffs each field against the populate-time
   snapshot into `Keep`/`Clear`/`Set` (numbers parsed here — `u16`/`u32`, and BPM guarded against
   NaN/inf/negative), short-circuits a no-op, applies via `library::tags::apply_tag_edit`, and toasts the
   report (`updated`/`failures`/`unsupported`). The cover release is one line added to the **single**
   `Dialog.on_closed` handler (`playlists/dialog.rs`), never a second registration. Wired from `main.rs`
   after `wire_playlist_files`. New i18n strings (`tag-multiple-values`, the completion-toast callbacks)
   in `settings.slint` + all six `.po`. Tests: pure diff/format logic (`callbacks/tests/tags_tests.rs`)
   + a `read_lyrics` FLAC/MP3 writer↔reader round-trip (`tag_writer_tests.rs`). `clippy --all-targets`
   and `cargo test` green.
7. ✅ **DONE** — housekeeping: `lofty` pinned to `"0.24.0"`; the stale `push_row_values` name in the
   `scan/mutations.rs` comment corrected to the real inline `qb.push_values(…)` closure. Also
   cleared out every `#[allow(dead_code)]` in hand-written code while in there (the eight
   `assert_send_sync` fns became anonymous `const _: fn()` assertions; `UiHandles`'
   `browse_ui`/`favorites_ui`/`search_ui` turned out to be keepalives guarding nothing — there is
   no `Arc::downgrade` or `Weak<…Ui>` anywhere, every `wire_*` closure clones its own strong `Arc`
   — and were deleted).
8. Docs: CLAUDE.md conventions entry (and the corrected dialog-close bullet from P1).

---

## Verification

- `cargo clippy --all-targets -- -D warnings` — the gate. Note `unwrap_used = deny` and
  `expect_used = warn` apply **to test code too**, so tests use `assert!`/`matches!`/
  `let … else`, never `unwrap`.
- `cargo test`. New tests:
  - `src/media/tests/tag_writer_tests.rs` — `apply_edit` as a pure fn: each field's
    Keep/Clear/Set; a year edit preserves an existing month/day; `FieldEdit::Keep` leaves
    ReplayGain/MusicBrainz keys untouched (the companion-tag guarantee).
    Then real round-trips against **copies in a `tempfile::TempDir`**: write, re-extract with
    `extract_metadata`, assert the values landed **and that the embedded picture survived**
    (the `read_cover_art` trap). Cover the containers where they disagree:
    - FLAC (VorbisComments) — assert lyrics land under **`LYRICS`**, not `UNSYNCEDLYRICS`.
    - MP3 (ID3v2) — assert BPM lands in **`TBPM`** (i.e. reads back through `IntegerBpm`),
      and that an **ID3v1-only** MP3 gets a fresh ID3v2 tag rather than losing album-artist,
      composer, BPM and lyrics.
    - **M4A (`Ilst`) — artwork Replace actually replaces.** Set a cover, re-extract through
      `find_and_cache_artwork`, assert the returned artwork hash is the **new** image. This is
      the *only* test that catches the `pic_type`-flattening trap; write it first, and confirm
      it fails against a plain `remove_picture_type(CoverFront)`.
    - **M4A cover format** — a **WebP** source normalizes to JPEG and the save **succeeds**.
      Guards the `mp4/ilst/write.rs` `FileEncodingError` path. (WebP, not TIFF: lofty's
      `from_reader` refuses a WebP outright, so it is the reachable case — see Artwork Trap 2.
      Assert lofty really does reject the raw WebP first, or the test proves nothing.)
    - **WAV** — assert it round-trips a *full* edit through a **fresh ID3v2 tag**, and that
      `UnsupportedFields` comes back **empty**. (Do **not** write the obvious "assert the
      unwritable fields are reported" test — it cannot pass. WAV's primary tag is ID3v2, not
      RIFF INFO; see "Every field maps" above. An earlier draft of this doc planned exactly that
      test.)
    - **OGG and AIFF** — a full-edit round-trip each. FLAC and WAV prove the *key mappings* for
      VorbisComments and ID3v2, but a tag type says nothing about the container writer wrapped
      around it: OGG rewrites pages and AIFF writes an ID3 chunk, and those are separate code
      paths in lofty — the risky half of a save. That is all seven containers Melodia scans.
    - **BPM bounds** — `NaN` / negative / absurd inputs. `f64::clamp` does **not** absorb NaN
      (both of its comparisons are false for NaN), so an unguarded `{:.0}` writes the literal
      string `"NaN"` into `TBPM` — and `str::parse::<f64>()` accepts `"nan"` and `"inf"`, so the
      dialog can hand one over. Bound once, and assert the integer and decimal keys agree.
    - **BPM writer↔reader round-trip** — an MP3 BPM edit must come back out of
      `extract_metadata`. Neither half means anything alone: the writer puts BPM in `TBPM`
      because that is the only key ID3v2 maps, so the reader's `IntegerBpm` fallback (P3) is
      what makes the edit visible in the app. See P3.
  - **Fixture placement matters.** `tests/headless.rs:34-47` adds **`tests/fixtures/` as a
    library folder**, scans it, and asserts `scanned == 1` / `tracks.len() == 1`. The scan is
    recursive, so *any* new audio file under `tests/fixtures/` — subdirectory included —
    breaks that test. Put the tag-write fixtures in a new, unscanned **`tests/assets/`** and
    copy them into the `TempDir` per test. (The existing `metadata_tests.rs:9` helper
    hand-writes a minimal RIFF WAV — too bare to serve as a tag-write fixture; the M4A and the
    ID3v1-only MP3 both need real files.)
  - `src/media/tests/self_writes_tests.rs` — mark → `take_recent` is true once and false the
    second time; an expired entry returns false and is swept; `unmark` drops it.
  - `src/library/tests/tags_tests.rs` — `DbPool::test_pool()` + `setup_seeded_db` + real temp
    files: a single-track edit updates the row and preserves `play_count` / `rating` /
    `is_favorite` (which `update_track_metadata` doesn't touch); a batch edit with one
    unwritable file reports the failure and still commits the rest; an album rename moves the
    track to a new `album_id` and the stats triggers roll the counts.
- Manual, in the running app (`cargo run`): right-click a track → Edit Tags → change the
  album → Save. Confirm (a) the row updates, (b) the Albums grid shows the new album, (c)
  the old album disappears if it is now empty (the `*_stats` views filter `track_count > 0`),
  (d) search finds it under the new name (the FTS trigger), (e) `RUST_LOG=info` shows **no**
  `Updated metadata for:` line from the watcher ~3 s later — that line means the suppression
  set failed. Repeat with a multi-selection, and with the *currently playing* track (the
  Now-Playing bar and Queue Sheet must both update).
- Cross-check the write against a second tagger (Kid3 / Picard / `metaflac --list`) at least
  once per container — the per-format key tables above are the whole point, and our own reader
  agreeing with our own writer proves nothing about interop.
- Release + `/usr/bin/time -v target/release/Melodia` once at the end for the RSS check.

## Known limits (worth stating in the docs)

- Writing tags changes `file_hash`, so `#MELODIA-HASH` lines in previously exported M3U8
  playlists go stale. Import re-matches by `file_path` first and only falls back to hash, so
  this degrades gracefully — no action, but worth a line in CLAUDE.md.
- If a directory contains an external `cover.jpg`, it shadows embedded art for *scanning*.
  The explicit `set_track_artwork` write makes a user-driven Replace stick anyway, but a
  later rescan of that folder would revert `artwork_path` to the external cover. Fully
  fixing that means teaching `find_and_cache_artwork` about user overrides — out of scope,
  but it should be called out rather than discovered.
- **Clearing the title gives you the filename, not an empty title.** `extract_metadata` falls
  back to the file stem when the title tag is empty (`metadata.rs:177`), so a cleared title
  round-trips into the DB as the file's name. That is what every player does; it just isn't
  obvious.
- **A batch artwork Replace embeds the picked image into every selected file.** A 5 MB PNG
  across a 200-track album is a ~1 GB rewrite. Reference players behave the same way, but the
  cost is worth knowing before clicking Save on a whole discography.
- **Only the primary tag is edited — and every *other* tag is rewritten unchanged.**
  `TaggedFile::save_to` (`file/tagged_file.rs:440-451`) loops over all tags and re-serializes
  each, so an MP3 carrying a legacy ID3v1 keeps its (now stale) ID3v1, a WAV keeps its stale
  RIFF INFO chunk, and an AIFF keeps its stale AIFF TEXT chunks. We write the primary tag —
  which is what our reader looks at first, and what every other modern player looks at first —
  and leave the rest alone. Stripping them (`WriteOptions::remove_others(true)`) would be a
  bigger behavioural change than the staleness it fixes.
- **Multi-value fields collapse on edit.** A FLAC with three `ARTIST` entries shows only the
  first in the dialog (`get_string` returns the first), and *setting* the field replaces all
  of them with the single new value (`Tag::insert` → `insert_unchecked` `retain`s out every item
  with that key first, `tag/mod.rs:382-385`). Leaving the field untouched (`Keep`) preserves all
  three.
- **`Other`-typed embedded pictures are collateral on an artwork edit.** `clear_front_cover`
  removes both `CoverFront` *and* `Other`, because MP4 flattens everything to `Other` (see
  Artwork). A FLAC that deliberately stores a non-cover image as `Other` loses it on a Replace or
  Remove; `CoverBack`, booklet and artist pictures survive. Accepted — Melodia's data model has
  exactly one cover per track, so it has nowhere to put the others anyway.
- **We write ID3v2.4**, not 2.3 (`WriteOptions`' `use_id3v23: false`). Some car stereos and older
  Windows Explorer builds read only 2.3. If that ever becomes a real complaint, it's a one-line
  change — but 2.4 is the correct default and what `.claude/rules/lofty.md` prescribes.
- **Star ratings still live only in the DB.** `tracks.rating` is not written to the file. lofty
  has the path for it — `ItemKey::Popularimeter` + `tag/items/popularimeter.rs`, mapping to
  `POPM` (ID3v2), `rate` (MP4), `RATING` (Vorbis) — so the writer built here is where that would
  land. Out of scope, but it is the natural next ask after tag editing, and worth not designing
  it out.
