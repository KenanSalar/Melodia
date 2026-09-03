use super::accent_brushes;
use crate::themes::get;

/// One swatch per accent the theme declares, for each of the three accent-count shapes in the
/// registry. It sits here rather than beside the registry tables because the count it checks is
/// the picker's, and `accent_brushes` is the only half of the theme pipeline that builds a
/// `Brush`.
#[test]
fn accent_brushes_returns_one_per_accent() {
    let theme = get("catppuccin");
    let brushes = accent_brushes(theme, "mocha");
    assert_eq!(brushes.len(), 14);

    let gnome = get("gnome-adwaita");
    assert_eq!(accent_brushes(gnome, "dark").len(), 9);

    let macos = get("macos");
    assert_eq!(accent_brushes(macos, "light").len(), 8);
}
