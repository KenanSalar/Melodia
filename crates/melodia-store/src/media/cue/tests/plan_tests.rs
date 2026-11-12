//! Which sheet claims which image, and what a scan owes for it, over a real folder walked as the
//! scan walks one: sheets as text on disk, images as empty files the walk lists.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use crate::media::ingest::scanner::{Unobserved, collect_media_files};

fn splitting(image: &str) -> String {
    format!(
        "FILE \"{image}\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 04:00:00"
    )
}

struct Folder {
    tmp: TempDir,
}

impl Folder {
    fn with(files: &[(&str, &str)]) -> Result<Self, AppError> {
        let tmp = TempDir::new()?;
        for (name, content) in files {
            fs::write(tmp.path().join(name), content)?;
        }
        Ok(Self { tmp })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.tmp.path().join(name)
    }

    fn spelled(&self, name: &str) -> String {
        self.path(name).to_string_lossy().into_owned()
    }

    fn name_of<'a>(&self, path: &'a Path) -> &'a str {
        path.strip_prefix(self.tmp.path()).ok().and_then(Path::to_str).unwrap_or_default()
    }

    /// The plan for this folder, `changed` and `gone` named as the incremental filter would hand
    /// them over and `held` as the library's `(image, sheet)` claims.
    fn plan(
        &self,
        changed: &[&str],
        gone: &[&str],
        held: &[(&str, &str)],
    ) -> Result<CuePlan, AppError> {
        self.plan_owning(changed, gone, held, |_| true)
    }

    fn plan_owning(
        &self,
        changed: &[&str],
        gone: &[&str],
        held: &[(&str, &str)],
        owns: impl Fn(&Path) -> bool + Sync,
    ) -> Result<CuePlan, AppError> {
        let walk = collect_media_files(self.tmp.path(), &Unobserved).ok_or_else(|| {
            AppError::validation_msg("a walk nothing cancelled came back stopped")
        })?;
        let audio = WalkIndex::new(walk.files, walk.unreadable.clone());
        let sheets = WalkIndex::new(walk.sheets, walk.unreadable);
        let (changed_sheets, changed_audio): (Vec<PathBuf>, Vec<PathBuf>) =
            changed.iter().map(|name| self.path(name)).partition(|path| sheets.contains(path));
        let gone_sheets: Vec<String> = gone.iter().map(|name| self.spelled(name)).collect();
        let claims: HashMap<String, String> =
            held.iter().map(|(image, sheet)| (self.spelled(image), self.spelled(sheet))).collect();
        let input = PlanInput {
            audio: &audio,
            sheets: &sheets,
            changed_audio: &changed_audio,
            changed_sheets: &changed_sheets,
            gone_sheets: &gone_sheets,
            claims: &claims,
        };
        Ok(settle(&input, owns))
    }

    /// Each job as `(sheet, read cleanly, images claimed, images given back)`, by file name.
    fn jobs<'a>(&self, plan: &'a CuePlan) -> Vec<(&'a str, bool, Vec<&'a str>, Vec<&'a str>)> {
        let mut jobs: Vec<_> = plan
            .jobs
            .iter()
            .map(|job| {
                let claims = job.claims.iter().map(|claim| self.name_of(&claim.image)).collect();
                let releases = job.releases.iter().map(|image| self.name_of(image)).collect();
                (self.name_of(&job.path), job.read.is_some(), claims, releases)
            })
            .collect();
        jobs.sort();
        jobs
    }
}

#[test]
fn a_new_sheet_claims_the_image_beside_it() -> Result<(), AppError> {
    let folder = Folder::with(&[("album.flac", ""), ("album.cue", &splitting("album.flac"))])?;

    let plan = folder.plan(&["album.flac", "album.cue"], &[], &[])?;

    assert_eq!(folder.jobs(&plan), [("album.cue", true, vec!["album.flac"], vec![])]);
    Ok(())
}

#[test]
fn every_image_a_plan_settles_leaves_the_ordinary_scan() -> Result<(), AppError> {
    let folder = Folder::with(&[
        ("album.flac", ""),
        ("single.flac", ""),
        ("album.cue", &splitting("album.flac")),
    ])?;
    let plan = folder.plan(&["album.flac", "single.flac", "album.cue"], &[], &[])?;
    let mut to_scan = vec![folder.path("album.flac"), folder.path("single.flac")];

    plan.take_out_of(&mut to_scan);

    assert_eq!(to_scan, [folder.path("single.flac")]);
    Ok(())
}

#[test]
fn a_new_sheet_claiming_nothing_still_owes_its_stamp() -> Result<(), AppError> {
    let rip = "FILE \"01.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n\
               FILE \"02.flac\" WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00";
    let folder = Folder::with(&[("01.flac", ""), ("02.flac", ""), ("rip.cue", rip)])?;

    let plan = folder.plan(&["01.flac", "02.flac", "rip.cue"], &[], &[])?;

    assert_eq!(folder.jobs(&plan), [("rip.cue", true, vec![], vec![])]);
    Ok(())
}

#[test]
fn a_sheet_s_name_ranks_it_for_an_image() {
    let image = Path::new("Album.flac");
    let rows = [("Album.cue", 0), ("Album.flac.cue", 1), ("Disc.cue", 2), ("album.cue", 2)];
    for (sheet, rank) in rows {
        assert_eq!(name_rank(Path::new(sheet), image), rank, "{sheet}");
    }
}

#[test]
fn of_several_sheets_naming_one_image_the_closest_name_wins() -> Result<(), AppError> {
    let folder = Folder::with(&[
        ("album.flac", ""),
        ("a.cue", &splitting("album.flac")),
        ("album.flac.cue", &splitting("album.flac")),
    ])?;

    let plan = folder.plan(&["album.flac", "a.cue", "album.flac.cue"], &[], &[])?;

    assert_eq!(
        folder.jobs(&plan),
        [("a.cue", true, vec![], vec![]), ("album.flac.cue", true, vec!["album.flac"], vec![])]
    );
    Ok(())
}

#[test]
fn of_sheets_named_alike_for_an_image_the_lowest_path_wins() -> Result<(), AppError> {
    let folder = Folder::with(&[
        ("album.flac", ""),
        ("b.cue", &splitting("album.flac")),
        ("a.cue", &splitting("album.flac")),
    ])?;

    let plan = folder.plan(&["album.flac", "a.cue", "b.cue"], &[], &[])?;

    assert_eq!(
        folder.jobs(&plan),
        [("a.cue", true, vec!["album.flac"], vec![]), ("b.cue", true, vec![], vec![])]
    );
    Ok(())
}

#[test]
fn a_claim_whose_image_and_sheet_are_unchanged_owes_nothing() -> Result<(), AppError> {
    let folder = Folder::with(&[("album.flac", ""), ("album.cue", &splitting("album.flac"))])?;

    let plan = folder.plan(&[], &[], &[("album.flac", "album.cue")])?;

    assert!(plan.jobs.is_empty(), "{:?}", folder.jobs(&plan));
    assert!(plan.settles(&folder.path("album.flac")), "and the image is still the sheet's");
    Ok(())
}

#[test]
fn a_claim_is_owed_again_when_its_image_or_its_sheet_changed() -> Result<(), AppError> {
    let folder = Folder::with(&[("album.flac", ""), ("album.cue", &splitting("album.flac"))])?;
    let held = [("album.flac", "album.cue")];

    for changed in ["album.flac", "album.cue"] {
        let plan = folder.plan(&[changed], &[], &held)?;
        let expected = [("album.cue", true, vec!["album.flac"], vec![])];
        assert_eq!(folder.jobs(&plan), expected, "{changed} changed");
    }
    Ok(())
}

/// The claimer the library holds is read again beside the new sheet and keeps the image by name,
/// so it owes no rewrite either.
#[test]
fn a_new_sheet_does_not_take_an_image_from_a_closer_named_claimer() -> Result<(), AppError> {
    let folder = Folder::with(&[
        ("album.flac", ""),
        ("album.cue", &splitting("album.flac")),
        ("other.cue", &splitting("album.flac")),
    ])?;

    let plan = folder.plan(&["other.cue"], &[], &[("album.flac", "album.cue")])?;

    assert_eq!(folder.jobs(&plan), [("other.cue", true, vec![], vec![])]);
    Ok(())
}

#[test]
fn a_new_sheet_with_a_closer_name_takes_the_image_over() -> Result<(), AppError> {
    let folder = Folder::with(&[
        ("album.flac", ""),
        ("album.cue", &splitting("album.flac")),
        ("other.cue", &splitting("album.flac")),
    ])?;

    let plan = folder.plan(&["album.cue"], &[], &[("album.flac", "other.cue")])?;

    assert_eq!(folder.jobs(&plan), [("album.cue", true, vec!["album.flac"], vec![])]);
    Ok(())
}

#[test]
fn a_held_sheet_that_fails_to_read_keeps_its_claim_untouched() -> Result<(), AppError> {
    let folder = Folder::with(&[("album.flac", ""), ("album.cue", "TITLE \"Half written")])?;

    let plan = folder.plan(&["album.cue"], &[], &[("album.flac", "album.cue")])?;

    assert!(plan.jobs.is_empty(), "{:?}", folder.jobs(&plan));
    assert!(plan.settles(&folder.path("album.flac")), "and the image stays out of the scan");
    Ok(())
}

#[test]
fn a_gone_sheet_gives_its_image_back() -> Result<(), AppError> {
    let folder = Folder::with(&[("album.flac", "")])?;

    let plan = folder.plan(&[], &["album.cue"], &[("album.flac", "album.cue")])?;

    assert_eq!(folder.jobs(&plan), [("album.cue", false, vec![], vec!["album.flac"])]);
    Ok(())
}

#[test]
fn a_sheet_that_reads_cleanly_without_naming_the_image_gives_it_back() -> Result<(), AppError> {
    let folder = Folder::with(&[("album.flac", ""), ("album.cue", &splitting("elsewhere.flac"))])?;

    let plan = folder.plan(&["album.cue"], &[], &[("album.flac", "album.cue")])?;

    assert_eq!(folder.jobs(&plan), [("album.cue", true, vec![], vec!["album.flac"])]);
    Ok(())
}

#[test]
fn a_gone_sheet_whose_image_is_gone_too_gives_nothing_back() -> Result<(), AppError> {
    let folder = Folder::with(&[])?;

    let plan = folder.plan(&[], &["album.cue"], &[("album.flac", "album.cue")])?;

    assert!(plan.jobs.is_empty(), "{:?}", folder.jobs(&plan));
    Ok(())
}

#[test]
fn an_image_the_caller_does_not_own_is_neither_claimed_nor_given_back() -> Result<(), AppError> {
    let folder = Folder::with(&[
        ("album.flac", ""),
        ("other.flac", ""),
        ("album.cue", &splitting("album.flac")),
    ])?;
    let not_album = |path: &Path| path.file_name() != Some("album.flac".as_ref());

    let claimed = folder.plan_owning(&["album.cue"], &[], &[], not_album)?;
    let released =
        folder.plan_owning(&[], &["gone.cue"], &[("album.flac", "gone.cue")], not_album)?;

    assert_eq!(folder.jobs(&claimed), [("album.cue", true, vec![], vec![])]);
    assert!(released.jobs.is_empty(), "{:?}", folder.jobs(&released));
    Ok(())
}

fn two_image_sheet(first: &str, second: &str) -> String {
    format!("{}\n{}", splitting(first), splitting(second).replace("TRACK 0", "TRACK 1"))
}

#[test]
fn chunks_keep_each_job_whole_within_the_image_budget() -> Result<(), AppError> {
    let folder = Folder::with(&[
        ("a1.flac", ""),
        ("a2.flac", ""),
        ("b1.flac", ""),
        ("b2.flac", ""),
        ("c.flac", ""),
        ("a.cue", &two_image_sheet("a1.flac", "a2.flac")),
        ("b.cue", &two_image_sheet("b1.flac", "b2.flac")),
        ("c.cue", &splitting("c.flac")),
    ])?;
    let changed = ["a.cue", "b.cue", "c.cue"];
    let images_per_chunk = |max: usize| -> Result<Vec<usize>, AppError> {
        let plan = folder.plan(&changed, &[], &[])?;
        Ok(plan.into_chunks(max).iter().map(|chunk| chunk.images().len()).collect())
    };

    let rows = [(1, vec![2, 2, 1]), (3, vec![2, 3]), (4, vec![4, 1]), (5, vec![5])];
    for (max, sizes) in rows {
        assert_eq!(images_per_chunk(max)?, sizes, "at most {max} images a chunk");
    }
    Ok(())
}

#[test]
fn the_plan_counts_the_images_it_reads_and_the_rows_it_may_write() -> Result<(), AppError> {
    let folder = Folder::with(&[
        ("a1.flac", ""),
        ("a2.flac", ""),
        ("a.cue", &two_image_sheet("a1.flac", "a2.flac")),
        ("b.flac", ""),
    ])?;

    let plan = folder.plan(&["a.cue"], &["b.cue"], &[("b.flac", "b.cue")])?;

    assert_eq!((plan.images(), plan.rows()), (3, 5));
    Ok(())
}
