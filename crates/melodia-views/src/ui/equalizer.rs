//! The graphic equalizer's wiring: the Equalizer card on Settings ▸ Effects and the overflow
//! menu's dialog both drive the one `Equalizer` global. Each callback applies to the engine
//! synchronously and persists on the blocking pool; a band drag updates the model in place and
//! persists only on release (`commit-band`), the `set-volume` / `commit-volume` split. Reset raises
//! the undo notice, which puts the curve back through the same writer a preset pick uses.

use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::ui::settings_bind::{read_or_default, toggle_binding};
use crate::ui::undo::{Removal, UndoWindow};
use melodia_app::library;
use melodia_app::library::settings::EqCurve;
use melodia_app::state::AppState;
use melodia_playback::player::playback::equalizer;
use melodia_ui::{AppWindow, Equalizer};

pub fn install_equalizer(ui: &AppWindow, state: &AppState, undo: &Rc<UndoWindow>) {
    let model = seed(ui, state);
    let curves =
        Rc::new(CurveWriter { state: state.clone(), model: model.clone(), ui: ui.as_weak() });

    ui.global::<Equalizer>().on_set_enabled(toggle_binding(
        state,
        "persist eq_enabled",
        library::playback::player_set_eq_enabled,
        library::settings::set_eq_enabled,
    ));
    wire_set_band(ui, state, &model);
    wire_commit_band(ui, state, &model);
    wire_select_preset(ui, &curves);
    wire_reset(ui, &curves, undo);
    wire_set_preamp(ui, state);
    wire_commit_preamp(ui, state);
}

/// What Reset took, and the writer that puts it back.
pub struct ResetUndo {
    before: EqCurve,
    curves: Rc<CurveWriter>,
}

impl ResetUndo {
    pub fn restore(self) {
        self.curves.apply(self.before);
    }
}

/// Every write of a whole curve: the band model, the engine, the global and `settings.json`
/// together, so the card, the dialog and what plays never disagree.
struct CurveWriter {
    state: AppState,
    model: Rc<VecModel<f32>>,
    ui: slint::Weak<AppWindow>,
}

impl CurveWriter {
    fn apply(&self, curve: EqCurve) {
        self.model.set_vec(curve.gains.to_vec());
        let ctx = self.state.playback_ctx();
        library::playback::player_set_eq_gains(&ctx, &curve.gains);
        library::playback::player_set_eq_preamp(&ctx, curve.preamp_db);
        if let Some(ui) = self.ui.upgrade() {
            let eq = ui.global::<Equalizer>();
            eq.set_preset_idx(preset_slot(curve.preset));
            eq.set_preamp(curve.preamp_db);
        }
        self.state.persist_blocking("persist eq curve", move |paths| {
            library::settings::set_eq_curve(paths, &curve)
        });
    }

    /// A built-in preset's gains, with the preamp as it stands.
    fn apply_preset(&self, preset: &equalizer::EqPreset) {
        let Some(ui) = self.ui.upgrade() else { return };
        let preamp_db = ui.global::<Equalizer>().get_preamp();
        self.apply(EqCurve { gains: preset.gains, preset: preset.name, preamp_db });
    }

    /// `None` once the window is gone.
    fn current(&self) -> Option<EqCurve> {
        let ui = self.ui.upgrade()?;
        let eq = ui.global::<Equalizer>();
        let gains: Vec<f32> = self.model.iter().collect();
        Some(EqCurve {
            gains: equalizer::normalize_gains(&gains),
            preset: preset_name(eq.get_preset_idx()),
            preamp_db: eq.get_preamp(),
        })
    }
}

/// The synthetic "Custom" entry's dropdown index, one past the last built-in preset.
fn custom_preset_index() -> i32 {
    i32::try_from(equalizer::PRESET_COUNT).unwrap_or(0)
}

/// The dropdown index a persisted name selects, "Custom" for one that isn't built in.
fn preset_slot(name: &str) -> i32 {
    equalizer::preset_index(name)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or_else(custom_preset_index)
}

/// The name a dropdown index persists as.
fn preset_name(slot: i32) -> &'static str {
    usize::try_from(slot)
        .ok()
        .and_then(|i| equalizer::PRESETS.get(i))
        .map_or(equalizer::CUSTOM_PRESET, |preset| preset.name)
}

/// Write the persisted state onto the global, handing back the `band-gains` model the callbacks
/// share with the card and the dialog.
fn seed(ui: &AppWindow, state: &AppState) -> Rc<VecModel<f32>> {
    let flags = read_or_default(state, "equalizer").equalizer;
    let gains = equalizer::normalize_gains(&flags.eq_band_gains);

    let model: Rc<VecModel<f32>> = Rc::new(VecModel::from(gains.to_vec()));

    let eq = ui.global::<Equalizer>();
    eq.set_enabled(flags.eq_enabled);
    eq.set_band_gains(ModelRc::from(model.clone()));
    eq.set_preset_idx(preset_slot(&flags.eq_selected_preset));
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
fn wire_select_preset(ui: &AppWindow, curves: &Rc<CurveWriter>) {
    let curves = curves.clone();
    ui.global::<Equalizer>().on_select_preset(move |idx| {
        let Ok(i) = usize::try_from(idx) else { return };
        let Some(preset) = equalizer::PRESETS.get(i) else {
            return;
        };
        curves.apply_preset(preset);
    });
}

/// Full return to neutral: flat curve, "Flat" preset, 0 dB preamp. A curve that was already flat
/// had nothing to take, so it raises no notice.
fn wire_reset(ui: &AppWindow, curves: &Rc<CurveWriter>, undo: &Rc<UndoWindow>) {
    let curves = curves.clone();
    let undo = undo.clone();
    ui.global::<Equalizer>().on_reset(move || {
        let Some(before) = curves.current() else { return };
        curves.apply(EqCurve::flat());
        if !before.is_flat() {
            undo.offer(Removal::Equalizer(ResetUndo { before, curves: curves.clone() }));
        }
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

/// Persist a dragged curve with the preset it now answers to.
fn persist_curve(state: &AppState, gains: Vec<f32>, preset: &str) {
    let preset = preset.to_owned();
    state.persist_blocking("persist eq band gains + preset", move |paths| {
        library::settings::set_eq_band_gains_and_preset(paths, &gains, preset)
    });
}
