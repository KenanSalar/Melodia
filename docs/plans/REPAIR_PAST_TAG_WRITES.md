# Plan: repair what past tag writes left behind

Status: **in progress** · Owner: Kenan · Created: 2026-11-24 · Branch `fix/durable-file-writes`

Working doc. Delete when every phase has shipped.

## Context

**What you see today.** Nothing on this machine. A user who rated or edited files in v0.17.0 to
v0.23.0 may see:

- An M4A's stars in Melodia and in no other player. A library rebuild loses them.
- A WAV or AIFF listed under its file name, untagged in every app, after a tag edit or a star click.

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
  its loudness measurement (`retag.rs:116`). The repair re-reads through it, not through a rescan:
  the general ingest never carries loudness, so the album would be measured again.

## Structure

| File | Owns after this plan |
|---|---|
| `melodia-app/src/tasks/one_shot.rs` | the keyset page walk (`Pages`), the scan-pool page read (`read_page`) and the per-file read that counts what was out of reach (`read_files`), beside `spawn` |
| `melodia-store/src/database/queries/track/lookup.rs` | one whole-file page query by codec, the rated-MP4 page, and one row's rating |
| `melodia-store/src/media/ingest/metadata.rs` | `codec_name`, the one spelling of the `codec` column |
| `melodia-store/src/media/ingest/rating_tags.rs` | `read_stars`, one file's stars |
| `melodia-app/src/tasks/m4a_rating_backfill.rs` (new) | the M4A pass |
| `melodia-store/src/media/ingest/iff_repair.rs` (new) | the damage fingerprints over chunk headers, and the repair through `atomic_file::rewrite_with_sync` |
| `melodia-app/src/tasks/iff_tag_repair.rs` (new) | the WAV and AIFF pass: page, repair, re-read |
| `melodia-app/src/services/settings/library.rs` | two markers, `m4a_ratings_backfilled` and `iff_tags_repaired` |
| `melodia/src/boot/tasks.rs` | spawns both beside `rating_import` |

**Duplication collapsed first:**

- **The page loop.** It was spelled out in `rating_import.rs` and `embedded_cue_probe.rs`, each
  with its own `PAGE_ROWS` of 2,000. It moves into `one_shot` before a third and fourth copy
  exist, and so does the cue probe's split of a page's reads into read, unreadable and out of
  reach, which the M4A pass and the repair share.
- **The codec page query.** `WHOLE_FLACS_AFTER` (`lookup.rs`) becomes a query taking its codecs
  as lofty `FileType`s, spelled through `codec_name` as the ingest writes them. The cue probe asks
  for `Flac`, the repair for `Wav` and `Aiff`.
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

### Phase 2: repair WAV and AIFF tag chunks (open)

1. **Reproduce every damaged shape first.** Run lofty 0.25.5's `save_to_path` on generated
   fixtures, with each combination:
   - the old `ID3 ` chunk last, not last, or absent
   - a new tag that fits in place (WAV's `JUNK` path) or doesn't
   - WAV and AIFF
   - an upper- and a lower-case chunk id

   Record each resulting shape. One is confirmed so far, on a WAV whose tag chunk was last and
   grew: the walk lands on `D3 ` with the chunk's size, `ID3` starts 7 bytes on, and exactly one
   stray byte follows the chunk. Where the chunk was not last or absent, reading lofty's splice
   suggests the `I` survives and the walk takes it for the previous chunk's pad instead. Step 1
   settles that.
2. **Fingerprints.** One per shape found, each matched exactly from chunk headers alone: a few small
   reads per file. Anything else is left alone.
3. **The repair.**
   - Rebuild the chunk header, drop the stray byte, and restate the container size.
   - Write through `atomic_file::rewrite_with_sync` with the file marked as a self-write.
   - Run `tag_writer::require_same_audio` in the closure as a guard.
   - Tried by hand on the confirmed shape: the chunk walk came out clean, ffprobe read the original
     title and the edited album, and the audio matched the pristine file.
4. **The re-read.** Extract the repaired file and commit it through `commit_retag` with an edit
   that keeps everything. The row gets its tags back and its loudness measurement is carried. The
   row's stars give way only to a rating the tag holds (`mutations.rs:315`).
5. **No switch.** It runs whatever "write ratings to tags" says: a tag edit damages a file as
   surely as a star click, and the repair restores the user's own data.
6. **Failures and log:** as in Phase 1. One summary line names how many files were repaired, and
   one debug line per file that matched no fingerprint while looking damaged (a walk that
   breaks).
7. **Docs this phase makes wrong:** `include_final_pad`'s doc, which names the bug the repair
   undoes, gains a pointer to `iff_repair`.

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
    outside the repo.
  - Launch. Each file's chunk walk is clean, its tags read back in ffprobe, its audio decodes
    identically, its row shows its title again, and its loudness row is kept.
  - A clean WAV and AIFF, and the 3,575 even-sized files, stay byte for byte.
