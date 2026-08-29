# Plan: `self-update` Cargo feature gate

Status: **proposed** · Owner: Kenan · Created: 2026-06-16

> **Validated 2026-06-16** against The Cargo Book (Context7
> `/websites/doc_rust-lang_cargo`, feature-flags reference) and the live source
> tree. Corrections from the first draft are flagged inline with **[fix]**.

## Goal

Make the in-app auto-updater a **compile-time Cargo feature** (`self-update`,
default-on) so we can ship two build configurations from one codebase:

- **GitHub-release artifacts** (the current release pipeline: tarball, AppImage,
  RPM, DEB, MSI) — feature **on**. These *are* the self-update channel: the updater
  downloads the matching signed artifact and installs it (Linux via the polkit
  helper / `pkexec`, Windows via `msiexec`).
- **External package-manager builds** (future: COPR/dnf, Flatpak/Flathub, AUR,
  Debian/Ubuntu repos, Microsoft Store MSIX) — feature **off**. The repository /
  store owns updates, and the network-fetch + self-replace code is *provably
  absent* from the binary (what distro/Flathub/Store reviewers require), not
  merely hidden at runtime.

This is **not "two versions of the app."** It is one codebase, one version
number, two build configs selected by `--no-default-features`. Same `Cargo.toml`
version everywhere; the disabled build simply lacks the update UI and code.

**Why this is an *additive* feature (per Cargo guidance).** The Cargo Book's
rule is that features should be additive — enabling one only *adds* capability.
Here the base build (feature off) is the smaller, repo-friendly binary; enabling
`self-update` *adds* the updater. So `--no-default-features` yields a valid,
smaller build, never a broken one. This is the correct polarity.

## Non-goals

- Authoring the per-repo packaging pipelines (COPR `.spec`, Flatpak manifest,
  AUR `PKGBUILD`, Store MSIX). Tracked separately — see *Follow-ups*.
- Touching the runtime install-detection (`linux_pkg::detect`,
  `system_install::probe`). It stays; the feature flag complements it.
- Gating `desktop_integration` self-deploy (separate concern — see *Follow-ups*).

## Which channel gets which feature state

| Artifact | Built by | `self-update` |
|---|---|---|
| tarball / AppImage / RPM / DEB / MSI in **GitHub Releases** | existing release pipeline | **on** (unchanged) |
| COPR / dnf repo RPM | new `.spec` pipeline | **off** |
| Flatpak / Flathub | new manifest | **off** |
| AUR (`-bin` or source) | `PKGBUILD` | **off** |
| Debian / Ubuntu repo / PPA | new source package | **off** |
| Microsoft Store MSIX | new pipeline | **off** |

> Note: the GitHub-release RPM/DEB keep the updater **on** — the polkit helper
> ships in those packages precisely so the updater can `pkexec`-install the next
> version. The feature-off builds are only the externally-hosted repo artifacts.

## The feature

`Cargo.toml`:

```toml
[features]
default = ["self-update"]
self-update = ["dep:minisign-verify"]
```

- The `dep:minisign-verify` form is the Cargo-recommended syntax for gating an
  optional dependency: it **suppresses the implicit `minisign-verify` feature**,
  so the crate can be pulled in *only* via `self-update` (no second way to enable
  it, no accidental exposure).
- Normal `cargo run` / `cargo build` / GitHub CI → feature on (no change to
  current behavior).
- Feature-off build: `cargo build --release --no-default-features`
  (still needs the usual Linux GUI/system deps — features only drop the updater).

## Dependency change

`minisign-verify` is used **only** by the `minisign` submodule of the updater
(confirmed: every referencing file is under `src/services/updater/`). Make it
optional so it disappears from feature-off builds — fewer crates for distro
auditors:

```toml
# was: minisign-verify = "0.2.5"
minisign-verify = { version = "0.2.5", optional = true }
```

`reqwest` stays unconditional — it is **shared** with `src/media/deezer.rs` and
`src/services/artist_images.rs`, so it can't be made update-only. (All the
*update-side* reqwest usage lives in the gated submodules, so the feature-off
build still won't perform any update network I/O.)

## Module split — the crux

Split `src/services/updater/` into "detection/metadata" (always compiled, cheap,
no heavy deps, referenced by non-updater code) vs "active behavior" (network +
crypto + binary swap, gated). This is what lets `minisign-verify` become
optional.

**Verified dependency boundary** (`src/services/updater/`):
- `system_install.rs` imports `super::install_target`, `super::linux_pkg`,
  `super::probe::dir_is_writable`, `super::target::current_target_key` — all
  ungated. It references `super::install::download_and_install` **only in a doc
  comment**, not in code.
- `linux_pkg.rs` imports `super::install_target` (ungated); `super::install` only
  in a doc comment.
- `target.rs` imports `linux_pkg` (ungated). `probe.rs` / `version.rs` import no
  siblings.
- `event.rs` / `state.rs` import **nothing** — pure data enums.

`src/services/updater/mod.rs` submodule decls:

| Submodule | Gate | Why |
|---|---|---|
| `system_install`, `linux_pkg`, `probe`, `target`, `version` | **ungated** | pure detection/metadata; called by `main.rs` + `updater_settings::install` |
| `event`, `state` | **gated** | update-only data types; gating keeps them out of the feature-off binary and avoids a broken doc link (event→install) |
| `check`, `github`, `manifest`, `minisign`, `install`, `asset_cache` | **gated** | network / crypto / swap (these pull `reqwest` stream + `minisign-verify`) |

*(A `test_support` submodule used to sit in this table. It held the env-var mutex the
ungated detection tests take, and the row said to keep it `#[cfg(test)]`-only rather than
feature-gate it. It has since moved to `src/test_support.rs` at the crate root — the lock
had to cover `settings_tests` too — so the question no longer arises here: a crate-root
`#[cfg(test)]` module is outside the `self-update` gate by construction. Nothing to do,
but don't re-add the submodule when working through this plan.)*

`mod.rs` free functions **[fix]** (the first draft conflated these two):

| Item | Gate | Why |
|---|---|---|
| `install_target()` (mod.rs:85) | **ungated** | std-only (`current_exe`/`$APPIMAGE`); used by `system_install.rs:55` + `linux_pkg.rs:40` |
| `install_target_old()` (mod.rs:74) | **gated** | calls `install::old_path()` — depends on the gated `install` submodule |

`mod.rs` re-exports (lines 52-56):
- **Ungated:** `pub use system_install::is_system_install;`
- **Gated** (`#[cfg(feature = "self-update")]`): `check::{CheckOutcome, check_for_update}`,
  `event::{FailureKind, UpdaterEvent}`, `install::{download_and_install, prune_stale_staging}`,
  `state::UpdaterState`.

`pub mod updater;` in `src/services/mod.rs` stays — only its active submodules
are gated internally.

## Rust touch points

`src/main.rs`
- **Keep ungated:** the `--version` literal-first branch (forward-compat
  contract; it only prints, never calls into the gated install path).
- **[fix] Gate the `.old` reaper** at `src/main.rs:79-84` — it calls
  `install_target_old()`, now gated. Change `#[cfg(target_os = "linux")]` to
  `#[cfg(all(target_os = "linux", feature = "self-update"))]`. (A feature-off
  build never produces a `.old`, so reaping it is moot anyway.)
- **Gate** (`#[cfg(feature = "self-update")]`) the updater block at
  ~`src/main.rs:394-437`:
  - `updater_event_tx/rx` channel
  - `ui::updater_settings::install_event_subscriber(...)`
  - `ui::callbacks::wire_updater(...)`
  - the `updater_daily::spawn(...)` gate (incl. the `else` log branch)
  - the `prune_stale_staging()` boot task at ~`:434-437`

`src/tasks/mod.rs`
- Gate `pub mod updater_daily;` (`src/tasks/mod.rs:22`).

`src/ui/callbacks/mod.rs`
- Gate `mod updater;` (line 25) and `pub use updater::wire as wire_updater;`
  (line 52). The `src/ui/callbacks/updater/` dir (`check.rs`, `install.rs`,
  `paint.rs`, `mod.rs`) is referenced **only** via `wire_updater` (verified — no
  other module imports it), so gating the whole dir is clean.

`src/ui/settings/updater_settings.rs`
- **`install()` stays ungated** — it seeds `MelodiaUpdater.current-version`
  (also read by `about-section.slint` for the About version row),
  `system-managed` (via the ungated `is_system_install`), and `platform-kind`.
  Gating it would blank the About version. **[fix]** It also already seeds
  `updates-supported` from `is_available()` — the runtime source-build gate, which
  landed after this plan was written and takes the same card by the same route.
  Fold into it rather than adding a second boolean:
  `updater.set_updates_supported(cfg!(feature = "self-update") && is_available());`
  (`cfg!(...)` is a runtime bool literal that compiles in both configs).
- **[fix] Split the import** (line 24): `use crate::services::updater::is_system_install;`
  stays unconditional; move `UpdaterEvent` to
  `#[cfg(feature = "self-update")] use crate::services::updater::UpdaterEvent;`
  (otherwise it's an unused import — and a denied warning — when the feature is off).
- **Gate `install_event_subscriber()` and the private `dispatch()`**
  (`#[cfg(feature = "self-update")]`) — both consume `UpdaterEvent`.

## Slint touch points

**[fix] Nothing to add — the property already exists.**
`melodia-ui/ui/globals/updater.slint` carries
`in property <bool> updates-supported: true;`, and
`melodia-ui/ui/views/settings/update-section.slint` already ANDs it into
`has-matches`, which takes the card and its settings-search hits together (every
row lives inside `if has-matches: SectionCard`, so no per-row AND is needed).
Seed it as above and the feature-off build gets the same collapse the source-build
gate gets. The `install()` / `restart()` / `check()` callbacks stay defined; with
the feature off the Rust side simply never wires or invokes them (unwired Slint
callbacks are no-ops).

> **Slint pitfall — do NOT wrap the mount in `if`.** In
> `melodia-ui/ui/views/settings/pages/about-page.slint:27` the section is
> `updates := UpdateSection { … }`, and the page's `has-matches` references it by
> id (`updates.has-matches`, line 14), which the tab-level no-results predicate in
> `settings-tabs.slint` then reads. Wrapping `updates` in an `if` would put the id
> inside a conditional and break that sibling reference (and the
> `vertical-stretch` collapse math). Keep the component mounted; gate its content
> + `has-matches` internally instead.

`melodia-ui/ui/views/settings/about-section.slint` — unchanged (reads `current-version`,
still seeded by the ungated `install()`).

## Doc-comment cleanup (avoids `cargo doc --no-default-features` warnings)

**[fix]** A few ungated files carry intra-doc links to now-gated items. Under
`cargo doc --no-default-features` the link targets won't exist → broken
intra-doc-link warnings. These do **not** fail the clippy/build gate below
(rustdoc lints fire only under `cargo doc`), but per house style fix them by
converting the `[path]` links to plain backticked text:
- `src/services/updater/system_install.rs:~13` → `super::install::download_and_install`
- `src/services/updater/linux_pkg.rs:~14` → `super::install::download_and_install`
- `src/ui/settings/updater_settings.rs:~3, ~12` → `UpdaterEvent`

(`event.rs:75`'s link to `install` is fine — `event` is gated, so it only
compiles when `install` also exists.)

## CI changes

`.github/workflows/release-build.yml`
- **No `--no-default-features` here.** Existing artifacts are the self-update
  channel and stay feature-on.
- **Add a dual-config build check** so the feature-off path can't bit-rot — one
  cheap job (or a step in the existing lint job):
  ```bash
  cargo clippy --no-default-features --all-targets -- -D warnings
  ```
  (Optionally also `--all-features`, but with a single feature that's identical
  to the default build.)

`--no-default-features` lands in the **new** per-repo pipelines (COPR, Flatpak,
AUR, Store) when those are authored — out of scope here.

## Tests

**[fix]** Test modules are declared per-source-file
(`#[cfg(test)] #[path = "tests/<name>_tests.rs"] mod tests;`), so each rides with
its parent module automatically — no separate gating step:
- Gated parents auto-gate their tests: `manifest_tests` (in `manifest.rs`),
  `minisign_tests` (in `minisign.rs`), `install_tests` (declared from
  `install/mod.rs:182` via `#[path = "../tests/install_tests.rs"]`).
- Ungated detection tests stay compiled: `version_tests`, `linux_pkg_tests`,
  `probe_tests`, `target_tests`, `system_install_tests` (declared from their
  ungated source files). Three of them take the shared env helpers, which now
  live in `crate::test_support` at the crate root — outside the `self-update`
  gate by construction, so nothing here has to keep them out of it (see the note
  where the split table's `test_support` row used to be).
- `check`/`github`/`event`/`state`/`asset_cache` declare no test module today.

Run matrix:
- `cargo test` (default features) — full suite as today.
- `cargo test --no-default-features` — detection tests only, must pass.

## Verification checklist

- [ ] `cargo clippy --all-targets -- -D warnings` (feature on) — clean.
- [ ] `cargo clippy --no-default-features --all-targets -- -D warnings` — clean,
      no dead-code / unused-import warnings (watch the `updater_settings` import).
- [ ] `cargo build --no-default-features` then
      `cargo tree --no-default-features | grep -i minisign` → **empty**.
- [ ] (Optional) `cargo doc --no-default-features` — no broken intra-doc links
      after the doc cleanup above.
- [ ] Feature-on run: Settings → Updates section visible, check/install works,
      About shows the version.
- [ ] Feature-off run: Settings → Updates section absent, no-results predicate
      and trailing spacer still behave, About **still shows the version**, no
      daily-check task spawns (the `updater_daily: not spawning` log line is also
      absent because the whole block is gone), no panics.
- [ ] `cargo test` and `cargo test --no-default-features` both green.

## Risks / notes

- **Dead-code / unused-import drift** when feature off — mitigated by the
  dual-config clippy job. The one easy-to-miss site is the `UpdaterEvent` import
  split in `updater_settings.rs`.
- **About version regression** — guarded by keeping `install()` ungated; called
  out in the checklist.
- **Slint id-reference breakage** — avoided by the "gate inside, don't `if` the
  mount" approach above.
- **`install_target` vs `install_target_old`** — easy to gate the wrong one; the
  std-only `install_target()` must stay, the `install`-dependent
  `install_target_old()` (and its reaper) must go behind the flag.
- Binary-size win on feature-off builds is modest (`minisign-verify` is small)
  but the real value is reviewer-facing: the self-replace + network path is
  *gone*, not dormant.

## Follow-ups (separate efforts)

- Per-repo packaging using the feature-off build: AUR `PKGBUILD`, winget
  manifest (uses the feature-on MSI), COPR `.spec`, Flathub manifest +
  `<releases>` in `metainfo.xml`, Chocolatey, apt repo/PPA, Store MSIX.
- Optional sibling feature `desktop-self-deploy` to compile out
  `desktop_integration::refresh_user_install` for Flatpak/sandboxed builds
  (currently runtime-skipped on AppImage/RPM/DEB).
