//! Headphone correction's library API, and the only door onto `AutoEq`.
//!
//! **`autoeq_client` may not move.** It is the one read of `correction_online`, so a submodule
//! reaching `state.http_client()` directly is traffic a user who left the switch off still pays.
//! Import reads only what the user hands it, and works with the switch off.

mod apply;
mod list;
mod parametric;

pub use apply::{ImportError, Imported, apply, apply_from_list, import_file, import_paste};
pub use list::{CachedList, ListAnswer, ListCheck, cached_list, recheck_list};

use std::sync::Arc;

use crate::library;
use crate::state::AppState;
use melodia_core::entities::correction::DeviceCorrections;
use melodia_core::error::AppError;

/// The device a write lands on, as the card named it when the user acted: a profile still
/// downloading as the output moves lands where it was asked for.
pub struct CorrectionTarget {
    /// `None` is System default.
    pub device: Option<String>,
    /// Stored as the correction's `device_name`.
    pub device_name: Option<String>,
}

pub fn clear(state: &AppState, device: Option<&str>) {
    commit(state, |corrections| corrections.remove(device));
}

/// The shared client, reachable only while finding corrections online is switched on.
fn autoeq_client(state: &AppState) -> Result<&reqwest::Client, AppError> {
    if !state.correction_online.get() {
        return Err(AppError::settings_msg("Finding corrections online is switched off"));
    }
    Ok(state.http_client())
}

/// Every write to the map: the shadow here, then the engine and the file handed the map as it
/// stands, so they take writes in the order the shadow did.
fn commit(state: &AppState, edit: impl FnOnce(&mut DeviceCorrections)) {
    edit(&mut state.corrections.picked());
    let ctx = state.playback_ctx();
    let shadow = Arc::clone(&state.corrections);
    state.persist_blocking("persist corrections", move |paths| {
        shadow.apply_latest(DeviceCorrections::clone, |corrections| {
            library::playback::player_set_corrections(&ctx, corrections.clone());
            library::settings::set_corrections(paths, corrections)
        })
    });
}
