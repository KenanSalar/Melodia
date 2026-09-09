# Multi-artist credits

Working doc. Delete it when the feature ships.

## The problem

A track's artist is one opaque string end to end. `metadata.rs` reads it through lofty's
`Accessor::artist()`, which returns the first value only, so a file carrying two artist values
loses the second at scan time. On ID3v2.4 the values arrive NUL-joined inside one string
instead, which `utils::fold` already documents and papers over when *comparing* while the
column keeps the raw NUL. The Edit Tags dialog offers one field per artist, and `tag_writer`
writes through `Tag::insert_text`, which replaces every value under a key with one, so any tag
edit collapses a multi-artist file to a single name.

## The shape a credit has

An ordered list of `(name, join phrase)` pairs. The join phrase follows its name and is free
text as printed on the release: `feat.`, `&`, `+`, `with`, `vs.`, `,`, `presents`. The last
credit's phrase is empty. Two tags carry it, and both are read across the ecosystem:

| tag | meaning | ID3v2 | Vorbis | MP4 | APE |
|---|---|---|---|---|---|
| `ARTIST` | the credit as printed, one value | `TPE1` | `ARTIST` | `©ART` | `Artist` |
| `ARTISTS` | one value per artist | `TXXX:ARTISTS` | `ARTISTS` | `----:com.apple.iTunes:ARTISTS` | `Artists` |

`ALBUMARTIST` / `ALBUMARTISTS` are the exact siblings. lofty models all four through
`ItemKey::{TrackArtist, TrackArtists, AlbumArtist, AlbumArtists}` and owns the per-format
encoding, so nothing here needs a hand-rolled separator.

**A single-value artist string is never split on a delimiter**, not at scan and not as a
migration. `AC/DC`, `Simon & Garfunkel` and `Earth, Wind & Fire` are one artist each, and a
correct exceptions list is unmaintainable. Structured tags are the only source of a split.

## Where each layer keeps what

| layer | holds |
|---|---|
| the file | `ARTIST` = rendered credit, `ARTISTS` = one value per name |
| `tracks.artist` / `tracks.album_artist` | the rendered credit |
| `tracks.artist_id` | the primary credited artist |
| `track_artists` / `album_artists` | the ordered credit, join phrases included |

Keeping the rendered credit in the existing text columns is the load-bearing decision. It is
what leaves FTS, scrobbling, the Now Playing ladder, M3U8 export, smart playlists, `row_match`
and `track_sort` untouched, and it is why no FTS rebuild is needed: the display string already
contains every name the `ARTISTS` list does.

## Phases

- [x] **1 · Vocabulary and files.** `ArtistCredit`, `JOIN_PHRASES`, `render`, `credits_from`;
      the read ladder in `metadata.rs`; `set_texts` / `apply_credits` in `tag_writer.rs`;
      `TagEdit` field types.
- [x] **2 · Schema and ingest.** The migration, `stats.rs` trigger consts,
      `recalculate_all_stats`, `replace_credits`, the three write sites.
- [x] **3 · Read paths.** The artist-scoped queries move to a join, checked under
      `EXPLAIN QUERY PLAN`.
- [x] **4 · The dialog.** Model, global, `CreditEditor` mounted twice, callbacks.
- [x] **5 · Backfill sweep.** `one_shot` `Sweep` re-reading tags for existing installs.

## Traps worth writing down

- **The join tables carry no `ON DELETE CASCADE` on the parent id.** A cascade fires after the
  parent row is gone, so the stats trigger could no longer read the duration it has to
  subtract. A `BEFORE DELETE` trigger on `tracks` / `albums` deletes the join rows explicitly,
  routing them through the normal decrement path while the parent still exists.
- **Three things move in lockstep with the migration.** `queries/stats.rs` keeps the trigger
  text as Rust consts and drops and recreates them around bulk scan chunks, so the new triggers
  join that dance; `recalculate_all_stats` recomputes the artist columns and moves to the join
  tables; `artist_stats`'s `track_count > 0` filter is what starts admitting featured-only
  artists.
- **The join-phrase array is a Rust-populated `[string]` on purpose.** A join phrase is literal
  tag content, not a UI label, so translating it would corrupt the file. This is the one place
  where "a Rust-populated `[string]` renders untranslated" is the wanted behaviour.
- **The dialog reads a single track's credits off the file.** It already opens that file for
  lyrics, and the file is the authority. A multi-track selection reads the database instead,
  since re-reading N files on the open path is what `TagEditRow` exists to avoid. Until the
  backfill sweep has run, that split is also what keeps the dialog right on a library whose
  join rows were seeded from `artist_id` alone.

## Not in scope

- `lyrics_directory/recording.rs`'s `ARTIST_SPLITS`. It exists because credits were
  unstructured and real credits would let it retire, but the lyrics path is fed
  `TrackSummary.artist` and plumbing credits through that projection is its own change.
- Any heuristic split of an existing single-value artist string.
