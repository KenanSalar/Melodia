//! Streaming download with HTTP-range resume, ETag-aware `If-Range`
//! freshness, and a 5 %-slack size bound that aborts a runaway transfer
//! before it can fill the user's disk.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use futures_util::StreamExt;

use crate::error::{AppError, AppResult};

use super::staging::{
    StagedMeta, discard_staging_if_sidecar_mismatches, sidecar_meta_path, write_staged_meta,
};

/// 5 % slack over `asset.size` is enough to absorb the rare CDN that
/// over-reports `Content-Length` (chunked transfer accounting,
/// gzip-on-the-wire that the client decodes to a slightly different
/// byte count) without giving a malicious or buggy manifest enough
/// rope to fill the user's disk. A compromised manifest claiming
/// 80 MiB but serving 10 GiB trips this check at the 84 MiB mark,
/// well before the signature mismatch would otherwise have to wait
/// for the whole transfer.
pub(crate) fn exceeds_size_bound(downloaded: u64, expected_size: u64) -> bool {
    // `expected_size * 1.05` without floats. Saturate to dodge overflow
    // on a pathological manifest where `expected_size * 105` would
    // wrap — the saturation always biases toward "too big" which is
    // the safe direction (we'd reject a giant claim, never accept one).
    let bound = expected_size.saturating_mul(105) / 100;
    downloaded > bound
}

/// Filter an `ETag` header value down to a strong tag, dropping weak
/// ones (`W/"..."`) and any header that didn't decode as ASCII.
///
/// RFC 9110 §13.1.5 forbids generating an `If-Range` containing a weak
/// entity-tag, and §8.8.3.2 strong comparison means a server that
/// received one would always evaluate it false — silently forcing a full
/// re-download on every resume attempt. Returning `None` for weak tags
/// makes the resume protocol fall back to plain `Range:`, which is still
/// safe: the post-download minisign signature verify catches any
/// concatenation accident.
///
/// Production target (GitHub Releases CDN, Azure-Blob via Fastly) serves
/// strong tags today (e.g. `"0x8DBF736055F8CFD"`); this filter is a
/// futureproof for an origin switch rather than a current production
/// fix.
pub(crate) fn capture_strong_etag(header_value: Option<&str>) -> Option<String> {
    header_value
        .filter(|tag| !tag.starts_with("W/"))
        .map(str::to_owned)
}

/// Decision tree for an existing `dest` file when a download starts:
///
/// * **`Skip`** — bytes already on disk match the manifest's `size`
///   (within a single-byte tolerance — most CDNs report exact lengths).
///   This is the "retention on failure" hot path: a previously-verified
///   `.rpm`/`.deb` whose `dnf install` was cancelled or hit a transient
///   error stays on disk. Re-downloading wastes the user's bandwidth
///   *and* their patience. The caller's `verify_staged` step still
///   runs, so a corrupted file gets caught.
/// * **`Resume(offset)`** — partial bytes exist (`0 < existing < expected`).
///   Try a `Range: bytes=<offset>-` request; the caller falls back to
///   `Fresh` if the server returns 200 instead of 206.
/// * **`Fresh`** — start a new download into a truncated file. Covers
///   "no file", "empty file", and the bizarre "existing > expected"
///   case (likely an ancient leftover from a release with a larger
///   asset; safest is to discard and re-download from scratch).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResumeState {
    Skip,
    Resume(u64),
    Fresh,
}

pub(crate) fn plan_resume(existing_size: u64, expected_size: u64) -> ResumeState {
    if expected_size == 0 {
        // Manifest reports zero size — malformed. Force fresh so the
        // bound check at C2 catches whatever the server actually serves.
        return ResumeState::Fresh;
    }
    if existing_size == 0 {
        return ResumeState::Fresh;
    }
    if existing_size == expected_size {
        return ResumeState::Skip;
    }
    if existing_size < expected_size {
        return ResumeState::Resume(existing_size);
    }
    // existing > expected — leftover from a different release, or a
    // wild manifest-size shrink. Discard.
    ResumeState::Fresh
}

/// Write the `(version, size, url, etag?)` staging sidecar next to a
/// staged file. Single source of truth for sidecar construction —
/// [`download_to_file`] writes it once at download start and refreshes
/// it whenever the captured `ETag` changes.
fn write_sidecar(
    path: &Path,
    version: &str,
    size: u64,
    url: &str,
    etag: Option<&str>,
) -> AppResult<()> {
    write_staged_meta(
        path,
        &StagedMeta {
            version: version.to_string(),
            size,
            asset_url: url.to_string(),
            etag: etag.map(str::to_owned),
        },
    )
}

pub(super) async fn download_to_file(
    http: &reqwest::Client,
    url: &str,
    expected_version: &str,
    expected_size: u64,
    dest: &Path,
    on_progress: &(impl Fn(u8) + Send + Sync),
) -> AppResult<()> {
    // Sidecar check before any size-based decision. If bytes exist
    // alongside a missing / mismatching sidecar, they're from a
    // different release (or pre-sidecar code path) — can't be trusted
    // for `Skip` or `Resume`. Discarding here means a `--clobber`
    // re-push at the same Cargo.toml version, or any cross-release
    // resume, restarts cleanly instead of gluing mismatched bytes.
    //
    // The surviving sidecar (if any) carries the previous attempt's ETag,
    // which a resume can send back as `If-Range`. `None` for fresh
    // downloads, pre-etag-format sidecars, CDN responses that omitted
    // the header, or weak ETags filtered at capture (see response_etag).
    let existing_etag =
        discard_staging_if_sidecar_mismatches(dest, expected_version, expected_size, url)
            .and_then(|m| m.etag);

    let existing_size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let resume = plan_resume(existing_size, expected_size);

    // Write (or refresh) the sidecar at the *start* of the download so a
    // kill-mid-stream leaves a matching `(version, size, url, etag?)` next
    // to the partial bytes — the next attempt resumes safely instead of
    // discarding. Preserves any pre-existing etag so the kill-before-GET
    // window doesn't lose freshness metadata that's still correlated with
    // the disk bytes. Best-effort: a sidecar write failure shouldn't
    // abort the download (worst case the next attempt re-discards and
    // starts fresh, which is the same as today's behaviour without
    // sidecars).
    if let Err(e) = write_sidecar(
        &sidecar_meta_path(dest),
        expected_version,
        expected_size,
        url,
        existing_etag.as_deref(),
    ) {
        log::warn!(
            "updater: failed to write staging sidecar at {}: {e} (download continues; \
             worst case a future retry restarts from zero)",
            sidecar_meta_path(dest).display()
        );
    }

    if matches!(resume, ResumeState::Skip) {
        log::info!(
            "updater: skipping download — {} already has {existing_size} bytes (matches manifest); \
             verify step will validate",
            dest.display()
        );
        on_progress(100);
        return Ok(());
    }

    let mut req = http.get(url);
    if let ResumeState::Resume(offset) = resume {
        log::info!(
            "updater: resuming download at byte {offset} (have {existing_size} of {expected_size})"
        );
        req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        // If-Range: <etag> — the server returns 206 only when the
        // resource is byte-identical to the version that produced the
        // ETag we're showing; otherwise it falls back to 200 + the full
        // body. Closes the "release re-uploaded with --clobber
        // mid-download" hole that would otherwise concatenate bytes
        // from two different artifacts. Server-side support is universal
        // — RFC 9110 mandates it whenever Range is implemented.
        if let Some(ref tag) = existing_etag
            && let Ok(value) = reqwest::header::HeaderValue::from_str(tag)
        {
            req = req.header(reqwest::header::IF_RANGE, value);
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| AppError::Network(format!("update download GET {url} failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::Network(format!(
            "update download {url} returned HTTP {status}"
        )));
    }

    // Capture the response ETag so future resumes can send it back as
    // `If-Range`. Done once, before consuming the body, because reqwest
    // moves headers out of the response on the first call to most
    // streaming APIs in some versions; safest to read it eagerly.
    // `capture_strong_etag` drops weak tags at the boundary so they
    // never reach the sidecar — see its doc for the RFC 9110 rationale.
    let response_etag = capture_strong_etag(
        resp.headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok()),
    );

    // Server-side resume support is best-effort. A 206 Partial Content
    // means the server honoured our `Range:` (and any `If-Range:`)
    // request and the stream continues from `offset`. A 200 OK means
    // either the server ignored the Range header outright (some CDNs,
    // ranges through a transparent proxy, …) or our `If-Range:` ETag
    // didn't match (resource changed since we last fetched). Either way
    // we have to discard our partial bytes and start over rather than
    // concatenating into a garbled file.
    let sidecar_path = sidecar_meta_path(dest);
    let refresh_sidecar = |etag: Option<&str>| -> AppResult<()> {
        write_sidecar(&sidecar_path, expected_version, expected_size, url, etag)
    };
    let (mut file, mut downloaded) = match (resume, status) {
        (ResumeState::Resume(offset), reqwest::StatusCode::PARTIAL_CONTENT) => {
            // 206 — existing bytes stay correlated with `existing_etag`.
            // Update the sidecar only if the server bumped the ETag (rare
            // but legal: weak validators, transport-encoding switches).
            if response_etag.is_some() && response_etag.as_deref() != existing_etag.as_deref() {
                let _ = refresh_sidecar(response_etag.as_deref());
            }
            let f = std::fs::OpenOptions::new().append(true).open(dest)?;
            (f, offset)
        }
        (ResumeState::Resume(_), _) => {
            // 200 (or other 2xx) where we asked to resume — server
            // either ignored `Range:` or rejected our `If-Range:` ETag
            // because the resource changed. Reset bytes + sidecar etag
            // so the next attempt resumes against the new resource.
            log::info!(
                "updater: server returned {status} on Range request (likely If-Range mismatch \
                 or proxy stripped the header); restarting from offset 0"
            );
            let _ = refresh_sidecar(response_etag.as_deref());
            (File::create(dest)?, 0u64)
        }
        _ => {
            // Fresh download — write sidecar with the captured ETag so
            // the next resume (if this one is interrupted) can use it.
            let _ = refresh_sidecar(response_etag.as_deref());
            (File::create(dest)?, 0u64)
        }
    };

    let chunk_total = resp.content_length().unwrap_or(expected_size).max(1);
    // Progress percentage is based on the *manifest* expected_size, not
    // the response's `Content-Length` — a resumed download's
    // Content-Length is the remaining bytes, which would make progress
    // appear to start at 0% and march to 100% even though the file is
    // already 80% done. Using expected_size keeps the bar honest.
    let progress_denominator = expected_size.max(chunk_total);
    let mut stream = resp.bytes_stream();
    let mut last_pct: u8 = u8::try_from(
        (downloaded.saturating_mul(100) / progress_denominator).min(100),
    )
    .unwrap_or(0);
    on_progress(last_pct);

    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| AppError::Network(format!("update download stream broke: {e}")))?;
        let new_total = downloaded.saturating_add(chunk.len() as u64);
        if exceeds_size_bound(new_total, expected_size) {
            // Drop the file handle + remove the partial bytes before
            // surfacing the error so the staging dir doesn't keep a
            // disk-bloating ghost of the rejected download. The
            // retention-on-failure pattern is for *verified* bytes
            // (post-signature); a size-bound abort means the bytes
            // never had a chance to pass verify, so cleanup is right.
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(AppError::Network(format!(
                "update download exceeded declared size: {new_total} > {expected_size} (5% slack); \
                 manifest may be compromised or the CDN is misbehaving"
            )));
        }
        file.write_all(&chunk)?;
        downloaded = new_total;
        let pct = u8::try_from(
            (downloaded.saturating_mul(100) / progress_denominator).min(100),
        )
        .unwrap_or(100);
        if pct != last_pct {
            on_progress(pct);
            last_pct = pct;
        }
    }
    file.flush()?;
    Ok(())
}
