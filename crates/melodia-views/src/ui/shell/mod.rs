//! The app shell — the surfaces that surround every view rather than being one:
//! the three `watch` subscribers behind the `Player` global ([`bridge`]), the
//! toast stack, the tray's two directions plus its close-to-tray visibility
//! shadow, and the miniplayer swap.
//!
//! What keeps them out of the shared component library at the `ui/` root is that none is
//! machinery a view reaches for — each *is* a piece of the window, installed once at boot.
//! [`event_sink`] imports no Slint at all, which is what lets `tasks/` reach it.

pub mod bridge;
pub mod event_sink;
pub mod mini_player;
pub mod notifications;
pub mod tray_bridge;
