use melodia_core::error::AppError;
use melodia_testkit::{binding_value, block_body, code_tokens};

use super::*;

const CHROME_SECTION: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/window-chrome-section.slint");
const DOT_GRID: &str =
    include_str!("../../../../../melodia-ui/ui/components/settings/color-dot-grid.slint");

const TOGGLE_ROW: &str = "if root.show-window-border: SettingRow {";
const COLOR_ROW: &str = "if root.show-border-color: SettingRowStacked {";

/// A shipped theme, so the slots are the real grid rather than a fixture that could drift from it.
fn theme() -> &'static ThemeDef {
    themes::get("macos")
}

/// The body of the row `mount` opens, `mount` ending on its `{`.
fn row_body<'a>(section: &'a str, mount: &str) -> Option<&'a str> {
    section.find(mount).and_then(|at| block_body(section, at + mount.len() - 1))
}

const ACCENT: u32 = 0x00_00_78_D7;

// On a dark palette the edge lightens toward the text, which is what makes it visible there.
#[test]
fn a_dark_surface_moves_a_fifth_of_the_way_to_its_text() {
    assert_eq!(neutral_border(0x00_00_00_00, 0x00_FF_FF_FF), 0x00_33_33_33);
}

// And darkens on a light one, from the same rule.
#[test]
fn a_light_surface_moves_a_fifth_of_the_way_to_its_text() {
    assert_eq!(neutral_border(0x00_FF_FF_FF, 0x00_00_00_00), 0x00_CC_CC_CC);
}

// Each channel mixes on its own, so a tinted palette keeps its hue in the edge.
#[test]
fn each_channel_mixes_on_its_own() {
    assert_eq!(neutral_border(0x00_50_00_00, 0x00_00_00_A0), 0x00_40_00_20);
}

#[test]
fn system_paints_the_os_accent_on_a_focused_window() {
    assert_eq!(system_border(Some(ACCENT), 0x00_33_33_33).0, ACCENT);
}

/// Windows takes an inactive window's border back to neutral whatever the accent setting, and an
/// outline that kept the accent would be the one window on screen still looking focused.
#[test]
fn system_goes_neutral_on_an_unfocused_window_even_with_an_os_accent() {
    assert_eq!(system_border(Some(ACCENT), 0x00_33_33_33).1, 0x00_33_33_33);
}

#[test]
fn system_is_neutral_throughout_where_the_os_has_no_accent_on_borders() {
    assert_eq!(system_border(None, 0x00_33_33_33), (0x00_33_33_33, 0x00_33_33_33));
}

// A pick is the user's colour, focused or not, whatever the OS would draw.
#[test]
fn a_picked_accent_paints_both_states_over_the_os_accent() {
    assert_eq!(
        border_colors(Some(0x00_A6_E3_A1), Some(ACCENT), 0x00_33_33_33),
        (0x00_A6_E3_A1, 0x00_A6_E3_A1)
    );
}

#[test]
fn no_pick_paints_system() {
    assert_eq!(
        border_colors(None, Some(ACCENT), 0x00_33_33_33),
        system_border(Some(ACCENT), 0x00_33_33_33)
    );
}

#[test]
fn system_is_the_first_slot() {
    assert_eq!(swatch_index(theme(), WINDOW_BORDER_SYSTEM_COLOR), 0);
}

/// A pick made under a theme that had it, read back under one that doesn't, has to land on the
/// colour it paints as rather than on nothing selected.
#[test]
fn an_id_the_theme_does_not_have_selects_system() {
    assert_eq!(swatch_index(theme(), "not-an-accent"), 0);
}

#[test]
fn the_accents_follow_system_in_the_themes_order() -> Result<(), AppError> {
    let (Some(first), Some(last)) = (theme().accents.first(), theme().accents.last()) else {
        return Err(AppError::NotFound("the fixture theme ships no accents".to_owned()));
    };

    assert_eq!(swatch_index(theme(), first.id), 1);
    assert_eq!(swatch_index(theme(), last.id), theme().accents.len());
    Ok(())
}

#[test]
fn clicking_the_first_slot_picks_system() {
    assert_eq!(color_id_at(theme(), 0), Some(WINDOW_BORDER_SYSTEM_COLOR));
}

// Both ends of the accent run, where an off-by-one in the System offset shows.
#[test]
fn clicking_an_accent_slot_picks_that_accent() -> Result<(), AppError> {
    let (Some(first), Some(last)) = (theme().accents.first(), theme().accents.last()) else {
        return Err(AppError::NotFound("the fixture theme ships no accents".to_owned()));
    };

    assert_eq!(color_id_at(theme(), 1), Some(first.id));
    assert_eq!(color_id_at(theme(), theme().accents.len()), Some(last.id));
    Ok(())
}

// A stale index from a theme with more accents picks nothing rather than a neighbour.
#[test]
fn clicking_past_the_grid_picks_nothing() {
    assert_eq!(color_id_at(theme(), theme().accents.len() + 1), None);
}

/// The native titlebar still drops its frame for the miniplayer, so a row greyed out under it puts
/// the one outline that mode draws out of reach.
#[test]
fn no_border_row_greys_out_under_the_native_titlebar() -> Result<(), AppError> {
    let section = code_tokens(CHROME_SECTION);
    let (Some(toggle), Some(color)) =
        (row_body(&section, TOGGLE_ROW), row_body(&section, COLOR_ROW))
    else {
        return Err(AppError::NotFound(
            "a window border row is no longer mounted where this pin looks".to_owned(),
        ));
    };

    let gated = [toggle, color].iter().any(|row| row.contains("native-titlebar"));

    assert!(!gated, "a window border row reads the titlebar mode:\n{toggle}\n{color}");
    Ok(())
}

/// Under the native titlebar the OS frame outlines the full window, and a description promising the
/// whole window there describes an outline the toggle never draws.
#[test]
fn the_border_description_names_only_the_miniplayer_under_the_native_titlebar() {
    let section = code_tokens(CHROME_SECTION);

    let desc = binding_value(&section, "property <string> window-border-desc:").trim();

    assert!(
        desc.starts_with(
            r#"root.native-titlebar ? @tr("Draw a thin outline around the miniplayer."#
        ),
        "the native titlebar's border description no longer limits itself to the miniplayer:\n{desc}"
    );
}

/// Rust names the System slot with an empty string, `@tr` translating only literals, so the mount
/// has to name it or the swatch's tooltip is blank.
#[test]
fn the_system_swatch_is_named_at_the_mount() {
    let section = code_tokens(CHROME_SECTION);

    let named = section.matches(r#"first-label: @tr("System");"#).count();

    assert_eq!(named, 1, "the border grid no longer names its System swatch");
}

/// The grid's half: a `first-label` it never reads names nothing.
#[test]
fn the_grid_names_its_first_swatch_by_first_label_when_given_one() {
    let grid = code_tokens(DOT_GRID);

    let label = binding_value(&grid, "label: i == 0 &&").trim();

    assert_eq!(
        label,
        r#"root.first-label != "" ? root.first-label : i < root.labels.length ? root.labels[i] : """#,
        "the colour grid no longer lets `first-label` name its first swatch"
    );
}
