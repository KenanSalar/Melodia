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

# The Now-Playing visualizer: UI half

What spans `crates/melodia-views/src/ui/visualizer/`, the `.slint` strip and pickers, and the tap
in `melodia-playback`, whose DSP is `audio-stack.md`'s. `ui::visualizer`'s `//!` argues arming and
off-screen windows, `pulse.rs` the frame counter, and the strip's files their own geometry and rate.

## Arming the producer

- **`Visualizer.enabled` decides only whether the strip mounts; `set-active` arms the tap.**
  `AppWindow`'s `watched-viz-active` mirror fires it, the view having no unmount hook and
  `now-playing-open-changed`'s one slot being `wire_now_playing_open`'s. So `hydrate_audio_dsp`
  skips the visualizer, `VisualizerShared::new(false)` being the right boot state, and
  `library::settings::set_visualizer_enabled` persists only: `ui::visualizer` is the sole writer
  of the arm state, always on the UI thread, where the first arm builds the rings.

## The tick's gates

- **A new style ANDs into `Session::advance`'s `analyzing` term and draws off `analysis_rate`,
  never an early return**, which leaves `idle` stuck and the Timer running.
- **`visualizer_tests` pins the Timer's `running` gate against `visualizer-strip.slint`** and the
  stall rule against `FrameWatch`. Drop either and the only symptom is a tick for a window nobody
  can see.

## The strip and its styles

- **The bars are asked for and the trace is pushed** (`spectrum-bars.slint`, `wire_bars_figure`).
  Nothing strip-local may stand in for the ask: a `changed width` or a width-reading Timer is a
  tracker inside an `if` (`slint-pitfalls.md`).

## The two style pickers

- **Both pickers render `VizStylePresets.viz-style-names`, mirroring `ui::visualizer`'s `STYLES` by
  position**, a row's loop index being what `set-style` takes; `visualizer_tests` reads both
  `.slint` mounts.
- **A popup host with flyouts closes through one `dismiss()` function**, every closing path
  calling it: a function rather than a callback, since a Rust handler would replace its body.
