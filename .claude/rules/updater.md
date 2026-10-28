---
paths:
  - crates/melodia-app/src/services/updater/**/*.rs
  - crates/melodia-platform/src/services/platform/install_kind/**/*.rs
  - crates/melodia-platform/src/services/platform/desktop_integration.rs
  - crates/melodia-app/src/tasks/updater_daily.rs
  - crates/melodia-views/src/ui/settings/updater_settings.rs
  - crates/melodia/wix/main.wxs
  - scripts/build-latest-json.py
  - scripts/build-appimage.sh
  - scripts/build-rpm.sh
  - scripts/build-tarball.sh
  - scripts/install-linux.sh
  - .github/workflows/release*.yml
  - .github/workflows/refresh-manifest.yml
  - .github/actions/build-manifest/action.yml
  - .github/actions/minisign-sign/action.yml
---

# In-app updater, packaging and release

The trust boundary is the GitHub repo: a manifest the publisher signed is trusted content, so the
threat model stops at transport and integrity. `docs/adr/` argues it and what the line costs.
`main()`'s `--version` branch is root `CLAUDE.md`'s, being breakable from `main.rs` alone.

Four couplings between the client, the workflows and the script are held by
`crates/melodia/tests/updater_contract.rs`: the trusted-comment fields, the prehashed signing step,
the platform keys and the schema default. `manifest=true` is argued at `verify_manifest_bytes`.

## Install methods

- **Two gates, two questions.** `updater::is_available()` is false on a source build, whose
  `target/` cargo owns: no daily task, and `MelodiaUpdater.updates-supported` takes the card out of
  the settings search through `UpdateSection.has-matches`. `is_system_install()` is true where no
  in-app path reaches (`system_install`'s module doc): the daily task stays down, the manual check
  survives, and Download and Skip give way to a hint. Both ways into `MelodiaUpdater.install()`
  read the flag, the panel's button in `update-section.slint` and the Available toast in
  `updater_settings::dispatch`. Widening the second to cover a dev build would offer that hint to
  `cargo run`.
- **Only the atomic swap (AppImage, tarball) keeps a `.old`**, which `install/mod.rs` smoke-tests
  and rolls back from and `main()` reaps on the next Linux boot. The RPM/DEB install and the MSI
  leave the replace to the format and skip the smoke test.
- **Windows is a per-machine MSI the updater re-runs.** `swap::install_via_msiexec` and `main.wxs`
  argue the Restart Manager close, the relaunch and the property both spell.
- **The polkit helper is spelled in four places**: `swap.rs`'s `HELPER` and its `rpm`/`deb` argv,
  `packaging/melodia-update-helper`'s `case`, the policy's `exec.path`, and both packages' install
  lines (`build-rpm.sh`, `crates/melodia/Cargo.toml`'s deb assets). Without it the updater runs
  `pkexec <dnf5|dnf|apt|apt-get> install` directly.
- **An upgrade breaks the running binary's path two ways.** An RPM/DEB upgrade unlinks it, and
  `utils::exe::current_exe` resolves the `" (deleted)"` marker. Only a late caller can meet one, so
  `desktop_integration`'s `Exec=` and `linux_pkg::detect` are safe today by running at boot and
  caching. Keep them on `install_target()` regardless: a marked path makes `rpm -qf` miss, and the
  miss compounds into a tarball offered to an RPM install, a wrong `install=` in crash reports and
  `Exec=… (deleted)` written into the launcher. The atomic swap renames instead, which no lookup
  sees through, so `window_chrome::set_respawn_exe`'s capture before the swap stays load-bearing.

## Manifest

**It is built twice and users read the second**: `release-publish.yml` at draft time,
`refresh-manifest.yml` on publish with the final notes, both through
`.github/actions/build-manifest`. A refresh carries `version`, `manifest_schema_version`, `critical`
and `pub_date` over and changes only the notes. The workflow argues it, the `--artifacts` contract
and the `workflow_dispatch` recovery included.

## Release matrix

- **Ten slots, each of the five formats on x86_64 and aarch64** (`release.yml`'s matrix), each one
  attested and signed in `release-build.yml`.
- **A platform key is spelled in three tables.** The contract test reads `target::format_key`
  against `build-latest-json.py`'s `PLATFORM_PATTERNS`; `staging::install_method_for_key` is the
  third, outside it, and argues what a typo there costs. The patterns follow the artifact names
  `release-build.yml` and `scripts/build-*.sh` produce, and the script aborts on a file none
  matches.
