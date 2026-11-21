# Lyrics finder: pick lyrics by hand, on a list shared with Find Headphones

Working doc. Delete when the feature ships.

Status: **Phase 3 landed; Phase 4 next** · Created: 2026-11-21 · Branch `feat/enhanced-lrc`, after
[ENHANCED_LRC.md](ENHANCED_LRC.md)'s Phase 5

> Researched **2026-11-21** against `feat/enhanced-lrc` at `8db89fc3`; lrclib's API reference
> (`lrclib.net/docs.txt`) and live `/api/search` answers; `serde-saphyr`'s published versions; and
> the Strawberry, fooyin, rox and sonora checkouts (no termusic clone is on disk).

---

## What you see today

- **A wrong or missing online answer has one recourse**, "Look up again", which asks lrclib the
  same question and stores the same answer. lrclib's other candidates are never shown.
- **Settings › Lyrics holds three switches and no action.**
- **An lrclib record with word timings lights line by line**: the client reads `syncedLyrics` and
  ignores the record's `lyricsfile`.

## What ships

- **"Find lyrics…" in the ♪ lyrics menu** of Now Playing, and a **"Find lyrics"** row with a
  **Find…** button in Settings › Lyrics. Both open the finder for the playing track, and both are
  enabled when lyrics and "Look up lyrics online" are on and a track is loaded.
- **The finder lists lrclib.net's matches, best first.** A row shows title, artist · album,
  duration and a badge: Word-timed, Synced, Plain or Instrumental. The dialog's message states the
  track's own duration beside them.
- **The search field opens prefilled** with the track's title and artist. Enter searches again with
  whatever was typed.
- **Picking a row previews its first lines** under the list.
- **Apply stores the pick as the track's online answer**, replacing the automatic one, and the
  Now Playing panel reloads with it.
- **A pick the track's own lyrics would hide is named as such** in the dialog: a `.lrc` beside the
  track, timed lyrics in its tag, or a plain tag against a plain pick. Apply still stores it.
- **lrclib records with word timings light word by word**, from automatic lookups as well.

**What keeps working:**

- Find Headphones looks and behaves as today.
- Automatic lookups run only when nothing is stored, so a pick stays until "Look up again" (the
  reset to automatic), a retag renaming the recording, or the store's pruning.
- With "Look up lyrics online" off, nothing reaches lrclib: both entries are disabled and the
  facade refuses.
- "Save to file" writes a picked sheet into the tag as it does any online answer.

---

## Decisions

### Where a pick lands: the store

- The pick replaces the track's stored online answer (`store::forget`, then `store::write`), the
  same files an automatic answer uses. No audio file is touched.
- A `.lrc` beside the track and timed lyrics in its tag still come first, and a plain tag beats a
  plain pick (`library/lyrics/mod.rs`, `resolve`). Writing the pick there is the user's move:
  "Save to file", or a `.lrc` placed beside the song.
- The dialog says when that will hide the pick, so Apply never seems to do nothing.

### Consent: the online switch

- Both entries need "Look up lyrics online" on, as Find Headphones needs its online switch
  (`views/settings/headphones-section.slint:73`). The switch's description names lrclib.net, and
  the README promises nothing contacts it until the switch is on (`README.md:166`).
- The facade's `search` reads the switch through the one `online_lookup_enabled`, so
  `lyrics_switch.rs`' single-reader pin holds.
- A request leaves only on open and on Enter, never per keystroke, through the lyrics pacer and its
  `Retry-After` stop.

### The request: `/api/search?q=`

- One shape for both the prefilled and the typed query: `q` matches the title, artist and album
  fields (lrclib's reference, Search). The prefill is `search_terms`
  (`lyrics_directory/recording.rs`), the terms the automatic search asks with.
- v1, not the v2 beta: v1 rows carry the lyrics, so the preview and Apply need no second request,
  and v2's shape may still change.
- Rows are kept with plain and instrumental ones, unlike `pick_timed`'s filter.
- Ranked this recording's first (`Recording::matches`), then by whole seconds off, then by kind
  (words, lines, plain, instrumental), a tie keeping lrclib's order.
- Deduplicated on the converted text: lrclib files one upload under every release, and each copy's
  Lyricsfile names its album, so the raw document differs where the sheet doesn't. Twenty rows for
  one song came down to three sheets.

### Word timings: Lyricsfile to enhanced LRC, inside the client

- lrclib marks a record `hasWordSync` when its `lyricsfile` YAML holds `lines[].words`. A line has
  `text`, `start_ms`, `end_ms` and optional `words`; a word has `text`, with its trailing space, and
  `start_ms`.
- The client converts it to the enhanced LRC Melodia already parses: `[start]`, then
  `<start>word` per word, then `<end>` after the last. An empty line becomes a blank stamp; a line
  without words stays line-timed.
- A document that doesn't parse falls back to `syncedLyrics`, and Melodia's span check still lights
  a line whole when its times don't add up.
- The conversion lives in `lyrics_directory`, so callers still get a `LyricsAnswer` and never the
  wire shape (`lyrics_directory/mod.rs`' `//!`).
- Word times outrank a gloss: between uploads the same second off, a word-timed one wins, then a
  glossed one. A signature answer with word times settles the lookup, and the index's row replaces
  a timed one only by bringing word times or a gloss.
- `serde-saphyr` 2.2.0, the latest stable release (3.0.0-rc.1 is a release candidate), with
  `default-features = false` and `deserialize` only. `serde_yaml` and `serde_yml` are both
  published as deprecated on crates.io.

### The shared list

- Slint has no generic components, so the list is shared through one row struct and a component
  with inputs and callbacks; each feature keeps its own global and strings.
- `PickRow { index, title, subtitle, trailing, badge }`. `badge` indexes a `[string]` the host
  passes as an inline `[@tr(…)]`, the idiom for labels Rust orders. A pure `subtitle-for(PickRow)`
  callback lets AutoEq keep "Measured by {}" with its translator note.
- AutoEq narrows a list it holds, while the finder asks the network, so `PickList` raises both
  `filter-changed` and `filter-accepted` and each host wires what it needs.

---

## Prior art

Read for shape; rox is AGPL-3.0-only, sonora, Strawberry and fooyin GPL-3.0-or-later, so nothing is
copied.

- **Entry points.** Strawberry, fooyin and rox open a per-track search from the lyrics view or the
  track menu (`strawberry/src/songeditor/lyricssearchdialog.cpp`,
  `fooyin/src/plugins/lyrics/lyricssearchdialog.cpp`, `rox/crates/rox/src/lyrics/matcher.rs`).
  All three prefill an editable query and preview the pick. sonora has no picker.
- **Request shape.** Strawberry and fooyin ask `/api/get`, which answers with one row
  (`strawberry/src/lyrics/lrcliblyricsprovider.cpp:47`,
  `fooyin/src/plugins/lyrics/sources/lrcliblyrics.cpp:38`).
  Not copied: the finder searches, as rox does (`rox-net/src/providers/lrclib.rs:1-26`).
- **Rows.** Only rox shows each candidate's duration beside the track's own
  (`matcher.rs:298-378`, `:500-507`). fooyin's No/Line/Word column
  (`lyricssearchmodel.cpp:142-156`) is the badge's shape. Not copied: fooyin's default sort by
  title and Strawberry's rows without a duration.
- **A pick masked by local lyrics.** rox saves to its store while a sidecar wins the load, so the
  pick seems to do nothing (`matcher.rs:248`, `rox-library/src/lyrics.rs:117-142`). Here the dialog
  says so.
- **Lyricsfile.** sonora reads lrclib's `lyricsfile` through `serde_saphyr`
  (`sonora/crates/music/src/lrclib/mod.rs:106-184`).
- **Duplicates.** sonora dedupes by a text fingerprint (`music/src/lyrics/mod.rs:235-274`).
- **Not copied:** lookups on by default (Strawberry, sonora) and sonora's prefetch of tracks not yet
  played (`state/src/lyrics.rs:364-381`).

## What this does not do

- Other lyrics services. lrclib stays the one directory.
- Writing a pick to a `.lrc` beside the track. Nothing in Melodia writes one, and it would outrank
  everything.
- A search from the track context menu or from Edit Tags.

---

## Structure

| File | Owns, after this | Lines today |
|---|---|---|
| `melodia-ui/ui/components/dialog/pick-list.slint` (new) | `PickList`: search bar, the well with `ListView` and `OverlayScrollbar`, empty state, failure line, the single-pick row | |
| `melodia-ui/ui/models.slint` | `PickRow`, replacing `CorrectionPickRow` | 530 |
| `melodia-ui/ui/components/dialog/headphone-picker-body.slint` | `HeadphonePicker` bound onto `PickList`, with AutoEq's strings | 161 |
| `melodia-ui/ui/components/dialog/lyrics-finder-body.slint` (new) | `PickList`, the preview, the hidden-pick note, the badge labels | |
| `melodia-ui/ui/globals/lyrics-finder.slint` (new) | `LyricsFinder` and its phase and masking enums | |
| `melodia-ui/ui/globals/dialog.slint` | `DialogKind.lyrics-finder`, its routing and teardown, `open-lyrics-finder()` | 465 |
| `melodia-ui/ui/components/dialog/dialog.slint` | the kind's width, scroller, body mount and confirm gate | 316 |
| `melodia-ui/ui/components/now-playing/lyrics-menu.slint` | the "Find lyrics…" row | 137 |
| `melodia-ui/ui/views/settings/lyrics-section.slint` | the "Find lyrics" row | 82 |
| `melodia-net/src/services/net/lyrics_directory/lyricsfile.rs` (new) | Lyricsfile YAML to enhanced LRC | |
| `melodia-net/src/services/net/lyrics_directory/lrclib.rs` | `lyricsfile` and `hasWordSync` read; the `q` search, ranked and deduplicated | 371 |
| `melodia-core/src/entities/lyrics.rs` | `LyricsCandidate` | |
| `melodia-app/src/library/lyrics/finder.rs` (new) | `default_query`, `search`, `choose`, `outranking` | |
| `melodia-views/src/ui/correction/picker.rs` | the AutoEq rows as `PickRow` | 203 |
| `melodia-views/src/ui/now_playing/lyrics/finder.rs` (new) | the dialog's controller on `DialogTicket` | |

Every file stays well under 800.

**Duplication collapsed first:**

- `PickList` comes out of the headphone picker before the finder mounts it (Phase 1).
- `PickRowItem` writes `HeadphonePicker` directly; `PickList` owns `picked` and each host binds it.
- The reload after Apply reuses the release and kick "Look up again" runs
  (`now_playing/lyrics/sheet.rs`).
- The ranking reuses `Recording::matches` and the duration terms `pick_timed` ranks by.
- Left alone: the kept-out, duplicates and correction-import bodies hand-roll the same well, but
  around multi-select or grouped rows, and a component wrapping `ListView` would lose its
  virtualization.

---

## Phase 1: `PickList` out of the headphone picker (no visible change)

- [x] `PickRow` replaces `CorrectionPickRow`; `picker.rs` fills `title` and `subtitle`, `badge: -1`.
- [x] `pick-list.slint`:
  - inputs: rows, `listing`, placeholder, state icon, title and body, failure text, `badges`;
  - in-out: `filter`, `picked`, `blur-tick`;
  - callbacks: `filter-changed`, `filter-accepted`, `row-picked`, pure `subtitle-for(PickRow)`.
- [x] `headphone-picker-body.slint` inherits it; every `@tr` string stays in that file, the
      translator note on its line.
- [x] Docs: `ui-patterns.md`'s component catalogue gains `PickList`.
- [x] Also: the three wells' "No `clip`" note names the duplicates finder, a lyrics finder being
      on the way.

**Gate:** fmt, clippy, the suite. `scrollbars.rs` holds the well's policies and on-card track;
`translations.rs` meets no new msgid.

## Phase 2: lrclib's word timings (`melodia-net`)

- [x] `serde-saphyr` as a workspace dependency, named by `melodia-net` only. It adds four packages,
      all MIT or Apache-2.0, so `licenses/ATTRIBUTION.txt` owes nothing.
- [x] `lyricsfile.rs`: the conversion and its fallback (Decisions).
- [x] `ApiLyrics` reads `lyricsfile` and `hasWordSync`; `into_answer` prefers the converted sheet.
      `pick_timed` and `settles_it` count word timings as the better timed answer.
- [x] Docs: `lrclib.rs`' `//!` names `lyricsfile`, and `LyricsAnswer.synced` says its words can be
      timed. `lyrics_directory`'s `//!` holds: callers still get LRC.
- [x] Also: both legs hand back the raw row and `fetch` converts the one it keeps, so ranking reads
      `hasWordSync` without parsing a sheet.

**Gate:** fmt, clippy, the suite.
**Visible:** an online answer with word timings lights word by word.

## Phase 3: Search and choose (no UI)

- [x] `LyricsCandidate { title, artist, album, duration_secs, answer }` in `melodia-core`.
- [x] `lyrics_directory::search(client, pacer, query, title, credit, duration_ms)`: one
      `/api/search?q=` through `send` (pacer, `Retry-After` stop, `read_capped` at the search cap),
      ranked and deduplicated; `query_for` for the prefill.
- [x] `library::lyrics::finder`:
  - `default_query(state, track)`;
  - `search(state, track, query)`, behind `ensure_enabled` and `online_lookup_enabled`, failing
    as `SearchFailure::RateLimited` or `Failed`; each `Choice` carries its kind and a preview;
  - `choose(state, track, candidate)`, forgetting what was stored first;
  - `outranking(state, track)`: a sidecar, a timed tag or a plain tag, from `read_local`.
- [x] Docs: `library::lyrics`' `//!`, `lyrics_directory`'s `//!` (its two calls), `lrclib.rs`'
      "at most two requests per track change" extended to a search a person asks for.
- [x] Also: `search_terms` gives the search's title-and-artist fallback one home, used by the
      automatic search and the prefill; `Fetched::of` gives an answer's store form one home, used by
      the lookup and the pick.

**Gate:** fmt, clippy, the suite.

## Phase 4: The finder and its two entries (`melodia-ui`, `melodia-views`)

- [ ] `LyricsFinder`: phase (loading, listing, rate-limited, failed, as `SearchFailure` tells
      them apart), query, rows, picked, preview, masking, applying; `load`, `search`,
      `picked-changed`, `apply`, `released`.
- [ ] `lyrics-finder-body.slint`; `DialogKind.lyrics-finder` at `max-w`, `body-scrolls`, the body
      mount, the confirm gate (listing, a pick, not applying), `accepted` and `closed-teardown`.
- [ ] `Dialog.open-lyrics-finder()`, the kind having two callers; `dialog_openers.rs`'
      `FOLDED_KINDS` gains it.
- [ ] `ui/now_playing/lyrics/finder.rs`:
  - `load` searches the default query and `search` the typed one, off the UI thread, under the
    ticket;
  - `picked-changed` sets the preview, stamps stripped, and the masking for that row's kind;
  - `apply` chooses, closes the dialog, and releases and kicks the panel when it is mounted.
- [ ] The lyrics menu's row, enabled on `Lyrics.online-enabled && Player.vm.has_track`, never
      behind an `if`; `menu-h` becomes `* 6`, the count `lyrics_switch.rs` derives from the rows.
- [ ] Settings › Lyrics' row with a "Find…" button (an existing msgid), enabled under the same
      conditions plus `Settings.lyrics-enabled`.
- [ ] Strings: `scripts/update-translations.py`, then every catalog filled, no empty `msgstr` and
      no `#, fuzzy`. "Apply", "Cancel" and "Find…" are reused.
- [ ] Docs: `README.md`'s lyrics lines; `lyrics-section.slint`'s header note.

**Gate:** fmt, clippy, the suite: `dialog_openers.rs`, `lyrics_switch.rs`, `scrollbars.rs`,
`translations.rs`.

## Phase 5: Tests

### Hand tests

Every phase's hand check gathers here and runs last: after Phase 4, before any test is written, in
the dev data folder, each against `main` where it should match.

- [ ] Phase 1, Find Headphones as on `main`: rows, filter, pick, the loading and failure states,
      Apply; a model shows no trailing text and no badge.
- [ ] Phase 2, a word-synced lrclib record lighting word by word from an automatic lookup; a
      malformed `lyricsfile` falling back to `syncedLyrics`.
- [ ] Phases 3 and 4, the finder:
  - from the menu and from Settings on a real track; Enter with an edited query;
  - a pick applied and shown; "Look up again" back to automatic; the pick surviving a restart;
  - a pick hidden by a `.lrc` and by a timed tag, the note shown; a plain pick under a plain tag;
  - online lookup off: both entries greyed, nothing sent;
  - a rate-limited answer; an unreachable lrclib;
  - a word-synced record from the finder, badged Word-timed;
  - long titles, Arabic and Korean rows, the preview of a long sheet.

### Tests

- [ ] On a go naming them, after the hand tests.

---

## Cross-cutting

- Every phase: `cargo fmt --all`, `cargo clippy --all-targets --locked --workspace -- -D warnings`,
  `cargo test --locked --workspace`, then `code-style.md`'s comment audit.
- Memory: the finder's rows and candidates live only while the dialog is open, as the AutoEq list
  does; no RSS check owed.
- No ADR: the store keeps ADR 38's order, and the reasons live at their anchors.

## Open questions

None blocking.
