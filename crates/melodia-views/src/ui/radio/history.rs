//! The titles a station announces while it plays.
//!
//! ICY metadata lands on the feed thread, is reconciled into `PlayerState.radio.live_title` by the
//! playback monitor, and reaches here on the same view-model watch the Now Playing bar reads — so
//! a history costs one subscriber and no change to the player at all.
//!
//! **Kept per station and dropped when a *different* one starts, not when playback stops.** A
//! station you paused or stopped is still the one on screen, and a list that empties the moment
//! you press stop is a list nobody gets to read.
//!
//! Radio owns it because the Radio page draws it, and the Now Playing view reads the *same*
//! model rather than a second one: `Radio.history-rows` is by definition the playing station's
//! titles, and the station page gates them on `Radio.detail-station-is-playing`. So the rows are
//! written here, once per move, and a seat change only re-answers the flag.

use std::collections::VecDeque;
use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::AppWindow;
use melodia_engine::player::engine::event_sink::PlayerSinks;

use super::{RadioUi, detail};

/// How many titles one station keeps.
///
/// A couple of hours of listening at three or four minutes a song, which is the span the list is
/// for: naming something you heard a while back and did not write down. Bounded because a stream
/// can be left running for days.
const HISTORY_CAP: usize = 50;

/// One station's titles, newest first.
#[derive(Default)]
pub struct StationHistory {
    /// Whose titles these are, keyed by stream URL — the one field every station has. The
    /// database id is `0` for a browsed station and the uuid empty for a hand-typed one.
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
        // Stations re-send the current title on a timer, and plenty send it verbatim with every
        // metadata block.
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

/// Subscribe to the player's view model and keep the ring current.
///
/// Its own subscriber rather than a hook inside `ui::shell::bridge`'s, so the push-side glue stays
/// ignorant of radio; a `watch` is built for several observers and the queue and position
/// channels already have one each.
pub fn install(
    weak: slint::Weak<AppWindow>,
    radio_ui: &Arc<RadioUi>,
    sinks: &Arc<PlayerSinks>,
) -> Result<(), slint::EventLoopError> {
    let mut rx = sinks.view_model.subscribe();
    let radio_ui = radio_ui.clone();

    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            // Folded inside the borrow, so a tick that carries no station and a title that has
            // not moved both cost nothing — this fires on every state change, not just a title.
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

/// Publish the ring into `Radio.history-rows` and re-answer whose station it describes.
///
/// The borrow is scoped so the flag's own read of the ring is a second acquisition rather than a
/// re-entrant one — `parking_lot::Mutex` would deadlock on the latter.
pub fn apply(ui: &AppWindow, radio_ui: &RadioUi) {
    let titles: Vec<slint::SharedString> = {
        let history = radio_ui.history.lock();
        history.titles().iter().map(|title| slint::SharedString::from(title.as_str())).collect()
    };
    ui.global::<crate::Radio>()
        .set_history_rows(slint::ModelRc::new(slint::VecModel::from(titles)));
    detail::sync_history_seat(ui, radio_ui);
}

#[cfg(test)]
#[path = "tests/history_tests.rs"]
mod tests;
