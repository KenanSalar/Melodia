# Audio Visualizer — Implementation Plan

A real-time, audio-reactive **spectrum visualizer** for the full-screen Now-Playing
view. A reviewer flagged that Melodia lacks the visualizations that established players
(Winamp, foobar2000, VLC, iTunes) have long shipped. Melodia already has a rich
Now-Playing surface (blur mosaic, accent-tinted chrome), so a visualizer is the natural
next step for "now playing" immersion.

> **Status:** planning. This is a working doc — keep the phase checkboxes current and
> delete the file when the feature ships. No code has been written yet.

---

## 1. What a visualizer is (scope)

Animated graphics driven in real time by the *playing audio*. We ship the two highest-value,
lowest-risk forms first and leave fancier styles as opt-in extensions:

- **Spectrum bars** (primary) — N vertical bars whose heights track per-band frequency
  magnitude (bass left → treble right). The canonical form; cheapest to draw in Slint.
- **Waveform / oscilloscope** (later) — a polyline of the raw signal. Needs Slint's `Path`
  element, which the project doesn't use yet.
- **Ambient** (later) — an accent-tinted glow/pulse that breathes with overall energy, to
  match the existing blur aesthetic rather than a hard analyzer look.

Non-goals: MilkDrop-style shader scenes, per-frame GPU compositing, standalone visualizer window.

---

## 2. Architecture at a glance

The one hard rule: **the tap must never affect playback.** We only ever *read a copy* of
samples the DSP has already produced, and the whole path is a no-op when disabled.

```
 audio thread (rodio)                  UI thread (Slint event loop)
 ┌─────────────────────┐              ┌──────────────────────────────────┐
 │ EqSource::next()    │   push       │ Slint Timer (~16ms, mounted only  │
 │  post-DSP f32 frame │──mono──▶ [ring buffer ] ──▶ while NP view open)  │
 │  (EQ/RG/fade/clamp) │  (lock-free  │   snapshot last FFT_SIZE samples  │
 └─────────────────────┘   overwrite) │   → Hann window                    │
        writer                        │   → realfft (reused buffers)       │
                                      │   → magnitudes → log bands         │
   Arc<VisualizerShared>             │   → temporal smoothing (decay)     │
   held on RodioPlayer,              │   → write [float] bar model        │
   cloned into every EqSource        │      → `for` Rectangle bars redraw │
                                      └──────────────────────────────────┘
                                                 reader
```

**Why this shape** (validated against the codebase and external best practice):

- **Tap in `EqSource`, post-DSP.** Every audible sample already flows through `EqSource`
  (EQ + ReplayGain + crossfade + clamp). Copying the finished frame there means the
  visualizer shows *what you actually hear*, and it's a pure read — playback is untouched.
- **Lock-free ring, not a `watch` channel.** The codebase deliberately throttles even 2 Hz
  position ticks to 1 Hz to avoid property-dirty churn (`bridge.rs`, `handlers.rs`). Pushing
  ~60 fps of frames through a `watch` would fight that design. A lock-free overwrite ring
  (audio = wait-free producer, UI = reader) decouples the two cadences cleanly.
- **FFT on the UI-thread Timer tick.** A 2048-point real FFT is on the order of tens of µs;
  running it in the ~16 ms Timer callback is negligible and avoids a third thread + a second
  shared cell. (Phase 7 documents a background-task offload if profiling ever shows jank.)
- **Bars via a `for`-loop of `Rectangle`s.** The project has **no `Path`/canvas drawing**;
  the EQ dialog already draws bar-like visuals as computed-geometry `Rectangle`s
  (`ui/components/dialog/eq-band-slider.slint`). We mirror that exactly — no new primitive.

**Real-time / DSP best practices we adopt** (from RealFFT docs + spectrum-analyzer prior art):

- Allocation-free on the audio thread — the ring is fixed-size atomics, the push is wait-free.
- Reuse FFT input/output/scratch vecs across ticks (`process_with_scratch`), never re-plan.
- Apply a **Hann window** before the FFT to cut spectral leakage.
- **Logarithmic** frequency banding — human pitch perception is geometric; linear bins waste
  the display on treble.
- **Temporal smoothing** — fast attack / slow decay so bars feel alive but not twitchy.

---

## 3. Dependency

Add exactly one crate (pins the latest published versions; RealFFT pulls RustFFT transitively):

```toml
# Cargo.toml
realfft = "3.5.0"   # real-to-complex FFT, wraps rustfft 6.4.1; audio-optimized
```

`realfft` is the right choice over raw `rustfft`: real input → `N/2+1` complex bins with a
half-length internal FFT (less work, no manual complex packing), and a planner that caches
instances + hands out reusable scratch buffers.

---

## 4. Design decisions (locked)

| Knob | Choice | Rationale |
|------|--------|-----------|
| FFT size | `2048` (const, power of two) | ~46 ms window @ 44.1 kHz — good bass resolution, still responsive |
| Ring capacity | `4096` f32 (16 KiB) | ≥ 2× FFT size so a snapshot always has a full recent window |
| Sample domain | **mono** (average channels at frame boundary) | one FFT, half the ring; stereo split is not worth it for bars |
| Bands | `32` default (const, room to make it a setting later) | reads well at NP-view width; cheap to draw |
| Band spacing | logarithmic (geometric edges) | perceptual frequency mapping |
| Magnitude | `norm()` → scaled log/dB → normalized 0..1 | wide dynamic range compressed for display |
| Smoothing | peak-follow: instant/fast attack, exponential decay (~0.8/frame) | lively but calm |
| Redraw | Slint `Timer`, ~16 ms, **mounted inside the NP view** | self-gates: unmounts (stops) when NP closes |
| Bar smoothing | done in **Rust** (decay), **no** Slint `animate` on height | avoids the "animate a value that's already animated" phase-lag pitfall |
| FFT precision | `f32`, power-of-two size | half the bandwidth of `f64`, ample for display; power-of-two hits the SIMD/AVX planner fast path |
| Compute gate | only while NP view open **and** playing | a paused/closed player runs no FFT — biggest idle-CPU saver |
| Default state | **OFF** | new visible behavior ships off (project convention for a live/public app) |

---

## 5. Design principles (SOLID & DRY)

The design is layered so each concern has exactly one home and existing codebase patterns are
reused rather than re-invented.

- **Single Responsibility (SRP).** Four small units, one job each:
  - `player/visualizer.rs::VisualizerShared` — lock-free *sample transport* only (ring +
    enabled flag + sample rate). Knows nothing about FFT, bands, or Slint.
  - `player/spectrum.rs` — *pure DSP* only (Hann, FFT, banding, smoothing). Owns no
    audio/UI/threading types; every fn is input → output.
  - `ui/visualizer.rs` — *wiring/render* only (read ring → run DSP → write model). No DSP
    math, no persistence.
  - `library/settings/visualizer.rs` — *persistence* only.
  The `EqSource` change is a 3-line side-copy at existing return sites — it gains no
  visualizer logic beyond `viz.push(...)`.
- **Open/Closed (OCP).** New styles (waveform, mirrored, ambient — Phase 5) are added by a new
  `style` value + a Slint component switched on `Visualizer.style`; the Phase 1–2 pipeline
  (tap → ring → FFT → bands) is **not modified**. The analyzer exposes both the raw snapshot
  and the band model, so a waveform style consumes the snapshot without touching banding.
- **Liskov (LSP).** The tap lives behind `EqSource`, which stays a well-behaved rodio `Source`
  — `next()` returns exactly the same samples; the copy is an invisible side-read, so the
  source remains substitutable everywhere.
- **Interface Segregation (ISP).** `VisualizerShared` presents two *narrow* faces: a producer
  API for the audio thread (`push`, `set_sample_rate`, `enabled` read) and a consumer API for
  the UI (`snapshot`, `is_enabled`). Neither side sees methods it doesn't need. (Optionally
  split into a `Producer`/`Consumer` handle pair to let the compiler enforce it.)
- **Dependency Inversion (DIP).** The UI depends on the *abstraction* (a `[float]` band model),
  not on rodio internals; `spectrum.rs` depends on nothing from `player`/`ui`. This preserves
  the project's layering rules — `player`/`tasks` never import `ui::*`, and `spectrum.rs` is a
  leaf with no upward deps.

- **DRY — reuse, don't re-roll:**
  - Mirror `EqShared`'s atomic/`Arc` shape for `VisualizerShared`; don't invent a new sharing
    scheme.
  - Reuse the `install_*` convention, `settings_bind::toggle_binding`, and `PlaybackContext`
    helpers (`player_set_*`) — the toggle path is identical in shape to the EQ/crossfade toggles.
  - Reuse the EQ band-slider `Rectangle` idiom for the bars; the bar is one reusable component
    instanced by the `for` loop — no new drawing primitive.
  - Reuse `Player.np-accent` for color (already computed per artwork) — don't recompute an accent.
  - Precompute-once, reuse-many: the Hann table, FFT plan/buffers, and the bin→band index map
    are built once and reused every frame (see §6).

---

## 6. Performance best practices

Bake these in during implementation — they're not a post-hoc pass. Sourced from RustFFT and
Slint docs (context7).

**Audio thread (producer) — the hard real-time path:**
- **Wait-free, allocation-free, lock-free.** The push is a single atomic store + cursor bump
  into a fixed ring — no `Vec` growth, no mutex, no syscall. (rodio's sample is already `f32`,
  so the copy is trivial.)
- **Disabled = zero cost.** Every push early-returns on `!enabled` before touching the ring —
  an idle visualizer costs one predictable-branch atomic load per frame.
- **Downmix cheaply** — average the channels in the frame you already have; never re-read or
  re-decode.

**Analysis / FFT (per tick):**
- **Plan once, reuse forever.** Build the `RealFftPlanner` + FFT instance a single time in
  `install_visualizer`; never per tick. RustFFT shares twiddle-factor buffers across a reused
  planner and auto-selects the AVX/SIMD backend.
- **`process_with_scratch` with pre-allocated buffers.** Hold `input`/`spectrum`/`scratch` vecs
  (sized via `get_scratch_len()`) across ticks — zero per-frame allocation in the hot loop.
- **f32 + power-of-two size (2048).** Half the bandwidth of `f64`, enough for display; a
  power-of-two length hits the fastest planner path.
- **Precompute the bin→band map once.** Geometric band edges depend only on `fs` + `FFT_SIZE`;
  compute per-band bin ranges once and rebuild **only when the sample rate changes**
  (track-to-track), not every frame.
- **Precompute the Hann table once** and multiply in place.

**UI / render:**
- **Mutate the existing model, don't replace it.** Update the bars `VecModel` in place
  (`set_vec`/`set_row_data`) — the Slint docs are explicit that operating on the model via its
  notify is more efficient than resetting the property with a new model. Never
  `set_bars(ModelRc::from(new))` each frame.
- **No `animate` on bar height.** Rust already smooths; animating a smoothed value phase-lags
  (documented pitfall) *and* re-evaluates bindings every vsync. One shared 16 ms Timer +
  math-derived heights beats N concurrent `animate` blocks.
- **Keep the `for` body trivial.** Bind `height` to a precomputed 0..1 value and a cached
  brush; do all math in Rust, none per-bar per-frame.
- **Modest bar count (32) and frame rate.** 60 fps (16 ms) is smooth; 30 fps (33 ms) halves
  the work and is usually indistinguishable for bars — keep it a one-line change to dial down.
- **Gate compute on visibility *and* playback.** The Timer only exists while the NP view is
  mounted (auto), and its `running` should also track playback status so a **paused/stopped**
  player runs no FFT — the single biggest idle-CPU saver.

**Main-thread budget:**
- Slint's guidance is "minimal work on the main thread." A 2048-pt f32 FFT is sub-millisecond,
  so v1 runs it on the UI Timer for simplicity. **If** a profile ever shows main-thread jank,
  Phase 7's background-analysis-task variant moves FFT + banding to a `spawn_cancellable`
  worker publishing bands into a second lock-free cell, leaving the UI to copy only 32 floats.

**Memory:** everything is allocated once (ring 16 KiB, FFT buffers + Hann ~30–40 KiB, bars
model ~128 B) and reused — no per-frame allocation anywhere, nothing resident-heavy, nothing
computed while the NP view is closed. Net cost is tens of KiB; no bearing on the ~200 MB ceiling.

### Cross-check against `.claude/rules/` and CLAUDE.md

Every choice above was checked against the repo's rule files and conventions. The plan already
matches them; the reconciliations and the concrete adjustments they drove are folded into the
phases below.

- **`rodio-symphonia.md` — "audio output runs on its own thread — avoid blocking it with
  expensive operations"; samples are `f32` since rodio 0.22.** The tap is a wait-free `f32`
  copy — the *only* thing the audio thread does for the visualizer; no FFT/lock/alloc there. ✓
- **CLAUDE.md Slint pitfall — "Concurrent `animate` blocks aren't free at vsync … for periodic
  visuals prefer one shared `Timer` + counter + math-derived bindings."** This is exactly the
  render design: one 16 ms `Timer`, heights math-derived in Rust, zero `animate`. ✓
- **`slint.md` — "avoid setting properties from a tight loop on the UI thread."** *Reconciled,
  not violated:* the update is frame-bounded (16–33 ms), gated on visibility **and** playback,
  and mutates the existing `VecModel` in place (one `ModelNotify`/tick) — the endorsed
  periodic-visual pattern above, not an unbounded busy loop.
- **`slint.md` — "precompute in Rust and feed via a property"; "always `as_weak()` in
  callbacks."** All DSP is in Rust; the `for`-body is trivial; the `on_tick` handler captures
  `ui.as_weak()`. *(Phase 3 now states the weak-capture explicitly.)*
- **CLAUDE.md — "Renderer is FemtoVG" + the HiDPI rounded-clip blur pitfall.** Bars are
  **childless** `Rectangle`s, so rounded corners never trigger the offscreen-upscale blur.
  *(Added to Phase 3.)*
- **CLAUDE.md — the `EqSource` `frame_phase == 0` poll gate is load-bearing (a half-frame end
  permanently flips that deck's mixer channel parity).** The viz push is a pure side-read taken
  *after* the sample is produced; it must not join the generation-poll gate or alter frame
  advancement. *(Added to Phase 1 as a hard constraint.)*
- **CLAUDE.md — EQ/RG are orthogonal to `PlayerState` (infallible `player_set_*`, no
  `with_state_emit`/`PlayerAction`); `PlaybackContext` for `library::playback::*`.** The
  visualizer follows the identical orthogonal path. ✓
- **CLAUDE.md — `tasks/` never imports `ui::*`; `TaskSpawner::spawn_cancellable`; force-exit
  shutdown mustn't be pinned.** The optional Phase 7 offload lives in `tasks/`, writes bands to
  a lock-free cell (no Slint write, no `ui::*`), and is `spawn_cancellable` so the shutdown
  token drops it. *(Clarified in Phase 7.)*
- **`rust-performance.md` — atomics for lock-free flags; reuse buffers; `array_windows`/
  `as_array` for frame-based DSP; don't parallelize small work.** `VisualizerShared` mirrors
  `EqShared`'s `[AtomicU32; N]` f32-bits layout (`to_bits`/`from_bits`, no `unwrap`); FFT
  buffers are reused; frame slicing can use `slice::as_array`; the single 2048-pt FFT is
  deliberately **not** Rayon'd — below its overhead threshold (`rayon.md`: "don't parallelize
  small collections"). ✓
- **`serde.md` + the live-app rule — `#[serde(default)]` on new fields.** All `VisualizerFlags`
  fields are `#[serde(default)]` so shipped `settings.json` stays loadable. ✓
- **CLAUDE.md — no `#[allow(dead_code)]`; the `Send + Sync` assert idiom.** `VisualizerShared`
  gets a `const _: fn() = || { … };` assertion like the existing eight. *(Added to Phase 1.)*
- **`tokio.md` — never hold a `MutexGuard` across `.await`; `watch` when only the latest
  matters.** The design is lock-free end-to-end; the Phase 7 bands cell is latest-only. ✓

---

## 7. Phased plan

Each phase is independently reviewable and leaves the tree compiling. File anchors below are
current `file:line` references — treat them as "near here", they'll drift as code changes.

### Phase 1 — Audio tap & lock-free ring `[ ]`

Goal: post-DSP samples land in a shared ring buffer, gated by an enabled flag, with zero
playback impact.

- **Add `src/player/visualizer.rs`** defining `VisualizerShared` and its `Arc` constructor,
  mirroring the *ownership* convention of `EqShared`/`ReplayGainShared`/`FadeShared`
  (`src/player/dsp.rs:11-42` documents the family) — but **not** the `Generation` poll pattern.
  That pattern is control-thread-writer → audio-reader; the visualizer is the *inverse*
  (audio-writer → UI-reader, continuous), so no `bump()`/poll is involved. Fields:
  - `enabled: AtomicBool` — the whole feature's on/off; checked first in the push.
  - `sample_rate: AtomicU32` — current source's per-channel rate (f32 bits), written by
    `EqSource` on start so the UI banding knows the true `fs` (it varies per track).
  - `write_cursor: AtomicUsize` + `ring: Box<[AtomicU32; RING_CAP]>` — samples as f32 bits.
    Wait-free `push(sample)`: store at `cursor % CAP`, `fetch_add` the cursor (`Relaxed` is
    fine — visualization tolerates a torn read). `Sync`, cloneable via `Arc`, writable through
    `&self` so multiple transient `EqSource`s can hold it.
  - `snapshot(out: &mut [f32; FFT_SIZE])` — reader-side: copy the most-recent `FFT_SIZE`
    samples ending at the current cursor.
  - **Conventions:** store f32 via `to_bits`/`from_bits` over `AtomicU32` (mirrors `EqShared`'s
    `gains_bits: [AtomicU32; 10]`, no `unwrap`); add a `const _: fn() = || { … };` `Send + Sync`
    assertion beside the type like the existing eight `assert_send_sync` fns (CLAUDE.md).
- **Thread it through `RodioPlayer`** (`src/player/rodio_backend.rs`):
  - add `viz: Arc<VisualizerShared>` field beside `eq`/`rg`/`xf` (~`:183-191`);
  - seed it in `new()` (~`:203`);
  - `.clone()` it into the source in `build_source()` (~`:397`) — the single wrap point every
    playing/preloaded/crossfaded source goes through.
  - add infallible `set_visualizer_enabled(bool)` next to `set_eq_enabled` (~`:584`).
- **Extend `EqSource`** (`src/player/equalizer.rs`):
  - `EqSource::new(...)` (`:415`) takes the new `Arc<VisualizerShared>`; store it + write
    `sample_rate` into the shared cell once.
  - Push a mono sample at all **three** leaf return paths so the tap works EQ-on *and* EQ-off:
    - active DSP path — after the final write loop (`:753-759`), where `self.frame[..frame_len]`
      is the finished interleaved frame: average the channels → one mono sample → `viz.push(...)`.
    - `next_bypass()` (`:677`) and `next_bypass_faded()` (`:692`) — accumulate channel samples
      across `frame_phase` and push one mono value per completed frame (frame-boundary aligned;
      `frame_phase == 0` is the boundary, per `advance_frame_phase` `:523-529`).
  - **Every push early-returns when `!viz.enabled`** — disabled is truly zero cost, matching
    the `EqSource` bypass philosophy.
  - **Frame-parity constraint (load-bearing).** The push is a *pure side-read* taken **after**
    the sample is produced. It must **not** be folded into the `frame_phase == 0`
    generation-poll gate, nor alter frame advancement or the fade-end path — CLAUDE.md notes a
    half-frame end permanently flips that deck's mixer channel parity. Read `viz.enabled`, push,
    touch nothing else in `next()`.
- **Crossfade caveat (documented, accepted for v1):** during a crossfade two `EqSource`s (one
  per deck) both hold the shared ring and both push, interleaving for the ~1–2 s overlap → a
  slightly noisier spectrum. Cosmetically irrelevant. Phase 6 offers an exact two-ring-sum fix.

**Memory:** ring 16 KiB + one `Arc` — resident always, trivial. Push is O(1) wait-free.

### Phase 2 — DSP analysis (pure, unit-tested) `[ ]`

Goal: samples → normalized, smoothed band magnitudes, as pure functions with no I/O — the
part worth testing.

- **Add `src/player/spectrum.rs`** (pure DSP; owns no audio/UI types):
  - `hann_window(size) -> Box<[f32]>` — precomputed table, applied element-wise to the snapshot.
  - a `SpectrumAnalyzer` struct owning the reusable `realfft` plan + `input`/`spectrum`/`scratch`
    vecs (allocated once; `process_with_scratch` each call — **no per-tick allocation**).
  - `magnitudes_to_bands(spectrum, fs, bands) -> Vec<f32>` — geometric (log) band edges from
    ~20 Hz to Nyquist; per band take the max (or mean) magnitude, compress via log/dB, normalize
    to 0..1. Bins past Nyquist skipped.
  - `smooth(prev: &mut [f32], next: &[f32], attack, decay)` — peak-follow: rise fast, fall slow.
- **Tests** in `src/player/tests/spectrum_tests.rs` (wired via
  `#[cfg(test)] #[path = "tests/spectrum_tests.rs"] mod tests;`, per project convention):
  - Hann window endpoints ≈ 0, midpoint ≈ 1, symmetry.
  - a synthesized single-frequency sine lands in the expected band, silence → all-zero bands.
  - `smooth` rises immediately to a spike and decays monotonically toward a lower next value.

**Memory:** plan + 3 FFT buffers + Hann table ≈ 30–40 KiB, allocated once and held by the
UI-side analyzer instance. Negligible.

### Phase 3 — UI rendering: spectrum bars `[ ]`

Goal: bars render and react while the NP view is open, colored to the artwork accent.

- **`ui/globals.slint` — add a `Visualizer` global:** `in property <[float]> bars;`
  (0..1 heights, owned by Rust), `in property <bool> enabled;`, `in property <string> style;`,
  and `callback tick();` (fired by the Slint Timer each frame). Register the global in
  `app-window.slint`'s import **and** `export {}` re-export (Slint prunes un-re-exported
  globals from the Rust API).
- **`ui/components/now-playing/spectrum-bars.slint` — new component:** a `HorizontalLayout`
  with `for h[i] in Visualizer.bars: Rectangle { ... height: parent.height * h; ... }`, radius
  on top, filled with `Player.np-accent` (`ui/globals.slint:101` — the existing per-artwork
  accent, so the visualizer harmonizes with the blur). Mirror `eq-band-slider.slint`'s
  computed-geometry approach. Keep bars **childless** (no text inside) so rounded corners never
  trip the FemtoVG HiDPI clip-blur pitfall (CLAUDE.md). **No `animate height`** — Rust already
  smooths; animating a smoothed value phase-lags (documented Slint pitfall).
- **Mount in `ui/views/now-playing-view.slint`:** at the **bottom of the centered artwork
  column** (`:170-268`) — as the last child, directly **after** the metadata chip strip
  (the `if Player.vm.has_track: chip-area` block ends ~`:267`, before the column's closing
  brace ~`:268`). This keeps the natural cover → title → chips grouping intact and makes the
  visualizer a **base strip** under the whole block; the column is `alignment: center`, so the
  group simply re-centers as a whole when the strip mounts. Gate it `if Visualizer.enabled`.
  Give the strip a **modest fixed height** (~48–72 px via `min-height`/`max-height`,
  `vertical-stretch: 0`) so it never crowds the title on narrow windows (where the cover
  clamps to 200 px) — don't let it stretch.
- **Driving Timer** — add it beside the strip in the same view (precedent: the
  `Timer { interval: 1ms; ... }` at `:248-255`):
  `Timer { interval: 16ms; running: Visualizer.enabled && <playing>; triggered => { Visualizer.tick(); } }`,
  where `<playing>` is the VM's playback-status flag — so a **paused** player runs no FFT
  (§6). Because the Timer lives inside `NowPlayingView` — mounted only under
  `if Nav.now-playing-open` (`app-window.slint:900-905`) — it **also stops automatically when
  the view closes**. Both gates together mean the analyzer only runs when it's actually visible
  and moving.
- **`src/ui/visualizer.rs` — `install_visualizer(ui: &AppWindow, state: &AppState)`** following
  the `install_equalizer`/`install_replaygain` shape (`src/ui/equalizer.rs:27`,
  `src/ui/replaygain.rs:23`). It:
  - captures an `Arc<VisualizerShared>` clone (reachable via `state.rodio`) + `ui.as_weak()`
    (never a strong `AppWindow` handle in the callback — slint.md);
  - owns a `SpectrumAnalyzer` + a smoothing state buffer + the bars `VecModel<f32>` across ticks
    (held in an `Rc<RefCell<…>>` — UI-thread only, no `AppState` widening);
  - registers `Visualizer.on_tick`: snapshot ring → Hann → FFT → bands → smooth → write the
    `VecModel`. All on the UI thread, sub-millisecond.
  - Because it needs the `state.rodio` ring handle, call it from `src/main.rs` after `state`
    exists (like `now_playing::install` at `main.rs:333-338`), or from
    `install_library_settings_and_friends` (`boot/ui_setup.rs:233`) if a `&AppState` is enough.

**Memory:** bars `VecModel` = 32 floats; the analyzer buffers from Phase 2. Nothing renders or
computes while the NP view is closed. Well under the ~200 MB ceiling — no RSS follow-up needed.

### Phase 4 — Settings, persistence & toggle UI `[ ]`

Goal: the user can turn it on/pick a style, and the choice persists.

- **`src/services/settings/data.rs` — add `VisualizerFlags`** (mirror `EqualizerFlags`
  `:135-156`): `#[serde(default)] enabled: bool` (default **false**), `#[serde(default)]
  style: String` (default `"bars"`). `#[serde(flatten)]` it into `SettingsData` (~`:562`) and
  `SettingsData::default()` (~`:601`). (`#[serde(default)]` keeps already-shipped
  `settings.json` files loadable — required for the live/public build.)
- **`src/library/settings/visualizer.rs` — setters** mirroring
  `library/settings/equalizer.rs`: apply to the live backend synchronously via
  `ui::settings_bind::toggle_binding`, then `persist_blocking`.
- **Live-apply plumbing:** add `player_set_visualizer_enabled(ctx, bool)` in
  `src/library/playback.rs` beside `player_set_eq_*` (`:210-227`), calling
  `RodioPlayer::set_visualizer_enabled` from Phase 1.
- **Boot hydration:** seed enabled/style in `install_visualizer` by reading
  `settings::read_settings(&state.paths)` (the UI-feature hydration path, like
  `install_equalizer` `equalizer.rs:35-52`) and pushing `VisualizerShared.enabled`.
- **Toggle surface — pick one (recommend the overflow row):**
  - *Now-Playing overflow menu row* (recommended): a permanent "Visualizer" `OverflowRow`
    (`ui/components/now-playing/overflow-menu.slint`) that toggles on/off, matching the
    Equalizer/ReplayGain rows. Adding it makes the overflow **6 permanent rows** — bump the
    `menu-h` row count (CLAUDE.md documents this). Style selection can be a small left-side
    flyout like the speed/sleep flyouts, or deferred to Settings.
  - *Settings → Playback* (alternative/complement): a section with the enable toggle + a style
    dropdown, wired via `ui/settings_bind::toggle_binding` like the crossfade toggles.
- **i18n:** any new literal (row label, style names, settings copy) gets `@tr(...)` and the
  same msgid/msgstr added to **every** `translations/<lang>/LC_MESSAGES/Melodia.po`
  (en, de, fr, es, tr, el, it). Don't translate the style *ids* (`"bars"`), only labels.

### Phase 5 — Additional styles (optional) `[ ]`

Only if there's appetite after bars ship. Each is a new `style` value + a Slint component
switched on `Visualizer.style`; the Phase 1–2 pipeline is unchanged (waveform reads the raw
snapshot instead of bands).

- **Waveform / oscilloscope** — introduces Slint's `Path` element (currently unused in the
  project): a polyline of `LineTo` points bound to a `[float]` model of the downsampled
  snapshot. Prototype the `Path` API in isolation first — it's new ground here.
- **Mirrored bars** — bars growing from a center line; pure Slint, reuses the bands model.
- **Ambient pulse** — a single accent-tinted `Rectangle`/glow whose opacity/scale tracks total
  RMS energy; cheapest, best match for the blur aesthetic.

### Phase 6 — Exact crossfade mix (optional refinement) `[ ]`

Replace the single shared ring with a **per-deck** ring (deck-scoped like `FadeShared`,
`src/player/decks.rs:68-71`); the analyzer sums the two decks' aligned windows to reconstruct
the true mixer output — correct spectrum through crossfades, no interleave artifact. Skip
unless the Phase 1 caveat proves visible in practice.

### Phase 7 — Polish, perf, docs `[ ]`

- **Perf pass:** with the visualizer on and NP open, run
  `/usr/bin/time -v target/release/Melodia` and confirm no idle-RSS regression; if the
  UI-thread FFT ever shows jank (it shouldn't at 2048), move Phase 2 into a background
  `TaskSpawner::spawn_cancellable` analysis loop publishing bands into a second lock-free cell,
  with the UI Timer only reading them. That loop lives in `tasks/` (no `ui::*` import, no Slint
  write — the UI-layer installer still owns the property write) and, being `spawn_cancellable`,
  is dropped by the shutdown token so it never pins the force-exit path. Verify
  `cargo clippy --all-targets -- -D warnings` clean.
- **Docs:** add a "Graphic/spectrum visualizer" bullet to CLAUDE.md's audio-feature conventions
  (tap point, ring, UI-Timer render, gating, persistence) and a feature line to `README.md`.
- **Delete this file** once shipped.

---

## 8. Files touched (summary)

**New**
- `src/player/visualizer.rs` — `VisualizerShared` (ring + atomics).
- `src/player/spectrum.rs` + `src/player/tests/spectrum_tests.rs` — pure DSP + tests.
- `src/ui/visualizer.rs` — `install_visualizer`, UI-thread analyzer + render loop.
- `src/library/settings/visualizer.rs` — persistence setters.
- `ui/components/now-playing/spectrum-bars.slint` — bars component.

**Changed**
- `Cargo.toml` — add `realfft = "3.5.0"`.
- `src/player/rodio_backend.rs` — `viz` field, seed in `new()`, clone in `build_source`,
  `set_visualizer_enabled`.
- `src/player/equalizer.rs` — `EqSource::new` arg + pushes at the three leaf paths + `fs` write.
- `src/library/playback.rs` — `player_set_visualizer_enabled`.
- `src/services/settings/data.rs` — `VisualizerFlags` + flatten + default.
- `ui/globals.slint` — `Visualizer` global (+ `app-window.slint` import/export).
- `ui/views/now-playing-view.slint` — mount bars + driving Timer.
- `ui/components/now-playing/overflow-menu.slint` (+ `menu-h` row count) — toggle row.
- `src/main.rs` **or** `src/boot/ui_setup.rs` — call `install_visualizer`.
- `translations/*/LC_MESSAGES/Melodia.po` — new `@tr` strings (7 locales).

---

## 9. Risks & caveats

- **Playback safety** is the invariant: the tap only reads already-computed samples and pushes
  wait-free; a disabled visualizer early-returns before any work. Nothing in this plan can
  alter the audio output.
- **Torn ring reads** are possible (concurrent writer) but cosmetically invisible; a seqlock or
  double-buffer is available if ever needed (not for v1).
- **Crossfade interleave** — see Phase 1 caveat / Phase 6 fix.
- **Variable sample rate** across tracks — handled by carrying `fs` in the shared cell so
  band edges are computed against the true rate.
- **Slint `Path` is unproven here** — only Phase 5's waveform needs it; bars use the existing
  `Rectangle` idiom, so the primary deliverable carries no new-primitive risk.
- **Memory** — total resident footprint of the feature is on the order of tens of KiB, all
  allocated once, and no compute runs while the NP view is closed. No memory-discipline concern.

---

## 10. Sources

External best-practice research backing the DSP choices:

- [RealFFT — reusable scratch buffers & planner caching](https://docs.rs/realfft) (`process_with_scratch`, allocate-once)
- [spectrum-analyzer crate (Hann/Hamming windowing, real FFT)](https://crates.io/crates/spectrum-analyzer)
- [A better FFT-based audio visualization — Daniel Beer (log banding, log-power bars)](https://dlbeer.co.nz/articles/fftvis.html)
- [JUCE spectrum analyser tutorial (FFT → bins → bars, smoothing constant)](https://docs.juce.com/master/tutorial_spectrum_analyser.html)
- [audioviz — buffer distribution for smoother output](https://docs.rs/audioviz)
- [bevy_audioviz — CPAL + FFT real-time reference (lock-free UI handoff)](https://github.com/lowband21/bevy_audioviz)
