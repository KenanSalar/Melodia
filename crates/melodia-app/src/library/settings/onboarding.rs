//! The welcome card's persisted revision.

use crate::services;
use crate::services::settings::ONBOARDING_VERSION;
use crate::state::AppState;
use melodia_core::error::AppError;

/// Record that the card has been seen, at the revision this build ships.
///
/// A pure disk write: the overlay is already closing and nothing else in the session reads
/// the field back. Every dismissal path lands here — an X, the backdrop, Escape and Skip all
/// mean the same thing, since a card that returns because it was closed early is a nag.
pub fn set_onboarding_seen(state: &AppState) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, |settings| {
        settings.onboarding.onboarding_version = ONBOARDING_VERSION;
    })
}
