//! Scrobble detection for a station, beside [`super::detector`] rather than inside it because the
//! two answer "was this heard" from different evidence. A track has a length and a position to
//! measure a play against; a stream has neither, only the moments its announcement changes.
//!
//! **A song counts only when it was heard from one announcement to the next**, uninterrupted and
//! for longer than [`MIN_TRACK_MS`]. The song playing when a station is tuned, or when a paused one
//! reconnects, started before we were listening, and nothing on the wire says how much of it was
//! missed, so it gets a now-playing and never a scrobble. Pause closes the stream, so any stretch
//! out of `playing` is the same interruption. A line naming no artist is a jingle or an ad as far as
//! both services are concerned, and ends the song before it without starting one.
//!
//! Pure like its sibling: the view model, the clock and the setting come in as values.

use crate::services::integrations::scrobble::model::{MIN_TRACK_MS, ScrobbleTrack};
use melodia_engine::player::engine::state::PlayerViewModelLight;

/// What the task must act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEffect {
    NowPlaying(ScrobbleTrack),
    /// A song heard in full, stamped with when its announcement arrived.
    Scrobble {
        track: ScrobbleTrack,
        timestamp: i64,
    },
}

/// The announcement last seen, which is what tells the next one from a republish.
#[derive(Debug)]
struct Heard {
    stream_url: String,
    line: Option<String>,
    /// Whether the station has sent a line since tuning in. Kept apart from `line`, because
    /// plenty of stations blank their title between songs, and the song after the blank still
    /// started while we were listening.
    announced: bool,
}

/// A song whose start we witnessed, still playing.
#[derive(Debug)]
struct OnAir {
    track: ScrobbleTrack,
    started_at: i64,
}

#[derive(Debug, Default)]
pub struct LiveDetector {
    last: Option<Heard>,
    on_air: Option<OnAir>,
    /// The last song that may not start again: one already scrobbled, or one joined part way. A
    /// station that blanks or ads over a song and then restores its line would otherwise count it.
    spent: Option<ScrobbleTrack>,
}

impl LiveDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// React to a published view model. `enabled` is the user's setting, read per call so turning
    /// it off mid-song drops that song rather than scrobbling it once back on.
    pub fn on_view_model(
        &mut self,
        vm: Option<&PlayerViewModelLight>,
        now_ts: i64,
        enabled: bool,
    ) -> Vec<LiveEffect> {
        let radio =
            vm.filter(|vm| enabled && vm.status == "playing").and_then(|vm| vm.radio.as_deref());
        let Some(radio) = radio else {
            self.interrupt();
            return Vec::new();
        };

        let line = radio.live_title.as_deref();
        let same_station = self.last.as_ref().filter(|last| last.stream_url == radio.stream_url);
        if same_station.is_some_and(|last| last.line.as_deref() == line) {
            return Vec::new();
        }

        // A turnover only once this same station has announced something: the first line after
        // tuning in names a song that was already under way. A song another station's line
        // displaced was cut off rather than finished, so it is dropped.
        let turnover = same_station.is_some_and(|last| last.announced);
        let announced = radio.announcement().map(ScrobbleTrack::heard_on_air);
        let mut effects = Vec::new();
        if turnover {
            effects.extend(self.finish(now_ts));
        } else {
            self.on_air = None;
            self.spent.clone_from(&announced);
        }
        self.last = Some(Heard {
            stream_url: radio.stream_url.clone(),
            line: line.map(str::to_owned),
            announced: turnover || line.is_some(),
        });

        if let Some(track) = announced {
            effects.push(LiveEffect::NowPlaying(track.clone()));
            if turnover && self.spent.as_ref() != Some(&track) {
                self.on_air = Some(OnAir { track, started_at: now_ts });
            }
        }
        effects
    }

    /// The song on air, as a scrobble if it ran long enough, clearing it either way.
    fn finish(&mut self, now_ts: i64) -> Option<LiveEffect> {
        let song = self.on_air.take()?;
        let heard_secs = u64::try_from(now_ts.saturating_sub(song.started_at)).unwrap_or(0);
        let scrobbled = heard_secs.saturating_mul(1000) > MIN_TRACK_MS;
        self.spent = scrobbled.then(|| song.track.clone());
        scrobbled.then_some(LiveEffect::Scrobble { track: song.track, timestamp: song.started_at })
    }

    fn interrupt(&mut self) {
        self.last = None;
        self.on_air = None;
        self.spent = None;
    }
}
