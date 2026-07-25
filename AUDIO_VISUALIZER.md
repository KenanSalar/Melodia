# Audio Visualizer — Implementation Plan

A real-time, audio-reactive **spectrum visualizer** for the full-screen Now-Playing
view. A reviewer flagged that Melodia lacks the visualizations that established players
(Winamp, foobar2000, VLC, iTunes) have long shipped. Melodia already has a rich
Now-Playing surface (blur mosaic, accent-tinted chrome), so a visualizer is the natural
next step for "now playing" immersion.

> **Status:** Phases 1–4 shipped — the feature is complete and usable: audio tap + lock-free
> ring, the DSP analyzer, the Now-Playing bars strip, and a persisted toggle in
> Settings → Playback. Phases 5–6 are optional extras; Phase 7 (docs + perf pass) is what remains before this
> file can go. This is a working doc — keep the phase checkboxes current and delete the file
> when the feature ships.

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
| Default state | **ON** | deliberate exception to "new visible behavior ships off" — the visualizer is a Now-Playing flourish that changes nothing audible, so there's no surprise to guard against, and a feature nobody discovers is a feature nobody has. Upgraders pick it up via `#[serde(default)]`; turning it off persists |

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

### Phase 1 — Audio tap & lock-free ring `[x]`

Goal: post-DSP samples land in a shared ring buffer, gated by an enabled flag, with zero
playback impact.

> **The shipped shape differs from the original sketch.** This phase was planned as a tap
> *inside* `EqSource` (6th ctor param + pushes at its three leaf return paths). It shipped
> instead as a thin **wrapper source**, `VisualizerTap<S>`, applied at the one existing wrap
> point. It sees byte-for-byte the same samples, but leaves `src/player/equalizer.rs`
> untouched — which keeps Phase 1 clear of that file's load-bearing `frame_phase == 0` poll
> gate and bit-identical bypass path, and leaves the ~10 `EqSource::new` call sites in
> `equalizer_tests.rs` alone. The whole feature lives in one new file, which is what §5's SRP
> split asked for anyway.

- **`src/player/visualizer.rs` (new)** holds both halves.
  - **`VisualizerShared`** — the ring, and nothing else. Mirrors the *ownership* convention of
    `EqShared`/`FadeShared` (`Arc`, `&self` mutation, f32 held as bit patterns in atomics) but
    **not** the `Generation` poll pattern (`src/player/dsp.rs:11-42`): that pattern is
    control-thread-writer → audio-reader; this is the *inverse* — audio-writer → UI-reader,
    continuous — so there is nothing to poll. Fields: `enabled: AtomicBool` (checked first in
    the push), `sample_rate: AtomicU32` (**integer Hz**, not f32 bits — it comes straight off
    `SampleRate::get()`), `write_cursor: AtomicUsize` (monotonic), and
    `ring: Box<[AtomicU32]>` of `RING_CAP = 4096`.
    - Producer: `push(sample)` early-returns on `!enabled`, then `fetch_add`s the cursor and
      stores `to_bits()` at `idx % RING_CAP`. All `Relaxed`; wait-free, allocation-free. A
      torn read is possible and cosmetically invisible.
    - Consumer: `snapshot(out: &mut [f32])` fills any width, oldest-first, padding at the
      **front** when history is short so the newest sample is always the last element. A
      request wider than `RING_CAP` is answered for the last `RING_CAP` only.
    - A `const _: fn() = || { … };` `Send + Sync` assertion beside the type, like the existing
      eight (CLAUDE.md) — no `#[allow(dead_code)]`.
  - **`VisualizerTap<S>`** — a Rodio `Source` that forwards every sample untouched while
    accumulating one **mono** value per interleaved frame (`accum * inv_channels`). Channel
    count, its reciprocal and the rate are captured once at construction.
    - **The rate is published on the first completed frame, not in `new`** — a gapless
      successor is *built* when it is staged, seconds before it plays, so announcing its rate
      then would leave a differently-rated current track's tail analysed against the wrong `fs`.
    - `try_seek` forwards, then resets the accumulator and phase: `EqSource::try_seek` restarts
      its own interleave phase at 0, and a tap left half-full would straddle frames from there on.
    - Cost with the visualizer off: one float add, one increment and one compare per sample,
      plus a per-frame `Relaxed` load that early-returns.
- **`RodioPlayer`** (`src/player/rodio_backend.rs`): `viz: Arc<VisualizerShared>` field beside
  `eq`/`rg`/`xf`, seeded `VisualizerShared::new(false)` in `new()`; `build_source` wraps its
  `EqSource` in a `VisualizerTap` — the single point every playing / preloaded / crossfaded
  source goes through; infallible `set_visualizer_enabled(bool)` plus a `visualizer()` accessor
  (the handle Phase 3 reaches through `state.rodio`) next to the EQ setters.
- **Crossfade caveat (documented, accepted for v1):** during a crossfade both decks' taps push
  into the one shared ring, interleaving for the ~1–2 s overlap → a slightly noisier spectrum.
  Cosmetically irrelevant. Phase 6 offers an exact two-ring-sum fix.

**Tests:** `src/player/tests/visualizer_tests.rs` — 13, covering the ring (disabled push is a
true no-op, mid-stream enable, most-recent-window ordering, front padding, wraparound,
over-wide request, rate round-trip) and the tap (bit-identical passthrough enabled *and*
disabled, stereo channel-average, mono 1:1, partial trailing frame dropped, first-frame rate
publish, seek realignment). `tests/crossfade.rs` now pulls through `VisualizerTap` over a real
device-less mixer, so its amplitude assertions double as the transparency check.

**Memory:** ring 16 KiB + one `Arc` — resident always, trivial. Push is O(1) wait-free.

### Phase 2 — DSP analysis (pure, unit-tested) `[x]`

Goal: samples → normalized, smoothed band magnitudes, as pure functions with no I/O — the
part worth testing.

- **`src/player/spectrum.rs` (new)** — the pipeline as free functions, each unit-tested:
  `hann_window(size)`, `coherent_gain_scale(window)`, `band_bins(bands, fft_size, fs)`,
  `level_from_magnitude(mag)`, `bands_from_spectrum(spectrum, map, scale, out)` and
  `smooth(levels, next, attack, decay)`. Consts: `FFT_SIZE 2048`, `NUM_BANDS 32`,
  `MIN_HZ 20`, `FLOOR_DB -70`, `ATTACK 0.0`, `DECAY 0.8`.
- **`SpectrumAnalyzer`** is the only stateful piece — it holds exactly what must not be
  rebuilt per frame (the `realfft` plan + its three buffers, the Hann table and its scale,
  the bin→band map plus the rate it was built for, and the two band buffers). Nothing in
  `analyze` allocates.

> **Two deliberate departures from the sketch above.**
> 1. **`window_mut()` + `analyze(fs)`, not `magnitudes_to_bands(...) -> Vec<f32>`.** Returning
>    a `Vec` would allocate every frame, against §6's own rule. Handing out the FFT's own
>    input buffer lets Phase 3 snapshot the ring *straight into* the buffer the transform
>    reads — no intermediate window, no per-tick copy. (`realfft` treats `input` as scratch
>    and clobbers it, which is exactly right here: it is refilled every tick.)
> 2. **The analyzer owns the smoothing buffer**, not `ui/visualizer.rs` as Phase 3 sketches.
>    §5 says the UI layer carries "no DSP math", and smoothing state *is* DSP state. Phase 3's
>    tick is correspondingly a two-liner: `viz.snapshot(a.window_mut())` then
>    `a.analyze(viz.sample_rate())` → write the `VecModel`.

Details worth keeping: bin 0 (DC) is excluded from every band; a band takes its **loudest**
bin, not the mean (a wide treble band averaged against its empty bins reads dead); the map
is rebuilt only when the sample rate changes; `fs == 0` (nothing played yet) skips the FFT
but still smooths, so bars **decay** rather than freezing; and `process_with_scratch`'s
`Result` is never unwrapped — its buffers come from its own plan, so on the unreachable
`Err` the bars decay instead of panicking on the UI thread.

`src/player/dsp.rs` gained `linear_to_db`, the missing inverse of `db_to_linear`, which also
replaced the open-coded `20.0 * peak.log10()` in the EQ limiter.

**Tests:** `src/player/tests/spectrum_tests.rs` — 25, covering the Hann window (endpoints,
midpoint at both odd and even lengths, symmetry, degenerate sizes, coherent gain ≈ ½), the
band map (non-empty / contiguous / monotonic / never past Nyquist, swept over 44.1 / 48 /
96 kHz; nonsense rates; more bands than bins), level compression (silence, full scale,
clamping, the floor), per-band max and tail-zeroing, peak-follow smoothing (instant rise,
gradual fall, convergence, never below the band), and end-to-end: a 1 kHz full-scale sine
peaks in the band containing bin 46 at > 0.95 with the bass band < 0.2.

**Memory:** plan + 3 FFT buffers + Hann table ≈ 30–40 KiB, allocated once and held by the
UI-side analyzer instance. Negligible.

### Phase 3 — UI rendering: spectrum bars `[x]`

Goal: bars render and react while the NP view is open, colored to the artwork accent.

- **`ui/globals.slint` — the `Visualizer` global** (after `ReplayGain`, so the three audio-DSP
  globals sit together): `in-out property <bool> enabled;`, `in property <[float]> bars;`,
  `in property <bool> idle: true;`, `callback set-enabled(bool);` and
  `callback tick(bool /*is-playing*/);`. Registered in `app-window.slint`'s import **and**
  `export {}` re-export (Slint prunes un-re-exported globals from the Rust API).
  **No `style` property** — Phase 5 is optional and may never land, so shipping an unswitched
  style token would be dead weight; adding it later is purely additive.
- **`ui/components/now-playing/spectrum-bars.slint`:** a `HorizontalLayout` of
  `for level in Visualizer.bars`, each a stretching transparent container holding one
  **childless** bottom-anchored `Rectangle` (`height: max(2px, parent.height * level)`,
  `y: parent.height - self.height`) filled with `Player.np-accent-bright`. Mirrors
  `eq-band-slider.slint`'s computed-geometry idiom. Childless keeps the rounded cap off
  FemtoVG's offscreen-layer path (the HiDPI clip-blur pitfall); the 2 px floor keeps a visible
  baseline on silence. **No `animate height`** — Rust already smooths, and animating a smoothed
  value phase-lags. Fixed 56 px via `min-height`/`max-height` + `vertical-stretch: 0`, so the
  strip can't crowd the title on a short window.
- **Mounted in `ui/views/now-playing-view.slint`** as the last child of the centered artwork
  column, after the metadata chip strip — a base strip under the whole cover → title → chips
  group, which simply re-centers as a whole (the column is `alignment: center`). Gated
  `if Visualizer.enabled` and wrapped in a centring `HorizontalLayout`, because a fixed-width
  child doesn't centre in a wider `VerticalLayout` — the same reason the cover above it is
  wrapped. The cover's `clamp(root.width * 0.22, 200px, 380px)` was hoisted to a
  `cover-size` property on the view root so the cover and the strip stay aligned from one
  source.

> **The bars use a tone-floored accent, not the raw one.** Shipping with plain
> `Player.np-accent` made the strip nearly invisible on dark album art: the extracted accent
> *is* the artwork's dominant colour, so a dark cover yields a near-black accent painted over
> a backdrop the view's `Theme.crust.with-alpha(0.45)` scrim already darkens. Every other
> `np-accent` consumer draws it at `with-alpha(0.22)`, where that's harmless — the bars are the
> first surface to paint it **opaque**. Fix is a sibling `Player.np-accent-bright`, written
> beside `np-accent` in `track_change.rs` from
> `material_you::lift_to_min_tone(argb, 70.0)` — HCT tone is M3's contrast axis, so flooring
> it lifts the colour into view while hue and chroma (hence the album's identity) survive.
> Slint's `.brighter()` is **not** a substitute: it scales HSV value, so it can't lift a
> near-black colour at all. No upper cap is needed — the scrim keeps the backdrop from ever
> getting brighter than mid, which is the same reason the foreground text is legible over any
> cover. Five tests in `material_you_tests.rs` pin the guarantee (black lifts, hue survives,
> already-light is untouched, idempotent).

> **The Timer lives inside the strip component, and its gate is not just "playing".**
> Phase 3 was sketched as `running: Visualizer.enabled && <playing>`. That freezes the bars:
> pausing stops the tap but leaves the last window of audio in the ring, so halting the Timer
> strands them on a stale spectrum rather than letting them fall. What shipped is
> `running: Player.vm.is_playing || !Visualizer.idle` — the tick keeps firing past a pause,
> feeding the smoother silence (`analyze(0)`, which skips the FFT entirely) until Rust reports
> every band under `IDLE_LEVEL`, then stops. Decay is geometric at `DECAY = 0.8`, so the tail
> is ~31 frames (~0.5 s) and a paused, settled visualizer costs nothing — §6's idle-CPU goal,
> without the artifact. The two *other* gates still come for free: the component only mounts
> under `if Visualizer.enabled`, and the view only under `if Nav.now-playing-open`.

- **`src/ui/visualizer.rs` — `install_visualizer(ui: &AppWindow, state: &AppState)`**, shaped
  like `install_equalizer`, called from `boot/ui_setup.rs` right after `install_replaygain`
  (`&AppState` is enough — `state.rodio` is public, so the `main.rs` call site isn't needed).
  It seeds the global from `settings.json`, owns the bars `VecModel<f32>`, and registers two
  callbacks. `on_tick` captures the `Arc<VisualizerShared>` + `ui.as_weak()` (never a strong
  handle) and **owns the `SpectrumAnalyzer` by value** — Slint callbacks are `FnMut`, so no
  `Rc<RefCell<…>>` is needed. The tick is
  `viz.snapshot(analyzer.window_mut())` → `analyzer.analyze(rate)` → `set_row_data` per band.
  `set_row_data`, **not** `set_vec`: the latter takes a `Vec` by value and would allocate every
  frame. Smoothing state lives inside the analyzer (Phase 2), so the UI layer holds no DSP
  state.

**Memory:** bars `VecModel` = 32 floats; the analyzer buffers from Phase 2. Nothing renders or
computes while the NP view is closed. Well under the ~200 MB ceiling — no RSS follow-up needed.

### Phase 4 — Settings, persistence & toggle UI `[x]`

Goal: the user can turn it off, and the choice persists.

Shipped alongside Phase 3 — the toggle is the feature's only control surface, so splitting them
would have left one half untestable.

- **`src/services/settings/data.rs` — `VisualizerFlags { viz_enabled: bool }`**, whole-struct
  `#[serde(default)]` + `#[serde(flatten)]` into `SettingsData` and its `default()`, mirroring
  `EqualizerFlags`. Ships **true** (see §4) via a hand-written `impl Default` — `#[derive]`
  would give `false`. Because the flag is `#[serde(default)]`, an upgrading install (no
  `viz_enabled` key) picks the new default up and the bars appear; an explicit *off* writes
  `false` and sticks. **One field, no `style`** — see the Phase 3 note; adding it later is
  purely additive.
  > The default-on choice has one non-obvious consequence: `install_visualizer`'s
  > `read_settings` failure fallback must be `VisualizerFlags::default().viz_enabled`, **not** a
  > literal. `AppState::init` falls back to a whole `SettingsData::default()` on the same
  > failure, so a hardcoded `false` there would arm the tap while leaving the bars hidden.
- **`src/library/settings/visualizer.rs`** — `set_visualizer_enabled`, a `mutate_settings`
  two-liner mirroring `equalizer.rs::set_eq_enabled`.
- **Live-apply plumbing:** `player_set_visualizer_enabled(ctx, bool)` in
  `src/library/playback.rs` beside `player_set_eq_*`, calling Phase 1's
  `RodioPlayer::set_visualizer_enabled`. Infallible and lock-free like its siblings.
- **Boot hydration is split, deliberately.** The *backend* tap is armed in
  `hydrate_audio_dsp` (`src/state/mod.rs`), which already seeds EQ / ReplayGain / crossfade at
  `AppState::init` and is the one place that owns backend hydration; `install_visualizer`'s own
  `read_settings` seeds only the Slint property. (The original sketch put both in the
  installer.)
- **Toggle surface: Settings → Playback**, a `SettingRow` + `ToggleSwitch` after "Resume on
  Startup". It binds `checked <=> Visualizer.enabled` and fires `Visualizer.set-enabled(v)` —
  binding the **`Visualizer` global directly** rather than mirroring the flag onto `Settings`,
  the way the EQ dialog binds `Equalizer.enabled`, so the visualizer keeps its state and
  callback in its own module. Because the two-way binding already lands the value, the Rust
  handler is a plain `toggle_binding` with no write-back (the crossfade / gapless shape).
  The row joins the section's `row-visible` search filter, and the three upstream
  `SectionDivider` gates each gained `|| root.show-visualizer` so a hidden neighbour can't
  strand a divider. **No overflow-menu row** — an earlier revision put one there; it was
  removed and `menu-h` reverted 6 → 5.
- **Icon:** `bar_chart` (`graphic_eq` and `tune` are taken by the Equalizer and ReplayGain
  overflow rows). Added to `scripts/icons.txt` and re-subset via
  `scripts/subset-icon-fonts.sh`; `scripts/check-icons.py` passes on both faces.
- **i18n:** two new msgids — `"Visualizer"` and the row description
  `"Show a spectrum analyzer on the Now Playing screen"` — in all six shipped `.po` files.

**Tests:** none added. Phase 1–2 already cover the ring, tap and the whole DSP pipeline (37
tests); what Phases 3–4 add is Slint markup and callback wiring, which the project keeps
deliberately untested — and `EqualizerFlags` / `CrossfadeFlags` have no settings tests either,
so there was no precedent to follow.

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
- `src/player/visualizer.rs` + `src/player/tests/visualizer_tests.rs` — `VisualizerShared`
  (ring + atomics) and `VisualizerTap` (the transparent tap source). **Done.**
- `src/player/spectrum.rs` + `src/player/tests/spectrum_tests.rs` — pure DSP + `SpectrumAnalyzer`
  + tests. **Done.**
- `src/ui/visualizer.rs` — `install_visualizer`, UI-thread analyzer + render loop. **Done.**
- `src/library/settings/visualizer.rs` — persistence setter. **Done.**
- `ui/components/now-playing/spectrum-bars.slint` — bars component + driving Timer. **Done.**

**Changed**
- `Cargo.toml` — `realfft = "3.5.0"` in the `# Audio` group (+ `Cargo.lock`). **Done.**
- `src/player/dsp.rs` — `linear_to_db`, the inverse of `db_to_linear`. **Done.**
- `src/player/equalizer.rs` — the limiter's `20.0 * peak.log10()` now calls `linear_to_db`.
  **Done.**
- `src/player/mod.rs` — `pub mod visualizer;`, `pub mod spectrum;`. **Done.**
- `src/player/rodio_backend.rs` — `viz` field, seed in `new()`, `VisualizerTap` wrap in
  `build_source`, `set_visualizer_enabled` + `visualizer()`. **Done.**
- `src/library/playback.rs` — `player_set_visualizer_enabled`. **Done.**
- `src/library/settings/mod.rs` — `pub mod visualizer;` + re-export. **Done.**
- `src/services/settings/data.rs` — `VisualizerFlags` + flatten + default. **Done.**
- `src/state/mod.rs` — arm the tap in `hydrate_audio_dsp`. **Done.**
- `src/services/material_you.rs` (+ tests) — `lift_to_min_tone`. **Done.**
- `src/ui/now_playing/track_change.rs` — write `np-accent-bright`. **Done.**
- `ui/globals.slint` — `Visualizer` global (+ `app-window.slint` import/export). **Done.**
- `ui/views/now-playing-view.slint` — mount the strip; hoist `cover-size`. **Done.**
- `ui/views/settings/playback-section.slint` — toggle row + divider gates. **Done.**
- `src/ui/mod.rs` + `src/boot/ui_setup.rs` — register + call `install_visualizer`. **Done.**
- `scripts/icons.txt` + both subset TTFs — `bar_chart`. **Done.**
- `translations/*/LC_MESSAGES/Melodia.po` — `"Visualizer"` (6 locales). **Done.**

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
