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

The release matrix itself is `.claude/rules/updater.md` and the procedure that fires it is
`docs/RELEASING.md`; this is the gate around both and the obligations every artifact carries.
`release.yml` holds the shape of a release and calls `release-{prepare,build,publish}.yml`, each of
which argues itself; a filename below names whichever of the four owns the thing under discussion.

## The PR gate

`pr-validation.yml`, on PRs into `main`: `changes` (skip matrix) → `audit` ∥ `fmt` ∥ `clippy` ∥
`test` ∥ `clippy-windows` ∥ `test-windows`. Both `clippy` jobs are one step
(`--all-targets --locked --workspace -- -D warnings`); `test` is `cargo test --locked --workspace`.
All six hang off `changes` alone — chaining `test` behind `clippy` made the gate's wall clock their
sum, and what waits on it is a person deciding whether to merge. That is also why `audit` and `fmt`
stay siblings despite compiling nothing and finishing in seconds: as parents they would buy back
under half a minute of skipped work on the rare red run, and cost a whole round trip on it, since
neither an advisory hit nor a reflow would ever reach the lint and test results. `audit` is the
worst candidate of the two — it is the one job that reddens for reasons outside the PR, so as a
parent an overnight advisory would block every open PR's feedback rather than one check. No
coverage on this path.

- **The aggregate `pr-validation` job is the required status check, and `needs:` is the only list.**
  The check step reads `toJSON(needs)` through `env:` and asks `jq` for anything that is neither
  `success` nor `skipped`, so adding a job is one edit and the offenders get named. It used to be
  two, a `results=( … )` bash array restating `needs:`, and a job in only the first was silently
  unenforced. What `toJSON` cannot see is a job that never reached `needs:` at all — the aggregate
  then doesn't wait on it and reports green while it is still running — so
  `crates/melodia/tests/packaging.rs`'s `the_aggregate_waits_on_every_job_in_the_gate_workflow` walks the file for that.

- **The advisory scan is one policy with two callers.** `.github/actions/cargo-audit` (composite)
  holds the `taiki-e/install-action` pin and `cargo audit --deny unsound --deny unmaintained`;
  `pr-validation.yml` and `release.yml` both `uses:` it, neither spelling the flags. **It takes no
  inputs on purpose** — a `deny-level` knob is what would let the early warning reach a different
  verdict than the gate holding the release. No toolchain, no system deps (prebuilt binary over the
  committed `Cargo.lock`). Escape hatch is the documented `ignore` list in `.cargo/audit.toml`; a
  stale entry sits on the critical path of unrelated work, so the `revisit:` dates matter.

- **The skip matrix is a denylist, and `predicate-quantifier: 'every'` is what makes it work.**
  `dorny/paths-filter` gates the compiling jobs on `'**'` plus `!` exclusions, *not* a list of
  source globs — the gate counts `skipped` as a pass, so under an allowlist an unlisted path merges
  green with zero checks run. **Never drop the quantifier**: the default `'some'` makes a filter
  true when *any* pattern matches and every file matches `'**'`, so the exclusions would do nothing
  and the job would never skip. `fkirc/skip-duplicate-actions` catches the identical tree arriving
  twice and is given **no** `paths_filter`, so the two can't drift into a stale pass.

- **`.github/` is excluded per-file, never as `.github/**`.** `refresh-manifest`, `FUNDING.yml` and
  `ISSUE_TEMPLATE/` compile nothing and nothing reads them, so each is named
  (`pull_request_template.md` rides the `*.md` line). What stays in is what the gate runs or a test
  reads: the four composites `pr-validation.yml` uses, `pr-validation.yml` itself, and
  **`deploy-coverage.yml`, which used to be excluded and stopped qualifying the day
  `the_two_workflows_disagree_about_debug_info_on_purpose` began reading it**, the same move
  `LICENSE` made below; excluding it would leave that pin blind to the edit that breaks it.
  The release-only composites, the four `release*.yml` and `CODEOWNERS` stay in on the
  other argument, nothing exercising any of them before a merge: a release workflow first runs on
  a pushed tag, after review is over, and `CODEOWNERS` governs who may approve a workflow change.
  A control of that reach failing closed is worth one wasted run.
  Excluding a composite lands a broken provisioning step green; excluding `pr-validation.yml` lets
  a change to the clippy invocation or the job list merge without the jobs it governs ever running.
  Anything new under `.github/` runs everything by default — an exclusion is earned.

- **Headless audio** — `test` runs `crates/melodia/tests/headless.rs` and `AppState::init` opens cpal's default
  device, which no runner has. `.github/actions/headless-audio` is the whole shim and argues
  itself. It works because the shim is an *ALSA* one and `output::device` opens whatever
  `pcm.!default` names, then walks every config the device reports rather than giving up on the
  first — a stricter open fails that test looking like a scan bug. System libs come from
  `.github/actions/linux-system-deps`, which the release slots take as well, adding the
  packaging-only `rpm` through its `extra-packages` input: one base list, so a
  gate and a release cannot provision differently. `Swatinem/rust-cache` per job, `ci-*`
  shared-keys, distinct from release's `rust-release-*`. The action appends `os.type()` and
  `os.arch()` *after* the shared key, so each Linux/Windows pair passes one key (`ci-test`,
  `ci-clippy`) and still gets two entries — which is why neither Windows job spells a key of its
  own. It is also what makes `arch` redundant in release's key, and only because every slot builds
  on a runner of its own arch — a slot that ever cross-compiles wants it back.

- **The two Windows jobs are the only non-Linux ones, and between them they close a gap
  `release-build.yml` never touched.**
  It already compiles the Windows lib and bin on both arches, so the `cfg(windows)`
  arms are not uncompiled; they are compiled once a tag is pushed, after review is over, and
  **nothing has ever run the tests there.** That is what the `.gitattributes` LF pin, the
  joined-not-spelled path fixtures, `redact_home`'s `dirs::home_dir()` and `windows_swap`'s
  `MoveFileExW` all rest on today. A release build never passes `--cfg test` either, so a
  `cfg(windows)` test has never been type-checked, let alone run: `updater::install`'s four and
  the `all(test, target_os = "windows")` re-export written to feed them were authored blind. That
  is the bigger half of what this job unlocks. `services::platform::dwm_titlebar::is_dark_from_rgb`, the
  third copy of the luminance threshold, split on `lum < 0.5` where its two siblings split on
  `lum > 0.5` for *light*, so the caption disagreed with the chrome under it on every colour
  landing exactly on the threshold; the pin that holds it now is a `cfg(windows)` test, which is to
  say this job or nothing. **`clippy-windows` is the lint half, and the Linux job does not make it
  redundant**: clippy's verdict is a function of the code it type-checks, so everything compiling
  on both platforms already got its answer on Linux, but what `cfg` hides from Linux
  (`dwm_titlebar` is gated at its `pub mod`, so that file is not even parsed there) had never met
  clippy-driver anywhere. `release-build.yml` does not close that either — `cargo build` is rustc,
  and lib plus bin, so `[workspace.lints.clippy]` and its `unwrap_used = "deny"` reached no
  `cfg(windows)` arm and no test target on any platform. Nor does cross-checking from the Linux
  runner: `ring`, `aws-lc-sys`, `libsqlite3-sys` and `blake3` compile C, so
  `--target x86_64-pc-windows-msvc` wants an MSVC toolchain wherever it runs. It is a **sibling**
  of `test-windows` rather than a step inside it, for the reason `fmt` is a sibling of `clippy`,
  and it carries no `CARGO_BUILD_JOBS` cap because a check-only build never reaches the codegen
  peak that cap is sized against. **The headless test
  is skipped by name** (`-- --skip headless_scan_persists_track`), not by target selection and not
  by a `cfg_attr(windows, ignore)`: what is missing is the runner's audio endpoint rather than
  Windows, and a name filter leaves a *new* integration test running here by default, which target
  selection would not. **`windows-latest` here, against a pinned label in release**: the gate
  wants GitHub's rollovers, an image change being something that should redden a PR rather than a
  tag, and the other four jobs take that same bet on `ubuntu-latest`. The label release pins is
  `windows-2025-vs2026`, which is **not** the durable one — GitHub shipped it to test the Visual
  Studio 2026 migration and folded it onto `windows-2025` when that finished, so all three spell
  one image today and only `windows-2025` is documented to keep doing so. No system deps and so no
  `linux-system-deps` twin: cpal reaches WASAPI through `windows-sys` and Slint needs no package,
  which makes the shape `fmt`'s rather than `test`'s.

- **`CARGO_BUILD_JOBS: 4` on `test` and `test-windows` is a memory cap, not a CPU match — don't
  raise it to `.cargo/config.toml`'s dev-machine jobs=8.** The peak is a single `rustc` on
  **`melodia-ui`**, and the count scales *one* process's peak because its LLVM codegen threads draw
  jobserver tokens: 13.30 / 10.71 / 8.83 GiB at jobs=8/4/2 against a 16 GB runner. Over the ceiling
  nothing fails — it **swaps**, and a 14-minute job runs for most of an hour looking like a hang.
  jobs=4 leaves ~4 GB headroom; jobs=2 buys 1.9 GB more for +300 s. The Windows runner is the same
  4-core / 16 GB box, so the number carries; `release-build.yml` leaves its Windows slots at 8
  because a release build is not the shape that needed the cap. `clippy` is the third compiling
  job and sets nothing: it never codegens, so it doesn't reach that peak. **`RUST_TEST_THREADS` is
  deliberately not capped alongside it** — `.cargo/config.toml`'s 8 stands, oversubscribing a
  4-core runner 2:1, because the cap answers a memory ceiling and most harness threads are waiting
  rather than running. `crates/melodia/tests/crossfade.rs` is the exception, its thirteen tests each turning the
  mixer from a spin loop, and what that reached was a wait budgeted in frames rather than in wall
  clock: the budget then measures the puller's throughput, not the thing it waits for.
  `CONTROL_OP_BUDGET` is what replaced it, and `taskset -c 0` against a couple of dozen spinners
  reproduces the old shape off Windows. The tightest budgets left are `single_instance_tests`' two
  1 s `recv_timeout`s, tighter for standing up a real transport, and the first place to read if
  `test-windows` reddens.

- **`test` and `test-windows` cap build time as well as memory.** `cargo test` links 42 test
  binaries and full debuginfo is most of that tail, worst on MSVC where it is PDBs. Cold on both
  sides, cargo's own build phase reads 18m29s → 12m39s on Linux and 31m13s → 17m54s on Windows, so
  the MSVC half is where it pays. Both set `CARGO_PROFILE_{DEV,TEST}_DEBUG` to `line-tables-only`,
  both halves because `cargo test` uses both profiles, and not the `0` `deploy-coverage.yml` sets
  because this is the job whose backtraces get read. In the workflow rather than `[profile.dev]`,
  so local and release builds are untouched; `rust-cache` hashes `CARGO_*`, so a change here costs
  one cold run per platform. That cold pair is also the only place those numbers can come from:
  both `clippy` jobs dropped further still across the same two runs on cache warmth alone.

- **Four Windows levers that look obvious and are not**, checked so they aren't re-proposed. A
  Defender exclusion: the image already disables real-time monitoring and excludes both drives.
  `CARGO_INCREMENTAL: 0`: `rust-cache` sets it. Folding `clippy-windows` into `test-windows`: check
  and build units share nothing past the proc-macro and build-script ones, so one job compiles the
  graph twice in series where two do it twice in parallel. Capping `clippy-windows`' jobs: same
  reason it has no cap.

- **The two-phase build is gone.** Pre-crate-split the generated unit compiled twice per
  `cargo test` (rlib + `--test` harness) at 20.96 GiB concurrently, so both workflows carried a
  `--lib`-then-everything split; 10.71 GiB now fits in one command with margin. Cheap levers were
  measured **for memory** and none move it: `split-debuginfo=unpacked`, `debug=0` and dropping
  coverage instrumentation are ~0.5 GiB each, `codegen-units` does nothing across 16..4096 (worse
  at 1), cutting test binaries does nothing. The floor is `melodia-ui` itself (7.59 of the 10.71)
  — the next lever is a smaller generated unit, not another scheduling trick.

- **Coverage → Pages** — **off the PR path**: the instrumented build is the most expensive thing in
  this repo's CI and has OOM-killed the runner (143). `deploy-coverage.yml` runs `cargo llvm-cov`
  on **`workflow_dispatch` only**, then deploys the HTML to **GitHub Pages**
  (`https://kenansalar.github.io/Melodia/`) from a second job in the *same* run — no cross-workflow
  artifact fetch. **`CARGO_PROFILE_{DEV,TEST}_DEBUG: "0"`** there, not `line-tables-only`: LLVM
  source-based coverage carries its own file/region table in `__llvm_covmap` and reads nothing from
  DWARF, and nothing reads a backtrace out of that job (the gate's two `test` jobs keep
  `line-tables-only` for the opposite reason, and
  `crates/melodia/tests/packaging.rs`'s `the_two_workflows_disagree_about_debug_info_on_purpose` holds the pair apart,
  tidying them into agreement reddening nothing and silently costing the gate its `file:line`).
  Both `report` calls pass **`--ignore-filename-regex`** scoped to
  `melodia-ui`'s `OUT_DIR` — cargo-llvm-cov's built-in excludes reach the sysroot and registry but
  not a workspace member's generated code, and `app-window.rs` would swamp the denominator. **The
  dispatch takes a ref**, so the dropdown picks what gets measured and runs that branch's copy; the
  button exists only because the workflow sits on `main`. Two one-time settings: **Pages enabled**
  (Source = GitHub Actions) **and** the **`github-pages` environment's** branch allowlist,
  currently `main` alone — dispatching from elsewhere builds and uploads, then fails at `deploy`.

- **Every action is SHA-pinned** with a trailing `# vX.Y.Z` comment — no floating tags, no
  `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24` (every pin resolves to `node24` or a composite over one);
  re-pin with `gh api repos/OWNER/REPO/commits/TAG -q .sha`. Anything under `.github/workflows/` is
  CODEOWNER-protected (`@KenanSalar`). **Every job that compiles goes through
  `.github/actions/setup-rust`**, which installs the pin from the file and argues why there;
  what that leaves here is the half it can't reach, `rust-toolchain.toml`'s `components` list,
  where clippy and `llvm-tools-preview` have to be named or they silently go missing.

## File associations — five Linux spellings and one Windows one, all pinned

The `MimeType=` list makes Melodia *offerable*; the `Exec=` field code is what makes the handoff
work, and the two shipped apart for a whole release.

- **All four `.desktop` sources end `Exec=` with ` %F`** (`scripts/Melodia.desktop`, the two build
  heredocs, `assets/desktop/Melodia.desktop.tmpl`). Not `%U`, a list of *URLs* the spec lets arrive
  as `file://` and so obliges percent-decoding; not `%f`, one process per selected file.
  `all_desktop_sources_agree_on_mime_and_wmclass` matches **extracted lines**, every one of them —
  two sources are whole shell scripts, where a comment about the line reads exactly like the line,
  and a second heredoc is what checking only the first would walk past.

- **`scripts/install-linux.sh` is the fifth, and it is a rewriter.** It seds the `Exec=` of the
  same file the DEB ships verbatim; anchored at `.*` it ate the field code too, so `%F` lived in
  the DEB and died in the tarball. `s|^Exec=[^ ]*|` takes the command token only, pinned both ways
  by `the_tarball_installer_rewrite_keeps_the_field_code`. Its quoting arm answers the same
  question as `desktop_integration::render_desktop`'s — Exec is parsed with shell-like quoting, so
  a path with a space needs them, **only when it does** — but not with the same predicate: the
  shell `case` tests for a space, `quote_exec` for the spec's whole reserved set and its two
  unescaping layers. Deliberate; a `$` or a backtick in `$HOME` doesn't earn shell-quoting
  gymnastics in an installer.

- **Windows is `crates/melodia/wix/main.wxs`'s `FileAssociations` component, plain `RegistryValue` rows.** WiX's
  `ProgId`/`Extension`/`Verb` predate Vista: no `Capabilities` + `RegisteredApplications` (what
  Win10/11 reads for Default apps), and `Extension` claims `HKCR\.mp3`'s default outright.
  **`ApplicationDescription` is required** — without it the app is absent from that list and every
  key under it unreachable. `MultiSelectModel=Player` hands a whole selection to one invocation.
  `the_msi_offers_every_audio_extension` walks `utils::audio_ext::AUDIO_EXTENSIONS` against the
  comment-stripped wxs, the one format no Linux runner builds.

## Licences — every format ships `licenses/`, and the five spellings are pinned by name

The two fonts and the vendored winit fork compile *into* the binary, so each artifact redistributes
them and owes the licence text (Apache-2.0 §4(a); SIL's OFL FAQ recommends it for a bundled font).
Five formats, five toolchains, one an MSI no Linux runner can build — so a format that quietly
stops shipping the text fails nowhere until a packager files it.
**`crates/melodia/tests/packaging.rs`'s `every_package_format_ships_the_licenses_dir` holds a named list**
(`build-rpm.sh`'s `%license`, the binary manifest's asset glob, `build-tarball.sh`'s `cp`,
`build-appimage.sh`'s `cp`, `main.wxs`'s `File` set), each needle the *mechanism* rather than
the word. Named because the set of formats is closed; the *font* set is open, so its sibling pin
walks the directory instead.

- **Four of the five glob the directory and WiX does not**, so a fourth licence file is free
  everywhere except `main.wxs`. `the_msi_names_every_licence_file` **walks `licenses/`** and fails
  on any file the wxs doesn't name. The two aren't redundant: the named list catches the MSI
  dropping the directory (a deliberate act), the walk catches a file going missing from it (an
  omission).

- **The RPM is in that four only because `%license` takes `licenses/*`** — spell the three out and
  Fedora acquires WiX's per-file edit without WiX's excuse. It is also the one format whose needle
  is not the statement that stages the files: `build-rpm.sh` copies them into the source tarball
  *and* names them in `%files`, and only the second ships anything, so the pin reads the `%license`
  line (delete it and the staged copy goes unread, with no warning anywhere).

- **`packaging/debian-copyright` is copied verbatim only because it opens with a DEP-5 key** —
  cargo-deb's `has_copyright_metadata` scans the first ten lines, and without one it *generates* a
  copyright from `license` + `authors`, declaring the whole package AGPL by one author, which the
  fonts and winit falsify. **Hence `release-build.yml` pins cargo-deb to an exact version**: that
  check and the bare-string `license-file` spelling both live in its `config.rs`, not its README,
  so a bump can narrow either with nothing to say so, and no test can see it — bumping means
  re-reading `has_copyright_metadata`, not editing a number. Policy 12.5 lets a package reference
  `/usr/share/common-licenses` only for what ships there, so the AGPL and OFL bodies are
  **quoted**, and `the_debian_copyright_quotes_the_licences_it_ships` re-derives both from their
  sources rather than trusting the copy.

- **`LICENSE` is an input to those tests** — hence its absence from the skip denylist above:
  compiling nothing is not the same as being unexercised. The other four needles live in
  `scripts/`, `crates/melodia/Cargo.toml` and `crates/melodia/wix/`, which the denylist never
  reaches.

- **A face added under `crates/melodia-ui/ui/assets/fonts/` owes an entry in
  `licenses/ATTRIBUTION.txt`** — `every_bundled_font_is_named_in_the_attribution` walks the
  directory rather than listing the faces, keying on each face's repo-relative path, the family
  name not being derivable from the file.

## The workspace split, as the release workflow sees it

**Before adding a member, grep the `release*.yml` and the packaging scripts for `melodia`.**
The split to `melodia` + `melodia-ui` broke that workflow twice, both times on something
identifying a thing by *name* that was unambiguous with one member, and both deep in a matrix slot:

- `cargo wix` picks a package for you only when the workspace has exactly *one*, so the MSI step
  names `--package melodia`. It selects the *manifest*, and cargo-wix then reads `wix/main.wxs`
  relative to that, which is why the wxs sits under `crates/melodia/` rather than at the repo root
  behind an `-I`: that flag's resolution base is undocumented and nothing short of a tagged Windows
  release would exercise it.

- The artifact upload's `path:` was a bare **`melodia-*`**, a prefix glob matching what was then a
  `melodia-ui/` **directory** at the repo root — all ten slots uploaded the Slint source tree and
  `gh release upload artifacts/*` died on "is a directory". Now extension-qualified like the attest
  and sign steps, **the third spelling of one list**. C13 moved that directory under `crates/`, so
  the collision is gone and the qualifier is what keeps the next one from landing.

Four places scrape the version out of the manifest rather than asking cargo, because they run
without a toolchain (`release-prepare.yml`, `build-rpm.sh`, `build-appimage.sh`,
`build-tarball.sh`); all four anchor on `[workspace.package]` by name, and every member carries
`version.workspace = true`.

**A toolchain bump moves four things in lockstep** — `rust-toolchain.toml`, `Cargo.toml`'s
`[workspace.package] rust-version`, `clippy.toml`'s `msrv` (an explicit clippy msrv *overrides* the
Cargo one), and the two docs (`README.md` prerequisites + the root `CLAUDE.md`) — and should expect
fresh `pedantic`/`style` lints on the first run.
