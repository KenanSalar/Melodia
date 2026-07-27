//! Tests for the parts of the visualizer no compiler checks: the style table's
//! mirrors, and the strip Timer's gate.
//!
//! `STYLES` is mirrored by two things the compiler can't see: the translated
//! name array both pickers render, and the key `VisualizerStrip` branches on.
//! Both drift silently — a reordered picker would repoint every install's saved
//! style, and a renamed key would leave the strip blank — so they are pinned
//! here against the `.slint` sources.
//!
//! The Timer's `running` and `interval` are pinned for the same reason and are
//! worse when they drift: Rust would still publish every property, the strip
//! would still look right on screen, and the only symptom would be a tick
//! running at 60 Hz for a window nobody is looking at.

use super::*;
use crate::services::settings::{DEFAULT_VIZ_STYLE, VisualizerFlags};

const PLAYBACK_SECTION: &str = include_str!("../../../../ui/views/settings/playback-section.slint");
const FLYOUT_PRESETS: &str =
    include_str!("../../../../ui/components/now-playing/flyout-presets.slint");
const STRIP: &str = include_str!("../../../../ui/components/now-playing/visualizer-strip.slint");
const SPECTRUM_BARS: &str = include_str!("../../../../ui/components/now-playing/spectrum-bars.slint");

#[test]
fn the_picker_names_one_style_per_key() {
    // `@tr(` occurrences inside `property <[string]> viz-style-names: [ … ];`.
    // `None` when the array is renamed or removed, which fails loudly.
    let names = FLYOUT_PRESETS
        .split_once("property <[string]> viz-style-names: [")
        .and_then(|(_, tail)| tail.split_once("];"))
        .map(|(body, _)| body.matches("@tr(").count());

    assert_eq!(names, Some(STYLES.len()));
}

#[test]
fn the_settings_picker_renders_the_shared_name_list() {
    // Both pickers have to name the same styles in the same order. They can
    // only do that by rendering the one array — a re-introduced local copy in
    // the Settings section would drift silently.
    assert!(
        PLAYBACK_SECTION.contains("options: VizStylePresets.viz-style-names;"),
        "the Settings style picker no longer binds the shared name list"
    );
}

#[test]
fn the_strip_branches_on_a_key_the_table_knows() {
    assert!(STYLES.contains(&STYLE_WAVEFORM));
    assert!(
        STRIP.contains(&format!("Visualizer.style == \"{STYLE_WAVEFORM}\"")),
        "the strip's mount branch no longer matches STYLE_WAVEFORM"
    );
    // The other branch is the catch-all, so an unrecognized key still draws.
    assert!(
        STRIP.contains(&format!("Visualizer.style != \"{STYLE_WAVEFORM}\"")),
        "the strip lost its fallback branch"
    );
    // Mirrored has no component of its own — it rides the catch-all branch and
    // only flips the bars' anchor, so what has to hold is the whole binding,
    // not just the key appearing somewhere in the file. Both ends are pinned:
    // a dead comparison left behind after `centred:` was dropped would pass the
    // first assertion, and the two files drift independently.
    assert!(STYLES.contains(&STYLE_MIRRORED));
    assert!(
        STRIP.contains(&format!("centred: Visualizer.style == \"{STYLE_MIRRORED}\"")),
        "the strip no longer anchors the bars off STYLE_MIRRORED"
    );
    assert!(
        SPECTRUM_BARS.contains("in property <bool> centred;"),
        "SpectrumBars no longer takes the anchor flag the strip sets"
    );
}

#[test]
fn the_strip_stops_ticking_for_a_window_the_os_calls_hidden() {
    // The certain half of the gate. `idle` has to stay in it or a pause would
    // freeze the drawing mid-shape instead of letting it fall.
    assert!(
        STRIP.contains(
            "running: (Player.vm.is_playing && Visualizer.window-shown) || !Visualizer.idle;"
        ),
        "the strip's Timer no longer gates on Visualizer.window-shown"
    );
}

#[test]
fn a_dormant_strip_polls_rather_than_running_at_frame_rate() {
    // The inferred half. Nothing can wake this one, so it slows the Timer down
    // instead of stopping it — drop the arm and the Timer is back at 60 Hz for
    // a window Wayland never admitted was minimized.
    assert!(
        STRIP.contains("Visualizer.dormant ? 500ms"),
        "the strip's Timer lost its dormant polling interval"
    );
}

#[test]
fn every_style_key_round_trips_through_its_index() {
    for (i, key) in STYLES.iter().enumerate() {
        assert_eq!(style_index(key), i);
    }
}

#[test]
fn an_unknown_style_key_falls_back_to_the_default() {
    assert_eq!(style_index("not-a-style"), 0);
    assert_eq!(style_index(""), 0);
    assert_eq!(STYLES[style_index("not-a-style")], STYLE_BARS);
}

#[test]
fn only_the_waveform_index_selects_the_waveform() {
    assert!(!is_waveform(style_index(STYLE_BARS)));
    assert!(is_waveform(style_index(STYLE_WAVEFORM)));
    // Mirrored takes the bars path, so it must not answer here.
    assert!(!is_waveform(style_index(STYLE_MIRRORED)));
    // Past the end of the table, so it draws the default rather than nothing.
    assert!(!is_waveform(STYLES.len()));
}

#[test]
fn a_picker_index_outside_the_table_falls_back_the_same_way() {
    // Both fallbacks have to agree, or a drifted picker and a hand-edited file
    // would land on different styles.
    for (i, _) in STYLES.iter().enumerate() {
        assert_eq!(style_index_from_i32(i32::try_from(i).unwrap_or(0)), i);
    }
    assert_eq!(style_index_from_i32(-1), 0);
    assert_eq!(style_index_from_i32(i32::MIN), 0);
    assert_eq!(style_index_from_i32(i32::MAX), 0);
    assert_eq!(
        style_index_from_i32(i32::try_from(STYLES.len()).unwrap_or(0)),
        0
    );
}

#[test]
fn the_persisted_default_is_the_tables_first_entry() {
    // `style_index`'s miss and `style_index_from_i32`'s both answer 0, so the
    // key `VisualizerFlags` ships with has to be the key at that index — else a
    // fresh install and a corrupt one would disagree about the default style.
    // The two are spelled out independently, in `data.rs` and in the table, so
    // this compares literals rather than restating one of them.
    assert_eq!(STYLES.first().copied(), Some(DEFAULT_VIZ_STYLE));
    assert_eq!(VisualizerFlags::default().viz_style, DEFAULT_VIZ_STYLE);
}
