//! Wires a cover tier's decoded-batch signal to the Slint generation a card binding reads.
//!
//! The lookups are `pure` callbacks, cached until a dependency dirties, so an `int` the binding
//! also reads is what brings a scheduled decode to screen: one bump per batch, not per cover.

use std::sync::Arc;

use slint::ComponentHandle;

use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_ui::AppWindow;

/// Calls `bump` on the UI thread whenever `tier` finishes a batch a miss handed it. A closure
/// rather than a trait, `bump` being all that differs between tiers.
pub fn notify_on_decode(
    tier: &CoverThumbs,
    app: &AppWindow,
    bump: impl Fn(&AppWindow) + Send + Sync + 'static,
) {
    let weak = app.as_weak();
    // The notifier is an `Fn`, so each batch needs its own handle for the event loop.
    let bump = Arc::new(bump);
    tier.set_decoded_notifier(move || {
        let bump = Arc::clone(&bump);
        // Fails only once the event loop is gone, with nothing left to repaint.
        let _ = weak.upgrade_in_event_loop(move |app| bump(&app));
    });
}
