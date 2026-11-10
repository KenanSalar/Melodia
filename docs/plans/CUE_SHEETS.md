# CUE Sheets

Working doc. Delete when the feature ships.

Status: **in progress**, Phase 1 at its gate · Created: 2026-11-04

Issue [#78](https://github.com/KenanSalar/Melodia/issues/78).

> Verified **2026-11-10** against this tree at `18ada620` (v0.23.0), the pinned `rusqlite` 0.41.1
> and `symphonia-*` 0.6.1 sources in the registry, the SQLite docs on `ALTER TABLE` and
> `PRAGMA foreign_keys`, RFC 9639 §8.7 (FLAC CUESHEET), and the CDRWIN cue syntax appendix. Anything
> marked ⚠️ **re-verify** drifts; check it again on the day.

---

## The symptom

A whole-disc rip is one long FLAC or WAV, with a `.cue` sidecar that names each track and says
where it starts. Melodia shows that disc as **a single track 50 minutes long, named after the
file**. The sheet next to it is invisible: the walk skips it because `.cue` is not an audio
extension.

So the album can't be used. There are no titles to read, nothing to queue on its own, and nothing
to skip to; skipping forward leaves the disc entirely. Seeking is the only way to find a song.

Opening the `.cue` directly doesn't help either. Command-line paths, drag and drop and open-with
all go through `import_files`, which keeps audio extensions only, so the sheet is rejected there
too.

## What ships

- Every audio track the sheet describes scans as an ordinary track, with its own row, title,
  artist and number. It queues, rates, plays, scrobbles and joins playlists like any other track.
- Metadata comes from the sheet where the sheet has it, and from the image's own tags otherwise.
- While a sheet claims an image, the image has no row of its own. Deleting the sheet gives it one
  back. When an image is split, the playlists that held the whole image get its tracks in its
  place. When a sheet goes away, they get the whole image back.
- Each track starts and ends on the exact sample the sheet names. Playing on from one track to the
  next in the same image is sample-continuous: there is no crossfade at those joints even with
  crossfade on, and the open decoder carries on rather than reopening the file.
- Audio hidden before track 1 (HTOA) gets a row of its own, numbered 0, so it can be played.
- Tag edits on these tracks are written into the `.cue` sheet itself, in the sheet's own encoding,
  and they survive a rescan. Ratings and favourites stay in the database. Neither touches the
  image.
- A cue sheet embedded in the file splits it too: a `CUESHEET` tag, or FLAC's native CUESHEET
  block.
- A `.cue` opens from the file manager, the command line or a drop, and queues its tracks.
- It is always on, with no setting. A library without sheets sees no change. A sheet that describes
  files already split one per track claims nothing, so those files keep their own tags.

---

## Decisions

### Identity: a rebuilt `tracks` keyed on `(file_path, cue_track)`

`tracks.file_path` is `NOT NULL UNIQUE` inline (`initial_schema.sql:72`). SQLite backs an inline
constraint with an automatic index that nothing can drop, so several rows can't share a path
without rebuilding the table.

The other option keeps the constraint and gives each sheet track a virtual path. Everything that
opens `file_path` as a real file would then have to translate that path back: the decoder, the tag
writer, lyrics, reveal, the hash task and the watcher. That moves the "one file, one track"
assumption into a mapping layer rather than retiring it.

The rebuild carries the key the data actually has:

| column | meaning |
|---|---|
| `cue_track INTEGER` | `NULL`: the row plays the whole file. `0`: the hidden track before track 1. `1` to `99`: the sheet's own `TRACK` number |
| `cue_start INTEGER` | where the track starts, in cue frames (1/75 s), from the start of the file's audio. `NULL` when `cue_track` is |
| `cue_end INTEGER` | where it ends, in cue frames; `NULL` plays on to the end of the file |
| `cue_sheet_id INTEGER REFERENCES cue_sheets(id) ON DELETE CASCADE` | the external sheet that made the row; `NULL` for whole files and embedded sheets |

The uniqueness is split in two:

- `UNIQUE (file_path, cue_track)` for sheet tracks.
- `CREATE UNIQUE INDEX … ON tracks (file_path) WHERE cue_track IS NULL`, so a whole file still has
  exactly one row. A table-level `UNIQUE` alone would let two `NULL`s through, since SQLite treats
  `NULL`s as distinct.

`idx_tracks_cue_sheet_id`, partial on `cue_sheet_id IS NOT NULL`, lets a sheet's delete find its
tracks without scanning `tracks`, at no cost to a library without sheets.

The composite's leading `file_path` keeps the folder range reads (`subtree_bounds`, `list.rs:33`)
and every `=` and `IN` on an index.

Two `CHECK`s hold the shape:

- `(cue_track IS NULL) = (cue_start IS NULL)`
- `cue_track BETWEEN 0 AND 99`

`cue_sheets(id, path UNIQUE, folder_id → folders ON DELETE CASCADE, file_size, date_modified)` holds
each external sheet's stamp. A rescan compares that stamp the same way a track's own stamp is
compared.

**Keyed on the TRACK number, not on the start time.** Correcting a timestamp or a typo keeps every
row's id, and with it the play counts, ratings and playlist entries. The cost is that renumbering a
sheet moves identity by number. That is rare, and it is the right trade: a number is what the sheet
says the track *is*.

**`file_size` stays the whole file's size on every row.** It is the stamp a scan compares
(`FileStamp::matches`, `entities/scan.rs:25`, through `WalkIndex::changed`, `scanner.rs:271`), and
nothing in the tree sums it. A smart-playlist `FileSize` rule matches the image a track plays from,
which is what the column says.

### The rebuild cannot eat playlists

With foreign keys on, `DROP TABLE tracks` first runs an implicit `DELETE FROM tracks`, and that
fires every `ON DELETE CASCADE`. For `playlist_items` (`initial_schema.sql:198`) and
`duplicate_dismissals` it would delete every row. The credit tables don't cascade, so on a library
with credits the drop fails outright (checked on a copy of the dev DB), and on one without them the
cascades go through. `PRAGMA foreign_keys` is a no-op inside a
transaction, and `database::migrate` runs every migration inside one (`run_and_record`,
`migrate.rs:132`). `PRAGMA defer_foreign_keys` is no way out, since the cascades still fire. So
enforcement goes off on the connection, before the transaction opens:

- **The migration's first line is `-- foreign-keys-off`.** `build.rs` reads it into a `ForeignKeys`
  field on the embedded `Migration`, and `apply_one` turns enforcement off before the transaction
  and back on after it, on commit or rollback alike, since the writer outlives the migration. Only
  this migration opts in: replaying the shipped ones with cascades off would change what they do.
  `build.rs`'s refusal of `-- no-transaction` stays.
- **The script holds no `PRAGMA`, `BEGIN` or `COMMIT`.** A `BEGIN` fails under the migrator's
  transaction, and a `PRAGMA foreign_keys` there does nothing and says nothing. It runs SQLite's
  documented rebuild:
  1. `CREATE TABLE cue_sheets`
  2. Drop every trigger whose SQL names `tracks`.
  3. `CREATE TABLE tracks_new`, then `INSERT … SELECT` with ids preserved.
  4. `DROP TABLE tracks;` then `ALTER TABLE tracks_new RENAME TO tracks;`
  5. Recreate every index and trigger, verbatim apart from the new key.
  6. `ANALYZE tracks`, since the drop took its `sqlite_stat1` rows.

  Dropping the other tables' triggers is not optional. Since SQLite 3.26 the rename re-parses the
  whole schema, and a trigger body still naming the dropped `tracks` fails it. No view names
  `tracks` at this tree, the views reading the stored counts; Phase 1's dump checks that again.
- **`tracks_fts` stays as it is.** It is external content over `content_rowid = 'id'`, and the copy
  keeps every id, so its index stays valid. Only its three triggers come back.
- **`PRAGMA foreign_key_check` runs inside the transaction, ahead of the history row**
  (`run_and_record`), read through a fetch: `execute_batch` steps each statement once and discards
  its rows, so a check in the script would pass whatever it found. Any row is a new `MigrationError`
  variant and rolls the migration back, and `init_database` already names the backup
  (`database/mod.rs:89-98`). With enforcement off for the duration, this is what proves the copy
  kept every reference.
- **A crash re-runs the migration on an untouched database.** The schema and the history row commit
  together (`migrate.rs:42`).

### Boundaries in cue frames, converted with the decoder's own rate

A sheet states times as `mm:ss:ff` at 75 frames per second. Storing that integer keeps exactly what
the sheet said and depends on no scan-time guess at the sample rate. The decoder converts it with
its real rate, `frames × rate / 75`, in one function.

- That conversion is exact at 44.1, 48, 88.2, 96, 176.4 and 192 kHz.
- At the odd rates (32, 16, 8 kHz) it floors.
- Track *n*'s end and track *n+1*'s start come from the same stored value through the same function.
  So a boundary never gains or loses a sample, whatever the rate.

Milliseconds would put a boundary up to 44 samples off at 44.1 kHz. A `Duration` can't hold 1/75 s
exactly either, and Symphonia's `calc_timestamp` rounds down (`units.rs:1031-1052`). So the seek to
a span goes in as `SeekTo::TimeStamp` ticks, never through `Duration`.

### Where a track starts and ends

- A track starts at its `INDEX 01` and ends where the next track's `INDEX 01` starts. The gap
  between `INDEX 00` and `INDEX 01` therefore plays at the end of the track before it, which is
  where the disc puts it when played straight through.
- The last track of a file plays to the end of the file. Its `duration_ms` is the image's length
  less its start, taken at scan. The decoder's end of file is what actually stops it.
- **Hidden track one audio:** when `TRACK 01` has `INDEX 00` ahead of its `INDEX 01`, the audio
  between them becomes row `cue_track = 0`, from `INDEX 00` to track 1's start. A region shorter
  than one second is ordinary pregap and stays part of nothing, so a few frames of lead-in don't
  become a "track". The bound is a named constant beside the layout.
- `PREGAP` and `POSTGAP` are silence a burner generates. They take no data from the file, so they
  move no boundary.
- Tracks that aren't `AUDIO` are skipped and keep their numbers. A track starting past the end of
  the image, or ending before it starts, is dropped and logged on its own; the rest of the sheet
  stands.
- **A track belongs to the `FILE` its `INDEX 01` sits under.** In the multi-file layout where a
  track's `INDEX 00` sits under the previous `FILE` line and its `INDEX 01` under a new one, the
  track starts at the new file's `INDEX 01`. Getting this wrong misclaims per-track rips.

### Which sheet claims which file

- **A sheet's `FILE` resolves against the walked audio files, in this order:**
  1. the path as written, relative to the sheet's folder;
  2. the same name, ignoring case and Unicode normalization form;
  3. the same stem with any `is_audio_extension` extension.

  A sheet saying `album.wav` beside a re-encoded `album.flac` is the common case for step 3. A
  Windows absolute path or a backslash path falls back to its file name.
- **The `FILE` type keyword (`WAVE`, `BINARY`, `MP3`, `FLAC`) is ignored.** What decides is whether
  the name resolves to a walked audio file. Sheets get the type wrong routinely.
- **A `FILE` is claimed only when the sheet carves it into at least two rows**, the hidden track
  counting as one. A file holding one track that starts at `00:00:00` is a per-track rip; it keeps
  its own row and its own tags.
- **When several sheets claim one image, one wins, deterministically:**
  1. the sheet whose stem is the image's stem (`Album.cue` for `Album.flac`);
  2. then the sheet named after the whole image file name (`Album.flac.cue`);
  3. then the lowest path.

  The losers are logged.
- **An external sheet beats an embedded one.** It is the one the user can see and edit, and adding
  one is how a wrong embedded sheet gets corrected.
- **A sheet that fails to read or parse keeps its previous claim untouched.** Only a sheet that is
  gone, or that parses cleanly without naming the image any more, releases the image. A write
  caught half-done by the watcher would otherwise delete twelve rows and re-insert them with new
  ids.
- **An image in `excluded_files` produces no rows**, whatever sheet names it. `apply_cue_unit` asks
  inside its inserting transaction, the rule in `.claude/rules/library-data.md`. A folder scan
  restamps the kept-out image (`restamp_on`), and an import lifts the exclusion, the user having
  named the file (`commit_import`).

### What a row takes from the sheet and what from the image

A sheet value applies only when it isn't empty. Everything a sheet doesn't state comes from the
image, **except fields that describe a single recording**, which the image's tags state for the
whole disc:

| field | from the sheet | otherwise |
|---|---|---|
| title | the track's `TITLE` | the image's file stem and the two-digit number, the shape an untagged file's fallback already has. Never the image's own title, which names the disc |
| artist | the track's `PERFORMER`, then the sheet's | the image's artist |
| album / album artist | the sheet's `TITLE` / `PERFORMER` | the image's |
| composer credit | `SONGWRITER` or `REM COMPOSER`, track then sheet | the image's |
| genre, date | `REM GENRE`, `REM DATE`, track then sheet | the image's |
| track number / total | the `TRACK` number / the audio tracks the sheet places on that image | none |
| disc number / total | `REM DISCNUMBER` / `REM TOTALDISCS` | the image's |
| ISRC | the track's `ISRC` | none |
| barcode | `CATALOG` | the image's |
| ReplayGain, track pair | the track's `REM REPLAYGAIN_TRACK_*` | none. `replaygain.rs:104-138` already falls back to the album pair |
| ReplayGain, album pair | the sheet's `REM REPLAYGAIN_ALBUM_*` | the image's album pair, then the image's *track* pair, which over a whole disc *is* the album's gain |
| lyrics, BPM, key, mood, rating, MusicBrainz recording and release-track ids | none | none: per recording |
| artwork, codec, bitrate, rate, depth, channels, MusicBrainz release ids | none | the image's |

`REM` lines attach to the scope they appear in: before the first `TRACK` they describe the sheet,
after it they describe that track. Indentation decides nothing.

### Encoding

Sheets arrive as UTF-8 with or without a BOM, UTF-16, Windows-1252, Windows-1251, Shift_JIS, GBK
and EUC-KR. The decode chain:

1. A BOM picks the encoding.
2. A NUL-byte pattern picks BOM-less UTF-16.
3. Strict UTF-8 is taken verbatim.
4. Otherwise `chardetng` guesses from the whole sheet at once, with no TLD hint, and `encoding_rs`
   decodes.

Feeding the whole sheet pools every `TITLE` into one sample, which is what makes a guess on short
CJK titles workable. There is no per-sheet override: a sheet that guesses wrong reads correctly once
it is saved as UTF-8, which step 3 takes as it stands.

A sheet larger than any sheet could be is refused before it is read, and the cap is a named
constant, so a stray multi-megabyte `.cue` never lands in memory.

### A hand-written parser

The six pure-Rust cue crates on crates.io each fail one of these:

- non-UTF-8 input;
- the multi-file layout where `INDEX 01` follows a new `FILE` with no new `TRACK`;
- keeping the `TRACK` mode, number and per-track `REM` lines.

The seventh crate binds libcue, which is C. Hand-written, the parser is a line reader over text that
is already decoded, and it tolerates sloppy input:

- keywords in any case, quotes optional, trailing junk ignored;
- minutes past 99;
- a bad time rejects its own line, never the whole sheet.

It keeps the raw lines with their spans, which is what Phase 7's writer edits in place.

### Playback: the span is the decoder's

The deck clock counts samples pulled from the source, anchored wherever the track started
(`voice.rs:254-281`). So if a source starts at its span's first sample, ends on its last, and states
the span's length as its duration, every reader of position and duration works unchanged:

- the progress bar and seeking;
- MPRIS `Position` and `Seeked`;
- the scrobble threshold;
- Discord's timestamps;
- skip counting;
- resuming after a restart.

The span therefore lives in `FileDecoder` and nowhere above it. The decoder seeks to the span's start
on open and counts down to its end with the `End::Counted` that container padding already uses
(`file_decode.rs:89-94`). Seeks are offset by the span's start and capped at its length.

**Continuous decode at joints.** When one track runs into the next track of the same image, the
decoder the first one finished with is already standing on the second one's first sample:

- the feeder thread parks it in a one-slot hand-off the backend owns;
- the preload of `(path, start == where it stopped)` takes it back with no reopen, no seek and no
  decoder reset;
- the voice already carries a gapless successor through the same resampler when the shapes match
  (`voice.rs:499-515`).

For a lossy image, this is what keeps a joint from restarting the codec's overlap state. For a long
MP3 image it also saves the accurate seek, which re-parses frame headers from the start of the
stream. A manual jump, a seek or a crossfade still reopens and seeks. That is exact for FLAC and
PCM, whose decoder reset does nothing (`.claude/rules/symphonia.md`).

### Edits go to the sheet

A tag write to the image would stamp every track on the disc. So an edit to a sheet track rewrites
the fields the sheet can hold, at the scope each belongs to:

| fields | scope |
|---|---|
| title, artist (`PERFORMER`), composer (`SONGWRITER`), ISRC, genre, date | the track |
| album (`TITLE`), album artist (`PERFORMER`), disc number, barcode (`CATALOG`) | the sheet |

How the rewrite works:

- Every line it doesn't touch stays byte for byte, line endings and BOM included.
- The new text is encoded in the sheet's own encoding when every character fits, and as UTF-8 with
  a BOM when one doesn't.
- The file is replaced atomically through the tag writer's rename.
- The scan reads the sheet back, so an edit is never undone by the next rescan.

What a sheet can't hold (lyrics, BPM, cover art, mood, the hidden track's title) is read-only for
these rows, with a note saying why. Ratings, favourites and play counts are database-only for sheet
tracks, and nothing writes them into the image.

### Embedded sheets

The sources, in precedence order after an external sheet:

1. **A `CUESHEET` Vorbis comment:** the full sheet text, parsed by the same parser with its `FILE`
   line ignored, plus any `CUE_TRACKnn_<FIELD>` tags overriding per track.
2. **FLAC's native CUESHEET block:** sample offsets and ISRCs, no titles. Its offsets are rounded to
   cue frames, which is exact for anything imported from a sheet, since those offsets are multiples
   of `rate / 75`. Titles come from `CUE_TRACKnn_TITLE` where present.

The block is read by a small reader over the metadata block chain, not through Symphonia's
`chapters()`. Symphonia 0.6.1 maps the block there but drops the track numbers and the audio/data
flag, and it would cost a second probe per file at scan time.

The Vorbis comment comes from opening FLAC as lofty's concrete `FlacFile` and converting it into
the `TaggedFile` the rest of `metadata::extract` already uses. That keeps the single-open discipline
`crates/melodia/tests/lofty_open.rs` pins. Edits to rows from a `CUESHEET` tag rewrite that tag's
text through the same writer. Rows from the native block alone are read-only.

---

## What this does not do

- **Monkey's Audio, WavPack and TTA images** decode nowhere in Melodia, so they are never walked,
  and a sheet naming one claims nothing. Adding a decoder is its own issue.
- **No silence is synthesized for `PREGAP` or `POSTGAP`.** There is no audio in the file to play.
- **No per-track loudness measurement.** The gain comes from the sheet's `REM` lines and the image's
  tags.
- **No `CDTEXTFILE`.** It names a binary CD-TEXT file, not text.
- **No encoding override in Settings** (see Encoding).
- **The duplicate finder can dismiss a sheet track but not remove it.** `excluded_files` excludes a
  whole file (`excluded_files.sql:10`), so removing one track would hide all its siblings on the
  next scan.

---

## Structure

| what | where | note |
|---|---|---|
| `CueFrames`, `CueSpan`, `CueTrack` | `crates/melodia-core/src/entities/cue.rs` | each newtype checks its invariant at construction (`CueTrack` 0 to 99), with `TryFrom` from the raw integer; `CueFrames::to_frames(rate)` is the one conversion |
| `is_cue_sheet` | `crates/melodia-core/src/utils/audio_ext.rs` | beside `is_audio_extension`, the one home for what an extension means |
| bytes → text | `crates/melodia-store/src/media/cue/text.rs` | the BOM / UTF-16 / UTF-8 / `chardetng` chain and the size cap |
| text → sheet | `media/cue/parse.rs` | raw lines kept with their spans; sheet and track scopes |
| sheet → rows per file | `media/cue/layout.rs` | starts and ends, hidden track, data tracks, the multi-file layout, the claim rule |
| `FILE` → walked path | `media/cue/resolve.rs` | the three-step match |
| sheet + image tags → row fields | `media/cue/merge.rs` | the table above; produces the `ExtractedMetadata` shape the ordinary ingest already writes |
| sheet rewrite | `media/cue/write.rs` | Phase 7 |
| embedded sources | `media/cue/embedded.rs` | Phase 8: the CUESHEET block reader and the tag text |
| store units of work | `crates/melodia-store/src/database/queries/scan/cue_units.rs` | `apply_cue_unit` (upsert by key, drop missing numbers, carry playlist entries off the whole-file row, write the sheet stamp) and `release_image` (the reverse), `pub(crate)` on the caller's transaction: the scan chunk's, whose stats triggers drop and return inside it (`commit_scan_chunk`), or the watcher batch's |
| the planner | `crates/melodia-store/src/media/cue/plan.rs` | which sheet claims which image, settled before the walk is cut into chunks (`discover`, `scan/mod.rs:308`; `TX_CHUNK_FILES`, `:150`), so an image and its sheet are never split across two chunks. The scan and the watcher's `process_batch` (`file_event_processor/reconcile.rs`) both call it |
| the span on a deck | `crates/melodia-audio/src/player/source/span.rs` | beside `aac_trim.rs`: the open-at-start, the counted end, the seek offset. `file_decode.rs` calls it |
| the parked decoder | `crates/melodia-audio/src/player/source/park.rs` | Phase 6: the one-slot hand-off and its match rule |

The parser and planner sit in the store because ingest is their only consumer, and a `.cue` the
user opens goes through the store too. `melodia-views` learns nothing about sheets.

**The duplication this collapses.** Per-file statements assume one row per path or per hash in at
least twelve places:

- `update_track_metadata` (`mutations.rs:277`)
- `update_track_location` (`mutations.rs:253`), which re-pointed one row by id and now keys on the
  old path
- `get_track_id_by_path` (`lookups.rs:67`), deleted: its one caller, the rename, asks
  `track_exists_by_path`
- `track_exists_by_path` (`lookups.rs:10`)
- `fold_folder_tracks` (`lookups.rs:46`)
- `load_existing` (`ingest.rs:303`)
- the retag commit (`retag.rs:110`)
- the cover backfill (`ingest.rs:474`)
- the hash move (`moves.rs`, `rows_by_hash_on`, `lookup.rs:240`)
- `get_track_ids_by_paths` (`lookup.rs:195`), one arbitrary id per path, read by `import_files` and
  `match_entries`
- `get_track_ids_by_hashes` (`lookup.rs:215`), whose "any one wins, either playing the same audio"
  is false for a sheet's rows
- both `IDS_IN_SUBTREE` statements (`entity_ids.rs:103`)

Each one gets one of two answers, owned in `queries/track/file_rows.rs`:

- **The whole-file row**, `WHOLE_FILE` (`cue_track IS NULL`), for anything that reads or writes a
  file's own tags.
- **File-level**, every row of the path, for anything that moves or deletes a file. A file moves,
  and all its rows move with it. Keyed on the path, this needs no fragment; an order over files
  ends in `FILE_ORDER`, which adds `cue_track`.

The two id lookups are the exception: they answer every row with its `cue_track`, for the caller to
match on (Phase 5). A statement that interpolates a fragment is a `LazyLock<String>`, as
`TRACKS_IN_DIRECTORY` is, so it stays cached (`database::sql`'s `//!`).

The `RETURNING id, file_path` match in `insert_tracks_batch` (`mutations.rs:217`) keys on the pair.

---

## Phase 1: The rebuilt `tracks` (no behaviour change)

- [x] Dump the current definitions with `/usr/bin/sqlite3`:
      `SELECT type, name, sql FROM sqlite_schema WHERE sql LIKE '%tracks%'`, run on a database
      migrated to head. At this tree that is 15 triggers and 18 named indexes, the order-key ones
      included.
- [x] `build.rs` reads `-- foreign-keys-off` into a `ForeignKeys` field on `Migration`, and `render`
      writes it.
- [x] `database/migrate.rs`: `apply_one` suspends enforcement around the transaction, and
      `run_and_record` runs `PRAGMA foreign_key_check` ahead of the history row. A violation is a
      new `MigrationError` variant in `melodia-core`'s `error.rs`.
- [x] New migration `migrations/20261110000000_cue_tracks.sql`:
  - [x] Its first line is `-- foreign-keys-off`.
  - [x] It creates `cue_sheets` and rebuilds `tracks` with the four columns, the split uniqueness
        and the two `CHECK`s.
  - [x] It drops and recreates every dependent trigger and index, in the order *The rebuild cannot
        eat playlists* gives.
  - [x] Its comments say why enforcement is off around it, for the reader of a file that can never
        be edited again.
- [x] `queries/stats.rs`: the trigger bodies come back unchanged, so only its "must match" note
      repoints, at the new migration.
- [x] `entities/cue.rs` with the three newtypes (`to_frames` lands with its reader, in Phase 3):
  - [x] `CueTrack` and `CueFrames` take a `FromSql` beside them and a `param_from!` line in
        `sql.rs`, as `DismissalSet` does.
  - [x] `Track` gains `cue_track` and `span`.
  - [x] `TrackSummary` gains `span`, which playback reads, a nested `by_name!` entry over
        `cue_start` and `cue_end`.
  - [x] `TagEditRow` and `PlaylistExportRow` gain `cue_track`.
  - [x] `rows/track.rs` reads them, and `TRACK_SUMMARY`, `TRACK_TAG_EDIT` and `PLAYLIST_EXPORT`
        select them: a missed column fails only when the statement runs.
- [x] The whole-file and file-level fragments, and every site listed under Structure moved onto one
      of them. `TRACK_INSERT_COLUMNS` gains the four columns, bound from `NewTrackRow::cue`, and
      `INSERT_CHUNK_ROWS` re-derives itself. `insert_tracks_batch` matches on
      `(file_path, cue_track)`.
- [x] Folder Play orders by `file_path, cue_track` in both `IDS_IN_SUBTREE` statements
      (`entity_ids.rs:103`).
- [x] Docs:
  - [x] `.claude/rules/library-data.md` gains the two fragments and the "a file moves with all its
        rows" rule, since that prohibition can be broken from any query file.
  - [x] `.claude/rules/rusqlite.md`: `insert_tracks_batch`'s match names the pair, and the migrator
        line names the directive.

**Gate:**
- [x] The static gates.
- [x] On copies of the dev database (`~/.local/share/Melodia-dev`) and of the 51,012-track test
      library's database (`backups/melodia-pre-restore-20261108-133149.db`), all of these hold:
  - [x] `sqlite_schema` differs only by `tracks`, its two new indexes and `cue_sheets`;
  - [x] the row counts of `tracks`, `playlist_items`, `track_artists`, `track_credits`,
        `track_genres` and `duplicate_dismissals` are unchanged;
  - [x] `PRAGMA foreign_key_check` and `PRAGMA integrity_check` are clean;
  - [x] `INSERT INTO tracks_fts(tracks_fts) VALUES('integrity-check')` passes;
  - [x] a search still ranks.
- [x] Kenan's hand pass: the app starts, and the library, playlists, play counts, search and stats
      are as before. Run by Claude on Kenan's go (2026-11-10), with watcher renames, moves, a hash
      move, a retag, an import and a queue restore on top.

## Phase 2: Sheets become tracks (the full scan)

- [ ] `encoding_rs` and `chardetng` join `[workspace.dependencies]`, latest at `x.y.z` per
      `cargo search` ⚠️ **re-verify**, named by `melodia-store` alone. Check each against
      `licenses/ATTRIBUTION.txt`'s criterion: an entry is owed only for a crate that asks something
      of whoever ships it, such as a NOTICE.
- [ ] `is_cue_sheet`. `collect_media_files` (`scanner.rs:118`) keeps sheets in their own packed
      `WalkedPaths` on `MediaWalk`, pruned by the same `is_trash_dir`. In `files` they would be sent
      to the audio parse and counted as audio.
- [ ] `WalkIndex::add_row` (`scanner.rs:250`) takes several rows per path, the first stamp winning.
- [ ] `media/cue/{text,parse,layout,resolve,merge,plan}.rs` as Structure gives them.
- [ ] The planner, called from `discover` (`scan/mod.rs:308`) between the row fold and
      `index.changed`:
  - [ ] re-plans a directory only when a sheet's stamp moved, a sheet is new or gone, or an audio
        file in it passed the filter: every launch rescans, and the packed walk exists to keep
        per-file allocations out of that window;
  - [ ] parses those sheets and resolves their `FILE`s;
  - [ ] ranks competing sheets;
  - [ ] applies the claim rule;
  - [ ] keeps claimed images out of `index.changed` through its `keep`, and adds to `to_scan` each
        image owed a cue unit and each one released, since `WalkIndex` stamps an image from any of
        its rows and `changed` alone would never read it again;
  - [ ] hands each chunk the claims of the images in it. A claimed image is parsed like any other
        and then routed to `apply_cue_unit`, never to the ordinary path.
- [ ] A cue unit extracts the image once, through the existing `metadata::extract`, with its hash,
      tags, cover and length. `merge` turns that and the sheet into the rows, and `apply_cue_unit`
      writes them, with `search_text`, the order keys and the credit and genre joins, through the
      shared `bind_track_columns`.
- [ ] `apply_cue_unit` runs on `commit_scan_chunk`'s transaction, asks `excluded_files` there and
      reports into `IngestResult`. `Ingested::for_delta` counts its rows.
- [ ] Carrying playlists:
  - [ ] `apply_cue_unit` replaces a whole-file row's playlist entries with the new rows in sheet
        order, at its position, before deleting it.
  - [ ] `release_image` gives the playlists that held any of the sheet's tracks the whole-file row,
        at the first one's position.
  - [ ] Play count, rating and favourite on the whole-file row are not carried over; they can't be
        attributed to a single track.
- [ ] `load_existing` reads whole-file rows only, so a released image, which the planner puts in
      `to_scan`, is a new file to the ordinary path.
- [ ] A sheet the walk didn't find:
  - [ ] with its image still walked, releases it at once;
  - [ ] with its image gone too, is held in `Unwalked`, which gains `sheets`, because a pass purges
        last so a later folder can re-point the rows (`Orphans::HoldIn`). `finish_on`
        (`queries/scan/finish.rs`) drops its stamp row with the orphans.
- [ ] Docs:
  - [ ] `README.md`'s feature list names cue sheets.
  - [ ] `library-data.md` gains the claim rule and "a failed parse keeps the claim", each where a
        reader would break it.

**Gate:**
- [ ] The static gates, and idle RSS on the 50,500-track library against v0.23.0, with no sheets in
      it: a library without sheets owes nothing to this phase.
- [ ] Kenan's hand pass over this fixture set (`ffmpeg` builds the audio):
  - [ ] a FLAC image with a UTF-8 sheet;
  - [ ] a WAV image with a Shift_JIS sheet;
  - [ ] a Windows-1251 sheet;
  - [ ] a sheet naming `.wav` beside a `.flac`;
  - [ ] a hidden-track sheet;
  - [ ] the multi-file layout over per-track files, which must claim nothing;
  - [ ] two sheets naming one image;
  - [ ] an image that already sat in a playlist.
- [ ] Checks, by a full rescan where it applies:
  - [ ] the titles, numbers and artists are right;
  - [ ] the image's own row is gone and its playlist holds the tracks;
  - [ ] deleting the sheet brings the image back.

  Playback still plays from the top of the file until Phase 3.

## Phase 3: A track plays its span

- [ ] `CueFrames::to_frames(rate)` in `entities/cue.rs`, the one conversion.
- [ ] `source/span.rs`. `FileDecoder` gets an open that takes the span:
  - [ ] It seeks to the start in timeline ticks, through a frames-to-ticks conversion that inverts
        `decode::ticks_to_frames`, and trims to the exact sample with `samples_before`.
  - [ ] It states the span's length as `total_duration`.
  - [ ] It arms `End::Counted` for the span even with no trim. `end_after` loses its
        `let trim = self.trim?` early return (`file_decode.rs:299`). A span's end replaces the
        trim's end, keeping `End`'s "never both"; the last track, with no end, keeps the trim's.
  - [ ] `try_seek` adds the start and caps at the span's length less `SEEK_END_MARGIN`.
  - [ ] `probe_duration` stays whole-file.
- [ ] The span rides with the path. `PlayMedia`, `BeginCrossfade` and `Seek` (`state/action.rs:7-41`),
      `PlayerBackend` (`player_backend.rs:79-101`) and the preload (`handlers.rs:481`,
      `backend/mod.rs:580-613`) carry one value, a path plus an `Option<CueSpan>`, in place of the
      bare `file_path`, so no call site can drop the span. The builders in `transport.rs` fill it
      from `TrackSummary`.
- [ ] The preload seeks. Today it opens and stages without seeking, which would play the next sheet
      track from the top of the image.
- [ ] `same_album` (`crossfade.rs:88-90`): the same `file_path` is the same album. Without it, a
      sheet whose tracks name different performers would crossfade inside its own disc.
- [ ] Docs: none owed. The span's argument lives on `span.rs`.

**Gate:**
- [ ] The static gates, and peak and idle RSS on the 50,500-track library against v0.23.0.
- [ ] Kenan's hand pass:
  - [ ] each track starts and ends on its own audio;
  - [ ] a seek lands inside the track;
  - [ ] a restart resumes inside it;
  - [ ] the album plays through with crossfade on, and has no crossfade or gap at a joint inside
        the image;
  - [ ] MPRIS length and position, the scrobble and Discord show the track, not the disc;
  - [ ] the last track ends at the end of the file;
  - [ ] the hidden track plays.

## Phase 4: Keeping it current (watcher, moves, incremental)

- [ ] `is_library_file` (`watcher.rs:271`), the one place `classify_event` asks, passes sheets as
      well as audio.
- [ ] `process_batch` (`file_event_processor/reconcile.rs`) routes these to one re-plan of that
      folder: every sheet event, and every event on an image a sheet claims or could claim. It is
      the planner the scan uses, scoped to one directory and run ahead of the write transaction,
      since `apply_watch_batch` takes only what was already read (`watch_batch.rs`'s `//!`). The
      outcome reaches it as new `WatchedChange` variants, which run `apply_cue_unit` or
      `release_image` on its transaction. `paths_to_extract` keeps sheets from the tag reader.
- [ ] A rename already re-points every row of the path, `update_track_location` keying on the old
      path since Phase 1. A sheet rename moves its `cue_sheets` row.
- [ ] A move by hash carries the whole file, in the scan (`ingest.rs`) and the watcher alike.
      `MoveCandidates` (`moves.rs`) holds every row per hash and re-points one by id, which would
      carry one row of twelve. So ordinary reads take whole-file rows only, and a sheet image is one
      candidate, named by its gone *path* and spent whole.
- [ ] Incremental scan: a cue unit is current when the image's stamp matches its rows' and the
      sheet's matches `cue_sheets`.
- [ ] Docs: the watcher's module doc says sheets are library files.

**Gate:**
- [ ] The static gates.
- [ ] Kenan's hand pass, with play counts watched throughout:
  - [ ] edit a title in the sheet, and the row updates with its play count kept;
  - [ ] save a broken sheet, and nothing changes;
  - [ ] delete the sheet, and the image comes back;
  - [ ] restore the sheet, and the tracks come back;
  - [ ] move the album folder within the library and across filesystems, and the ids are kept;
  - [ ] delete the image, and its rows go.

## Phase 5: The rest of the library

Each item is a site from the map that breaks once twelve rows share a path:

- [ ] **Tag editor** (`library/tags.rs`, `views/.../tags/open.rs`): sheet tracks open read-only, with
      a note. Phase 7 opens the fields a sheet can hold.
- [ ] **Cover and lyrics writes** are refused for sheet tracks.
- [ ] **Rating write-back** (`ratings.rs:93-103`) is skipped for sheet tracks, so ratings stay in the
      database.
- [ ] **Lyrics:**
  - [ ] no sidecar and no embedded lyrics for sheet tracks, since both describe the disc;
  - [ ] the online-answer store keys on the track, path plus `cue_track` (`lyrics/store.rs`);
  - [ ] the panel's "already holds this song" compares track ids (`lyrics/sheet.rs:55,67`).
- [ ] **Duplicates:**
  - [ ] `is_identical` (`matcher.rs:114-120`) also requires the same span;
  - [ ] rows of one file are never duplicates of each other;
  - [ ] Remove is unavailable on a sheet track; Dismiss stays.
- [ ] **M3U** (`playlist_files/m3u.rs`):
  - [ ] Export writes the image path with a `#MELODIA-CUE-TRACK:<n>` line beside `#MELODIA-HASH`.
  - [ ] Import matches path plus number, and a hash re-match hash plus number (`match_entries`,
        `playlist_files.rs:297`). A foreign entry naming a claimed image, or a `.cue`, expands to
        all its tracks in order.
- [ ] **Browse** (`browse.rs:91-93`): a claimed image lists its tracks in place.
- [ ] **Background tasks:** the retroactive hash hashes each file once
      (`tasks/retroactive_hash.rs`). The rating import skips sheet tracks (`tasks/rating_import.rs`).
- [ ] **Opening a `.cue`:**
  - [ ] `import_files` (`import.rs:25`), `audio_files_from_argv` and the drop accept sheets and
        queue their tracks;
  - [ ] opening a claimed image queues all of its tracks in order, `import_files` taking every row
        `get_track_ids_by_paths` answers;
  - [ ] the four `.desktop` sources gain `application/x-cue`, and `main.wxs` gains its two `.cue`
        rows, which is the list `audio_ext.rs`'s doc names (`packaging.rs` pins it).
- [ ] i18n: each new string through `scripts/update-translations.py`, every catalog finished.
- [ ] Docs: `README.md`'s open-with list, if it names extensions.

**Gate:**
- [ ] The static gates.
- [ ] Kenan's hand pass over each item above.

## Phase 6: Continuous decode at joints

- [ ] `source/park.rs`, a one-slot hand-off owned by the backend:
  - [ ] When a span ends on its count (not at end of file, not on an error), `DecodeAhead`'s feeder
        parks its `FileDecoder` there with the path and the cue frame it stopped on.
  - [ ] An open of `(path, span)` whose start is that frame takes it, re-arms the count for the new
        span, and skips the open, the seek and the reset.
  - [ ] Any other open, a seek or a stop empties the slot. The slot holds one decoder at most.
- [ ] The preload is the only taker. A manual jump, a seek or a crossfade reopens as in Phase 3.
- [ ] Docs: the module doc carries the match rule and why only one decoder is ever held.

**Gate:**
- [ ] The static gates, and RSS against Phase 3.
- [ ] Kenan's hand pass on a live recording and an MP3 image, where the music runs across the
      joints: listen for a click; check that skipping, seeking and crossfading into the next track
      still behave.

## Phase 7: Edits written to the sheet

- [ ] `media/cue/write.rs`:
  - [ ] applies a field set to the parsed lines at its scope and leaves every other line alone;
  - [ ] encodes back in the sheet's encoding, or as UTF-8 with a BOM where a character doesn't fit;
  - [ ] keeps the line endings;
  - [ ] replaces the file through the tag writer's atomic rename.
- [ ] `write_tag_edit` (`library/tags.rs:233`) groups sheet tracks by sheet, writes each sheet once,
      registers it with `self_writes`, and rescans that sheet's unit. Ordinary tracks keep their
      path.
- [ ] The tag editor opens the fields the sheet can hold. The rest, and the hidden track's row, stay
      read-only with the note reworded.
- [ ] i18n as in Phase 5.
- [ ] Docs: `write.rs`'s module doc carries the encoding rule. `library-data.md` gains "a sheet
      track's tags live in its sheet" where a tag write could break it.

**Gate:**
- [ ] The static gates.
- [ ] Kenan's hand pass:
  - [ ] edit a title on a UTF-8 sheet and on a Shift_JIS sheet (once with a character Shift_JIS
        can't hold);
  - [ ] edit the album on a multi-select;
  - [ ] diff each sheet, expecting only the touched lines to change;
  - [ ] the edit survives a full rescan;
  - [ ] the watcher doesn't echo the write.

## Phase 8: Embedded sheets

- [ ] `metadata::extract` opens FLAC as lofty's `FlacFile`, takes its `CUESHEET` and
      `CUE_TRACKnn_*` comments, and converts it into the `TaggedFile` the rest of the function
      reads: still one open (`lofty_open.rs`).
- [ ] `media/cue/embedded.rs`: the CUESHEET block reader over the metadata block chain, RFC 9639
      §8.7. It drops the lead-out and non-audio tracks and rounds offsets to cue frames.
- [ ] Planner precedence: an external sheet, then the `CUESHEET` tag, then the block. Embedded rows
      have no `cue_sheet_id`, and their stamp is the image's own.
- [ ] Edits on `CUESHEET`-tag rows rewrite the tag's text through `write.rs` and save it through the
      concrete `VorbisComments`. Rows from the block alone are read-only.
- [ ] Docs: `README.md`'s cue line covers embedded sheets.

**Gate:**
- [ ] The static gates.
- [ ] Kenan's hand pass:
  - [ ] a FLAC carrying a `CUESHEET` tag;
  - [ ] one carrying only the native block, built with `metaflac --import-cuesheet-from`;
  - [ ] one carrying both, plus an external sheet that should win;
  - [ ] a per-track FLAC whose block lists one track, which must claim nothing.

## Phase 9: Tests

Tests are added where the landed change earns them, on Kenan's go, after the hand passes above.
Existing tests a phase breaks are fixed in that phase.

---

## Cross-cutting

- **Docs land in the phase that stales them**, held to the anchor rule. Most of the argument above
  moves into the module docs of `cue/`, `span.rs` and `park.rs`, and into the migration's comments,
  as each lands.
- **800 lines:** `backend/mod.rs` (647), `ingest.rs` (598), `voice.rs` (593), `metadata.rs` (537)
  and `watch_batch.rs` (387) are the files at risk. The new logic goes into its own modules (`cue/`,
  `cue_units.rs`, `span.rs`, `park.rs`), so each of these gains call sites, not bodies.
- **Memory:** `TrackSummary` gains an `Option<CueSpan>` per queued or cached track, and Phase 6
  holds at most one parked decoder. Both are measured at the gates where they land.
- **Lock order** (ADR 0035) is untouched:
  - the span rides on the plain-data actions;
  - every open and seek stays off the decks lock (`backend/mod.rs:227-229`);
  - the park slot is taken in the preload, ahead of the decks lock.

## Open questions

- ⚠️ **re-verify** on the day: the latest `encoding_rs` and `chardetng` releases, and whether either
  ships a NOTICE.
- The one-second floor for a hidden track is a judgement. A real disc with a shorter hidden track
  would move it, and the constant's doc says what it separates, not why one second.
