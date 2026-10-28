//! What `boot::ui_setup::views::install_views` hands every view slice's `install`.

use std::sync::Arc;

use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_ui::AppWindow;

/// The window, the app state, the shared row-tier cover cache and `views.json` as read once at
/// boot. One value, so each slice's `install` signature differs only in **which peers it
/// needs**: `artists::install(cx, &albums_ui)` doesn't resolve until `albums_ui` is bound, so
/// the order can't be edited past. `ui::my_library::install` takes none of it, running before
/// the tab slices and the cache both.
#[derive(Clone, Copy)]
pub struct ViewCtx<'a> {
    pub app: &'a AppWindow,
    pub state: &'a AppState,
    /// The row-tier LRU every track table shares. The card grids' is `grid_prewarm::tier()`,
    /// there being one of it and no view owning it.
    pub cover_thumbs: &'a Arc<CoverThumbs>,
    /// `None` on a fresh install *and* on an unreadable file — a slice seeding
    /// from it falls back to its Slint-declared default.
    pub view_state: Option<&'a ViewStateData>,
}
