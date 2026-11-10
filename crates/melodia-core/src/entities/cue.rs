//! Where a cue sheet puts one of its tracks: the sheet's number for it, and the stretch of the
//! file's audio it plays.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};

/// A sheet's `TRACK` number, 0 being the audio hidden ahead of track 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CueTrack(u8);

impl CueTrack {
    const LAST: u8 = 99;
}

impl TryFrom<i64> for CueTrack {
    type Error = CueError;

    fn try_from(number: i64) -> Result<Self, CueError> {
        match u8::try_from(number) {
            Ok(track) if track <= Self::LAST => Ok(Self(track)),
            _ => Err(CueError::Track(number)),
        }
    }
}

impl From<CueTrack> for i64 {
    fn from(track: CueTrack) -> Self {
        track.0.into()
    }
}

impl FromSql for CueTrack {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let number = i64::column_result(value)?;
        Self::try_from(number).map_err(|_| FromSqlError::OutOfRange(number))
    }
}

/// A time from the start of the file's audio in cue frames, 75 to the second, the unit a sheet's
/// `mm:ss:ff` counts in. Kept as the sheet said it, since the sample rate that converts it exactly
/// is the decoder's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CueFrames(u32);

impl TryFrom<i64> for CueFrames {
    type Error = CueError;

    fn try_from(frames: i64) -> Result<Self, CueError> {
        u32::try_from(frames).map(Self).map_err(|_| CueError::Frames(frames))
    }
}

impl From<CueFrames> for i64 {
    fn from(frames: CueFrames) -> Self {
        frames.0.into()
    }
}

impl FromSql for CueFrames {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let frames = i64::column_result(value)?;
        Self::try_from(frames).map_err(|_| FromSqlError::OutOfRange(frames))
    }
}

/// The stretch of a file one sheet track plays, from `start` to `end`, or on to the end of the
/// file where `end` is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CueSpan {
    start: CueFrames,
    end: Option<CueFrames>,
}

impl CueSpan {
    /// # Errors
    ///
    /// [`CueError::Span`] where `end` doesn't fall after `start`.
    pub fn new(start: CueFrames, end: Option<CueFrames>) -> Result<Self, CueError> {
        match end {
            Some(end) if end <= start => Err(CueError::Span { start: start.0, end: end.0 }),
            _ => Ok(Self { start, end }),
        }
    }

    pub fn start(self) -> CueFrames {
        self.start
    }

    pub fn end(self) -> Option<CueFrames> {
        self.end
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CueError {
    #[error("cue track {0} is outside 0 to {last}", last = CueTrack::LAST)]
    Track(i64),

    #[error("cue time {0} is not a frame count")]
    Frames(i64),

    #[error("cue span ends at frame {end}, not after its start at frame {start}")]
    Span { start: u32, end: u32 },
}
