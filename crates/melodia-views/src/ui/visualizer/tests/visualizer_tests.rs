//! Tests for the parts of the visualizer no compiler checks: the style table's
//! mirrors, the strip Timer's gate, and the stall rule behind `dormant`.
//!
//! `STYLES` is mirrored by two things the compiler can't see — the translated name array
//! both pickers render, and the key `VisualizerStrip` branches on — and both drift
//! silently: a reordered picker repoints every install's saved style, a renamed key
//! leaves the strip blank.
//!
//! The Timer's `running` and `interval` are worse when they drift. Rust still publishes
//! every property, the strip still looks right, and the only symptom is a tick running
//! at 60 Hz for a window nobody is looking at.
//!
//! [`FrameWatch`] is here for the opposite reason: ordinary logic, whose only route in
//! the running app is to stop painting the window. And the resting drawing is what a
//! strip remounting over a paused player comes up on, which never ticks — so nothing
//! downstream would correct a wrong one.

use super::*;
use crate::services::settings::{DEFAULT_VIZ_STYLE, VisualizerFlags};

const PLAYBACK_SECTION: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/playback-section.slint");
const FLYOUT_PRESETS: &str =
    include_str!("../../../../../melodia-ui/ui/components/now-playing/flyout-presets.slint");
const STRIP: &str =
    include_str!("../../../../../melodia-ui/ui/components/now-playing/visualizer-strip.slint");
const SPECTRUM_BARS: &str =
    include_str!("../../../../../melodia-ui/ui/components/now-playing/spectrum-bars.slint");
const VIZ_FLYOUT: &str =
    include_str!("../../../../../melodia-ui/ui/components/now-playing/visualizer-flyout.slint");
const NOW_PLAYING_VIEW: &str =
    include_str!("../../../../../melodia-ui/ui/views/now-playing-view.slint");

#[test]
fn the_picker_names_one_style_per_key() {
    // `None` when the array is renamed or removed, which fails loudly.
    let names = FLYOUT_PRESETS
        .split_once("property <[string]> viz-style-names: [")
        .and_then(|(_, tail)| tail.split_once("];"))
        .map(|(body, _)| body.matches("@tr(").count());

    assert_eq!(names, Some(STYLES.len()));
}

#[test]
fn the_settings_picker_renders_the_shared_name_list() {
    // Both pickers have to name the same styles in the same order, which they can only
    // do by rendering the one array — a local copy here would drift silently.
    assert!(
        PLAYBACK_SECTION.contains("options: VizStylePresets.viz-style-names;"),
        "the Settings style picker no longer binds the shared name list"
    );
}

#[test]
fn the_view_flyout_renders_the_shared_name_list() {
    // The other picker, pinned the same way. It takes each row's index straight off the
    // loop, which keeps its leading "Off" row from shifting the rest out of step.
    assert!(
        VIZ_FLYOUT.contains("for name[i] in VizStylePresets.viz-style-names:"),
        "the Now-Playing style flyout no longer renders the shared name list"
    );
}

/// The y coordinate of every vertex a path visits, in order. The commands are `M{x} {y}`
/// then `L{x} {y}` with a bare `Z` stuck to the last, so the y values are exactly the
/// tokens that don't open with a command letter.
fn path_y_values(path: &str) -> Vec<&str> {
    path.split_whitespace()
        .filter(|token| !token.starts_with(['M', 'L']))
        .map(|token| token.trim_end_matches('Z'))
        .collect()
}

#[test]
fn the_resting_figure_is_what_a_decayed_trace_settles_to() {
    // `resting_wave_path` claims to be where a decay lands, and builds itself
    // through the real writer so it can be. Nothing checked the claim. Drive a
    // real analyzer from a full-scale trace down to rest and compare.
    //
    // Not the strings: the two draw a different number of columns on purpose —
    // the live trace follows the strip's width, the seed uses the narrowest
    // input that still describes a span. What has to match is the *figure*, so
    // the comparison is over the distinct y coordinates the columns land on.
    const RATE: u32 = 44_100;
    const COLUMNS: usize = 128;

    let mut analyzer = WaveformAnalyzer::new(RING_CAP, MAX_COLUMNS);
    for (i, sample) in analyzer.window_mut(RATE).iter_mut().enumerate() {
        *sample = if i % 2 == 0 { 1.0 } else { -1.0 };
    }
    let loud = analyzer.analyze(true, RATE, COLUMNS);
    let mut live = String::new();
    waveform::write_path_commands(loud, &mut live);

    let resting = resting_wave_path();
    let mut settled: Vec<&str> = path_y_values(&live);
    settled.dedup();
    assert_ne!(
        settled,
        path_y_values(&resting),
        "a full-scale trace has to start somewhere other than rest, or this proves nothing"
    );

    // Comfortably past the point 0.8-per-frame takes a full-scale column under
    // the drawn floor.
    let mut decayed = String::new();
    for _ in 0..200 {
        waveform::write_path_commands(analyzer.analyze(false, RATE, COLUMNS), &mut decayed);
    }

    let mut settled = path_y_values(&decayed);
    settled.dedup();
    let mut seeded = path_y_values(&resting);
    seeded.dedup();
    assert_eq!(
        settled, seeded,
        "the seeded resting trace is no longer the figure a decay settles to"
    );
}

#[test]
fn resting_bars_put_every_band_back_where_the_model_was_seeded() {
    // Rest is the seed `install_visualizer` builds the model with, the strip flooring
    // each band at a dot. Its own literal rather than shared with `rest_bars`, so the
    // two can actually disagree.
    const SEEDED_LEVEL: f32 = 0.0;

    let model = VecModel::from(vec![SEEDED_LEVEL; NUM_BANDS]);
    for band in 0..NUM_BANDS {
        model.set_row_data(band, 0.9);
    }
    rest_bars(&model);

    // Bit patterns rather than `==`: an exact-restore claim, and the crate denies a
    // loose float comparison anyway. Reported as the first band that strayed — sixty-four
    // levels side by side say nothing on failure.
    let strayed =
        model.iter().enumerate().find(|(_, level)| level.to_bits() != SEEDED_LEVEL.to_bits());
    assert!(strayed.is_none(), "band left off the seed: {strayed:?}");
    assert_eq!(model.row_count(), NUM_BANDS, "resting resized the model");
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
    // Mirrored has no component of its own, riding the catch-all branch and only
    // flipping the bars' anchor — so what has to hold is the whole binding, not the key
    // appearing somewhere in the file. Both ends, the two files drifting independently.
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
fn the_strips_height_comes_from_the_panel_rather_than_a_literal() {
    // Both ends, either alone still building and drawing: a strip that re-pins its own
    // height ignores what the view passes, a view that stops passing one leaves the
    // strip on its fallback. Neither shows on a small window — the fallback *is* the
    // small-window height — so the symptom is a maximized window drawing a sliver.
    assert!(
        STRIP.contains("min-height: root.strip-height;")
            && STRIP.contains("max-height: root.strip-height;"),
        "the strip no longer takes its height from the host"
    );
    assert!(
        NOW_PLAYING_VIEW.contains("strip-height: root.strip-h;"),
        "the Now Playing view no longer sizes the strip against the panel"
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
    // The inferred half. Nothing can wake this one, so it slows the Timer rather than
    // stopping it — drop the arm and it is back at frame rate for a window Wayland
    // never admitted was minimized.
    assert!(
        STRIP.contains("Visualizer.dormant ? 500ms"),
        "the strip's Timer lost its dormant polling interval"
    );
}

/// A watch that has never seen a frame, so a test controls the whole count.
fn watch() -> FrameWatch {
    FrameWatch {
        last: 0,
        stalled_ticks: 0,
    }
}

#[test]
fn a_standing_frame_count_reads_as_painting_until_the_stall_threshold() {
    let mut watch = watch();
    // The count never moves, so every tick is a stalled one. The last tick that
    // still counts as painting is the one *before* the threshold.
    for tick in 1..FRAME_STALL_TICKS {
        assert!(watch.painting(Some(0)), "stood down after only {tick} stalled tick(s)");
    }
    assert!(!watch.painting(Some(0)));
    // And it stays down rather than oscillating.
    assert!(!watch.painting(Some(0)));
}

#[test]
fn one_painted_frame_resets_the_stall() {
    let mut watch = watch();
    for _ in 0..FRAME_STALL_TICKS {
        watch.painting(Some(0));
    }
    assert!(!watch.painting(Some(0)));
    // A window that started being drawn again has to be believed on the first
    // frame — `ATTACK` is 0, so the strip repaints within that same tick.
    assert!(watch.painting(Some(1)));
}

#[test]
fn an_uncounted_window_is_assumed_to_be_painting() {
    let mut watch = watch();
    // No notifier means no evidence either way, and an inference with none
    // behind it must not be what blanks the strip — however long it runs.
    for _ in 0..FRAME_STALL_TICKS * 2 {
        assert!(watch.painting(None));
    }
    // Nor may those ticks have accrued against a later real count.
    assert!(watch.painting(Some(0)));
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
    assert_eq!(style_index_from_i32(i32::try_from(STYLES.len()).unwrap_or(0)), 0);
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
