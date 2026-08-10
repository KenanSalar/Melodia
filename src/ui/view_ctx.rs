//! What `boot::ui_setup::install_views` hands every view slice's `install`.

use std::sync::Arc;

use crate::AppWindow;
use crate::media::cover_thumbs::CoverThumbs;
use crate::services::view_state::ViewStateData;
use crate::state::AppState;

/// The window, the app state, the shared row-tier cover cache and
/// `views.json` as read once at boot.
///
/// One value rather than four parameters so each slice's `install` signature
/// differs only in the part that genuinely differs — **which peers it needs**.
/// That is the point of the shape: `artists::install(cx, &albums_ui)` does not
/// resolve until `albums_ui` is bound, so "Albums before Artists" stops being a
/// comment a later edit can reorder past.
///
/// `Copy` so passing it nine times reads like passing nothing.
#[derive(Clone, Copy)]
pub struct ViewCtx<'a> {
    pub app: &'a AppWindow,
    pub state: &'a AppState,
    /// The one 72 px row-tier LRU every track table shares. Each slice clones
    /// it into its handle; the private grid / mosaic tiers stay the handle's.
    pub cover_thumbs: &'a Arc<CoverThumbs>,
    /// `None` on a fresh install *and* on an unreadable file — a slice seeding
    /// from it falls back to its Slint-declared default.
    pub view_state: Option<&'a ViewStateData>,
}
