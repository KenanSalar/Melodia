//! `Search.*` lazy cover-lookup callbacks — album strip, artist strip,
//! and the kind-routed Top Result tile. (Result rows resolve through the
//! shared `RowCovers` global like every other `TrackListRowItem`.) See
//! [`super::wire`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::search::SearchUi;
use crate::{AppWindow, Search};

/// Wire the three `request-*-cover` callbacks.
pub(super) fn wire(ui: &AppWindow, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();

    {
        let su = search_ui.clone();
        g.on_request_album_strip_cover(move |path| su.album_strip_cover(path.as_str()));
    }
    {
        let su = search_ui.clone();
        g.on_request_artist_strip_cover(move |path| su.artist_strip_cover(path.as_str()));
    }
    // Top Result cover routes per kind — album → album-strip tier,
    // artist → artist-strip tier. Cache parity with the strip below
    // means clicking a strip card after the top card hits a warm LRU.
    // A genre still reaches this, with an empty path — `ArtworkImage`
    // reads `cover` in every one of its branch conditions, so the card's
    // binding evaluates whatever the kind is. `get_or_load_opt` filters
    // the empty string to `Image::default()`, so which tier it lands in
    // is a no-op and the card paints its gradient branch. Same for a
    // coverless album or artist, which is why the `else` needs no arm.
    {
        let su = search_ui.clone();
        let weak = weak.clone();
        g.on_request_top_result_cover(move |path| {
            let Some(ui) = weak.upgrade() else {
                return slint::Image::default();
            };
            let kind = ui.global::<Search>().get_top_kind();
            if kind.as_str() == "artist" {
                su.artist_strip_cover(path.as_str())
            } else {
                su.album_strip_cover(path.as_str())
            }
        });
    }
}
