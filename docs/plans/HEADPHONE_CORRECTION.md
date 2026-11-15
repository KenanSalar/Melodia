# Headphone Correction

Working doc. Delete when the feature ships.

Status: **Phase 6, hand tests done but Windows; Tests next** · Created: 2026-11-14

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
- **Import** opens a paste box with a Choose File chip beside it, for the parametric format AutoEq
  and autoeq.app write. Before applying, it lists every line it won't use and why; a file setting
  each ear separately, or holding more filters than a correction may, is refused whole.
- The card's switch turns correction off and on for a before and after, every device's correction
  kept.
- Each output device keeps its own correction, System default counting as one. Switching device
  in Melodia switches the correction with it, through a short ramp. Clear removes the current
  device's alone and offers Undo, which Settings ▸ Accessibility can turn off as its own kind.
- **Find corrections online**, a switch of its own, off by default, gates every request to AutoEq.
  Import works with it off. On the card it shows only while the correction is on, as Radio's rows
  do, keeping its value while hidden. The onboarding card's "What Melodia can reach" gains the
  same row, shown whatever the correction's switch says, as the lyrics lookup's is.
- The curves live in `eq.json`, the switches over them in `settings.json` with the other
  preferences: every device's correction, and the equalizer's bands, preset and preamp, moved out
  of a `settings.json` older than it at the first boot.
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
(`melodia-playback/src/player/playback/equalizer.rs:578-626`). The per-sample order is: decoder
sample × Preamp × ReplayGain, then the correction filters with each bank's output scaled by its
headroom, then the active bands, then the limiter, the clamp and the deck's fade. Both decks read
one shared correction, as they read one `EqShared`.

- Not after the mixer: the limiter is per source, and the correction's boosts are what it has to
  catch. Moving the limiter would change today's sound.
- The correction's switch is the engine's, so the Equalizer's toggle doesn't gate it, the way it
  doesn't gate ReplayGain (`rebuild`, `equalizer.rs:457-471`).
- `dsp_engaged` reports it, so the signal path grades it (`signal_path.rs:124`).

### Every filter runs in 64-bit

The ten bands and the correction run `biquad::DirectForm1<f64>`: one widening at the first filter,
one narrowing after the last. Form 1 stays, its delay line holding across a coefficient swap
(`equalizer.rs:1-3`). A filter's poles crowd towards z = 1 as its frequency falls against the
rate, a 105 Hz shelf at 192 kHz opening every AutoEq profile read for this plan, and that is where
32-bit coefficients and state stop holding the curve. The cascade is serially dependent at either
width, so nothing vectorised is given up; the hand tests' profile records what it costs.

- **No new filter crate.** `biquad` 0.6.0 is generic over the coefficient and sample types, and its
  `LowShelf`, `HighShelf` and `PeakingEQ` are the RBJ cookbook's forms with `alpha = sin(w0)/(2Q)`
  (`src/coefficients.rs:228-286`), the same formulas AutoEq optimises its filters against
  (`autoeq/peq.py`). A shelf defined any other way would draw another curve from the same file.
- A filter at or past the source's Nyquist is inactive, as a band is (`equalizer.rs:499-506`).

### The level: headroom from the combined response

- **Headroom is the loudest point of the active correction filters and the active bands
  together**, evaluated at the source's own rate. `|H(e^jw)|²` of each biquad multiplies across the
  cascade at each point, each filter's divided out on its own so the product can't underflow, and
  a rebuild ends in one square root. The grid is fixed log spacing from 20 Hz to the lower of
  20 kHz and the source's band edge (`GRID_STEPS_PER_OCTAVE`, a twenty-fourth to start), the edge
  itself, and each active filter's own centre clamped into that band, where the peaks sit. The
  fixed points' cosines are built once per source in `EqSource::new`, which runs on the control
  thread (`backend/mod.rs:287-288`); a rebuild allocates nothing.
- The gain is `min(1, 1 / sqrt(peak power))`: the level only comes down. Each bank's output takes
  its own, which equals scaling the input and lets a swap's mix carry both levels. The file's
  `Preamp:` line is read and not applied.
- Only while a correction is engaged. With it off nothing is added, so today's sound holds.
- **A slider move takes the new headroom at once**, as the Preamp does today; only a correction
  swap ramps (below).
- The Preamp slider stays the user's, outside the headroom, and the limiter stays the net
  (`equalizer.rs:53-58`).

### A swap crossfades two banks

A swap is any change of the correction itself: an apply, a clear, the switch, a device switch.

- The stage holds two banks per channel. On a swap the current bank becomes the outgoing one and
  the incoming is built from rest; for `CORRECTION_SWAP_MS` (50 ms to start, set by ear in the
  hand tests) both run and their outputs mix linearly, frame by frame. Linear, because the two are
  the same signal through two filters, where an equal-power curve would bump the level mid-ramp.
  Each bank carries its own headroom, so the level moves with the ramp.
- Coefficient interpolation is not the answer here: a swap changes the filter count and the
  filter types, and interpolating between two unrelated sets passes through curves neither asked
  for.
- A swap arriving mid-ramp is taken when the ramp ends, the poll leaving its sequence unread
  until then. A swap between two banks with nothing running takes no ramp.
- A new source (next track, a gapless successor, a seek's replacement) takes the set as it is,
  with no ramp. A seek resets the current bank and ends a ramp at its target, as it resets the bands
  (`equalizer.rs:682-696`).
- Bypass needs the stage inert as well: no correction engaged and no ramp in flight
  (`equalizer.rs:469-470`).

### Publishing a set to the audio thread

`CorrectionShared` holds up to 20 filter cells and a sequence counter, and no switch: the engine
publishes an empty set while correction is off, so the stage knows only the set to play.
`CorrectionWriter`, not `Clone` and publishing through `&mut self`, is the one handle that writes;
the engine keeps it inside its correction lock. A write makes the counter odd, rewrites the cells,
then makes it even. A source reads the counter, the cells, then the counter again; an odd or moved
count keeps the set it has and tries at the next frame. The audio thread never spins and never
allocates.

The `Generation` pattern alone (`dsp.rs:62-80`) isn't enough here: a reader landing during the
next write would build a set mixing two headphones and ramp to it.

### One correction per device

- **The key is the device the output is aimed at**, one home: `OutputChoice::heard_device()`
  (`backend/output.rs`), the claim's device in exclusive mode and `shared_device()` in shared
  (`:47-51`), which names a device on Windows and none on Linux. `None` is System default. A
  device's key is `OutputDevice.id`, stable across reboots and replugs
  (`output/request.rs:113-117`).
- **The engine selects, in one place**: `CorrectionControl::select` (`backend/correction.rs`),
  behind the engine's correction lock. `set_corrections`, `set_correction_enabled` and the aim
  ending `set_output_choice` all end in it, so the picker, boot and the bit-perfect switch switch
  it the same way. `set_output_choice` aims once its reopen returns (`backend/output.rs:277-314`),
  so the old device doesn't play the new device's correction as it closes, and reads the choice
  under the correction lock, so two switches crossing land on the newer.
- **The map:** `DeviceCorrections { system_default: Option<Correction>, devices: BTreeMap<String,
  Correction> }` in `melodia-core`, two fields rather than a sentinel key a device id could share.
  `Correction` carries its origin, which names it (an AutoEq model with its source, a file by its
  name, a paste, which the card names in Slint since `@tr` takes literals only), the device's name
  when it was set (for display while unplugged) and `CorrectionFilters`, a newtype holding at most
  20 finite filters in range, built through `TryFrom` (serde `try_from`), so a hand-edited
  `eq.json` can't hand the DSP a bad one. A bad one fails the `corrections` key alone, which the
  loader resets and logs (`utils/atomic_file.rs:36-66`).
- **Persisted in `eq.json`, its switches in `settings.json`.** `eq.json` (`services::settings::eq`)
  holds the curves: the map as `corrections`, and the equalizer's `band_gains`, `preset` and
  `preamp`. Its read-mutate-write is `settings.json`'s, one `LockedFile` each (`settings/io.rs`).
  The switches stay prefs, `CorrectionFlags { correction_enabled, correction_online }` beside
  `EqualizerFlags { eq_enabled }`, so Make Bit-Perfect stays one write. Apply, import and clear
  share one synchronous shadow, `AppState.corrections`, a `Shadowed` (`state/signal.rs`, the Output
  card's too) edited before each persist; `library::correction::commit` is the one writer, its
  `apply_latest` handing the engine and the file the map as it stands. Nothing reads the map off
  disk, so nothing is kicked after the write.
- **The equalizer's curve has shipped in `settings.json`, so boot moves it**
  (`move_legacy_eq`): `LegacyEq` reads the old keys, `eq.json` is written from them, and only
  then are they dropped, so a failed write leaves them for the next boot. It runs ahead of the
  engine's seed, which a `one_shot` sweep would come too late for. An `eq.json` already there
  wins.
- **A write's device comes from the caller** (`CorrectionTarget`), as the card named it when the
  user acted, so a profile still downloading as the output moves lands where it was asked for.
- **The card names the device the engine heard**, never the Output card's copy, which may not
  have listed a device this run: `PlaybackEngine::heard_device` reads `CorrectionControl`'s, and
  `library::correction::heard` (blocking) names it through `output_devices()`, an unlisted device
  by its correction's stored name. System default lists nothing.
- **`AppState.output_changed` is bumped inside `player_set_output_choice`**, after the engine
  call returns and so after the aim, carried on `PlaybackContext` for it. Only
  `output_settings.rs`' `apply` and its bit-perfect switch change the choice; boot calls the
  engine before any subscriber exists.

### Online, the way radio and lyrics are

- **`library::correction` is the facade, and the only place that names `services::net::autoeq`.**
  Its `autoeq_client` checks the `correction_online` `SharedFlag`, then hands out
  `state.http_client()`, the shape of `library::radio`'s `directory_client`
  (`library/radio/mod.rs:51-63`), and is the flag's one read. The check has no other caller, so
  there is no `ensure_online` beside it.
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
  KiB, an import the same order, file and paste alike.

### The cached list

- On disk as `Paths.autoeq_list_path` (the list's text) and `Paths.autoeq_list_meta_path` (its
  `ETag` and when it was last checked), written through `atomic_file`, the list first.
- **A fresh copy replaces the cache only if it parses to at least one entry**, so a moved or
  broken upstream file never overwrites a working list.
- **Rechecked when the picker opens** and the last check is older than `LIST_RECHECK_AFTER` (a day
  to start). The cached list shows at once, the check runs behind it, and a changed list repaints
  the open picker.
- What the picker says: the list; "Couldn't reach AutoEq" with no cache and no network; "AutoEq's
  list couldn't be read" when what arrived, or a cache with no answer behind it, parses to
  nothing. Both point at Import. `recheck_list`'s `ListAnswer` carries the four outcomes, a
  failed recheck behind a usable cache answering `Unchanged`.
- **In memory only while open.** The parsed entries are held beside the picker's model and dropped
  in the dialog's `closed-teardown`, as the kept-out list is (`duplicates/kept_out.rs:217-226`).
  The model draws at most `PICKER_ROW_CAP` rows, the needle searching them all, as the radio facet
  picker does (`ui/radio/facets.rs:226-244`).

### Reading the list defensively

- An entry is a line `- [<name>](<path>)` whose path starts `./`. The link ends at the line's last
  `)`, since names and paths carry parentheses (2,048 of the list's paths on 2026-11-14), and the
  name ends at the first `](./`.
- Segments are percent-decoded (the list escapes spaces and UTF-8) and a path needs at least a
  source and a model; an empty, `.` or `..` segment, or one decoding to a `/`, is refused.
- A line that opens `- [` and doesn't parse is left out, the count logged once at `debug` with the
  first such line.
- Entries are `AutoEqEntry { name, source, path }` of `Box<str>` in `melodia-core`, so the views
  can name them without naming `melodia-net`.

### Reading a parametric file

The Equalizer APO syntax AutoEq writes, parsed beside its facade
(`library/correction/parametric.rs`, as `library/lyrics/lrc.rs` is: `services/` holds what needs
the database or the app state, and the parser needs neither), shared by Import and by a fetched
profile:

- `Filter[ n]: ON PK|PEQ|Modal|LSC|HSC Fc <hz> Hz Gain <db> dB Q <q>` is a filter.
- `Preamp: <db> dB` is read and not applied.
- Blank lines and `#` comments are skipped.
- Every other line is listed back with its reason: a filter switched `OFF`, a type the correction
  has no stage for (`LP`, `HP`, `BP`, `NO`, `AP`, the `x dB` slope and `6dB`/`12dB` shelves, a
  `BW Oct` width), a value out of range or not finite, or a line it can't read (`GraphicEQ:`
  among them).
- Refused whole, as typed variants: a `Channel:` line selecting anything but every channel (one ear
  set apart), more than 20 filters, no filter at all.
- An import past `IMPORT_MAX_BYTES`, file or paste, is `ImportError::TooLarge`. A file is decoded
  lossily, so a comment in another codepage doesn't refuse it; a stray byte in a filter line leaves
  that line unreadable, which the review lists.
- **A fetched profile with any line listed is refused**, logged with the lines, rather than shown a
  review: AutoEq's layout isn't a promise, and a changed format must not apply half a curve. Import
  is the way in if it moves. The refusal is an `AppError::Validation`, which the card tells from a
  `Network` failure and the switch's `Settings` refusal by variant.

### Bit-perfect

Make Bit-Perfect turns the correction's switch off with the Equalizer and ReplayGain
(`library/playback.rs:538-555`), its corrections kept as the curve is, and `apply_bit_perfect_reset`
persists it (`library/settings/playback.rs:71-81`).

---

## What this does not do

- **System default is one device**: a change of the system's default output under it, a
  headphone plug on Linux or a default endpoint move on Windows, keeps System default's correction.
  On Linux in shared mode the sound always reaches the default, so whatever card is picked for
  exclusive, shared playback is corrected as System default.
- **A stand-in for the chosen device keeps its correction**: an exclusive claim falling back to
  shared, a missing or refusing shared device on Windows with the default playing instead, and a
  reopen refused whole, the previous device carrying on.
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
| the stage | `crates/melodia-playback/src/player/playback/correction/` | `shared.rs`: `CorrectionShared`, its sequence counter and `CorrectionWriter`. `bank.rs`: `CorrectionStage`, the per-channel banks and the swap ramp. `response.rs`: the grid, `\|H\|²` and the headroom |
| the wrapper | `player/playback/equalizer.rs` | `DspControls` in, the stage polled with the other two generations, the f64 bands, bypass and seek |
| engine | `crates/melodia-engine/src/player/engine/backend/{mod,correction,output}.rs`, `signal_path.rs` | `DspControls` owned once; `correction.rs`: `CorrectionControl` (the writer, the map, the switch, the heard device) and its `select`; `OutputChoice::heard_device` and the aim; `SignalInputs.correction_on` |
| net | `crates/melodia-net/src/services/net/mod.rs`, `services/net/autoeq/` | `get_capped_if_changed`; `autoeq/mod.rs`: the URLs and their guard, the list fetch, the profile fetch. `autoeq/list.rs`: the list parser |
| settings | `crates/melodia-app/src/services/settings/{playback,eq,io}.rs`, `library/settings/correction.rs` | `CorrectionFlags` beside `EqualizerFlags`, the switches; `eq.rs`'s `EqData` for `eq.json`, the curves, and `move_legacy_eq`; `io.rs`'s `LockedFile` both files share; the setters through `mutate_settings` and `mutate_eq` |
| facade | `crates/melodia-app/src/library/correction/` | `mod.rs`: `autoeq_client`, `CorrectionTarget`, `heard`, `correction_for`, `commit` (the map's one writer), `clear`, `restore` (Clear's undo). `list.rs`: the cache, the recheck, `ListAnswer`. `apply.rs`: apply from the list, import a file or a paste, apply. `parametric.rs`: the format above |
| app glue | `library/playback.rs`, `state/{mod,signal}.rs`, `crates/melodia-core/src/config.rs` | the player setters and Make Bit-Perfect; `hydrate_audio_dsp` seeds the engine's map and switch (`state/mod.rs:337-370`); `Shadowed`, `AppState.corrections` and `correction_online`; `output_changed`; the two list paths |
| card | `crates/melodia-ui/ui/views/settings/correction-section.slint`, `components/settings/correction-controls.slint`, `globals/audio.slint` | `CorrectionText` shared by the card and both dialogs, the `HeadphoneCorrection` global |
| dialogs | `components/dialog/headphone-picker-body.slint`, `correction-import-body.slint`, `globals/correction-dialogs.slint`, `globals/dialog.slint`, `components/dialog/dialog.slint` | two kinds; `HeadphonePicker` and `CorrectionImport`, the import's step in the second |
| views | `crates/melodia-views/src/ui/correction/`, `ui/dialog_landing.rs` | `mod.rs`: install, seed, repaint on `output_changed`, Clear and its undo. `picker.rs`: load, filter, apply, release. `import.rs`: file, paste, review. `dialog_landing.rs`: the ticket both dialogs and the duplicates pair land through |
| undo | `crates/melodia-core/src/entities/undo.rs`, `melodia-views/src/ui/undo/removal.rs`, `undo-section.slint` | `UndoKind::Correction`, `Removal::Correction`, its Accessibility row |

`melodia-views` learns nothing about the network: it reads `AutoEqEntry` and the parse result from
`melodia-core` through `library::correction`. The DSP learns nothing about devices: the engine hands
it a set.

**The duplication this collapses first** (Phase 1):

- **`EqSource::new`'s engine-wide cells become one `DspControls`**, the Equalizer and ReplayGain
  now and the correction in Phase 2, so the one call site (`backend/mod.rs:424`) stops growing an
  argument per stage.
- **The updater's conditional fetch** (`services/updater/github.rs`: `If-None-Match`, the 304, the
  `ETag`, the capped read) becomes `get_capped_if_changed` beside `get_capped`, sharing its
  status and length checks; the AutoEq list is its second caller. The body's name, deadline and
  cap travel as one `CappedGet`, which every `get_capped` caller declares as a `const`, so no
  fetch function takes more than four parameters.
- **`PICKER_ROW_CAP`** moves out of `ui/radio/facets.rs` to `ui/row_match.rs`, its doc saying what
  both pickers share: a cap on the drawing, never on what the needle searches.
- **One filter precision**: the bands move to f64 with the stage, not beside it.
- **The dialog ticket** (Phase 5): `DialogTicket` and `TicketedDialog` move out of
  `ui/duplicates/landing.rs` to `ui/dialog_landing.rs`, the paged-row fetch staying behind as
  `DialogRows`, so the picker and the import land as the duplicates pair do.

---

## Phase 1: Groundwork (no audible change)

- [x] `DspControls` (EQ, ReplayGain) built once in the backend and handed to `EqSource::new`.
- [x] The ten bands on `DirectForm1<f64>` with f64 coefficients, widened once and narrowed once.
      Bypass paths stay bit-identical; the active path moves in its last bits.
- [x] `get_capped_if_changed` in `melodia-net`, `get_capped` and `get_capped_text` moved onto
      `CappedGet` with their radio callers; the updater's manifest step on it. Its one change: the
      manifest fetch takes a named deadline where it relied on the client's per-read timeout.
- [x] `PICKER_ROW_CAP` in `ui/row_match.rs`, the facet picker reading it there, its doc saying
      what both pickers share.

**Gate:**
- [x] The static gates, and the existing tests fixed where `EqSource::new`'s signature breaks them.

## Phase 2: The correction stage (`melodia-core`, `melodia-playback`)

- [x] `entities/correction.rs`: `MAX_CORRECTION_FILTERS`, the three value ranges (about twice
      AutoEq's own optimiser bounds), `FilterKind`, `CorrectionFilter` and its `check`, the
      `CorrectionFilters` newtype through `TryFrom`, and their two error types. The rest of the
      table's row lands with its first user.
- [x] `correction/shared.rs`: the cells, the counter, `CorrectionWriter` (`publish`, `clear`) and
      `read_if_changed` (source).
- [x] `correction/response.rs`: the grid built per source, `|H|²`, the headroom over the correction
      and the bands.
- [x] `correction/bank.rs`: building a bank from a set at a rate, inactive past Nyquist; the two
      banks and the linear ramp over `CORRECTION_SWAP_MS`; a mid-ramp swap waits; seek resets.
- [x] `EqSource`: the stage in `DspControls`, its sequence polled at the frame boundary with the
      other two generations, the headroom at each bank's output, bypass and `dsp_engaged` counting
      it. `dsp::PASSTHROUGH` replaces the private identity coefficients.
- [x] Docs: the stage's order and its why at `equalizer.rs`'s, `playback/mod.rs`'s and
      `correction/`'s `//!`; `.claude/rules/unsafe-rust.md`'s hot-path lines. ADR 0010's shape
      (one wrapper, lock-free state) holds, so it stays.

**Gate:**
- [x] The static gates, the existing equalizer suite passing unchanged once its `DspControls`
      literals take `..DspControls::default()`.

## Phase 3: One correction per device (`melodia-engine`, `melodia-app`)

- [x] `entities/correction.rs`: `Correction`, `CorrectionOrigin`, `DeviceCorrections::for_device`,
      and the serde derives (`CorrectionFilters` through `try_from`).
- [x] `OutputChoice::heard_device`; `backend/correction.rs`'s `CorrectionControl` and its
      `select`, publishing only when the set heard changes, since the same set again would still
      crossfade into a bank starting from rest; the engine's `set_corrections` and
      `set_correction_enabled`; `set_output_choice` aiming after its reopen. The backend builds
      `DspControls` around the writer's `shared()` rather than through `default()`.
- [x] `SignalInputs.correction_on`, fed where `eq_on` and `rg_on` are.
- [x] `CorrectionFlags { correction_enabled, corrections }` and its setters;
      `player_set_corrections`, `player_set_correction_enabled`; `hydrate_audio_dsp`.
- [x] Make Bit-Perfect and `apply_bit_perfect_reset` turn it off.
- [x] Lock order: the correction lock is taken after the decks and output guards of a reopen have
      dropped, and only the output choice's is taken under it.

The `AppState` shadow and `correction_online` moved to Phase 4 and `output_changed` to Phase 5,
each landing with its first user.

**Gate:**
- [x] The static gates, `signal_path_tests.rs`'s inputs taking the new field.

## Phase 4: AutoEq online (`melodia-net`, `melodia-app`)

- [x] `services/net/autoeq/`: the base, the list URL, the profile URL and its guard; the
      conditional list fetch; the profile fetch; the list parser, its skip count logged.
      `percent-encoding` joins the workspace, already in the lock through `url`.
- [x] `entities/correction.rs`: `AutoEqEntry`, the parse result, `DeviceCorrections::insert` and
      `remove`.
- [x] `library/correction/parametric.rs`, its refusals typed, each filter line through
      `CorrectionFilter::check`.
- [x] The map's shadow on `AppState`. First, the Output card's `Shadowed` and its `apply_latest`
      (`melodia-views`' `ui/settings/output_settings.rs`) move into `melodia-app` and both
      share it: the value under one lock and the apply under another, so an older write can't
      land after a newer. The two tests on its locks move with it.
- [x] `library/correction/`: the online seam, the cache and its recheck, apply from the list,
      import a file or a paste, clear; each write through `commit`.
- [x] `correction_online`: its `CorrectionFlags` field, a `SharedFlag` and its setter; the two
      `Paths` entries, in `rooted_at`.
- [x] Docs: `CLAUDE.md`'s Facades gains "nothing outside `library::correction` names
      `services::net::autoeq`" (the `Paths` fields hold the bare word); the facade's `//!` argues
      the seam, as `library/radio/mod.rs:8-10` does.

**Gate:**
- [x] The static gates, and the three `Shadowed` tests passing where the move left them.

## Phase 5: The card, the picker and the import (`melodia-ui`, `melodia-views`)

- [x] The card above `EqualizerSection` (`effects-page.slint`, its "two stages" comment made
      three): the switch; while on, the device and its correction with Find headphones, Import and
      Clear; the online switch, its description naming github.com as the lyrics row names
      lrclib.net. Every row's words declared once in `CorrectionText`.
- [x] The picker: a `SearchBar` over a `ListView` with `OverlayScrollbar` and drag-pan off, rows of
      name and source, filtered in Rust through `Needle`, Apply, the three states, released on
      close, a fetch still running aborted by the close.
- [x] The import: one dialog kind with two steps, its step in its own global (paste, then review).
      The paste step's Choose File chip lands a file on the review through the dialog ticket:
      `DialogClaim` holds only while no dialog is open, so it can't guard an answer for one up.
- [x] `MultilineInput` gains the right-click edit menu `SingleLineInput` has
      (`single-line-input.slint:96-103`): a paste box with no Paste entry is the obvious gap. It is
      the tree's one wrapping box, so the tag editor's lyrics tab gains the menu too.
- [x] `AppState.output_changed`, bumped in `player_set_output_choice`, repainting the card; the
      engine's `heard_device` and the facade's `heard` naming what it shows.
- [x] Clear offers Undo, `UndoKind::Correction` with its own Accessibility switch.
- [x] `show_bit_perfect_reset` (`ui/settings/signal_path.rs:47-53`) turns the card's switch off
      with the Equalizer's.
- [x] The onboarding row, its label and description `CorrectionText`'s.
- [x] The signal path panel names the correction, the Effects row naming up to three effects in
      the order the samples meet them.
- [x] `headphones` into `scripts/icons.txt`, the re-subset and a clean `scripts/check-icons.py`.
- [x] Strings through `@tr`, counts as plurals; `scripts/update-translations.py` and all fifteen
      catalogs finished.
- [x] Docs: `README.md`'s effects, undo and data-folder lines and its acknowledgments;
      `.claude/rules/ui-patterns.md`'s contract for an answer landing on a dialog already up. The
      picker body is the correction's own, so the catalogue gains no entry.
- [x] The curves into `eq.json`: `EqData` and its `LockedFile` beside `settings.json`'s, the
      shipped equalizer keys moved at boot by `move_legacy_eq`, the switches left in
      `settings.json`. `CLAUDE.md`'s state-file line and README's data-folder table name it.
- [x] The online row mounts only while the correction is on, keeping its value.

**Gate:**
- [x] The static gates, and the onboarding and settings tests fixed where the new rows break them.
      fmt, clippy, `scripts/check-icons.py` and the whole suite with the catalogs pass; the
      bit-perfect reset's test reads the curve from `eq.json`.

## Phase 6: Testing

Every phase before this one stops at its static gates. The hand tests come first, once the whole
feature has landed; the tests follow them.

### Hand tests

Kenan's hand pass, or run by Claude on his go, in the dev data folder with a backup.

**Groundwork:**
- [x] With the correction off, every preset and a hand curve sound as before.
      2026-11-15, `main` (`78db3781`) against this build, each preset on a 0 dBFS master at 48 kHz:
      Flat bit-identical, the rest within 0.03 dB in every band. On -20 dBFS tones with Bass Boost
      this build reads the cookbook to 0.0000 dB at all three rates, where `main`'s f32 bands were
      0.014 dB off at 44.1 kHz and 0.44 dB at 40 Hz at 192 kHz.
- [x] An update check against an unchanged release logs its 304.
      `run_check` twice against the live release from a scratch folder, a binary under `target/`
      having no updater: 200 and the ETag stored, then `updater: 304 Not Modified`.
- [x] The radio Tag picker draws and narrows as before. Kenan, on Radio's Genre chip.

**One correction per device**, corrections written into `eq.json` by hand from two AutoEq
profiles, one for a card and one for System default:
- [x] The switch on: the signal path names the stage and the sound changes; a big bass boost on a
      loudly mastered track neither clips nor pumps.
      The panel named Headphone correction, then Equalizer. 2026-11-15: a brickwall master (2,075 samples at full scale) with DT 990 Pro
      (250 Ohm) peaks at -1.92 dBFS, and with Bass Boost on top at -2.60 dBFS. No sample reaches
      full scale, and against the linear path no 10 ms block shows 0.1 dB of limiter gain.
- [x] Switching device mid-track switches the correction, with no click and no jump in level;
      `CORRECTION_SWAP_MS` settled by ear and recorded here, once more against a profile with a
      high-Q sub-bass peak, which settles from rest for longer than the swap.
      **50 ms stays.** Kenan on DT 990 Pro (250 Ohm) through a UMC22, System default shared (DT
      990) to the UMC22 exclusive (HIFIMAN Sundara, 38 Hz at Q 2.27) and back four times, and the
      switch toggled on each: clean, the only break the reopen's own gap.
- [x] A device with nothing set plays uncorrected; System default plays its own.
      Shared, so System default: with only the card corrected every tone reads 0.0000 dB; with its
      own correction it plays that curve (the table below).
- [x] The switch off: exclusive output reads Bit-perfect with the Equalizer off, and a flat
      Equalizer is bit-identical (`bit_perfect.rs`'s rig).
      The panel read Bit-perfect on the UMC22 with a 16-bit 48 kHz WAV. The rig passes its three
      cases, and with everything off shared F32 at unity volume carries the 48 kHz master bit for
      bit.
- [x] Make Bit-Perfect turns the correction off and keeps both corrections.
      The correction, Equalizer and ReplayGain off, volume 100, rate followed; System default's
      and both cards' corrections kept.
- [x] The curve: sines at 44.1, 96 and 192 kHz through the null sink, read back per tone against
      the analytic response of the correction plus the bands, every tone within 0.1 dB, the table
      recorded here (the issue's 192 kHz check).
      21 tones from 25 Hz to 16 kHz at -20 dBFS per rate, each over the same run with everything
      off, against complex evaluation of the cookbook filters times the stage's headroom:

      | source | headroom, DT 990 / + Bass Boost | worst tone, DT 990 / + Bass Boost |
      |---|---|---|
      | 44.1 kHz | -5.832 / -11.298 dB | 0.0000 / 0.0000 dB |
      | 96 kHz | -5.832 / -11.299 dB | 0.0000 / 0.0004 dB |
      | 192 kHz | -5.832 / -11.299 dB | 0.0000 / 0.0053 dB |

      The 192 kHz figure is the same at every tone, so it is the level: `response.rs`'s cos-form
      `|H|²` cancels near DC at high rates and reads a sub-bass peak slightly off, up to 0.028 dB
      in the worst set built (a 31 Hz +12 dB band).
- [x] The rebuild on the audio thread with a 20-filter correction at 192 kHz during a slider drag:
      `perf record` on the output thread, `rebuild`'s share recorded here.
      `cycles:u` on `cpal_alsa_out`, 30 s of drag (63 band writes, all ten bands ending active):
      139.2 ± 3.3 M cycles/s against 137.4 ± 2.4 right after, so the rebuilds add about 1.3%;
      `Bank::update_headroom` took 0.11% of the window's samples. Nearly all of the thread is the
      per-sample path, and no underrun was logged.

**The card, the picker and the import:**
- [x] The picker finds a model by part of its name, applies it, and the card shows it; a second
      open with nothing changed sends one conditional request.
      "dt 990 pro (250" found crinacle's DT 990 Pro (250 Ohm), applied with all ten filters. With
      no cache: the list download, then the profile; the second open, its check aged past a day:
      one request, the check time moved and the list left unwritten. Applied while exclusive, it
      landed on the UMC22 alone.
- [x] The picker offline: with a cache it lists, without one it says it couldn't reach AutoEq; a
      hand-corrupted cache says the list couldn't be read; Import works either way.
      Offline through a dead `HTTPS_PROXY`, each open's recheck failing: all three as stated, and
      a file import applied.
- [x] Importing a file with lines it can't use lists them before applying; a per-ear file and a
      30-filter file are refused; a paste applies.
      The mixed file listed its OFF, `LP`, `BW Oct`, out-of-range and `GraphicEQ:` lines and
      applied the other six filters.
- [x] The switch gives a before and after with no click; Clear on one device leaves the other.
      Clear emptied System default alone, and its Undo put the correction back.
- [x] The online switch off: Find headphones is unavailable and nothing reaches GitHub, Import
      still applies; the onboarding row flips the same switch. With the correction off the online
      row is gone, and turning the correction back on shows it as it was left.
      The onboarding row showed with the correction off, and the card's row showed as it was left.
- [x] A `settings.json` from the released build, a curve on its equalizer: the first boot writes
      `eq.json` with that curve, drops the three keys from `settings.json`, and the curve plays.
      Ten times, each on the `settings.json` `main` wrote at quit with one preset or the hand
      curve: every `eq.json` carried it, every `settings.json` lost the three keys, and each played
      as `main` did (the Groundwork row).
- [x] The settings search finds the card by "headphone", "correction" and "AutoEq".

**Windows** (on Kenan's go):
- [ ] It builds; switching between two endpoints in shared mode switches the correction; System
      default plays its own.
- [ ] Whether a shared stream on System default follows a change of the Windows default, recorded
      here as the answer to the issue's open note.

**Memory:**
- [x] RSS sampled from `/proc` with the picker open on the full list and two minutes after it
      closes, against a run that never opened it; idle after close within noise, recorded here.
      Never opened, at the same point after boot: 179.2 MB VmRSS, 26.87 MB anonymous. Open: 185.4
      and 28.56. Two minutes after closing: 184.2 and 27.25, the list's heap given back to within
      0.4 MB; the rest is code and GPU pages faulted in to draw the dialog, which stay resident.

### Tests

Added where the landed change earns them, on Kenan's go, after the hand tests above. Existing
tests a phase breaks are fixed in that phase. The facade's corpus walk lands here, and
`CLAUDE.md`'s facade line then names its file.

---

## Cross-cutting

- **Docs land in the phase that stales them**, each argument above moving to the doc comment of
  what it constrains as that lands.
- **800 lines:** `equalizer.rs` (709 with the stage wired) gains no more; if it passes 760 the
  limiter moves to its own file first. `backend/mod.rs` (677 with the correction's field) holds
  every stage's cells in one `DspControls`. `app-window.slint` (751) gained its export lines only.
  `library/playback.rs` (567 with the heard-device getter and the bump) gains nothing more.
  Everything else new is its own module.
- **Memory:** two banks of 20 f64 filters per channel per source, a few KiB; the list only while
  the picker is open. The hand tests measure it.
- **Lock order** (ADR 0035) is untouched: only the output choice's lock is taken under the
  correction's, and the audio thread reads through the counter, never a lock.
- **The live public release:** both switches default off and every new field is `serde(default)`.
  The one migration is the equalizer's curve, moved from `settings.json` to `eq.json` at boot.

## Open questions

- Whether System default should key by the endpoint Windows resolves it to, which the Windows hand
  test's answer decides is worth asking.
- Whether the full index joins the picker later, each row then naming its rig.
- Whether a stand-in plays System default's correction, which needs every reopen to re-aim (a
  track start's, the health check's `reopen_output`).
