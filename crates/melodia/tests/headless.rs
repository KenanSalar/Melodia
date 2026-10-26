//! End-to-end scans: boot the full backend in a tempdir, scan a folder through the library API,
//! and assert on what lands in the database and on what the lists are told.
//!
//! Notes:
//! - Roots `Paths` in the tempdir through `Paths::rooted_at`, which derives every
//!   path from a root it is handed. What that leaves uncovered is `resolve`'s
//!   choice of root, and steering that means `MELODIA_DATA_DIR` or
//!   `XDG_DATA_HOME`, a process-global mutation this binary would need `unsafe`
//!   for. `crates/melodia-core/src/tests/config_tests.rs` drives the choice
//!   under the env lock instead.
//! - `AppState::init` opens the default audio device; machines without
//!   audio will fail here. The `test` job points ALSA's default PCM at the
//!   userspace `null` device, so this runs headless there; `test-windows` skips
//!   every test here by the `headless_scan` prefix instead, WASAPI having no
//!   equivalent short of a signed virtual audio driver. Both jobs are in
//!   `.github/workflows/pr-validation.yml`.
//! - A completed scan looks up its artists' images, so the client goes through a proxy that
//!   refuses every request: nothing here reaches the network, and no image lands to bump the
//!   signal the scan tests read.

use std::path::{Path, PathBuf};

use reqwest::Proxy;

use melodia_app::library;
use melodia_app::library::scan::{OnStop, ScanOutcome};
use melodia_app::state::{AppState, StartupChannels};
use melodia_core::config::Paths;
use melodia_core::entities::track_list::{SortDir, TrackListScope, TrackOrder};
use melodia_core::error::AppError;
use melodia_core::utils::needle::fold_needle;
use melodia_testkit::http::{TestResponse, TestServer};

/// The backend, and what has to live as long as it does.
struct Booted {
    state: AppState,
    _channels: StartupChannels,
    _refusing_proxy: TestServer,
}

async fn boot(data_root: &Path) -> Result<Booted, AppError> {
    let paths = Paths::rooted_at(data_root.to_path_buf());
    paths.create_dirs()?;
    let (state, channels) = AppState::init(paths, tokio::runtime::Handle::current()).await?;

    let proxy = TestServer::start(|_| TestResponse::status(502))?;
    let client = reqwest::Client::builder()
        .proxy(Proxy::all(proxy.base_url()).map_err(|e| AppError::network("proxy", e))?)
        .build()
        .map_err(|e| AppError::network("client", e))?;
    if state.http_client_cell().set(client).is_err() {
        return Err(AppError::validation_msg("the client was built before it could be routed"));
    }
    Ok(Booted { state, _channels: channels, _refusing_proxy: proxy })
}

fn repo_file(parts: &[&str]) -> PathBuf {
    parts.iter().fold(PathBuf::from(env!("MELODIA_REPO_ROOT")), |path, part| path.join(part))
}

/// A library folder holding two different files, scanned once.
struct Library {
    booted: Booted,
    dir: PathBuf,
    folder_id: i64,
    _tmp: tempfile::TempDir,
}

impl Library {
    async fn scanned() -> Result<Self, AppError> {
        let tmp = tempfile::tempdir()?;
        let dir = tmp.path().join("library");
        std::fs::create_dir(&dir)?;
        std::fs::copy(
            repo_file(&["crates", "melodia", "tests", "fixtures", "silence-1s.flac"]),
            dir.join("a.flac"),
        )?;
        std::fs::copy(repo_file(&["test-assets", "silence.wav"]), dir.join("b.wav"))?;
        let booted = boot(&tmp.path().join("data")).await?;
        let folder =
            library::settings::add_folder(&booted.state, dir.to_string_lossy().into_owned())
                .await?;

        let first = library::scan::scan_folder(&booted.state, folder.id, OnStop::Keep).await?;

        assert_eq!(first, ScanOutcome::Completed { inserted: 2 }, "the first scan of the two");
        Ok(Self { booted, dir, folder_id: folder.id, _tmp: tmp })
    }

    fn state(&self) -> &AppState {
        &self.booted.state
    }

    async fn rescan(&self) -> Result<ScanOutcome, AppError> {
        library::scan::scan_folder(self.state(), self.folder_id, OnStop::Keep).await
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).to_string_lossy().into_owned()
    }

    async fn track_count(&self) -> Result<i64, AppError> {
        let count = sqlx::query_scalar("SELECT COUNT(*) FROM tracks")
            .fetch_one(self.state().db.read())
            .await?;
        Ok(count)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_scan_persists_track() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let booted = boot(tmp.path()).await?;
    let state = &booted.state;

    // Its own directory rather than the shared corpus, and that is the assertion below:
    // this scans a folder and counts what lands, so the fixture set has to be exactly one.
    let fixtures = repo_file(&["crates", "melodia", "tests", "fixtures"]);
    assert!(fixtures.join("silence-1s.flac").exists(), "fixture missing — regenerate with ffmpeg");

    let folder =
        library::settings::add_folder(state, fixtures.to_string_lossy().into_owned()).await?;

    let outcome = library::scan::scan_folder(state, folder.id, OnStop::Keep).await?;
    assert_eq!(
        outcome,
        ScanOutcome::Completed { inserted: 1 },
        "expected exactly the silence fixture to ingest"
    );

    // Read the way the Songs tab reads it: the library's keyset, then its rows by id.
    let order = TrackOrder::new("title", SortDir::Asc);
    let ids =
        library::keyset::track_ids(&state.db, TrackListScope::Library, order, &fold_needle(""))
            .await?;
    let tracks = library::keyset::rows_by_ids(&state.db, &ids).await?;
    assert_eq!(tracks.len(), 1, "exactly one track should be in DB");

    let row = &tracks[0];
    assert_eq!(row.title, "Silence Fixture");
    assert_eq!(row.artist.as_deref(), Some("Test Artist"));
    assert_eq!(row.album.as_deref(), Some("Test Album"));
    Ok(())
}

/// Every list showing the library refetches on a bump, so a scan that only removed a track has to
/// send one or the deleted track stays listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_scan_bumps_the_lists_when_it_only_drops_a_deleted_file() -> Result<(), AppError> {
    let library = Library::scanned().await?;
    std::fs::remove_file(library.dir.join("b.wav"))?;
    let changed = library.state().library_changed.subscribe();

    library.rescan().await?;

    assert_eq!(changed.has_changed().ok(), Some(true));
    Ok(())
}

/// A launch scans every folder, and a bump makes every list refetch the whole library, so a scan
/// that changed nothing sends none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_scan_that_changes_nothing_bumps_no_list() -> Result<(), AppError> {
    let library = Library::scanned().await?;
    let changed = library.state().library_changed.subscribe();

    let outcome = library.rescan().await?;

    assert_eq!(
        (outcome, changed.has_changed().ok()),
        (ScanOutcome::Completed { inserted: 0 }, Some(false)),
        "(outcome, bumped)"
    );
    Ok(())
}

/// A file renamed while Melodia was closed arrives as a new path and an orphan, and the import
/// re-points the row before the purge, so the play count stays with the track.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_scan_keeps_a_moved_files_play_count() -> Result<(), AppError> {
    let library = Library::scanned().await?;
    sqlx::query("UPDATE tracks SET play_count = 2 WHERE file_path = ?")
        .bind(library.path("a.flac"))
        .execute(library.state().db.write())
        .await?;
    std::fs::rename(library.dir.join("a.flac"), library.dir.join("renamed.flac"))?;

    library.rescan().await?;

    let play_count: Option<i64> =
        sqlx::query_scalar("SELECT play_count FROM tracks WHERE file_path = ?")
            .bind(library.path("renamed.flac"))
            .fetch_optional(library.state().db.read())
            .await?;
    assert_eq!(play_count, Some(2));
    Ok(())
}

/// A folder that walks empty is as likely an unmounted drive as an emptied one, and purging it
/// would take every rating and play count in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_scan_keeps_every_row_when_the_walk_finds_nothing() -> Result<(), AppError> {
    let library = Library::scanned().await?;
    std::fs::remove_file(library.dir.join("a.flac"))?;
    std::fs::remove_file(library.dir.join("b.wav"))?;

    library.rescan().await?;

    assert_eq!(library.track_count().await?, 2);
    Ok(())
}
