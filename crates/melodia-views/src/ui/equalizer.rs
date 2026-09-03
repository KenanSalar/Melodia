//! Wire the graphic-equalizer dialog (`Dialog.kind == "equalizer"`) to Rust.
//!
//! Seeds the `Equalizer` global from `settings.json` at startup (enabled flag,
//! the band-gains model, and the selected-preset dropdown index) and registers
//! the five callbacks. Each follows the established two-phase shape (see
//! [`crate::ui::settings::playback_settings`]): apply to the live playback engine
//! synchronously, then persist on the blocking pool. Live band drags update the
//! Slint model in place (so the slider tracks the cursor) and persist only on
//! release via `commit-band`, mirroring the `set-volume` / `commit-volume`
//! split.
//!
//! EQ state lives on the playback engine's lock-free shared cell, not the
//! `PlayerState` machine, so the runtime apply goes through the infallible
//! `library::playback::player_set_eq_*` helpers.

use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::ui::settings_bind::{read_or_default, toggle_binding};
use crate::{AppWindow, Equalizer};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_playback::player::playback::equalizer;

pub fn install_equalizer(ui: &AppWindow, state: &AppState) {
    // Dropdown index of the synthetic "Custom" entry (one past the last
    // built-in preset). `PRESET_COUNT` is small and constant, so the
    // conversion never truncates.
    let custom_idx = i32::try_from(equalizer::PRESET_COUNT).unwrap_or(0);

    // A missing / unreadable file falls back to the inert defaults (off, flat,
    // "Flat").
    let flags = read_or_default(state, "equalizer").equalizer;
    let enabled = flags.eq_enabled;
    let gains = equalizer::normalize_gains(&flags.eq_band_gains);
    let preset_idx = equalizer::preset_index(&flags.eq_selected_preset)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(custom_idx);
    let preamp = equalizer::clamp_preamp(flags.eq_preamp);

    // Backing model for the `band-gains` `[float]` global property. Kept here
    // (cloned into the callbacks) so preset / reset / drag updates mutate the
    // same model the dialog reads.
    let model: Rc<VecModel<f32>> = Rc::new(VecModel::from(gains.to_vec()));

    let eq = ui.global::<Equalizer>();
    eq.set_enabled(enabled);
    eq.set_band_gains(ModelRc::from(model.clone()));
    eq.set_preset_idx(preset_idx);
    eq.set_preamp(preamp);

    // Seed the dB ranges from the DSP constants so Rust stays the single source
    // of truth (the band/preamp sliders read these). Runs at boot, before the
    // dialog can open.
    eq.set_min_gain(equalizer::MIN_GAIN_DB);
    eq.set_max_gain(equalizer::MAX_GAIN_DB);
    eq.set_min_preamp(equalizer::MIN_PREAMP_DB);
    eq.set_max_preamp(equalizer::MAX_PREAMP_DB);

    // set-enabled — live toggle + persist.
    eq.on_set_enabled(toggle_binding(
        state,
        "persist eq_enabled",
        library::playback::player_set_eq_enabled,
        library::settings::set_eq_enabled,
    ));

    // set-band — live band change during a drag: update the model (so the
    // slider tracks), apply to the player, and flip the dropdown to "Custom".
    // No disk write (commit-band persists on release).
    {
        let state = state.clone();
        let model = model.clone();
        let weak = ui.as_weak();
        eq.on_set_band(move |idx, db| {
            let Ok(i) = usize::try_from(idx) else { return };
            let db = equalizer::clamp_gain(db);
            model.set_row_data(i, db);
            library::playback::player_set_eq_band(&state.playback_ctx(), i, db);
            if let Some(ui) = weak.upgrade() {
                ui.global::<Equalizer>().set_preset_idx(custom_idx);
            }
        });
    }

    // commit-band — drag release: persist the current curve as a Custom preset.
    {
        let state = state.clone();
        let model = model.clone();
        eq.on_commit_band(move |idx, db| {
            if let Ok(i) = usize::try_from(idx) {
                model.set_row_data(i, equalizer::clamp_gain(db));
            }
            let gains: Vec<f32> = model.iter().collect();
            state.persist_blocking("persist eq band gains + preset", move |s| {
                library::settings::set_eq_band_gains_and_preset(
                    s,
                    &gains,
                    equalizer::CUSTOM_PRESET.to_owned(),
                )
            });
        });
    }

    // select-preset — apply a built-in preset's gains. The "Custom" entry (and
    // any out-of-range index) is a no-op: the gains stay as the user left them.
    {
        let state = state.clone();
        let model = model.clone();
        eq.on_select_preset(move |idx| {
            let Ok(i) = usize::try_from(idx) else { return };
            let Some(preset) = equalizer::PRESETS.get(i) else {
                return;
            };
            let gains = preset.gains;
            let name = preset.name.to_owned();
            model.set_vec(gains.to_vec());
            library::playback::player_set_eq_gains(&state.playback_ctx(), &gains);
            state.persist_blocking("persist eq band gains + preset", move |s| {
                library::settings::set_eq_band_gains_and_preset(s, &gains, name)
            });
        });
    }

    // reset — full return to neutral: flat curve, "Flat" preset, 0 dB preamp.
    {
        let state = state.clone();
        let model = model.clone();
        let weak = ui.as_weak();
        eq.on_reset(move || {
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
            let s = state.clone();
            state.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_eq_band_gains_and_preset(
                    &s,
                    &flat,
                    equalizer::DEFAULT_PRESET.to_owned(),
                ) {
                    log::warn!("persist eq band gains + preset: {e}");
                }
                if let Err(e) = library::settings::set_eq_preamp(&s, 0.0) {
                    log::warn!("persist eq_preamp: {e}");
                }
            });
        });
    }

    // set-preamp — live preamp change during a drag: apply to the player and
    // update the property so the slider's dB readout tracks. No disk write.
    {
        let state = state.clone();
        let weak = ui.as_weak();
        eq.on_set_preamp(move |db| {
            let db = equalizer::clamp_preamp(db);
            library::playback::player_set_eq_preamp(&state.playback_ctx(), db);
            if let Some(ui) = weak.upgrade() {
                ui.global::<Equalizer>().set_preamp(db);
            }
        });
    }

    // commit-preamp — drag release: persist the preamp.
    {
        let state = state.clone();
        eq.on_commit_preamp(move |db| {
            let db = equalizer::clamp_preamp(db);
            state.persist_blocking("persist eq_preamp", move |s| {
                library::settings::set_eq_preamp(s, db)
            });
        });
    }
}
