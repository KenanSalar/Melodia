# Lyrics finder: pick lyrics by hand, on a list shared with Find Headphones

Working doc. Delete when the feature ships.

Status: **Phase 6's hand tests pass; tests wait for a go naming them** · Created: 2026-11-21 · Branch `feat/enhanced-lrc`, after
[ENHANCED_LRC.md](ENHANCED_LRC.md)'s Phase 5

> Researched **2026-11-21** against `feat/enhanced-lrc` at `8db89fc3`; lrclib's API reference
> (`lrclib.net/docs.txt`) and live `/api/search` answers; `serde-saphyr`'s published versions; and
> the Strawberry, fooyin, rox and sonora checkouts (no termusic clone is on disk). Phase 5
> researched the same day at `c6805f10`, against the same checkouts.

---

## What you see today

- **A wrong or missing online answer has one recourse**, "Look up again", which asks lrclib the
  same question and stores the same answer. lrclib's other candidates are never shown.
- **Settings › Lyrics holds three switches and no action.**
- **"Look up lyrics online" is both the consent and the automatic lookup**: one switch decides
  whether lrclib may be reached at all and whether it is asked on every track change.
- **An lrclib record with word timings lights line by line**: the client reads `syncedLyrics` and
  ignores the record's `lyricsfile`.

## What ships

- **"Find lyrics…" in the ♪ lyrics menu** of Now Playing, and a **"Find lyrics"** row with a
  **Find…** button in Settings › Lyrics. Both open the finder for the playing track, and both are
  enabled once a track is loaded, the Settings row also needing lyrics on and saying so while
  greyed. With "Look up lyrics online" off, the dialog opens on a notice whose **Turn On** is that
  switch.
- **The card eases between steps**, from Searching to the list and back on Enter, as the
  duplicates finder's does.
- **The finder lists lrclib.net's matches, best first.** A row shows title, artist · album,
  duration and a badge: Word-timed, Synced, Plain or Instrumental. The dialog's message states the
  track's own duration beside them.
- **The search field opens prefilled** with the track's title and artist. Enter searches again with
  whatever was typed.
- **Picking a row shows its whole sheet** under the list, scrolling inside a box the layout sizes.
- **Apply stores the pick as the track's online answer**, replacing the automatic one, and the
  Now Playing panel reloads with it.
- **A pick the track's own lyrics would hide is named as such** in the dialog: a `.lrc` beside the
  track, timed lyrics in its tag, or a plain tag against a plain or instrumental pick. Apply still
  stores it.
- **lrclib records with word timings light word by word**, from automatic lookups as well.
- **"Look up automatically"**, on by default, under "Look up lyrics online" in Settings › Lyrics
  and in the ♪ menu in "Look up again"'s place. Off, a track change sends nothing: picks and
  stored answers still show, and a track with neither reads "No lyrics found" / "Find them in the
  Lyrics menu". Turned on, it looks the playing track up. With the lookup off it greys in both
  places, Settings reading "Turn on "Look up lyrics online" above first".
- **"Look up lyrics online" says what it gates**: "Let Melodia ask lrclib.net for lyrics", on the
  Settings and welcome cards alike. Its old sentence moves to the automatic row.
- **"Couldn't reach the lyrics service" hints "Find them in the Lyrics menu"** too.

**What keeps working:**

- Find Headphones looks and behaves as today.
- Every install's lookups on upgrade: a `settings.json` without the automatic switch reads it on.
- Automatic lookups run only when nothing is stored, so a pick stays until the next pick, a retag
  renaming the recording, or the store's pruning.
- With "Look up lyrics online" off, nothing reaches lrclib until the person turns it on, from the
  dialog's notice or the switch itself, and the facade refuses until then.
- "Save to file" writes a picked sheet into the tag as it does any online answer.

---

## Decisions

### Where a pick lands: the store

- The pick replaces the track's stored online answer (`store::forget`, then `store::write`), the
  same files an automatic answer uses. No audio file is touched.
- A `.lrc` beside the track and timed lyrics in its tag still come first, and a plain tag beats a
  plain or instrumental pick (`library/lyrics/mod.rs`, `resolve` and `timed_first`). Writing the
  pick there is the user's move: "Save to file", or a `.lrc` placed beside the song.
- The dialog says when that will hide the pick, so Apply never seems to do nothing. The rule has
  one home, `finder::Outranking::hides`.

### Consent: the online switch

- The switch's description names lrclib.net, and the README promises nothing contacts it until
  the switch is on (`README.md:166`). Both entries open with it off, the dialog asking first: its
  **Turn On** persists the switch and searches, and the panel looks again only once a pick is
  stored or the dialog closes, so its automatic answer can't land over the pick.
- The facade's `search` reads the switch through the one `online_lookup_enabled`, so
  `lyrics_switch.rs`' single-reader pin holds.
- A request leaves only on open and on Enter, never per keystroke, through the lyrics pacer and its
  `Retry-After` stop.
- Turn On grants access only. Whether later tracks are looked up is the automatic switch's, and
  the held re-ask matters only while that one is on.

### Automatic lookup: a switch under the online one (Phase 5)

- Two booleans rather than one Off / When I search / Automatically picker: the welcome card and the
  menu are toggles, and the consent keeps its one reader.
- `lyrics_auto_lookup` ships on, so an upgrade keeps today's lookups. A default that told upgrades
  from new installs apart would need a three-state field.
- `auto_lookup_enabled` is its one reader, in `resolve` beside `online_lookup_enabled`. The
  finder's `search` doesn't read it, a search being asked for.
- Greyed while the lookup is off, as the Find and scrobble-radio rows grey, Settings naming the
  switch it waits on. Left live, it read "on" while doing nothing, and it bought no manual-only
  start: Settings closes Now Playing, so turning the lookup on and then automatic off sends nothing
  between the two.
- "Look up again" goes. A wrong match, nothing found and an outage are each a search in the
  finder, which lists the answer the lookup would store beside the rest. `library::lyrics::forget`
  goes with it, `forget_all` staying for a retag.
- The empty panel's hints name the menu, as the "off" state's does, rather than holding a button:
  the panel is mounted behind an `if` and may hold no `animate` or `changed` (`lyrics_switch.rs`),
  and `section-button.slint` and `icon-button.slint` both animate.

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
- The finder binds `listing: true`: its box holds the query, so it stays up while lrclib answers or
  refuses, and a search empties the rows, which raises the state block. `PickList` takes no input
  of its own for it.
- The enums are `LyricsSearchPhase` and `LyricsMasking`, `dialog.slint` already saying "finder"
  for the duplicates finder. `loading` is the opener's and `searching` each search Rust sends, so
  the card opens at rest on the default state and every later step, a second Enter's included,
  has a state of its own to ease into.
- Never hidden: the menu reserves a row for each entry (`lyrics_switch.rs`), and a hidden command
  can't be found. With the lookup off both entries open and the dialog explains, its footer
  reading **Turn On** in a `lookup-off` step with no state of its own, so the card opens at rest
  on it. The Settings row greys only with lyrics off, as the radio scrobble row does, naming the
  switch.
- The preview is the whole sheet in a `DialogScrollPane`, scrolled back to its first line on each
  pick. Its box stretches beside the list from a short minimum, so a short window shrinks both,
  and one too short for the floors cuts the body above the footer rather than pushing it off.

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
- **Automatic versus by hand.** fooyin and rox keep reaching a service apart from asking it on
  every track, and their manual search ignores the automatic switch
  (`fooyin/src/plugins/lyrics/settings/lyricssearchingpage.cpp:66,93`,
  `lyricsplaybackservice.cpp:202-208`; `rox/crates/rox/src/settings/window/providers_page.rs:1-14`,
  `rox-panels/src/lyrics.rs:184-186`). Strawberry's automatic switch sits in its lyrics view's
  menu (`strawberry/src/context/contextview.cpp:362-369`), as the ♪ menu's does.
- **Not copied:** lookups on by default (Strawberry, sonora) and sonora's prefetch of tracks not yet
  played (`state/src/lyrics.rs:364-381`); fooyin's and rox's automatic search off by default, an
  upgrade here keeping its lookups; rox's "Search Online" button in the empty view
  (`rox-panels/src/lyrics.rs:1436-1452`).

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
| `melodia-ui/ui/components/dialog/lyrics-finder-body.slint` (new) | `PickList`, the preview, the hidden-pick note, the badge labels, the cut below its floors | 86 |
| `melodia-ui/ui/globals/lyrics-finder.slint` (new) | `LyricsFinder`, `LyricsSearchPhase` and `LyricsMasking` | 32 |
| `melodia-ui/ui/globals/dialog.slint` | `DialogKind.lyrics-finder`, its routing and teardown, `open-lyrics-finder()` | 490 |
| `melodia-ui/ui/components/dialog/dialog.slint` | the kind's width, scroller, body mount and confirm gate | 326 |
| `melodia-ui/ui/components/now-playing/lyrics-menu.slint` | the "Find lyrics…" row; "Look up automatically" in "Look up again"'s place | 148 |
| `melodia-ui/ui/views/settings/lyrics-section.slint` | the "Find lyrics" row; the automatic row; the lookup's description | 104 |
| `melodia-ui/ui/components/onboarding/features-panel.slint` | the lookup's description | 90 |
| `melodia-ui/ui/components/now-playing/lyrics-panel.slint` | the `missing` and `unavailable` hints | 298 |
| `melodia-ui/ui/globals/lyrics.slint` | `auto-lookup` and `set-auto-lookup`; `refresh()` gone | 102 |
| `melodia-ui/ui/settings.slint` | `lyrics-auto-lookup` and its callback | 625 |
| `melodia-net/src/services/net/lyrics_directory/lyricsfile.rs` (new) | Lyricsfile YAML to enhanced LRC | |
| `melodia-net/src/services/net/lyrics_directory/lrclib.rs` | `lyricsfile` and `hasWordSync` read; the `q` search, ranked and deduplicated | 371 |
| `melodia-core/src/entities/lyrics.rs` | `LyricsCandidate` | |
| `melodia-app/src/library/lyrics/finder.rs` (new) | `default_query`, `search`, `choose`, `outranking` | |
| `melodia-app/src/services/settings/services.rs` | `LyricsFlags.lyrics_auto_lookup`, on by default | 66 |
| `melodia-app/src/state/mod.rs` | `lyrics_auto_lookup: SharedFlag` | 398 |
| `melodia-app/src/library/settings/lyrics.rs` | `set_lyrics_auto_lookup` | 31 |
| `melodia-app/src/library/lyrics/mod.rs` | `auto_lookup_enabled`, read in `resolve`; `forget` gone | 250 |
| `melodia-views/src/ui/correction/picker.rs` | the AutoEq rows as `PickRow` | 203 |
| `melodia-views/src/ui/now_playing/lyrics/finder.rs` (new) | the dialog's controller on `DialogTicket` | 267 |
| `melodia-views/src/ui/settings/lyrics_settings.rs` | `auto_lookup_handler` over `mirrored` | 107 |
| `melodia-views/src/ui/now_playing/lyrics/mod.rs` | the menu's automatic switch, on the online switch's re-ask | 334 |
| `melodia-views/src/ui/now_playing/lyrics/sheet.rs` | `on_refresh` gone | 273 |

Every file stays well under 800.

**Duplication collapsed first:**

- `PickList` comes out of the headphone picker before the finder mounts it (Phase 1).
- `PickRowItem` writes `HeadphonePicker` directly; `PickList` owns `picked` and each host binds it.
- The reload after Apply reuses the panel's `release` and `kick` (`now_playing/lyrics/sheet.rs`).
- Phase 5: the online switch's re-ask (release and kick when turned on or over an empty panel)
  becomes one fn the automatic switch calls too.
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
  - `outranking(state, track)`: a sidecar, a timed tag or a plain tag, from `read_local`, behind
    `ensure_enabled`; `Outranking::hides(kind)` says whether it hides a pick of that kind.
- [x] Docs: `library::lyrics`' `//!`, `lyrics_directory`'s `//!` (its two calls), `lrclib.rs`'
      "at most two requests per track change" extended to a search a person asks for.
- [x] Also: `search_terms` gives the search's title-and-artist fallback one home, used by the
      automatic search and the prefill; `Fetched::of` gives an answer's store form one home, used by
      the lookup and the pick.

**Gate:** fmt, clippy, the suite.

## Phase 4: The finder and its two entries (`melodia-ui`, `melodia-views`)

- [x] `LyricsFinder`: phase (loading, listing, rate-limited, failed, as `SearchFailure` tells
      them apart), query, rows, picked, preview, masking, applying, apply-failed; `load`,
      `search`, `picked-changed`, `apply`, `released`.
- [x] `lyrics-finder-body.slint`; `DialogKind.lyrics-finder` at `max-w`, `body-scrolls`, the body
      mount, the confirm gate (listing, a pick, not applying), `accepted` and `closed-teardown`.
- [x] `Dialog.open-lyrics-finder()`, the kind having two callers; `dialog_openers.rs`'
      `FOLDED_KINDS` gains it.
- [x] `ui/now_playing/lyrics/finder.rs`:
  - `load` searches the default query and `search` the typed one, off the UI thread, under the
    ticket;
  - `picked-changed` sets the preview, stamps stripped, and the masking from
    `Outranking::hides` for that row's kind;
  - `apply` chooses, closes the dialog, and releases and kicks the panel when it is mounted.
- [x] The lyrics menu's row, enabled on `Lyrics.online-enabled && Player.vm.has_track`, never
      behind an `if`; `menu-h` becomes `* 6`, the count `lyrics_switch.rs` derives from the rows.
- [x] Settings › Lyrics' row with a "Find…" button (an existing msgid), enabled under the same
      conditions plus `Settings.lyrics-enabled`.
- [x] Strings: `scripts/update-translations.py`, then every catalog filled, no empty `msgstr` and
      no `#, fuzzy`. "Apply", "Cancel", "Find…" and "Searching…" are reused.
- [x] Docs: `README.md`'s lyrics lines; `lyrics-section.slint`'s header note.
- [x] Also: `ui-patterns.md`'s catalogue names Find Lyrics beside Find Headphones; a search waits
      out a pick being stored, whose close its retired ticket would strand.
- [x] After the first hand check: the card eases between steps (`lyrics-searching`, `lyrics-list`,
      `lyrics-settled` in `dialog.slint`); with the lookup off both entries open on a notice whose
      Turn On is the switch, the panel's re-ask held to a stored pick or the close; the greyed
      Settings row names "Show lyrics"; the preview shows the whole sheet, scrolling
      (`DialogScrollPane.scroll-to-start()`), with room inside its clip for the first and last
      lines' ink.

**Gate:** fmt, clippy, the suite: `dialog_openers.rs`, `lyrics_switch.rs`, `scrollbars.rs`,
`translations.rs`.

## Phase 5: Automatic lookup as its own switch

One phase: the switch, the menu row and the panel's copy are only testable together.

- [x] `LyricsFlags.lyrics_auto_lookup`, on by default; `AppState.lyrics_auto_lookup`;
      `library::settings::set_lyrics_auto_lookup`.
- [x] `library::lyrics::auto_lookup_enabled` in `resolve`, beside `online_lookup_enabled`;
      `forget` deleted.
- [x] `lyrics_settings.rs`' `auto_lookup_handler`; the menu's `set-auto-lookup` through
      `with_panel_reask`, which the online switch's re-ask became; `on_refresh` deleted.
- [x] Slint:
  - `Lyrics.auto-lookup` and `set-auto-lookup`, `refresh()` deleted; `Settings.lyrics-auto-lookup`;
  - the Settings row under the lookup's, its label and description declared once;
  - the menu row (`auto_awesome`, already subset) in "Look up again"'s place, `menu-h` still `* 6`;
  - the panel's `missing` and `unavailable` hints, under the existing `// Translators:` line;
  - the lookup's description on both cards.
- [x] Strings: `scripts/update-translations.py`, then every catalog filled, no empty `msgstr` and
      no `#, fuzzy`. New: "Let Melodia ask lrclib.net for lyrics", "Look up automatically", "Find
      them in the Lyrics menu". Gone: "Look up again", "Try again from the Lyrics menu". "When a
      track carries no words of its own, ask lrclib.net for them" moves to the new row, its
      translations kept.
- [x] `lyrics_switch.rs`' description pin follows the lookup's new sentence.
- [x] Docs: `README.md`'s lyrics block; the `LyricsFlags`, `library::lyrics`,
      `library::settings::lyrics`, `lyrics_settings.rs` and `finder.rs` docs; `lyrics.slint`'s and
      `lyrics-section.slint`'s header notes; the `Unavailable` claim's comment in `sheet.rs`.
- [x] After the first hand check: the automatic row greys while the lookup is off, in Settings
      reading "Turn on "Look up lyrics online" above first" (every catalog filled) and in the
      menu through `button-enabled`; the menu's three switches leave it open, its Find lyrics…,
      Save to file and Edit lyrics… closing it (`lyrics-menu.slint`'s header).

**Gate:** fmt, clippy, the suite: `lyrics_switch.rs`, `translations.rs`.

## Phase 6: Tests

### Hand tests

Every phase's hand check gathers here and runs last: after Phase 5, before any test is written, in
the dev data folder, each against `main` where it should match.

- [x] Phase 1, Find Headphones as on `main`: rows, filter, pick, the loading and failure states,
      Apply; a model shows no trailing text and no badge.
- [x] Phase 2, a word-synced lrclib record lighting word by word from an automatic lookup; a
      malformed `lyricsfile` falling back to `syncedLyrics`.
- [x] Phases 3 and 4, the finder:
  - from the menu and from Settings on a real track; Enter with an edited query;
  - a pick applied and shown; the pick surviving a restart;
  - a pick hidden by a `.lrc` and by a timed tag, the note shown; a plain pick under a plain tag;
  - Enter in the empty box the moment the dialog opens, a hidden pick still showing the note;
  - online lookup off: both entries open on the notice, nothing sent before Turn On; Turn On
    flipping the switch in the menu and in Settings, then searching; the Settings row greyed and
    naming "Show lyrics" while lyrics are off;
  - Turn On, then a pick applied before the panel would have answered, the pick kept on screen
    and after a restart; Turn On, then Cancel, the panel looking up on its own;
  - the card easing from Searching to the list, and down and back up on an Enter over a list;
  - a rate-limited answer; an unreachable lrclib;
  - a word-synced record from the finder, badged Word-timed;
  - a song ending under the dialog, the pick landing on the track it opened on;
  - long titles, Arabic and Korean rows; a long sheet scrolling in the preview, the next pick
    reading from its first line; an Arabic sheet's and the `vi` placeholder's first and last
    lines whole, at rest and scrolled to the end;
  - the shortest window reachable, a pick showing, the footer inside the card.
- [x] After the hand run: `LyricsFinderBody` cuts its column on a window too short for its floors,
      so `ModalCard.height-cap` no longer pushes the footer off the card.
- [x] Phase 5, the automatic switch:
  - an upgraded `settings.json` with the lookup on and no automatic key: lookups run as before;
  - lookup on, automatic off: a track with no lyrics of its own sends nothing at
    `RUST_LOG=debug`, and the panel reads "No lyrics found" with its hint; a stored answer still
    shows; a pick shows and survives a restart;
  - automatic turned on from the menu: the playing track looked up at once;
  - lookup and automatic off, Turn On: one search, later tracks sending nothing; lookup off and
    automatic on, Turn On: the held re-ask as before;
  - the Settings row and the menu row in step both ways; both greyed with the lookup off, their
    state kept and shown dimmed, Settings naming the switch;
    Settings search finding it by label and by description;
  - an unreachable lrclib: the new hint, the next track asking again;
  - the welcome card's new description; the menu's six rows, none clipped;
  - the menu kept open across each switch, the automatic row greying and returning as the lookup
    flips beside it, the romanization row as the panel reloads; a click outside and the window
    losing focus still closing it; Find lyrics…, Save to file and Edit lyrics… closing it.
- [x] The follow clock (`now_playing/lyrics/clock.rs`, found by hand 2026-11-21: words lit ahead,
      then went dark), on a word-timed sheet in Now Playing and the miniplayer:
  - launch and play; paused, click a line, then play: no word lights and goes dark again;
  - playing, click lines up and down the sheet, the one just sung included; a volume change
    mid-line; pause and resume mid-word: nothing steps back;
  - the slider several seconds back snaps the words back, and a seek back under `DRIFT_SLACK_MS`
    holds them that long instead (the trade); a lyric click always jumps;
  - the first of two lines under one stamp, clicked, lights from the live position as before;
    next track, Next twice inside a track's first second, and repeat-one start clean.

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
