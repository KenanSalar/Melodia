//! `Favorites.*` lazy cover-lookup callbacks — one per grid tab, Most Played
//! and Artists. (Songs-tab rows resolve through the shared `RowCovers` global
//! like every other `TrackListRowItem`.) See [`super::wire`].
//!
//! The two grid lookups carry `Favorites.covers-generation`, which is both
//! what makes their `pure` bindings re-evaluate when a prewarm lands and the
//! "is this tier warm" flag itself — see [`FavoritesUi::artist_cover`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::favorites::FavoritesUi;
use crate::{AppWindow, Favorites};

/// Wire the two `request-*-cover` callbacks.
pub(super) fn wire(ui: &AppWindow, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();

    {
        let fu = fav_ui.clone();
        g.on_request_most_played_cover(move |path, generation| {
            fu.most_played_cover(path.as_str(), generation)
        });
    }
    {
        let fu = fav_ui.clone();
        g.on_request_artist_cover(move |path, generation| {
            fu.artist_cover(path.as_str(), generation)
        });
    }
}
