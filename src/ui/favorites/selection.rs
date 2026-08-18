//! Favorites All-Songs row selection: modifier-aware clicks, clear, and the per-row `selected`
//! flag writer that drives row highlight + checkbox state.
//!
//! Thin adapter over [`crate::ui::list_selection`] — the `TrackList` component reads each row's
//! `selected: bool` for its checkbox tick and accent-tinted background, so a click that only
//! updates `selected-ids` (without re-stamping the per-row flag) leaves the row visually
//! un-selected even though it counts toward the "{n} selected" chip.

use super::FavoritesUi;
use crate::Favorites;
use crate::ui::list_selection::impl_row_selection;

impl_row_selection!(Favorites, FavoritesUi);
