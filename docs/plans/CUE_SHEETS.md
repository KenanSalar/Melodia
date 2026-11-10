# CUE Sheets

Working doc. Delete when the feature ships.

Status: **in progress**, Phase 5.5 through its gates, Phase 6 next; Phase 3 owes Kenan's scrobble
and Discord check ·
Created: 2026-11-04

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
- A file or a `.cue` opened from outside every library folder plays without joining the library,
  and stays only while a playlist or the queue holds it.
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
  2. the same name, ignoring case, accents and Unicode normalization form (`utils::fold`), which
     runs only once the exact spelling missed;
  3. the same stem with any `is_audio_extension` extension.

  A sheet saying `album.wav` beside a re-encoded `album.flac` is the common case for step 3. A
  Windows absolute path or a backslash path falls back to its file name.
- **The `FILE` type keyword (`WAVE`, `BINARY`, `MP3`, `FLAC`) is ignored.** What decides is whether
  the name resolves to a walked audio file. Sheets get the type wrong routinely.
- **A `FILE` is claimed only when the sheet carves it into at least two rows**, the hidden track
  counting as one. A file holding one track that starts at `00:00:00` is a per-track rip; it keeps
  its own row and its own tags. The rule is held again once the image is read and the tracks past
  its end are dropped: an image left with fewer rows (one track of the disc that `FILE` resolved
  to, a truncated image) keeps the rows it had, and the sheet goes unstamped so the next scan reads
  both again.
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
  ids. Failing means a read error, a sheet over the size cap, or one placing no track at all; a
  truncated or partly broken sheet still parses. A failed sheet records no stamp, so every scan
  reads it again, and logs it, until it parses.
- **A claim is owed**, and the image read again, when the image or its winning sheet is new or
  changed, or the winner isn't the sheet the library holds the image under. Anything else is
  current, and the scan leaves it alone.
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

### Files opened from outside the library

Opening a file or a `.cue` that sits outside every library folder plays it without adding it to
the library. The library is the folders the user added, which is what onboarding and Settings say,
and it is what the folder-based players do:

- fooyin files such a track under `LibraryID = -1`, keeps it out of its library views, and deletes
  it at quit once no playlist holds it (`removeUnmanagedTracks`).
- Strawberry keeps it as a playlist item only, the collection untouched.
- Windows 11's Media Player plays or queues it without adding it.
- Apple Music imports it, but its library has no folders to be outside of.

How Melodia holds one:

- **It keeps a row**, since the queue, playlists, play counts and scrobbles key on a track id. The
  disabled folder an import files it under already says it is outside the library.
- **It links to no album, artist or genre**: no `album_id`, `artist_id` or `genre_id`, and no
  `track_artists`, `track_genres` or `track_credits` rows. Every album, artist and genre surface
  leaves it out by construction, their counts being kept by triggers on those links
  (`queries/stats.rs`), where filtering each trigger and entity page instead would spread the rule
  over all of them. Its own artist and album text still show.
- **Every track-level library surface reads through one predicate**, `IN_LIBRARY`: the track
  isn't filed under a disabled folder. It is asked as the outside rows' ids, which every index
  carries, so a list walking its order's index stays index-only.
- **It goes once nothing holds it.** A boot sweep deletes an outside file none of whose rows a
  playlist or the restored queue holds, then an outside folder left empty, its cue sheets with it.
  Its play count goes too, having belonged to a file the library never held. A disabled folder
  inside a library folder isn't outside: it waits for that folder's scan to absorb it.
- **The same sweep holds the rule on every launch**, unlinking any outside row that still has a
  link, so an install from before this, a restored database or a stray write is put right
  whatever wrote it. It is one database pass that ends at once without an outside folder, so it
  needs no `one_shot` marker, which lives in `settings.json` and can disagree with a restored
  database. It logs each row it unlinks, so a path that keeps linking shows up rather than being
  quietly undone.
- **A library folder added around its directory takes it over**, as it does today
  (`finish::absorb_nested`), and that scan gives it its links.

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
- **The watcher re-plans only the directories a batch touches**, and those its claims sit in. A
  sheet edited to name a file in some other directory claims it the next time that sheet changes,
  since the edit stamps it.

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
| store units of work | `crates/melodia-store/src/database/queries/scan/cue_units.rs` | `SheetWriter::apply`: per sheet job, the stamp, `apply_cue_unit` (a gone image's rows carried by hash, upsert by key, drop missing numbers, carry playlist entries off the whole-file row) and `release_image` (the reverse), `pub(crate)` on the caller's transaction: the scan chunk's, whose stats triggers drop and return inside it (`commit_scan_chunk`), or the watcher batch's (`handle_sheets`), each job filed under its sheet's folder |
| playlist carry | `crates/melodia-store/src/database/queries/playlist_carry.rs` | `replace_in_playlists` and `merge_into_playlists`, beside the other playlist membership writers |
| the planner | `crates/melodia-store/src/media/cue/plan.rs` | which sheet claims which image, settled in `discover` (`scan/mod.rs`) after the incremental filter, whose output decides which directories it reads again, and before the walk is cut into chunks. Each sheet's work is one job that `CuePlan::into_chunks` never splits, so a sheet's stamp commits with everything it caused. The scan calls it, and the watcher's `process_batch` (`file_event_processor/sheets.rs`) and an import (`library/import.rs`) call it over a few directories through `media/cue/around.rs`'s `settle_dirs` |
| the span on a deck | `crates/melodia-audio/src/player/source/span.rs` | beside `aac_trim.rs`: the open-at-start, the counted end, the seek offset. `file_decode.rs` calls it |
| the parked decoder | `crates/melodia-audio/src/player/source/park.rs` | Phase 6: the one-slot hand-off and its match rule |
| files opened from outside | `queries/track/file_rows.rs`, `queries/scan/mod.rs`, `queries/outside.rs`, `library/outside.rs` | Phase 5.5: `IN_LIBRARY` and `LINKED_STAMP`; `TrackFolder` and `ResolvedIds.links`; the boot sweep and `unlink_on`; which folders it sweeps |

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

- [x] `encoding_rs` 0.8.44 and `chardetng` 1.0.0 join `[workspace.dependencies]` (`cargo search`,
      2026-11-10), named by `melodia-store` alone. Neither ships a NOTICE, nor do `core_detect` and
      `multiversion_no_op`, which `encoding_rs` brings in, so `ATTRIBUTION.txt` owes no entry.
- [x] `is_cue_sheet`. `collect_media_files` keeps sheets in their own packed `WalkedPaths` on
      `MediaWalk`, pruned by the same `is_trash_dir`. In `files` they would be sent to the audio
      parse and counted as audio.
- [x] The sheets get a `WalkIndex` of their own, fed from `cue_sheets` (`fold_folder_sheets`), so a
      new or changed sheet comes out of `changed` and a gone one out of `take_orphans`, spared under
      an unreadable directory as a track is.
- [x] `WalkIndex::add_row` takes several rows per path, the first stamp winning, and `take_orphans`
      names a path once however many rows it held.
- [x] `media/cue/{text,parse,layout,resolve,merge,plan}.rs` as Structure gives them.
- [x] The planner, called from `discover` after `index.changed`, whose output it needs:
  - [x] re-plans a directory only when a sheet's stamp moved, a sheet is new or gone, or an audio
        file in it, or claimed from it, passed the filter: every launch rescans, and the packed walk
        exists to keep per-file allocations out of that window;
  - [x] parses those sheets and resolves their `FILE`s;
  - [x] ranks competing sheets by name, the library's claimer competing unread where nothing
        around it moved;
  - [x] applies the claim rule and the owed rule;
  - [x] takes every image it settled out of `to_scan` (`CuePlan::take_out_of`), and reads each one
        owed a cue unit or released in its sheet's job instead, since `WalkIndex` stamps an image
        from any of its rows and `changed` alone would never read it again;
  - [x] hands the scan its jobs in chunks of their own after the ordinary ones, a job never split.
        A claimed image is parsed like any other and routed to `apply_cue_unit`, never to the
        ordinary path.
- [x] A cue unit extracts the image once, through the existing `metadata::extract`, with its hash,
      tags, cover and length. `merge` turns that and the sheet into the rows, and `apply_cue_unit`
      writes them, with `search_text`, the order keys and the credit and genre joins, through the
      shared `bind_track_columns` (`update_cue_track` shares `update_track_metadata`'s `SET` list).
- [x] `apply_cue_unit` runs on `commit_scan_chunk`'s transaction, asks `excluded_files` there and
      reports into `IngestResult`. `Ingested::for_delta` counts its rows.
- [x] Carrying playlists:
  - [x] `apply_cue_unit` replaces a whole-file row's playlist entries with the new rows in sheet
        order, at its position, before deleting it.
  - [x] `release_image` gives the playlists that held any of the sheet's tracks the whole-file row,
        at the first one's position. The chunk's ordinary ingest writes that row first.
  - [x] Play count, rating and favourite on the whole-file row are not carried over; they can't be
        attributed to a single track.
- [x] `load_existing` reads whole-file rows only (Phase 1), so a released image is a new file to the
      ordinary path.
- [x] A sheet the walk didn't find:
  - [x] with its image still walked, releases it in the scan's chunks;
  - [x] goes to `Unwalked`, which gains `sheets`, because a pass purges last so a later folder can
        re-point the rows (`Orphans::HoldIn`). `finish_on` (`queries/scan/finish.rs`) drops its
        stamp row with the orphans, which takes any row of an image gone with it.
- [x] Docs:
  - [x] `README.md`'s feature list names cue sheets.
  - [x] `library-data.md` gains the claim rule and "a failed parse keeps the claim", each where a
        reader would break it.

**Gate:**
- [x] The static gates.
- [x] Idle RSS on the 50,500-track library against v0.23.0, with no sheets in it: a library without
      sheets owes nothing to this phase. Run by Claude on Kenan's go (2026-11-10): release builds,
      a fresh scan of the library compacted in both roots, ABAB with the page cache dropped. Idle
      Anonymous 36.2 / 36.5 MiB on v0.23.0, 36.3 / 36.5 on `bc60bdde`; peak RSS level.
- [x] Kenan's hand pass over this fixture set (`ffmpeg` builds the audio). Run by Claude on Kenan's
      go (2026-11-10), four launches against `Melodia-dev`, restored after; the sheets are kept in
      `~/Development/melodia-cue-test-files`:
  - [x] a FLAC image with a UTF-8 sheet;
  - [x] a WAV image with a Shift_JIS sheet;
  - [x] a Windows-1251 sheet;
  - [x] a sheet naming `.wav` beside a `.flac`;
  - [x] a hidden-track sheet;
  - [x] the multi-file layout over per-track files, which must claim nothing;
  - [x] two sheets naming one image;
  - [x] an image that already sat in a playlist;
  - [x] twelve real EAC sheets from public parser test suites, audio built to their timings: ISRC
        and `CATALOG`, sub-second track-1 pregaps (no hidden track), `.ape` and `.wav` names over
        `.flac`, Shift_JIS, GBK and UTF-8 with a BOM, and two EAC multi-file rips, one with `PREGAP`.
- [x] Checks, by a full rescan where it applies:
  - [x] the titles, numbers and artists are right;
  - [x] the image's own row is gone and its playlist holds the tracks;
  - [x] deleting the sheet brings the image back;
  - [x] a reread sheet keeps its rows' ids and play counts, and only the owed images are read;
  - [x] an empty sheet keeps its tracks and is read again next scan.

  Playback still plays from the top of the file until Phase 3.

## Phase 3: A track plays its span

- [x] `CueFrames::to_frames(rate)` in `entities/cue.rs`, the one conversion.
- [x] `source/span.rs`. `FileDecoder::open_track(&TrackAudio)` takes the span:
  - [x] It seeks to the start in timeline ticks, through `decode::frames_to_ticks`, the inverse of
        `ticks_to_frames`, and trims to the exact sample with `samples_before`. Every seek, a
        user's included, goes through one `seek_to_frame` as a `SeekTo::Timestamp`; `SeekTo::Time`
        is left for a track with no timebase.
  - [x] It states the span's length as `total_duration`.
  - [x] It arms `End::Counted` for the span even with no trim. A span's end replaces the trim's,
        keeping `End`'s "never both"; the last track, with no end, keeps the trim's.
  - [x] `try_seek` caps at the span's length less `SEEK_END_MARGIN` and lands past its start.
  - [x] `probe_duration` stays whole-file.
- [x] The span rides with the path. `PlayMedia`, `BeginCrossfade` and `Seek`, `PlayerBackend` and
      the preload carry one `TrackAudio` (`entities/track.rs`, a path plus an `Option<CueSpan>`)
      in place of the bare `file_path`. `TrackSummary::audio()` fills it.
- [x] The preload seeks: `open_for_gapless` opens through `open_track`. The probe cache stays keyed
      on the path, every span of a file sharing its shape.
- [x] **A joint inside one image never crossfades** (`crossfade::Joint`). `Continuous`, the next
      track starting on the sample this one ends, is never eligible whatever the settings, and is
      staged with gapless off too, that setting being about joints between files (Kenan's call,
      2026-11-10). `SameAlbum` also takes two tracks of one file, so a sheet naming a performer
      per track still counts as one album under "skip same album". The plan's `same_album` item
      alone would only have held with that setting on.
- [x] Docs: the span's argument lives on `span.rs`, the joint's on `Joint`.
      `.claude/rules/symphonia.md`'s seeking line names the timestamp seek.

**Gate:**
- [x] The static gates, and peak and idle RSS on the 50,500-track library against v0.23.0. Run by
      Claude on Kenan's go (2026-11-10), as Phase 2's, plus a "Playing, list view" window on a
      FLAC opened from the command line. ABAB, v0.23.0 / Phase 3: idle Anonymous 35.9, 35.7 /
      36.5, 36.4 MiB; playing 37.8, 38.1 / 38.3, 38.4; peak RSS 260.8, 252.9 / 255.1, 261.8.
      Playing adds no more over idle on Phase 3 than on v0.23.0, and idle runs none of Phase 3's
      code; v0.23.0 read 36.2 and 36.5 idle in Phase 2's batch.
- [ ] Kenan's hand pass. Run by Claude on Kenan's go (2026-11-10): the Phase 3 release build
      against `Melodia-dev`, restored after, its audio recorded off a null sink. The fixtures are
      time-coded, each second its own tone, so the recording says which sample of the file
      played; they are kept in `~/Development/melodia-cue-test-files/phase3-playback`: a 44.1 kHz
      image of five tracks, one behind a pregap, a 48 kHz image with a hidden track, and a FLAC,
      MP3, Opus and AAC file.
  - [x] each track starts and ends on its own audio: both images played through as one stretch
        each, from the file's first sample to its last, with no sample lost or doubled at any
        joint; a lone cue track ending the queue stops on its span's last sample;
  - [x] a seek lands inside the track: on the span's start plus the target, at 44.1 and 48 kHz,
        and a seek past the end stops at the margin before the span's end;
  - [x] a restart resumes inside it: Previous lands on the span's first sample, and a relaunch
        resumes at the saved position inside the span;
  - [x] the album plays through with crossfade on, and has no crossfade or gap at a joint inside
        the image: crossfade on with "skip same album" and gapless off, the only fade was the
        one out of the image into another file;
  - [ ] MPRIS length and position, the scrobble and Discord show the track, not the disc: MPRIS
        shows each track's span length and a position counted from its start, and each track
        played through counts its own play. The scrobble and Discord, both off in `Melodia-dev`,
        are left to Kenan;
  - [x] the last track ends at the end of the file;
  - [x] the hidden track plays.
  - [x] Ordinary files still start at their first sample and seek to the target, FLAC, MP3, Opus
        and AAC alike.

## Phase 4: Keeping it current (watcher, moves, incremental)

- [x] `is_library_file` (`watcher.rs`), the one place `classify_event` asks, passes sheets as well
      as audio (`is_sheet_file`), so a removed sheet is a `Removed`, never a `DirectoryGone`.
- [x] `process_batch` re-plans through `file_event_processor/sheets.rs`, ahead of the write
      transaction, since `apply_watch_batch` takes only what was already read:
  - [x] one read (`sheets_around`) finds the sheets on record in the batch's directories and the
        claims around them, and stops at an empty `cue_sheets`, so a library without sheets pays
        one `EXISTS`;
  - [x] the walk lists the batch's directories and those its claims sit in, never a whole folder
        (`list_media_files`), and the scan's `cue::settle` plans them, changed and gone taken off
        the events;
  - [x] a sheet's `Created` or `Modified`, and an image the plan settles, leave the ordinary path
        (`CuePlan::settles`), and the outcome reaches the transaction as one
        `WatchedChange::Sheets` (`CuePlan::into_chunk`), applied after the other changes and
        before the deletes. `handle_sheets` runs each job in a savepoint of its own
        (`sql::in_savepoint`), on a copy of the batch's caches kept only with it, a released
        image's whole-file row written inside it through `create_track`, and files it under its
        sheet's folder;
  - [x] `paths_to_extract` keeps sheets from the tag reader, and a removed sheet forgets its stamp
        row once no row names it (`forget_gone_sheet`), applied after every other delete. The plan
        takes a sheet on record that the walk didn't find for gone, whether or not the batch saw it
        go. A failed read or plan leaves the sheets to the next scan, their events safe applied
        alone.
- [x] Renames: an image rename re-points every row, `update_track_location` keying on the old path
      since Phase 1. A claimed image's claim follows it to the new name before the plan
      (`follow_renamed_images`), which updates its rows by number where the sheet still names it
      and gives the image back, playlist places carried, where it doesn't. A sheet's rename is
      split into its removal and its creation: the new name claims the images by number, so every
      row keeps its id, and a rename over another sheet needs no case of its own.
- [x] A move by hash carries the whole file, in the scan and the watcher alike. Ordinary reads take
      whole-file rows only (Phase 1). `MoveCandidates` gains the images a sheet splits, one per
      path under its lowest id (`images_by_hash_on`), and `apply_cue_unit` re-points a gone
      image's rows onto a claimed one whose path holds none (`carry_moved_rows`). The scan reads
      them in `commit_scan_chunk` (`image_move_candidates`), the watcher in `moved_candidates`.
- [x] Incremental scan: a cue unit is current when the image's stamp matches its rows' and the
      sheet's matches `cue_sheets`. Landed with Phase 2's planner, as the owed rule.
- [x] Docs: `watcher.rs` has no `//!`, so `classify_event`, `is_library_file` and `DirectoryGone`
      name sheets. `plan.rs`, `cue/mod.rs`, `cue_units.rs`, `moves.rs`, `queries/scan/mod.rs` and
      `reconcile.rs` say the watcher plans and writes too, and `library-data.md`'s claim rule
      names the watcher batch, its glob widened to `file_event_processor/`.

**Gate:**
- [x] The static gates.
- [x] Kenan's hand pass, with play counts watched throughout. Run by Claude on Kenan's go
      (2026-11-10): the debug build against `Melodia-dev` with watching on, restored after, over
      two scratch library roots, one on btrfs and one on tmpfs. Play counts and ratings were seeded
      into the sheet rows and a playlist put around them, and the database read after every change:
  - [x] edit a title in the sheet, written in place, and the row updates with its id and play count
        kept;
  - [x] save a broken sheet through a rename, as an editor saves, and nothing changes; saving it
        fixed changes nothing either;
  - [x] delete the sheet, and the image comes back, in the playlist where its tracks sat;
  - [x] restore the sheet, and the tracks come back in its place;
  - [x] move the album folder within the library, and across filesystems and back, and the ids,
        play counts, ratings and playlist places are kept. A root the move leaves empty reads as an
        unmounted drive, as it does for any file: its rows stay, and a twin still on disk elsewhere
        keeps the move from being taken;
  - [x] delete the image, and its rows go, its playlist entries with them;
  - [x] a sheet renamed and back, and an image renamed by case, keep every id; a sheet copied in
        ahead of its image claims the image when it lands; an album folder copied in or moved in
        from outside the library is split.
- [x] Recheck after the review fixes, run by Claude on Kenan's go (2026-11-10) as the pass above:
  - [x] an image renamed to a name its sheet doesn't resolve comes back whole, in the playlist
        where its tracks sat, and splits again in place renamed back; renamed by case it stays
        split with every id and play count;
  - [x] a sheet renamed and back keeps every id, only the current name on record; a sheet deleted
        gives the image back and is forgotten, and deleted with its image in one batch both go;
  - [x] a sheet on record whose file is gone without an event is taken for gone: the sheet beside
        it claims the image back with every id;
  - [x] a job failed through a test trigger rolls back alone, its tracks and sheet record kept,
        while the next job in the batch releases its image with the artist the failed job had
        inserted written again rather than taken from the cache; once the trigger is gone, the
        image comes back whole.

## Phase 5: The rest of the library

Each item is a site from the map that breaks once twelve rows share a path:

- [x] **The shapes:** `TrackSummary` gains `cue_track` beside `span`, as `Track` and `TagEditRow`
      have it. `audio_ext.rs` gains `CUE_SHEET_EXTENSION` and `is_library_extension`, which the
      watcher's `is_library_file` and the argv filter read. `get_track_ids_by_paths` and
      `get_track_ids_by_hashes` answer a `FileTracks`: each key's rows with their `cue_track` in
      `FILE_ORDER`, a hash answering every copy's. A number matches the first copy holding it, and
      no number a copy played whole before the first copy's tracks.
- [x] **Tag editor** (`views/.../tags/open.rs`): any sheet track in the selection opens it read-only
      (`TagEditor.read-only`), a note above the tabs. Every field takes `read-only` through a local
      `TagField` and the credit and name editors, the cover buttons and Insert found lyrics hide,
      and Save is off. A single sheet track reads its credits from the database, not the image.
      Phase 7 opens the fields a sheet can hold.
- [x] **Cover, lyrics and rating writes** go through `get_whole_file_paths_by_ids`, which answers
      whole-file rows only, so the dialog's Save, lyrics' Save to file and the rating write-back
      (`ratings.rs`) never reach the image. `update_track_metadata` answers whether it matched, and
      the retag counts a miss as `Gone` rather than `Updated`.
- [x] **Lyrics:**
  - [x] no sidecar and no embedded lyrics for sheet tracks, since both describe the disc
        (`read_local`, `read_local_text`);
  - [x] the online-answer store keys on `LyricsKey`, path plus the number past a NUL, a whole
        file's hash unchanged (`lyrics/store.rs`);
  - [x] the panel's "already holds this song" compares track ids (`LyricsUi::holds`), and Save to
        file isn't offered for a sheet track.
- [x] **Duplicates:**
  - [x] `is_identical` also requires the same span (`DuplicateCopy.span`);
  - [x] a group keeps one row per file, the keeper's (`matcher::one_per_file`);
  - [x] a sheet track isn't removable (`DuplicateCopy::is_removable`): no checkbox, a "From a cue
        sheet" chip, never picked by a toggle, Keep or a suggestion, and `MergePlan` skips one
        should it arrive. Dismiss stays.
- [x] **M3U** (`playlist_files/m3u.rs`):
  - [x] Export writes `#MELODIA-CUE-TRACK:<n>` beside `#MELODIA-HASH`.
  - [x] Import matches path plus number, and a hash re-match hash plus number (`match_entries`).
        A number no copy holds takes the file played whole, as `release_image` hands a playlist
        the image once its sheet goes. An entry with no number takes every row of its file, and a
        `.cue` entry the rows of the files it names (`import::files_opened_by`).
- [x] **Browse** (`browse.rs`): a claimed image lists its tracks in place, `TRACKS_IN_DIRECTORY`
      ordered by `FILE_ORDER`.
- [x] **Background tasks:** the retroactive hash hashes each file once and writes every row of it.
      The rating import's page (`get_unrated_track_paths_after`) takes whole-file rows only.
- [x] **Opening a `.cue`:**
  - [x] `import_files` takes sheets: each opens the audio files it names in order
        (`cue::named_files`), so a per-track rip's sheet queues its files. A file with no row, or a
        sheet with no stamp, is planned over its directory (`cue::settle_dirs`, `owns` the opened
        files) and committed through `commit_import`, which now runs sheet jobs too, each filed as
        `IngestSource::folder_of` says. An image opened beside its sheet is split, outside the
        library too.
  - [x] opening a claimed image queues all of its tracks in order, `import_files` answering one
        block per opened path. `import_and_summarize` orders the blocks by first title
        (`in_play_order`); the queue's and the playlist import's own sorts are gone.
  - [x] `library_files_from_argv`; the drop needed nothing.
  - [x] the four `.desktop` sources gain `application/x-cue` (`desktop_integration_tests.rs`), and
        `main.wxs` gains its two `.cue` rows on a `Melodia.AssocFile.CueSheet` ProgId
        (`packaging.rs`'s `the_msi_offers_every_library_extension`).
- [x] i18n: the two new strings through `scripts/update-translations.py`, every catalog finished.
- [x] Docs: `README.md`'s cue line, `library-data.md`'s claim rule, "a cue sheet's track owns no
      file", the exclusion and the `FileTracks` answer.

**Gate:**
- [x] The static gates.
- [x] Kenan's hand pass over each item above. Run by Claude on Kenan's go (2026-11-10): the debug
      build against `Melodia-dev` on a null sink, restored after, fixtures in a `phase5/` folder
      under the `phase3-playback` root and in the scratchpad outside every library folder, the UI
      driven through ydotool and KWin scripts:
  - [x] opened on the command line: a recorded sheet, a claimed image, an unrecorded sheet outside
        the library (split under a disabled folder, no whole-file row), a per-track rip's sheet
        (its files), an image beside its unrecorded sheet (split), a kept-out image (split, its
        exclusion lifted), and three paths at once (blocks by first title, a track reached twice
        queued once); forwarded by a second instance; dropped from Dolphin onto the queue panel.
        Each queued in sheet order, titled against it so title order would show;
  - [x] the tag editor opens read-only with the note over two sheet tracks, a mixed selection and
        one sheet track (database credits, Lyrics tab locked), typing changes nothing and Save is
        off; a whole file alone edits as before;
  - [x] a star on a sheet track lands in the database and "wrote 4 into 0 file(s)", the image
        byte-identical; on a whole file it writes the file;
  - [x] lyrics: each track of one image shows its own stored answer, the panel reloading from
        track to track, a disc `.lrc` beside the image ignored, Save to file off;
  - [x] duplicates: a byte copy of an image groups as Identical per track, each copy with the
        chip and no checkbox, a click picks nothing, two "Interlude" tracks of one disc don't
        group, and Not Duplicates writes a set over both rows;
  - [x] M3U: the export writes `#MELODIA-CUE-TRACK`, re-imports to the same four tracks, and a
        foreign file expands an image to its tracks and a `.cue` to its, a stale path taking its
        hash and number;
  - [x] Browse lists a claimed image's tracks;
  - [x] the retroactive hash reads an image once for all its rows, and the rating import leaves a
        sheet's tracks unrated beside a whole file it rates.

## Phase 5.5: Files opened from outside the library

Opening a file or a `.cue` from outside every library folder adds it to My Library for good:

- It shows in Songs, Albums, Artists, search and the counts.
- Settings ▸ Library Folders lists its directory as a library folder, and that row's Rescan would
  import the whole directory and everything below it.
- Nothing scans or watches that folder, so deleting the download leaves its tracks behind.

It predates this feature, but the feature makes it common: a `.cue` opened from Downloads brings in
a whole disc. *Files opened from outside the library* gives the model.

- [x] `IN_LIBRARY` beside `WHOLE_FILE` (`file_rows.rs`), asked as the outside rows' ids. Asked as
      `folder_id IN` the enabled folders, it sorts the whole library in a temporary B-tree
      instead of walking `idx_tracks_sort_key` (`EXPLAIN QUERY PLAN` on the 51,012-track test
      library's database, 2026-11-10). The outside ids come through a `CROSS JOIN` from
      `folders`: a plain join, on a database with no `sqlite_stat1` yet, walks all of
      `idx_tracks_folder_id`. Every reader of `tracks` that answers for the library reads it:
  - `Membership`'s `Library` and `Favorites` arms (`track/membership.rs`), under Songs, Favorites
    and every scope's facts and keysets;
  - `smart_playlist::push_where` and `first_cover`, which every smart read goes through, the
    `Drawn` arm's pick included;
  - Most Played, the favourite stats and the Favorites collage (`track/ranking.rs`), Recently
    Played (`track/list.rs`), `search_all`'s tracks, `fold_duplicate_keys`, `count_tracks`, the
    scrobbler's favourites, and the test oracle `ALL_TRACK_LIST_ROWS`.
  - `missing_ids_on` stops asking the `Library` arm and asks every row, so the queue and the
    playlists keep an outside track.
- [x] An outside row writes no album, artist or genre link. `TrackFolder` says where a row is
      filed and whether that folder links it, before any name is looked up. The import, a cue
      unit, a retag and the watcher each pass one. A library row an import re-points by hash into
      an outside folder is unlinked in the same write.
- [x] A library scan reads a row with no links as never stamped (`fold_folder_tracks`,
      `load_existing`). So the scan absorbing an outside directory links the rows it reads, and so
      does the next scan after a hash move into the library.
- [x] The boot sweep (`library::outside`, `queries::outside`), every launch, after the queue is
      restored and before the launch's own files open, ending at once without an outside folder.
      An outside folder is a disabled one that no library folder holds (`folders_inside`, the
      absorb's own rule):
  - [x] unlinks any outside row still linked, logged at warn per row;
  - [x] deletes the outside files none of whose rows a playlist or the restored queue holds, every
        row of a path together. `tracks_credits_cleanup` takes their join rows;
  - [x] then the outside folders left empty, cascading their `cue_sheets`;
  - [x] then the albums, artists and genres left with no track (`prune_orphans`).
- [x] No migration: `album_id`, `artist_id` and `genre_id` are already nullable and `folder_id`
      already indexed (`idx_tracks_folder_id`).
- [x] Settings ▸ Library Folders lists library folders only (`get_folders`), and an outside folder
      refuses no add. An add at its path takes it over, and cancelling that scan keeps its tracks
      (`OnStop::Keep`). `scan_one` refuses a folder that isn't a library folder.
- [x] Edit Tags reads an outside file's genres, role credits and release tags from the file
      (`read_file_tags`), the database holding none for it (Kenan's call, 2026-11-10). An outside
      sheet track's are its sheet read over its image by the scan's own rules
      (`cue::read_track_tags`, `merge::track_tags`).
- [x] Docs:
  - `library-data.md` gains "a library surface reads through `IN_LIBRARY`" and "an outside track
    links to no album, artist or genre";
  - `README.md` says opening a file plays it without adding it;
  - the release notes say an install's opened files leave My Library and stay in their playlists.
    They are written on the draft release, so the line waits for the release.

**Gate:**
- [x] The static gates.
- [x] Kenan's hand pass. Run by Claude on Kenan's go (2026-11-10): the debug build against
      `Melodia-dev` on a null sink, restored after, the UI driven through ydotool and KWin scripts,
      the fixtures in the scratchpad outside every library folder:
  - [x] a file and a `.cue` opened from outside play and queue, and show on no library page, in no
        search and in no count: Songs stayed at 525 and Albums at 93, a search for their titles
        found nothing while one played, and their row menus offer no Go to entry;
  - [x] one added to a playlist survives a restart, and once removed from it is gone after the
        next; one only the restored queue holds survives too;
  - [x] Settings lists library folders only, four outside folders on record;
  - [x] adding that folder as a library folder brings its tracks in, linked: the rescan after the
        cancel below re-read the opened file whatever its stamp and linked it under its id;
  - [x] cancelling the scan of a library folder added at an opened file's directory keeps the
        tracks: 103,001 files, cancelled after 10,000, the opened file, sorted last and unread,
        kept under the folder taken over;
  - [x] an outside folder nested in a library folder survives a relaunch until that folder's scan
        absorbs it, linked: the first scan quit after 10,000 files, the relaunch's sweep left the
        nested row, and the resumed scan linked it under its id;
  - [x] Edit Tags on an opened file, alone and beside library tracks, shows its genres, composer
        and label, and a Save touching only the title leaves them in the file;
  - [x] an install carrying opened files from before keeps them in their playlists only: opened
        and added to a playlist on the HEAD build, the new build's first launch unlinked all five
        rows with a warning each, deleted the one nothing held, kept a disc whole for its one
        held track, and pruned their albums, artists and genres.

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
- [ ] An outside sheet track's fields open on what `cue::read_track_tags` reads, its row holding
      no join rows, and the re-read of its sheet after a write files it as
      `TrackFolder::Outside`, or it gains the links Phase 5.5 keeps from it.
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
- [ ] Edit Tags reads an outside embedded-sheet track through its embedded sheet: `read_files`
      (`callbacks/tags/open.rs`) finds a sheet only through `cue_sheet_id`, so such a track opens
      with its genres, credits and release tags blank.
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
- **800 lines:** `backend/mod.rs` (647), `ingest.rs` (625), `voice.rs` (593), `metadata.rs` (541)
  and `watch_batch.rs` (387) are the files at risk. The new logic goes into its own modules (`cue/`,
  `cue_units.rs`, `span.rs`, `park.rs`), so each of these gains call sites, not bodies.
- **Memory:** `TrackSummary` gains an `Option<CueSpan>` per queued or cached track, and Phase 6
  holds at most one parked decoder. Both are measured at the gates where they land.
- **Lock order** (ADR 0035) is untouched:
  - the span rides on the plain-data actions;
  - every open and seek stays off the decks lock (`backend/mod.rs:227-229`);
  - the park slot is taken in the preload, ahead of the decks lock.

## Open questions

- The one-second floor for a hidden track is a judgement. A real disc with a shorter hidden track
  would move it, and the constant's doc says what it separates, not why one second.
