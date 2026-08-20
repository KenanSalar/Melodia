//! Library API — direct, in-process replacement for the Tauri `commands/` layer.
//!
//! Each submodule mirrors a former `#[tauri::command]` group. Functions are plain
//! `pub async fn` (or `pub fn`) that take `&AppState` (or specific `Arc<T>`s when
//! `&AppState` would over-couple) and return `Result<T, AppError>`.
//!
//! State propagation to the UI happens via the watch channels on `AppState::sinks`
//! (driven by `with_state_emit` in `player::state`) — never `app.emit(...)`.

pub mod albums;
pub mod artists;
pub mod browse;
pub mod favorites;
pub mod genres;
pub mod import;
pub mod mbid;
pub mod playback;
pub mod playlist_files;
pub mod playlists;
pub mod queue;
pub mod radio;
pub mod radio_files;
pub mod ratings;
pub mod recently_played;
pub mod search;
pub mod settings;
pub mod smart_playlists;
pub mod tags;
pub mod tracks;
pub mod window;
