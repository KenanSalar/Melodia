//! Genre Detail row-selection — the per-view adapter over the generic
//! [`crate::ui::detail_selection`] logic.

use super::GenresUi;
use crate::ui::detail_selection::impl_detail_selection;
use melodia_ui::GenreDetail;

impl_detail_selection!(GenresUi, GenreDetail);
