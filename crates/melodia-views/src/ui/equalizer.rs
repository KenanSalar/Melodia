//! The graphic equalizer's wiring: the Equalizer card on Settings ▸ Effects and the overflow
//! menu's dialog both drive the one `Equalizer` global. Each callback applies to the engine
//! synchronously and persists on the blocking pool; a band drag updates the model in place and
//! persists only on release (`commit-band`), the `set-volume` / `commit-volume` split.

use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::ui::settings_bind::{read_or_default, toggle_binding};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_playback::player::playback::equalizer;
use melodia_ui::{AppWindow, Equalizer};

pub fn install_equalizer(ui: &AppWindow, state: &AppState) {
    let model = seed(ui, state);

    ui.global::<Equalizer>().on_set_enabled(toggle_binding(
        state,
        "persist eq_enabled",
        library::playback::player_set_eq_enabled,
        library::settings::set_eq_enabled,
    ));
    wire_set_band(ui, state, &model);
    wire_commit_band(ui, state, &model);
    wire_select_preset(ui, state, &model);
    wire_reset(ui, state, &model);
    wire_set_preamp(ui, state);
    wire_commit_preamp(ui, state);
}

/// The synthetic "Custom" entry's dropdown index, one past the last built-in preset.
fn custom_preset_index() -> i32 {
    i32::try_from(equalizer::PRESET_COUNT).unwrap_or(0)
}

/// Write the persisted state onto the global, handing back the `band-gains` model the callbacks
/// share with the card and the dialog.
fn seed(ui: &AppWindow, state: &AppState) -> Rc<VecModel<f32>> {
    let flags = read_or_default(state, "equalizer").equalizer;
    let gains = equalizer::normalize_gains(&flags.eq_band_gains);
    let preset_idx = equalizer::preset_index(&flags.eq_selected_preset)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or_else(custom_preset_index);

    let model: Rc<VecModel<f32>> = Rc::new(VecModel::from(gains.to_vec()));

    let eq = ui.global::<Equalizer>();
    eq.set_enabled(flags.eq_enabled);
    eq.set_band_gains(ModelRc::from(model.clone()));
    eq.set_preset_idx(preset_idx);
    eq.set_preamp(equalizer::clamp_preamp(flags.eq_preamp));

    // The sliders' ranges are the DSP's constants, seeded before either surface can mount.
    eq.set_min_gain(equalizer::MIN_GAIN_DB);
    eq.set_max_gain(equalizer::MAX_GAIN_DB);
    eq.set_min_preamp(equalizer::MIN_PREAMP_DB);
    eq.set_max_preamp(equalizer::MAX_PREAMP_DB);
    model
}

/// A live band drag: the model and the player follow and the dropdown flips to "Custom", the
/// release persisting.
fn wire_set_band(ui: &AppWindow, state: &AppState, model: &Rc<VecModel<f32>>) {
    let state = state.clone();
    let model = model.clone();
    let weak = ui.as_weak();
    ui.global::<Equalizer>().on_set_band(move |idx, db| {
        let Ok(i) = usize::try_from(idx) else { return };
        let db = equalizer::clamp_gain(db);
        model.set_row_data(i, db);
        library::playback::player_set_eq_band(&state.playback_ctx(), i, db);
        if let Some(ui) = weak.upgrade() {
            ui.global::<Equalizer>().set_preset_idx(custom_preset_index());
        }
    });
}

/// Drag release: persist the current curve as a Custom preset.
fn wire_commit_band(ui: &AppWindow, state: &AppState, model: &Rc<VecModel<f32>>) {
    let state = state.clone();
    let model = model.clone();
    ui.global::<Equalizer>().on_commit_band(move |idx, db| {
        if let Ok(i) = usize::try_from(idx) {
            model.set_row_data(i, equalizer::clamp_gain(db));
        }
        persist_curve(&state, model.iter().collect(), equalizer::CUSTOM_PRESET);
    });
}

/// Apply a built-in preset's gains. The "Custom" entry (and
/// any out-of-range index) is a no-op: the gains stay as the user left them.
fn wire_select_preset(ui: &AppWindow, state: &AppState, model: &Rc<VecModel<f32>>) {
    let state = state.clone();
    let model = model.clone();
    ui.global::<Equalizer>().on_select_preset(move |idx| {
        let Ok(i) = usize::try_from(idx) else { return };
        let Some(preset) = equalizer::PRESETS.get(i) else {
            return;
        };
        model.set_vec(preset.gains.to_vec());
        library::playback::player_set_eq_gains(&state.playback_ctx(), &preset.gains);
        persist_curve(&state, preset.gains.to_vec(), preset.name);
    });
}

/// Full return to neutral: flat curve, "Flat" preset, 0 dB preamp.
fn wire_reset(ui: &AppWindow, state: &AppState, model: &Rc<VecModel<f32>>) {
    let state = state.clone();
    let model = model.clone();
    let weak = ui.as_weak();
    ui.global::<Equalizer>().on_reset(move || {
        let flat = [0.0_f32; equalizer::NUM_BANDS];
        model.set_vec(flat.to_vec());
        let ctx = state.playback_ctx();
        library::playback::player_set_eq_gains(&ctx, &flat);
        library::playback::player_set_eq_preamp(&ctx, 0.0);
        if let Some(ui) = weak.upgrade() {
            let eq = ui.global::<Equalizer>();
            eq.set_preset_idx(0);
            eq.set_preamp(0.0);
        }
        persist_curve(&state, flat.to_vec(), equalizer::DEFAULT_PRESET);
        state.persist_blocking("persist eq_preamp", |paths| {
            library::settings::set_eq_preamp(paths, 0.0)
        });
    });
}

/// A live preamp drag, the readout following and the release persisting.
fn wire_set_preamp(ui: &AppWindow, state: &AppState) {
    let state = state.clone();
    let weak = ui.as_weak();
    ui.global::<Equalizer>().on_set_preamp(move |db| {
        let db = equalizer::clamp_preamp(db);
        library::playback::player_set_eq_preamp(&state.playback_ctx(), db);
        if let Some(ui) = weak.upgrade() {
            ui.global::<Equalizer>().set_preamp(db);
        }
    });
}

/// Drag release: persist the preamp.
fn wire_commit_preamp(ui: &AppWindow, state: &AppState) {
    let state = state.clone();
    ui.global::<Equalizer>().on_commit_preamp(move |db| {
        let db = equalizer::clamp_preamp(db);
        state.persist_blocking("persist eq_preamp", move |paths| {
            library::settings::set_eq_preamp(paths, db)
        });
    });
}

/// Persist a curve with the preset it now answers to.
fn persist_curve(state: &AppState, gains: Vec<f32>, preset: &str) {
    let preset = preset.to_owned();
    state.persist_blocking("persist eq band gains + preset", move |paths| {
        library::settings::set_eq_band_gains_and_preset(paths, &gains, preset)
    });
}
