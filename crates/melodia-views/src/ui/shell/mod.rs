//! The surfaces round every view rather than one of them, each installed once at boot: the
//! `Player` global's subscribers, the toast stack, the tray and the miniplayer swap.

pub mod bridge;
pub mod event_sink;
pub mod mini_player;
pub mod notifications;
pub mod tray_bridge;
