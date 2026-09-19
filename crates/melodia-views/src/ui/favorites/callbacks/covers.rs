//! `Favorites.*` lazy cover-lookup callbacks — one per grid tab, Most Played
//! and Artists. (Songs-tab rows resolve through the shared `RowCovers` global
//! like every other `TrackListRowItem`.) See [`super::wire`].
//!
//! The two grid lookups carry `Favorites.covers-generation`, which is both
//! what makes their `pure` bindings re-evaluate when a prewarm lands and the
//! "is this tier warm" flag itself — see [`FavoritesUi::artist_cover`].

use slint::ComponentHandle;

use melodia_ui::{AppWindow, Favorites};

/// Wire the two `request-*-cover` callbacks.
///
/// `generation` is read for its effect on the binding, never its value — the tier is shared and
/// the one notifier that moves every grid's counter is
/// `boot::ui_setup::views::install_grid_covers`.
pub(super) fn wire(ui: &AppWindow) {
    let g = ui.global::<Favorites>();
    g.on_request_most_played_cover(|path, _generation| {
        crate::ui::grid_prewarm::grid_cover(path.as_str())
    });
    g.on_request_artist_cover(|path, _generation| {
        crate::ui::grid_prewarm::grid_cover(path.as_str())
    });
}
