//! `RecentlyPlayed` lazy cover-lookup callbacks — the hero mosaic tiles and
//! the Most Played strip cards. (List rows resolve through the shared
//! `RowCovers` global like every other `TrackListRowItem`.) See
//! [`super::wire_recently_played`].

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
        let ru = rp_ui.clone();
        g.on_request_most_played_cover(move |path| ru.most_played_cover(path.as_str()));
    }
}
