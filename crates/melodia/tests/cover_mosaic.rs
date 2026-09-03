//! The cover mosaic is the pickers' alone.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// `CoverMosaic` lays a live 2×2 out in Slint, and this banner used to draw one — four lazy
/// per-tile lookups beside a second composition of the same covers for the backdrop. It draws one
/// composed collage now, and this stops that branch returning: the picker is the component's whole
/// remaining audience, and it wants the live form, its tiles following an uncomposed selection.
#[test]
fn the_cover_mosaic_is_the_pickers_alone() {
    let mounts: Vec<String> = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
        .into_iter()
        .filter(|(_, src)| src.contains("CoverMosaic {"))
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        mounts,
        ["components/dialog/playlist-mosaic-picker.slint"],
        "only the playlist mosaic picker may mount `CoverMosaic` — a hero wanting a live \
         mosaic is a hero composing its covers twice"
    );
}
