//! The strips' and the Top Result's cover lookups; result rows resolve through `RowCovers` like
//! every list's.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::search::SearchUi;
use melodia_ui::{AppWindow, Search};

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
    // Each kind asks its own strip's tier. A genre or a coverless result still asks, with an
    // empty path that loads nothing from either, so the `else` needs no genre arm.
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
