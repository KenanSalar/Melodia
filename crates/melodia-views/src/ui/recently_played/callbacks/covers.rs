//! `RecentlyPlayed`'s lazy cover-lookup callback — the Most Played grid cards.
//! (Songs rows resolve through the shared `RowCovers` global like every other
//! `TrackListRowItem`.) See [`super::wire`].

use slint::ComponentHandle;

use melodia_ui::{AppWindow, RecentlyPlayed};

/// Wire the `request-most-played-cover` callback.
///
/// The second argument is `RecentlyPlayed.covers-generation`, read for its effect on the card's
/// `pure` binding and never for its value: it is what re-runs the lookup once a scheduled decode
/// lands behind an already-mounted grid.
pub(super) fn wire(ui: &AppWindow) {
    ui.global::<RecentlyPlayed>().on_request_most_played_cover(|path, _generation| {
        crate::ui::grid_prewarm::grid_cover(path.as_str())
    });
}
