//! What a pass writes, each job in one transaction, so a stop or a failure leaves no album half
//! recorded.

use melodia_audio::loudness::{AlbumOutcome, Gain, TrackOutcome};
use rusqlite::Transaction;

use super::{AlbumJob, MeasureTrack, untagged};
use crate::database::queries::track::json_array;
use crate::database::{DbPool, sql};
use melodia_core::entities::cue::CueSpan;
use melodia_core::error::AppError;

/// Records a measurement on its track, unless the row's hash or span moved, or it gained a tag,
/// since the job was read: the audio heard is then no longer the audio there. A re-measured row
/// drops the album values it held, its album being measured again around it.
const RECORD_TRACK: &str = concat!(
    "INSERT INTO track_loudness (track_id, file_hash, cue_start, cue_end, track_gain, track_peak)
     SELECT t.id, t.file_hash, t.cue_start, t.cue_end, ?5, ?6 FROM tracks t
     WHERE t.id = ?1 AND t.file_hash = ?2 AND t.cue_start IS ?3 AND t.cue_end IS ?4 AND ",
    untagged!(t),
    "
     ON CONFLICT (track_id) DO UPDATE SET
         file_hash = excluded.file_hash, cue_start = excluded.cue_start,
         cue_end = excluded.cue_end, track_gain = excluded.track_gain,
         track_peak = excluded.track_peak,
         album_id = NULL, album_tracks = NULL, album_gain = NULL, album_peak = NULL"
);

/// Records `job`'s members, then the album's values on every one once all of them wrote, the
/// gain left NULL where the album had nothing to level. A member out of reach writes nothing and
/// so leaves the album owed. Answers how many members wrote.
pub async fn record_album(
    db: &DbPool,
    job: &AlbumJob,
    outcome: &AlbumOutcome,
) -> Result<usize, AppError> {
    let rows: Vec<MemberRow> = job
        .members
        .iter()
        .zip(&outcome.tracks)
        .filter_map(|(track, outcome)| MemberRow::of(track, *outcome))
        .collect();
    let members = job.members.len();
    let album = AlbumRow {
        album_id: job.album_id,
        members: i64::try_from(members).unwrap_or(i64::MAX),
        gain: outcome.album,
        tracks: json_array(&job.members.iter().map(|track| track.id).collect::<Vec<_>>()),
    };
    db.transaction(move |tx| {
        let mut written = 0;
        for row in &rows {
            written += usize::from(row.write(tx)?);
        }
        if written == members {
            album.write(tx)?;
        }
        Ok(written)
    })
    .await
}

/// Records one track measured on its own, answering whether it wrote. Out of reach writes nothing.
pub async fn record_track(
    db: &DbPool,
    track: &MeasureTrack,
    outcome: TrackOutcome,
) -> Result<bool, AppError> {
    let Some(row) = MemberRow::of(track, outcome) else {
        return Ok(false);
    };
    db.transaction(move |tx| row.write(tx)).await
}

/// One member's measurement, copied out of its job so the write owns what it binds.
struct MemberRow {
    id: i64,
    file_hash: String,
    cue_start: Option<i64>,
    cue_end: Option<i64>,
    gain: Option<Gain>,
}

impl MemberRow {
    /// `None` for a track out of reach, which may come back and so isn't recorded.
    fn of(track: &MeasureTrack, outcome: TrackOutcome) -> Option<Self> {
        let gain = match outcome {
            TrackOutcome::Measured(gain) => Some(gain),
            TrackOutcome::NoResult => None,
            TrackOutcome::OutOfReach => return None,
        };
        let span = track.audio.span;
        Some(Self {
            id: track.id,
            file_hash: track.file_hash.clone(),
            cue_start: span.map(|span| span.start().into()),
            cue_end: span.and_then(CueSpan::end).map(Into::into),
            gain,
        })
    }

    /// Answers whether the row was still the one measured, and so wrote.
    fn write(&self, tx: &Transaction<'_>) -> Result<bool, AppError> {
        let written = sql::execute(
            tx,
            RECORD_TRACK,
            (
                self.id,
                self.file_hash.as_str(),
                self.cue_start,
                self.cue_end,
                self.gain.map(|gain| gain.db),
                self.gain.map(|gain| gain.peak),
            ),
        )?;
        Ok(written > 0)
    }
}

/// An album's values, set on every member's row together.
struct AlbumRow {
    album_id: i64,
    members: i64,
    gain: Option<Gain>,
    /// The members' ids, as the JSON array `json_each` reads.
    tracks: String,
}

impl AlbumRow {
    fn write(&self, tx: &Transaction<'_>) -> Result<(), AppError> {
        sql::execute(
            tx,
            "UPDATE track_loudness
             SET album_id = ?1, album_tracks = ?2, album_gain = ?3, album_peak = ?4
             WHERE track_id IN (SELECT value FROM json_each(?5))",
            (
                self.album_id,
                self.members,
                self.gain.map(|gain| gain.db),
                self.gain.map(|gain| gain.peak),
                self.tracks.as_str(),
            ),
        )?;
        Ok(())
    }
}
