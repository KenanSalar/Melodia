//! The Output card's signal path: what the playing track goes through to reach the device, and
//! the button that turns off whatever the user chose that changes it.
//!
//! The grading is `signal_path::evaluate`'s and arrives finished on `AppState::signal_path_tx`.
//! This module only turns it into the `SignalPathUi` global's numbers and strings.

use async_compat::Compat;
use slint::{ComponentHandle, SharedString};

use crate::ui::util;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_engine::player::engine::signal_path::{Grade, SignalPath, Verdict};
use melodia_playback::player::playback::output::OutputFormat;
use melodia_playback::player::playback::output::claim::FallbackReason;
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
        Verdict::Fallback => 3,
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
    g.set_exclusive(matches!(device.format, OutputFormat::Exclusive(_)));
    let fallback = device.fallback.as_ref();
    g.set_fallback_reason(fallback.map_or(-1, |fallback| fallback_index(&fallback.reason)));
    g.set_fallback_by(match fallback.map(|fallback| &fallback.reason) {
        Some(FallbackReason::Reserved { by }) => by.into(),
        _ => SharedString::new(),
    });
    g.set_refused_by(fallback.and_then(|fallback| fallback.device.as_deref()).unwrap_or("").into());
    let source_rate = rate_text(source.rate.get());
    let device_rate = rate_text(device.shape.rate.get());
    g.set_source_format(
        format!("{source_rate} · {} · {} ch", inputs.source.format, source.channels).into(),
    );
    g.set_device_format(
        format!("{device_rate} · {} · {} ch", device.format, device.shape.channels).into(),
    );
    g.set_source_rate(source_rate.into());
    g.set_device_rate(device_rate.into());
    g.set_device_name(device.device_name.clone().unwrap_or_default().into());
    g.set_device_period(
        device
            .period
            .map(|frames| period_text(frames, device.shape.rate.get()))
            .unwrap_or_default()
            .into(),
    );

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

/// The index of the reason's words in `output-section.slint`'s inline list, which follows this
/// order.
fn fallback_index(reason: &FallbackReason) -> i32 {
    match reason {
        FallbackReason::Busy => 0,
        FallbackReason::Reserved { .. } => 1,
        FallbackReason::RateRefused => 2,
        FallbackReason::ChannelsRefused => 3,
        FallbackReason::FormatRefused => 4,
        FallbackReason::NotConnected => 5,
        FallbackReason::Unsupported => 6,
        FallbackReason::Io => 7,
        FallbackReason::NotAllowed => 8,
    }
}

fn rate_text(hz: u32) -> String {
    util::format_sample_rate(i32::try_from(hz).unwrap_or(i32::MAX))
}

/// `20 ms`, `21.3 ms`: what a period of `frames` lasts at `hz`, to the tenth a driver's rounding
/// shows up in.
fn period_text(frames: u32, hz: u32) -> String {
    let ms = f64::from(frames) * 1000.0 / f64::from(hz.max(1));
    format!("{} ms", (ms * 10.0).round() / 10.0)
}
