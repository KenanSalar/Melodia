//! Opt-in memory sampler: with `MELODIA_RSS_SAMPLE` set to anything, logs `/proc/self/status`'s
//! resident breakdown and the current view every 500 ms, so a memory move can be matched to what
//! the user did. Linux only.
//!
//! **The loop is handed back, not spawned**: its tag reads Slint globals, so it runs on the UI
//! thread, and this crate names no Slint.
//!
//! The tag is the view, then `+NP` with Now Playing open and `+QP` with the queue panel shown. The
//! breakdown says which metric an external monitor tracks, many showing PSS or anon only.

#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// The loop, or `None` outside a diagnostic session. `view_tag` answers `None` once the window is
/// gone, which ends it.
pub fn sampler(
    view_tag: impl Fn() -> Option<String> + 'static,
) -> Option<impl Future<Output = ()>> {
    if std::env::var_os("MELODIA_RSS_SAMPLE").is_none_or(|v| v.is_empty()) {
        return None;
    }

    #[cfg(target_os = "linux")]
    {
        log::info!("[MEM] sampler enabled (interval {} ms)", SAMPLE_INTERVAL.as_millis());
        Some(run(view_tag))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = view_tag;
        log::info!("[MEM] sampler env set but no /proc on this platform — skipped");
        None::<std::future::Ready<()>>
    }
}

#[cfg(target_os = "linux")]
async fn run(view_tag: impl Fn() -> Option<String>) {
    let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick is immediate, before the boot's own fetches have started.
    interval.tick().await;
    loop {
        interval.tick().await;
        let Some(view) = view_tag() else { break };
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

/// In KiB.
#[cfg(target_os = "linux")]
#[derive(Default)]
struct MemSnapshot {
    /// What most "total memory" displays show.
    vm_rss: u64,
    /// Heap and stack: what this process allocated itself.
    rss_anon: u64,
    /// Libraries and mapped files; Mesa's GPU buffers often land here.
    rss_file: u64,
    /// Shared memory, which some graphics drivers pass buffers through.
    rss_shmem: u64,
    /// Private virtual size, the working-set ceiling even as pages leave RSS.
    vm_data: u64,
}

/// `None` on a failed read, the tick skipped rather than logged.
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
