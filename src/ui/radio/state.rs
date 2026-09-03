//! The page's own state, in the shape every other view keeps it in.
//!
//! `albums/state.rs`, `favorites/state.rs` and `recently_played/state.rs` are the siblings, and
//! the reason is theirs: the public `RadioUi` surface stays small while the caches behind it are
//! documented where they live. This page skipped the split and carried its fifteen fields in the
//! module's own front door.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::artwork_cache::BlurSpec;
use crate::ui::detail_artwork::DetailArtwork;
use crate::ui::section_state::SectionState;

use super::browse::BrowseState;
use super::logos::LogoMemo;
use super::{covers, detail, facets, history, kept};

/// Rust-side state for the Radio page.
pub struct RadioUi {
    /// Whether the page is on screen. **Seeded at wire time rather than left to the gate**, which
    /// fires on transitions only and whose `ChangeTracker` baselines silently inside
    /// `AppWindow::new()` — a section seeded wrong has no edge left to correct it.
    ///
    /// The dirty flag rides with it, and describes the **hero and nothing else**. What a leave
    /// hands back here is the logo tier and, where a station page is open, the hero's decoded
    /// images and the colours it published into the two globals six heroes share — the three
    /// grids and the directory page survive, which is this page's whole departure from the
    /// tabbed-page contract. The enter re-warms the tier unconditionally and rebuilds the hero
    /// only when the flag says it was given up.
    pub(super) section: SectionState,
    /// The directory page on screen, and the query it answers.
    pub(super) browse: Mutex<BrowseState>,
    /// The directory uuids this install has starred. Refreshed on section enter and flipped
    /// optimistically by the toggle, which is what lets the star respond on the click's own frame.
    ///
    /// Derived from the same fetch that fills [`Self::kept`], since a starred station *is* a kept
    /// one — a query of its own would be a second answer to keep true.
    pub(super) starred: Mutex<HashSet<String>>,
    /// The logo every kept station already has, keyed on directory uuid.
    ///
    /// **A row answers a question the memo cannot.** The memo is keyed on the `favicon_url` a
    /// browse asked about, so it only ever holds what *that URL* returned this session; a station
    /// whose favicon 404s and whose logo was found on its own site has an `artwork_path` and no
    /// memo entry, and a page carrying it drew a monogram beside the two local tabs painting the
    /// real thing. Filled from the same fetch as [`Self::starred`], for the same reason.
    pub(super) known_logos: Mutex<HashMap<String, String>>,
    /// The station ids the logo repair has already asked about this session.
    ///
    /// `kept::refresh` runs on a section enter, on every star flip — Browse's included — and on
    /// every removal, and the repair walks every logo-less row each time. A station that failed is
    /// a stored backoff, so the repeat buys two queries and the same answer; this is that answer
    /// held where a click cannot spend a round trip on it.
    pub(super) healed: Mutex<HashSet<i64>>,
    /// The Favorites tab: everything starred, plus every station typed in by hand.
    pub(super) kept: Mutex<kept::KeptState>,
    /// The Recently Played tab: every station with a play behind it, starred or not.
    pub(super) recent: Mutex<kept::KeptState>,
    /// What this session knows about station logos, keyed on the URL they came from.
    pub(super) logos: LogoMemo,
    /// The open picker's list, whole. Kept beside the Slint model because the picker's needle
    /// narrows it and Slint cannot filter an array, so every keystroke rebuilds the model from
    /// here rather than re-asking the facade across the runtime.
    pub(super) facet_list: Mutex<Option<Arc<[crate::entities::radio::Facet]>>>,
    /// All four directory lists at once, filled by `facets::prime` on the first section enter.
    ///
    /// The sibling above is whichever list the open picker is narrowing and answers a question
    /// about one chip; this answers a question about the *needle*, which is why it has to hold
    /// every list and to be readable without an `.await` — the scope suggestions are recomputed on
    /// each settled keystroke, on the UI thread.
    pub(super) facet_index: Mutex<facets::FacetIndex>,
    /// A station page per tab, since one opens from all three and a tab move must not evict what
    /// another is holding. `detail.rs` owns the shape.
    pub(super) detail: Mutex<detail::DetailState>,
    /// Held by the station-detail writer for the length of its `views.json` write.
    ///
    /// **That is the ordering** — two `spawn_blocking` tasks have none of their own, and with a
    /// name written on every tab move a bounce queues alternating values that can land reversed,
    /// naming the station the restored tab is *not* showing. `IndexPersist`'s shape minus the
    /// atomic, which is what the tab index beside it takes instead: the value a writer reloads
    /// under this is an `Option<i64>` and already lives beside the seats.
    pub(super) persist_writer: Mutex<()>,
    /// The titles the station currently playing has announced. Not the detail's, and deliberately
    /// outside it: the ring fills whether or not anybody has the page open, which is the only way
    /// it can be there to read when they do.
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
            known_logos: Mutex::new(HashMap::new()),
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

    /// Mark the hero stale. Written synchronously on the UI thread at section leave, before the
    /// release task is spawned.
    pub(super) fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Give the hero's decode tier back, and walk the arena after it. The Slint image slots are
    /// the leave's own.
    ///
    /// **The trim is part of the job, not a caller's follow-up**, which is why the one leave that
    /// releases no tier (`callbacks::lifecycle`) has to ask for it separately. Every caller is
    /// already on the blocking pool, where a `malloc_trim` may run; none may call this from the
    /// event loop.
    pub(super) fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
        crate::services::allocator::trim();
    }

    /// Whether the page is the section on screen.
    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Let the logo repair ask about a station again.
    ///
    /// [`Self::healed`] is what stops a refresh re-asking about a station whose backoff already
    /// answered, and a website or logo URL the user has just typed is precisely the new evidence
    /// that claim was made without. Without this the field they filled in has no effect until the
    /// next launch.
    pub(super) fn forget_heal(&self, id: i64) {
        self.healed.lock().remove(&id);
    }

    /// Flip a station's star in the shadow the grid is built from.
    ///
    /// The optimistic half of the toggle, and its revert: the write is a round trip through
    /// `SQLite` and the star has to answer on the click's own frame.
    pub(super) fn set_local_favorite(&self, station_uuid: &str, favorite: bool) {
        let mut starred = self.starred.lock();
        if favorite {
            starred.insert(station_uuid.to_owned());
        } else {
            starred.remove(station_uuid);
        }
    }
}
