//! The card's measure switch and the loudness pass's bar under it.
//!
//! The switch moved either way clears a Stop, which otherwise holds for the session, so off reads
//! as idle and on measures again.

use async_compat::Compat;
use slint::ComponentHandle;

use crate::ui::settings_bind::toggle_binding;
use crate::ui::util::count_as_i32;
use melodia_app::library;
use melodia_app::state::{AppState, MeasureProgress};
use melodia_ui::{AppWindow, MeasurePhase, ReplayGain};

pub(super) fn install(ui: &AppWindow, state: &AppState) -> Result<(), slint::EventLoopError> {
    let rg = ui.global::<ReplayGain>();

    {
        let mut set_measured = toggle_binding(
            state,
            "persist rg_measure_untagged",
            library::playback::player_set_replaygain_measured,
            library::settings::set_replaygain_measure_untagged,
        );
        let loudness = state.loudness.clone();
        rg.on_set_measure_untagged(move |on| {
            set_measured(on);
            loudness.resume();
        });
    }

    {
        let loudness = state.loudness.clone();
        rg.on_stop_measuring(move || loudness.stop());
    }

    {
        let loudness = state.loudness.clone();
        rg.on_resume_measuring(move || loudness.resume());
    }

    spawn_progress_painter(ui, state)
}

fn spawn_progress_painter(ui: &AppWindow, state: &AppState) -> Result<(), slint::EventLoopError> {
    let mut rx = state.loudness.subscribe_progress();
    let weak = ui.as_weak();
    slint::spawn_local(Compat::new(async move {
        let mut progress = *rx.borrow_and_update();
        loop {
            match weak.upgrade() {
                Some(ui) => paint_progress(&ui.global::<ReplayGain>(), progress),
                None => break,
            }
            if rx.changed().await.is_err() {
                break;
            }
            progress = *rx.borrow();
        }
        log::debug!("ui::replaygain::measure progress subscriber stopped");
    }))?;
    Ok(())
}

fn paint_progress(rg: &ReplayGain<'_>, progress: MeasureProgress) {
    rg.set_measure_phase(ui_phase(progress.phase));
    rg.set_measured_count(count_as_i32(progress.done));
    rg.set_measure_total(count_as_i32(progress.total));
}

fn ui_phase(phase: melodia_app::state::MeasurePhase) -> MeasurePhase {
    use melodia_app::state::MeasurePhase as Backend;
    match phase {
        Backend::Idle => MeasurePhase::Idle,
        Backend::Running => MeasurePhase::Running,
        Backend::WaitingForScan => MeasurePhase::WaitingForScan,
        Backend::Stopped => MeasurePhase::Stopped,
    }
}
