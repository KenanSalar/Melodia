//! The Artist Detail's state. The grid's is `ui::card_grid`'s.

use parking_lot::Mutex;

use crate::ui::track_detail::DetailCache;
use melodia_core::entities::album::AlbumStats;
use melodia_ui::ArtistDetail;

/// Detail-side state: the shared track list's cache plus the Albums strip only this detail has.
pub(super) struct ArtistDetailState {
    pub cache: DetailCache<ArtistDetail<'static>>,
    /// The artist's albums, unfiltered: the filter pass narrows the strip from this.
    pub albums: Mutex<Vec<AlbumStats>>,
}
