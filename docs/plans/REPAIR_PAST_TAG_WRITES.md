# Plan: repair what past tag writes left behind

Status: **in progress** · Owner: Kenan · Created: 2026-11-24 · Branch `fix/durable-file-writes`

Working doc. Delete when every phase has shipped.

## Context

**What you see today.** Nothing on this machine. A user who rated or edited files in v0.17.0 to
v0.23.0 may see:

- An M4A's stars in Melodia and in no other player. A library rebuild loses them.
- A WAV or AIFF listed under its file name, untagged in every app, after a tag edit or a star click.
- Such an AIFF also won't play: symphonia's AIFF reader rejects the broken chunk past `SSND`, in
  `decode::probe`, which playback and the audio check share.

**Why.** Both bugs arrived with lofty 0.25.4, first shipped in v0.17.0 (2026-09-22) and still the
behaviour of 0.25.5, the latest release. "Write ratings to tags" ships on, so a star click was
enough.

- **M4A.** lofty's `Ilst` merge keeps a rating only as `email|stars|counter`
  (`lofty-0.25.5/src/mp4/ilst/mod.rs:887-899`). Melodia handed it the bare percentage, which it
  dropped while the write reported success. The row kept its stars: a re-read leaves them alone
  when the file has none (`queries/scan/mutations.rs:315`).
- **WAV and AIFF.** lofty's chunk writer splices a new tag in at the container's stated end
  (`lofty-0.25.5/src/id3/v2/write/chunk_file.rs:166-189`).
  - Where the header leaves out the final pad byte, that point falls inside the old tag chunk,
    which lofty then drains, cutting the new chunk's first byte.
  - The re-read after the write found no tags and reset the row to file-name metadata.
  - The audio is untouched.
- **New writes are fixed** by `rating_tags::write_stars`' `Ilst` arm and
  `tag_writer::include_final_pad`. Neither reaches a file written before them.

**Sized on 2026-11-24, read-only:**

| Library | Rated tracks | Rated M4A | WAV and AIFF | At-risk shape | Damaged |
|---|---|---|---|---|---|
| Installed (`Melodia/`) | 7 | 0 | 0 | 0 | 0 |
| Dev (`Melodia-dev/`) | 212 | 0 | 3,583 | 8, all in `~/Music/Madih` | 0 |

The at-risk shape is an odd stated size one byte short of the file. A tag write before the fix
would have damaged any of those 8.

**After.**

- **M4A stars.** Once per install, stars that live only in the database are written into the file,
  while "write ratings to tags" is on.
- **Damaged WAV and AIFF.** Once per install, a file the old writer damaged gets its tag chunk
  back, and its row gets its tags back.
- **Visibility.** Nothing on screen; each pass logs one summary line.

**What keeps working:**

- every tag write, through the same path and the audio check
- a star set while the switch was off stays in the database alone, as for every other format
- the watcher ignoring both passes' writes
- a loudness measurement surviving a repair, which changes only tags
- a file neither pass recognises staying byte for byte as it is

## Prior art in the tree

- **The pass shape.** `tasks/rating_import.rs` and `tasks/embedded_cue_probe.rs` are one-shot
  passes over keyset pages of `(id, path)` rows through `one_shot::spawn`, each marked in
  `settings.json` only after a full run (`OnFailure::Retry`). Both new passes take that shape.
- **The write.** `library::ratings::write_rating_to_files` is the rating write itself, through
  `write_tag_edit`: self-write marks, the audio check, and the retag commit that carries loudness.
  The M4A pass writes through it unchanged.
- **The re-read.** `queries::scan::commit_retag` refreshes a row from its re-read file and carries
  its loudness measurement (`retag.rs:116`). The repair re-reads through its refresh, not through
  a rescan: the general ingest never carries loudness, so the album would be measured again.

## Structure

| File | Owns after this plan |
|---|---|
| `melodia-app/src/tasks/one_shot.rs` | the keyset page walk (`Pages`), the scan-pool page read (`read_page`) and the per-file read that counts what was out of reach (`read_files`), beside `spawn` |
| `melodia-store/src/database/queries/track/lookup.rs` | the whole-file page queries by codec, the library's, every row's and the rated MP4 rows', over one builder; and one row's rating |
| `melodia-store/src/database/queries/scan/retag.rs` | `commit_reread`, `commit_retag`'s refresh for a re-read with no edit, which always rolls up album covers and prunes orphans |
| `melodia-store/src/media/ingest/metadata.rs` | `codec_name`, the one spelling of the `codec` column |
| `melodia-store/src/media/ingest/rating_tags.rs` | `read_stars`, one file's stars |
| `melodia-app/src/tasks/m4a_rating_backfill.rs` (new) | the M4A pass |
| `melodia-store/src/media/ingest/iff_repair.rs` (new) | the container size and `include_final_pad`, the damage fingerprints over chunk headers, and the repair through `atomic_file::rewrite_with_sync`, a chain folded into one chunk, the walk checked clean after the mend and again after the fold |
| `melodia-store/src/media/ingest/tag_writer.rs` | `save_tags`, open to the crate for the fold; `include_final_pad` leaves it |
| `melodia-app/src/tasks/iff_tag_repair.rs` (new) | the WAV and AIFF pass: page, repair, re-read |
| `melodia-app/src/services/settings/library.rs` | two markers, `m4a_ratings_backfilled` and `iff_tags_repaired` |
| `melodia/src/boot/tasks.rs` | spawns both beside `rating_import` |

**Duplication collapsed first:**

- **The page loop.** It was spelled out in `rating_import.rs` and `embedded_cue_probe.rs`, each
  with its own `PAGE_ROWS` of 2,000. It moves into `one_shot` before a third and fourth copy
  exist, and so does the cue probe's split of a page's reads into read, unreadable and out of
  reach, which the M4A pass, the repair and the rating import's per-file read share.
- **The codec page query.** `WHOLE_FLACS_AFTER` (`lookup.rs`) becomes a query taking its codecs
  as lofty `FileType`s, spelled through `codec_name` as the ingest writes them. The cue probe asks
  for `Flac`, the repair for `Wav` and `Aiff`. Phase 2 splits its scope: the cue probe keeps the
  library's rows (`get_library_file_paths_after`), the repair takes every row
  (`get_whole_file_paths_after`), a star on a file opened from outside the library having cut its
  chunk too.
- **The container size.** `include_final_pad` read and wrote it inside `tag_writer`. It moves into
  `iff_repair`, whose fingerprints and repair read and write the size through the same type.
- **`LibraryFlags`' lint reason.** Its `#[expect(clippy::struct_excessive_bools)]` said "six
  independent settings.json keys, four of them one-shot markers". Phase 1 restates it for seven
  and five, Phase 2 for eight and six.

## Phases

### Phase 1: write M4A stars the database holds alone (done)

1. **Extract the page loop and the codec page query** (Structure). `rating_import` and
   `embedded_cue_probe` move onto both with no change in behaviour.
2. **The work list.** Rated whole-file rows whose `codec` is `Mp4`, the value every `.m4a` and
   `.m4b` row holds. Rows outside the library are included: a star click writes theirs too.
3. **Per page:**
   - Read each file's stars on the scan pool through `rating_tags::read_stars`, which
     `rating_import::read_each` shares.
   - Then, one file at a time: re-read the row's stars, and write them through
     `write_rating_to_files` where the file disagrees, an unrated file included.
   - One file at a time so a quit stops the pass between whole-file rewrites: a group of one
     value was one uncancellable `write_tag_edit` over up to a page of copies, and the exit cuts
     a write short, leaving a `.tmp` beside the track.
   - The re-read sits beside each write, so a click landing in that one write's window gets its
     own write-back after the two-second quiet period, which lands last and wins.
4. **The switch.** Read once at the start. Off: the pass records its marker and writes nothing,
   since Melodia never pushes a star set while the switch was off for any format.
5. **Failures:**
   - A file that is there but won't read is skipped.
   - A file out of reach (gone, or held by another program) fails the pass after its walk, as
     the cue probe does, so an unmounted drive's M4As are written next launch.
   - A quit mid-pass stops it with an error, finished next launch.
   - A write that fails is logged by `write_rating_to_files` and the row keeps its stars.
   - A database error fails the pass. Every failure leaves the marker down.
   - A rerun rewrites nothing that already matches.
6. **Log.** One line: how many files took their stars, and how many failed.
7. **Docs this phase makes wrong:** `one_shot.rs`' `//!` (it owns the page loop too),
   `rating_import.rs`' and `embedded_cue_probe.rs`' paging docs, and `LibraryFlags`' doc.

### Phase 2: repair WAV and AIFF tag chunks (done)

1. **Reproduce every damaged shape first** (done 2026-11-24, a scratch crate outside the repo).
   144 hand-built at-risk fixtures, each written three times: twice by lofty 0.25.5 as old
   Melodia saved, then once after `include_final_pad`'s restate. They crossed:
   - WAV and AIFF
   - the old `ID3 ` chunk last, ahead of an odd last chunk, or absent
   - `ID3 ` and `id3 `, which made no difference
   - an old tag roomy enough for WAV's in-place `JUNK` path, or not, and small and large edits
   - with and without `LIST INFO` (WAV) or `NAME` (AIFF)

   232 of the 432 writes left damage, all in two shapes, both a walk that lands on `D3 ` with the
   chunk's size and `ID3` 7 bytes on, the drifted pad a zero byte past the new chunk:
   - **Cut id** (128): the chunk lost its `I` to the drain. The old chunk was last and the new one
     didn't fit, or a WAV's earlier in-place write left `JUNK` last, which swallowed the `I`.
     Every reader loses the tag.
   - **Landed early** (104): the chunk starts in the odd last chunk's pad slot, its `I` the pad.
     lofty and Melodia read it, ffmpeg doesn't, and TagLib only sometimes.

   A cut-id file written again grows a chain: the old writer splices each later chunk in ahead of
   the stray byte, and the fixed writer appends it past the byte, now counted. A landed-early file
   heals under the fixed writer. An AIFF with a `NAME` chunk was never damaged, its text-chunk
   write restating the size first. On a copy of one of the 8 Madih WAVs, one write cut the id and
   a second chained.
2. **Fingerprints.** One strict walk over the chunk headers, which reads the ID3v2 header too
   where one is cut. It requires the tag's stated length to match the chunk's, nothing but `ID3 `
   and `JUNK` chunks past it, and exactly one stray zero byte: the last one, or one just ahead of
   an `ID3 ` chunk. The `I` in the pad slot of an odd chunk tells the two shapes apart. Anything
   else is left alone.
3. **The repair.**
   - Write in the missing byte (an `I` at the break, or a zero pad ahead of a landed-early chunk),
     shift what follows up to the stray byte one place on over it, and restate the container size.
     The file keeps its length.
   - **Fold a chain.** TagLib reads only the first ID3 chunk, so a chain's later edits were
     invisible to it. lofty merges a chain's frames, later ones winning
     (`id3/v2/frame/list.rs:239`). The repair saves that merge over the first chunk through
     `tag_writer::save_tags` and cuts the rest.
   - Write through `atomic_file::rewrite_with_sync` with the file marked as a self-write. The copy
     must show the damage `diagnose` found, and walk clean once mended and again once folded.
   - Run `tag_writer::require_same_audio` in the closure as a guard. A damaged AIFF won't demux,
     so the guard writes it unchecked with a warning; the mended span holds tag chunks only.
   - Tried in the scratch crate on every damaged representative: each walk came out clean, the
     audio bytes matched, and lofty, TagLib and ffprobe agreed on every tag.
4. **The re-read.** Extract the repaired file and commit it through `commit_reread`. The row gets
   its tags back and its loudness measurement is carried. A repair moves its row off file-name
   metadata, so the commit always rolls up album covers and prunes orphans, or an album it
   recreates stays coverless until a later scan writes. The row's stars give way only to a rating
   the tag holds (`mutations.rs:315`).
5. **No switch.** It runs whatever "write ratings to tags" says: a tag edit damages a file as
   surely as a star click, and the repair restores the user's own data.
6. **Failures and log:** as in Phase 1. One summary line names how many files were repaired, and
   one debug line per file that matched no fingerprint while looking damaged (a walk that
   breaks). A failed repair or re-read is counted and unmarks the file, so the watcher reads back
   whatever did land.
7. **Docs this phase makes wrong:** `include_final_pad` moves into `iff_repair`, whose `//!`
   names the splice once for both the prevention and the undo.

### Phase 3: Tests (open)

## Verification

- **Each phase.** `cargo fmt --all`, `cargo clippy --all-targets --locked --workspace -- -D warnings`
  and `cargo test --locked --workspace`.
- **Phase 1, by hand, in the dev data folder (backed up and restored):**
  - Copy a few M4As into a test folder and give their rows stars with an `UPDATE` alone, the files
    holding none.
  - Launch. The files take the stars, and a parse of each `ilst` reads a text `rate` atom of the
    matching percentage. ffprobe maps no `rate`, so it can't show one.
  - A second launch runs nothing, the marker being recorded.
  - A rerun with the marker lowered rewrites nothing that already matches.
  - A file out of reach fails the pass, marker down, and the next launch writes it; a file that
    won't read is left out.
  - With the switch off, nothing is written and the marker is still recorded.
  - Done 2026-11-24, on three generated M4As opened from outside the library, all as above.
- **Phase 2, by hand:**
  - Damage copies of the 8 at-risk WAVs with plain lofty 0.25.5 setting an album, a scratch program
    outside the repo, one of them written twice for a chain, beside a landed-early fixture.
  - Launch. Each file's chunk walk is clean, its tags read back in ffprobe and TagLib, its audio
    decodes identically, its row shows its title again, and its loudness row is kept.
  - A clean WAV and AIFF, and the 3,575 even-sized files, stay byte for byte.
  - A second launch runs nothing.
  - Pending since the switch to `commit_reread`: an album the repair recreates shows its cover in
    the Albums grid, and no album or artist the file-name rows held is left orphaned.
  - Done 2026-11-24 under `cargo mcp`, the data folder backed up and restored. A test library
    folder held the 8 damaged Madih copies (one chained) and six step 1 shapes, beside one damaged
    WAV opened from outside the library and two clean controls:
    - With the outside file moved away, the pass repaired 14, failed 0, and failed the run as one
      out of reach, marker down. With the file back, the next launch repaired it alone and
      recorded the marker. The launch after ran nothing.
    - Each file walked clean, one ID3 chunk per chain, and TagLib and ffprobe read its original
      title with the edited fields. The audio bytes matched the originals.
    - Each row took back its title (Songs showed the 8 Madih titles under "Hand Test Album") and
      its seeded loudness row moved to the new hash with its gain kept. Later boots re-parsed
      none of the 16 library files.
    - The controls stayed byte for byte, and all 3,583 library WAV and AIFF files kept their
      inode, size and modified time.
    - The audio check passed the three damaged AIFFs unchecked (symphonia rejects the broken
      chunk), and decoded them once repaired.
