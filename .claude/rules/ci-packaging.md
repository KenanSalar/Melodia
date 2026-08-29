---
paths:
  - .github/**
  - .github/workflows/*.yml
  - .github/actions/**
  - packaging/**
  - licenses/**
  - wix/main.wxs
  - scripts/build-rpm.sh
  - scripts/build-appimage.sh
  - scripts/install-linux.sh
  - scripts/build-latest-json.py
  - Cargo.toml
  - rust-toolchain.toml
  - clippy.toml
  - .cargo/audit.toml
---

# CI and packaging

The release matrix itself is `.claude/rules/updater.md` and the procedure that fires it is
`docs/RELEASING.md`; this is the gate around both and the obligations every artifact carries.

## The PR gate

`pr-validation.yml`, on PRs into `main`: `changes` (skip matrix) → `audit` ∥ `fmt` ∥ `clippy` ∥
`test` ∥ `test-windows`. `clippy` is one step (`--all-targets --locked -- -D warnings`, both
packages); `test` is plain `cargo test --locked`. All five hang off `changes` alone — chaining
`test` behind `clippy` made the gate's wall clock their sum, and what waits on it is a person
deciding whether to merge. `audit` and `fmt` compile nothing, so neither an advisory hit nor a
reflow hides the lint and test results. No coverage on this path.

- **The aggregate `pr-validation` job is the required status check, and adding a job to it is two
  edits**: `needs:` *and* the `results=( … )` bash array the check step loops over. No
  `toJSON(needs)` fallback, so a job named in only the first is silently unenforced.

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

- **`.github/` is excluded per-file, never as `.github/**`.** `deploy-coverage`,
  `refresh-manifest`, `FUNDING.yml` and `ISSUE_TEMPLATE/` compile nothing and nothing reads them,
  so each is named (`pull_request_template.md` rides the `*.md` line). Four exercised paths stay
  in: `.github/actions/{linux-system-deps,cargo-audit}`, `pr-validation.yml` itself, and
  `release.yml` — which compiles nothing but **is read by `test`** (the licence pins below).
  `CODEOWNERS` stays in as well and is exercised by nothing, so a PR touching only it pays the
  full gate; it governs who may approve a workflow change, and a control of that reach failing
  closed is worth one wasted run.
  Excluding a composite lands a broken provisioning step green; excluding `pr-validation.yml` lets
  a change to the clippy invocation, the job list or the `results` array merge without the jobs it
  governs ever running. Anything new under `.github/` runs everything by default — an exclusion is
  earned.

- **Headless audio** — `test` runs `tests/headless.rs` and `AppState::init` opens rodio's default
  device. GitHub's Azure runner ships **no `snd-dummy`/`snd-aloop`**, so CI points ALSA's default
  PCM at alsa-lib's built-in userspace `null` device via `/etc/asound.conf`
  (`pcm.!default { type null }`) — no kernel module, no extra package. System libs come from
  `.github/actions/linux-system-deps` (composite: Azure apt-mirror swap + retrying install of the
  Slint/ALSA/Wayland/D-Bus set). `Swatinem/rust-cache` per job, `ci-*` shared-keys, distinct from
  release's `rust-release-*`. The action appends `os.type()` and `os.arch()` *after* the shared
  key, so `test` and `test-windows` both pass `ci-test` and still get their own entry.

- **`test-windows` is the one non-Linux job, and it runs the suite rather than linting it.**
  `release.yml` already compiles the Windows lib and bin on both arches, so the `cfg(windows)`
  arms are not uncompiled; they are compiled once a tag is pushed, after review is over, and
  **nothing has ever run the tests there.** That is what the `.gitattributes` LF pin, the
  joined-not-spelled path fixtures, `redact_home`'s `dirs::home_dir()` and `windows_swap`'s
  `MoveFileExW` all rest on today. **No Windows clippy twin**: clippy's verdict is a function of
  the code it type-checks, so everything compiling on both platforms already got its answer from
  the Linux job, and what `cfg` hides from Linux (`services::dwm_titlebar` is gated at its
  `pub mod`, so that file is not even parsed there) is FFI glue `release.yml` compiles anyway.
  Cross-checking from the Linux runner is no substitute either: `ring`, `aws-lc-sys`,
  `libsqlite3-sys` and `blake3` compile C, so `--target x86_64-pc-windows-msvc` wants an MSVC
  toolchain wherever it runs. **The headless test is skipped by name**
  (`-- --skip headless_scan_persists_track`), not by target selection and not by a
  `cfg_attr(windows, ignore)`: what is missing is the runner's audio endpoint rather than Windows,
  and a name filter leaves a *new* integration test running here by default, which target
  selection would not. `windows-latest` rather than release's `windows-2025-vs2026`, whose
  explicit label buys the preinstalled WiX this job has no use for. No system deps and so no
  `linux-system-deps` twin: cpal reaches WASAPI through `windows-sys` and Slint needs no package,
  which makes the shape `fmt`'s rather than `test`'s.

- **`CARGO_BUILD_JOBS: 4` on `test` and `test-windows` is a memory cap, not a CPU match — don't
  raise it to `.cargo/config.toml`'s dev-machine jobs=8.** The peak is a single `rustc` on
  **`melodia-ui`**, and the count scales *one* process's peak because its LLVM codegen threads draw
  jobserver tokens: 13.30 / 10.71 / 8.83 GiB at jobs=8/4/2 against a 16 GB runner. Over the ceiling
  nothing fails — it **swaps**, and a 14-minute job runs for most of an hour looking like a hang.
  jobs=4 leaves ~4 GB headroom; jobs=2 buys 1.9 GB more for +300 s. The Windows runner is the same
  4-core / 16 GB box, so the number carries; `release.yml` leaves its Windows slots at 8 because a
  release build is not the shape that needed the cap. `clippy` is the third compiling job and sets
  nothing: it never codegens, so it doesn't reach that peak.

- **The two-phase build is gone.** Pre-crate-split the generated unit compiled twice per
  `cargo test` (rlib + `--test` harness) at 20.96 GiB concurrently, so both workflows carried a
  `--lib`-then-everything split; 10.71 GiB now fits in one command with margin. Cheap levers were
  measured and **none** move it: `split-debuginfo=unpacked`, `debug=0` and dropping coverage
  instrumentation are ~0.5 GiB each, `codegen-units` does nothing across 16..4096 (worse at 1),
  cutting test binaries does nothing. The floor is `melodia-ui` itself (7.59 of the 10.71) — the
  next lever is a smaller generated unit, not another scheduling trick.

- **Coverage → Pages** — **off the PR path**: the instrumented build is the most expensive thing in
  this repo's CI and has OOM-killed the runner (143). `deploy-coverage.yml` runs `cargo llvm-cov`
  on **`workflow_dispatch` only**, then deploys the HTML to **GitHub Pages**
  (`https://kenansalar.github.io/Melodia/`) from a second job in the *same* run — no cross-workflow
  artifact fetch. **`CARGO_PROFILE_{DEV,TEST}_DEBUG: "0"`** there, not `line-tables-only`: LLVM
  source-based coverage carries its own file/region table in `__llvm_covmap` and reads nothing from
  DWARF, and nothing reads a backtrace out of that job (the PR `test` job keeps default debug info
  for the opposite reason). Both `report` calls pass **`--ignore-filename-regex`** scoped to
  `melodia-ui`'s `OUT_DIR` — cargo-llvm-cov's built-in excludes reach the sysroot and registry but
  not a workspace member's generated code, and `app-window.rs` would swamp the denominator. **The
  dispatch takes a ref**, so the dropdown picks what gets measured and runs that branch's copy; the
  button exists only because the workflow sits on `main`. Two one-time settings: **Pages enabled**
  (Source = GitHub Actions) **and** the **`github-pages` environment's** branch allowlist,
  currently `main` alone — dispatching from elsewhere builds and uploads, then fails at `deploy`.

- **Every action is SHA-pinned** with a trailing `# vX.Y.Z` comment — no floating tags, no
  `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24` (every pin resolves to `node24` or a composite over one);
  re-pin with `gh api repos/OWNER/REPO/commits/TAG -q .sha`. Anything under `.github/workflows/` is
  CODEOWNER-protected (`@KenanSalar`). **The toolchain pin is installed *from the file***: a bare
  `rustup toolchain install`, no `dtolnay/rust-toolchain`, whose `rustup default` outranks it —
  components belong in the file's `components` list, else clippy and `llvm-tools-preview` silently
  go missing.

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

- **Windows is `wix/main.wxs`'s `FileAssociations` component, plain `RegistryValue` rows.** WiX's
  `ProgId`/`Extension`/`Verb` predate Vista: no `Capabilities` + `RegisteredApplications` (what
  Win10/11 reads for Default apps), and `Extension` claims `HKCR\.mp3`'s default outright.
  **`ApplicationDescription` is required** — without it the app is absent from that list and every
  key under it unreachable. `MultiSelectModel=Player` hands a whole selection to one invocation.
  `the_msi_offers_every_audio_extension` walks `media::AUDIO_EXTENSIONS` against the
  comment-stripped wxs, the one format no Linux runner builds.

## Licences — every format ships `licenses/`, and the five spellings are pinned by name

The two fonts and the vendored winit fork compile *into* the binary, so each artifact redistributes
them and owes the licence text (Apache-2.0 §4(a); SIL's OFL FAQ recommends it for a bundled font).
Five formats, five toolchains, one an MSI no Linux runner can build — so a format that quietly
stops shipping the text fails nowhere until a packager files it.
**`services::tests::every_package_format_ships_the_licenses_dir` holds a named list**
(`build-rpm.sh`'s `%license`, `Cargo.toml`'s asset glob, `release.yml`'s `cp`,
`build-appimage.sh`'s `cp`, `wix/main.wxs`'s `File` set), each needle the *mechanism* rather than
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
  fonts and winit falsify. **Hence `release.yml` pins cargo-deb to an exact version**: that check
  and the bare-string `license-file` spelling both live in its `config.rs`, not its README, so a
  bump can narrow either with nothing to say so, and no test can see it — bumping means re-reading
  `has_copyright_metadata`, not editing a number. Policy 12.5 lets a package reference
  `/usr/share/common-licenses` only for what ships there, so the AGPL and OFL bodies are
  **quoted**, and `the_debian_copyright_quotes_the_licences_it_ships` re-derives both from their
  sources rather than trusting the copy.

- **`release.yml` and `LICENSE` are inputs to those tests** — hence their absence from the skip
  denylist above: compiling nothing is not the same as being unexercised.

- **A face added under `melodia-ui/ui/assets/fonts/` owes an entry in
  `licenses/ATTRIBUTION.txt`** — `every_bundled_font_is_named_in_the_attribution` walks the
  directory rather than listing the faces, keying on each face's repo-relative path, the family
  name not being derivable from the file.

## The workspace split, as the release workflow sees it

**Before adding a third member, grep `release.yml` and the packaging scripts for `melodia`.** The
split to `melodia` + `melodia-ui` broke that workflow twice, both times on something identifying a
thing by *name* that was unambiguous with one member, and both deep in a matrix slot:

- `cargo wix` picks a package for you only when the workspace has exactly *one*, so the MSI step
  now names `--package Melodia`.

- The artifact upload's `path:` was a bare **`melodia-*`**, a prefix glob matching the new
  `melodia-ui/` **directory** — all ten slots uploaded the Slint source tree and
  `gh release upload artifacts/*` died on "is a directory". Now extension-qualified like the attest
  and sign steps, **the third spelling of one list**.

Three places scrape the version out of the manifest rather than asking cargo, because they run
without a toolchain (`release.yml`'s `prepare`, `build-rpm.sh`, `build-appimage.sh`); all three
anchor on `[workspace.package]` by name, and both packages carry `version.workspace = true`.

**A toolchain bump moves four things in lockstep** — `rust-toolchain.toml`, `Cargo.toml`'s
`[workspace.package] rust-version`, `clippy.toml`'s `msrv` (an explicit clippy msrv *overrides* the
Cargo one), and the two docs (`README.md` prerequisites + the root `CLAUDE.md`) — and should expect
fresh `pedantic`/`style` lints on the first run.
