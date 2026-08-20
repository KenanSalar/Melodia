//! What `boot::ui_setup::install_views` hands every view slice's `install`.

use std::sync::Arc;

use crate::AppWindow;
use crate::media::cover_thumbs::CoverThumbs;
use crate::services::view_state::ViewStateData;
use crate::state::AppState;

/// The window, the app state, the shared row-tier cover cache and `views.json` as read
/// once at boot.
///
/// One value rather than four parameters so each slice's `install` signature differs only
/// in **which peers it needs**: `artists::install(cx, &albums_ui)` doesn't resolve until
/// `albums_ui` is bound, so "Albums before Artists" stops being a comment a later edit can
/// reorder past.
///
/// Ten, not eleven — `ui::my_library::install` takes a bare `(&AppWindow, &AppState)`,
/// needing neither the cover cache nor `views.json` and having to run *before* the five
/// tab slices, above the point where the cache exists to build a `cx` from.
#[derive(Clone, Copy)]
pub struct ViewCtx<'a> {
    pub app: &'a AppWindow,
    pub state: &'a AppState,
    /// The one row-tier LRU every track table shares. Each slice clones it into its
    /// handle; the private grid / mosaic tiers stay the handle's.
    pub cover_thumbs: &'a Arc<CoverThumbs>,
    /// `None` on a fresh install *and* on an unreadable file — a slice seeding
    /// from it falls back to its Slint-declared default.
    pub view_state: Option<&'a ViewStateData>,
}
