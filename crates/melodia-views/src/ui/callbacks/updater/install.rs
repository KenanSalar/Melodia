//! `Updater.install()` — download, verify and install backend.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::{ComponentHandle, Weak};
use tokio::sync::watch;

use melodia_app::services::updater::{
    self, Checked, FailureKind, InstallOutcome, UpdaterEvent, Verdict, asset_cache,
    check_for_update,
};
use melodia_app::state::AppState;
use melodia_core::error::{AppResult, describe};
use melodia_platform::services::platform::install_kind::install_target;
use melodia_ui::{AppWindow, MelodiaUpdater};

use super::paint::{
    hand_install_back, paint_install_started, paint_restart_needed, report_failure,
};

/// Whether a `download_and_install` is in flight. The UI gates the Install button on
/// `is-installing`, but a toast tap and the Settings button in one tick could still
/// double-spawn. Held through [`InstallGuard`].
static INSTALL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// [`INSTALL_IN_PROGRESS`], released on drop on every exit, a panic included.
struct InstallGuard;

impl InstallGuard {
    /// `None` while another install is in flight, whose download would clobber the same `.new`
    /// sibling.
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
    paint_install_started(&weak);
    let runtime = state.runtime.clone();
    runtime.spawn(async move {
        let _guard = guard;

        // Captured *before* `download_and_install` renames the running binary to
        // `<target>.old`, after which the OS reports the stale path; the respawn in
        // `shutdown::respawn_if_requested` uses this one. `window_chrome::set_respawn_exe`
        // says why the unlinking installs need no capture.
        let install_target = install_target();

        let Some(cached) = resolve_asset(&state, &weak, &event_tx).await else { return };

        // The signature must carry `version=<cached.version>`, so a manifest that flipped
        // release between the Available toast and the click fails rather than installs.
        match updater::download_and_install(
            state.http_client(),
            &cached.asset,
            &cached.version,
            progress_painter(weak.clone()),
        )
        .await
        {
            Ok(InstallOutcome::Installed) => finish_installed(&weak, &event_tx, &install_target),
            // The user's own answer to the elevation prompt, so it earns no toast: the panel
            // simply offers the same version again.
            Ok(InstallOutcome::Cancelled) => hand_install_back(&weak),
            Err(e) => {
                log::warn!("updater: install failed ({:?}): {}", e.kind, describe(&e));
                abort_install(&weak, &event_tx, describe(&e), e.kind);
            }
        }
    });
}

/// The asset to download, or `None` once the reason it can't be is reported. A fresh manifest
/// read picks up any URL or signature change since the check; where it fails, the asset cached at
/// the last `Available` stands in, the downstream signature check catching any drift.
async fn resolve_asset(
    state: &AppState,
    weak: &Weak<AppWindow>,
    event_tx: &watch::Sender<Option<UpdaterEvent>>,
) -> Option<asset_cache::CachedAsset> {
    // A full read rather than a revalidation: a `304` carries no
    // asset, and the manifest is a few KB against the download it
    // precedes.
    let outcome = check_for_update(
        state.http_client(),
        updater::RELEASES_BASE,
        &updater::installed_version(),
    )
    .await;
    match outcome {
        Ok(Checked { manifest, verdict: Verdict::Available(asset), .. }) => {
            asset_cache::store(manifest.version.clone(), asset.clone());
            Some(asset_cache::CachedAsset { version: manifest.version, asset })
        }
        Ok(Checked { verdict: Verdict::UpToDate, .. }) => {
            hand_install_back(weak);
            log::warn!(
                "updater: install clicked but server now reports up-to-date \
                 (manifest changed between check and install)"
            );
            None
        }
        Ok(Checked { manifest, verdict: Verdict::UnsupportedSchema, .. }) => {
            // The schema moved between the last Available and the click.
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
            // The user saw an Available toast, so the click acts on what it told them about.
            let Some(cached) = asset_cache::snapshot() else {
                log::warn!(
                    "updater: re-fetch before install failed ({:?}) and \
                     no cached asset available: {}",
                    e.kind,
                    describe(&e)
                );
                let reason = format!("re-fetch before install failed: {}", describe(&e));
                abort_install(weak, event_tx, reason, e.kind);
                return None;
            };
            log::warn!(
                "updater: re-fetch before install failed ({:?}); \
                 falling back to cached asset: {}",
                e.kind,
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
    hand_install_back(weak);
    report_failure(weak, event_tx, reason, kind);
}
