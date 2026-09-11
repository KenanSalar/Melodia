---
paths:
  - crates/melodia-views/src/ui/visualizer/**/*.rs
  - crates/melodia-ui/ui/views/now-playing-view.slint
  - crates/melodia-ui/ui/views/settings/playback-section.slint
  - crates/melodia-ui/ui/components/now-playing/visualizer-strip.slint
  - crates/melodia-ui/ui/components/now-playing/visualizer-flyout.slint
  - crates/melodia-ui/ui/components/now-playing/spectrum-bars.slint
  - crates/melodia-ui/ui/components/now-playing/waveform-trace.slint
  - crates/melodia-ui/ui/components/now-playing/visualizer-picker.slint
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
  clears — a user who never opens Now Playing never pays for the plans. The tick's one shadow, the
  `FrameWatch`, lives in that struct rather than beside it, so dropping it starts the next session
  counting from a clean slate instead of from whatever the last one stalled at.

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

- **Both dimensions are the *view's*, not the strip's.** Width is
  `max(cover-size, min(content-width * 0.75, strip-w-max))` — three quarters of the column the
  metadata chips wrap against, derived arithmetically from view-root properties rather than read
  off the `MetaChipStrip` (inside `if Player.vm.has_track`, and a binding-loop risk); the `max` is
  load-bearing at the window's 350 px floor, where `content-width` goes negative. Height is
  `clamp(root.height * 0.12, 28px, 128px)`, handed down as `VisualizerStrip.strip-height` since a
  component root cannot reach `parent`. The floor is low because the strip is what a short panel
  buys the cover's floor back with; 56 px stays the *component's* fallback, so a call site that
  forgets still gets a strip at the height it used to pin. **The width ceiling is a per-band column
  pitch times `Visualizer.band-count`, argued at `strip-w-max`**, and a maximized window on an
  ordinary desktop is what reaches it. That count is published once from `spectrum::NUM_BANDS`
  rather than read off the figure, which carries its band count only in its own geometry; the
  ceiling is also clamped to the column less both scrollbar lanes, `cover-size`'s 200px floor
  being wider than the whole column on any panel under about 900px.

- **What keeps the strip inside the panel is the column's scroller; what the column spends before
  reaching for it is the *cover slot*.** Slint's shrink pass (`solve_box_layout` falls through to
  `layout_items` the moment the column's preferred sizes stop fitting) can only take height off a
  cell with room between `min` and `preferred`, and positions from the top, so the last child is
  the one that leaves the box. Every child of the artwork column has `min == preferred` except the
  cover, in a plain `Rectangle` slot carrying `preferred-height`/`max-height: cover-h` over a
  `cover-min` floor. The tile inside spells out `x` and `y`, so it contributes nothing to the
  slot's constraints (`gen_layout_info_prop`, argued in `slint-pitfalls.md`). Wrap it in a centring
  layout again and the slot's min returns to its preferred, the column loses its only slack, and
  every panel too short for the group at full size scrolls rather than resizing the artwork.
  Past that floor the scroller is what answers: the `ScrollView` is handed
  `max(visible-height, group.min-height)`, so the column's own minimum decides when a bar appears
  and nothing is drawn in half.

- **A style needn't be its own component, and the anchor is Rust's.** "Mirrored" is the same bands
  under a different anchor: `spectrum::write_bar_path` takes a `BarAnchor` that `bar_anchor`
  resolves off the style key on the *catch-all* branch, so switching Bars↔Mirrored changes which
  figure the writer emits and the `.slint` side mounts one `Path` either way. A bar's height is its
  **total** in both anchorings, so a centred one puts half either side rather than a full bar each
  way, which is what a mirrored analyzer owes; doubling would clip past level 0.5 at any strip
  height. The word is overloaded elsewhere, where *mirror* often means the horizontal fold with
  bass in the centre, which we don't build.

- **Every style ticks at 33 ms** — one interval for all three (`visualizer-strip.slint`,
  `dormant ? 500ms : 33ms`), not one per style. 30 Hz rather than 60 because both figures are
  rebuilt and re-tessellated from scratch on every tick, so the rate is what that costs, and a
  trace has no decay animation to keep smooth besides, so a high rate only makes it look frantic.
  `VISUALIZER_DECAY` being per *frame*, one interval means all three settle in about the same
  second, and there is no per-style rate to retune. **Both styles' geometry crosses as an SVG
  `commands` string with a fixed viewbox**, neither as a model — `slint-pitfalls.md`'s `Path` entry.

- **The trace is the visualizer's most expensive frame, and the `x` half of it is cached.**
  Rebuilding the path string outweighs a whole spectrum frame, two FFTs included, and it is the
  number *formatting* that costs, not the arithmetic. `player/playback/waveform.rs` holds the whole
  argument: `XPrefixes` for the cache, `push_fixed` for the writer both halves of a vertex share,
  and why a `write!("{x:.4}")` beside it would disagree in the last place.

## The two style pickers

- **Settings → Playback chips and the Now-Playing picker render the same list**, so the
  translated names live once as the `viz-style-names` `@tr` literal array in
  `flyout-presets.slint`, and the flyout's style rows take their picker index off the `for name[i]`
  loop so its leading "Off" row can't shift them.

- **`VisualizerFlyout` is the whole of the picker's popup, not a column in a menu**, which is what
  `visualizer-picker.slint` bought by replacing the Now-Playing view menu: no second column, so no
  fixed reserve to keep in step, no `PopupDismissCatcher` over gaps that no longer exist, and no
  collapse flag — the geometry is the flyout's own height off `FlyoutMetrics` and the preset count.
  The `FocusLossWatcher` is mounted directly and gated on `PopupHighlight.id`, the volume popup's
  shape, where the bar's overflow menu takes `MenuSurface` and its `popup-id`. That menu keeps the
  Speed and Sleep flyouts and the whole two-column apparatus; it is the reference for what a *menu*
  costs, not a second host of this list.

- **Each host keeps one `dismiss()` function** holding its close set — `pop.close()`, whatever
  flyout flags it has, and `PopupHighlight.id = ""` — that every closing path calls; a function, not a
  callback, for the same single-handler-slot reason the Dialog teardown is one. What can't move
  into `MenuSurface` is the rows' `VerticalLayout`: the popup sizes itself off that layout's
  `preferred-width` and only the host can name a descendant, so it stays in the host and passes
  through `@children`.

- **The picker needs no Rust.** "Off" writes **both** halves of `Visualizer.enabled` (the two-way
  binding, so the Settings toggle and `watched-viz-active` follow, plus `set-enabled` to persist);
  a style row re-enables before `set-style`, which already resolves the index, publishes `style` +
  `style-idx` and persists. **The picker's trigger carries the state the popup used to** — a bare
  glyph has no disc for `force-bg`, so `filled` reads `enabled` and `active` covers both drawing
  and picking.
