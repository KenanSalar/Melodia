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
//!
//! **Spelled a second time in Slint**, as `Player.source-{title,subtitle,tertiary}`, because only
//! `@tr` at a literal reaches the catalogues and a translated fallback is one this side cannot
//! hand over. The song-then-station order is the same and the empty arms deliberately are not: a
//! label always paints something, where an absent field is what an OS surface wants rather than a
//! placeholder nothing asked for.

use super::state::PlayerViewModelLight;
use super::types::RadioNowPlaying;

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
/// Shaped after the fields the OS media controls, the tray tooltip and a Discord activity all want,
/// since those three agree on the questions and only disagree on which to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSummary<'a> {
    pub id: SourceId<'a>,
    /// The song: a track's title, or what the stream announced. A station that has announced
    /// nothing yet lends its own name here, so this is never empty for a station.
    pub title: &'a str,
    /// Who by: a track's artist, or the station's name once [`Self::title`] is the song. `None`
    /// where there is nothing to add — an untagged track, or a station still lending its name.
    pub secondary: Option<&'a str>,
    /// A track's album. A station lends its name here once [`Self::secondary`] is an artist, so an
    /// OS panel with three slots still says what is tuned.
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
            let station = radio.name.as_str();
            let announced = radio.live_title.as_deref().and_then(non_empty);
            let (title, secondary, album) = match (radio.announcement(), announced) {
                (Some(song), _) => (song.title, Some(song.artist), Some(station)),
                (None, Some(line)) => (line, Some(station), None),
                (None, None) => (station, None, None),
            };
            return Some(SourceSummary {
                id: SourceId::Station(radio.stream_url.as_str()),
                title,
                secondary,
                album,
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

/// A stream's announcement read as a song by someone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Announcement<'a> {
    pub artist: &'a str,
    pub title: &'a str,
}

impl RadioNowPlaying {
    /// What the stream announced, as artist and title, or `None` where it named no artist.
    ///
    /// **Split on the first `" - "`**, because `Artist - Title` is the convention stations follow
    /// and a title carries a second dash far more often than an artist does (`Song - Remastered`).
    /// It is a convention and not a format, so a line that doesn't split is a jingle, an ad or a
    /// show name, and every consumer falls back to showing it whole.
    ///
    /// **Two halves with no letter between them are not a song.** Some stations briefly announce
    /// their automation's catalogue ids (`403761 - 287105`) before the real line. One numeric half
    /// is kept, since band names and titles like `311` or `1979` exist.
    pub fn announcement(&self) -> Option<Announcement<'_>> {
        let (artist, title) = self.live_title.as_deref()?.split_once(" - ")?;
        let song = Announcement { artist: non_empty(artist)?, title: non_empty(title)? };
        let has_letter = |text: &str| text.chars().any(char::is_alphabetic);
        (has_letter(song.artist) || has_letter(song.title)).then_some(song)
    }
}

/// A field with something in it, trimmed. The ICY reader already trims what it stores, so this is
/// what keeps the ladder honest for the fields nobody else guards — a track's blank artist column
/// reaching MPRIS as an empty string rather than as an absent one.
fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
#[path = "tests/now_playing_tests.rs"]
mod tests;
