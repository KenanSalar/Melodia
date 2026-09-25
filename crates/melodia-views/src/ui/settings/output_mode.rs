//! The Output card's two pickers: shared or exclusive, and which card an exclusive claim takes.
//!
//! **A pick is applied on the blocking pool, never here**: claiming a card or handing it back
//! opens a device. Both pickers change the one choice, so each writes a synchronous shadow and
//! the task reads the shadow when it runs rather than capturing a value. Whichever task runs last
//! then applies and persists the latest pick, in whatever order the pool ran them.

use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::ui::settings_bind::read_or_default;
use melodia_app::library;
use melodia_app::services::settings::OutputModeKey;
use melodia_app::state::AppState;
use melodia_engine::player::engine::backend::OutputChoice;
use melodia_playback::player::playback::output::OutputDevice;
use melodia_ui::{AppWindow, Settings};

/// The choice as last picked, and the cards the device picker's indices point into.
struct Picked {
    mode: OutputModeKey,
    device: Option<String>,
    devices: Vec<OutputDevice>,
}

type Shadow = Arc<Mutex<Picked>>;

pub fn install(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Settings>();
    g.set_exclusive_supported(library::playback::EXCLUSIVE_SUPPORTED);
    if !library::playback::EXCLUSIVE_SUPPORTED {
        return;
    }

    let saved = read_or_default(state, "output mode").output;
    g.set_output_mode_idx(mode_index(saved.output_mode));
    let shadow: Shadow = Arc::new(Mutex::new(Picked {
        mode: saved.output_mode,
        device: saved.output_device,
        devices: Vec::new(),
    }));
    list_devices(ui, state, &shadow);

    let state_mode = state.clone();
    let shadow_mode = Arc::clone(&shadow);
    let weak = ui.as_weak();
    g.on_output_mode_changed(move |idx| {
        shadow_mode.lock().mode = mode_from_index(idx);
        // Cards come and go, and this is the moment the list is about to be looked at.
        if let Some(ui) = weak.upgrade() {
            list_devices(&ui, &state_mode, &shadow_mode);
        }
        apply(&state_mode, &shadow_mode);
    });

    let state_device = state.clone();
    g.on_output_device_changed(move |idx| {
        {
            let mut picked = shadow.lock();
            let chosen = usize::try_from(idx).ok().and_then(|i| picked.devices.get(i));
            picked.device = chosen.map(|device| device.id.clone());
        }
        apply(&state_device, &shadow);
    });
}

/// Apply the shadow's choice to the engine and persist it, off the UI thread.
fn apply(state: &AppState, shadow: &Shadow) {
    let ctx = state.playback_ctx();
    let shadow = Arc::clone(shadow);
    state.persist_blocking("persist output choice", move |state| {
        let (mode, device) = {
            let picked = shadow.lock();
            (picked.mode, picked.device.clone())
        };
        let choice = OutputChoice { mode: mode.into(), device: device.clone() };
        library::playback::player_set_output_choice(&ctx, choice);
        library::settings::set_output_choice(state, mode, device)
    });
}

/// Ask the cards for themselves off the UI thread, then fill the picker. A saved card that isn't
/// connected selects nothing, and the signal path says why.
fn list_devices(ui: &AppWindow, state: &AppState, shadow: &Shadow) {
    let weak = ui.as_weak();
    let shadow = Arc::clone(shadow);
    state.runtime.spawn_blocking(move || {
        let devices = library::playback::output_devices();
        let names: Vec<SharedString> =
            devices.iter().map(|device| device.name.as_str().into()).collect();
        let selected = {
            let mut picked = shadow.lock();
            let selected = match &picked.device {
                Some(id) => devices.iter().position(|device| &device.id == id),
                // No saved card means the first one listed, which is what the claim takes.
                None => (!devices.is_empty()).then_some(0),
            };
            picked.devices = devices;
            selected
        };
        let selected = selected.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let g = ui.global::<Settings>();
            g.set_output_device_names(ModelRc::from(Rc::new(VecModel::from(names))));
            g.set_output_device_idx(selected);
        });
    });
}

/// The chip's index, in the order of the inline list in `output-section.slint`.
fn mode_index(mode: OutputModeKey) -> i32 {
    match mode {
        OutputModeKey::Shared => 0,
        OutputModeKey::Exclusive => 1,
    }
}

fn mode_from_index(idx: i32) -> OutputModeKey {
    match idx {
        1 => OutputModeKey::Exclusive,
        _ => OutputModeKey::Shared,
    }
}
