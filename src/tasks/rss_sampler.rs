//! Opt-in periodic memory sampler for diagnostics.
//!
//! Enabled by setting `MELODIA_RSS_SAMPLE=1` (any non-empty value) before
//! launch. Logs the process's `/proc/self/status` resident-memory breakdown
//! plus the current view tag (which Nav section, whether a detail page is
//! open, whether Now Playing / the queue sheet are overlaid) every 500 ms so
//! leave/enter/scroll memory transitions can be correlated with user
//! actions in the log timeline.
//!
//! Off by default to avoid log noise; no-op on non-Linux (no `/proc`).
//! Cheap when enabled — one ~4 KiB pseudo-file read + a handful of Slint
//! property reads per tick. Runs on the UI thread via `slint::spawn_local`
//! so it can read the `Nav` / `AlbumDetail` / `ArtistDetail` / `GenreDetail`
//! globals without an atomic-shadow plumbing pass through `ui/*`.
//!
//! **Diagnostic exception to the `tasks/` no-`ui::*`-imports rule** — the
//! file imports generated boundary types (`AppWindow`, `Nav`, …) and
//! `ui::window_chrome::is_queue_sheet_open` so the view tag can include the
//! overlay state. Acceptable because the whole module is gated behind an
//! env var; production sessions never reach the import sites.
//!
//! Sample output (INFO level):
//!
//! ```text
//! [MEM view=Albums VmRSS=147924 RssAnon=98112 RssFile=46500 RssShmem=3312 VmData=120480 (KiB)]
//! [MEM view=AlbumDetail(42)+NP VmRSS=… …]
//! [MEM view=Tracks+QS VmRSS=… …]
//! ```
//!
//! Tags after the view name: `+NP` = Now Playing full-screen open;
//! `+QS` = queue-sheet open. The breakdown fields let you see *which*
//! metric your external monitor is tracking — system monitors often display
//! PSS or anon-only, which can move while total `VmRSS` stays flat.

#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use async_compat::Compat;
#[cfg(target_os = "linux")]
use slint::Weak;

// `tasks/` imports no `ui::*` — this module is the documented, env-gated exception,
// since a memory tag naming the view has to read the view's own globals. The tag
// itself is `ui::view_tag`, shared with the verbose log's navigation line.
#[cfg(target_os = "linux")]
use crate::AppWindow;
#[cfg(target_os = "linux")]
use crate::ui::view_tag::format_view;

#[cfg(target_os = "linux")]
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// Env-var gate so the sampler only runs in diagnostic sessions. Anything
/// non-empty enables (`MELODIA_RSS_SAMPLE=1`, `=on`, …); unset or empty
/// keeps the task entirely unspawned. Called once at startup with the
/// `AppWindow` weak handle so the per-tick loop can read Nav state on the
/// UI thread.
pub fn install(weak: &slint::Weak<crate::AppWindow>) {
    if std::env::var_os("MELODIA_RSS_SAMPLE").is_none_or(|v| v.is_empty()) {
        return;
    }

    #[cfg(target_os = "linux")]
    {
        log::info!(
            "[MEM] sampler enabled (interval {} ms)",
            SAMPLE_INTERVAL.as_millis()
        );
        let _ = slint::spawn_local(Compat::new(run(weak.clone())));
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = weak;
        log::info!("[MEM] sampler env set but no /proc on this platform — skipped");
    }
}

/// Per-tick loop: snapshot Slint nav state + `/proc/self/status`, format,
/// emit one INFO line. Exits cleanly when the UI window is dropped (weak
/// upgrade fails).
#[cfg(target_os = "linux")]
async fn run(weak: Weak<AppWindow>) {
    let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the immediate first tick — `tokio::time::interval` fires it
    // instantly, which would log before the boot pre-fetch even started.
    interval.tick().await;
    loop {
        interval.tick().await;
        let Some(ui) = weak.upgrade() else { break };
        let view = format_view(&ui);
        if let Some(s) = read_mem_breakdown() {
            log::info!(
                "[MEM view={view} VmRSS={} RssAnon={} RssFile={} RssShmem={} VmData={} (KiB)]",
                s.vm_rss,
                s.rss_anon,
                s.rss_file,
                s.rss_shmem,
                s.vm_data
            );
        }
    }
}

/// Per-tick memory snapshot. All values in KiB.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct MemSnapshot {
    /// Total resident set size — what most "total memory" displays show.
    vm_rss: u64,
    /// Anonymous resident (heap + stack). Closest match to "what *this
    /// process* allocated on its own", excluding shared libs / file maps.
    rss_anon: u64,
    /// File-backed resident (loaded libs, mapped files). Driver-side GPU
    /// buffers often land here on Mesa.
    rss_file: u64,
    /// Shared-memory resident (`shmem`, `tmpfs`, anonymous shared). Some
    /// graphics drivers use this for inter-process buffer sharing.
    rss_shmem: u64,
    /// "Data" virtual size — anonymous + heap + private mappings. Tracks
    /// the working-set ceiling even when pages get unmapped from RSS.
    vm_data: u64,
}

/// Read all the resident-memory breakdown fields from `/proc/self/status`
/// in a single pass. Returns `None` on read failure — the sampler silently
/// skips that tick rather than spam errors.
#[cfg(target_os = "linux")]
fn read_mem_breakdown() -> Option<MemSnapshot> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let mut snap = MemSnapshot::default();
    for line in status.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let field = match key {
            "VmRSS" => &mut snap.vm_rss,
            "RssAnon" => &mut snap.rss_anon,
            "RssFile" => &mut snap.rss_file,
            "RssShmem" => &mut snap.rss_shmem,
            "VmData" => &mut snap.vm_data,
            _ => continue,
        };
        let trimmed = val.trim();
        let kib_str = trimmed.strip_suffix(" kB").unwrap_or(trimmed);
        if let Ok(n) = kib_str.trim().parse::<u64>() {
            *field = n;
        }
    }
    Some(snap)
}
