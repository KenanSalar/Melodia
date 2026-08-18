//! Recently-Played row selection: modifier-aware clicks, clear, and the per-row `selected` flag
//! writer. Thin adapter over [`crate::ui::list_selection`], stamped from the same body as
//! `favorites::selection` — the two were identical.

use super::RecentlyPlayedUi;
use crate::RecentlyPlayed;
use crate::ui::list_selection::impl_row_selection;

impl_row_selection!(RecentlyPlayed, RecentlyPlayedUi);
