//! What is playing, with the track-or-station split made once for the OS media panel, the tray,
//! the Discord card and the Slint bridge: the song a stream announced, falling back to the
//! station's name. Borrowed, since [`PlayerViewModelLight::source`] runs on every emit.
//!
//! Spelled again in Slint as `Player.source-{title,subtitle,tertiary}`, only `@tr` at a literal
//! reaching the catalogues. Same order, different empty arms: a label always paints something,
//! where an OS surface wants an absent field.

use super::state::PlayerViewModelLight;
use super::types::RadioNowPlaying;

/// Which source a summary describes, and what tells one instance of it from the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceId<'a> {
    Track(i64),
    /// The stream URL, `station_id` being `0` for every station only browsed to.
    Station(&'a str),
}

/// What a Now Playing surface states about the deck, shaped after what the OS media controls, the
/// tray tooltip and a Discord activity all ask.
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
    /// `None` for a live source: MPRIS renders an absent length and a zero one differently.
    pub duration_ms: Option<u64>,
}

impl PlayerViewModelLight {
    /// Radio is asked first, so a view model ever carrying both reads as the more specific.
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

impl<'a> Announcement<'a> {
    /// Splits on the first `" - "`, a title carrying a second dash more often than an artist does.
    /// `None` for a line that doesn't split (a jingle, an ad, a show) or whose halves both lack a
    /// letter (an automation's catalogue ids; one numeric half is a band like `311`).
    pub fn parse(line: &'a str) -> Option<Self> {
        let (artist, title) = line.split_once(" - ")?;
        let song = Self { artist: non_empty(artist)?, title: non_empty(title)? };
        let has_letter = |text: &str| text.chars().any(char::is_alphabetic);
        (has_letter(song.artist) || has_letter(song.title)).then_some(song)
    }
}

impl RadioNowPlaying {
    /// What the stream announced, as artist and title, read by [`Announcement::parse`]: the
    /// station's history splits its past lines through the same call.
    pub fn announcement(&self) -> Option<Announcement<'_>> {
        Announcement::parse(self.live_title.as_deref()?)
    }
}

/// Trimmed and non-empty, so a blank artist column reaches MPRIS as absent rather than empty.
fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
#[path = "tests/now_playing_tests.rs"]
mod tests;
