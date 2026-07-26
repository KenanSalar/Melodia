# Audio Visualizer — Implementation Plan

A real-time, audio-reactive **visualizer** for the full-screen Now-Playing view — spectrum
bars or an oscilloscope trace. A reviewer flagged that Melodia lacks the visualizations that established players
(Winamp, foobar2000, VLC, iTunes) have long shipped. Melodia already has a rich
Now-Playing surface (blur mosaic, accent-tinted chrome), so a visualizer is the natural
next step for "now playing" immersion.

> **Status:** Shipped. Phases 1–4 (audio tap + lock-free ring, the DSP analyzer, the
> Now-Playing bars strip, a persisted toggle in Settings → Playback), **Phase 5's shared
> groundwork and 5.1 (the waveform style)**, and Phase 7's docs are all done — CLAUDE.md
> carries the conventions bullet and README the feature line. What's left of Phase 7 is the
> release gate, not code: `cargo clippy --all-targets -- -D warnings` and a release-build RSS
> reading.
>
> Retained deliberately, against the usual delete-on-ship rule: **5.2 (mirrored bars) and 5.3
> (ambient pulse) are committed and not yet built**, and Phase 6 (exact crossfade mix) stays
> optional. Both are now cheap — the style token, the picker and the shared driver all exist,
> so each is a key plus a component. Delete this file once they ship and Phase 6 is ruled out.

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
  the display on treble. (Strawberry's analyzer is linear, and the standing complaint against
  it is that "the entire right half is occupied by the frequency band between about 10.025 kHz
  and 22.05 kHz".) Keep the band edges **fractional**: rounding them to whole bins reintroduces
  the same unevenness at the *bottom* of the range instead.
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
| Bands | `64` default (const, room to make it a setting later) | reads well at the strip's NP-view width; cheap to draw |
| Band spacing | logarithmic (geometric, **fractional** bin edges) | perceptual frequency mapping; fractional edges keep every bar the same width in octaves |
| Frequency range | `50 Hz` – `16 kHz`, top `min`'d with Nyquist | 50 Hz matches CAVA / `DeaDBeeF`; 16 kHz matches the EQ's top ISO band and every lossy lowpass |
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
- **Modest bar count (64) and frame rate.** 60 fps (16 ms) is smooth; 30 fps (33 ms) halves
  the work and is usually indistinguishable for bars — keep it a one-line change to dial down.
- **Gate compute on visibility *and* playback.** The Timer only exists while the NP view is
  mounted (auto), and its `running` should also track playback status so a **paused/stopped**
  player runs no FFT — the single biggest idle-CPU saver.

**Main-thread budget:**
- Slint's guidance is "minimal work on the main thread." A 2048-pt f32 FFT is sub-millisecond,
  so v1 runs it on the UI Timer for simplicity. **If** a profile ever shows main-thread jank,
  Phase 7's background-analysis-task variant moves FFT + banding to a `spawn_cancellable`
  worker publishing bands into a second lock-free cell, leaving the UI to copy only 64 floats.

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
  `hann_window(size)`, `coherent_gain_scale(window)`, `band_edges(bands, fft_size, fs)`,
  `level_from_magnitude(mag)`, `bands_from_spectrum(spectrum, edges, scale, out)` and
  `smooth(levels, next, attack, decay)`. Consts: `FFT_SIZE 2048`, `NUM_BANDS 64`,
  `MIN_HZ 50`, `MAX_HZ 16_000`, `FLOOR_DB -70`, `ATTACK 0.0`, `DECAY 0.8`.
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
  **No `style` property** — shipping an unswitched style token would have been dead weight
  while bars were the only style; it arrives with Phase 5, purely additively.
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
  wrapped. Width is `max(cover-size, content-width * 0.75)`: three quarters of the column the
  metadata chips wrap against, so the strip reads as the base of the whole cover → title →
  chips group rather than of the cover alone. `content-width` is a view-root property
  (the panel minus its left padding, the inter-column gap and `up-next-width`) — derived
  arithmetically rather than read off `chip-area`, which only exists inside
  `if Player.vm.has_track`. The `max` is load-bearing: at the window's 350 px floor
  `content-width` goes negative.

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
  It seeds the global from `settings.json`, owns the bars `VecModel<f32>`, and registers three
  callbacks (`tick`, `set-active`, `set-enabled` — see the Phase 7 note on the last two).
  `on_tick` captures the `Arc<VisualizerShared>` + `ui.as_weak()` (never a strong
  handle) and **owns the `SpectrumAnalyzer` by value** — Slint callbacks are `FnMut`, so no
  `Rc<RefCell<…>>` is needed. The tick is
  `viz.snapshot(analyzer.window_mut())` → `analyzer.analyze(rate)` → `set_row_data` per band.
  `set_row_data`, **not** `set_vec`: the latter takes a `Vec` by value and would allocate every
  frame. Smoothing state lives inside the analyzer (Phase 2), so the UI layer holds no DSP
  state.

**Memory:** bars `VecModel` = 64 floats; the analyzer buffers from Phase 2. Nothing renders or
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
  `false` and sticks. **One field, no `style`** — see the Phase 3 note; Phase 5 adds it
  additively.
  > The flag decides whether the strip *mounts*, and nothing else — the Phase 7 producer gate
  > below took arming the tap away from it, so the `read_settings` failure fallback is no
  > longer load-bearing for keeping the two halves in step. Phase 5 adds a sibling `style`
  > field beside it.
- **`src/library/settings/visualizer.rs`** — `set_visualizer_enabled`, a `mutate_settings`
  two-liner mirroring `equalizer.rs::set_eq_enabled`.
- **No live-apply half, unlike its audio siblings.** It briefly had one
  (`player_set_visualizer_enabled` in `src/library/playback.rs`, plus arming in
  `hydrate_audio_dsp`); the Phase 7 producer gate removed both, since the tap now follows the
  view's visibility rather than the setting.
- **Toggle surface: Settings → Playback**, a `SettingRow` + `ToggleSwitch` after "Resume on
  Startup". It binds `checked <=> Visualizer.enabled` and fires `Visualizer.set-enabled(v)` —
  binding the **`Visualizer` global directly** rather than mirroring the flag onto `Settings`,
  the way the EQ dialog binds `Equalizer.enabled`, so the visualizer keeps its state and
  callback in its own module. Because the two-way binding already lands the value, the Rust
  handler needs no write-back — and since Phase 7 it needs no apply half either, so it is a
  bare `persist_blocking` rather than the siblings' `toggle_binding`.
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

### Phase 5 — Additional styles `[ ]`

Three styles beside the shipped bars, each a new `style` value + a Slint component switched on
`Visualizer.style`. The Phase 1–2 pipeline is **unchanged** — this is the OCP payoff from §5:
the tap, ring and analyzer stay as they are, and a style either consumes the existing band
model or reads the raw snapshot.

**Shared groundwork `[x]`.** Phases 3 and 4 both deliberately shipped *without* a style token,
so every sub-phase queued behind it. It shipped with 5.1; four things, none of them
style-specific:

- **`Visualizer.style` as a string key, not an int index.** `VisualizerFlags` gained
  `viz_style: String` (whole-struct `#[serde(default)]` → `"bars"`), matching how `themes/`
  keys its registry and how `MATERIAL_YOU_ACCENT_ID` / `CUSTOM_PRESET` work. An index would
  silently repoint every existing install's setting the day the list is reordered — and the app
  is publicly released. The key table is `STYLES` in `src/ui/visualizer.rs`; an unrecognized key
  resolves to index 0 rather than erroring, so a file from a newer build degrades.
  **Two Slint properties, one Rust writer:** `style` (the key, what the strip mounts on) and
  `style-idx` (what the picker binds). The mount branch reading the key is what makes a
  reordered picker a non-event.
- **`ChipGroup` in Settings → Playback, not a `Dropdown`.** The sketch above assumed four
  styles; with two, the row is a copy of the "Play Button Animation" row 25 lines above it in
  the same section — `SettingRowStacked` + `ChipGroup`, default (non-`manual`) mode with a
  two-way `<=>` on `style-idx`, which sidesteps the orphaned-binding pitfall a one-way binding
  would have needed `manual: true` for. Gated `if Visualizer.enabled` (never `visible: false`,
  slint#7377), the crossfade sub-row precedent. The toggle and the picker **search as one
  unit**, like the crossfade cluster — that is also what leaves the three upstream
  `SectionDivider` gates correct without touching them.
- **The name list is a named `@tr` literal array at the use site** (`viz-style-names`),
  mirroring `STYLES` **by position** — `@tr()` only translates literals at codegen, so a
  `[string]` seeded from Rust renders untranslated. `src/ui/tests/visualizer_tests.rs` pins the
  length against `STYLES` with the same `include_str!` + `split_once` shape
  `smart_criteria_tests` uses, and also pins the strip's mount branch to `STYLE_WAVEFORM`.
- **The Timer is hoisted out of `spectrum-bars.slint` into `visualizer-strip.slint`.** This was
  the load-bearing one. The 16 ms Timer *and* its three-part gate used to live inside the bars
  component: `running: Player.vm.is_playing || !Visualizer.idle`, the
  `tray_bridge::is_window_visible()` AND, and the rule that `idle` is written at the **end** of
  `tick` so an early return strands the Timer spinning at 60 Hz forever. Phase 7 warns twice
  that a second style's driver needs the same guard, so rather than duplicate it,
  `VisualizerStrip` owns the footprint and the one driver and the style components are pure
  render. `Visualizer.set-active` is unaffected — every style arms the tap identically.

The tick branches on the active style, so a style that needs no bands runs no FFT (see 5.3);
5.1 already exercises that path. Build order was to have been 5.2 → 5.3 → 5.1 by ascending
risk, but 5.1 shipped first: its one unknown (Slint `Path`) resolved on inspection rather than
by spike — see below — which left it no riskier than the others.

#### Phase 5.1 — Waveform / oscilloscope `[x]`

Goal: a live trace of the raw signal, in place of the band bars.

> **The `Path` question resolved by reading the compiler, not by a spike.** §9 listed Slint
> `Path` as unproven, and the specific open question was whether it can take `for`-generated
> `LineTo` children. It cannot: `i-slint-compiler`'s `passes/compile_paths.rs` errors with
> *"Path elements are not supported with `for`-`in` syntax, yet"* (slint-ui/slint#754). The same
> pass does accept **any** binding whose type is `String` for `commands`, routing it to
> `PathData::Commands` — the software-renderer caveat there doesn't apply to this FemtoVG-only
> tree. So the geometry is a Rust-built SVG string, the fallback column plot was never needed,
> and the trace is a real polyline.

> **A column is a range, not a point — and that was the second thing the phase got wrong.**
> The trace first shipped reducing ~5 samples per drawn point to their *signed peak* and joining
> the points with a 2 px line. It scribbled: peak-picking selects the local extreme, so above a
> couple of kHz consecutive points land on opposite extremes and the line whips top to bottom.
> A survey of what actually ships found nobody does this. There are exactly four strategies —
> draw every sample and let the overdraw form a band (`foobar2000`, ~4400 vertices a frame,
> unaffordable through a `Path` string); **min/max per column** (`DeaDBeeF`'s
> `ddb_scope_point_t { ymin, ymax }`, and every audio editor); resample properly before drawing
> (`foobar2000`, opt-in); or point-sample and accept honest aliasing (Winamp / Audacious, 75
> points with a hardcoded 2× gain). Three of the four converge on the same idea, and the second
> is the one this architecture can afford, so that is what it now does. The write-up of the
> survey is in the conversation that produced it; the numbers that mattered are folded in below.

- **New data:** `Visualizer.wave-path: string` — SVG commands, not a `[float]` model, for the
  reason above. `src/player/waveform.rs` is a **sibling of `spectrum.rs`, not an addition to
  it**: `spectrum.rs` is named for the spectrum and its every line is FFT/banding, so the trace
  got its own leaf module and its own `tests/waveform_tests.rs`. It reads the raw snapshot and
  bypasses banding entirely — this style runs **no FFT at all**, which is the OCP payoff §5
  predicted, arriving one sub-phase earlier than expected.
- **`min_max_columns` reduces the span to one `Column { min, max }` per drawn column**, and
  `write_path_commands` closes the two edges into **one filled figure** — lower edge left to
  right, upper edge back, `Z`. Nothing is skipped, so nothing can alias. The figure cannot fold
  over itself because `min <= max` by construction.
- **The figure is never degenerate, and never a hole.** Two rendering traps, both found on
  screen rather than in a test:
  - `MIN_HALF_THICKNESS` floors each column about its own midpoint, so a silent trace never
    lands both edges on the axis. Coincident edges close a **zero-area polygon** *and* lay the
    outline exactly on top of itself, which is geometry nothing owes you anything sensible for
    — it rendered as a dashed line. The floor's *size* is then chosen against the stroke: half
    a pixel either side, close enough that the two 1.25 px strokes still overlap into one line
    at rest rather than reading as a pair of rails.
  - The edges are emitted **lower-first** because that is what gives the closed figure a
    *positive* signed area. `i-slint-renderer-femtovg`'s `draw_path` runs
    `area += (x - prev.x) * (y + prev.y)` over each subpath and hands femtovg
    `Solidity::Hole` when it comes out negative — which upper-first does. A lone subpath
    declared as a hole is not a thing to rely on a renderer being sensible about.
- **Colour:** `Player.np-accent-bright` — the same tone-floored accent as the bars — as a
  `transparentize(0.55)` fill inside a 1.25 px stroke of the same brush. The fill alone reads as
  a shapeless blob at this height and the stroke alone loses the sense of a body of sound;
  together they sit in the translucency language the metadata chips already use. (Dropping the
  stroke and painting the fill opaque was tried as part of the dashed-line fix and reverted —
  the floor is what fixes the geometry, and the paint was never the problem.)
- **The trace is seeded at install.** The bars come up at rest for free — their `VecModel`
  carries a level per band and each floors at a dot — but the trace's only source is the Timer,
  and at boot the Timer isn't running (nothing is playing, `idle` is true). Without a seed a
  Now-Playing view opened on a freshly started app shows an empty strip. `install_visualizer`
  writes the resting figure through the real `write_path_commands`, so it is exactly what a
  decayed trace settles to rather than a hand-written literal.
- **Columns follow the strip width**, `columns_for_width` at one per 2 logical px, clamped to
  `64..=512`. `DeaDBeeF` uses one per pixel; one per two is indistinguishable under the
  envelope's own 1.25 px stroke and halves the string, its re-parse and the tessellation — which
  matters here in a way it doesn't for a scope drawing straight to a canvas. The width crosses
  as a `float` argument on `tick`, so a garbage value (NaN, the pre-layout zero) draws the
  *cheapest* trace rather than the most expensive.
- **The span is milliseconds, not samples.** `WAVE_SPAN_MS 40` + `TRIGGER_SLACK_MS 20`, resolved
  against `analysis_rate()` — so it stays 40 ms of what you *hear* regardless of the file's rate
  or the playback speed. A fixed sample count showed a 96 kHz file half the music of a 44.1 kHz
  one. **`RING_CAP` moved 4096 → 16384** to hold 60 ms at 192 kHz; above that the window clamps
  and the trace just narrows in time.
- **No gain stage, deliberately.** Winamp's hardcoded 2× works because it draws a *trace*, which
  crosses zero constantly and so never fills. A peak envelope with the same gain pins to full
  height on almost anything and becomes a fat wobbling bar. Envelope or gain, not both.
- **Per-frame allocation** lands at exactly one `SharedString`: `write_path_commands` clears and
  refills a `String` the installer owns (reserved at `MAX_COLUMNS * 2 * 20`; 512 columns measure
  14 848 bytes, so it never regrows). Slint re-parses that string with lyon on every render and
  caches nothing, which is fine against geometry that changes every frame anyway.
- **A viewbox is mandatory and `fit` must be `fill`.** `Path::fitted_path_events` fits the path
  to the element; with no viewbox it fits the path's *own bounding box*, which renormalizes
  every frame — a whisper would draw as loud as a chorus. `fit` defaults to `contain`
  (`FitStyle::Min`), which would letterbox the 1×2 box into a sliver. The declared box is
  `0 -1 1 2` and Rust normalizes into it, so no column count crosses the language boundary.
- **Screen y grows downward**, so `write_path_commands` flips both edges — otherwise positive
  peaks draw *below* the centre line and the whole trace is upside down. It subtracts from zero
  rather than negating, so a resting trace formats as `0.000` and not hundreds of `-0.000`.
- **Trigger alignment was required, not polish.** Consecutive snapshots start at an arbitrary
  phase, so an untriggered trace slides sideways every frame and reads as broken.
  `find_trigger` takes the **most recent** rising zero crossing in the slack region (lowest
  latency; every candidate is the same polarity, so which one is picked changes the latency and
  not the shape) and needs **hysteresis** — without it the trigger chases noise around the axis
  and jitters exactly as if it had none. Only `foobar2000` has a trigger at all and it ships
  *off*, taking the *first* crossing with a three-sample confirmation; keeping ours is a
  deliberate divergence, and it costs nothing.
- **Idle:** an inactive tick decays the whole column buffer toward the centre line rather than
  re-reading the ring, so a paused player collapses and settles instead of freezing mid-shape —
  the bars' behaviour, reusing the hoisted Timer's gate unchanged. The *whole* buffer, not the
  drawn prefix, so a strip resized while paused can't widen an undecayed column back into view.
- **The refresh is per style**, `33ms` for the trace against the bars' `16ms`, set on the
  hoisted Timer's `interval`. Bars want every frame — their decay is an animation and 60 Hz is
  what makes it smooth. A trace has no animation to be smooth, so a high rate only makes it look
  frantic; `foobar2000` caps its oscilloscope at 20 Hz by default for the same reason.
- **Speed:** unlike the band edges, playback speed is not a correctness bug here — folding it in
  via `analysis_rate()` just keeps the span 40 ms of wall clock. Commented so nobody "fixes" it.

**Tests:** `src/player/tests/waveform_tests.rs` — 35, covering the trigger (rising crossing on a
sine, most-recent selection, `search_len` bound, silence, both DC polarities, sub-hysteresis
noise, empty), `min_max_columns` (the full range of each column, column independence,
`min <= max` never inverting, no sample ever skipped, nearest-sample hold when upsampling, empty
either side), `columns_for_width` (follows the width, bounded both ends, nonsense input),
`write_path_commands` (one closed figure with both edges, the 0..1 x span out and back, the y
flip, the thickness floor opening a silent column about its own midpoint without widening a loud
one, the positive winding — computed with femtovg's own area formula — buffer reuse, empty), and
the analyzer end to end (equal time span at
44.1 / 48 / 96 kHz, the window never outrunning its buffer, the requested column count honoured,
a full-scale sine reaching the top, two snapshots of the same tone at different phases drawing
the same trace — the point of triggering, decay to rest without re-reading the window, and a
column widened back into view while paused having decayed with the rest). Slint markup stays
untested per convention.

**Memory:** the ring at 64 KiB (up from 16), a 512-column buffer and a ~20 KiB string buffer,
all allocated once. Negligible.

#### Phase 5.2 — Mirrored bars `[ ]`

Goal: the same bands, growing symmetrically from a centre line.

- **Pure Slint — no Rust changes at all.** Reuses `Visualizer.bars` untouched, which is why it
  goes first: it exercises the style switch end-to-end with nothing else in flight.
- **Geometry:** `height: max(floor, parent.height * level)` with
  `y: (parent.height - self.height) / 2`, against the shipped bars' bottom anchoring.
- **Childless `Rectangle`s**, as in Phase 3 — a child would put the rounded cap on FemtoVG's
  offscreen-layer path (the HiDPI clip-blur pitfall).
- **Clamp the corner radius on both axes** — `min(self.width / 2, self.height / 2, <cap>)`.
  FemtoVG clamps a corner's x- and y-radii independently, so a short wide bar with a large
  radius pinches into a lens. The strip has already been bitten by this twice (the paused-bar
  fix, then the resting-dot change); a mirrored bar is the same size-varying shape.
- **Resting shape:** bars currently rest as dots rather than a floored bar — the mirrored
  variant should rest as a dot on the midline, or the strip reads as a dashed rule.

**Tests:** none — no new Rust. **Memory:** none.

#### Phase 5.3 — Ambient pulse `[ ]`

Goal: an accent-tinted glow breathing with overall energy, matching the blur aesthetic rather
than reading as an analyzer.

- **New data:** `Visualizer.energy: float`, from a pure `rms(samples)` in `spectrum.rs`, smoothed
  through the existing peak-follow `smooth` so it inherits the same attack/decay feel as the bars
  (and the same pause-decay behaviour for free).
- **This style runs no FFT.** With the tick branching on style, ambient is snapshot → RMS →
  smooth and skips the transform entirely — cheaper than bars, not merely equal. That it's a
  branch rather than a rewrite is exactly what §5's layering bought.
- **Render:** a childless accent `Rectangle` whose opacity and/or scale derive from `energy`.
  **No `animate`** — Rust already smooths, and animating a smoothed value phase-lags (documented
  pitfall). Clamp the radius on both axes if it's rounded, per 5.2.
- **The design risk is compositional, not technical.** It shares a surface with the artwork blur,
  the `Theme.crust.with-alpha(0.45)` scrim and the accent-tinted chips — a full-bleed pulsing
  glow can easily fight all three. Prototype inside the existing strip's footprint before letting
  it spread behind the cover.

**Tests:** `rms` is pure → unit tests (silence, full-scale sine, DC offset, empty slice).

**Memory:** one float. Negligible.

### Phase 6 — Exact crossfade mix (optional refinement) `[ ]`

Replace the single shared ring with a **per-deck** ring (deck-scoped like `FadeShared`,
`src/player/decks.rs:68-71`); the analyzer sums the two decks' aligned windows to reconstruct
the true mixer output — correct spectrum through crossfades, no interleave artifact. Skip
unless the Phase 1 caveat proves visible in practice.

### Phase 7 — Polish, perf, docs `[~]`

- **Docs `[x]`:** the "spectrum visualizer is a read-only tap" bullet is in CLAUDE.md's
  audio-feature conventions (tap point, ring, UI-Timer render, gating, persistence) and the
  feature line is in `README.md`.
- **Perf pass `[ ]`:** with the visualizer on and NP open, run
  `/usr/bin/time -v target/release/Melodia` and confirm no idle-RSS regression; if the
  UI-thread FFT ever shows jank (it shouldn't at 2048), move Phase 2 into a background
  `TaskSpawner::spawn_cancellable` analysis loop publishing bands into a second lock-free cell,
  with the UI Timer only reading them. That loop lives in `tasks/` (no `ui::*` import, no Slint
  write — the UI-layer installer still owns the property write) and, being `spawn_cancellable`,
  is dropped by the shutdown token so it never pins the force-exit path. Verify
  `cargo clippy --all-targets -- -D warnings` clean.

Two post-review fixes landed on top of Phase 4 and are worth carrying forward if Phase 5
adds a style:

- **Speed-aware band edges.** The tap sits *inside* rodio's `Speed` wrapper, which forwards
  samples verbatim and only reports a multiplied `sample_rate()` upward. `VisualizerShared`
  therefore carries the speed too, and the analyzer reads `analysis_rate()` (rate × speed)
  rather than `sample_rate()` — otherwise a 2× listener sees the file's pitch, an octave
  below what they hear. Any future style reading the raw snapshot needs the same rate.
- **Visibility gate.** Slint `Timer`s fire off the event loop, not the render loop, and the
  loop stays alive through a close-to-tray hide — so `tick` ANDs
  `tray_bridge::is_window_visible()` into the same gate that skips the transform on a paused
  player. It must not be an early return: `idle` is written at the *end* of `tick`, and the
  Timer's `running` reads it, so returning early leaves a hidden-then-paused player spinning
  at 60 Hz forever. Feeding rate `0` skips the snapshot and the FFT but keeps the decay path,
  so the bars settle and the Timer stops. A second style's driver needs the same guard.
- **Producer gate — `enabled` vs `set-active`.** The two gates above only silence the
  *consumer*. The tap itself was armed from `viz_enabled`, which ships on, so the audio thread
  kept filling the ring for a view nobody had open and for a window hidden to tray. The setting
  and the arm state are now separate things: `Visualizer.enabled` decides only whether the
  strip mounts, and a new `Visualizer.set-active` — mirrored out of `AppWindow` as
  `watched-viz-active: Nav.now-playing-open && Visualizer.enabled` — arms the tap. It has to be
  a mirror on the always-mounted root (`NowPlayingView` is destroyed while closed and Slint has
  no unmount callback), and its own property rather than a second handler on
  `Nav.now-playing-open-changed`, whose single slot belongs to `wire_now_playing_open`.
  `src/ui/visualizer.rs` is the sole writer of the arm state, from exactly two places:
  `set-active` at the mount/unmount boundary and `tick` in steady state
  (`viz.set_enabled(analyzing)`, which picks up pause, minimise and hide-to-tray for free).
  Hence `hydrate_audio_dsp` skips the visualizer — `VisualizerShared::new(false)` is the
  correct boot state. Re-arming can miss one 16 ms window; `snapshot` front-pads with silence,
  so the first frame back reads a touch low, never wrong. A second style must not reintroduce
  a settings-driven arm.

---

## 8. Files touched (summary)

**New**
- `src/player/visualizer.rs` + `src/player/tests/visualizer_tests.rs` — `VisualizerShared`
  (ring + atomics) and `VisualizerTap` (the transparent tap source). **Done.**
- `src/player/spectrum.rs` + `src/player/tests/spectrum_tests.rs` — pure DSP + `SpectrumAnalyzer`
  + tests. **Done.**
- `src/ui/visualizer.rs` (+ `src/ui/tests/visualizer_tests.rs`) — `install_visualizer`,
  UI-thread analyzers + render loop, the `STYLES` key table and its `.slint` pins. **Done.**
- `src/library/settings/visualizer.rs` — persistence setters. **Done.**
- `ui/components/now-playing/spectrum-bars.slint` — bars component. **Done.**
- `src/player/waveform.rs` + `src/player/tests/waveform_tests.rs` — trigger, min/max columns,
  path string + `WaveformAnalyzer` (Phase 5.1). **Done.**
- `ui/components/now-playing/visualizer-strip.slint` — footprint, style switch, the one driving
  Timer (Phase 5.1). **Done.**
- `ui/components/now-playing/waveform-trace.slint` — the stroked `Path` (Phase 5.1). **Done.**

**Changed**
- `Cargo.toml` — `realfft = "3.5.0"` in the `# Audio` group (+ `Cargo.lock`). **Done.**
- `src/player/dsp.rs` — `linear_to_db`, the inverse of `db_to_linear`. **Done.**
- `src/player/equalizer.rs` — the limiter's `20.0 * peak.log10()` now calls `linear_to_db`.
  **Done.**
- `src/player/mod.rs` — `pub mod visualizer;`, `pub mod spectrum;`. **Done.**
- `src/player/visualizer.rs` — `RING_CAP` 4096 → 16384, so a millisecond-denominated waveform
  window fits at any rate a music file plausibly carries (Phase 5.1). **Done.**
- `src/player/rodio_backend.rs` — `viz` field, seed in `new()`, `VisualizerTap` wrap in
  `build_source`, `set_visualizer_enabled` + `visualizer()`. **Done.**
- `src/library/playback.rs` — briefly held `player_set_visualizer_enabled`; removed by the
  Phase 7 producer gate. **Done.**
- `src/library/settings/mod.rs` — `pub mod visualizer;` + re-export. **Done.**
- `src/services/settings/data.rs` — `VisualizerFlags` + flatten + default; `viz_style` key
  (Phase 5.1). **Done.**
- `src/state/mod.rs` — `hydrate_audio_dsp` deliberately skips the tap (Phase 7). **Done.**
- `src/services/material_you.rs` (+ tests) — `lift_to_min_tone`. **Done.**
- `src/ui/now_playing/track_change.rs` — write `np-accent-bright`. **Done.**
- `ui/globals.slint` — `Visualizer` global (+ `app-window.slint` import/export, and its
  `watched-viz-active` mirror from Phase 7). **Done.**
- `ui/views/now-playing-view.slint` — mount the strip; hoist `cover-size`. **Done.**
- `ui/views/settings/playback-section.slint` — toggle row + divider gates; the style chip row
  and its `viz-style-names` array (Phase 5.1). **Done.**
- `src/ui/mod.rs` + `src/boot/ui_setup.rs` — register + call `install_visualizer`. **Done.**
- `src/player/mod.rs` — `pub mod waveform;` (Phase 5.1). **Done.**
- `scripts/icons.txt` + both subset TTFs — `bar_chart`, plus `show_chart` (Phase 5.1). **Done.**
- `translations/*/LC_MESSAGES/Melodia.po` — `"Visualizer"`, plus `"Visualizer Style"` /
  `"Bars"` / `"Waveform"` / the style description, and the toggle's own description reworded off
  "spectrum analyzer" now that it isn't only one (6 locales). **Done.**

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
- **Slint `Path`** — *resolved in 5.1.* `for`-generated path elements are rejected outright
  (slint-ui/slint#754), so the waveform's geometry is a dynamic `commands` string with a fixed
  viewbox. Bars still use the `Rectangle` idiom, so the primary deliverable never carried the
  new-primitive risk.
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
