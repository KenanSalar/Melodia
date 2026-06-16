//! Wire the graphic-equalizer dialog (`Dialog.kind == "equalizer"`) to Rust.
//!
//! Seeds the `Equalizer` global from `settings.json` at startup (enabled flag,
//! the band-gains model, and the selected-preset dropdown index) and registers
//! the five callbacks. Each follows the established two-phase shape (see
//! [`crate::ui::playback_settings`]): apply to the live Rodio player
//! synchronously, then persist on the blocking pool. Live band drags update the
//! Slint model in place (so the slider tracks the cursor) and persist only on
//! release via `commit-band`, mirroring the `set-volume` / `commit-volume`
//! split.
//!
//! EQ state lives on the Rodio backend's lock-free shared cell, not the
//! `PlayerState` machine, so the runtime apply goes through the infallible
//! `library::playback::player_set_eq_*` helpers.

use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::library;
use crate::player::equalizer;
use crate::services::settings;
use crate::state::AppState;
use crate::{AppWindow, Equalizer};

pub fn install_equalizer(ui: &AppWindow, state: &AppState) {
    // Dropdown index of the synthetic "Custom" entry (one past the last
    // built-in preset). `PRESET_COUNT` is small and constant, so the
    // conversion never truncates.
    let custom_idx = i32::try_from(equalizer::PRESET_COUNT).unwrap_or(0);

    // Read persisted EQ config; a missing / unreadable file falls back to the
    // inert defaults (off, flat, "Flat").
    let (enabled, gains, preset_idx) = match settings::read_settings(&state.paths) {
        Ok(s) => {
            let gains = equalizer::normalize_gains(&s.equalizer.eq_band_gains);
            let idx = equalizer::preset_index(&s.equalizer.eq_selected_preset)
                .and_then(|i| i32::try_from(i).ok())
                .unwrap_or(custom_idx);
            (s.equalizer.eq_enabled, gains, idx)
        }
        Err(e) => {
            log::warn!("read settings for equalizer: {e}");
            (false, equalizer::normalize_gains(&[]), 0)
        }
    };

    // Backing model for the `band-gains` `[float]` global property. Kept here
    // (cloned into the callbacks) so preset / reset / drag updates mutate the
    // same model the dialog reads.
    let model: Rc<VecModel<f32>> = Rc::new(VecModel::from(gains.to_vec()));

    let eq = ui.global::<Equalizer>();
    eq.set_enabled(enabled);
    eq.set_band_gains(ModelRc::from(model.clone()));
    eq.set_preset_idx(preset_idx);

    // set-enabled — live toggle + persist.
    {
        let state = state.clone();
        eq.on_set_enabled(move |on| {
            library::playback::player_set_eq_enabled(&state.playback_ctx(), on);
            let s = state.clone();
            state.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_eq_enabled(&s, on) {
                    log::warn!("persist eq_enabled: {e}");
                }
            });
        });
    }

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
            let s = state.clone();
            state.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_eq_band_gains(&s, &gains) {
                    log::warn!("persist eq_band_gains: {e}");
                }
                if let Err(e) = library::settings::set_eq_selected_preset(&s, "Custom".to_owned()) {
                    log::warn!("persist eq_selected_preset: {e}");
                }
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
            let Some(preset) = equalizer::PRESETS.get(i) else { return };
            let gains = preset.gains;
            let name = preset.name.to_owned();
            model.set_vec(gains.to_vec());
            library::playback::player_set_eq_gains(&state.playback_ctx(), &gains);
            let s = state.clone();
            state.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_eq_band_gains(&s, &gains) {
                    log::warn!("persist eq_band_gains: {e}");
                }
                if let Err(e) = library::settings::set_eq_selected_preset(&s, name) {
                    log::warn!("persist eq_selected_preset: {e}");
                }
            });
        });
    }

    // reset — flat curve, back to the "Flat" preset.
    {
        let state = state.clone();
        let model = model.clone();
        let weak = ui.as_weak();
        eq.on_reset(move || {
            let flat = [0.0_f32; equalizer::NUM_BANDS];
            model.set_vec(flat.to_vec());
            library::playback::player_set_eq_gains(&state.playback_ctx(), &flat);
            if let Some(ui) = weak.upgrade() {
                ui.global::<Equalizer>().set_preset_idx(0);
            }
            let s = state.clone();
            state.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_eq_band_gains(&s, &flat) {
                    log::warn!("persist eq_band_gains: {e}");
                }
                if let Err(e) =
                    library::settings::set_eq_selected_preset(&s, equalizer::DEFAULT_PRESET.to_owned())
                {
                    log::warn!("persist eq_selected_preset: {e}");
                }
            });
        });
    }
}
