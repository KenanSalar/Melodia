//! Tests for the visualizer's style table.
//!
//! `STYLES` is mirrored by two things the compiler can't see: the translated
//! name array the Settings picker renders, and the key `VisualizerStrip`
//! branches on. Both drift silently — a reordered picker would repoint every
//! install's saved style, and a renamed key would leave the strip blank — so
//! they are pinned here against the `.slint` sources.

use super::*;

const PLAYBACK_SECTION: &str = include_str!("../../../ui/views/settings/playback-section.slint");
const STRIP: &str = include_str!("../../../ui/components/now-playing/visualizer-strip.slint");

#[test]
fn the_picker_names_one_style_per_key() {
    // `@tr(` occurrences inside `property <[string]> viz-style-names: [ … ];`.
    // `None` when the array is renamed or removed, which fails loudly.
    let names = PLAYBACK_SECTION
        .split_once("property <[string]> viz-style-names: [")
        .and_then(|(_, tail)| tail.split_once("];"))
        .map(|(body, _)| body.matches("@tr(").count());

    assert_eq!(names, Some(STYLES.len()));
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
}

#[test]
fn every_style_key_round_trips_through_its_index() {
    for (i, key) in STYLES.iter().enumerate() {
        assert_eq!(style_index(key), i);
    }
}

#[test]
fn an_unknown_style_key_falls_back_to_the_default() {
    assert_eq!(style_index("mirrored"), 0);
    assert_eq!(style_index(""), 0);
    assert_eq!(STYLES[style_index("mirrored")], STYLE_BARS);
}

#[test]
fn only_the_waveform_index_selects_the_waveform() {
    assert!(!is_waveform(style_index(STYLE_BARS)));
    assert!(is_waveform(style_index(STYLE_WAVEFORM)));
    // Past the end of the table, so it draws the default rather than nothing.
    assert!(!is_waveform(STYLES.len()));
}

#[test]
fn the_default_settings_style_is_a_known_key() {
    let default = crate::services::settings::VisualizerFlags::default().viz_style;
    assert!(
        STYLES.contains(&default.as_str()),
        "VisualizerFlags defaults to {default:?}, which is not in STYLES"
    );
}
