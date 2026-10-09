//! `Updater.install()` — download + atomic-swap install backend.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::{ComponentHandle, Weak};
use tokio::sync::watch;

use melodia_app::services::updater::{
    self, Checked, FailureKind, UpdaterEvent, Verdict, asset_cache, check_for_update,
};
use melodia_app::state::AppState;
use melodia_core::error::{AppResult, describe};
use melodia_platform::services::platform::install_kind::install_target;
use melodia_ui::{AppWindow, MelodiaUpdater};

use super::paint::{paint_restart_needed, report_failure, set_is_installing};

/// True iff a `download_and_install` future is currently in flight on
/// this process. The Slint UI gates the Install button via
/// `is-installing`, but a programmatic double-invoke (toast tap +
/// Settings button in the same tick, or an event subscriber re-firing)
/// could double-spawn. Held through [`InstallGuard`]. Same pattern as
/// `ui::window_chrome::RESPAWN_AFTER_EXIT`.
static INSTALL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// [`INSTALL_IN_PROGRESS`], released on drop in every exit path of the
/// install future (early-return, error, success) — and a panic still cleans
/// up.
struct InstallGuard;

impl InstallGuard {
    /// `None` while another install is in flight. CAS-acquire prevents a
    /// double-spawn from racing two downloads against the same `.new`
    /// sibling path — both would `File::create(dest)` and clobber each
    /// other's bytes.
    fn acquire() -> Option<Self> {
        INSTALL_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(Self)
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALL_IN_PROGRESS.store(false, Ordering::Release);
    }
}

pub(super) fn spawn_install(
    state: AppState,
    weak: Weak<AppWindow>,
    event_tx: watch::Sender<Option<UpdaterEvent>>,
) {
    let Some(guard) = InstallGuard::acquire() else {
        log::info!("updater: install already in progress; ignoring re-trigger");
        return;
    };
    set_is_installing(&weak, true);
    let runtime = state.runtime.clone();
    runtime.spawn(async move {
        let _guard = guard;

        // Capture the install target *before* `download_and_install`
        // swaps the binary on disk. The atomic swap *renames* the running
        // binary to `<target>.old`, and nothing recovers that after the
        // fact — the OS reports the stale path with a straight face — so
        // the post-exit respawn in `shutdown::respawn_if_requested` must
        // use this captured path. See `ui::window_chrome::set_respawn_exe`
        // for why the unlinking installs need no such capture.
        let install_target = install_target();

        let Some(cached) = resolve_asset(&state, &weak, &event_tx).await else { return };

        // The version threads into `verify_stream` for the trusted-comment
        // cross-check: signature must carry `version=<cached.version>`.
        // Pinning to the *observed* version (not whatever the install-time
        // re-fetch happens to show) means a manifest that flipped to a
        // different release between Available-toast and Install-click is
        // caught by the signature mismatch, not silently installed.
        match updater::download_and_install(
            state.http_client(),
            &cached.asset,
            &cached.version,
            progress_painter(weak.clone()),
        )
        .await
        {
            Ok(()) => finish_installed(&weak, &event_tx, &install_target),
            Err(e) => {
                let kind = FailureKind::classify(&e);
                log::warn!("updater: install failed ({kind:?}): {}", describe(&e));
                abort_install(&weak, &event_tx, format!("{e}"), kind);
            }
        }
    });
}

/// The asset to download, or `None` once the reason it can't be has been
/// reported.
///
/// Happy path: re-read the manifest and use the fresh asset — picks up any
/// URL/signature changes between check and install. Sad path: re-fetch fails
/// (offline, captive portal), fall back to the asset cached at last
/// `Available` observation. The signature check downstream catches any drift
/// between cache and on-disk artifact, so the fallback can't compromise
/// safety.
async fn resolve_asset(
    state: &AppState,
    weak: &Weak<AppWindow>,
    event_tx: &watch::Sender<Option<UpdaterEvent>>,
) -> Option<asset_cache::CachedAsset> {
    // A full read rather than a revalidation: a `304` carries no
    // asset, and the manifest is a few KB against the download it
    // precedes.
    let outcome =
        check_for_update(state.http_client(), updater::RELEASES_BASE, env!("CARGO_PKG_VERSION"))
            .await;
    match outcome {
        Ok(Checked { manifest, verdict: Verdict::Available(asset), .. }) => {
            asset_cache::store(manifest.version.clone(), asset.clone());
            Some(asset_cache::CachedAsset { version: manifest.version, asset })
        }
        Ok(Checked { verdict: Verdict::UpToDate, .. }) => {
            set_is_installing(weak, false);
            log::warn!(
                "updater: install clicked but server now reports up-to-date \
                 (manifest changed between check and install)"
            );
            None
        }
        Ok(Checked { manifest, verdict: Verdict::UnsupportedSchema, .. }) => {
            // Server bumped the manifest schema between the user's
            // last Available observation and the Install click.
            // Same outcome as UpToDate from the install path's POV
            // — can't proceed, surface a short error so they know
            // why the click didn't act.
            let reason = format!(
                "manifest schema {} is newer than this binary supports",
                manifest.manifest_schema_version
            );
            log::warn!("updater: install rejected — {reason}");
            abort_install(weak, event_tx, reason, FailureKind::Other);
            None
        }
        Ok(Checked { verdict: Verdict::NoAssetForTarget, .. }) => {
            let reason = "no installable asset for this platform".to_owned();
            log::warn!("updater: install rejected — {reason}");
            abort_install(weak, event_tx, reason, FailureKind::Other);
            None
        }
        Err(e) => {
            // Re-fetch failed (network / DNS / TLS / server 5xx).
            // Fall back to the cached asset if any — the user
            // already saw an Available toast, so they expect this
            // click to act on whatever they were told about.
            let kind = FailureKind::classify(&e);
            let Some(cached) = asset_cache::snapshot() else {
                log::warn!(
                    "updater: re-fetch before install failed ({kind:?}) and \
                     no cached asset available: {}",
                    describe(&e)
                );
                abort_install(weak, event_tx, format!("re-fetch before install failed: {e}"), kind);
                return None;
            };
            log::warn!(
                "updater: re-fetch before install failed ({kind:?}); \
                 falling back to cached asset: {}",
                describe(&e)
            );
            Some(cached)
        }
    }
}

fn progress_painter(weak: Weak<AppWindow>) -> impl Fn(u8) + Send + Sync {
    move |pct: u8| {
        let pct_i = i32::from(pct);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.global::<MelodiaUpdater>().set_download_progress(pct_i);
        });
    }
}

fn finish_installed(
    weak: &Weak<AppWindow>,
    event_tx: &watch::Sender<Option<UpdaterEvent>>,
    install_target: &AppResult<PathBuf>,
) {
    asset_cache::clear();
    // Record the pre-swap binary path so the "Restart Now"
    // respawn relaunches the freshly-installed binary, not
    // the pre-swap path the OS still reports after a rename.
    match install_target {
        Ok(target) => {
            crate::ui::window_chrome::set_respawn_exe(target.clone());
        }
        Err(e) => log::warn!(
            "updater: install_target lookup failed; \
             restart may relaunch the wrong binary: {}",
            describe(&e)
        ),
    }
    paint_restart_needed(weak);
    let _ = event_tx.send(Some(UpdaterEvent::Installed));
}

/// Hand the Install button back and report why.
fn abort_install(
    weak: &Weak<AppWindow>,
    event_tx: &watch::Sender<Option<UpdaterEvent>>,
    reason: String,
    kind: FailureKind,
) {
    set_is_installing(weak, false);
    report_failure(weak, event_tx, reason, kind);
}
