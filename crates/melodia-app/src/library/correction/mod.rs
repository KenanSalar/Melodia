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
use melodia_core::entities::correction::{Correction, DeviceCorrections};
use melodia_core::error::AppError;

/// The device a write lands on, as the card named it when the user acted: a profile still
/// downloading as the output moves lands where it was asked for.
#[derive(Clone, Debug)]
pub struct CorrectionTarget {
    /// `None` is System default.
    pub device: Option<String>,
    /// Stored as the correction's `device_name`.
    pub device_name: Option<String>,
}

/// The device the correction plays for, as the card names it.
#[derive(Clone, Debug)]
pub struct Heard {
    pub target: CorrectionTarget,
    /// Whether the listing found it. One it didn't is named by what its correction stored, if
    /// anything; System default always counts as found.
    pub connected: bool,
}

/// Blocking while a device is heard: naming it asks every device.
pub fn heard(state: &AppState) -> Heard {
    let Some(id) = library::playback::player_heard_device(&state.playback_ctx()) else {
        let target = CorrectionTarget { device: None, device_name: None };
        return Heard { target, connected: true };
    };
    let listed = library::playback::output_devices()
        .into_iter()
        .find(|device| device.id == id)
        .map(|device| device.name);
    let connected = listed.is_some();
    let device_name = listed
        .or_else(|| correction_for(state, Some(&id)).and_then(|correction| correction.device_name));
    Heard { target: CorrectionTarget { device: Some(id), device_name }, connected }
}

/// The correction set for `device` as last written, `None` naming System default.
pub fn correction_for(state: &AppState, device: Option<&str>) -> Option<Correction> {
    state.corrections.picked().for_device(device).cloned()
}

/// Hands back what it took, so an undo restores exactly that.
pub fn clear(state: &AppState, device: Option<&str>) -> Option<Correction> {
    commit(state, |corrections| corrections.remove(device))
}

/// Puts back a correction [`clear`] took.
pub fn restore(state: &AppState, device: Option<String>, correction: Correction) {
    commit(state, |corrections| corrections.insert(device, correction));
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
fn commit<R>(state: &AppState, edit: impl FnOnce(&mut DeviceCorrections) -> R) -> R {
    let edited = edit(&mut state.corrections.picked());
    let ctx = state.playback_ctx();
    let shadow = Arc::clone(&state.corrections);
    state.persist_blocking("persist corrections", move |paths| {
        shadow.apply_latest(DeviceCorrections::clone, |corrections| {
            library::playback::player_set_corrections(&ctx, corrections.clone());
            library::settings::set_corrections(paths, corrections)
        })
    });
    edited
}
