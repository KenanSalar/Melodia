//! Which sheet claims which image, settled before a folder scan is cut into chunks or a watcher
//! batch's write opens.
//!
//! Only the directories where something moved are read again: a sheet new, changed or gone, or an
//! audio file the incremental filter let through beside a sheet or claimed by one. Every launch
//! rescans, and a library with no sheets returns before this allocates anything.
//!
//! Where several sheets claim one image the winner goes by name ([`name_rank`]), so a sheet the
//! scan didn't read again still competes through the claim the library holds. A claim is **owed**,
//! and the image read again, when the image or its winning sheet is new or changed, or the winner
//! isn't the sheet the library holds. Every image the plan settles leaves the scan's ordinary files
//! and is read in its sheet's job: met there alone, an image with sheet rows would get a whole-file
//! row beside its tracks.
//!
//! **A sheet that fails to read keeps its claims untouched** and records no stamp, so the next scan
//! reads it again. A write caught half-done would otherwise trade the disc's rows for new ones, and
//! their play counts with them. Only a sheet that is gone, or that reads cleanly without naming the
//! image, releases it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use super::layout::{self, FileLayout};
use super::parse::{self, Sheet};
use super::resolve::resolve;
use super::text::read_sheet;
use crate::media::ingest::metadata::date_modified_from_metadata;
use crate::media::ingest::scanner::WalkIndex;
use melodia_core::entities::scan::FileStamp;
use melodia_core::error::{AppError, describe};

/// What one folder's walk and rows say about its sheets.
pub struct PlanInput<'a> {
    pub audio: &'a WalkIndex,
    pub sheets: &'a WalkIndex,
    /// The audio files the incremental filter let through.
    pub changed_audio: &'a [PathBuf],
    /// The sheets new since the last scan, or changed.
    pub changed_sheets: &'a [PathBuf],
    pub gone_sheets: &'a [String],
    /// Each claimed image's sheet, image first, as the library holds them.
    pub claims: &'a HashMap<String, String>,
}

/// The cue work a folder scan owes, as whole sheet jobs.
#[derive(Default)]
pub struct CuePlan {
    jobs: Vec<SheetJob>,
    /// Every image settled here, which the ordinary ingest leaves alone.
    settled: HashSet<PathBuf>,
}

impl CuePlan {
    /// Takes every image this plan settled out of `to_scan`.
    pub fn take_out_of(&self, to_scan: &mut Vec<PathBuf>) {
        if !self.settled.is_empty() {
            to_scan.retain(|path| !self.settles(path));
        }
    }

    /// Whether this plan settled `image`, which the ordinary ingest then leaves alone.
    pub fn settles(&self, image: &Path) -> bool {
        self.settled.contains(image)
    }

    /// The images the jobs read.
    pub fn images(&self) -> usize {
        self.jobs.iter().map(SheetJob::images).sum()
    }

    /// The most rows the jobs write.
    pub fn rows(&self) -> usize {
        self.jobs.iter().map(SheetJob::rows).sum()
    }

    /// The jobs in chunks of about `max_images` images. A job is never split, so a sheet's stamp
    /// commits with everything it caused.
    pub fn into_chunks(self, max_images: usize) -> Vec<CueChunk> {
        let mut chunks = Vec::new();
        let mut current = CueChunk::default();
        let mut images = 0;
        for job in self.jobs {
            if images > 0 && images + job.images() > max_images {
                chunks.push(std::mem::take(&mut current));
                images = 0;
            }
            images += job.images();
            current.jobs.push(job);
        }
        if !current.jobs.is_empty() {
            chunks.push(current);
        }
        chunks
    }

    /// Every job in one chunk, for a watcher batch, which commits in one transaction.
    pub fn into_chunk(self) -> CueChunk {
        CueChunk { jobs: self.jobs }
    }
}

/// Sheet jobs that commit together.
#[derive(Default)]
pub struct CueChunk {
    jobs: Vec<SheetJob>,
}

impl CueChunk {
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// The images to read for this chunk: those claimed, and those released to rows of their own.
    pub fn images(&self) -> Vec<PathBuf> {
        let jobs = self.jobs.iter();
        let claimed = jobs.clone().flat_map(|job| job.claims.iter().map(|claim| &claim.image));
        claimed.chain(jobs.flat_map(|job| &job.releases)).cloned().collect()
    }

    pub(crate) fn jobs(&self) -> &[SheetJob] {
        &self.jobs
    }

    /// Whether a sheet here claims `image`.
    pub(crate) fn claims(&self, image: &Path) -> bool {
        self.jobs.iter().any(|job| job.claims.iter().any(|claim| claim.image == image))
    }
}

/// One sheet and what the scan owes for it.
pub(crate) struct SheetJob {
    pub(crate) path: PathBuf,
    /// `None` for a sheet gone from disk, whose stamp row the scan's commit deletes.
    pub(crate) read: Option<SheetRead>,
    pub(crate) claims: Vec<Claim>,
    /// Images the sheet held and gives back.
    pub(crate) releases: Vec<PathBuf>,
}

impl SheetJob {
    fn images(&self) -> usize {
        self.claims.len() + self.releases.len()
    }

    fn rows(&self) -> usize {
        let carved: usize = self.claims.iter().map(|claim| claim.layout.rows.len()).sum();
        carved + self.releases.len()
    }
}

pub(crate) struct SheetRead {
    pub(crate) stamp: FileStamp,
    pub(crate) sheet: Sheet,
}

pub(crate) struct Claim {
    pub(crate) image: PathBuf,
    pub(crate) layout: FileLayout,
}

/// Plans the cue work for one folder, reading only the images `owns` admits.
pub fn settle(input: &PlanInput<'_>, owns: impl Fn(&Path) -> bool + Sync) -> CuePlan {
    if input.sheets.is_empty() && input.gone_sheets.is_empty() && input.claims.is_empty() {
        return CuePlan::default();
    }
    let held: HashMap<PathBuf, PathBuf> =
        input.claims.iter().map(|(image, sheet)| (image.into(), sheet.into())).collect();
    let dirs = dirs_to_plan(input, &held);
    let read: Vec<(PathBuf, Option<Parsed>)> = dirs
        .par_iter()
        .flat_map_iter(|dir| input.sheets.in_dir(dir))
        .map(|path| (path.to_path_buf(), parse_sheet(path, input.audio, &owns)))
        .collect();
    Settler::new(input, &held, read).settle(&owns)
}

/// The directories whose sheets are read again.
fn dirs_to_plan(input: &PlanInput<'_>, held: &HashMap<PathBuf, PathBuf>) -> BTreeSet<PathBuf> {
    let mut dirs = BTreeSet::new();
    let sheets = input.changed_sheets.iter().map(PathBuf::as_path);
    let gone = input.gone_sheets.iter().map(Path::new);
    dirs.extend(sheets.chain(gone).filter_map(Path::parent).map(Path::to_path_buf));

    let mut last_dir = None;
    for file in input.changed_audio {
        if let Some(sheet) = held.get(file).and_then(|sheet| sheet.parent()) {
            dirs.insert(sheet.to_path_buf());
        }
        let Some(dir) = file.parent() else {
            continue;
        };
        // Neighbours in path order mostly share a directory, so a run asks for its sheets once.
        if last_dir != Some(dir) && input.sheets.in_dir(dir).next().is_some() {
            dirs.insert(dir.to_path_buf());
        }
        last_dir = Some(dir);
    }
    dirs
}

/// A sheet read cleanly, with the images it claims.
struct Parsed {
    read: SheetRead,
    claims: Vec<Claim>,
}

/// Reads the sheet at `path`, or logs why it couldn't and answers `None`.
fn parse_sheet(path: &Path, audio: &WalkIndex, owns: &impl Fn(&Path) -> bool) -> Option<Parsed> {
    match try_parse_sheet(path, audio, owns) {
        Ok(parsed) => Some(parsed),
        Err(e) => {
            log::warn!("Skipping cue sheet {}: {}", path.display(), describe(&e));
            None
        }
    }
}

/// Reads the sheet at `path` and the images it claims. Its stamp is taken ahead of the read, so a
/// write landing between the two leaves an older stamp, which the next scan reads past.
fn try_parse_sheet(
    path: &Path,
    audio: &WalkIndex,
    owns: &impl Fn(&Path) -> bool,
) -> Result<Parsed, AppError> {
    let stat = std::fs::metadata(path)
        .map_err(|e| AppError::metadata(format!("Failed to stat {}", path.display()), e))?;
    let stamp = FileStamp {
        file_size: Some(i64::try_from(stat.len()).unwrap_or(i64::MAX)),
        date_modified: date_modified_from_metadata(&stat),
    };
    let sheet = parse::parse(&read_sheet(path)?, path);
    if !sheet.places_a_track() {
        return Err(AppError::metadata_msg(format!("{} places no track", path.display())));
    }

    let sheet_dir = path.parent().unwrap_or(Path::new(""));
    let mut claims: Vec<Claim> = Vec::new();
    for layout in layout::lay_out(&sheet, path) {
        if !layout.claims() {
            continue;
        }
        let Some(image) = resolve(&sheet.files[layout.file], sheet_dir, audio) else {
            log::debug!("{} names no audio file as {}", path.display(), sheet.files[layout.file]);
            continue;
        };
        if owns(&image) && !claims.iter().any(|claim| claim.image == image) {
            claims.push(Claim { image, layout });
        }
    }
    Ok(Parsed { read: SheetRead { stamp, sheet }, claims })
}

/// A sheet competing for an image.
#[derive(Clone, Copy)]
enum Candidate<'a> {
    /// Read this scan, at this position in the reads.
    Read(usize, &'a Path),
    /// The library's claimer, not read again since nothing around it moved.
    Held(&'a Path),
}

impl<'a> Candidate<'a> {
    fn sheet(self) -> &'a Path {
        match self {
            Self::Read(_, sheet) | Self::Held(sheet) => sheet,
        }
    }
}

/// How closely a sheet's name ties it to `image`, closest first: `Album.cue` for `Album.flac`, then
/// `Album.flac.cue`, then any other.
fn name_rank(sheet: &Path, image: &Path) -> u8 {
    let stem = sheet.file_stem();
    if stem == image.file_stem() {
        0
    } else if stem == image.file_name() {
        1
    } else {
        2
    }
}

/// The decisions over one folder's reads.
struct Settler<'a> {
    input: &'a PlanInput<'a>,
    held: &'a HashMap<PathBuf, PathBuf>,
    reads: Vec<(PathBuf, Option<Parsed>)>,
    /// Each read sheet's position in `reads`.
    positions: HashMap<PathBuf, usize>,
    changed_audio: HashSet<&'a Path>,
    changed_sheets: HashSet<&'a Path>,
    gone: HashSet<&'a Path>,
}

/// What the jobs owe, gathered before the reads are moved into them.
#[derive(Default)]
struct Owed {
    claims: BTreeMap<usize, Vec<PathBuf>>,
    releases: BTreeMap<PathBuf, Vec<PathBuf>>,
    settled: HashSet<PathBuf>,
}

impl<'a> Settler<'a> {
    fn new(
        input: &'a PlanInput<'a>,
        held: &'a HashMap<PathBuf, PathBuf>,
        reads: Vec<(PathBuf, Option<Parsed>)>,
    ) -> Self {
        let positions = reads
            .iter()
            .enumerate()
            .map(|(position, (path, _))| (path.clone(), position))
            .collect();
        Self {
            input,
            held,
            reads,
            positions,
            changed_audio: input.changed_audio.iter().map(PathBuf::as_path).collect(),
            changed_sheets: input.changed_sheets.iter().map(PathBuf::as_path).collect(),
            gone: input.gone_sheets.iter().map(Path::new).collect(),
        }
    }

    fn settle(self, owns: &impl Fn(&Path) -> bool) -> CuePlan {
        let candidates = self.candidates();
        let mut owed = Owed::default();
        let held = self.held.keys().map(PathBuf::as_path);
        let images: BTreeSet<&Path> = candidates.keys().copied().chain(held).collect();
        for image in images {
            owed.settled.insert(image.to_path_buf());
            self.decide(image, candidates.get(image).map_or(&[], Vec::as_slice), owns, &mut owed);
        }
        self.into_plan(owed)
    }

    /// Every sheet claiming each image: those read this scan, and the library's claimer where it
    /// wasn't read again and is still there.
    fn candidates(&self) -> HashMap<&Path, Vec<Candidate<'_>>> {
        let mut candidates: HashMap<&Path, Vec<Candidate<'_>>> = HashMap::new();
        for (position, (sheet, parsed)) in self.reads.iter().enumerate() {
            let claims = parsed.iter().flat_map(|parsed| &parsed.claims);
            for claim in claims {
                candidates.entry(&claim.image).or_default().push(Candidate::Read(position, sheet));
            }
        }
        for (image, sheet) in self.held {
            if !self.positions.contains_key(sheet) && self.input.sheets.contains(sheet) {
                candidates.entry(image).or_default().push(Candidate::Held(sheet));
            }
        }
        candidates
    }

    /// Settles `image` between its `candidates` and the sheet the library holds it under.
    fn decide(
        &self,
        image: &Path,
        candidates: &[Candidate<'_>],
        owns: &impl Fn(&Path) -> bool,
        owed: &mut Owed,
    ) {
        let holder = self.held.get(image).map(PathBuf::as_path);
        if holder.is_some_and(|holder| self.keeps_claims(holder)) {
            return;
        }
        let winner = candidates
            .iter()
            .copied()
            .min_by_key(|candidate| (name_rank(candidate.sheet(), image), candidate.sheet()));
        let Some(winner) = winner else {
            if let Some(holder) = holder
                && self.input.audio.contains(image)
                && owns(image)
            {
                owed.releases.entry(holder.to_path_buf()).or_default().push(image.to_path_buf());
            }
            return;
        };

        for loser in candidates.iter().filter(|candidate| candidate.sheet() != winner.sheet()) {
            log::info!(
                "{} also names {}; {} claims it",
                loser.sheet().display(),
                image.display(),
                winner.sheet().display()
            );
        }
        if let Candidate::Read(position, sheet) = winner {
            let is_owed = self.changed_audio.contains(image)
                || self.changed_sheets.contains(sheet)
                || holder != Some(sheet);
            if is_owed {
                owed.claims.entry(position).or_default().push(image.to_path_buf());
            }
        }
    }

    /// Whether the library's claimer of an image keeps its claim untouched: it failed to read, or
    /// sits where the walk couldn't look.
    fn keeps_claims(&self, holder: &Path) -> bool {
        if self.gone.contains(holder) {
            return false;
        }
        match self.positions.get(holder) {
            Some(&position) => self.reads[position].1.is_none(),
            None => !self.input.sheets.contains(holder),
        }
    }

    /// The jobs: each sheet owing a claim or a release, and each new or changed sheet read cleanly,
    /// whose stamp is owed even where it claims nothing.
    fn into_plan(self, mut owed: Owed) -> CuePlan {
        let mut jobs = Vec::new();
        for (position, (path, parsed)) in self.reads.into_iter().enumerate() {
            let Some(parsed) = parsed else {
                continue;
            };
            let images = owed.claims.remove(&position).unwrap_or_default();
            let releases = owed.releases.remove(&path).unwrap_or_default();
            let changed = self.changed_sheets.contains(path.as_path());
            if images.is_empty() && releases.is_empty() && !changed {
                continue;
            }
            let claims =
                parsed.claims.into_iter().filter(|claim| images.contains(&claim.image)).collect();
            jobs.push(SheetJob { path, read: Some(parsed.read), claims, releases });
        }
        // What is left releases from sheets gone from disk.
        for (path, releases) in owed.releases {
            jobs.push(SheetJob { path, read: None, claims: Vec::new(), releases });
        }
        CuePlan { jobs, settled: owed.settled }
    }
}
