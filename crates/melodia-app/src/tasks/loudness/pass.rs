//! One pass: send back the albums whose values went stale, count what is owed, then measure it a
//! job at a time on the pool, each job written as it lands.
//!
//! **The queue goes first.** At each job boundary the albums and tracks the queue window wants
//! levelled are taken ahead of the keyset, which then skips them, so during a long backlog what is
//! about to play is measured next.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::pool::{MEASURE_WORKERS, Workers};
use crate::library::playback::player_replaygain_levels_measured;
use crate::state::{LoudnessControl, MeasurePhase, MeasureProgress, PlaybackContext, ScanControl};
use melodia_audio::loudness::{
    AlbumOutcome, Gain, Stopped, TrackOutcome, measure_album, measure_track,
};
use melodia_core::entities::loudness::REPLAYGAIN_REFERENCE_LUFS;
use melodia_core::error::{AppResult, describe};
use melodia_platform::services::platform::allocator::trim;
use melodia_store::database::DbPool;
use melodia_store::database::queries::loudness::{self, AlbumJob, MeasureTrack, QueuedJobs};

/// Albums read per keyset page: enough to keep the pool fed between reads, few enough that a page
/// holds a handful of paths.
const ALBUMS_PER_PAGE: i64 = 32;

/// Tracks read per keyset page, by the same measure.
const TRACKS_PER_PAGE: i64 = 256;

/// A job's identity across passes, for the session's out-of-reach set and a pass's own skips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum JobKey {
    Album(i64),
    Track(i64),
}

/// What a pass answers to, shared by every pass the wake loop runs.
pub(super) struct Controls {
    pub(super) db: DbPool,
    pub(super) loudness: Arc<LoudnessControl>,
    pub(super) scan: Arc<ScanControl>,
    pub(super) playback: PlaybackContext,
    pub(super) shutdown: CancellationToken,
}

impl Controls {
    /// `ReplayGain` and measuring both on, and not stopped this session.
    pub(super) fn measuring(&self) -> bool {
        !self.loudness.is_stopped() && player_replaygain_levels_measured(&self.playback)
    }

    fn halt(&self) -> Option<Halt> {
        if self.shutdown.is_cancelled() {
            Some(Halt::Shutdown)
        } else if self.loudness.is_stopped() {
            Some(Halt::User)
        } else if !player_replaygain_levels_measured(&self.playback) {
            Some(Halt::Off)
        } else if self.scan.is_busy() {
            Some(Halt::Scan)
        } else {
            None
        }
    }
}

/// Why a pass ended before its work did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Halt {
    Shutdown,
    User,
    /// `ReplayGain` or measuring switched off, by hand or by bit-perfect's reset.
    Off,
    Scan,
    /// A read or a write failed, and the next wake tries again.
    Failed,
}

/// The stop a pass's decodes ask between blocks. The first reason seen holds for the rest of the
/// pass, so a condition flickering back can't leave one worker running on what another stopped.
struct PassStop {
    controls: Arc<Controls>,
    halted: OnceLock<Halt>,
}

impl PassStop {
    fn should_stop(&self) -> bool {
        if self.halted.get().is_some() {
            return true;
        }
        let Some(halt) = self.controls.halt() else {
            return false;
        };
        self.halted.get_or_init(|| halt);
        true
    }

    fn abandon(&self) {
        self.halted.get_or_init(|| Halt::Failed);
    }
}

/// How a pass ended, for the wake loop to publish.
pub(super) struct PassEnd {
    halt: Option<Halt>,
    done: u32,
    total: u32,
}

impl PassEnd {
    /// `None` at shutdown, when nobody is left to read it.
    pub(super) fn progress(&self) -> Option<MeasureProgress> {
        let left = self.done < self.total;
        let phase = match self.halt {
            Some(Halt::Shutdown) => return None,
            Some(Halt::User) if left => MeasurePhase::Stopped,
            Some(Halt::Scan) if left => MeasurePhase::WaitingForScan,
            _ => return Some(MeasureProgress::default()),
        };
        Some(MeasureProgress { phase, done: self.done, total: self.total })
    }
}

/// Runs one pass to its end, `out_of_reach` holding the jobs this session found out of reach,
/// which it adds to.
pub(super) async fn run(controls: &Arc<Controls>, out_of_reach: &mut HashSet<JobKey>) -> PassEnd {
    let stop = Arc::new(PassStop { controls: controls.clone(), halted: OnceLock::new() });
    let mut tally = Tally::default();
    if let Err(e) = measure_owed(controls, &stop, out_of_reach, &mut tally).await {
        log::warn!("Loudness pass failed: {}", describe(&e));
    }
    if tally.dispatched {
        // A pass outlasts the startup trims, so it hands back what its decoders freed itself.
        trim();
    }
    PassEnd { halt: stop.halted.get().copied(), done: tally.done, total: tally.total }
}

async fn measure_owed(
    controls: &Controls,
    stop: &Arc<PassStop>,
    out_of_reach: &mut HashSet<JobKey>,
    tally: &mut Tally,
) -> AppResult<()> {
    let stale = loudness::settle(&controls.db).await?;
    if stale > 0 {
        log::debug!("Loudness: {stale} album(s) sent back to be measured whole again");
    }
    let pending = loudness::pending(&controls.db).await?;
    if pending.tracks == 0 {
        return Ok(());
    }
    tally.total = pending.tracks;

    let started = Instant::now();
    let mut workers = Workers::default();
    let (landed_tx, mut landed) = mpsc::unbounded_channel();
    let mut work = WorkList::new(controls.db.clone(), controls.loudness.subscribe_queued());
    let mut in_flight: Vec<JobKey> = Vec::with_capacity(MEASURE_WORKERS);
    let outcome: AppResult<()> = async {
        loop {
            while in_flight.len() < MEASURE_WORKERS && !stop.should_stop() {
                let Some(job) = work.next(&in_flight, out_of_reach).await? else {
                    break;
                };
                if !tally.dispatched {
                    tally.dispatched = true;
                    log::info!(
                        "Loudness pass: {} track(s) owed a measurement, {} album(s) among them",
                        pending.tracks,
                        pending.albums
                    );
                    publish_running(controls, tally, &work);
                }
                in_flight.push(job.key());
                let stop = stop.clone();
                let landed_tx = landed_tx.clone();
                workers.spawn(move || {
                    let key = job.key();
                    // A closed channel means the pass gave up on its jobs; there is nobody to tell.
                    let _ = landed_tx.send((key, job.measure(&stop)));
                })?;
            }
            if in_flight.is_empty() {
                return Ok(());
            }
            let Some((key, measured)) = landed.recv().await else {
                return Ok(());
            };
            in_flight.retain(|held| *held != key);
            work.landed(key);
            if let Ok(done) = measured {
                record(&controls.db, done, tally, out_of_reach).await?;
                controls.loudness.gains_committed();
                publish_running(controls, tally, &work);
            }
        }
    }
    .await;
    tally.total = reachable_total(tally, &work);
    if outcome.is_err() {
        stop.abandon();
    }
    if tally.dispatched {
        log::info!(
            "Loudness pass {} after {:.1}s: {} album(s) and {} track(s) measured, {} with nothing \
             to level, {} out of reach",
            stop.halted.get().map_or("finished", |halt| halt.ended()),
            started.elapsed().as_secs_f64(),
            tally.albums,
            tally.measured,
            tally.no_result,
            tally.out_of_reach,
        );
    }
    outcome
}

impl Halt {
    fn ended(self) -> &'static str {
        match self {
            Self::Shutdown => "stopped by quit",
            Self::User => "stopped by the user",
            Self::Off => "stopped as measuring was switched off",
            Self::Scan => "stopped for a library scan",
            Self::Failed => "failed",
        }
    }
}

fn publish_running(controls: &Controls, tally: &Tally, work: &WorkList) {
    controls.loudness.publish(MeasureProgress {
        phase: MeasurePhase::Running,
        done: tally.done,
        total: reachable_total(tally, work),
    });
}

fn reachable_total(tally: &Tally, work: &WorkList) -> u32 {
    tally.total.saturating_sub(work.skipped_tracks).max(tally.done)
}

/// What a pass has done, for its progress and its closing line.
#[derive(Default)]
struct Tally {
    dispatched: bool,
    /// Tracks decoded, of the `total` the pass set out to.
    done: u32,
    total: u32,
    albums: u32,
    measured: u32,
    no_result: u32,
    out_of_reach: u32,
}

impl Tally {
    fn count(&mut self, outcome: TrackOutcome) {
        self.done += 1;
        match outcome {
            TrackOutcome::Measured(_) => self.measured += 1,
            TrackOutcome::NoResult => self.no_result += 1,
            TrackOutcome::OutOfReach => self.out_of_reach += 1,
        }
    }
}

enum Job {
    Album(AlbumJob),
    Track(MeasureTrack),
}

/// A job measured to its end, with what it measured.
enum Done {
    Album(AlbumJob, AlbumOutcome),
    Track(MeasureTrack, TrackOutcome),
}

impl Job {
    fn key(&self) -> JobKey {
        match self {
            Self::Album(job) => JobKey::Album(job.album_id),
            Self::Track(track) => JobKey::Track(track.id),
        }
    }

    fn tracks(&self) -> u32 {
        match self {
            Self::Album(job) => u32::try_from(job.members.len()).unwrap_or(u32::MAX),
            Self::Track(_) => 1,
        }
    }

    /// **Blocking**: decodes the job through, asking `stop` between blocks.
    fn measure(self, stop: &PassStop) -> Result<Done, Stopped> {
        let stopping = || stop.should_stop();
        match self {
            Self::Album(job) => {
                let outcome =
                    measure_album(job.members.iter().map(|track| &track.audio), stopping)?;
                Ok(Done::Album(job, outcome))
            }
            Self::Track(track) => {
                let outcome = measure_track(&track.audio, stopping)?;
                Ok(Done::Track(track, outcome))
            }
        }
    }
}

async fn record(
    db: &DbPool,
    done: Done,
    tally: &mut Tally,
    out_of_reach: &mut HashSet<JobKey>,
) -> AppResult<()> {
    match done {
        Done::Album(job, outcome) => {
            let written = loudness::record_album(db, &job, &outcome).await?;
            log_album(&job, &outcome, written);
            tally.albums += 1;
            for track in &outcome.tracks {
                tally.count(*track);
            }
            if outcome.tracks.contains(&TrackOutcome::OutOfReach) {
                out_of_reach.insert(JobKey::Album(job.album_id));
            }
        }
        Done::Track(track, outcome) => {
            loudness::record_track(db, &track, outcome).await?;
            log_track(&track, outcome);
            tally.count(outcome);
            if outcome == TrackOutcome::OutOfReach {
                out_of_reach.insert(JobKey::Track(track.id));
            }
        }
    }
    Ok(())
}

fn log_album(job: &AlbumJob, outcome: &AlbumOutcome, written: usize) {
    for (track, measured) in job.members.iter().zip(&outcome.tracks) {
        log_track(track, *measured);
    }
    let folder = job.members.first().and_then(|track| Path::new(&track.audio.path).parent());
    let folder = folder.map_or_else(String::new, |folder| folder.display().to_string());
    if written < job.members.len() {
        log::debug!(
            "Loudness: album {} ({folder}) not recorded whole, {written} of {} member(s) written",
            job.album_id,
            job.members.len()
        );
        return;
    }
    match outcome.album {
        Some(gain) => log::debug!("Loudness: album {} ({folder}): {}", job.album_id, levels(gain)),
        None => log::debug!("Loudness: album {} ({folder}): nothing to level", job.album_id),
    }
}

fn log_track(track: &MeasureTrack, outcome: TrackOutcome) {
    match outcome {
        TrackOutcome::Measured(gain) => log::debug!("Loudness: {}: {}", track.audio, levels(gain)),
        TrackOutcome::NoResult => log::debug!("Loudness: {}: nothing to level", track.audio),
        TrackOutcome::OutOfReach => {
            log::debug!("Loudness: {}: out of reach, tried again next launch", track.audio);
        }
    }
}

fn levels(gain: Gain) -> String {
    let loudness = REPLAYGAIN_REFERENCE_LUFS - gain.db;
    format!("{loudness:.2} LUFS, gain {:+.2} dB, peak {:.6}", gain.db, gain.peak)
}

/// The jobs a pass has yet to take: the queue's first, then the keyset's albums, then its lone
/// tracks.
struct WorkList {
    db: DbPool,
    queue_ids: watch::Receiver<Vec<i64>>,
    queued: VecDeque<Job>,
    /// Taken from the queue this pass, which the keyset skips when it gets there.
    queue_taken: HashSet<JobKey>,
    stage: Stage,
    page: VecDeque<Job>,
    /// Tracks the keyset passed over as out of reach this session, off the pass's total.
    skipped_tracks: u32,
}

#[derive(Clone, Copy)]
enum Stage {
    Albums { after: i64 },
    Tracks { after: i64 },
    Done,
}

impl WorkList {
    fn new(db: DbPool, mut queue_ids: watch::Receiver<Vec<i64>>) -> Self {
        // So the first job boundary reads what the window already holds.
        queue_ids.mark_changed();
        Self {
            db,
            queue_ids,
            queued: VecDeque::new(),
            queue_taken: HashSet::new(),
            stage: Stage::Albums { after: 0 },
            page: VecDeque::new(),
            skipped_tracks: 0,
        }
    }

    /// Drops the queue's copy of `key`, read while the keyset's ran, so it isn't measured twice.
    fn landed(&mut self, key: JobKey) {
        self.queued.retain(|job| job.key() != key);
    }

    async fn next(
        &mut self,
        in_flight: &[JobKey],
        out_of_reach: &HashSet<JobKey>,
    ) -> AppResult<Option<Job>> {
        if self.queue_ids.has_changed().unwrap_or(false) {
            let ids = self.queue_ids.borrow_and_update().clone();
            let QueuedJobs { albums, tracks } = loudness::queued_jobs(&self.db, &ids).await?;
            let queued =
                albums.into_iter().map(Job::Album).chain(tracks.into_iter().map(Job::Track));
            self.queued = queued.filter(|job| !self.queue_taken.contains(&job.key())).collect();
        }
        while let Some(job) = self.queued.pop_front() {
            let key = job.key();
            if in_flight.contains(&key) || out_of_reach.contains(&key) {
                continue;
            }
            self.queue_taken.insert(key);
            return Ok(Some(job));
        }
        while let Some(job) = self.next_in_keyset().await? {
            let key = job.key();
            if self.queue_taken.contains(&key) {
                continue;
            }
            if out_of_reach.contains(&key) {
                self.skipped_tracks += job.tracks();
                continue;
            }
            return Ok(Some(job));
        }
        Ok(None)
    }

    async fn next_in_keyset(&mut self) -> AppResult<Option<Job>> {
        loop {
            if let Some(job) = self.page.pop_front() {
                return Ok(Some(job));
            }
            match self.stage {
                // Only an empty page ends a stage: an album page reads its members apart from its
                // ids, so an album emptied between the two leaves a full page looking short.
                Stage::Albums { after } => {
                    let page = loudness::album_jobs_after(&self.db, after, ALBUMS_PER_PAGE).await?;
                    self.stage = match page.last() {
                        Some(last) => Stage::Albums { after: last.album_id },
                        None => Stage::Tracks { after: 0 },
                    };
                    self.page.extend(page.into_iter().map(Job::Album));
                }
                Stage::Tracks { after } => {
                    let page = loudness::track_jobs_after(&self.db, after, TRACKS_PER_PAGE).await?;
                    self.stage = match page.last() {
                        Some(last) => Stage::Tracks { after: last.id },
                        None => Stage::Done,
                    };
                    self.page.extend(page.into_iter().map(Job::Track));
                }
                Stage::Done => return Ok(None),
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/loudness_pass_tests.rs"]
mod tests;
