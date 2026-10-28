//! The titles a station announces while it plays, off the view-model watch the Now Playing bar
//! reads, so a history costs one subscriber and no change to the player.
//!
//! **Kept per station and dropped when a different one starts**, not on stop: a stopped station is
//! still the one on screen. `Radio.history-rows` is the playing station's titles for the station
//! page and Now Playing both, so it is written here and a seat change only re-answers the flag.

use std::collections::VecDeque;
use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::now_playing::Announcement;
use melodia_ui::{AppWindow, StationHistoryRow};

use super::{RadioUi, detail};

/// How many titles one station keeps: a couple of hours of listening, bounded because a stream can
/// run for days.
const HISTORY_CAP: usize = 50;

/// One station's titles, newest first.
#[derive(Default)]
pub struct StationHistory {
    /// Whose titles these are, by stream URL, the one field every station has.
    stream_url: String,
    titles: VecDeque<String>,
}

impl StationHistory {
    /// Fold one view-model tick in, answering whether anything moved.
    fn note(&mut self, stream_url: &str, title: Option<&str>) -> bool {
        let mut moved = false;
        if self.stream_url != stream_url {
            stream_url.clone_into(&mut self.stream_url);
            moved = !self.titles.is_empty();
            self.titles.clear();
        }

        let Some(title) = title.map(str::trim).filter(|title| !title.is_empty()) else {
            return moved;
        };
        // Stations re-send the current title on a timer, often with every metadata block.
        if self.titles.front().is_some_and(|newest| newest == title) {
            return moved;
        }
        self.titles.push_front(title.to_owned());
        self.titles.truncate(HISTORY_CAP);
        true
    }

    /// Everything held, whichever station it belongs to.
    pub fn titles(&self) -> &VecDeque<String> {
        &self.titles
    }

    /// Whether the ring is holding `stream_url`'s titles.
    pub fn describes(&self, stream_url: &str) -> bool {
        self.stream_url == stream_url
    }
}

/// Subscribes to the player's view model and keeps the ring current, as its own subscriber so
/// `ui::shell::bridge` stays ignorant of radio.
pub fn install(
    weak: slint::Weak<AppWindow>,
    radio_ui: &Arc<RadioUi>,
    sinks: &PlayerSinks,
) -> Result<(), slint::EventLoopError> {
    let mut rx = sinks.view_model.subscribe();
    let radio_ui = radio_ui.clone();

    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            // Folded inside the borrow: this fires on every state change, not just a title.
            let moved = {
                let vm = rx.borrow_and_update();
                vm.as_ref().and_then(|vm| vm.radio.as_ref()).is_some_and(|radio| {
                    radio_ui.history.lock().note(&radio.stream_url, radio.live_title.as_deref())
                })
            };
            if !moved {
                continue;
            }
            let Some(ui) = weak.upgrade() else { break };
            apply(&ui, &radio_ui);
        }
        log::debug!("ui::radio::history subscriber stopped");
    }))?;
    Ok(())
}

/// Publishes the ring into `Radio.history-rows` and re-answers whose station it describes. The
/// borrow is scoped because the flag reads the ring too, and `parking_lot::Mutex` isn't re-entrant.
pub fn apply(ui: &AppWindow, radio_ui: &RadioUi) {
    let rows: Vec<StationHistoryRow> = {
        let history = radio_ui.history.lock();
        history.titles().iter().map(|title| history_row(title)).collect()
    };
    ui.global::<melodia_ui::Radio>()
        .set_history_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    detail::sync_history_seat(ui, radio_ui);
}

/// One title, split here rather than in Slint so a row's Copy Title and Copy Artist name what the
/// player bar showed while it played.
fn history_row(title: &str) -> StationHistoryRow {
    let song = Announcement::parse(title);
    StationHistoryRow {
        line: title.into(),
        artist: song.map_or_else(Default::default, |song| song.artist.into()),
        title: song.map_or_else(Default::default, |song| song.title.into()),
    }
}

#[cfg(test)]
#[path = "tests/history_tests.rs"]
mod tests;
