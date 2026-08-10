//! `Updater.install()` — download + atomic-swap install backend.

use std::sync::atomic::{AtomicBool, Ordering};

use slint::{ComponentHandle, Weak};
use tokio::sync::watch;

use crate::services::updater::{
    self, CheckOutcome, FailureKind, UpdaterEvent, asset_cache, check_for_update,
};
use crate::state::AppState;
use crate::{AppWindow, MelodiaUpdater};

use super::paint::{paint_error, paint_restart_needed, set_is_installing};
use super::read_etag;

/// True iff a `download_and_install` future is currently in flight on
/// this process. The Slint UI gates the Install button via
/// `is-installing`, but a programmatic double-invoke (toast tap +
/// Settings button in the same tick, or an event subscriber re-firing)
/// could double-spawn. Acquired with CAS in [`spawn_install`]; released
/// in every exit path (success, error, early-return). Same pattern as
/// `ui::window_chrome::RESPAWN_AFTER_EXIT`.
static INSTALL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

pub(super) fn spawn_install(
    state: AppState,
    weak: Weak<AppWindow>,
    event_tx: watch::Sender<Option<UpdaterEvent>>,
) {
    // Concurrency guard. CAS-acquire prevents a double-spawn from
    // racing two downloads against the same `.new` sibling path —
    // both would `File::create(dest)` and clobber each other's bytes.
    // Released in every exit path of the spawned future.
    if INSTALL_IN_PROGRESS
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        log::info!("updater: install already in progress; ignoring re-trigger");
        return;
    }

    // Resolve the asset to download. Happy path: re-fetch the
    // manifest (usually 304 thanks to the cached ETag) and use the
    // fresh asset — picks up any URL/signature changes between check
    // and install. Sad path: re-fetch fails (offline, captive portal),
    // fall back to the asset cached at last `Available` observation.
    // The signature check downstream catches any drift between cache
    // and on-disk artifact, so the fallback can't compromise safety.
    set_is_installing(&weak, true);
    let runtime = state.runtime.clone();
    runtime.spawn(async move {
        // RAII release of the in-progress flag in every exit path
        // (early-return, error, success) — saves threading manual
        // `INSTALL_IN_PROGRESS.store(false, …)` calls through ten
        // branches and lets future panics still clean up.
        struct InstallGuard;
        impl Drop for InstallGuard {
            fn drop(&mut self) {
                INSTALL_IN_PROGRESS.store(false, Ordering::Release);
            }
        }
        let _install_guard = InstallGuard;

        // Capture the install target *before* `download_and_install`
        // swaps the binary on disk. The atomic swap *renames* the running
        // binary to `<target>.old`, and nothing recovers that after the
        // fact — the OS reports the stale path with a straight face — so
        // the post-exit respawn in `shutdown::respawn_if_requested` must
        // use this captured path. See `ui::window_chrome::set_respawn_exe`
        // for why the unlinking installs need no such capture.
        let install_target = updater::install_target();

        let etag = read_etag(&state);
        // `force_refresh = false`: this re-fetch is the install path's
        // best-effort freshness check (the asset URL/signature could
        // have rotated between the user clicking "Check" and clicking
        // "Install"). A 304 here is the happy case — we fall through to
        // the cached asset, which already passed verification.
        let outcome = check_for_update(
            state.http_client(),
            etag.as_deref(),
            env!("CARGO_PKG_VERSION"),
            false,
        )
        .await;
        let cached = match outcome {
            Ok(CheckOutcome::Available { manifest, asset, .. }) => {
                asset_cache::store(manifest.version.clone(), asset.clone());
                asset_cache::CachedAsset { version: manifest.version, asset }
            }
            Ok(CheckOutcome::NotModified) => {
                // 304 — no fresh asset blob in the response. Use
                // whatever's cached from the last successful check.
                let Some(cached) = asset_cache::snapshot() else {
                    set_is_installing(&weak, false);
                    log::warn!(
                        "updater: install clicked but no cached asset \
                         and server returned 304 (stale UI state)"
                    );
                    return;
                };
                cached
            }
            Ok(CheckOutcome::UpToDate) => {
                set_is_installing(&weak, false);
                log::warn!(
                    "updater: install clicked but server now reports up-to-date \
                     (manifest changed between check and install)"
                );
                return;
            }
            Ok(CheckOutcome::UnsupportedSchema { schema, .. }) => {
                // Server bumped the manifest schema between the user's
                // last Available observation and the Install click.
                // Same outcome as UpToDate from the install path's POV
                // — can't proceed, surface a short error so they know
                // why the click didn't act.
                set_is_installing(&weak, false);
                let reason = format!(
                    "manifest schema {schema} is newer than this binary supports"
                );
                log::warn!("updater: install rejected — {reason}");
                paint_error(&weak, reason);
                let _ = event_tx.send(Some(UpdaterEvent::Failed { kind: FailureKind::Other }));
                return;
            }
            Ok(CheckOutcome::NoAssetForTarget { .. }) => {
                set_is_installing(&weak, false);
                let reason = "no installable asset for this platform".to_owned();
                log::warn!("updater: install rejected — {reason}");
                paint_error(&weak, reason);
                let _ = event_tx.send(Some(UpdaterEvent::Failed { kind: FailureKind::Other }));
                return;
            }
            Err(e) => {
                // Re-fetch failed (network / DNS / TLS / server 5xx).
                // Fall back to the cached asset if any — the user
                // already saw an Available toast, so they expect this
                // click to act on whatever they were told about.
                let kind = FailureKind::classify(&e);
                let Some(cached) = asset_cache::snapshot() else {
                    set_is_installing(&weak, false);
                    log::warn!(
                        "updater: re-fetch before install failed ({kind:?}) and \
                         no cached asset available: {e}"
                    );
                    paint_error(&weak, format!("re-fetch before install failed: {e}"));
                    let _ = event_tx.send(Some(UpdaterEvent::Failed { kind }));
                    return;
                };
                log::warn!(
                    "updater: re-fetch before install failed ({kind:?}); \
                     falling back to cached asset: {e}"
                );
                cached
            }
        };

        let on_progress = {
            let weak = weak.clone();
            move |pct: u8| {
                let pct_i = i32::from(pct);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<MelodiaUpdater>().set_download_progress(pct_i);
                });
            }
        };

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
            on_progress,
        )
        .await
        {
            Ok(()) => {
                asset_cache::clear();
                // Record the pre-swap binary path so the "Restart Now"
                // respawn relaunches the freshly-installed binary, not
                // the pre-swap path the OS still reports after a rename.
                match &install_target {
                    Ok(target) => {
                        crate::ui::window_chrome::set_respawn_exe(target.clone());
                    }
                    Err(e) => log::warn!(
                        "updater: install_target lookup failed; \
                         restart may relaunch the wrong binary: {e}"
                    ),
                }
                paint_restart_needed(&weak);
                let _ = event_tx.send(Some(UpdaterEvent::Installed));
            }
            Err(e) => {
                set_is_installing(&weak, false);
                let kind = FailureKind::classify(&e);
                log::warn!("updater: install failed ({kind:?}): {e}");
                paint_error(&weak, format!("{e}"));
                let _ = event_tx.send(Some(UpdaterEvent::Failed { kind }));
            }
        }
    });
}
