# Lyrics Display

Working doc for [#32](https://github.com/KenanSalar/Melodia/issues/32). Delete when the feature
ships; leave an ADR behind first.

Status: **proposed** · Created: 2026-09-07 · Branch: `feat/lyrics-display`

> Upstream and local facts below were verified **2026-09-07** against this tree, the pinned
> `lofty 0.24.0` sources in the registry, and `i-slint-compiler 1.16.1`'s own widget sources. The
> LRCLIB response shape was read off a live request rather than off its documentation, and that
> half is expected to drift.

---

## Context

Issue #32 asks for lyrics for the playing track, synced where the source supports it. Melodia can
already *write* lyrics — the Edit Tags dialog has a Lyrics tab, and `tag_writer::apply_lyrics`
writes them keyed by tag type — but nothing reads them back anywhere else. A user playing a track
with a perfectly good `USLT` frame, or a `.lrc` sitting beside it, has no way to read it while the
song plays.

The panel goes in the Now Playing view, behind a toggle in that view's 3-dot menu.

### ADR 21 is not overturned

`docs/adr/0021-lyrics-live-in-the-file.md` refuses a `tracks.lyrics` column, and it refuses it for
a memory reason: the scanner collects every scanned file into one vector before its caller chunks
them for ingest, so a lyrics column would hold the whole library's lyrics resident for the length
of a scan, and lyrics are the one tag with no natural bound.

A now-playing display sidesteps that entirely. It reads one file, for one track, on a track change,
which is exactly what the tag dialog already does through `library::tags::read_lyrics`. No
migration, no column, no backfill, and the scan path is untouched.

The ADR's own exit ramp — *"the moment a lyrics display or search feature is wanted, the column
becomes correct"* — turns on **search**, which needs to query lyrics across the library. This
feature never does. Worth stating in an ADR of its own, because otherwise the next reader reopens
the question.

---

## Where the panel goes

The Now Playing body is two columns: the artwork / title / chips / visualizer column
(`horizontal-stretch: 1`), and a fixed-width right column
(`up-next-width: clamp(root.width * 0.35, 280px, 420px)`) that already swaps its body between
`UpNextList` and the station panel on `Player.vm.has_station`.

**Lyrics become a third arm of that same swap.** The 3-dot toggle decides whether the column shows
Up Next or Lyrics. A station keeps the station panel whatever the toggle says, having nothing to
look up.

Two alternatives were weighed and rejected on structure rather than taste:

- **A third column.** `content-width` is `root.width - pad-lg - pad-md - up-next-width`, and the
  chip wrap (`Player.recompute-chip-rows`), `strip-w` and `cover-size` all derive from it. A third
  column leaves the artwork column near its floor on an ordinary window, and the layout would
  reflow every time a track without lyrics came on — the worst property a panel can have, since
  availability is data rather than intent.
- **Under the visualizer.** The cover slot (`now-playing-view.slint:223-226`) is deliberately the
  artwork column's *only* cell with slack between `min` and `preferred`; every other child has
  `min == preferred`. That is what keeps the visualizer strip inside the panel on a short window,
  and `.claude/rules/visualizer.md` argues it at length. A new child there spends that budget. A
  wide-short region is also the worst shape for synced lyrics, which want vertical context above
  and below the sung line.

Replacing the column costs nothing structurally: `content-width`, the cover slot and the strip are
all untouched, and the station panel block (`now-playing-view.slint:364-426`) is a copy-ready
template that already satisfies `crates/melodia/tests/scrollbars.rs`. Queue access isn't lost
either — `Q` opens the full queue sheet.

---

## Decisions

1. **Resolution order: sidecar → embedded → online.** A sidecar is the one of the three a user
   places deliberately, so it outranks whatever a tagger happened to write and whatever a stranger
   uploaded. That ordering is what makes the feature correctable: a wrong sheet is fixed by
   dropping a `.lrc` next to the file, with no setting to find and nothing to re-scan.
2. **LRCLIB ships off**, guarded at the `library::lyrics` facade the way ADR 15 guards radio, and
   surfaced on the first-run card so the switch is discoverable.
3. **A fetched sheet lands in `<data>/lyrics/` and never in the user's audio files.** The automatic
   path must not write a tag nobody asked for — a lookup that silently rewrote files would be
   doing tag editing, which this app already has a deliberate, confirmed, undoable surface for.
   The worst case a store write can produce is a stale cache entry the next fetch replaces.
4. **Line-level sync only.** Enhanced word tags are parsed and dropped so an enhanced file reads as
   a normal one instead of rendering `<00:12.34>` as text. Karaoke sweep is a follow-up.

### Why the switch is not a judgement call

`packaging/com.github.kenansalar.melodia.metainfo.xml` and `crates/melodia/Cargo.toml`'s
`extended-description` both ship this sentence:

> Your music collection lives entirely on your own machine, and every online feature is a setting
> you control: scrobbling, Discord presence and internet radio all ship switched off.

An online lyrics lookup that shipped on would make that text false. This is exactly the argument
ADR 15 used to force radio off, applied to the same sentence. So the flag defaults `false`, and the
enumeration in both shipped descriptions gains a fourth item.

ADR 15 also names the cost honestly — *"it exists and nobody sees it until they go looking"* — and
the onboarding row is the answer to it.

---

## Phases

### Phase 0 — branch · **done**

`feat/lyrics-display`, pushed.

### Phase 1 — the vocabulary · **done**

`melodia-views` can name neither `melodia-store` nor `melodia-net` (cargo enforces it, and there
are no cross-crate re-exports), so the type the panel consumes lives in core.

**`crates/melodia-core/src/entities/lyrics.rs`**, registered in `entities/mod.rs`. No serde: this
is a vocabulary two layers share, not a persisted row, so it follows `entities/tags.rs` rather than
the `FromRow` projections.

```rust
pub struct LyricLine {
    pub at_ms: Option<i64>,
    pub text: String,
    pub translation: Option<String>,
}
pub struct Lyrics { pub lines: Vec<LyricLine>, pub source: LyricsSource }
pub enum LyricsSource { Sidecar, Tag, Online }
```

`at_ms: Option` rather than two structs: a plain sheet is the synced one with no stamps, and the
panel renders both from one model.

`translation` is the gloss a bilingual sheet carries under the words. Two fields rather than one
string with a separator still in it: the panel draws the two at different sizes, and the parser is
the half that knows how the sheet spelled the break.

Two things landed differently from the sketch above, both narrowing what can be stated wrongly:

- **`synced` is a derived `is_synced()`, not a field.** A stored bool is a second answer to a
  question the lines already settle, and the two can drift. Called once per track change, so the
  walk costs nothing, and it short-circuits on the first line of a timed sheet.
- **`Lyrics::new` returns `Option<Self>`** and refuses a sheet whose lines are all blank. All three
  resolver arms end there, so "blank means no answer" is settled once instead of three times, and
  a caller holding a `Lyrics` has something to draw. Blank rather than empty, because a lyrics tag
  containing only newlines is one a tagger created and nobody filled in. Blank lines *inside* a
  sheet are kept, being how a plain one spaces its verses.

### Phase 2 — the LRC parser · **done**

**`crates/melodia-app/src/library/lyrics/lrc.rs`** (new), pure, no I/O. The precedent is
`library/playlist_files/m3u.rs`: hand-rolled, no crate. The `lrc` crate pulls `regex` + `educe` +
`unicase` for something under a hundred lines, and the format is stable enough that the dependency
would be carried forever against a parser that never needs to change.

- `[mm:ss.xx]`, `[mm:ss.xxx]`, `[mm:ss]`, and `,` as the decimal separator.
- **Multiple stamps on one line** — `[00:21.10][00:45.10]chorus` emits the line twice. This is how
  the format spells a repeated chorus, so it is the common case rather than an edge one, and a
  parser that takes only the first stamp silently loses the second half of the song.
- **`[offset:±ms]`** applied to every stamp. It is part of the format and it is the only correction
  a hand-timed sheet carries, so dropping it means the sheets that most need the nudge are the ones
  that run early.
- **`[ti:]`/`[ar:]`/`[al:]`/`[by:]` dropped**, which falls out of the timestamp parse failing —
  no separate branch.
- **Enhanced word tags `<mm:ss.xx>` parsed out of the text and discarded.** A non-timestamp `<3`
  stays literal.
- **A `^` between words and their translation splits the line into two fields.** No part of the
  format, but the directories carry sheets written this way, and left in the string the gloss is
  drawn as the tail of the line it is glossing — the one thing it must not look like. Only a caret
  with text either side is a separator, so a sheet that simply contains one keeps it.
- Sorted by stamp — a stable sort, so duplicate stamps keep file order.
- No stamps anywhere means an untimed sheet, lines in file order.

Three notes from building it:

- **`is_probably_lrc` is not there, and no caller wants it.** `parse` is total: it takes a sidecar,
  a lyrics tag or an API field without being told which, and answers with a timed sheet, a plain
  one, or nothing. lofty's advice to "try parsing it as LRC" is discharged by calling `parse` and
  reading `Lyrics::is_synced`, so a predicate beside it would be a second way to ask one question,
  which is the same thing the derived `is_synced` avoids.
- **A timed sheet keeps only its timed lines.** That is what makes "a sheet is timed or it is not"
  true downstream rather than merely hoped for, and it is why the mixed case never reaches the
  panel. A sheet with no stamps at all keeps everything, including its `[Chorus]` markers and its
  blank lines, which is how a plain sheet spaces its verses.
- **`pub mod lrc` for now, `mod lrc` in Phase 4.** `playlist_files` keeps its `m3u` private
  because the facade is the door, and this owes the same. It cannot be private yet: nothing calls
  it until Phase 4, and a private module with no callers is `dead_code` under the workspace lints.

The offset direction was verified rather than assumed: a **positive** `[offset:]` means the sheet
runs early, so it comes off the stamps. Getting the sign backwards would double the error on
exactly the sheets that carry the tag.

### Phase 3 — reading the file · **closed, nothing to build**

**Don't re-propose a `SYLT` reader.** It was written, tested against a real frame, and then
deleted, and the reasons are worth keeping so the next reader does not spend the day again.

The phase existed to reach `SYLT`, the only *specified* container for timed lyrics. Building it
turned up four things:

- **A binary frame cannot reach a generic `Tag`.** `Id3v2Tag`'s `split_tag` returns
  `FRAME_RETAINED` for `Frame::Binary`, keeping it in the `ID3v2` remainder instead of lowering it
  to a `TagItem`. So a `TaggedFile` cannot carry a `SYLT` however `read_tags` is asked. Reaching
  one means parsing the concrete file, which is a second lofty opener.
- **`crates/melodia/tests/lofty_open.rs` forbids exactly that**, and names `tag_writer.rs` as the
  file that broke it before. `metadata.rs` is the sanctioned opener and the reader did work there,
  but it turns "the single place a path is handed to lofty" into "the single module", which is a
  weaker sentence bought for one rare frame.
- **The cost is per track change, not one-off.** In the resolution order below, `SYLT` is only
  reached when the text tag did not already yield timed lyrics, which for most libraries is every
  track. Keeping it meant a full second `MpegFile` parse on every MP3 track change, hunting a frame
  that is almost never there. `ParseOptions::read_cover_art` also defaults **on**, so the first
  draft decoded embedded album art on every one of those parses.
- **Nothing can produce a file it would read.** Melodia's own tag editor writes
  `ItemKey::Lyrics` / `UnsyncLyrics` as text, and so does every tagger and lyrics tool worth
  naming. The universal carrier of timed lyrics in a tag is LRC *text* in `USLT` or `LYRICS`, which
  `read_lyrics` already returns and Phase 2's parser already reads, timed case included. That is
  the whole reason `SYLT` looked necessary and the reason it is not.

So the embedded arm is `read_lyrics` + `lrc::parse`, both of which already exist. The reader was
never committed and is in no history; if a genuine `SYLT` file ever turns up, the four points above
are the whole recipe: `MpegFile::read_from` inside `metadata.rs`, gated on the extension rather
than the header, refusing frame-unit timestamps and every content type but `Lyrics` and
`TextTranscription`, with `read_cover_art(false)` beside `read_properties(false)`.

**One thing was kept**, because it stands on its own and cost a debugging round to find:
`FileType::from_buffer` makes no attempt to search past a leading `ID3v2` tag, so it answers `None`
for the great majority of MP3s, which open with one. `sniff_file_type` is therefore a **fallback**,
correct where `read_tags` asks it once the extension resolved to nothing, and silently wrong as a
primary gate. Its doc comment now says so.

### Phase 4 — the resolver and its off switch · **done**

**`crates/melodia-app/src/library/lyrics/mod.rs`** is the one door, shaped like
`library/radio/mod.rs`: the module holds the switch, and the arms are private files, so no caller
learns which one answered.

```rust
pub async fn for_track(state: &AppState, track: &TrackSummary) -> Result<Option<Lyrics>, AppError>
```

Order — **sidecar → embedded → online**:

1. **`sidecar.rs`** — `<stem>.lrc` *and* `<file-name-with-extension>.lrc`, in that order. Both are
   in circulation and which one a user has depends on whatever produced it, so checking a single
   name is a coin flip. A missing sidecar is not a failure; anything else that went wrong is
   reported, a non-UTF-8 sheet included, since `read_to_string` refuses those.
   **Spelled rather than searched**: matching a `.LRC` on a case-sensitive filesystem would mean
   listing the directory on every track change, which is a real cost on a large folder against a
   spelling almost nothing writes. (The plan's `eq_ignore_ascii_case` note did not apply; the names
   are constructed, not compared.)
2. **`embedded.rs`** — `tags::read_lyrics` through `lrc::parse`, which is the whole of it. The tag
   holds LRC text or plain prose and declares neither, and the parser answers both. Phase 3 says
   why there is no `SYLT` arm.
3. **The lookup** — deferred to Phase 5, where the request is.

**`mod lrc` is private now**, as Phase 2 promised, since its three sibling arms are its callers.

Four things landed differently:

- **There is no `online.rs`.** A module whose only content was a guard and a placeholder drew two
  lints in a row — `unused_async`, then `unnecessary_wraps` — each correctly saying the function
  did nothing. Both would have wanted a suppression that Phase 5 immediately deletes. The guard
  went into `for_track` instead, where it is genuinely in the path, and Phase 5 replaces the
  trailing `Ok(None)` with the call. Nothing is stubbed and nothing is suppressed.
- **The guard answers `bool`, not `Result`.** Radio's `ensure_enabled` refuses because radio
  unmounts its section when off, so a call arriving there is a bug. Here the switch being off is
  the ordinary state of most libraries, and the honest answer is "no sheet" rather than an error.
  `online_lookup_enabled` is still the only place `lyrics_online_enabled` is named.
- **A file-source error propagates today.** Whether a tag this could not read should still reach
  the lookup is a real question, and the answer only pays for itself once there is a lookup to
  reach, so Phase 5 settles it. Noted at the call site rather than decided early.
- **The shipped descriptions are Phase 5's**, not this one's. Nothing here opens a socket, so the
  sentence they carry is not false yet. It becomes load-bearing the moment a request can happen.

**The switch:**

- **`LyricsFlags { lyrics_panel_shown: bool, lyrics_online_enabled: bool }`**, a new
  `#[serde(flatten)]` group rather than two more bools in `LibraryFlags`, which already carries an
  `#[expect(clippy::struct_excessive_bools)]` at the cap. Both default `false`, so the derive is
  the honest `Default` and no hand-written impl is owed.
- **`AppState::lyrics_online_enabled`**, a `SharedFlag` seeded at boot. Only the online flag gets
  a shadow: the panel flag is read once at boot by the Slint seeding, exactly as
  `VisualizerFlags::viz_enabled` is, and never on a worker.
- `library::settings::{set_lyrics_panel_shown, set_lyrics_online_enabled}` persist through
  `mutate_settings`; the UI callback writes the shadow **synchronously before** spawning the
  persist (Phase 9).
- **Reading a `.lrc` already in the store is not traffic**, so a cached sheet keeps working with
  the switch off. That is what the toggle sells ("no traffic"), not "no lyrics".

Unlike radio's, this switch has no nav section, no playing source and no persisted index to fold,
so `ui::radio::disable`'s four consequences have no analogue. Turning it off stops the fetch and
nothing else.

### Phase 5 — LRCLIB · **done**

**`crates/melodia-net/src/services/net/lrclib.rs`** (new), beside `radio_browser`.

`GET https://lrclib.net/api/get` with `track_name`, `artist_name`, `album_name`, `duration` (whole
seconds) — all four already on `TrackSummary`, so no database round trip. `/api/get` rather than
`/api/search`: the four fields together identify one recording, so the service answers or it
doesn't, and there is no candidate list to score. A search endpoint would hand back near-matches
and put the burden of deciding which one is this track onto us — a whole scoring pass whose only
job would be to reconstruct the certainty `/api/get` starts with.

- The status is checked here and the body goes through `services::net::read_capped`, which is
  `radio_browser::get_json`'s shape. **Not `get_capped`**, which treats every non-success as an
  error: a `404` is this endpoint's ordinary answer and folding it into a failure would report an
  outage for every track nobody has written lyrics for.
- URL built with `reqwest::Url` + `query_pairs_mut`, never string-formatted.
- `User-Agent: Melodia/<version> (https://github.com/KenanSalar/Melodia)`, which is the shape the
  service asks for. The shared client already sends `Melodia/<version>`; this adds the project URL
  and only on requests to this host.
- **The request is skipped where it could only miss.** The directory identifies a recording by
  artist, title and duration together, so a track with no artist tag, or one whose duration never
  made it out of the scan, is not asked about at all: it is traffic spent to learn nothing.
- ~256 KiB cap, timeout in line with `station_logo`'s.

**`LyricsAnswer` is the boundary type and lives in `melodia-core`.** The service's own response
shape stays private to `melodia-net`, exactly as `ApiStation` does, and what crosses is a domain
answer. It carries three outcomes rather than two, since a track the directory *knows* has no words
deserves different copy and a different retry from one it simply has not got.

Two things the phase turned up:

- **An empty `syncedLyrics` shadowed a real `plainLyrics`.** `text()` picked the timed field and
  tested it afterwards, so a directory answering with `""` beside real plain lyrics produced
  nothing. Each field is judged before it is preferred now. Found by writing the assertion, not by
  reading the code: the test passed against the wrong expectation and the message contradicting it
  is what showed the bug.
- **An instrumental still collapses to "no sheet".** The answer carries the flag and the fetch
  reads it, but `for_track` returns `Option<Lyrics>` and a `Lyrics` is a sheet, so nothing can
  express "this track has no words" yet. The outcome type that tells that apart from "nothing
  found" belongs with the panel that draws the difference (Phase 7), and the store that must not
  re-ask (Phase 6).

No new dependency: `melodia-net` already carries `reqwest` and `serde_json`.

**The shipped descriptions gained their fourth item here**, handed over by Phase 4:
`packaging/com.github.kenansalar.melodia.metainfo.xml` and `crates/melodia/Cargo.toml`'s
`extended-description` now read "scrobbling, Discord presence, internet radio and online lyrics all
ship switched off", which this is the commit that makes true. **`README.md` waits for Phase 10**:
its bullet would describe a panel that does not exist yet, where those two are a promise about
network behaviour that just became load-bearing.

### Phase 6 — the store · **done, bar the prune**

**`crates/melodia-app/src/library/lyrics/store.rs`**, plus `Paths::lyrics_dir` and its line in
`create_dirs`.

- **Three names, not two.** `<hash>.lrc` for a sheet, `<hash>.instrumental` and `<hash>.none` for
  the two empty answers. The markers are what the phase turned out to need a type for (below).
- **The hash is of the path, not the contents.** The artwork store hashes contents on purpose and
  this deliberately does not: a tag edit rewrites the file and moves its content hash, so a sheet
  keyed that way would be orphaned by the user correcting a typo in the title. Both schemes wear
  the same 16 hex characters, which is exactly why the module says which one it is.
- Written through `utils::atomic_file::write_text_sync`, and **the sheet is stored as text** rather
  than as a parsed `Lyrics`: what goes on disk is what the directory sent, so re-reading it later
  cannot differ from reading it now, and a round trip through a serializer would quietly lose
  whatever the parser drops.
- **A file that will not read is a miss, not an error.** That is the honest semantics rather than a
  swallowed failure: every answer this gives is an optimisation, and the caller's next move for
  "unreadable" and for "absent" is the same one.

**`LyricsOutcome` came forward from Phase 7, because the store forced it.** With only
`Option<Lyrics>` an instrumental is indistinguishable from a miss, so it would be written as a miss
and re-fetched forever on the expiry below. Three states settle it: a sheet, a recording the
directory *knows* has no words, and one nobody has. `for_track` returns that now.

**Expiry is checked on read, not left to the prune.** A `.none` older than the window reads as
"not cached" and the track is asked about again, so the retry happens whether or not any
maintenance pass has run. `.instrumental` and `.lrc` never expire: the first is a permanent fact
about the recording, the second is the answer itself.

**The store sits on the local side of the switch.** Turning the lookup off buys no traffic, not no
lyrics, so a sheet already fetched keeps being read. In the chain it sits *after* the sidecar and
the tag and before the network, since it is only ever a copy of the last answer while those two are
the user's own files.

**The prune is Phase 8's**, and it is deferred rather than dropped. The store only grows while
something is rendering Now Playing, which is the only thing that calls `for_track`, so the trigger
is that view closing: `library::radio`'s exact argument for pruning on a section leave rather than
after a scan. Phase 8 already opens `wire_now_playing_open`'s `!is_open` arm to release the sheet,
and the pass belongs beside it rather than written now with nothing to call it.

### Phase 7 — the panel · **done**

**`crates/melodia-ui/ui/globals/lyrics.slint`**: the `Lyrics` global — `rows`, `state`, `synced`,
`active-index`, `active-offset`, `active-height`, the four type-scale properties below, `shown`,
`set-shown(bool)`, `seek-at(length)`, `tick()`. Imported **and** added to `app-window.slint`'s flat
`export { }` block, or Slint prunes it from the Rust API; `LyricRow` joins the model list the same
way.

**`state` is a Slint `enum`, not the int the plan asked for.** The reason for refusing a set of
bools was that the states are mutually exclusive and flags can spell combinations none of them
means, and an enum says that in the type instead of in a comment. `NavEnterFrom` is the precedent,
including that an enum reaches Rust off the *import* line without an export entry.

**The `Lyrics` global holds the sheet's whole type scale, and Rust reads all of it from there** —
`font-size` / `line-height` for the words and `translation-font-size` / `translation-line-height`
for the gloss under them. The panel multiplies each height by the row's line count; Rust multiplies
it by the same to build the offset table it follows the song with, and measures its wrap estimate
against the sizes. Spelled once and read from there rather than twice, because a drift is a scroll
that is subtly and then increasingly wrong down a long sheet.

A step above the rest of the column: this is the one panel a reader looks *at* rather than glances
over, and it has the column to itself.

**Two gaps, and the first has to stay clearly under half the second.** `translation-gap` separates
a line from its own gloss; `row-gap` is the layout's `spacing` between one line and the next.
Proximity is what groups the pair, not the size step — a reader pairs by distance before they pair
by weight — so setting the two anywhere near each other leaves one evenly-spaced column with no
telling which line belongs to which.

**`crates/melodia-ui/ui/components/now-playing/lyrics-panel.slint`** (new), built by copying the
station panel block: a `ScrollView` with both scrollbar policies `always-off`,
`viewport-width: self.width`, content inset by `Theme.scrollbar-slot`, and a sibling
`OverlayScrollbar` at `parent.width - self.width` carrying the same five `Player.np-accent-*`
bindings. Copying the block satisfies `crates/melodia/tests/scrollbars.rs` rather than working
around it.

**A plain `ScrollView` with a `for`, not a `ListView`, and the reason is not the one it looks
like.** Read `i-slint-core`'s `update_visible_instances` rather than the widget: the listview
repeater *does* measure each mounted row's real height, so uneven rows are no objection. What it
also does is **own the scroller's geometry** — it writes `viewport-y` itself on every layout pass
(unless the property carries a binding, which this one deliberately does not), and it publishes
`viewport-height` as `cached_item_height * row_count`, the cache being an average over whichever
rows happen to be mounted. Both are fatal here: the panel centres the sung line by *writing*
`viewport-y` off an exact offset table, and the `OverlayScrollbar` gauges extent off
`viewport-height`, which under an average would drift as the sheet scrolled through rows a gloss
makes nearly twice as tall. It also sidesteps the nested-`ListView` wheel-swallowing trap and the
drag-pan question.

What makes the un-virtualized `for` right instead is that a sheet is *small* and, via
`MAX_ROWS`, *bounded* — three elements a row over at most a few hundred rows, mounted only while
the panel is open. Nothing about a `.lrc` file or a lyrics tag is length-limited and both come from
outside, so the cap is what turns "small" from an assumption into a property.

**Row height is Rust's, and that is what makes auto-scroll exact.** A `for` loop exposes no
per-item element whose `y` could be read back — the wall the `TabBar` underline hit. So each row
carries a `line_count` estimated in Rust and is a box sized from it. Rust chose the height, so
Rust's cumulative offset table cannot disagree with the layout.
`ui::chips::estimated_chip_width` is the precedent, including which way to bias: over-estimating
costs whitespace, under-estimating elides, so bias generous.

**Nothing in the panel carries a `changed` handler, and the plan's follow design had to go.** It
proposed an animated `follow-y` with `changed follow-y => { sv.viewport-y = … }`, which is the
tracker shape `slint-pitfalls.md` calls fatal: this component is mounted behind an `if`, a tracker
in a dropped branch stays registered against whatever it watched, and an `animate` re-dirties its
target *in the same frame* the branch goes when the user hits the toggle mid-glide. So:

- The follow is driven imperatively from the panel's own `Timer`, which dies with the branch, and
  the easing is arithmetic: `scroll-y += (target-y - scroll-y) * follow-step` at 33 ms, settling
  inside a third of a second. That reads as a glide and needs no `animate` block at all.
- **The scrollbar is polled, not watched.** `if (bar.dragging)` inside the tick is what a
  `changed dragging` would have been, minus the tracker.
- A wheel is noticed by a full-panel `TouchArea` whose `scroll-event` returns `reject`, so the
  scroller still gets it. Handling an *event* is not a tracker, so this half is safe as written.
- **Suspension is a tick countdown, restarted on the *last* scroll rather than the first**, or a
  reader paging back through a verse gets yanked forward mid-gesture. While suspended the eased
  position is kept with the reader, so resuming glides from where they left the panel rather than
  from where the song is. Four seconds. No "resume" pill in v1.
- Click a line to seek, gated on `Lyrics.synced`: an untimed sheet has nowhere to send the player,
  and a row that looked clickable and did nothing would be worse than one that never did. **The
  pin on the clicked row is Phase 8's**, since the thing being pinned against is Rust's tick.
  **This shipped broken and the fix moved the click off the rows.** A per-row `TouchArea` could
  never have received it: the wheel-notice area covers the whole panel and is declared *after* the
  scroller, so it was over every row and swallowing every press. One area does both jobs now, and
  it reports the *point* — which row that is comes out of the offset table Rust already keeps, so
  it is arithmetic rather than a second answer, and a sheet is three elements a row rather than
  four. The gap between two rows belongs to the one above, the gaps being wide enough to be missed
  into; below the last row nothing is seeked, that whitespace being most of what gets clicked by
  accident.

**Empty / loading / missing / off states** reuse the hand-rolled centred block at
`up-next-list.slint:212-236` — glyph plus one line of copy on `Player.np-on-backdrop-muted` — not
`GridEmptyState`, whose `background: Theme.base` is opaque and would punch a hole in the backdrop.
The *off* state is the one that earns its copy: it says the online lookup is off and where the
switch is.

**`now-playing-view.slint` changes in exactly two places** and nothing else in the file moves:

- the column heading's ternary gains a third arm (Station / Lyrics / Up Next);
- `if !Player.vm.has_station: UpNextList` becomes two branches on `Lyrics.shown`.

`up-next-width` and `content-width` are **not** touched. Two pins in that file must survive the
edit verbatim: `crates/melodia/tests/backdrop_mounts.rs` (one `property <bool> aurora-shown:`, one
mount per stack, each line *starting with* its gate) and `visualizer_tests`' exact
`strip-height: root.strip-h;`.

**`view-menu.slint`** gets the toggle row: an
`OverflowRow { icon: "lyrics"; label: @tr("Lyrics"); active: Lyrics.shown; }` writing both halves
the way the Visualizer row does — the `in-out` property so the panel swaps immediately, and the
callback to persist. **`menu-h` moved from `menu-row-h` to `menu-row-h * 2` in the same edit**;
forgetting it clips the popup.

`"lyrics"` went into `scripts/icons.txt` and both faces were re-subset. `scripts/check-icons.py`
caught the drift, which is exactly the value of it: the name was in the list and resolved in
neither face, so the row would have drawn a tofu box and nothing else would have said so.

**Six new `@tr` literals, so six new msgids in each of the six catalogues.** The one worth
checking on a re-read is *"Turn it on under Settings ▸ Services"*, which names a place: each
catalogue's copy uses that catalogue's own translations of the Settings and Services labels rather
than the English ones, or the copy sends a reader somewhere they cannot find.

### Phase 8 — the Rust wiring · **done**

**`crates/melodia-views/src/ui/now_playing/lyrics.rs`**, plus `tasks::lyrics_cache` and the two
hooks in the existing subscriber and teardown.

Four things worth knowing before reading it:

- **The fetch is gated on `Lyrics.shown`.** A hidden panel would otherwise cost a file read on
  every track change and, with the switch on, a request on every track change for something nobody
  is looking at. The gate sits at the top of `apply_source_change`, beside the existing one for the
  view being open at all.
- **Every position is milliseconds in an `i32`**, which is what the `Player` global already
  publishes and what a row carries. Clippy's cast lints are what forced the question, and the
  answer is better than what the plan sketched: an `i32` reaches past three weeks of one track and
  converts into `f64` without losing a bit, so the interpolation needs no cast anywhere. The wrap
  count is a `u8` for the same reason.
- **The wrap estimate is bucketed rather than rounded**, so the line count comes out of two
  comparisons instead of a float-to-int cast. It is capped at three lines: past that a lyric line
  has stopped being a line, and the panel would scroll more than it shows.
- **Being wrong about the wrap costs nothing structural.** The panel draws whatever `line_count`
  comes back and the offset table is built from the same number, so the table and the layout agree
  however bad the guess. Only wrap quality is at stake, which is why the width can be polled
  loosely rather than measured.
- **The estimate is in ems, and a character is measured against the script it is in.** Hangul and
  CJK are drawn on a square em where Latin sits near half of one, so a single averaged width
  under-estimates a Korean line by nearly half — enough to elide words the panel had the room for,
  which is the one error direction this estimator is written to avoid.

`apply` takes `online_enabled` from its caller rather than reading it, because the flag decides
only which of two sentences an empty panel shows and the caller is the half holding `AppState`.

- **The fetch hooks into `apply_source_change`** (`now_playing/source_change.rs:108-179`), beside
  `fetch_track_meta`, which is the exact template — including its two staleness guards: the Rust
  `key != current_source` re-check after the await, and a Slint-side gate mirroring
  `MetaChipStrip.show-rows: Player.track-meta.track_id == Player.vm.track.id`. That function is
  already where all three callers converge (live change, seed-on-open, square miniplayer), so one
  hook covers every path, and it only runs while something is rendering.
- **The tick.** `PositionTick` reaches the UI at **~1 Hz, not 500 ms** — `handlers.rs`'s
  `SecondGate` admits one per whole second on purpose, because the bar renders seconds and a
  slider thumb moves invisibly between two 500 ms samples. A once-a-second highlight is visibly
  late, so the panel interpolates: a `Timer` mounted *inside* the panel (so it dies with it) fires
  at ~100 ms and calls `Lyrics.tick()`; Rust advances from the last `Player.position-ms` write by
  wall-clock elapsed × the current playback rate, binary-searches the stamps, and writes
  `active-index` / `active-offset` only when the line changes. The visualizer strip's
  `Timer { running: … }` in this same view is the shape, including gating `running` on playback so
  a paused player stops the tick.
  **No lookahead**, and the tempting one would be backwards: `.claude/rules/audio-stack.md` notes
  the reported position already runs a device buffer *ahead* of the ear, so any positive nudge
  compounds an error the chain already has. The buffer is far too small to matter at line
  granularity, so the honest answer is zero.
- **Teardown.** The parsed sheet is released in the existing `!is_open` arm of
  `wire_now_playing_open` (`up_next.rs:133`), beside `np_artwork.clear()`. **No second handler on
  `on_now_playing_open_changed`** — it has one slot and that function owns it. Only the current
  track's sheet is ever resident, so there is no in-memory LRU to size: the disk store is the cache.
- **`tasks::lyrics_cache`, handed over by Phase 6**, spawns from that same `!is_open` arm. The
  store only grows while something is rendering this view, since nothing else calls `for_track`, so
  the close is exactly when it stops: `library::radio`'s argument for pruning on a section leave
  rather than after a scan, and `tasks::radio_logo_cache::spawn` is the shape. It caps the
  directory by total bytes, oldest-mtime-first, and sweeps expired `.none` markers; `.lrc` and
  `.instrumental` are permanent answers and only the byte cap reaches them. Detached and tracked,
  so a shutdown mid-pass waits for the unlinks.
  **It touches only names the store itself wrote**, which is the artwork sweep's rule and applies
  for its reason: this directory sits under the user's data root, and a pass that deleted whatever
  it found would delete whatever someone else put there. It reaches the store through
  `library::lyrics::prune_store` rather than naming a submodule, so the naming scheme stays behind
  the door.

### Phase 9 — Settings row and onboarding · **done, bar the version bump**

**Settings ▸ Services**, not Library. That page holds Radio, Scrobbling and Discord — precisely
the three the shipped description enumerates — so a fourth online feature belongs beside them.

**The label and description are two msgids, read by both surfaces.** The Settings card and the
welcome card name the same switch, and the panel's own comment already asked for that ("Same labels
as the Services tab's own rows, so the two never describe one feature differently"). Declaring them
once means the two cannot drift, and it is also what keeps the search predicate honest, since it is
fed `label + " " + desc`.

**The card configures traffic and nothing else.** Whether the column shows lyrics or Up Next is a
view preference flipped from that view's own overflow menu, so it is deliberately not a second row
here: a setting that duplicated the menu toggle would be two controls for one answer.

New `crates/melodia-ui/ui/views/settings/lyrics-section.slint`, copying `radio-section.slint` line
for line: `in property <string> tab-name`, a `card-title`, the local `row-visible(label, desc)`
wrapper over `SettingsPage.row-visible`, the label and description declared **once** as properties
(a pair spelled at both sites lets a reworded description stop its own row matching), the `show-*`
bool, `out property <bool> has-matches`, and the `VerticalLayout { if has-matches: SectionCard { … } }`
wrapper with the row behind `if root.show-*:` rather than `visible: false`.

Mounted in `pages/services-page.slint`'s **second** column (the first already carries two cards)
with `tab-name: @tr("Services");`, and its `has-matches` ORed into that page's chain. The
description names lrclib.net for radio's stated reason: a user deciding to stay local is entitled
to know who would be contacted.

**Onboarding needs no new step.** Step 3 (`components/onboarding/features-panel.slint`) is already
*"What Melodia can reach — Melodia works offline. These are the parts that reach out."*, one
`OnboardingFeatureRow` per feature. Lyrics is one more row plus a `SectionDivider`, placed third —
after Discord, before the Scrobbling button — which keeps the file's existing grouping of plain
switches, then the button, then the one row that ships on.

That satisfies every constraint on that directory for free. The row is a `ToggleSwitch` firing the
same `Settings` callback its Settings row fires, so it carries **no `changed` handler** — the
prohibition exists because a tracker in that dropped `if` branch survives it and panics the next
time the watched property moves, which the Settings ▸ About row does by re-opening the card. And
`Onboarding.step-count` does not move, so the four-way lockstep between `step-count`, `PANELS`, the
branch list and the `[0, 1, 2]` dot literal is untouched.

Two things not to break in that file: `crates/melodia/tests/onboarding.rs` asserts
`features-panel.slint` still contains both `auto-check-enabled` and `!MelodiaUpdater.system-managed`,
and the whole directory still greps clean for `changed `.

**`ONBOARDING_VERSION` is deliberately NOT bumped, and that is the one thing this phase left
open.** `OnboardingFlags::needs_onboarding` is `onboarding_version < ONBOARDING_VERSION`, so an
install that has already run the card carries `1` and **will never see the new row** — which is
most of why the row was put there. The constant's own doc comment says a bump is exactly the
affordance for this: *"A revision rather than a bool so a later feature that belongs in the welcome
card can bump `ONBOARDING_VERSION` and reach installs that already ran the flow, instead of owing a
separate what's-new surface."*

It is left alone because it is a product call rather than a wiring one, and because this would be
its first use: bumping re-shows the **whole three-step card**, folder picker and all, to every
existing install, which is a heavy greeting for one new switch. The alternatives are to bump anyway
and accept that, to wait for a release carrying more than one card-worthy change, or to give the
card a way to open on a later step. New installs see the row either way.

### Phase 10 — tests · **done** · docs · **not started**

**Tests**

| suite | cases |
|---|---|
| `library/lyrics/tests/lrc_tests.rs` | 22 |
| `library/lyrics/tests/store_tests.rs` | 14 |
| `now_playing/tests/lyrics_tests.rs` | 12 |
| `entities/tests/lyrics_tests.rs` | 9 |
| `library/lyrics/tests/sidecar_tests.rs` | 7 |
| `crates/melodia/tests/lyrics_switch.rs` | 5 |

Designed by partition and boundary rather than by walking the code: the timestamp field's four
precisions and its two invalid signs, the offset in both directions and past the first stamp, the
expiry window on the step either side, and the empty / blank / metadata-only partitions of "no
sheet". Each case has one reason to fail, and the custom messages argue rather than restate.

**Every pin in `lyrics_switch.rs` was checked by breaking the code**, which is the only way to know
a pin is not decoration. Five mutations — reverting `menu-h` to one row, adding a second reader of
the setting, reaching the directory from `library::tracks`, rewording the welcome card's label, and
renaming the seam — each failed **exactly one** test, and the right one.

Two notes on coverage rather than a number:

- **The embedded arm's glue is not pinned end to end**, and deliberately. It is three lines,
  `tags::read_lyrics` into `lrc::parse`; the tag read is already pinned by
  `tag_writer_tests::read_lyrics_round_trips_on_flac_and_mp3` and the parse by the 22 above, so a
  fixture test here would be a system-level answer to a question two unit levels already answer.
  The plan's `USLT` and `LYRICS` fixtures were dropped for that reason, not forgotten.
- **The scrollbar contract needed no new pin.** `crates/melodia/tests/scrollbars.rs` walks the tree
  and already covers the panel's scroller, which is what copying the station-panel block bought.

**Docs**

- **`docs/adr/0038-lyrics-display-reads-the-file.md`** plus its row in `docs/adr/README.md` under
  *Library and data*. It exists because a future reader will otherwise reopen the question ADR 21
  left open: the display reads one file per track change, so the scan-resident vector that argument
  turns on never comes into it, and the column stays refused. It also records why this store hashes
  the path where the artwork store hashes contents.
- `README.md`: a Now Playing feature line, an *Off until you switch it on* line mirroring the Radio
  section's, and `<data>/lyrics/` in the persisted-files table.
- `CLAUDE.md`: `library/lyrics` as the second facade carrying an off switch, `lyrics/` in the
  persistence list, and lyrics in `melodia-net`'s description, which currently says `net/` holds
  "the radio directory client and its blocklist".
- `.claude/rules/ui-patterns.md`: the Now Playing right column has three arms now.

---

## Verification

Static gates while implementing:

```bash
cargo fmt --all
cargo clippy --all-targets --locked --workspace -- -D warnings
cargo test --locked --workspace
python3 scripts/check-icons.py
```

The existing corpus walks are the real gate, and each fails on a specific mistake above:
`scrollbars.rs` (the new scroller's two policies and its sibling bar), `backdrop_mounts.rs` and
`visualizer_tests` (the two pins in `now-playing-view.slint`), `net_primitives.rs` (the fetch must
go through `http_url` and the capped read), `onboarding.rs` (no `changed ` under that directory),
`translations.rs` (every new `@tr` literal needs its msgid in all six catalogues — there is no
`en.po`), `workspace_shape.rs` and `error_as_string.rs`.

By hand, after that:

1. A track with a `.lrc` beside it — lines follow the song, clicking a line seeks, the panel
   scrolls itself and yields when you scroll it.
2. A track with an embedded `USLT` and no sidecar — plain text, no highlight, no seek.
3. A track with neither, online off — the *off* state names the switch.
4. Turn the switch on in Settings; the same track fetches, and `<data>/lyrics/` gains a `.lrc`.
5. A track LRCLIB doesn't have — a `.none` lands, and the second play makes no request.
6. Tune to a radio station — the column stays the station panel with the toggle on.
7. Toggle off and on from the 3-dot menu, close and reopen Now Playing, restart — the choice sticks.
8. `/usr/bin/time -v target/release/Melodia` once at the end, against the ~200 MB ceiling.

---

## Open

- Whether the `ONBOARDING_VERSION` bump ships with this feature or waits for a release that carries
  more than one card-worthy change (Phase 9).
- Word-level rendering. The parser drops `<mm:ss.xx>` rather than failing on it, so the timings are
  already in hand the day a karaoke sweep is wanted. What it would cost is a second animation
  running per word against a clock this feature only interpolates, which is a different problem
  from the one solved here.
- A per-track offset nudge, for the sheet that is right but half a second out. Cheap once
  `[offset:]` is honoured — the parser already applies one — but it needs somewhere to persist per
  track, and this feature has deliberately kept the database out of it (ADR 21).
