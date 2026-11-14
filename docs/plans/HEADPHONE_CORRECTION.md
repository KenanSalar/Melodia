# Headphone Correction

Working doc. Delete when the feature ships.

Status: **proposed** · Created: 2026-11-14

Issue [#144](https://github.com/KenanSalar/Melodia/issues/144).

> Verified **2026-11-14** against this tree at `78db3781`, the `biquad` 0.6.0 source
> (`src/coefficients.rs`), AutoEq's `results/README.md`, `results/INDEX.md` and `autoeq/peq.py`,
> Equalizer APO's configuration reference, and raw.githubusercontent.com's answers to a plain and
> a conditional GET.

---

## The symptom

Settings ▸ Effects has the Equalizer (ten sliders, nine presets, a Preamp) and ReplayGain. Neither
knows which headphones are plugged in, and every model colours the sound its own way, so a preset
ends up correcting the headphones as much as shaping the music, and a different pair needs a
different curve by hand. Switching output device changes nothing in the effects: the same curve
plays on every device.

## What ships

- A **Headphone correction** card on Settings ▸ Effects, above the Equalizer's, off by default.
- With it on, the card names the output device in use and the correction set for it, or that it
  has none.
- **Find headphones** opens a picker over AutoEq's recommended list, one entry per model, each row
  saying whose measurement it is. Apply fetches that model's profile once and keeps it.
- **Import** takes a file or pasted text in the parametric format AutoEq and autoeq.app write.
  Before applying, it lists every line it won't use and why; a file setting each ear separately,
  or holding more filters than a correction may, is refused whole.
- The card's switch turns correction off and on for a before and after, every device's correction
  kept.
- Each output device keeps its own correction, System default counting as one. Switching device
  in Melodia switches the correction with it, through a short ramp. Clear removes the current
  device's alone.
- **Find corrections online**, a switch of its own, off by default, gates every request to AutoEq.
  Import works with it off. The onboarding card's "What Melodia can reach" gains the same row.
- The level comes down by exactly the loudest point of the correction and the ten sliders
  together, recomputed as a slider moves, so a boost doesn't clip.
- The Output card's signal path names the correction as a stage, and Make Bit-Perfect turns it off.

**What keeps working:**

- With the correction off, the effects sound as they do today, and a flat equalizer with nothing
  else engaged is still a bit-identical passthrough.
- Picking a preset or moving a slider never touches the correction.
- The Preamp slider applies on top, as today, and the safety limiter stays where it is.
- Nothing reaches the network unless the online switch is on and the picker is opened or a model
  applied. No background task is added.
- Memory: the list is read from disk only while the picker is open, and released when it closes.

---

## Decisions

### The settled questions

- **A correction holds at most 20 filters** (`MAX_CORRECTION_FILTERS`). Ten covers every AutoEq
  profile (the issue's count); twenty leaves room for other tools' exports. The banks are fixed
  arrays of that size, and a filter costs nothing per sample until it is present. A file with more
  is refused, never cut.
- **The picker offers one measurement per model: AutoEq's recommended list**,
  `results/README.md` (6,033 models, 485,942 bytes, measured 2026-11-14), which keeps only the most
  accurate source for each. The full index (8,850 entries, one name listed up to 15 times) would
  need every row to name its rig as well as its source to be told apart; anything it holds reaches
  Melodia through Import. Each row's subtitle is the source, read off the entry's path, so a pick
  still says whose measurement it is.

### Where the stage sits

**Inside `EqSource`, per deck, before the ten bands and the limiter**
(`melodia-playback/src/player/playback/equalizer.rs:539-581`). The per-sample order becomes:
decoder sample × Preamp × ReplayGain × headroom, then the correction filters, then the active
bands, then the limiter, the clamp and the deck's fade. Both decks read one shared correction, as
they read one `EqShared` today.

- Not after the mixer: the limiter is per source, and the correction's boosts are what it has to
  catch. Moving the limiter would change today's sound.
- The correction has its own switch, so the Equalizer's toggle doesn't gate it, the way it
  doesn't gate ReplayGain (`rebuild`, `equalizer.rs:435-447`).
- `dsp_engaged` reports it, so the signal path grades it (`signal_path.rs:124`).

### Every filter runs in 64-bit

The ten bands and the correction run `biquad::DirectForm1<f64>`: one widening at the first filter,
one narrowing after the last. Form 1 stays, its delay line holding across a coefficient swap
(`equalizer.rs:1-3`). A filter's poles crowd towards z = 1 as its frequency falls against the
rate, a 105 Hz shelf at 192 kHz opening every AutoEq profile read for this plan, and that is where
32-bit coefficients and state stop holding the curve. The cascade is serially dependent at either
width, so nothing vectorised is given up; Phase 3's profile records what it costs.

- **No new filter crate.** `biquad` 0.6.0 is generic over the coefficient and sample types, and its
  `LowShelf`, `HighShelf` and `PeakingEQ` are the RBJ cookbook's forms with `alpha = sin(w0)/(2Q)`
  (`src/coefficients.rs:228-286`), the same formulas AutoEq optimises its filters against
  (`autoeq/peq.py`). A shelf defined any other way would draw another curve from the same file.
- A filter at or past the source's Nyquist is inactive, as a band is today (`equalizer.rs:463-470`).

### The level: headroom from the combined response

- **Headroom is the loudest point of the active correction filters and the active bands
  together**, evaluated at the source's own rate. `|H(e^jw)|²` of each biquad multiplies across the
  cascade at each point, so a rebuild takes one `log10` at the end and none per filter. The grid
  is fixed log spacing from 20 Hz to the lower of 20 kHz and the source's band edge
  (`GRID_STEPS_PER_OCTAVE`, a twenty-fourth to start), plus each active filter's own centre, where
  the peaks sit. The fixed points' cosines are built once per source in `EqSource::new`, which
  runs on the control thread (`backend/mod.rs:290`); a rebuild allocates nothing.
- The gain is `min(1, 1 / peak)`: the level only comes down. The file's `Preamp:` line is read and
  not applied.
- Only while a correction is engaged. With it off nothing is added, so today's sound holds.
- **A slider move takes the new headroom at once**, as the Preamp does today; only a correction
  swap ramps (below).
- The Preamp slider stays the user's, outside the headroom, and the limiter stays the net
  (`equalizer.rs:50-55`).

### A swap crossfades two banks

A swap is any change of the correction itself: an apply, a clear, the switch, a device switch.

- The stage holds two banks per channel. On a swap the current bank becomes the outgoing one and
  the incoming is built from rest; for `CORRECTION_SWAP` (50 ms to start, set by ear in Phase 3)
  both run and their outputs mix linearly, frame by frame. Linear, because the two are the same
  signal through two filters, where an equal-power curve would bump the level mid-ramp. Each bank
  carries its own headroom, so the level moves with the ramp.
- Coefficient interpolation is not the answer here: a swap changes the filter count and the
  filter types, and interpolating between two unrelated sets passes through curves neither asked
  for.
- A swap arriving mid-ramp is taken when the ramp ends, the poll leaving its generation unread
  until then.
- A new source (next track, a gapless successor, a seek's replacement) takes the set as it is,
  with no ramp. A seek resets both banks and ends a ramp at its target, as it resets the bands
  (`equalizer.rs:637-650`).
- Bypass needs the stage inert as well: no correction engaged and no ramp in flight
  (`equalizer.rs:490`).

### Publishing a set to the audio thread

`CorrectionShared` holds up to 20 filter cells, the switch and a sequence counter. The one writer,
the engine behind its correction lock, makes the counter odd, rewrites the cells, then makes it
even. A source reads the counter, the cells, then the counter again; an odd or moved count keeps
the set it has and tries at the next frame. The audio thread never spins and never allocates.

The `Generation` pattern alone (`dsp.rs:56-74`) isn't enough here: a reader landing during the
next write would build a set mixing two headphones and ramp to it.

### One correction per device

- **The key is the device the output is aimed at**, one home: `OutputChoice::heard_device()`
  (`backend/output.rs`), the claim's device in exclusive mode and `shared_device()` in shared
  (`:47-51`), which names a device on Windows and none on Linux. `None` is System default. A
  device's key is `OutputDevice.id`, stable across reboots and replugs
  (`output/request.rs:113-117`).
- **The engine selects, in one place.** `set_output_choice`, `set_corrections` and
  `set_correction_enabled` all end in `select_correction`, so the picker, boot and the bit-perfect
  switch all switch it the same way. `set_output_choice` selects once its reopen returns
  (`:265-290`), so the old device never plays the new device's correction.
- **The map:** `DeviceCorrections { system_default: Option<Correction>, devices: BTreeMap<String,
  Correction> }` in `melodia-core`, two fields rather than a sentinel key a device id could share.
  `Correction` carries its label, origin (an AutoEq model with its source, a file, a paste), the
  device's name when it was set (for display while unplugged) and `CorrectionFilters`, a newtype
  holding at most 20 finite filters in range, built through `TryFrom` (serde `try_from`), so a
  hand-edited `settings.json` can't hand the DSP a bad one.
- **Persisted in `settings.json`** as `CorrectionFlags { correction_enabled, correction_online,
  corrections }`, flattened and `serde(default)` like its siblings. Several writers (apply, import,
  clear) share one synchronous shadow on `AppState`, set before each persist, which writes the
  shadow's latest; the engine gets a copy at each write.
- The card learns of a device switch through a new `AppState.output_changed` `Signal`, bumped by
  `library::playback::player_set_output_choice`, the funnel every device change in the views goes
  through.

### Online, the way radio and lyrics are

- **`library::correction` is the facade, and the only place that names `services::net::autoeq`.**
  Its `autoeq_client` is `ensure_online(state)?` then `state.http_client()`, the shape of
  `library::radio`'s `directory_client` (`library/radio/mod.rs:51-63`), and the one read of the
  `correction_online` `SharedFlag`.
- **Where the files are:** `https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/`,
  the list `README.md` and a profile `<path>/<model> ParametricEQ.txt`, the model being the path's
  last segment. The base is parsed through `http_url`. A profile URL is the base joined with the
  entry's path, the file name pushed as one segment (`path_segments_mut`, which escapes it), and
  refused unless it stays under the base: a list entry is data off the network.
- **Conditional GET.** raw.githubusercontent.com tags the list with an `ETag` and answers
  `If-None-Match` with an empty 304 (checked 2026-11-14). The fetch goes through
  `get_capped_if_changed`, Phase 1's primitive, which the updater's manifest fetch shares.
- **No gzip.** reqwest's `gzip` feature is off in the workspace; the 79,067-byte compressed list
  would add a decompression crate to save a fetch that happens once per upstream change.
- **Caps and deadlines are named constants**: the list a few times today's size, a profile a few
  KiB, an import file the same order.

### The cached list

- On disk as `Paths.autoeq_list_path` (the list's text) and `Paths.autoeq_list_meta_path` (its
  `ETag` and when it was last checked), written through `atomic_file`, the list first.
- **A fresh copy replaces the cache only if it parses to at least one entry**, so a moved or
  broken upstream file never overwrites a working list.
- **Rechecked when the picker opens** and the last check is older than `LIST_RECHECK_AFTER` (a day
  to start). The cached list shows at once, the check runs behind it, and a changed list repaints
  the open picker.
- What the picker says: the list; "Couldn't reach AutoEq" with no cache and no network; "AutoEq's
  list couldn't be read" when what arrived parses to nothing. Both point at Import.
- **In memory only while open.** The parsed entries are held beside the picker's model and dropped
  in the dialog's `closed-teardown`, as the kept-out list is (`duplicates/kept_out.rs:217-226`).
  The model draws at most `PICKER_ROW_CAP` rows, the needle searching them all, as the radio facet
  picker does (`ui/radio/facets.rs:226-244`).

### Reading the list defensively

- An entry is a line `- [<name>](<path>)` whose path starts `./`. The link ends at the line's last
  `)`, since names and paths carry parentheses (2,048 of the list's paths on 2026-11-14), and the
  name ends at the first `](./`.
- Segments are percent-decoded (the list escapes spaces and UTF-8) and a path needs at least a
  source and a model; `..` is refused.
- A line that opens `- [` and doesn't parse is counted and logged at `debug`.
- Entries are `AutoEqEntry { name, source, path }` of `Box<str>` in `melodia-core`, so the views
  can name them without naming `melodia-net`.

### Reading a parametric file

The Equalizer APO syntax AutoEq writes, parsed in `melodia-app`
(`services/correction/parametric.rs`), shared by Import and by a fetched profile:

- `Filter[ n]: ON PK|PEQ|Modal|LSC|HSC Fc <hz> Hz Gain <db> dB Q <q>` is a filter.
- `Preamp: <db> dB` is read and not applied.
- Blank lines and `#` comments are skipped.
- Every other line is listed back with its reason: a filter switched `OFF`, a type the correction
  has no stage for (`LP`, `HP`, `BP`, `NO`, `AP`, the `x dB` slope and `6dB`/`12dB` shelves, a
  `BW Oct` width), a value out of range or not finite, or a line it can't read (`GraphicEQ:`
  among them).
- Refused whole, as typed variants: a `Channel:` line selecting anything but every channel (one ear
  set apart), more than 20 filters, no filter at all.
- **A fetched profile with any line listed is refused**, logged with the lines, rather than shown a
  review: AutoEq's layout isn't a promise, and a changed format must not apply half a curve. Import
  is the way in if it moves.

### Bit-perfect

Make Bit-Perfect turns the correction's switch off with the Equalizer and ReplayGain
(`library/playback.rs:529-545`), its corrections kept as the curve is, and `apply_bit_perfect_reset`
persists it (`library/settings/playback.rs:71-80`).

---

## What this does not do

- **System default is one device**: a change of the system's default output under it, a
  headphone plug on Linux or a default endpoint move on Windows, keeps System default's correction.
  On Linux in shared mode the sound always reaches the default, so whatever card is picked for
  exclusive, shared playback is corrected as System default.
- **An exclusive claim that falls back to shared** keeps the chosen device's correction.
- **No editing a correction's filters, no new targets, no drawn curve, no per-song equalizer**, and
  no per-ear correction.
- **Not in the overflow menu's Equalizer dialog**: the correction is set once per device, not mid
  listen.
- **No profile is cached beyond the one applied**, and nothing fetches in the background.

---

## Structure

| what | where | note |
|---|---|---|
| boundary types | `crates/melodia-core/src/entities/correction.rs` | `FilterKind`, `CorrectionFilter`, `CorrectionFilters` (newtype, `TryFrom`), `MAX_CORRECTION_FILTERS`, `Correction`, `CorrectionOrigin`, `DeviceCorrections::for_device`, `AutoEqEntry`, the parse result and its refusals |
| the stage | `crates/melodia-playback/src/player/playback/correction/` | `shared.rs`: `CorrectionShared` and its sequence counter. `bank.rs`: the per-channel banks and the swap ramp. `response.rs`: the grid, `\|H\|²` and the headroom |
| the wrapper | `player/playback/equalizer.rs` | `DspControls` in, the stage polled with the other two generations, the f64 bands, bypass and seek |
| engine | `crates/melodia-engine/src/player/engine/backend/{mod,controls,output}.rs`, `signal_path.rs` | `DspControls` owned once; the correction lock, the map, `select_correction`; `OutputChoice::heard_device`; `SignalInputs.correction_on` |
| net | `crates/melodia-net/src/services/net/mod.rs`, `services/net/autoeq/` | `get_capped_if_changed`; the URLs and their guard, the list fetch, the profile fetch, the list parser |
| parser | `crates/melodia-app/src/services/correction/parametric.rs` | the format above |
| settings | `crates/melodia-app/src/services/settings/playback.rs`, `library/settings/correction.rs` | `CorrectionFlags` beside `EqualizerFlags`; the setters through `mutate_settings` |
| facade | `crates/melodia-app/src/library/correction/` | `mod.rs`: `ensure_online`, `autoeq_client`, the shadow. `list.rs`: the cache, the recheck, the states. `apply.rs`: apply from the list, import a file or text, clear |
| app glue | `library/playback.rs`, `state/{mod,signal}.rs`, `crates/melodia-core/src/config.rs` | the player setters and Make Bit-Perfect; `hydrate_audio_dsp` seeds the map and switch before `set_output_choice` (`state/mod.rs:329-359`); `output_changed`; the two list paths |
| card | `crates/melodia-ui/ui/views/settings/correction-section.slint`, `components/settings/correction-controls.slint`, `globals/audio.slint` | `CorrectionText` shared by the card and both dialogs, the `HeadphoneCorrection` global |
| dialogs | `components/dialog/headphone-picker-body.slint`, `correction-import-body.slint`, `globals/dialog.slint`, `components/dialog/dialog.slint` | two kinds; the import's step in its own global |
| views | `crates/melodia-views/src/ui/correction/` | `mod.rs`: install, seed, repaint on `output_changed`. `picker.rs`: load, filter, apply, release. `import.rs`: file, paste, review |

`melodia-views` learns nothing about the network: it reads `AutoEqEntry` and the parse result from
`melodia-core` through `library::correction`. The DSP learns nothing about devices: the engine hands
it a set.

**The duplication this collapses first** (Phase 1):

- **`EqSource::new`'s engine-wide cells become one `DspControls`**, the Equalizer and ReplayGain
  now and the correction in Phase 2, so the one call site (`backend/mod.rs:426`) stops growing an
  argument per stage.
- **The updater's conditional fetch** (`services/updater/github.rs:40-63`: `If-None-Match`, the 304,
  the `ETag`, the capped read) becomes `get_capped_if_changed` beside `get_capped`, sharing its
  status and length checks; the AutoEq list is its second caller.
- **`PICKER_ROW_CAP`** moves out of `ui/radio/facets.rs` to `ui/row_match.rs`, its doc saying what
  both pickers share: a cap on the drawing, never on what the needle searches.
- **One filter precision**: the bands move to f64 with the stage, not beside it.

---

## Phase 1: Groundwork (no audible change)

- [ ] `DspControls` (EQ, ReplayGain) built once in the backend and handed to `EqSource::new`.
- [ ] The ten bands on `DirectForm1<f64>` with f64 coefficients, widened once and narrowed once.
      Bypass paths stay bit-identical; the active path moves in its last bits.
- [ ] `get_capped_if_changed` in `melodia-net`; the updater's manifest step on it. Its one change:
      the manifest fetch takes a named deadline where it relied on the client's per-read timeout.
- [ ] `PICKER_ROW_CAP` in `ui/row_match.rs`, the facet picker reading it there, its doc saying
      what both pickers share.

**Gate:**
- [ ] The static gates, and the existing tests fixed where `EqSource::new`'s signature breaks them.
- [ ] Kenan's hand pass: every preset and a hand curve sound as before; an update check against an
      unchanged release logs its 304; the radio Tag picker draws and narrows as before.

## Phase 2: The correction stage (`melodia-core`, `melodia-playback`)

- [ ] `entities/correction.rs`: the types, the newtype's checks, `MAX_CORRECTION_FILTERS`.
- [ ] `correction/shared.rs`: the cells, the counter, `publish` (writer) and `try_read` (source).
- [ ] `correction/response.rs`: the grid built per source, `|H|²`, the headroom over the correction
      and the bands.
- [ ] `correction/bank.rs`: building a bank from a set at a rate, inactive past Nyquist; the two
      banks and the linear ramp over `CORRECTION_SWAP`; a mid-ramp swap waits; seek resets.
- [ ] `EqSource`: the stage in `DspControls`, its generation polled at the frame boundary with the
      other two, the headroom in the input gain, bypass and `dsp_engaged` counting it.
- [ ] Docs: the stage's order and its why at `equalizer.rs`'s `//!` and `correction/`'s;
      `.claude/rules/unsafe-rust.md`'s hot-path lines ("ten cascaded biquads", `next_active`'s
      zipped loops) where the stage makes them wrong. ADR 0010's shape (one wrapper, lock-free
      state) holds, so it stays.

**Gate:**
- [ ] The static gates. Nothing sets a correction yet, so the run gates wait for Phase 3.

## Phase 3: One correction per device (`melodia-engine`, `melodia-app`)

- [ ] `OutputChoice::heard_device`; the engine's map, `set_corrections`, `set_correction_enabled`,
      `select_correction`, and `set_output_choice` selecting after its reopen.
- [ ] `SignalInputs.correction_on`, fed where `eq_on` and `rg_on` are.
- [ ] `CorrectionFlags` and its setters; the `AppState` shadow; `output_changed`;
      `player_set_corrections`, `player_set_correction_enabled`; `hydrate_audio_dsp`.
- [ ] Make Bit-Perfect and `apply_bit_perfect_reset` turn it off.
- [ ] Lock order: the correction lock is a leaf, taken after the decks and output guards of a
      reopen have dropped, and holding nothing else.

**Gate:**
- [ ] The static gates.
- [ ] In the dev data folder with a backup, corrections written into `settings.json` by hand from
      two AutoEq profiles, one for a card and one for System default (Kenan's hand pass, or run by
      Claude on his go):
  - [ ] the switch on: the signal path names the stage and the sound changes; a big bass boost on a
        loudly mastered track neither clips nor pumps;
  - [ ] switching device mid-track switches the correction, with no click and no jump in level;
        `CORRECTION_SWAP` settled by ear and recorded here;
  - [ ] a device with nothing set plays uncorrected; System default plays its own;
  - [ ] the switch off: exclusive output reads Bit-perfect with the Equalizer off, and a flat
        Equalizer is bit-identical (`bit_perfect.rs`'s rig);
  - [ ] Make Bit-Perfect turns the correction off and keeps both corrections;
  - [ ] the curve: sines at 44.1, 96 and 192 kHz through the null sink, read back per tone against
        the analytic response of the correction plus the bands, every tone within 0.1 dB, the
        table recorded here (the issue's 192 kHz check);
  - [ ] the rebuild on the audio thread with a 20-filter correction at 192 kHz during a slider
        drag: `perf record` on the output thread, `rebuild`'s share recorded here.

## Phase 4: AutoEq online (`melodia-net`, `melodia-app`)

- [ ] `services/net/autoeq/`: the base, the list URL, the profile URL and its guard; the
      conditional list fetch; the profile fetch; the list parser and its skip count.
- [ ] `services/correction/parametric.rs`, its refusals typed.
- [ ] `library/correction/`: the online seam, the cache and its recheck, apply from the list,
      import a file or text, clear; each write through the shadow, the engine, then the persist,
      kicking only after the write commits.
- [ ] `correction_online` as a `SharedFlag` with its setter; the two `Paths` entries, in
      `rooted_at`.
- [ ] Docs: `CLAUDE.md`'s Facades gains "Nothing outside `library::correction` names `autoeq`";
      the facade's `//!` argues the seam, as `library/radio/mod.rs:8-10` does.

**Gate:**
- [ ] The static gates. The card is the only caller, so the run gates wait for Phase 5.

## Phase 5: The card, the picker and the import (`melodia-ui`, `melodia-views`)

- [ ] The card above `EqualizerSection` (`effects-page.slint`, its "two stages" comment made
      three): the switch; while on, the device and its correction with Find headphones, Import and
      Clear; the online switch, its description naming github.com as the lyrics row names
      lrclib.net. Every row's label and description declared once for `row-visible`.
- [ ] The picker: a `SearchBar` over a `ListView` with `OverlayScrollbar` and drag-pan off, rows of
      name and source, filtered in Rust through `Needle`, Apply, the three states, released on
      close.
- [ ] The import: one dialog kind with two steps, its step in its own global (paste, then review);
      a file skips to review. A raise after the file dialog's await owes a `DialogClaim`.
- [ ] `MultilineInput` gains the right-click edit menu `SingleLineInput` has
      (`single-line-input.slint:96-103`): a paste box with no Paste entry is the obvious gap. It is
      the tree's one wrapping box, so the tag editor's lyrics tab gains the menu too.
- [ ] The onboarding row, its label and description the Settings row's.
- [ ] The signal path panel names the correction.
- [ ] `headphones` (and any other new ligature) into `scripts/icons.txt`, the re-subset and a clean
      `scripts/check-icons.py`.
- [ ] Strings through `@tr`, counts as plurals; `scripts/update-translations.py` and all fifteen
      catalogs finished.
- [ ] Docs: `README.md`'s effects lines; `.claude/rules/ui-patterns.md`'s catalogue if the picker
      body is shaped for reuse.

**Gate:**
- [ ] The static gates, and the onboarding and settings tests fixed where the new rows break them.
- [ ] Kenan's hand pass (or run by Claude on his go), in the dev data folder:
  - [ ] the picker finds a model by part of its name, applies it, and the card shows it; a second
        open with nothing changed sends one conditional request;
  - [ ] the picker offline: with a cache it lists, without one it says it couldn't reach AutoEq;
        a hand-corrupted cache says the list couldn't be read; Import works either way;
  - [ ] importing a file with lines it can't use lists them before applying; a per-ear file and a
        30-filter file are refused; a paste applies;
  - [ ] the switch gives a before and after with no click; Clear on one device leaves the other;
  - [ ] the online switch off: Find headphones is unavailable and nothing reaches GitHub, Import
        still applies; the onboarding row flips the same switch;
  - [ ] the settings search finds the card by "headphone", "correction" and "AutoEq".

## Phase 6: Windows and memory

- [ ] Windows (on Kenan's go): it builds; switching between two endpoints in shared mode switches
      the correction; System default plays its own; whether a shared stream on System default
      follows a change of the Windows default, recorded here as the answer to the issue's open
      note.
- [ ] Memory: RSS sampled from `/proc` with the picker open on the full list and two minutes after
      it closes, against a run that never opened it; idle after close within noise, recorded here.

## Phase 7: Tests

Tests are added where the landed change earns them, on Kenan's go, after the hand passes above.
Existing tests a phase breaks are fixed in that phase. The facade's corpus walk lands here, and
`CLAUDE.md`'s facade line then names its file.

---

## Cross-cutting

- **Docs land in the phase that stales them**, each argument above moving to the doc comment of
  what it constrains as that lands.
- **800 lines:** `equalizer.rs` (663) gains wiring, not the stage; if it passes 760 the limiter
  moves to its own file first. `backend/mod.rs` (671) loses two fields to `DspControls`.
  `app-window.slint` (750) gains its export lines only. `library/playback.rs` (551) gains two
  setters. Everything else new is its own module.
- **Memory:** two banks of 20 f64 filters per channel per source, a few KiB; the list only while
  the picker is open. Phase 6 measures it.
- **Lock order** (ADR 0035) is untouched: the correction lock is a leaf, and the audio thread reads
  through the counter, never a lock.
- **The live public release:** both switches default off, every new field is `serde(default)`, and
  there is no migration.

## Open questions

- Whether System default should key by the endpoint Windows resolves it to, which Phase 6's answer
  decides is worth asking.
- Whether the full index joins the picker later, each row then naming its rig.
