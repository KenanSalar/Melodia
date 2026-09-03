---
paths:
  - crates/melodia-views/src/ui/visualizer/**/*.rs
  - crates/melodia-ui/ui/views/now-playing-view.slint
  - crates/melodia-ui/ui/views/settings/playback-section.slint
  - crates/melodia-ui/ui/components/now-playing/visualizer-strip.slint
  - crates/melodia-ui/ui/components/now-playing/visualizer-flyout.slint
  - crates/melodia-ui/ui/components/now-playing/spectrum-bars.slint
  - crates/melodia-ui/ui/components/now-playing/waveform-trace.slint
  - crates/melodia-ui/ui/components/now-playing/view-menu.slint
  - crates/melodia-ui/ui/components/now-playing/overflow-menu.slint
  - crates/melodia-ui/ui/components/now-playing/menu-surface.slint
  - crates/melodia-ui/ui/components/now-playing/flyout-presets.slint
---

# The Now-Playing visualizer — UI half

Wiring, the per-frame tick, and the gates that decide when any of it runs — the half no `//!` doc
can hold, the subject spanning `crates/melodia-views/src/ui/visualizer/`, the `.slint` under
`crates/melodia-ui/ui/` and the tap below both. The DSP is
`crates/melodia-playback/src/player/playback/{visualizer,spectrum,waveform}.rs` under
`.claude/rules/audio-stack.md`; `mod.rs` argues arming and off-screen windows and `pulse.rs` the frame
counter, both worth reading before changing a gate.

## Arming the producer

- **`enabled` vs `set-active` is the split, and it is why there's no Rust runtime half.**
  `VisualizerFlags.viz_enabled` (→ `Visualizer.enabled`) is the persisted setting and decides only
  whether the strip *mounts*; the tap is armed by `Visualizer.set-active`, mirrored out of
  `AppWindow` as `watched-viz-active: Nav.now-playing-open && Visualizer.enabled`. A local mirror
  because `NowPlayingView` is destroyed while closed with no unmount callback, and its own property
  rather than a second handler on `Nav.now-playing-open-changed` (single slot, owned by
  `wire_now_playing_open`). So `hydrate_audio_dsp` **skips** the visualizer
  (`VisualizerShared::new(false)` is the correct boot state) and
  `library::settings::set_visualizer_enabled` has **no `library::playback` runtime half** —
  `crates/melodia-views/src/ui/visualizer/` is the sole writer of the arm state.

- **Re-arming drops the rings' history** — the newest samples down there may predate the close —
  and `snapshot` front-pads with silence, so the first frame back reads a touch low rather than
  stale.

- **`set-active(false)` also drops the session's buffers.** The two FFT plans with their windows,
  spectra and scratch, plus the trace's window, x-coordinate table and path string, live in an
  `Rc<RefCell<Option<Analyzers>>>` the tick builds on its first frame (`get_or_insert_with`, the
  one construction site, so no mount ordering can leave the tick without them) and that callback
  clears — a user who never opens Now Playing never pays for the plans. The tick's shadows
  (`was_idle`, `was_dormant`, the `FrameWatch`) live in that struct rather than beside it, so
  dropping it resets them to the resting values `set-active` publishes on the way out.

## The tick's gates

- **Both visibility signals AND into the same `analyzing` term — never early-return.** `idle` is
  only written at the end of `tick`, so an early return leaves it stuck and the Timer spins
  forever. Feeding rate `0` skips the snapshot and the FFT (the actual cost) while the decay path
  still runs, so the drawing settles, `idle` goes true and the Timer stops or slows; a re-show
  repaints within one tick because `ATTACK` is 0. **Any new style needs both that guard and
  `analysis_rate`.**

- **`visualizer_tests` pins both halves of the Timer's `running` gate against the `.slint`
  source**, and the stall rule itself — drop either and everything still builds and looks right,
  the only symptom a 60 Hz tick for a window nobody can see. `FrameWatch` is its own type to make
  that reachable: `painting` takes the count rather than reading `pulse::frames()`, so a test can
  drive it without stopping the window being painted.

- **`pulse::install` is deferred to the first `set-active(true)`, and it is not free to leave
  standing** — a live notifier costs the renderer an extra flush on every drawn frame of the whole
  window. `pulse.rs` argues the deferral, the one-shot guard and what the deferral costs; read it
  before moving the install back to boot.

## The strip and its styles

- **Both dimensions are the *view's*, not the strip's, and they are tied to each other.** Width is
  `max(cover-size, min(content-width * 0.75, strip-w-max))` — three quarters of the column the
  metadata chips wrap against, derived arithmetically from view-root properties rather than read
  off the `MetaChipStrip` (inside `if Player.vm.has_track`, and a binding-loop risk); the `max` is
  load-bearing at the window's 350 px floor, where `content-width` goes negative. Height is
  `clamp(root.height * 0.12, 56px, 128px)`, handed down as `VisualizerStrip.strip-height` since a
  component root cannot reach `parent` — 56 px is the old pinned height and stays both the floor
  and the component's fallback, so a call site that forgets still gets a strip. **The width ceiling
  exists because the band count doesn't move**: `spectrum::NUM_BANDS` is fixed, so a strip that
  widens without getting taller divides the same bars into ever-fatter columns, and a resting band
  drawn as a dot of its own column's width turns the row of beads into a row of slabs.
  `strip-w-max` is `strip-h / 4 * Visualizer.bars.length` — a band no wider than a quarter of the
  strip's height, read off the model rather than restating the constant. A 16:9 window never
  reaches it; a wide-and-short one does. `.length` lowers to `track_row_count_changes()`, which
  `set_row_data` doesn't dirty, so the per-band tick doesn't re-evaluate it.

- **What keeps the strip inside the panel is the *cover slot*, not anything the strip does.**
  Slint's shrink pass (`solve_box_layout` falls through to `layout_items` the moment the column's
  preferred sizes stop fitting) can only take height off a cell with room between `min` and
  `preferred`, and positions from the top — so the last child, the strip, overflows. Every child of
  the artwork column has `min == preferred` except the cover, in a plain `Rectangle` slot carrying
  `preferred-height`/`max-height: cover-size` and no min. The tile inside spells out `x` and `y`,
  so it contributes nothing to the slot's constraints (`gen_layout_info_prop`, argued in
  `slint-pitfalls.md`). Wrap it in a centring layout again and the slot's min returns to
  `cover-size`, the column loses its only slack, and a short — or merely wide, the tile growing
  with the width — window pushes the strip out of the panel.

- **A style needn't be its own component.** "Mirrored" is the same bars under a different anchor:
  `SpectrumBars` takes an `in property <bool> centred` the strip sets from the key on the
  *catch-all* branch, so switching Bars↔Mirrored re-evaluates one binding instead of rebuilding the
  whole 64-band subtree, and the column-width floor and two-axis radius clamp stay in one copy. The
  bar's `height` binding is its **total** height in both anchorings, so a centred bar puts
  `level * H/2` either side rather than a full bar each way — as every mirrored analyzer that ships
  does (CAVA halves its output for `ORIENT_SPLIT_H`, wavesurfer draws against a `halfHeight`,
  audioMotion's "perfect mirror" is `reflexRatio: 0.5`); doubling would clip past level 0.5 at any
  strip height. The word is overloaded — audioMotion's `mirror` and CAVA's `channels = stereo` mean
  the *horizontal* fold (bass in the centre), which we don't build.

- **Every style ticks at 33 ms** — one interval for all three (`visualizer-strip.slint`,
  `dormant ? 500ms : 33ms`), not one per style. 30 Hz rather than 60 because the bars' rounded-rect
  re-tessellation dominated allocation counts at vsync, and a trace has no decay animation to keep
  smooth besides — a high rate only makes it look frantic (`foobar2000`'s scope caps at 20 Hz).
  `VISUALIZER_DECAY` being per *frame*, one interval means all three settle in about the same
  second, and there is no per-style rate to retune. The trace's geometry crosses as an SVG
  `commands` string with a fixed viewbox rather than a model — `slint-pitfalls.md`'s `Path` entry.

- **The trace is the visualizer's most expensive frame, and the `x` half of it is cached.**
  Rebuilding the path string outweighs a whole spectrum frame, two FFTs included, and it is the
  number *formatting* that costs, not the arithmetic. `player/waveform.rs` holds the whole
  argument: `XPrefixes` for the cache, `push_fixed` for the writer both halves of a vertex share,
  and why a `write!("{x:.4}")` beside it would disagree in the last place.

## The two style pickers

- **Settings → Playback chips and the Now-Playing view menu render the same list**, so the
  translated names live once as the `viz-style-names` `@tr` literal array in
  `flyout-presets.slint`, and the flyout's style rows take their picker index off the `for name[i]`
  loop so its leading "Off" row can't shift them.

- **The view menu is the bar's overflow popup mirrored on the vertical axis** (trigger at the *top*
  ⇒ opens downward, columns anchored `y: 0`). The two share everything but geometry and rows:
  `OverflowRow`, `MenuSurface` (chrome + the `popup-id`-gated `FocusLossWatcher`),
  `PopupDismissCatcher`, `FlyoutMetrics.{menu-row-h,chrome-h}` for the column-height maths. **Not
  the trigger** — the bar's stays a plain `IconButton` (tooltip + bar-relative sizing, both
  hardcoded by `AccentDiscButton`), which is instead shared by the view's two accent discs, its
  back button and this menu's trigger.

- **Each host keeps one `public function dismiss()`** holding the close-triple (`pop.close()` +
  collapse the flyouts + clear `PopupHighlight.id`) that every row calls — a `public function`, not
  a callback, for the same single-handler-slot reason the Dialog teardown is one. What can't move
  into `MenuSurface` is the rows' `VerticalLayout`: the popup sizes itself off that layout's
  `preferred-width` and only the host can name a descendant, so it stays in the host and passes
  through `@children`.

- **The menu needs no Rust.** Its "Off" row writes **both** halves of `Visualizer.enabled` (the
  two-way binding, so the Settings toggle and `watched-viz-active` follow, plus `set-enabled` to
  persist); a style row re-enables before `set-style`, which already resolves the index, publishes
  `style` + `style-idx` and persists.
