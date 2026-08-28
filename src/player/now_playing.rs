//! What is playing, with the track-or-station split already made.
//!
//! Every surface that states the current source — the OS media panel, the tray, the Discord card,
//! and the Slint bridge — used to ask `current_track` and get `None` for the whole life of a
//! station. Each would need its own `vm.radio` arm, which is four copies of one ladder: the song
//! the stream announced, falling back to the station's name until it announces one.
//!
//! Borrowed throughout. [`PlayerViewModelLight::source`] runs on every state emit, so it may not
//! allocate; a consumer that needs to keep an answer past the borrow owns the few fields it
//! compares rather than the whole summary.

use super::state::PlayerViewModelLight;

/// Which source a summary describes, and what tells one instance of it from the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceId<'a> {
    Track(i64),
    /// Keyed on the stream URL rather than `station_id`, which is `0` for every station the user
    /// has only browsed to — so the id collides across all of them.
    Station(&'a str),
}

/// What a Now-Playing surface states about whatever is on the deck.
///
/// Shaped after the fields souvlaki, the tray tooltip and a Discord activity all want, since those
/// three agree on the questions and only disagree on which to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSummary<'a> {
    pub id: SourceId<'a>,
    /// The song: a track's title, or what the stream announced. A station that has announced
    /// nothing yet lends its own name here, so this is never empty for a station.
    pub title: &'a str,
    /// Who by: a track's artist, or the station's name once [`Self::title`] is the song. `None`
    /// where there is nothing to add — an untagged track, or a station still lending its name.
    pub secondary: Option<&'a str>,
    /// A station has no album, and never will.
    pub album: Option<&'a str>,
    pub artwork_path: Option<&'a str>,
    /// `None` for a live source, so a consumer publishes an *absent* length rather than a zero
    /// one. MPRIS renders the two differently.
    pub duration_ms: Option<u64>,
}

impl PlayerViewModelLight {
    /// The source on the deck, or `None` when there is none.
    ///
    /// The two halves are mutually exclusive by construction: `begin_track` clears `radio` and
    /// `build_station_connecting_actions` clears `current_track`. Radio is asked first anyway, so
    /// a future that leaves both set can only read as the more specific of the two.
    pub fn source(&self) -> Option<SourceSummary<'_>> {
        if let Some(radio) = self.radio.as_ref() {
            let announced = radio.live_title.as_deref().and_then(non_empty);
            return Some(SourceSummary {
                id: SourceId::Station(radio.stream_url.as_str()),
                title: announced.unwrap_or(radio.name.as_str()),
                secondary: announced.map(|_| radio.name.as_str()),
                album: None,
                artwork_path: radio.artwork_path.as_deref().and_then(non_empty),
                duration_ms: None,
            });
        }

        let track = self.current_track.as_deref()?;
        Some(SourceSummary {
            id: SourceId::Track(track.id),
            title: track.title.as_str(),
            secondary: track.artist.as_deref().and_then(non_empty),
            album: track.album.as_deref().and_then(non_empty),
            artwork_path: track.artwork_path.as_deref().and_then(non_empty),
            duration_ms: Some(self.duration_ms),
        })
    }
}

/// A field with something in it, trimmed. The ICY reader already trims what it stores, so this is
/// what keeps the ladder honest for the fields nobody else guards — a track's blank artist column
/// reaching MPRIS as an empty string rather than as an absent one.
fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}
