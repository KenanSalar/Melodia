//! `RecentlyPlayed` lazy cover-lookup callbacks — the hero mosaic tiles and
//! the Most Played grid cards. (Songs rows resolve through the shared
//! `RowCovers` global like every other `TrackListRowItem`.) See
//! [`super::wire`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::recently_played::RecentlyPlayedUi;
use crate::{AppWindow, RecentlyPlayed};

/// Wire the `request-mosaic-cover` + `request-most-played-cover` callbacks.
pub(super) fn wire(ui: &AppWindow, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    {
        let ru = rp_ui.clone();
        g.on_request_mosaic_cover(move |path| ru.mosaic_cover(path.as_str()));
    }
    {
        // The second argument is `RecentlyPlayed.covers-generation`: reading it
        // is what makes the card's `pure` binding re-evaluate once the tier is
        // warmed behind an already-mounted grid, and its value is the is-it-warm
        // flag `grid_cover` branches on.
        let ru = rp_ui.clone();
        g.on_request_most_played_cover(move |path, generation| {
            ru.most_played_cover(path.as_str(), generation)
        });
    }
}
