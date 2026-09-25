//! The Output card's signal path: what the playing track goes through to reach the device, and
//! the button that turns off whatever the user chose that changes it.
//!
//! The grading is `signal_path::evaluate`'s and arrives finished on `AppState::signal_path_tx`.
//! This module only turns it into the `SignalPathUi` global's numbers and strings.

use async_compat::Compat;
use slint::ComponentHandle;

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_engine::player::engine::signal_path::{Grade, SignalPath, Verdict};
use melodia_ui::{AppWindow, Equalizer, ReplayGain, Settings, SignalPathUi};

pub fn install(ui: &AppWindow, state: &AppState) {
    let mut rx = state.signal_path_tx.subscribe();
    paint(ui, rx.borrow_and_update().as_ref());
    let weak = ui.as_weak();
    let _ = slint::spawn_local(Compat::new(async move {
        while rx.changed().await.is_ok() {
            let Some(ui) = weak.upgrade() else { break };
            paint(&ui, rx.borrow().as_ref());
        }
    }));

    let state = state.clone();
    let weak = ui.as_weak();
    ui.global::<SignalPathUi>().on_make_bit_perfect(move || {
        let ctx = state.playback_ctx();
        state.runtime.spawn(async move { library::playback::player_make_bit_perfect(&ctx) });
        state.persist_blocking(
            "persist bit-perfect reset",
            library::settings::reset_for_bit_perfect,
        );
        // Volume and speed come back through the player's view model; these three are seeded
        // from Rust and repaint only when told.
        if let Some(ui) = weak.upgrade() {
            ui.global::<Equalizer>().set_enabled(false);
            ui.global::<ReplayGain>().set_enabled(false);
            if library::playback::FOLLOW_RATE_SUPPORTED {
                ui.global::<Settings>().set_output_follow_rate(true);
            }
        }
    });
}

fn paint(ui: &AppWindow, path: Option<&SignalPath>) {
    let g = ui.global::<SignalPathUi>();
    g.set_playing(path.is_some());
    let Some(path) = path else { return };

    g.set_verdict(match path.verdict {
        Verdict::BitPerfect => 0,
        Verdict::Enhanced => 1,
        Verdict::Converted => 2,
    });

    let stages = path.stages;
    g.set_source_grade(grade_index(stages.source));
    g.set_dsp_grade(grade_index(stages.dsp));
    g.set_speed_grade(grade_index(stages.speed));
    g.set_volume_grade(grade_index(stages.volume));
    g.set_crossfade_grade(grade_index(stages.crossfade));
    g.set_rate_grade(grade_index(stages.rate));
    g.set_channels_grade(grade_index(stages.channels));
    g.set_output_grade(grade_index(stages.output));

    let inputs = &path.inputs;
    let source = inputs.source.shape;
    let device = &inputs.negotiated;
    let source_rate = rate_text(source.rate.get());
    let device_rate = rate_text(device.shape.rate.get());
    let bits = inputs.source.format.bits;
    let depth = if inputs.source.format.float {
        format!("{bits}-bit float")
    } else {
        format!("{bits}-bit")
    };
    g.set_source_format(format!("{source_rate} · {depth} · {} ch", source.channels).into());
    g.set_device_format(
        format!("{device_rate} · {} · {} ch", device.format, device.shape.channels).into(),
    );
    g.set_source_rate(source_rate.into());
    g.set_device_rate(device_rate.into());
    g.set_device_name(device.device_name.clone().unwrap_or_default().into());

    g.set_eq_on(inputs.eq_on);
    g.set_rg_on(inputs.rg_on);
    let transport = inputs.transport;
    g.set_muted(transport.muted);
    g.set_volume(i32::try_from(transport.volume).unwrap_or(i32::MAX));
    g.set_speed(format!("{}×", transport.speed).into());
}

/// The global's encoding of a grade, which the `.slint` side documents beside the properties.
fn grade_index(grade: Grade) -> i32 {
    match grade {
        Grade::Clean => 0,
        Grade::Enhanced => 1,
        Grade::Converted => 2,
    }
}

/// `44.1 kHz`, `48 kHz`: `f64`'s shortest round-trip form drops a trailing `.0` on its own.
fn rate_text(hz: u32) -> String {
    format!("{} kHz", f64::from(hz) / 1000.0)
}
