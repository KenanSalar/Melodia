//! The Most Played grid both curated pages draw, Favorites' over the favourites and Recently
//! Played's over the whole library: its card and its rows by id.
//!
//! Each page implements its own `CardGrid` over these, the two differing in their ranking's
//! membership and in what their band states beside it.

use slint::{ModelRc, SharedString};

use crate::ui::paged_rows::PagedRow;
use crate::ui::paged_rows::grid::GridRow;
use crate::ui::util::{clamp_i64_to_i32, opt_shared};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::track::MostPlayedFavorite;
use melodia_core::error::AppResult;
use melodia_ui::{EntityGridRow as UiEntityGridRow, EntityStripRow as UiEntityStripRow};

impl GridRow for UiEntityGridRow {
    type Card = UiEntityStripRow;

    fn of(entities: ModelRc<UiEntityStripRow>) -> Self {
        Self { entities }
    }
}

impl PagedRow for UiEntityStripRow {
    // Blank but live: a card clicked before its fetch lands still plays and picks by id.
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), ..Self::default() }
    }
}

/// A Most Played card. Subtitle is the artist name; `play_count` rides in the `play_count` slot
/// so the grid's `show-play-count: true` reveals the badge.
pub fn to_slint_most_played_row(t: &MostPlayedFavorite) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(t.id),
        title: SharedString::from(t.title.as_str()),
        subtitle: opt_shared(t.artist.as_deref()),
        artwork_path: opt_shared(t.artwork_path.as_deref()),
        play_count: t.play_count,
        is_favorite: t.is_favorite,
    }
}

/// The Most Played cards behind `ids`, in the order asked.
pub async fn rows(state: AppState, ids: Vec<i64>) -> AppResult<Vec<MostPlayedFavorite>> {
    library::keyset::most_played_by_ids(&state.db, &ids).await
}
