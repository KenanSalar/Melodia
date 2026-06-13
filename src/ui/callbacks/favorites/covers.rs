//! `Favorites.*` lazy cover-lookup callbacks — one per card tier
//! (mosaic, most-played, artist). (All Songs rows resolve through the
//! shared `RowCovers` global like every other `TrackListRowItem`.)
//! See [`super::wire_favorites`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::favorites::FavoritesUi;
use crate::{AppWindow, Favorites};

/// Wire the three `request-*-cover` callbacks.
pub(super) fn wire(ui: &AppWindow, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();

    {
        let fu = fav_ui.clone();
        g.on_request_mosaic_cover(move |path| fu.mosaic_cover(path.as_str()));
    }
    {
        let fu = fav_ui.clone();
        g.on_request_most_played_cover(move |path| fu.most_played_cover(path.as_str()));
    }
    {
        let fu = fav_ui.clone();
        g.on_request_artist_cover(move |path| fu.artist_cover(path.as_str()));
    }
}
