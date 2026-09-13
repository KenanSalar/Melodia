//! What the OS panel was last told, and what a view model would change about it.
//!
//! Both backends answer the same question before they spend a platform call, so the answer lives
//! here once rather than drifting apart per platform.

use melodia_engine::player::engine::now_playing::SourceSummary;
use melodia_engine::player::engine::state::PlayerViewModelLight;
use melodia_engine::player::engine::types::{PlaybackStatus, RepeatMode};

/// The metadata last handed to the panel.
///
/// **The dedupe key is the metadata, not a proxy for it.** Keyed on the track id, a live title
/// arriving on the station already playing looked unchanged and never reached the panel — and so
/// did a track re-tagged in place. The identity is deliberately *not* part of the comparison:
/// two sources that would publish the same panel do not need a second platform round trip.
///
/// Owned, since it outlives the borrow it is built from, and compared *against* the borrowed form
/// so the steady state allocates nothing.
pub(super) struct PublishedMetadata {
    pub(super) title: String,
    pub(super) secondary: Option<String>,
    pub(super) album: Option<String>,
    pub(super) artwork_path: Option<String>,
    pub(super) duration_ms: Option<u64>,
}

impl PublishedMetadata {
    /// Whether the panel already says what `source` would say. Nothing held and nothing playing
    /// is a match: there is no metadata to clear.
    pub(super) fn still_current(held: Option<&Self>, source: Option<&SourceSummary<'_>>) -> bool {
        match (held, source) {
            (None, None) => true,
            (Some(held), Some(source)) => {
                held.title == source.title
                    && held.secondary.as_deref() == source.secondary
                    && held.album.as_deref() == source.album
                    && held.artwork_path.as_deref() == source.artwork_path
                    && held.duration_ms == source.duration_ms
            }
            _ => false,
        }
    }
}

impl From<&SourceSummary<'_>> for PublishedMetadata {
    fn from(source: &SourceSummary<'_>) -> Self {
        Self {
            title: source.title.to_owned(),
            secondary: source.secondary.map(str::to_owned),
            album: source.album.map(str::to_owned),
            artwork_path: source.artwork_path.map(str::to_owned),
            duration_ms: source.duration_ms,
        }
    }
}

/// Which parts of the panel a view model moves.
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per independent part of the panel, and each backend reads a different subset"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Changes {
    pub(super) metadata: bool,
    pub(super) status: bool,
    pub(super) position: bool,
    pub(super) volume: bool,
    pub(super) shuffle: bool,
    pub(super) repeat: bool,
}

/// The panel as last published.
#[derive(Default)]
pub(super) struct Published {
    pub(super) metadata: Option<PublishedMetadata>,
    pub(super) status: Option<PlaybackStatus>,
    pub(super) position_ms: u64,
    pub(super) volume: u32,
    pub(super) is_muted: bool,
    pub(super) shuffle_enabled: bool,
    pub(super) repeat_mode: Option<RepeatMode>,
}

impl Published {
    pub(super) fn changes(&self, vm: &PlayerViewModelLight, status: PlaybackStatus) -> Changes {
        Changes {
            metadata: !PublishedMetadata::still_current(
                self.metadata.as_ref(),
                vm.source().as_ref(),
            ),
            status: self.status != Some(status),
            position: self.position_ms != vm.position_ms,
            volume: self.volume != vm.volume || self.is_muted != vm.is_muted,
            shuffle: self.shuffle_enabled != vm.shuffle_enabled,
            repeat: self.repeat_mode != Some(vm.repeat_mode),
        }
    }

    pub(super) fn record(
        &mut self,
        vm: &PlayerViewModelLight,
        status: PlaybackStatus,
        changes: Changes,
    ) {
        // Only on a move: unchanged, the held value already describes the source, and a volume
        // drag reaches here per pointer move.
        if changes.metadata {
            self.metadata = vm.source().as_ref().map(PublishedMetadata::from);
        }
        self.status = Some(status);
        self.position_ms = vm.position_ms;
        self.volume = vm.volume;
        self.is_muted = vm.is_muted;
        self.shuffle_enabled = vm.shuffle_enabled;
        self.repeat_mode = Some(vm.repeat_mode);
    }
}

#[cfg(test)]
#[path = "tests/published_tests.rs"]
mod tests;
