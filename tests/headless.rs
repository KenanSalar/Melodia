//! End-to-end smoke test: boot the full backend in a tempdir, ingest a
//! fixture audio file via the library API, and assert the row lands in the DB.
//!
//! Notes:
//! - Roots `Paths` in the tempdir through `Paths::rooted_at`. Redirecting
//!   `dirs::data_dir()` with `XDG_DATA_HOME` would also cover that lookup and
//!   the `Melodia` join `resolve` adds on top, at the price of a process-global
//!   mutation this binary would need `unsafe` for.
//! - `AppState::init` opens the default audio device (rodio); machines without
//!   audio will fail here. CI points ALSA's default PCM at the userspace `null`
//!   device (see the `test` job in `.github/workflows/pr-validation.yml`), so
//!   this runs headless there too.

use std::path::PathBuf;

use melodia::config::Paths;
use melodia::error::AppError;
use melodia::library;
use melodia::state::AppState;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_scan_persists_track() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;

    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    let runtime = tokio::runtime::Handle::current();
    let (state, _channels) = AppState::init(paths, runtime).await?;

    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    assert!(
        fixtures.join("silence-1s.flac").exists(),
        "fixture missing — regenerate with ffmpeg (see § J in BACKEND_PORT.md)"
    );

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
