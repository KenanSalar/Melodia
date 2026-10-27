//! Getting a sheet for whatever is playing, and handing it back.
//!
//! **Three edges reach [`reseed`], and dropping any one shows an empty panel**: a track change, the
//! view re-opening (the sheet goes back on every close) and the header's switch. It is idempotent
//! by [`super::LyricsUi::holds`], so they may overlap: whichever gets there first pays.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Weak};

use super::rows::rows_for;
use super::{LyricsUi, NowPlayingState, follow};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::lyrics::{Lyrics as Sheet, LyricsOutcome, LyricsSource};
use melodia_core::entities::track::TrackSummary;
use melodia_core::utils::toast::{self, ToastKind};
use melodia_ui::{AppWindow, Lyrics, LyricsState};

/// A failure is logged and reads as nothing found: an old-codepage sidecar and a directory that's
/// down are both, to a reader, a panel with no words, and neither is worth a toast.
async fn fetch(state: &AppState, track: &TrackSummary) -> LyricsOutcome {
    match library::lyrics::for_track(state, track).await {
        Ok(outcome) => outcome,
        Err(e) => {
            log::debug!(
                "ui::now_playing lyrics for {}: {}",
                track.id,
                melodia_core::error::describe(&e)
            );
            LyricsOutcome::Absent
        }
    }
}

/// Brings the panel in line with what is playing, claiming the track before the first `.await`.
fn reseed(weak: &Weak<AppWindow>, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let Some(ui) = weak.upgrade() else { return };
    let ly = &np_state.lyrics;

    let track = playing_track(np_state);
    // Both terms: the switch is the setting, still on behind a closed view whose source-change
    // path runs on, and a request per track there would fill a panel nobody can see.
    let wanted = np_state.renders_panel() && library::lyrics::is_enabled(state);
    let Some(track) = track.filter(|_| wanted) else {
        // Closed, switched off, or a station, which has no words to look up. Either way the
        // previous song's sheet must not sit under it.
        if ly.holding.borrow().is_some() {
            release(&ui, ly);
        }
        return;
    };
    if ly.holds(&track.file_path) {
        return;
    }

    mark_loading(&ui, ly, &track.file_path);
    let weak = weak.clone();
    let state = state.clone();
    let np_state = np_state.clone();
    let res = slint::spawn_local(Compat::new(async move {
        let outcome = fetch(&state, &track).await;
        let Some(ui) = weak.upgrade() else { return };
        // The song may have moved under the lookup; the claim says so.
        if !np_state.lyrics.holds(&track.file_path) {
            return;
        }
        apply(&ui, &np_state.lyrics, &track.file_path, &outcome, state.lyrics_online_enabled.get());
    }));
    if let Err(e) = res {
        log::warn!(
            "ui::now_playing lyrics reseed task spawn_local: {}",
            melodia_core::error::describe(&e)
        );
    }
}

/// **Both actions read what is playing at click time**: the menu outlives any one track, so a
/// handle taken at wire time would name the wrong song by the second verse.
pub(crate) fn wire_menu(ui: &AppWindow, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let global = ui.global::<Lyrics>();

    {
        let weak = ui.as_weak();
        let state = state.clone();
        let np_state = np_state.clone();
        global.on_refresh(move || {
            let Some(track) = playing_track(&np_state) else {
                return;
            };
            let Some(ui) = weak.upgrade() else { return };

            // Given up before the lookup, not after: the claim is what stops the reseed below
            // deduping this against the sheet it is meant to replace.
            release(&ui, &np_state.lyrics);

            let weak = weak.clone();
            let state = state.clone();
            let np_state = np_state.clone();
            let res = slint::spawn_local(Compat::new(async move {
                if let Err(e) = library::lyrics::forget(&state, &track.file_path).await {
                    log::warn!("lyrics refresh: {}", melodia_core::error::describe(&e));
                }
                if weak.upgrade().is_some() {
                    np_state.lyrics.kick();
                }
            }));
            if let Err(e) = res {
                log::warn!(
                    "ui::now_playing lyrics refresh spawn_local: {}",
                    melodia_core::error::describe(&e)
                );
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        let np_state = np_state.clone();
        global.on_save_to_tag(move || {
            let Some(track) = playing_track(&np_state) else {
                return;
            };
            let state = state.clone();
            let weak = weak.clone();
            let np_state = np_state.clone();
            let res = slint::spawn_local(Compat::new(async move {
                let found = library::lyrics::resident_text(&state, &track.file_path).await;
                let text = match found {
                    Ok(Some(text)) => text,
                    // The row is only offered against a sheet on screen, so nothing here is the
                    // store having lost the copy it was resolved from.
                    Ok(None) => {
                        log::debug!("lyrics save: nothing resident for {}", track.id);
                        return;
                    }
                    Err(e) => return report_save_failure(&e),
                };
                if let Err(e) = library::lyrics::write_to_tag(&state, track.id, &text).await {
                    return report_save_failure(&e);
                }
                toast::notify(ToastKind::LyricsSaved, track.title.clone());
                // The tag now holds the sheet, which is the file's own from here.
                let Some(ui) = weak.upgrade() else { return };
                ui.global::<Lyrics>().set_can_save_to_tag(false);
                np_state.lyrics.holding.borrow_mut().take();
                np_state.lyrics.kick();
            }));
            if let Err(e) = res {
                log::warn!(
                    "ui::now_playing lyrics save spawn_local: {}",
                    melodia_core::error::describe(&e)
                );
            }
        });
    }
}

/// A toast rather than a log line, the radio vote's rule: a control that says it writes to the
/// user's file has to say why it didn't.
fn report_save_failure(e: &melodia_core::error::AppError) {
    let reason = melodia_core::error::describe(e);
    log::warn!("lyrics save: {reason}");
    toast::notify(ToastKind::OperationFailed, reason);
}

/// The track on the deck, or `None` on a station or an empty queue.
fn playing_track(np_state: &NowPlayingState) -> Option<Arc<TrackSummary>> {
    np_state.current_source.borrow().as_ref().and_then(|s| s.track.clone())
}

pub(crate) fn wire_reseed(ui: &AppWindow, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let weak_ui = ui.as_weak();
    let state = state.clone();
    let weak_np = Rc::downgrade(np_state);
    *np_state.lyrics.reseed.borrow_mut() = Some(Box::new(move || {
        let Some(np_state) = weak_np.upgrade() else {
            return;
        };
        reseed(&weak_ui, &state, &np_state);
    }));
}

/// `online_enabled` decides only which sentence an empty panel shows.
fn apply(
    ui: &AppWindow,
    ly: &Rc<LyricsUi>,
    track_path: &str,
    outcome: &LyricsOutcome,
    online_enabled: bool,
) {
    ly.pinned.set(None);
    let global = ui.global::<Lyrics>();

    // Whatever the outcome, so a re-open on an answered track pays no second lookup: an outcome
    // with no rows is an answer, not an absence.
    *ly.holding.borrow_mut() = Some(track_path.to_owned());

    match outcome {
        LyricsOutcome::Sheet(sheet) => {
            take_sheet(ly, sheet);
            follow::republish(ui, ly);
            global.set_synced(sheet.is_synced());
            // Every source but the file's own tag is worth offering to write into it. A sidecar
            // counts: it is the user's file, but it is not the one that travels with the track.
            global.set_can_save_to_tag(sheet.source != LyricsSource::Tag);
            global.set_state(LyricsState::Ready);
        }
        LyricsOutcome::Instrumental => {
            clear(ui, ly);
            global.set_state(LyricsState::Instrumental);
        }
        LyricsOutcome::Absent => {
            clear(ui, ly);
            // The one state that names a setting, being the only one a reader can act on here.
            global.set_state(if online_enabled { LyricsState::Missing } else { LyricsState::Off });
        }
        // **The claim above is still recorded, deliberately.** Nothing retries on its own, so
        // without it every tick that reaches `reseed` would start another lookup against a service
        // that has just told us to stop. Refresh is the retry, and it drops the claim itself.
        LyricsOutcome::Unavailable => {
            clear(ui, ly);
            global.set_state(LyricsState::Unavailable);
        }
    }
}

/// **Claims the track before the `.await`**: the lookup can reach a file and then a socket, and a
/// reseed landing in that window would start a second one for the same song.
fn mark_loading(ui: &AppWindow, ly: &Rc<LyricsUi>, track_path: &str) {
    clear(ui, ly);
    *ly.holding.borrow_mut() = Some(track_path.to_owned());
    ui.global::<Lyrics>().set_state(LyricsState::Loading);
}

/// Hands back the rows and the table with the claim, so the next open looks the sheet up again
/// rather than believing it is still on screen.
pub(crate) fn release(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    ly.holding.borrow_mut().take();
    clear(ui, ly);
    ui.global::<Lyrics>().set_state(LyricsState::Idle);
}

fn clear(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    ly.rows.borrow_mut().clear();
    ly.offsets.borrow_mut().clear();
    ly.pinned.set(None);
    ly.model.set_vec(Vec::new());

    let global = ui.global::<Lyrics>();
    global.set_synced(false);
    global.set_can_save_to_tag(false);
    global.set_active_index(-1);
    global.set_active_offset(0.0);
    global.set_active_height(0.0);
}

/// Keep the sheet's text and stamps. Heights are not kept, following the width instead.
fn take_sheet(ly: &Rc<LyricsUi>, sheet: &Sheet) {
    *ly.rows.borrow_mut() = rows_for(sheet);
}
