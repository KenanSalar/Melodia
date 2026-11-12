//! A sheet's job written through a scan chunk's commit, the planner run over a real folder and its
//! claims read back from the database. What these hold is what the user built up on the rows: ids
//! kept across a reread, and playlist places carried to the tracks and back.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use crate::database::queries::fixtures::{keep_out, make_test_metadata};
use crate::database::queries::ingest::{Ingested, ScanChunk, commit_scan_chunk};
use crate::database::queries::{self, scan};
use crate::database::{DbPool, sql};
use crate::media::cue::{CueChunk, PlanInput, settle};
use crate::media::ingest::scan_pool::ScanPool;
use crate::media::ingest::scanner::{Unobserved, WalkIndex, collect_media_files};
use melodia_core::entities::scan::ScannedFile;
use melodia_core::error::AppError;

const SCAN_TIMESTAMP: &str = "2026-11-11T00:00:00+00:00";

/// The images' length, past every track the sheets here place.
const IMAGE_MS: i64 = 600_000;

fn sheet(image: &str, starts: &[&str]) -> String {
    let mut lines = vec![format!("FILE \"{image}\" WAVE")];
    for (index, start) in starts.iter().enumerate() {
        lines.push(format!("TRACK {:02} AUDIO", index + 1));
        lines.push(format!("TITLE \"Track {}\"", index + 1));
        lines.push(format!("INDEX 01 {start}"));
    }
    lines.join("\n")
}

const THREE: [&str; 3] = ["00:00:00", "03:00:00", "07:00:00"];

/// Folder 1, the one `keep_out` files its exclusion under.
struct Library {
    tmp: TempDir,
    db: DbPool,
    folder_id: i64,
}

type Row = (Option<i64>, i64, String);

impl Library {
    async fn new() -> Result<Self, AppError> {
        let tmp = TempDir::new()?;
        let db = DbPool::test_pool().await?;
        let folder_id =
            queries::folder::insert_folder(&db, &tmp.path().to_string_lossy(), true).await?.id;
        Ok(Self { tmp, db, folder_id })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.tmp.path().join(name)
    }

    fn spelled(&self, name: &str) -> String {
        self.path(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, content: &str) -> Result<(), AppError> {
        Ok(fs::write(self.path(name), content)?)
    }

    fn read(path: PathBuf) -> ScannedFile {
        let stem =
            path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let mut metadata = make_test_metadata(&stem);
        metadata.duration_ms = IMAGE_MS;
        metadata.file_hash = blake3::hash(path.to_string_lossy().as_bytes()).to_hex().to_string();
        ScannedFile { path, metadata }
    }

    async fn scan_whole(&self, files: &[&str]) -> Result<Vec<i64>, AppError> {
        let files = files.iter().map(|name| Self::read(self.path(name))).collect();
        self.commit(files, CueChunk::default()).await
    }

    /// Plans the folder as a scan would with `changed` sheets new or edited and `gone` ones
    /// deleted, then commits the plan's jobs with every image they read.
    async fn scan_sheets(&self, changed: &[&str], gone: &[&str]) -> Result<(), AppError> {
        let walk = collect_media_files(self.tmp.path(), &Unobserved).ok_or_else(|| {
            AppError::validation_msg("a walk nothing cancelled came back stopped")
        })?;
        let audio = WalkIndex::new(walk.files, walk.unreadable.clone());
        let sheets = WalkIndex::new(walk.sheets, walk.unreadable);
        let claims: HashMap<String, String> = scan::sheet_claims(&self.db, self.folder_id).await?;
        let changed_sheets: Vec<PathBuf> = changed.iter().map(|name| self.path(name)).collect();
        let gone_sheets: Vec<String> = gone.iter().map(|name| self.spelled(name)).collect();
        let input = PlanInput {
            audio: &audio,
            sheets: &sheets,
            changed_audio: &[],
            changed_sheets: &changed_sheets,
            gone_sheets: &gone_sheets,
            claims: &claims,
        };
        let cue = settle(&input, |_| true).into_chunk();
        let images: Vec<ScannedFile> = cue.images().into_iter().map(Self::read).collect();
        self.commit(images, cue).await?;
        Ok(())
    }

    async fn commit(&self, files: Vec<ScannedFile>, cue: CueChunk) -> Result<Vec<i64>, AppError> {
        let chunk = ScanChunk { files, cue };
        let result = commit_scan_chunk(
            &self.db,
            chunk,
            self.folder_id,
            SCAN_TIMESTAMP.to_owned(),
            ScanPool::default(),
            &Ingested::for_delta(0),
        )
        .await?;
        Ok(result.inserted_track_ids)
    }

    async fn rows(&self, name: &str) -> Result<Vec<Row>, AppError> {
        let path = self.spelled(name);
        self.db
            .read(move |conn| {
                sql::fetch_all(
                    conn,
                    "SELECT cue_track, id, title FROM tracks WHERE file_path = ? ORDER BY cue_track",
                    (path,),
                )
            })
            .await
    }

    async fn ids(&self, name: &str) -> Result<Vec<i64>, AppError> {
        Ok(self.rows(name).await?.into_iter().map(|(_, id, _)| id).collect())
    }

    async fn playlist(&self, ids: &[i64]) -> Result<i64, AppError> {
        queries::playlist::create_playlist_with_tracks(
            &self.db,
            "Mix".to_owned(),
            None,
            ids.to_vec(),
        )
        .await
    }

    async fn playlist_items(&self, playlist_id: i64) -> Result<Vec<i64>, AppError> {
        self.db
            .read(move |conn| {
                sql::fetch_all(
                    conn,
                    "SELECT track_id FROM playlist_items WHERE playlist_id = ? ORDER BY position",
                    (playlist_id,),
                )
            })
            .await
    }
}

#[tokio::test]
async fn a_sheet_carves_a_row_per_track_in_place_of_the_whole_file() -> Result<(), AppError> {
    let library = Library::new().await?;
    library.write("album.flac", "")?;
    library.scan_whole(&["album.flac"]).await?;
    library.write("album.cue", &sheet("album.flac", &THREE))?;

    library.scan_sheets(&["album.cue"], &[]).await?;

    let rows: Vec<(Option<i64>, String)> = library
        .rows("album.flac")
        .await?
        .into_iter()
        .map(|(track, _, title)| (track, title))
        .collect();
    assert_eq!(
        rows,
        [
            (Some(1), "Track 1".to_owned()),
            (Some(2), "Track 2".to_owned()),
            (Some(3), "Track 3".to_owned())
        ]
    );
    Ok(())
}

#[tokio::test]
async fn each_row_stores_the_span_its_sheet_places_it_at() -> Result<(), AppError> {
    let library = Library::new().await?;
    library.write("album.flac", "")?;
    library.write("album.cue", &sheet("album.flac", &THREE))?;

    library.scan_sheets(&["album.cue"], &[]).await?;

    let path = library.spelled("album.flac");
    let spans: Vec<(i64, Option<i64>)> = library
        .db
        .read(move |conn| {
            sql::fetch_all(
                conn,
                "SELECT cue_start, cue_end FROM tracks WHERE file_path = ? ORDER BY cue_track",
                (path,),
            )
        })
        .await?;
    assert_eq!(spans, [(0, Some(13_500)), (13_500, Some(31_500)), (31_500, None)]);
    Ok(())
}

#[tokio::test]
async fn a_split_image_s_playlist_places_go_to_its_tracks_in_sheet_order() -> Result<(), AppError> {
    let library = Library::new().await?;
    for file in ["before.flac", "album.flac", "after.flac"] {
        library.write(file, "")?;
    }
    let ids = library.scan_whole(&["before.flac", "album.flac", "after.flac"]).await?;
    let playlist = library.playlist(&ids).await?;
    library.write("album.cue", &sheet("album.flac", &THREE))?;

    library.scan_sheets(&["album.cue"], &[]).await?;

    let tracks = library.ids("album.flac").await?;
    let expected = [&ids[..1], &tracks, &ids[2..]].concat();
    assert_eq!(library.playlist_items(playlist).await?, expected);
    Ok(())
}

#[tokio::test]
async fn a_reread_sheet_updates_each_track_in_place() -> Result<(), AppError> {
    let library = Library::new().await?;
    library.write("album.flac", "")?;
    library.write("album.cue", &sheet("album.flac", &THREE))?;
    library.scan_sheets(&["album.cue"], &[]).await?;
    let before = library.ids("album.flac").await?;
    let edited =
        sheet("album.flac", &["00:00:00", "03:30:00", "07:00:00"]).replace("Track 2", "Renamed");
    library.write("album.cue", &edited)?;

    library.scan_sheets(&["album.cue"], &[]).await?;

    let after = library.rows("album.flac").await?;
    let expected: Vec<Row> = vec![
        (Some(1), before[0], "Track 1".to_owned()),
        (Some(2), before[1], "Renamed".to_owned()),
        (Some(3), before[2], "Track 3".to_owned()),
    ];
    assert_eq!(after, expected);
    Ok(())
}

#[tokio::test]
async fn a_track_the_sheet_no_longer_numbers_goes_and_the_rest_stay() -> Result<(), AppError> {
    let library = Library::new().await?;
    library.write("album.flac", "")?;
    library.write("album.cue", &sheet("album.flac", &THREE))?;
    library.scan_sheets(&["album.cue"], &[]).await?;
    let before = library.ids("album.flac").await?;
    library.write("album.cue", &sheet("album.flac", &THREE[..2]))?;

    library.scan_sheets(&["album.cue"], &[]).await?;

    assert_eq!(library.ids("album.flac").await?, before[..2]);
    Ok(())
}

#[tokio::test]
async fn a_gone_sheet_gives_the_image_back_where_its_first_track_sat() -> Result<(), AppError> {
    let library = Library::new().await?;
    for file in ["before.flac", "album.flac", "after.flac"] {
        library.write(file, "")?;
    }
    library.write("album.cue", &sheet("album.flac", &THREE))?;
    let ids = library.scan_whole(&["before.flac", "after.flac"]).await?;
    library.scan_sheets(&["album.cue"], &[]).await?;
    let tracks = library.ids("album.flac").await?;
    let playlist = library.playlist(&[ids[0], tracks[0], tracks[1], tracks[2], ids[1]]).await?;
    fs::remove_file(library.path("album.cue"))?;

    library.scan_sheets(&[], &["album.cue"]).await?;

    let rows = library.rows("album.flac").await?;
    let whole: Vec<i64> =
        rows.iter().filter(|(track, _, _)| track.is_none()).map(|(_, id, _)| *id).collect();
    assert_eq!(rows.len(), 1, "only the whole-file row is left: {rows:?}");
    assert_eq!(library.playlist_items(playlist).await?, [&ids[..1], &whole, &ids[1..]].concat());
    Ok(())
}

/// A removed duplicate stays removed whatever sheet names it.
#[tokio::test]
async fn a_kept_out_image_gets_no_rows() -> Result<(), AppError> {
    let library = Library::new().await?;
    library.write("album.flac", "")?;
    library.write("album.cue", &sheet("album.flac", &THREE))?;
    let image = Library::read(library.path("album.flac"));
    keep_out(&library.db, &library.spelled("album.flac"), &image.metadata.file_hash).await?;

    library.scan_sheets(&["album.cue"], &[]).await?;

    assert_eq!(library.rows("album.flac").await?, Vec::<Row>::new());
    Ok(())
}

async fn sheet_stamp(library: &Library, name: &str) -> Result<Option<i64>, AppError> {
    let path = library.spelled(name);
    library
        .db
        .read(move |conn| {
            sql::fetch_one(conn, "SELECT file_size FROM cue_sheets WHERE path = ?", (path,))
        })
        .await
}

#[tokio::test]
async fn a_sheet_that_splits_its_images_is_stamped() -> Result<(), AppError> {
    let library = Library::new().await?;
    library.write("album.flac", "")?;
    let text = sheet("album.flac", &THREE);
    library.write("album.cue", &text)?;

    library.scan_sheets(&["album.cue"], &[]).await?;

    assert_eq!(sheet_stamp(&library, "album.cue").await?, i64::try_from(text.len()).ok());
    Ok(())
}

/// Only track 1 fits inside the image.
#[tokio::test]
async fn an_image_too_short_to_split_keeps_its_row_and_leaves_the_sheet_unstamped()
-> Result<(), AppError> {
    let library = Library::new().await?;
    library.write("album.flac", "")?;
    let whole = library.scan_whole(&["album.flac"]).await?;
    library.write("album.cue", &sheet("album.flac", &["00:00:00", "12:00:00"]))?;

    library.scan_sheets(&["album.cue"], &[]).await?;

    assert_eq!(library.ids("album.flac").await?, whole);
    assert_eq!(sheet_stamp(&library, "album.cue").await?, None);
    Ok(())
}
