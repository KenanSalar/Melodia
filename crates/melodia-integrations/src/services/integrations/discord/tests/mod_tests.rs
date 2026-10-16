//! The presence service's worker slot: when a thread runs behind it, and when none may.

use std::sync::{Arc, OnceLock};

use super::DiscordPresenceService;
use super::model::Presence;
use melodia_core::entities::integrations::DiscordFlags;

/// A card with nothing on it but a title, which the IPC suite hands its worker too.
pub(super) fn card() -> Presence {
    Presence {
        details: "Song".to_owned(),
        state: None,
        large_text: None,
        large_image: None,
        paused: false,
        start_ts: None,
        end_ts: None,
    }
}

/// Something the detector, the boot or a toggle does to the service.
type Call = fn(&DiscordPresenceService);

/// With the feature off nothing the detector or the boot sends may start a worker, which would
/// park a thread for the session; the check under the slot's lock is the only gate left.
#[test]
fn nothing_done_while_off_spawns_a_worker() {
    let calls: [(&str, Call); 4] = [
        ("apply", |service| service.apply(card())),
        ("clear", DiscordPresenceService::clear),
        ("start_if_enabled", DiscordPresenceService::start_if_enabled),
        ("set_flags off", |service| service.set_flags(DiscordFlags::default())),
    ];
    for (what, call) in calls {
        let service =
            DiscordPresenceService::init(&DiscordFlags::default(), Arc::new(OnceLock::new()));

        call(&service);

        let slot = service.worker.lock();
        let empty = (slot.commands.is_none(), slot.thread.is_none());
        assert_eq!(empty, (true, true), "{what}: (no channel, no thread)");
    }
}
