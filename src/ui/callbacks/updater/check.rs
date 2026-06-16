//! `Updater.check()` — manual "Check for Updates" backend.

use chrono::Utc;
use slint::Weak;
use tokio::sync::watch;

use crate::AppWindow;
use crate::library;
use crate::services::updater::{
    CheckOutcome, FailureKind, UpdaterEvent, asset_cache, check_for_update, version::is_upgrade,
};
use crate::state::AppState;

use super::paint::{paint_available, paint_error, paint_up_to_date, set_is_checking};
use super::{current_skipped_release, read_etag};

pub(super) fn spawn_manual_check(
    state: AppState,
    weak: Weak<AppWindow>,
    event_tx: watch::Sender<Option<UpdaterEvent>>,
) {
    set_is_checking(&weak, true);
    let runtime = state.runtime.clone();
    runtime.spawn(async move {
        let etag = read_etag(&state);
        // `force_refresh = true`: the user explicitly asked for a check,
        // so bypass the cached ETag. Clears a sticky `UnsupportedSchema`
        // state if the maintainer re-uploaded `latest.json` to fix a
        // mis-bumped `manifest_schema_version` — without this, the 304
        // short-circuit would keep returning the cached "schema too new"
        // outcome until the next manifest publish bumped the ETag.
        let result = check_for_update(
            state.http_client(),
            etag.as_deref(),
            env!("CARGO_PKG_VERSION"),
            true,
        )
        .await;
        set_is_checking(&weak, false);

        let now = Utc::now();
        match result {
            Ok(CheckOutcome::NotModified) => {
                let _ = library::settings::updates::record_check_success(&state, now, None, etag);
            }
            Ok(CheckOutcome::UpToDate) => {
                paint_up_to_date(&weak);
                let _ = library::settings::updates::record_check_success(&state, now, None, etag);
            }
            Ok(CheckOutcome::NoAssetForTarget { etag: server_etag }) => {
                let _ = library::settings::updates::record_check_success(
                    &state,
                    now,
                    None,
                    server_etag.or(etag),
                );
            }
            Ok(CheckOutcome::UnsupportedSchema { schema, etag: server_etag }) => {
                // Schema gate already logged at warn level inside
                // `services::updater::check`.
                // Settings panel stays on whatever it was painted with last;
                // manual check doesn't surface a toast either — the user
                // initiated the check and the in-app updater simply can't
                // act on the response.
                log::info!("updater: manual check returned unsupported schema {schema}");
                let _ = library::settings::updates::record_check_success(
                    &state,
                    now,
                    None,
                    server_etag.or(etag),
                );
            }
            Ok(CheckOutcome::Available { manifest, asset, etag: server_etag }) => {
                let skipped = current_skipped_release(&state);
                let critical = manifest.critical;
                let skip_still_active = !skipped.is_empty()
                    && matches!(is_upgrade(&skipped, &manifest.version), Ok(false));

                // Cache the asset so a later `Install` click can use
                // it even if the re-fetch fails (offline / flaky net).
                asset_cache::store(manifest.version.clone(), asset);
                paint_available(
                    &weak,
                    manifest.version.clone(),
                    manifest.notes_short.clone(),
                    critical,
                );
                let _ = library::settings::updates::record_check_success(
                    &state,
                    now,
                    Some(manifest.version.clone()),
                    server_etag.or(etag),
                );

                // Critical releases bypass the skip filter — see the same
                // comment on the daily-task path in
                // `tasks::updater_daily::handle_outcome`.
                if critical || !skip_still_active {
                    let _ = event_tx.send(Some(UpdaterEvent::Available {
                        version: manifest.version,
                        notes_short: manifest.notes_short,
                        critical,
                    }));
                }
            }
            Err(e) => {
                let kind = FailureKind::classify(&e);
                log::warn!("updater: manual check failed ({kind:?}): {e}");
                paint_error(&weak, format!("{e}"));
                let _ = library::settings::updates::record_check_failure(&state, now);
                let _ = event_tx.send(Some(UpdaterEvent::Failed { kind }));
            }
        }
    });
}
