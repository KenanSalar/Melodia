//! The app shell — the surfaces that surround every view rather than being one.
//!
//! * [`bridge`] — the three `watch` subscribers that drive the `Player` global,
//!   plus the view-model → Slint row conversions.
//! * [`notifications`] — the toast stack.
//! * [`tray_bridge`] — tray actions in, tray state out, and the close-to-tray
//!   window visibility shadow.
//! * [`mini_player`] — the miniplayer swap and its square artwork tier.
//! * [`event_sink`] — OS media-control events onto the runtime. Imports no
//!   Slint at all, which is what lets `tasks/` reach it.
//!
//! What distinguishes these from the shared component library still at the
//! `ui/` root is that none of them is reusable machinery a view reaches for —
//! each *is* a piece of the window, installed once at boot.

pub mod bridge;
pub mod event_sink;
pub mod mini_player;
pub mod notifications;
pub mod tray_bridge;
