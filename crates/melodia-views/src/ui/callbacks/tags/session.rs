//! What one open of the dialog carries between its handlers.

use std::path::PathBuf;

use melodia_core::entities::tags::ArtworkEdit;

use super::form::{FormState, ListFields};

/// Snapshot of one dialog open, shared across the UI-thread handlers via `Rc<RefCell<_>>` (the
/// `sleep_timer` pattern). `request-edit` overwrites the whole thing, so nothing leaks across
/// opens.
#[derive(Default)]
pub(super) struct TagSession {
    pub ids: Vec<i64>,
    /// The whole form **as populated into the Slint properties**, so the commit diff is a plain
    /// field-by-field compare against what the user was shown.
    pub originals: FormState,
    /// The four list fields as populated. Structural rather than rendered lines, for the reason
    /// [`ListFields`] gives.
    pub original_lists: ListFields,
    /// Picker label paired with what it renders as, in the order the Slint `[string]` holds them.
    /// Seeded from `JOIN_PHRASES` and extended by whatever the opened files already use.
    pub join_phrases: Vec<(String, String)>,
    pub artwork: ArtworkEdit,
    /// The picked cover path — set only while `artwork == Replace`. Rides to the
    /// orchestrator as `apply_tag_edit`'s separate `artwork_source` arg.
    pub picked: Option<PathBuf>,
    /// The sheet this track already has somewhere other than its own tag, held from the open so
    /// the Insert button costs no read of its own. `None` where there is nothing to offer.
    pub resident_lyrics: Option<String>,
}
