//! The page's own state, apart from the module's front door as the other tabbed pages keep theirs.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::ui::artwork_cache::BlurSpec;
use crate::ui::detail_artwork::DetailArtwork;
use crate::ui::section_state::SectionState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;

use super::browse::BrowseState;
use super::logos::LogoMemo;
use super::{covers, detail, facets, history, kept};

/// What a station's stored row tells Browse that a directory answer cannot.
#[derive(Default)]
pub(super) struct KeptAnswers {
    /// The logo the row holds.
    pub(super) logo: Option<String>,
    /// What an open measured the station to hold.
    pub(super) format: Option<String>,
}

/// Rust-side state for the Radio page.
pub struct RadioUi {
    /// Whether the page is on screen, seeded at wire time because the gate fires on transitions
    /// only and baselines silently inside `AppWindow::new()`.
    ///
    /// Its dirty flag describes the hero alone: a leave hands back the logo tier and any seated
    /// hero's images and published colours, keeping the grids and the directory page, so the enter
    /// re-warms the tier unconditionally and rebuilds the hero only when the flag says so.
    pub(super) section: SectionState,
    /// The directory page on screen, and the query it answers.
    pub(super) browse: Mutex<BrowseState>,
    /// The directory uuids this install has starred, flipped optimistically so the star answers on
    /// the click's own frame. Derived from the fetch that fills [`Self::kept`].
    pub(super) starred: Mutex<HashSet<String>>,
    /// What a kept station's own row answers about it, keyed on directory uuid: one map for both
    /// answers, off the same fetch and stale on the same write.
    pub(super) kept_answers: Mutex<HashMap<String, KeptAnswers>>,
    /// The station ids the logo repair has asked about this session, so a refresh on every enter,
    /// star flip and removal doesn't re-ask what a stored backoff already answered.
    pub(super) healed: Mutex<HashSet<i64>>,
    /// The Favorites tab: everything starred, plus every station typed in by hand.
    pub(super) kept: Mutex<kept::KeptState>,
    /// The Recently Played tab: every station with a play behind it, starred or not.
    pub(super) recent: Mutex<kept::KeptState>,
    /// What this session knows about station logos, keyed on the URL they came from.
    pub(super) logos: LogoMemo,
    /// The open picker's list, whole, which every keystroke narrows into the model.
    pub(super) facet_list: Mutex<Option<Arc<[melodia_core::entities::radio::Facet]>>>,
    /// All four directory lists, filled by `facets::prime` and readable without an `.await` for
    /// the suggestions each settled keystroke recomputes.
    pub(super) facet_index: Mutex<facets::FacetIndex>,
    /// A station page per tab; `detail.rs` owns the shape.
    pub(super) detail: Mutex<detail::DetailState>,
    /// Held by the station-detail writer across its `views.json` write, the ordering two
    /// `spawn_blocking` tasks lack, or a tab bounce lands the names reversed. `IndexPersist`'s
    /// shape, the value it reloads living beside the seats rather than in an atomic.
    pub(super) persist_writer: Mutex<()>,
    /// The playing station's announced titles, outside the detail because the ring fills whether
    /// or not the page is open.
    pub(super) history: Mutex<history::StationHistory>,
    /// The grid tier the cards decode into. Released by the section leave.
    pub(super) covers: Arc<CoverThumbs>,
    /// The hero's own tier, at detail size and with its blur half. Separate from [`Self::covers`]
    /// for `AlbumsUi`'s reason: one is a page of small cards, the other a single large tile.
    pub(super) detail_artwork: Arc<DetailArtwork>,
}

impl RadioUi {
    pub(super) fn new(section_active: bool, hero_blur: Option<BlurSpec>) -> Self {
        let section = SectionState::new();
        section.set_active(section_active);
        Self {
            section,
            browse: Mutex::new(BrowseState::default()),
            starred: Mutex::new(HashSet::new()),
            kept_answers: Mutex::new(HashMap::new()),
            healed: Mutex::new(HashSet::new()),
            kept: Mutex::new(kept::KeptState::default()),
            recent: Mutex::new(kept::KeptState::default()),
            logos: LogoMemo::new(),
            facet_list: Mutex::new(None),
            facet_index: Mutex::new(facets::FacetIndex::default()),
            detail: Mutex::new(detail::DetailState::default()),
            persist_writer: Mutex::new(()),
            history: Mutex::new(history::StationHistory::default()),
            covers: Arc::new(covers::new_tier()),
            detail_artwork: Arc::new(DetailArtwork::new(hero_blur)),
        }
    }

    /// Marks the hero stale, synchronously on the leave, before the release task spawns.
    pub(super) fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Gives the hero's decode tier back and trims the arena; the Slint image slots are the
    /// leave's. Blocking pool only, a `malloc_trim` having no place on the event loop.
    pub(super) fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
        melodia_platform::services::platform::allocator::trim();
    }

    /// Whether the page is the section on screen.
    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Lets the logo repair ask about a station again, once the user has typed a website or logo
    /// URL that [`Self::healed`]'s answer was made without.
    pub(super) fn forget_heal(&self, id: i64) {
        self.healed.lock().remove(&id);
    }

    /// Flips a station's star in the grid's shadow: the optimistic half of the toggle, and its
    /// revert.
    pub(super) fn set_local_favorite(&self, station_uuid: &str, favorite: bool) {
        let mut starred = self.starred.lock();
        if favorite {
            starred.insert(station_uuid.to_owned());
        } else {
            starred.remove(station_uuid);
        }
    }
}
