//! Artist Detail row-selection — the per-view adapter over the generic
//! [`crate::ui::detail_selection`] logic.

use super::ArtistsUi;
use crate::ui::detail_selection::impl_detail_selection;
use melodia_ui::ArtistDetail;

impl_detail_selection!(ArtistsUi, ArtistDetail);
