//! Playlist Detail row-selection — the per-view adapter over the generic
//! [`crate::ui::detail_selection`] logic.

use super::PlaylistsUi;
use crate::ui::detail_selection::impl_detail_selection;
use melodia_ui::PlaylistDetail;

impl_detail_selection!(PlaylistsUi, PlaylistDetail);
