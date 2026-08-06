---
paths:
  - src/services/updater/**/*.rs
  - src/services/desktop_integration.rs
  - src/tasks/updater_daily.rs
  - src/ui/updater_settings.rs
  - wix/main.wxs
  - scripts/build-latest-json.py
  - scripts/build-appimage.sh
  - scripts/build-rpm.sh
  - scripts/install-linux.sh
  - .github/workflows/release.yml
  - .github/workflows/refresh-manifest.yml
---

# In-app updater, packaging and release

The trust boundary is the GitHub repo — a manifest the publisher signed is trusted content,
so the threat model stops at transport and integrity, not at a hostile manifest.

`main()`'s `--version` literal-first branch is the one piece of this that lives in the root
`CLAUDE.md`, because it is a forward-compat contract older clients depend on and you can
break it from `main.rs` without ever opening this file.

## Install methods

- **Atomic-swap retains `.old` for rollback (Linux AppImage/tarball only).** Two-step rename: `target → target.old`, `staged → target`, smoke-test, rollback on failure. `.old` reaped on first successful boot. Single source: `install::old_path()`. `pkexec mv` cross-fs fallback, `install_via_package_manager` (Linux RPM/DEB), and `install_via_msiexec` (Windows MSI) retain NO `.old` — package format owns the replace. Smoke-test skipped on those paths via `InstallMethod` match in `download_and_install`. `main()`'s `.old` reaper is `cfg(target_os = "linux")`. macOS not a CI target — `swap_in_place` falls through to `std::fs::rename`.
- **Windows installs are per-machine MSIs at `C:\Program Files\Melodia\bin\`.** `wix/main.wxs` `Scope="perMachine"` + `ProgramFiles6432Folder`; UAC at install. Start Menu shortcut Component under `ProgramMenuFolder` (without it, Windows Search can't find the app). Console suppressed via `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` — release runs as GUI, `cargo run` keeps console for `RUST_LOG`. In-app updater downloads signed `.msi` to `%LocalAppData%\Melodia\update-staging\`, spawns `msiexec /i <staged> /qb!` non-blocking — UAC re-prompts, WiX `MajorUpgrade` + `util:RestartResource` handle replacing the running binary. Smoke test skipped. `system_install::probe` short-circuits `windows-*-msi` keys to `false` so updater UI stays visible — symmetric with `linux_pkg::detect`.
- **Polkit helper for in-app updater RPM/DEB installs.** `/usr/libexec/melodia-update-helper` argv-dispatches to dnf5/dnf/apt/apt-get; policy `com.github.kenansalar.melodia.update`. `install_via_package_manager` runtime-detects, falls back to direct `pkexec dnf install`. **`LinuxPackageFormat` variants double as helper argv-dispatch keys**.
- **`services::desktop_integration` self-deploys `.desktop` + icon on boot for tarball installs.** Compiled-in payloads `assets/desktop/Melodia.desktop.tmpl` (`@EXEC@`) + `logo-with-background.svg`. BLAKE3-gated idempotent writes to `~/.local/share/applications/...desktop` + `~/.local/share/icons/.../melodia.svg`. Skipped on AppImage (`$APPIMAGE` set) + RPM/DEB. Don't move source paths without updating `include_*!` call sites.

## Download path

- **Download bound check (5% over manifest size) aborts streams.** `exceeds_size_bound(downloaded, expected_size)` saturates `expected_size * 105` toward "reject" on overflow; tripping it drops file, removes partial bytes, returns `AppError::Network`.
- **HTTP Range resume via `plan_resume(existing_size, expected_size)`.** Returns `Skip` (existing == expected → skip network, verify catches corruption), `Resume(offset)` (send `Range: bytes=<offset>-`), or `Fresh`. 206 appends; 200 resets. Progress denominator stays on `expected_size`.

## Manifest

- **`latest.json` minisign-signed; client verifies before parse.** Same key as per-artifact sigs (`assets/updater-pubkey.b64`). `fetch_latest_manifest` fetches `latest.json.minisig` and calls `minisign::verify_manifest_bytes` — `manifest=true` trusted comment is a **domain-separation tag**. Fail-closed: missing/invalid sig → `AppError::Validation`. CI: `minisign -SHm latest.json -t "version=$VERSION manifest=true"`.
- **The manifest is built twice, and the second build is the one users read.** `release.yml` builds `latest.json` while the release is still a draft, so its `notes_short` is GitHub's `--generate-notes` text; `refresh-manifest.yml` fires on draft→published, re-reads the now-final (author-edited) body, and rebuilds + re-signs + re-uploads the manifest and `SHA256SUMS.txt`. `version` / `manifest_schema_version` / `critical` / `pub_date` are carried over from the published manifest verbatim, so a refresh changes the notes and nothing else — `pub_date` in particular must not bump. **`workflow_dispatch` with a `tag` input refreshes retroactively**, which is the recovery path when the job fails: the release keeps the draft-time manifest, which is complete and correctly signed, merely stale in its notes. **Both callers hand `build-latest-json.py` an `--artifacts` directory holding installable artifacts and their `.minisig` siblings and nothing else** — the script aborts on anything it can't classify (see below), and a rebuild's non-artifact inputs are staged in `$RUNNER_TEMP` for exactly that reason. Staging the published `latest.json` in `artifacts/` alongside them is what broke v0.9.0's refresh on the first publish after that guard landed.
- **Manifest schema gate + critical-release flag** (`src/services/updater/manifest.rs`). `manifest_schema_version: u32` (default 1) — `check.rs` returns `CheckOutcome::UnsupportedSchema` when `> SUPPORTED_MANIFEST_SCHEMA`, treated like `NoAssetForTarget`. Bumping requires bumping `build-latest-json.py`'s `--manifest-schema-version` + CI invocation. `critical: bool` (default false) hides "Skip this version" and bypasses `skipped_release` filter. Set via `--critical` in `scripts/build-latest-json.py`.

## Release matrix

- **Build provenance attestation.** `release.yml` runs `actions/attest-build-provenance` (v4, SHA-pinned) per matrix slot (needs `id-token: write` + `attestations: write`). Verify: `gh attestation verify <file> --repo KenanSalar/Melodia`. Upstream now calls v4 a thin wrapper over `actions/attest` and points new work at that directly — worth folding in next time this line is touched.
- **aarch64 builds alongside x86_64** (Linux + Windows). 10 `release.yml` matrix slots: 5 × x86_64 + 5 × aarch64 (`ubuntu-24.04-arm`/`windows-11-arm`). **`build-latest-json.py`'s `PLATFORM_PATTERNS` is downstream of `release.yml`'s packaging steps and must move with them.** Every pattern now pins an explicit arch token, so the two arch groups are disjoint and the aarch64-first ordering is readability rather than disambiguation — that ordering existed for cargo-deb's native `_arm64.deb`, which carries no leading-arch token, and `release.yml` renames the deb to the `melodia-<tag>-<arch>.deb` scheme every other slot produces so its `melodia-*.deb` globs match. The table was left on the old name and **v0.8.0 shipped with no deb entry at all** while both signed `.deb` files sat on the release; deb clients got `NoAssetForTarget` indefinitely. It survived because `classify()` returning `None` was a bare `continue` — an unclassifiable file in `artifacts/` is now a hard `SystemExit`, since that directory holds installable artifacts and their `.minisig` siblings and nothing else. Client `target::current_target_key()` `cfg!`-branched per `(target_os, target_arch)`; its key strings are the same table's values, so a renamed key breaks both ends. `build-{appimage,rpm}.sh` read `ARCH` (default `uname -m`) with per-arch pinned `linuxdeploy` SHA256s — bump in lockstep.
