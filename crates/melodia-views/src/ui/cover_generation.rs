//! Wires a cover tier's decoded-batch signal to the Slint generation a card binding reads.
//!
//! The lazy cover lookups are `pure` callbacks, so Slint caches each result until one of the
//! binding's dependencies dirties. A tier that answers a miss by scheduling therefore has no way
//! to show what it decoded — nothing the binding read has changed. An `int` the binding also
//! reads is the whole mechanism: bumping it re-runs the callback, which now hits.
//!
//! One bump per landed batch rather than per cover, so a screenful of misses costs one pass over
//! the mounted bindings instead of one per row.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::AppWindow;
use crate::media::image::cover_thumbs::CoverThumbs;

/// Call `bump` on the UI thread whenever `tier` finishes a batch it was handed by a miss.
///
/// `bump` is the two-line read-modify-write of one global's generation, which is all that differs
/// between the tiers — Slint globals share no trait to be generic over.
pub fn notify_on_decode(
    tier: &Arc<CoverThumbs>,
    app: &AppWindow,
    bump: impl Fn(&AppWindow) + Send + Sync + 'static,
) {
    let weak = app.as_weak();
    // Shared rather than moved: the notifier is an `Fn`, so every batch needs its own handle to
    // hand the event loop.
    let bump = Arc::new(bump);
    tier.set_decoded_notifier(move || {
        let bump = Arc::clone(&bump);
        // The drain runs on the decode pool. A failure here is a gone event loop, which is the
        // one case with nothing left to repaint.
        let _ = weak.upgrade_in_event_loop(move |app| bump(&app));
    });
}
