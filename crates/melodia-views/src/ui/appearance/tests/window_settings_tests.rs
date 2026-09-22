//! The caption style's chip index, which `settings.json` never sees and every Slint file carrying
//! the style compares against.
//!
//! The style persists by name, so a mapping that drifts from the chips still round-trips through
//! the file and only shows as a chip selecting some other style's buttons.

use super::*;
use melodia_testkit::strip_line_comments;

const WINDOW_CHROME_SECTION: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/window-chrome-section.slint");
const MINI_PLAYER_SECTION: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/mini-player-section.slint");
const CUSTOM_TITLEBAR: &str =
    include_str!("../../../../../melodia-ui/ui/components/custom-titlebar.slint");
const MINI_CAPTIONS: &str =
    include_str!("../../../../../melodia-ui/ui/components/mini-player/mini-captions.slint");

/// Every style beside the chip label naming it, in the chips' order.
const CHIPS: [(TitlebarButtonStyle, &str); 3] = [
    (TitlebarButtonStyle::Standard, r#""Windows""#),
    (TitlebarButtonStyle::Macos, r#""macOS""#),
    (TitlebarButtonStyle::Kde, r#""KDE""#),
];

/// The labels of the chip group two-way bound through `binding`, as spelled.
fn chip_labels(src: &str, binding: &str) -> Vec<String> {
    strip_line_comments(src)
        .split_once(binding)
        .and_then(|(head, _)| head.rsplit_once("options: [").map(|(_, rest)| rest.to_owned()))
        .and_then(|rest| rest.split_once(']').map(|(body, _)| body.to_owned()))
        .map(|body| body.split(',').map(|label| label.trim().to_owned()).collect())
        .unwrap_or_default()
}

#[test]
fn every_caption_style_reads_back_from_its_own_index() {
    let styles = CHIPS.map(|(style, _)| style);

    let read_back = styles.map(|style| style_for(idx_for(style)));

    assert_eq!(read_back, styles);
}

#[test]
fn each_caption_style_maps_to_its_chips_position() {
    let indices = CHIPS.map(|(style, _)| idx_for(style));

    assert_eq!(indices, [0, 1, 2]);
}

// No chip hands these over, so the answer only has to be one that draws buttons.
#[test]
fn an_index_past_either_end_reads_as_standard() {
    let past_last = idx_for(TitlebarButtonStyle::Kde) + 1;

    let read = [style_for(-1), style_for(past_last)];

    assert_eq!(read, [TitlebarButtonStyle::Standard, TitlebarButtonStyle::Standard]);
}

#[test]
fn the_titlebar_style_chips_sit_at_their_styles_indices() {
    let labels =
        chip_labels(WINDOW_CHROME_SECTION, "selected-index <=> Settings.titlebar-button-style;");

    assert_eq!(
        labels,
        CHIPS.map(|(_, label)| label),
        "the titlebar's chips no longer sit in `idx_for` order, so a chip selects the style at its \
         position rather than the one it names"
    );
}

#[test]
fn the_miniplayer_style_chips_sit_at_their_styles_indices() {
    let labels =
        chip_labels(MINI_PLAYER_SECTION, "selected-index <=> Settings.mini-player-button-style;");

    assert_eq!(
        labels,
        CHIPS.map(|(_, label)| label),
        "the miniplayer card's chips no longer sit in `idx_for` order, so a chip selects the style \
         at its position rather than the one it names"
    );
}

/// Each mount picks its buttons by comparing against the same integer the chip wrote, so a
/// comparison left on a moved index mounts one style's buttons under another's chip.
#[test]
fn both_caption_mounts_compare_against_the_mapped_index() {
    let macos = idx_for(TitlebarButtonStyle::Macos);
    let kde = idx_for(TitlebarButtonStyle::Kde);
    let titlebar = strip_line_comments(CUSTOM_TITLEBAR);
    let mini = strip_line_comments(MINI_CAPTIONS);
    let spellings = [
        (&titlebar, format!("mac-style: Theme.titlebar-button-style == {macos};")),
        (&titlebar, format!("caption-style: Theme.titlebar-button-style == {kde}")),
        (&mini, format!("if MiniPlayer.button-style == {macos}: MacOsTitleBarCluster")),
        (&mini, format!("style: MiniPlayer.button-style == {kde}")),
    ];

    let missing: Vec<&str> = spellings
        .iter()
        .filter(|(src, spelling)| !src.contains(spelling.as_str()))
        .map(|(_, spelling)| spelling.as_str())
        .collect();

    assert!(missing.is_empty(), "no caption mount compares against the mapped index: {missing:?}");
}
