//! End-to-end smoke test: boot the full backend in a tempdir, ingest a
//! fixture audio file via the library API, and assert the row lands in the DB.
//!
//! Notes:
//! - Roots `Paths` in the tempdir through `Paths::rooted_at`, which derives every
//!   path from a root it is handed. What that leaves uncovered is `resolve`'s
//!   choice of root, and steering that means `MELODIA_DATA_DIR` or
//!   `XDG_DATA_HOME`, a process-global mutation this binary would need `unsafe`
//!   for. `src/tests/config_tests.rs` drives the choice under the env lock
//!   instead.
//! - `AppState::init` opens the default audio device; machines without
//!   audio will fail here. The `test` job points ALSA's default PCM at the
//!   userspace `null` device, so this runs headless there; `test-windows` skips
//!   it by name instead, WASAPI having no equivalent short of a signed virtual
//!   audio driver. Both jobs are in `.github/workflows/pr-validation.yml`.

use std::path::PathBuf;

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_scan_persists_track() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;

    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    let runtime = tokio::runtime::Handle::current();
    let (state, _channels) = AppState::init(paths, runtime).await?;

    // Its own directory rather than the shared corpus, and that is the assertion below:
    // this scans a folder and counts what lands, so the fixture set has to be exactly one.
    let fixtures = PathBuf::from(env!("MELODIA_REPO_ROOT")).join("crates/melodia/tests/fixtures");
    assert!(fixtures.join("silence-1s.flac").exists(), "fixture missing — regenerate with ffmpeg");

    let folder =
        library::settings::add_folder(&state, fixtures.to_string_lossy().into_owned()).await?;

    let scanned = library::settings::scan_folder(&state, folder.id).await?;
    assert_eq!(scanned, 1, "expected exactly the silence fixture to ingest");

    let tracks = library::tracks::get_tracks(&state).await?;
    assert_eq!(tracks.len(), 1, "exactly one track should be in DB");

    let row = &tracks[0];
    assert_eq!(row.title, "Silence Fixture");
    assert_eq!(row.artist.as_deref(), Some("Test Artist"));
    assert_eq!(row.album.as_deref(), Some("Test Album"));
    Ok(())
}
