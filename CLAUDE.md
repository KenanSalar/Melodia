# CLAUDE.md

Guidance for Claude Code in this repo. Cross-platform desktop music player: **Slint 1.16** UI + pure-Rust backend, direct calls + tokio channels (no WebView/IPC).

## Where the docs live

This file holds what applies repo-wide. The subsystem contracts load on demand — one
`CLAUDE.md` sits beside the code it governs, the rest are keyed to globs — so read the
relevant one before changing anything it covers, and **put new subsystem detail there
rather than here.**

| file | covers |
|------|--------|
| `src/player/CLAUDE.md` | playback state machine, DSP chain (EQ / ReplayGain / limiter), crossfade + decks, visualizer tap |
| `.claude/rules/library-data.md` | scan ingest, the two change channels, track projections, ratings, tag write-through, M3U8, smart playlists |
| `.claude/rules/ui-patterns.md` | the shared components (pills, pickers, grids, toasts), cover prewarm, the two teardown paths, shortcuts |
| `.claude/rules/desktop-shell.md` | window chrome + winit, tray, media keys, always-on-top, force-exit shutdown |
| `.claude/rules/scrobbling.md` | Last.fm + ListenBrainz, the durable queue, love↔favorite sync, MBID auto-tagging |
| `.claude/rules/discord.md` | Rich Presence — the pure model, the hand-rolled IPC, external artwork URLs |
| `.claude/rules/updater.md` | install methods, download/resume, manifest signing, the release matrix |
| `.claude/rules/visualizer.md` | the visualizer's UI half — arming, the tick's gates, the strip and its pickers |
| `.claude/rules/*.md` (rest) | per-crate best practices (tokio, sqlx, slint, rodio-symphonia, lofty, rayon, serde, blake3, rust-performance) plus `slint-pitfalls` |

Every one is **path-scoped** by a `paths:` frontmatter glob and loads when Claude *reads* a
matching file — a grep hit or a clippy failure won't pull one in. That asymmetry decides what
belongs where: **descriptive** detail (how a subsystem is shaped) can live in a rule, because
you'll be reading those files anyway; a **prohibition** you could violate from outside the
directory stays here. That's why "no `unwrap`", the zbus footgun and the `--version` contract
sit in this file even though each is subsystem-flavoured.

A subsystem gets a `CLAUDE.md` when it **is** a directory, and a `.claude/rules/` entry when it
cuts across several. UI features always cut across: `.slint` lives under `ui/` and its Rust under
`src/ui/`, so a per-directory file would reach one tree and silently miss the other — which is why
the visualizer's UI half is `.claude/rules/visualizer.md` and not a file beside its Rust.

Only this file is re-injected after `/compact`; the nested ones come back on the next read in
their directory, so a long session can lose one without saying so.

Module `//!` docs are the third tier and usually the most current: `player/{visualizer,spectrum,waveform}.rs`
and `ui/visualizer/{mod,pulse,frame}.rs` argue their own designs, and tuning constants are justified at
their definitions. Don't restate them here — link instead.

## Build & Dev Commands

```bash
cargo run                                  # debug
cargo build --release && target/release/Melodia
cargo clippy --all-targets -- -D warnings  # lint + check (don't run cargo check)
cargo test                                 # no doctests — `[lib] doctest = false`
cargo llvm-cov --html                      # coverage → target/llvm-cov/html/
/usr/bin/time -v target/release/Melodia    # peak RSS (release only)
```

## Prerequisites

- **Rust** edition 2024, pinned to 1.97.0 by `rust-toolchain.toml` (rustup fetches on first `cargo` call).
- **Linux**: GTK / X11 / Wayland dev pkgs for Slint femtovg (`libfontconfig1-dev`, `libfreetype6-dev`, Vulkan/OpenGL; `libwayland-dev` on Wayland). No WebKitGTK.
- **macOS / Windows**: no extra deps.

## Architecture

### State & Playback Flow

- **Library API** (`src/library/`): pure Rust `pub async fn`s; return `Result<T, AppError>`.
- **PlayerState** (`src/player/state.rs`): state machine behind `std::sync::Mutex`. `with_state_emit()`: mutates → builds VM → drops lock → publishes on `watch` → returns `Vec<PlayerAction>` for `execute_actions()`. **`emit_and_execute()`** (`player/actions.rs`) is the serialized wrapper for the `with_state_emit`+`execute_actions` pair: holds a per-handle `exec_lock` across **both** halves so the side-effect phase can't interleave across tasks (mutation order == side-effect order, closing a cross-task TOCTOU). Every paired call site uses it (or the `PlaybackContext::emit_and_execute` forwarder); a bare `with_state_emit`+`execute_actions` pair is a bug.
- **ViewModel propagation** — three `tokio::sync::watch`: `view_model_tx` (state minus queue), `queue_tx`, `position_tx` (500 ms). Full `to_view_model()` is `#[cfg(test)]`-only; prod uses `to_view_model_light` + `to_queue_view_model` (no per-tick queue clone).
- **Bridge** (`src/ui/bridge.rs`): `slint::spawn_local(async_compat::Compat::new(...))` futures subscribe + write Slint properties on UI thread.
- **PlayerAction** enum: side effects after lock drops (`PlayMedia`, `Pause`, `Seek`, `SetVolume`, `PreloadGapless`, `UpdatePlayCount`, `SaveQueue`, …).
- **RodioPlayer**: wraps Rodio `Player`. Speakers `Box::leak`'d in `main.rs` for `'static` — `MixerDeviceSink` must outlive all `Player`s.

### Threading Rules

- Slint event loop runs on **main thread** only. Never block it.
- One **tokio multi-thread runtime** in `main.rs`, shared as `Arc<Runtime>`.
- **UI → backend**: callbacks `runtime.spawn(...)`, push results via channels.
- **Backend → UI** (never `ui.set_*` from background): (1) `slint::spawn_local(async_compat::Compat::new(...))` UI-thread task that `.await`s tokio futures — **preferred** for reactive loops; (2) `slint::invoke_from_event_loop(...)` fire-and-forget; (3) `weak_handle.upgrade_in_event_loop(|ui| …)` auto-handles dropped UI.
- **Models from background**: `upgrade_in_event_loop` + `as_any().downcast_ref::<VecModel<T>>()`.

### Module Map

Non-obvious wiring only — read the code for file roles. Directories carrying their own `CLAUDE.md` (`src/player/`, `src/ui/visualizer/`) keep their contracts there; those load when you touch the directory, so put subsystem detail in them rather than growing this file.

- `main.rs`: arena cap → runtime → `AppState::init` → `boot::*` → `app.show()` + `slint::run_event_loop_until_quit()` (stays alive while close-to-tray hides the window) → `shutdown::*` → `process::exit(0)`.
- `state/` — `AppState`; `PlaybackContext` via `state.playback_ctx()`. `error.rs` = `AppError` (thiserror).
- `database/` — SQLx + SQLite (WAL, two-pool R/W, `sqlx::migrate!`, FTS5).
- `media/` — scanner (Rayon), metadata (Lofty 0.24), artwork; **`cover_thumbs.rs`** path-keyed RGB8 LRU → `slint::Image`+`SharedPixelBuffer` (row 72/grid 448/detail 384 px); **`image_decode.rs`** (`decode_capped(path, max_dim)` + the canonical `MAX_SOURCE_DIM` forged-header guard — the sole bounded-decode preamble, used by `cover_thumbs` and every cover path in `ui/*`, and by Material You with its own smaller cap; `tag_writer` is the one exception — it decodes a picked cover from *memory*, so it builds its own reader and takes only the bound, via `capped_limits`); **watcher** (notify + debouncer → tokio mpsc).
- `library/` — playback, queue, tracks, albums, artists, genres, playlists, search, `settings/`, browse, import, window. `playback::*` takes `&PlaybackContext`.
- `player/` — `state.rs` (state machine + `PlayerAction`), `actions.rs` (`execute_actions`/`emit_and_execute`), `handlers.rs` (playback monitor; per-tick decision is the pure, backend-free-testable `evaluate_playing_tick`), `rodio_backend.rs` (`RodioPlayer`), `decks.rs`, `equalizer.rs`/`replaygain.rs`/`crossfade.rs` (DSP), `visualizer.rs`/`spectrum.rs`/`waveform.rs`, `queue.rs`. **Contracts live in `src/player/CLAUDE.md`.** `dsp.rs` is the shared `Generation` poll counter behind `EqShared`/`ReplayGainShared`/`FadeShared` plus the numeric primitives more than one DSP module needs (`db_to_linear`/`linear_to_db`, `index_to_f32`, `VISUALIZER_DECAY`) — put a cast or constant there rather than a second copy in a sibling.
- `tasks/` — `playback_monitor`, `file_event_processor`, **`queue_prune`** (subs `library_changed_tx`; prunes via `QueueState::prune_missing` inside `with_state_emit`; auto-skips pruned playing-track), `retroactive_hash` (BLAKE3 backfill), **`material_you`** (subs view_model + appearance kick; coalesces; `spawn_blocking` extract+generate; publishes `watch::Sender<SystemColorState>` to `ui::appearance` — `tasks/` imports no `ui::*`). `spawner.rs` = `TaskSpawner`.
- `themes/` — pluggable registry. `apply(...)` writes 23 brushes. `"system"` resolves via `system_{dark,light}_variant`; KDE Breeze re-sources `~/.config/kdeglobals` via `palette_from_kde()`. Material You wins when `system.material_you = Some(...)`; synthetic `MATERIAL_YOU_ACCENT_ID` follows dynamic primary, `last_static_accent` remembers last non-MY pick. Non-Catppuccin themes fold via `..Palette::fallback_semantics(overlay1)`.
- `ui/` — `bridge`, `icons`, **`notifications.rs`** (`VecModel<NotificationRow>` toast stack, cap 5), **`now_playing_artwork.rs`** (size-8 LRU → `(cover, blur)` `ArtworkPair` from one decode), **`detail_artwork.rs`** (Album-Detail size-12 sibling; released on `close_detail` via `AlbumsUi::release_detail_artwork`).
- `ui/callbacks/` — `wire_all` + `Nav` persist; macros `wire_sync!`/`spawn_logged!`/`wire_pb!`/`wire_sync_pb!`.
- `ui/appearance/` — `Arc<RwLock<SystemColorState>>`, MY `kick_tx`/`repaint_tx`, `PersistedAccent` shadow.
- `ui/window_chrome/` — install + AOT + maximize seed + `RESPAWN_AFTER_EXIT`; `drop_coalescer`; `winit_filter` (drag-window intercept + DnD routing). Hydrates `Theme.use-native-titlebar` *before* `app.run()`.
- `ui/now_playing/` — `pub(crate) write_crossfade_slot`; **`up_next`** subs `sinks.queue`, gated on `Nav.now-playing-open` (closed ⇒ stash snapshot, open ⇒ rebuild skipped if visible id slice unchanged).
- `ui/{albums,browse,tracks,queue_sheet}/` — per-view `Ui` handle. **`AlbumsUi`** releases grid covers on `open_album`, detail pair on `close_detail`. **`BrowseUi`** uses a stale-fetch token + library-folder-rooted breadcrumbs. **`TracksUi`**: like the entity grids, its refreshers are **visibility-gated** (hidden ⇒ mark dirty; re-enter ⇒ one deferred re-fetch) so background `library_changed` bumps don't rebuild a hidden list. **`QueueSheetHandles`**: two-phase open + epoch-guarded teardown.
- `ui/recently_played/` (routing nav index 8) — **`RecentlyPlayedUi`**, near-mirror of `FavoritesUi` minus the Favorite-Artists strip: Favorites-style hero mosaic → non-collapsible "Most Played" strip (`get_most_played`) → filterable `TrackList` of the 200 most-recently-played rows (`get_recently_played`, `last_played DESC`; index `idx_tracks_last_played`, migration `20260705000000`). Membership is fixed — search + re-sort re-walk the cached `tracks_all` **in memory**, never re-querying. 2nd subscriber to `stats_changed_tx`. **Sidebar placement:** the item keeps routing `index: 8` but sits directly under Favorites in `sidebar.slint` — visual order follows source order, not the index value. Settings occupies nav index 9.
- Requires `unstable-winit-030` on `slint`.

### UI Structure (Slint)

- `ui/app-window.slint` — root `Window`. Rounded mantle Rectangle wraps `VerticalLayout { CustomTitleBar?; HorizontalLayout { Sidebar; ContentArea; } NowPlayingBar; }`. Resize ring (4 edges + 4 corner `TouchArea`) gated on `!use-native-titlebar && !is-maximized`. `no-frame: !use-native-titlebar`; `resize-border-width: 4px` **must equal** edge-overlay thickness. Full-screen `NowPlayingView` replaces content panel when `Nav.now-playing-open`.
- `ui/theme.slint` — `Theme` global: 23 brushes (`in-out` for repaint); layout/typography/motion `out`. In-out non-brush: `use-native-titlebar`, `window-focused`, `shell-radius`, `native-content-radius`. Derived `shell-radius-inner = max(0px, shell-radius - 1px)`. Semantic tokens (`danger`/`danger-hover`/`danger-text`).
- `ui/settings.slint` — `Settings` global: theme/variant/accent lists+indices + dynamic-mode flags + per-row toggles. Owned by Rust.
- `ui/models.slint` — boundary structs (`TrackRow`, `AlbumRow`, `ArtistRow`, `GenreRow`, `PlaylistRow`, `PositionTick`, `PlayerViewModel`). **Mirror exactly in Rust.**
- `ui/layout/`, `ui/views/`, `ui/components/` (incl. `dialog` driven by `Dialog` global with `kind`+`target-id` routing).

## Important Conventions

- **Rust Edition 2024** — `gen` is reserved.
- **Lock discipline** — release `PlayerState` lock before side effects; `with_state_emit()` enforces. The follow-on `execute_actions` is serialized across tasks by `emit_and_execute`'s `exec_lock` (`std::sync::Mutex<()>` on `PlayerStateHandle`, held across mutate+execute, never across `.await`). Lock ordering is `exec_lock → PlayerState → rodio Player`, never reversed.
- **`PlaybackContext` for `library::playback::*`** (not `&AppState`) — `state.playback_ctx()` (cheap, five `Arc::clone`s); fields `player_state`/`sinks`/`rodio`/`db`/`paths`. Owned `Arc`s dodge lifetime issues in `async move`. Wire via `wire_pb!`/`wire_sync_pb!`. Don't propagate elsewhere in `library/*`.
- **`TaskSpawner` for `tasks/*::spawn`** — `(TaskTracker, CancellationToken)` bundle in `src/tasks/spawner.rs`. `TaskSpawner::from_state(&state)`; pass `&spawner`. `spawn(fut)` fire-and-forget; `spawn_cancellable(|shutdown| …)` for shutdown loops (loop + `select!` on `shutdown.cancelled()`).
- **The audio pipeline documents itself in `src/player/CLAUDE.md`** — the playback machine (gapless, position polling, Symphonia config, the volume ceiling, speed/position timelines, stale-playback skips), the DSP chain (ReplayGain → EQ bands → limiter → crossfade ramp → visualizer tap, all inside one `EqSource`), and the crossfade/deck contracts. The visualizer's UI half is `.claude/rules/visualizer.md`. Both load when you read the files they govern; don't re-home their contents here.
- **Persistence** — `settings.json` = app/user prefs (theme, locale, playback, window geom, updater); per-view UI state (column widths/visibility, sort, browse path, nav index, detail ids, section-collapse) → `views.json` (`src/services/view_state.rs` — `ViewStateData` + `read/write/mutate_view_state`). Window state on close; queue → `queue.json`; search history → `search_history.json` (cap 10).
- **SQLx migrations** — `./migrations/`, run on startup; DB backed up before applying.
- **File hashing & moved-file detection** — BLAKE3 + partial index `idx_tracks_file_hash`. Watcher retains hash on delete. `tasks/retroactive_hash.rs` backfills. See `.claude/rules/blake3.md`.
- **Natural sort** — `natord` + `sort_key` column on tracks.
- **No `#[allow(dead_code)]` in hand-written code — there are zero, keep it that way.** The only `dead_code` allows in the build are the ones the **Slint compiler** stamps on every generated `get_*`/`set_*`/`on_*`/`invoke_*` in `app-window.rs`. The two shapes that would otherwise need them both have a better spelling: a compile-time trait assertion is an anonymous **`const _: fn() = || { fn check<T: Send + Sync>() {} check::<FooUi>(); };`** (the eight `assert_send_sync` fns are this), and a never-read field is usually a **redundant** keepalive, not a necessary one — check before suppressing (every `wire_*` closure clones its own strong `Arc`; there is no `Arc::downgrade`/`Weak<…Ui>` in the tree). If a suppression is genuinely unavoidable, use **`#[expect(lint, reason = "…")]`**, never `allow` — `expect` fires `unfulfilled_lint_expectations` once it stops being needed.
- **Strict clippy + rustc lints** — `[lints.*]` + `clippy.toml`. Slint-generated `app-window.rs` via `mod generated_ui` with allows. **Don't enable 1.97's `dead_code_pub_in_binary`** — unusable for this crate's lib+bin split (`[lib] name = "melodia"` + a `main.rs` that consumes it). A test harness is itself an executable, so under `--all-targets` the lint decides `pub` exports nothing and flags every item reachable only from `main.rs` — the library's whole public API and the entire wiring layer (measured 670 hits with test targets included, 0 on `--lib`/`--bin`). The `[lints.rust]` block carries the same note.
- **The toolchain is pinned, and the pin is what makes the strict gate survivable.** `rust-toolchain.toml` fixes the compiler at an exact version for local dev, CI and release. `[lints.clippy]` sets `pedantic`/`style`/`complexity` to `warn` and CI runs `-D warnings`, so on a *floating* stable **every new lint in every new stable is an automatic CI failure on an unrelated PR**. Pinned, new lints arrive only when someone bumps on purpose. Two consequences: (1) **CI installs *from* the file** with a bare `rustup toolchain install` (takes the *active* toolchain) — do **not** reintroduce `dtolnay/rust-toolchain`, whose `rustup default` the file outranks; components belong in the file's `components` list, else clippy/`llvm-tools-preview` silently go missing. (2) A bump moves **four** things in lockstep — `rust-toolchain.toml`, `Cargo.toml`'s `rust-version`, `clippy.toml`'s `msrv` (an explicit clippy msrv *overrides* the Cargo one), and the two docs (`README.md` prerequisites + the Prerequisites bullet above) — and should expect fresh `pedantic`/`style` lints on the first run.
- **No `unwrap()` in non-test code** — use `?` and `AppError`. `expect()` only with invariant in message. **Tests too**: `unwrap_used = deny` + `expect_used = warn` (`[lints.clippy]`) apply crate-wide with no test exemption, and `-D warnings` promotes the `expect` warning to an error — so test code uses assert-based checks (`assert_eq!`, `matches!`, `.ok().map(...)`) or `let …/else`, not `unwrap`/`expect`.
- **`AppError` (`src/error.rs`)** — I/O-boundary variants (`Metadata`/`Network`/`Watcher`/`Scanner`) are **struct variants** `{ msg, #[source] source: Option<Box<dyn Error + Send + Sync>> }` that keep both a context message *and* the typed cause. Build them with `AppError::{metadata,network,watcher,scanner}(msg, source)` (or the `*_msg(msg)` message-only siblings); never flatten a source with `format!("…{e}")`. `io_source(e)` wraps an arbitrary error under `Io` preserving its `.source()` (vs `io_other(msg)`, message-only). `Database`/`Migration`/`Io` use `#[from]`; the remaining pure-message variants (`NotFound`/`Validation`/`Queue`/`Settings`/`Window`/`Player`) stay `String`.
- **Kick-after-persist for `mutate_settings` consumers.** Kick fires **inside** same `spawn_blocking` after write commits, only on `Ok(())`. Multi-write callbacks track `all_persists_ok`, kick only if every write committed.
- **Sibling-callback writes need a synchronous shadow.** Two callbacks reading/writing same field race through disk. Mirror in `Arc<parking_lot::Mutex<T>>`, update synchronously *before* spawning disk write; read cell from siblings.
- **Renderer is FemtoVG.** Set directly on `slint` dep — no Cargo feature. Software renderer dropped (slint-ui/slint#4176).
- **`--version` literal-first branch in `main()`** — prints `Melodia <CARGO_PKG_VERSION>` then returns. Updater smoke test (`verify_swapped_binary`) spawns new binary with `--version`, asserts exit 0 in 5 s + stdout starts with `Melodia ` and contains expected version; rolls back from `target.old` on failure. **Forward-compat contract** — don't remove/break this branch or prefix; breaks in-place updates for older clients. It lives here rather than in `.claude/rules/updater.md` (which holds the rest of the install/release path) because `main.rs` is where you'd break it, and that read doesn't load the rule.

## Slint Conventions

Three rules share the UI globs and answer different questions: `.claude/rules/slint.md` is general patterns, **`slint-pitfalls.md` is the things that build, look right, and are wrong**, and `ui-patterns.md` is the component to reach for before building a second one. Widen a glob rather than restating a rule here; `tokio.md` and `rust-performance.md` are `src/**/*.rs`, so those two are always on for Rust. Project-specific:

- **No `slint::slint!`** — `.slint` files via `build.rs`.
- **Animation tokens**: 200ms fast, 250ms medium, 400ms spatial.
- **Scrollbars — always `ui/components/overlay-scrollbar.slint`.** std-widgets' bar paints inside padded containers, can't be reskinned. (1) `ScrollView` primitive, both scrollbar policies `always-off`. (2) `OverlayScrollbar` at view root, sibling of padded layout, pinned via absolute coords. (3) Round-trip via `viewport-y` + `scroll-to`: `offset: -sv.viewport-y; scroll-to(o) => { sv.viewport-y = -o; }` (no `<=>` — sign flip blocks it). (4) `visible: content-size > visible-size`; mount both axes.

## Styling

- **Pluggable themes** in `src/themes/`, applied by writing brushes into `Theme`. Default: Catppuccin Mocha mauve.
- **System dark/light** — `services/system_theme.rs` (Linux XDG portal). `SYSTEM_VARIANT_ID` resolves via `system_{dark,light}_variant`. `spawn_color_watcher` listens on portal `SettingChanged`; `ui/appearance/system_watcher.rs` re-applies. KDE Breeze + system re-source from `~/.config/kdeglobals`.
- **Icons** — Material Symbols Rounded variable font (`MaterialSymbolsRounded.ttf` + `MaterialSymbolsRoundedFilled.ttf` for FILL=1). `import "...ttf";` OpenType ligatures (`text: "play_arrow"`).
- **Fonts** — Vazirmatn (OFL, Latin + Arabic) under `ui/assets/fonts/`, embedded via static `import "...ttf"`. UI base 14 px. **TTFs patched** — `scripts/patch_vazirmatn.py` rewrites OS/2 typo + hhea ascent/descent to `1650/-500` so glyph mass lands at line-box centre on FemtoVG. Patched output in `ui/assets/fonts/vazirmatn/`; pristine upstream isn't checked in — to update, re-download the three TTFs and re-run patch script.
- **Material Symbols Rounded subset** — `scripts/subset-icon-fonts.sh` trims both icon TTFs to only the ligatures in `scripts/icons.txt`, subsetting **by glyph id with `--no-layout-closure`**; do not regress to `--text=<names>` — feeding a–z into ligature closure reaches every icon and ships the whole catalogue. **Both faces carry the full used-set** (every icon available outlined *and* filled). Pristine source faces live in gitignored `scripts/fonts-src/` (never overwritten). `icons.txt` is load-bearing: a name used in code but absent renders as tofu; `scripts/check-icons.py` re-derives the used set from the `.slint` sources and fails on drift — run it (and re-run the subset) after adding an icon. Requires `fonttools`.

## Internationalization (i18n)

- **`@tr("English msgid")` macro.** Registers msgid at codegen; re-renders on locale switch. Plurals: `@tr("{n} track" | "{n} tracks" % count)`. Interpolation: `@tr("{} of {}", a, b)`.
- **Bundled translations.** `build.rs` calls `with_bundled_translations("translations")` + `with_default_translation_context(DefaultTranslationContext::None)`. Layout: `translations/<lang>/LC_MESSAGES/Melodia.po`. English is source baseline (no `en.po`).
- **Runtime switch.** `slint::select_bundled_translation(&code)` — in `main.rs` before `app.run()` from persisted `settings.locale`, and inside `Settings.language-changed(int)` per dropdown pick. No restart.
- **Locale wiring (`src/ui/locale.rs::install_locale`).** Hydrates `Settings.language-{names,codes,idx}` from `SUPPORTED_LOCALES` (`&["en","de","fr","es","tr","el","it"]`); change callback calls `select_bundled_translation`, updates `PersistedLocale` shadow, spawns `library::settings::set_locale`. Language names always native. New locale: append `SUPPORTED_LOCALES` + `LOCALE_NATIVE_NAMES` (1:1), drop `translations/<code>/LC_MESSAGES/Melodia.po`.
- **Don't translate.** Material Symbols ligatures, asset paths, theme tokens, debug logs, brand/proper-noun chip labels (`"Melodia"`, `"KDE"`, `"GNOME"`, `"macOS"`, `"Windows 11"`, Catppuccin names), fallback `Dialog.confirm-label: "OK"`.
- **`@tr()` only translates literal strings at codegen.** A `[string]` populated from Rust renders whatever Rust pushed. **Workaround**: inline literal `[@tr("A"), @tr("B"), …]` at use site (drop global property + Rust seeding); order must match Rust source. Theme Variant ternaries on `Settings.theme-idx == 0` swap between Catppuccin proper-noun list and Dark/Light/System.
- **Adding strings.** Wrap literal in `@tr(...)` → add same `msgid`/`msgstr` to **every** shipped `Melodia.po`. Stay msgid-aligned. Plurals: `msgid_plural` + `msgstr[0]`/`msgstr[1]` — Turkish keeps gettext layout but doesn't pluralize after numerals; Greek pluralizes regularly.
- **RTL deferred.** Slint 1.16 has no `direction: rtl` and no bidi-aware HorizontalLayout. fa/ar/he need manual layout mirroring + LRM/RLM markers.
- **`slint::translate_from_bundle` is `i_slint_core`-internal.** Slint does NOT publicly expose a Rust-callable `tr("...")`.

## Async / Tokio

See `.claude/rules/tokio.md`. Project-specific:

- One multi-thread runtime in `main.rs`. Inside Slint event loop, `.await` tokio futures via `slint::spawn_local(async_compat::Compat::new(...))`; pure background work uses `runtime.spawn(...)`.
- **`tokio::sync::Mutex` only when lock crosses `.await`.** Otherwise `parking_lot::Mutex`.
- **`watch` when only latest matters; `mpsc` for events you must not coalesce.**

## Testing

- **Unit tests** — per-module `tests/` subdirs. Each source file references via `#[cfg(test)] #[path = "tests/<name>_tests.rs"] mod tests;`. Never inline `#[cfg(test)] mod tests { ... }`.
- **Integration tests** — `./tests/`. Dev-deps: `tempfile`, `tokio` (`test-util`).
- **DB tests** — `DbPool::test_pool()` (in-memory). Helpers in `src/database/queries/tests/helpers.rs`: `make_test_metadata`, `insert_test_track`, `setup_seeded_db`.
- **Player DSP tests** — helpers in `src/player/tests/helpers.rs` (declared by the same nested `#[cfg(test)] mod tests { mod helpers; }` block `database/queries/mod.rs` uses): `TestSource` (in-memory rodio `Source`, `try_seek` rewinds to 0), `approx_eq`/`assert_approx`, `bits` (bit-identical passthrough without a float `==`), `fill_sine`. `crossfade_tests` takes none of it on purpose — pure predicates, and a tighter tolerance than `approx_eq`.
- **UI tests** — `slint::testing` exists but UI coverage intentionally light; test **library** layer thoroughly.

## Continuous Integration

- **PR gate** — `.github/workflows/pr-validation.yml`, on PRs into `dev`: `changes` (skip matrix, below) → `clippy` (`--all-targets --locked -- -D warnings`) → `test` (plain `cargo test --locked`, built in two phases — see below, `needs: [clippy]` so a lint failure fast-fails). The aggregate **`pr-validation`** job is the **required status check**. **No `fmt` job** — the tree isn't `cargo fmt`-clean. Coverage does **not** run here — see *Coverage → Pages*.
- **The skip matrix is a denylist, and `predicate-quantifier: 'every'` is what makes it work.** `dorny/paths-filter` decides whether the compiling jobs run; its filter is `'**'` plus `!` exclusions, *not* a list of source globs. That direction matters because the gate job counts `skipped` as a pass — under an allowlist, a path nobody remembered to list merges green with zero checks run. **Never drop `predicate-quantifier: 'every'`**: the default (`'some'`) makes a filter true when a changed file matches *any* pattern, and every file matches `'**'`, so the exclusions would silently do nothing and the job would never skip — it builds, it looks right, and it is wrong. `fkirc/skip-duplicate-actions` catches the identical tree arriving twice and is deliberately given **no** `paths_filter`: paths live in one action only, so the two can't drift into handing back a stale pass.
- **Merges into `main` come from `dev` or `hotfix/*`** (`enforce-pr-source.yml`; its `check-source-branch` is the **only** required check on `main`), so a `hotfix/*` can ship straight to `main` without routing through `dev`. A hotfix merge still triggers `release.yml` (ten matrix slots, drafts a release off `Cargo.toml`'s version), so **bump the version in the hotfix branch** or `prepare` short-circuits against the already-published tag. `pr-validation` does **not** run on PRs into `main` — a hotfix's clippy/test coverage is whatever you ran locally.
- **Headless audio (non-obvious)** — the `test` job runs `tests/headless.rs` (`AppState::init` opens rodio's default device). GitHub's Azure runner ships **no `snd-dummy`/`snd-aloop`**, so CI points ALSA's default PCM at alsa-lib's built-in userspace `null` device via `/etc/asound.conf` (`pcm.!default { type null }`) — no kernel module or extra package.
- **Shared provisioning** — `.github/actions/linux-system-deps` (composite) does the Azure apt-mirror swap + retrying install of the Slint/ALSA/Wayland/D-Bus system libs, reused by the compiling jobs. `Swatinem/rust-cache` per job, `ci-*` shared-keys (distinct from release's `rust-release-*`).
- **Every compiling job builds in two phases, and that is what keeps CI inside the runner's 16 GB.** The crate emits **four large link targets** — the lib test binary, the `Melodia` bin, and the two integration tests — and each links the ~390k-line generated `app-window.rs` at roughly 5.3 GiB. Cargo schedules three of them at once, so a single-command build measured **21.5 GiB** (plain `cargo test`, profile defaults) and **16.5 GiB** (`cargo llvm-cov`). Over the ceiling it does not fail — it **swaps**, and a 14-minute job silently runs for the better part of an hour looking like a hang. Phase 1 (`--lib`, `CARGO_BUILD_JOBS: 4`) builds the ~600 dependencies at full width — they are many small units and never the problem; phase 2 (`CARGO_BUILD_JOBS: 1`) has only the remaining links left and runs them one at a time. Measured **10.25 GiB** for the gate and **9.78 GiB** for coverage. **Don't collapse the two steps back into one command**: instrumentation is worth only ~0.5 GiB of the peak, debug info ~0.5 GiB, and the job count alone doesn't cap it — the phase split is the whole margin. `--tests` means *every* target with `test = true`, so phase 2 re-runs the lib tests too; duplicate profraw can't skew coverage, which is region-hit rather than hit-count. The real cure is fewer link targets or a smaller generated unit; until then this is load-bearing.
- **Coverage → Pages** — coverage is **off the PR path**; the instrumented build is the most expensive thing in this repo's CI and has OOM-killed the runner (143). `deploy-coverage.yml` runs `cargo llvm-cov` itself on **push to `dev`** (plus `workflow_dispatch` as the off-cycle escape hatch), then deploys the HTML to **GitHub Pages** (`https://kenansalar.github.io/Melodia/`) from a second job in the *same* run — no cross-workflow artifact fetch, so no `dawidd6/action-download-artifact`, no retry step, no is-it-empty guard. **`CARGO_PROFILE_{DEV,TEST}_DEBUG: "0"`** there, not `line-tables-only`: LLVM source-based coverage carries its own file/region table in `__llvm_covmap` and reads nothing from DWARF, and nothing reads a backtrace out of that job. The PR `test` job keeps default debug info for exactly the opposite reason. Depends on two one-time settings: **Pages enabled** (Source = GitHub Actions) **and** the **`github-pages` environment's** deployment-branch policy widened to allow **`dev`**.
- **Every action is SHA-pinned** with a trailing `# vX.Y.Z` comment — no floating tags anywhere, and no `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24`: every pinned action resolves to `node24` or to a composite over one, so the Node 20 removal (2026-09-16) needs no shim. Re-pin with `gh api repos/OWNER/REPO/commits/TAG -q .sha`. Anything under `.github/workflows/` is CODEOWNER-protected (`@KenanSalar`).

## Memory Discipline

Project exists *because* of memory regressions in the Tauri version.

- Run `/usr/bin/time -v target/release/melodia` after notable changes; track peak RSS.
- Feature adding > 20 MB idle RSS → profile with `heaptrack` before merging.
- Size-bound caches (LRU); construct heavy clients lazily. `Vec::with_capacity` whenever capacity known.
- **`RUST_LOG=info MELODIA_RSS_SAMPLE=1` for live memory diagnostics.** Opt-in `src/tasks/rss_sampler.rs` (UI thread, `rss_sampler::install(weak)`) logs `[MEM view=… VmRSS=… RssAnon=… RssFile=… …]` every 500 ms at INFO. `view=` tag captures Nav section + open detail id + `+NP`/`+QS` overlay flags. Env-gated diagnostic exception to `tasks/`-no-`ui::*`. `RssFile` growth is Mesa GPU pool, not Rust heap.
- **glibc arena cap via `mallopt(M_ARENA_MAX, 2)` at top of `main()`.** The playback-driven RSS drift was per-thread arenas, not a Slint cache leak: glibc's default (`8 × num_cpus`) gives each long-lived thread a 64 MiB virtual arena, and the cap drops idle anon memory. **Don't lower to 1** — serialises every allocation; audio thread can stall behind UI. Linux-glibc only (`cfg(all(target_os = "linux", target_env = "gnu"))`). **Literal first statement in `main()`**, before `env_logger::init()` and tokio. Pairs with `libc::malloc_trim(0)` one-shot at t=5 s.

## Known Gaps

- **zbus footgun**: never enable `features = ["tokio"]` on zbus — unifies into Slint's transitive `accesskit_unix → atspi → zbus` and panics Slint's a11y thread at startup. AOT uses `zbus::blocking::Connection` inside `tokio::task::spawn_blocking(...)`. `ksni` (Linux tray) pinned `default-features = false, features = ["blocking", "async-io"]` for the same reason — its default `tokio` feature pulls `zbus/tokio`; `async-io` (matching accesskit) is mandatory since ksni's `compat` module `compile_error!`s without an executor feature.
- **Vendored winit fork**: `winit/` is winit 0.30.13 + 3 Wayland-DnD commits; `Cargo.toml` `[patch.crates-io] winit = { path = "winit" }`. Fresh clones and CI build with no setup. Trimmed to essentials (`src/`, `build.rs`, `Cargo.toml`, `LICENSE` + `winit/README.md`). **`dpi` is NOT vendored** — `winit/Cargo.toml` pulls from crates.io (`dpi = "0.1.1"`, not `path = "dpi"`); a path-vendored `dpi` is a second un-unifiable instance that clashes with `muda`'s registry `dpi` on Windows (`i-slint-backend-winit` E0308). Upstream bump: rebase fork onto new tag, copy essential paths (don't rsync; re-apply registry-`dpi` edit). Retires (delete `winit/` + patch block) once winit 0.31 ships with the merged #4571 DnD API *and* a Slint release surfaces external drops as file paths on `DropArea` — that bump also renames `unstable-winit-030` → `-031` and `slint::winit_030` → `winit_031`. See `SLINT_NATIVE_ADOPTION.md`.

## User Conventions

- Always call big files/objects/functions **'monolithic'** — no synonyms.
