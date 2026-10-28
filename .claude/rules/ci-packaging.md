---
paths:
  - .github/**
  - packaging/**
  - licenses/**
  - crates/melodia/wix/main.wxs
  - scripts/build-rpm.sh
  - scripts/build-appimage.sh
  - scripts/build-tarball.sh
  - scripts/install-linux.sh
  - scripts/build-latest-json.py
  - Cargo.toml
  - crates/melodia/Cargo.toml
  - rust-toolchain.toml
  - clippy.toml
  - .cargo/audit.toml
---

# CI and packaging

The gate around a release and the obligations every artifact carries; the release matrix is
`.claude/rules/updater.md`'s. `release.yml` holds the shape of a release and calls
`release-{prepare,build,publish}.yml`. Each workflow and composite argues its own steps, so this
file holds what couples them to each other, to the tests that read them, or to the Rust tree.

## The PR gate

`pr-validation.yml`, on PRs into `main`: `changes` → `audit` ∥ `fmt` ∥ `clippy` ∥ `test` ∥
`clippy-windows` ∥ `test-windows`. Every job is a sibling under `changes`, so no failure hides
another job's result. No coverage on this path.

- **The aggregate `pr-validation` job is the required status check, and `needs:` is the only
  list.** A job that never reached `needs:` goes unwaited-on, which
  `crates/melodia/tests/packaging.rs` walks the workflow for.
- **The advisory scan is one policy with two callers**: `.github/actions/cargo-audit` takes no
  inputs, so the gate's early warning and the release's hold can't disagree.
- **The skip matrix is a denylist, and `predicate-quantifier: 'every'` is what makes it work**,
  both argued in the workflow.
- **A path leaves the denylist only once nothing builds, reads or runs it before a merge**, so
  `.github/` is excluded per file, never as `.github/**`. The composites the gate runs,
  `pr-validation.yml` itself and `deploy-coverage.yml`, which `packaging.rs` reads, stay in. So do
  the release workflows and `CODEOWNERS`: nothing exercises them before a tag, and a control of
  that reach failing closed is worth one wasted run. `LICENSE` stays in because the licence pins
  read it; compiling nothing is not being unexercised.
- **Headless audio.** `.github/actions/headless-audio` is the whole shim, and `output::device`'s
  module doc says what in the Rust tree leans on it, `crates/melodia/tests/headless.rs` being what
  fails without it. `test-windows` is the one job that can't have it, so it skips
  `headless_scan` by name rather than by target, and a new integration test still runs there.
- **The Windows jobs are the only place a `cfg(windows)` test runs or clippy meets a
  `cfg(windows)` arm.** `release-build.yml` builds the Windows lib and bin only on a tag, with
  rustc and no `--cfg test`. Cross-checking from Linux is no substitute: `ring`, `aws-lc-sys`,
  `libsqlite3-sys` and `blake3` compile C, so the MSVC target wants MSVC.
- **`CARGO_BUILD_JOBS: 4` on `test` and `test-windows` is a memory cap, not a CPU match; don't
  raise it to `.cargo/config.toml`'s 8.** The peak is one `rustc` on `melodia-ui`, whose codegen
  threads draw jobserver tokens, so the count scales that process; past the 16 GB runner the job
  swaps and reads as a hang. The clippy jobs never codegen and set none. `RUST_TEST_THREADS` keeps
  the config's 8, most harness threads waiting rather than running; `single_instance_tests`' two
  1 s `recv_timeout`s are the tightest budgets left if `test-windows` reddens on a timeout.
- **The memory floor is `melodia-ui` itself**, so the next lever is a smaller generated unit.
  `split-debuginfo`, `debug = 0`, `codegen-units` and fewer test binaries were measured and don't
  move it.
- **Debug info is `line-tables-only` in the gate's two `test` jobs and `0` in coverage**, both as
  `CARGO_PROFILE_{DEV,TEST}_DEBUG` in the workflow so local and release builds are untouched. Each
  workflow argues its side; `packaging.rs` holds them apart, agreement reddening nothing.
  rust-cache hashes `CARGO_*`, so a change costs one cold run per platform.
- **Four Windows levers that look obvious and are not:** a Defender exclusion (the image already
  disables real-time monitoring and excludes both drives); `CARGO_INCREMENTAL: 0` (rust-cache sets
  it); folding `clippy-windows` into `test-windows` (check and build units share nothing past
  proc-macros and build scripts, so one job compiles the graph twice in series); capping
  `clippy-windows`' jobs (it never codegens).
- **Coverage runs off the PR path**, `deploy-coverage.yml` on `workflow_dispatch` only, and leans on
  two repository settings no file holds: Pages enabled with Source = GitHub Actions, and the
  `github-pages` environment's branch allowlist (`main`). A dispatch from another ref builds, then
  fails at `deploy`. The report lands at `https://kenansalar.github.io/Melodia/`.
- **Every action is SHA-pinned** with a trailing `# vX.Y.Z`. Every pin resolves to `node24` or a
  composite over one, so no `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24`; re-pin with
  `gh api repos/OWNER/REPO/commits/TAG -q .sha`. `.github/workflows/` and `.github/actions/` are
  CODEOWNER-protected. Every compiling job installs through `.github/actions/setup-rust`, and every
  Linux one, release slots included, provisions through `linux-system-deps`.

## File associations: five Linux spellings and one Windows one

The `MimeType=` list makes Melodia offerable; the `Exec=` field code is what hands it the files.

- **All four `.desktop` sources end `Exec=` with ` %F`**: `scripts/Melodia.desktop`, the heredocs
  in `build-appimage.sh` and `build-rpm.sh`, and `assets/desktop/Melodia.desktop.tmpl`.
  `desktop_integration_tests.rs` matches every extracted `Exec=` line and argues `%F`.
- **`scripts/install-linux.sh` is the fifth, and it is a rewriter.** Its `sed` replaces the
  command token alone, the DEB shipping the same file verbatim, and is pinned beside the four. Its
  quoting tests for a space where `quote_exec` tests the spec's reserved set, on purpose.
- **Windows is `crates/melodia/wix/main.wxs`'s `FileAssociations` component**, plain
  `RegistryValue` rows the wxs argues, held to `utils::audio_ext::AUDIO_EXTENSIONS` by
  `packaging.rs`: the one format no Linux runner builds.

## Licences: every format ships `licenses/`

Why every artifact owes the text, and why there are five formats, are `docs/adr/`'s. A format that
stops shipping it fails nowhere until a packager files it, so `packaging.rs` names each.

- **Five formats, five spellings, one named list** in `every_package_format_ships_the_licenses_dir`:
  the RPM's `%license`, the deb's asset glob, the `cp` in `build-tarball.sh` and
  `build-appimage.sh`, and `main.wxs`'s `File` set. Each needle is the mechanism, not the word.
- **Four glob the directory and WiX does not**, so a new licence file costs one `main.wxs` edit,
  which `packaging.rs` walks `licenses/` for. Don't spell out the RPM's `licenses/*`.
- **`packaging/debian-copyright` ships verbatim only because it opens with a DEP-5 key**, so
  `release-build.yml` pins cargo-deb to an exact version: a bump means re-reading
  `has_copyright_metadata`, not editing a number.
- **`MPL-2.0` and `GPL-3.0` are standalone `License:` paragraphs no `Files:` stanza names**, and
  lintian's `unused-license-paragraph-in-dep5-copyright` on them is expected: those terms arrive
  through crates with no source here, so a `Files:` stanza would point at nothing. Nothing here
  runs lintian; if it ever gates, add a `usr/share/lintian/overrides/melodia` asset, never delete
  the paragraphs.

## The workspace split, as the release workflow sees it

**Before adding a member, grep the `release*.yml` and the packaging scripts for `melodia`.** A
step naming something unambiguous with one member breaks deep in a matrix slot, after review. The
split broke two in `release-build.yml`, each argued at its step: the MSI's `--package melodia`,
and the extension-qualified upload globs.

Four places scrape the version from the manifest without a toolchain: `release-prepare.yml`,
`build-rpm.sh`, `build-appimage.sh` and `build-tarball.sh`. All four anchor on
`[workspace.package]`, and every member carries `version.workspace = true`.
