//! The flag struct behind Settings ▸ Accessibility ▸ Undo.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use melodia_core::entities::undo::{RemovalSize, UndoKind};

/// How long the undo notice waits with nobody reading it, until Settings picks otherwise.
pub const DEFAULT_UNDO_NOTICE_MS: u32 = 6_000;

/// What the duration slider offers. A hand-edited value outside it is held to it on read.
pub const UNDO_NOTICE_MS_RANGE: RangeInclusive<u32> = 4_000..=10_000;

/// Undo ships on though new behaviour defaults off: a removal it covers doesn't ask first, so off by
/// default would leave those removals with neither.
///
/// The kinds are kept as the keys of those turned off, so a kind added later starts on and a key
/// no build knows is ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UndoFlags {
    pub undo_enabled: bool,
    pub undo_single_items: bool,
    pub undo_kinds_off: Vec<String>,
    pub undo_notice_ms: u32,
}

impl Default for UndoFlags {
    fn default() -> Self {
        Self {
            undo_enabled: true,
            undo_single_items: true,
            undo_kinds_off: Vec::new(),
            undo_notice_ms: DEFAULT_UNDO_NOTICE_MS,
        }
    }
}

impl UndoFlags {
    /// Whether a removal of `kind` taking `size` is offered an undo. One that isn't holds nothing
    /// and raises no notice, and asks first where the removal has a confirmation.
    pub fn covers(&self, kind: UndoKind, size: RemovalSize) -> bool {
        let quieted_as_single = !self.undo_single_items && size == RemovalSize::Items(1);
        self.undo_enabled && self.kind_on(kind) && !quieted_as_single
    }

    pub fn kind_on(&self, kind: UndoKind) -> bool {
        !self.undo_kinds_off.iter().any(|key| key == kind.key())
    }

    pub fn set_kind(&mut self, kind: UndoKind, on: bool) {
        self.undo_kinds_off.retain(|key| key != kind.key());
        if !on {
            self.undo_kinds_off.push(kind.key().to_owned());
        }
    }

    pub fn notice_ms(&self) -> u32 {
        self.undo_notice_ms.clamp(*UNDO_NOTICE_MS_RANGE.start(), *UNDO_NOTICE_MS_RANGE.end())
    }
}
