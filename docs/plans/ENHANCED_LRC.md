# Enhanced LRC: lyrics lit word by word

Working doc. Delete when the feature ships.

Status: **Phase 2 landed; Phase 3 next** · Created: 2026-11-20 · Branch
`feat/enhanced-lrc`, linked to #178 (#177 still to link from its Development sidebar)

Issue [#178](https://github.com/KenanSalar/Melodia/issues/178), from the request in
[#177](https://github.com/KenanSalar/Melodia/issues/177).

> Verified **2026-11-20** against `main` at `ccce7ace` (#180); lofty 0.25.5 and Slint 1.19.0 in
> the registry sources (`i-slint-compiler`, `i-slint-core`, `i-slint-common`,
> `i-slint-renderer-femtovg`, parley 0.12.0); and the rox, sonora, Strawberry and fooyin checkouts
> (no termusic clone is on disk).

---

## What you see today

- **A line with word times lights all at once.** `[00:12.50] <00:12.50> Hello <00:12.80> world`
  is one bold row, "Hello  world", from 12.50 until the next line, two spaces between the words.
  The same in Now Playing and in the miniplayer's column.
- **An M4A tagged by iTunes or Mp3tag shows its whole sheet in one row** ending in "…", plain
  lyrics included. A timed one opening with `[ti:]` shows as that same one plain row.
- **A file holding more than one lyrics entry shows whichever comes first**, so plain lyrics
  ahead of a timed copy never follow the song. Edit Tags' Lyrics tab opens on that same entry.

## What ships

- On a line with word times, each word (or syllable) lights when its time comes and the rest of
  the line stays dim until then.
- The same from a `.lrc` beside the track or from its lyrics tag (`USLT` in an MP3, `LYRICS` in
  FLAC and Ogg, `©lyr` in an M4A). An online answer carrying word times lights too, the parser
  not knowing its source.
- Every lyrics entry in a file is read and the best shown: word times over line times over plain
  text. Edit Tags' Lyrics tab opens on that same entry.
- A chorus written once under several line stamps lights its words on every repeat.
- `[offset:]` moves word times with line times.
- A line whose word times don't add up still shows, lit whole as today.
- A word-timed line ending in one more stamp after its last word ends there, so a long break after
  it shows the notes from that stamp (Decisions, the end stamp).
- Lone carriage returns split lines, and words show one space apart.
- The miniplayer's column gets all of it: it mounts the same panel.

**What keeps working:**

- Line-timed and plain lyrics look and behave exactly as today.
- The translation and romanization under a lyric, clicking a line to seek, the panel following the
  song, the notes in a long break.
- Save to tag and Edit Tags write the text as it stands, word times included.
- A `.lrc` beside the track still beats the tag, and a timed tag still beats the online lookup.

**Also fixed here** (found validating the issue, Phase 3): Save to tag replaces every lyrics entry
in every format, where an M4A or a FLAC can keep an old one today.

---

## Validating the issue

- **Holds: word times are thrown away.** `strip_word_stamps` (`lrc.rs:176-197`) runs on stamped
  lines only (`lrc.rs:41`). `LyricLine` has no field for them (`entities/lyrics.rs:11-18`), nor
  does Slint's `LyricRow` (`models.slint:517-530`).
- **Holds: the double space.** A stamp taken out keeps the space before it (`rest[..open]`,
  `lrc.rs:192`) and the one after it (the next `rest`, `lrc.rs:193`); `trim()` reaches only the
  ends (`lrc.rs:42`). `lrc_tests.rs`' word-stamp test writes no space after either stamp, so it
  passes.
- **Holds, and wider: the lone carriage return.** Every split is `str::lines` (`lrc.rs:26`, `:93`,
  `:104`; `entities/lyrics.rs:126`), which breaks on `\n` and `\r\n` only. A CR-only file is one
  line: only its leading stamps are read, `[offset:]` is lost (`bracketed` wants a whole line,
  `lrc.rs:113-115`), and one opening with `[ti:]` reads as untimed.
- **Holds: only the first entry is read.** `lyrics_from` (`tag_writer.rs:475-479`) takes
  `Tag::get_string`, which is `items.iter().find` (`lofty tag/mod.rs:328-339`). lofty keeps every
  entry in the generic tag: each `USLT` differing in language or description
  (`id3/v2/tag.rs:800-829`), each Vorbis `LYRICS`, each `©lyr`, so `Tag::get_strings`
  (`tag/mod.rs:500-508`) reaches them all. Edit Tags reads through the same `lyrics_from`
  (`tag_writer.rs:464-473`).
- **Partly holds: "saving replaces every lyrics entry".** `Set` writes one key, and lofty's insert
  drops only items of that key (`tag/mod.rs:381-384`). An MP3 loses every `USLT`, all read as
  `UnsyncLyrics`. A FLAC keeps its `UNSYNCEDLYRICS`. An M4A's `©lyr` reads back as `Lyrics` (the
  map's first key, `tag/item.rs:343`, `:70`) while `Set` writes `UnsyncLyrics`, so the old atom
  likely survives and, being first, is what the next read shows. Inferred from lofty's source,
  unconfirmed: the hand tests check it on `main`. Phase 3's fix doesn't wait on it.
- **Narrows the design: "anything Markdown would treat as formatting has to be escaped, and a line
  that still can't be read falls back".** `@markdown("…\{expr}…")` hands a string through
  `StringToStyledText` (`i-slint-compiler passes/resolving.rs:1493-1506`), which is
  `StyledText::from_plain_text` and never parsed (`i-slint-core styled_text.rs:267-277`). Only the
  template is Markdown, and the `.slint` build checks it. So the lyric needs no escaping and every
  line draws: no escaper and no fallback. A span around `\{expr}` still covers what it splices, its
  range read off the text's length at each tag (`i-slint-common styled_text.rs:497`, `:526`).
- **Narrows the design further: one layer, not two.** The issue stacks two copies so the colours
  stay the theme's. A colour can be interpolated into the markup instead: `<font color="\{expr}">`
  takes an expression converted to `color` (`resolving.rs:1481-1491`), and a brush converts
  (`langtype.rs:311`), warned only for a gradient literal (`expression_tree.rs:1112-1125`). The
  binding reads the brush each time it is evaluated, so a theme change and the miniplayer's eased
  ink still reach it. One `StyledText` draws the sung row: half the glyphs of two layers, nothing
  to keep aligned, and no wrapper element.
- **Holds: no font weight.** `StyledText`'s font request passes none (`items/text.rs:513-524`);
  `**…**` sets bold (`textlayout/sharedparley/shaping.rs:193-198`).
- **Holds: it clips rather than elides.** `overflow()` is fixed at `Clip` and `wrap()` at word wrap
  (`items/text.rs:555-561`).
- **Holds: a colour span moves no glyph.** It changes only the brush, and parley breaks a shaping
  run only on size, locale, variations, features or spacing (`parley shape/mod.rs:151-160`). So a
  syllable stamp inside an Arabic word keeps its joining.
- **New: a transparent text colour returns before any shaping** (`textlayout/sharedparley.rs:234-238`,
  ahead of the layout at `:265`). That is how the sung row's `Text` yields to the `StyledText`:
  `visible` would add a `Clip` element to every row (`passes/visible.rs:92-98`), and an `if` a
  sub-component.
- **New: the `if` stays out of the row's size only by pinning `x` and `y`.** At edition 1.19
  (`melodia-ui/build.rs:17`) an `if` child folds into a plain parent's layout info
  (`passes/default_geometry.rs:252-265`) unless it binds either (`:205-206`). Every part of the row
  pins both (`lyrics-panel.slint:88-92`, the interlude's `if` at `:139-143`). Unpinned, a
  `StyledText` whose width reads the row's would fold back into the row's own.
- **Holds: a click on the sung row still seeks.** A `StyledText` accepts every press, move and
  release (`items/text.rs:406-421`), but the sheet's one `TouchArea` lies over the scroller
  (`lyrics-panel.slint:172-195`) and takes them first. The wheel it rejects, `StyledText` ignores.
- **The cost of lighting a word.** FemtoVG redraws the whole window each frame (no partial
  rendering, `i-slint-renderer-femtovg lib.rs:227-235`), so each word lit is one frame. Shaped
  text is cached per item and reshaped only when something shaping read changes, the text and font
  among it (`textlayout/sharedparley/cache.rs:220-227`), so that frame reshapes the sung line alone.
  A property set to the value it holds marks nothing dirty (`properties.rs:1057-1068`), so the
  33 ms tick repaints nothing between words. Its magnitude against a line-timed sheet is unmeasured:
  Phase 4 measures it.
- **Holds: everything it uses is stable in 1.19.0.** `StyledText` is ungated
  (`builtin_elements.rs:1263-1365`), and what 1.19 added to it is selection, which this leaves off.
- **Holds: cue sheet tracks stay out** (`library/lyrics/mod.rs:168-170`, the store alone).
- **Narrows: no new clock.** The engine reports about once a second, but `follow` interpolates
  between reports (`follow.rs:116-148`) on the panel's 33 ms `Timer` while a timed sheet plays
  (`lyrics-panel.slint:222-224`), which is the resolution a word lights at.

---

## Decisions

### Word times in the sheet

- `LyricLine` gains `words: Vec<WordStamp>`, empty for a line lit whole. A `WordStamp` is a
  word's start as a byte offset into `text` and its time in ms, `[offset:]` applied. The text
  stays the one source of what is drawn: the lit part is `text[..cut]`, cut at the first word not
  yet due.
- Text before the first word stamp lights with the line's own stamp.
- Each stamp taken out leaves one of the seam's own whitespace characters, none where it had none,
  so a syllable stamp inside a word joins it. Phase 1 lands that rule on today's stripper.
- A stamp inside a `^` gloss is dropped, as today; the gloss lights whole with its line.
- `Lyrics::timing()` answers `Plain`, `Lines` or `Words`, ordered; `is_synced` reads it.
- Parsed once per track change, off the UI thread, as today (`read_local` in `spawn_blocking`).

### A line written under several stamps

- Word times are kept relative to the stamp they were written against: the latest of the line's
  stamps at or before its first word time. Each copy then takes its own stamp plus those gaps,
  the last copy taking the list by move as it takes the text (`lrc.rs:43-51`).
- So words timed against a chorus's second occurrence light on the first too, and a one-stamp line
  whose stamp comes after its words is refused rather than shifted.

### The end stamp (the issue's open question): taken

- A stamp with nothing after it ends the last word and becomes the line's `end_ms`, the field a
  blank stamp fills today. A long break after a word-timed line shows the notes once its last word
  is sung, not at the next line.
- It costs nothing new: `rows_for` already turns `end_ms` into a break, and a line-timed sheet
  carries no such stamp, so nothing changes for one. A blank stamp still fills an `end_ms` nothing
  set.

### When the times don't add up

- A line keeps its word times only if they run in order (equal is fine), none comes before the
  line's stamp, and none, the end stamp included, comes after the sheet's next later stamp, a
  blank one included. Checked after the sort, before blanks become ends.
- Otherwise the line drops its word times and its end stamp together and lights whole.

### Every entry, the best one shown

- The store reads every entry: `get_strings` over `Lyrics`, then `UnsyncLyrics`, blanks dropped.
  `FileTags.lyrics` becomes that list.
- One chooser in `library::lyrics::embedded` parses each and keeps the best by `timing()`: the
  first in tag order on a tie, an entry nothing parses from last. The panel's read, the editor's
  resident text and the Edit Tags field all go through it, so the three agree.
- It stops at the first entry ranking `Words`, which nothing outranks and which wins its tie, so a
  file with one entry, or its timed one first, parses once. Romanization runs on the winner alone,
  after resolution (`romanized`, `library/lyrics/mod.rs:60-77`).
- The precedence above the tag is unchanged: sidecar, then a timed tag over the store, then the
  store over a plain tag.

### Save to tag replaces every entry

- `Set` clears both keys before writing one, as `Clear` does, so a save replaces every lyrics
  entry in every format, other languages included: what the issue says happens today. Without it,
  once every entry is read, a file could show the old timed entry after a new one was saved.

### Drawing: one `StyledText`, the markup in Slint

- The look: words already sung take `ink`, the colour the whole sung line takes today, and the
  words to come take the unsung rows' colour (`ink-muted.transparentize(0.2)`), so they match the
  lines around them. The whole line is bold, as the sung line is today. No accent colour and no
  glow: Slint's `Text` and `StyledText` carry no blur, shadow or glow in 1.19.0, the
  `drop-shadow-*` properties belonging to rectangles (`passes/lower_shadows.rs:58-66`).
- On the sung row of a word-timed line, one `StyledText` at the `Text`'s geometry, its `x` and `y`
  pinned as the `Text`'s are, mounted behind an `if` so only that row builds one:
  `@markdown("**\{Lyrics.lit-words}<font color=\"\{root.ink-muted.transparentize(0.2)}\">\{Lyrics.unlit-words}</font>**")`,
  `default-color: root.ink`.
- The row's `Text` goes transparent there, through the colour ternary it already has, and draws
  nothing.
- Every other row, and a sung row without word times, keeps today's `Text`.
- The clip is the open risk for tall marks: the box keeps the slack above and below the line that
  the `Text`'s has. If the Arabic and Persian hand tests crop, the `StyledText` grows over the row's
  vertical padding, centred, which moves no glyph.

### What Rust publishes, and when

- `Lyrics` gains `active-word-timed`, `lit-words` and `unlit-words`, for the sung row only: one
  global as `interlude-progress` is, so a word lighting rewrites no model row. `LyricRow` is
  unchanged.
- `write_active` finds the cut with a `partition_point` over the sung row's word stamps, on rows
  that have them, and writes the two strings only when the row or the cut moves. Slint would skip
  an equal write anyway; the guard saves allocating two strings every tick.
- `Row` carries its word stamps as a `Box<[_]>` in `i32` ms, saturating as `at_ms` does: built once
  per sheet, never grown.

### What it costs

- Per track change: one parse, off the UI thread, and one copy of each line's word stamps into
  its row.
- Per 33 ms tick: a binary search over one row's word stamps, nothing allocated.
- Per word lit: two short strings, one markup parse of a one-line template, one reshape of the
  sung line, and one window frame. Phase 4 measures that against a line-timed sheet.
- Per theme change or miniplayer ink fade: the sung row re-parses its template and reshapes its
  line each frame of the fade, the dim colour being part of its text (`default-color` is read only
  at draw).

---

## Prior art

Read for shape; rox is AGPL-3.0-only, sonora and the C++ pair GPL-3.0-or-later, so nothing is
copied.

- **Line breaks.** Strawberry splits on `\r\n`, `\n` and a lone `\r`
  (`strawberry/src/utilities/lrcutils.cpp:58-60`), the rule Phase 1 takes. rox, sonora and fooyin
  don't.
- **Words as offsets, the end stamp.** rox keeps words as byte ranges into the line
  (`rox/crates/rox-library/src/lyrics.rs:500-516`) and reads a trailing stamp as the line's end
  (`:488-496`); sonora's trailing stamp closes the last word (`sonora/crates/music/src/lyrics/lrc.rs:849-854`).
- **Repeats.** rox shifts each copy by its stamp's distance from the first (`lyrics.rs:401-416`).
  sonora re-anchors by the gap between a line's stamp and its first word (`lrc.rs:765-805`) but
  also moves a one-stamp line whose stamp is later than its words (`:791-797`): not copied, the
  reference-stamp rule refuses that line. fooyin keeps only the earliest stamp of a word-timed line
  (`fooyin/src/plugins/lyrics/lyricsparser.cpp:419-423`), losing every repeat: not copied.
- **Validation.** fooyin stable-sorts words by time (`lyricsparser.cpp:286-303`), reordering the
  text; rox and sonora check nothing. Not copied: such a line lights whole here.
- **Also not copied.** fooyin reads `.5` as 50 ms (`lyricsparser.cpp:225-228`) and drops the rest
  of a line after `<3` (`:198-215`); sonora sums every `[offset:]` (`lrc.rs:87-89`) and rewrites
  the text it shows (`normalize`, `:130-166`).
- **Entries.** fooyin and rox take the first value of each key
  (`fooyin/src/plugins/lyrics/sources/taglyrics.cpp:43-56`, `rox-library/src/writer.rs:1334-1342`);
  Strawberry shows synced tag lyrics over plain (`src/context/contextview.cpp:730-752`).
- **Drawing.** fooyin clips the current word to its glyph runs' rectangles
  (`fooyin/src/plugins/lyrics/lyricsdelegate.cpp:105-204`) and sonora sweeps a width-clipped
  overlay (`sonora/crates/views/src/chrome/aside.rs:2421-2459`): both read glyph geometry Slint
  doesn't expose, the issue's reason for leaving the sweep out. rox draws the line once with the
  tail dimmed (`rox/crates/rox-panels/src/lyrics.rs:1896-1907`), the nearest to the colour spans
  here.

## What this does not do

- A fill sweeping through each word (above).
- ID3's `SYLT` frame, read for nothing today.
- Cue sheet tracks, whose tag states the whole disc.
- Choosing among entries by language: the best by timing wins, then the first.

---

## Structure

| File | Owns, after this | Lines today |
|---|---|---|
| `melodia-core/src/entities/lyrics.rs` | `WordStamp`, `LyricLine.words`, `Timing`, `sheet_lines` | 137 |
| `melodia-app/src/library/lyrics/lrc.rs` | the word grammar beside the line grammar it shares `stamp_ms` with: the split, the rebase, the end stamp, the span check | 209 |
| `melodia-app/src/library/lyrics/embedded.rs` | the one chooser over a tag's entries | 17 |
| `melodia-app/src/library/lyrics/mod.rs` | `read_local_text` through the chooser | 245 |
| `melodia-store/src/media/ingest/tag_writer.rs` | every entry read; `Set` replacing them all | 484 |
| `melodia-core/src/entities/tags.rs` | `FileTags.lyrics` as a list | 364 |
| `melodia-views/src/ui/callbacks/tags/open.rs` | the Lyrics field through the chooser | 642 |
| `melodia-views/src/ui/now_playing/lyrics/{mod,follow}.rs` | `Row`'s word stamps, the cut, its writes | 317, 238 |
| `melodia-ui/ui/globals/lyrics.slint` | the three properties | 96 |
| `melodia-ui/ui/components/now-playing/lyrics-panel.slint` | the sung row's `StyledText` | 280 |

Every file stays well under 800; `open.rs` grows by a line or two.

**Duplication collapsed first:**

- The four `.lines()` splits become `sheet_lines` (Phase 1).
- Three reads of a tag's lyrics text, two through `tags::read_lyrics` (`embedded::read`,
  `read_local_text`) and Edit Tags' through `FileTags`, become one chooser (Phase 3).
- `TagWrite::lyrics`' two key removals become one, `Set` clearing as `Clear` does (Phase 3).

---

## Phase 1: The two bugs (no Slint)

- [x] `sheet_lines` in `entities/lyrics.rs`: `str::lines`, each line split again at a lone `\r`, so
      `\n` and `\r\n` text splits exactly as today. `lrc.rs`' three sites and `carries_gloss` take it.
- [x] `strip_word_stamps` leaves one of a seam's own whitespace characters, none where it had none.
- [x] Docs: `strip_word_stamps`' and `sheet_lines`'. `lrc.rs`' `//!` and `carries_gloss`' hold, the
      line rule's anchor being `sheet_lines`.

**Gate:** fmt, clippy, the suite.

## Phase 2: Word times in the sheet (`melodia-core`, `melodia-app`; no Slint)

- [x] `WordStamp`, `LyricLine.words`, `Timing` and `Lyrics::timing`. Every `LyricLine` literal takes
      `words: Vec::new()`: `lrc.rs` and three test files (`entities/tests/lyrics_tests.rs`,
      `romanize_tests.rs`, the views' `lyrics_tests.rs`).
- [x] `strip_word_stamps` becomes `split_word_stamps`, returning the trimmed text and its stamps;
      the rebase per line stamp; the end stamp into `end_ms`; `[offset:]` through `on_track`, the
      line's own clamp. Word offsets index the trimmed text: a stamp at the line's start leaves no
      seam, and one at its end lands past the text, which is what marks it the end.
- [x] The span check after the sort.
- [x] Docs: `LyricLine`'s doc (`words`, and `end_ms`'s two sources), `lrc.rs`' `//!`,
      `close_at_blanks` and `rows_for` (an end stamp ends a line as a blank does).
- [x] Also: `stamp_ms` refuses a time past `i64` milliseconds, which overflowed.

**Gate:** fmt, clippy, the suite, the literals the field breaks fixed here.
**Visible:** only the end stamp, a break after a word-timed line showing its notes sooner.

## Phase 3: Every entry, the best shown (`melodia-store`, `melodia-core`, `melodia-app`, `melodia-views`)

- [ ] `lyrics_entries` in `tag_writer.rs` replacing `lyrics_from`; `read_lyrics` returns the list;
      `FileTags.lyrics: Vec<String>`, the two cue merges clearing it (`cue/merge.rs:181`,
      `cue/embedded/mod.rs:103`).
- [ ] The chooser in `embedded.rs`; `read_local_text` and `tags/open.rs` through it.
- [ ] `TagWrite::lyrics`: `Set` clears both keys first.
- [ ] Docs: the doc comments on `TagWrite::lyrics`, both `read_lyrics`, `FileTags.lyrics` and
      `embedded.rs`' `//!`. ADR 38 holds: the entries come off the one read per track change.

**Gate:** fmt, clippy, the suite; `tag_writer_tests.rs`, `mod_tests.rs` and `merge_tests.rs`
follow the type.

## Phase 4: Lighting the words (`melodia-ui`, `melodia-views`)

- [ ] `Row`'s word stamps; `LyricsUi`'s last published row and cut; the cut in `write_active`,
      written on change and reset where `clear` resets the sung row.
- [ ] `Lyrics`' three properties.
- [ ] The sung row's `StyledText` behind its `if`, and the row's `Text` transparent there.
- [ ] Docs: `README.md`'s lyrics lines (18, 163) say a sheet with word times follows word by word;
      `.claude/rules/slint-pitfalls.md`'s Text section takes `StyledText`: it clips at its box,
      takes no weight, draws none of its spans under a transparent `default-color`, accepts every
      press so a `TouchArea` beneath it gets none, and `@markdown` interpolates a string as plain
      text and a brush as a colour. The panel's header note holds (no `changed`, no `animate`).
- [ ] Measure, release build, dev data folder, Now Playing's lyrics open, the same 60 s of one
      track three times: the sheet with word times, the same sheet with them stripped, and lyrics
      off. CPU from `/proc/<pid>/stat` across each run. Resolves whether lighting words costs
      more than following line by line already does; recorded here.

**Gate:** fmt, clippy, the suite: `lyrics_switch.rs` holds the panel to no `changed` or `animate`,
and `opacity_layers.rs` and `palette_fade.rs` meet nothing new. No `@tr` string is added.

## Phase 5: Tests

### Hand tests

In the dev data folder, after Phase 4 and before any test is written: every phase's hand check
gathers here. Each against `main`'s behaviour where it should match.

- [ ] #177's line in a `.lrc`, its words one space apart; a `.lrc` with word times throughout.
- [ ] A chorus under two stamps lights on both; `[offset:+500]` and `[offset:-500]` move the words.
- [ ] On `main` first: an M4A made with `ffmpeg -metadata lyrics=…`, Save to tag over it, then
      `exiftool -a -G1 -Lyrics` (or `mutagen-inspect`); recorded in Validating the issue.
- [ ] The same lyrics saved into an MP3, a FLAC and an M4A, each reading back as one entry.
- [ ] An M4A whose lyrics came from iTunes or Mp3tag shows line by line; so does a `.lrc` broken by
      a lone `\r` (`printf '[ti:x]\r[00:01.00]one\r[00:03.00]two\r'`).
- [ ] A file holding plain and timed entries, in both orders, follows the song; Edit Tags opens on
      the timed one.
- [ ] Times inside a word (one per syllable): the word joins, Arabic included.
- [ ] Japanese or Korean lyrics; Arabic or Persian: marks against the clip, lit words from the right.
- [ ] A lyric long enough to wrap, and one that wraps past what was measured.
- [ ] The miniplayer's column, through a backdrop toggle while a line is half lit.
- [ ] Seeking into the middle of a line; clicking a line.
- [ ] A line with words out of order lights whole; an end stamp before a long break shows the notes
      from it.
- [ ] Line-timed and plain sheets as on `main`; romanization and a translation under a word-timed
      line.

### Tests

- [ ] On a go naming them, after the hand tests.

---

## Cross-cutting

- Every phase: `cargo fmt --all`, `cargo clippy --all-targets --locked --workspace -- -D warnings`,
  `cargo test --locked --workspace`, then `code-style.md`'s comment audit.
- Memory: a short list per line of the one resident sheet; no RSS check owed.
- No ADR: the reasons live at their anchors.
- #177 isn't linked to the branch: GitHub's API links a branch to an issue only by creating it, so
  it's linked from #177's Development sidebar, or the PR is linked there once open.

## Open questions

None blocking. The dim colour and the clip are settled by the hand tests.
