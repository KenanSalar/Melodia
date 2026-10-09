//! Library API — direct, in-process replacement for the Tauri `commands/` layer.
//!
//! Each submodule mirrors a former `#[tauri::command]` group. Functions are plain
//! `pub async fn` (or `pub fn`) returning `Result<T, AppError>`. One reading only `paths` or only
//! `db` takes that field, `&Paths` or `&DbPool`, and the rest take `&AppState`; `playback` takes
//! `PlaybackContext`. The signature then says what a call can touch, and a test drives it off a
//! `test_pool` or a seeded root, an `AppState` being unbuildable below `headless.rs`.
//! `melodia-views` passes `&state.db` without naming its type, so the store stays out of its
//! reach. A `SharedFlag` reader keeps the state as well: the flags share one type, so a
//! signature taking the flag would accept the wrong one.
//!
//! A door that reads several fields hands its decision to a private body taking only what that
//! reaches, and a body taking a seam its door fills in, a clock or a desktop probe, stays private
//! beside it for the same reason.
//!
//! State propagation to the UI happens via the watch channels on `AppState::sinks`
//! (driven by `with_state_emit` in `player::engine::state`) — never `app.emit(...)`.

pub mod albums;
pub mod artists;
pub mod browse;
pub mod clipboard;
pub mod entity_tracks;
pub mod favorites;
pub mod genres;
pub mod import;
pub mod lyrics;
pub mod playback;
pub mod playlist_files;
pub mod playlists;
pub mod queue;
pub mod radio;
pub mod radio_files;
pub mod ratings;
pub mod recently_played;
pub mod scan;
pub mod search;
pub mod settings;
pub mod smart_playlists;
pub mod tags;
pub mod tracks;
pub mod window;
