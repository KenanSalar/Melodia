//! Album Detail row-selection — the per-view adapter over the generic
//! [`crate::ui::detail_selection`] logic.

use super::AlbumsUi;
use crate::AlbumDetail;
use crate::ui::detail_selection::impl_detail_selection;

impl_detail_selection!(AlbumsUi, AlbumDetail);
