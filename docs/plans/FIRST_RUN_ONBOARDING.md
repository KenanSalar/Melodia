# First-Run Onboarding Overlay

Working doc. Delete when the feature ships.

Status: **phases 1–6 landed, awaiting manual testing** · Created: 2026-09-06 · Issue: #82

> Deviations from the plan as written, all decided while building:
>
> - **i18n moved out of phase 7 into each phase.** The catalogue walk is a test, so batching the
>   `.po` work at the end left every intermediate phase red. Each phase now adds its own msgids.
> - **`auto_check_enabled` had no Slint control at all** — the global, the seed and the callback
>   were wired and nothing mounted a switch. Settings ▸ Updates gained the row, so the card's
>   disclosure points at a permanent home.
> - **No `OnboardingFeature` model.** A data-driven table isn't expressible here: `@tr()` only
>   translates literals at codegen and a callback invocation can't be data. The shared
>   `OnboardingFeatureRow` component carries the markup; each row is one mount.
> - **No `reset_onboarding`.** The About row just re-opens the card; rewinding the flag would make
>   quitting mid-card repeat it next launch, which is worse.
> - **Deferral is a closure, not an `Onboarding.closed()` callback.** Rust already owns the unmount
>   timer, so `install` takes the deferred work and runs it there — no global callback with
>   single-handler semantics to get wrong.
> - **Two new icons** (`update`, `waving_hand`) meant `scripts/icons.txt` plus a re-subset; both
>   shipped TTFs changed.

---

## What ships

A first-run card over the app with three panels of real controls, closeable in a click. Not a
tour: no highlight bubbles, no arrows pointing at the sidebar. The app repaints underneath as
the controls are used, which is the whole demonstration.

1. **Appearance** — language, theme, variant, accent. All four already apply live.
2. **Your music** — reports the scan `tasks/first_launch.rs` has already started, and offers
   *Add another folder*. On the install where the auto-add found nothing, this panel is the
   reason the feature exists.
3. **What Melodia can reach** — one row per feature that ships off, plus the update check,
   which ships on and says so for the first time.

Three decisions taken up front:

- **Any close marks it seen.** X, backdrop, Escape and Skip all write the current revision.
- **The update-check row discloses; it does not flip the default.** `auto_check_enabled` stays
  `true` and the row shows the real state.
- **The persisted revision ships, the replay path does not.** v1 always shows all three panels;
  the per-panel "introduced in revision N" filter waits for a second revision to test against.

Carried in alongside, and deliberately **not** a panel row: **the system tray icon defaults
on**. Toggling `tray_enabled` needs a restart, and a welcome card ending in "restart to apply"
is a chore, so it is a `Default` flip in `TrayFlags` and nothing else.

## Structure

| tree | new |
|---|---|
| `crates/melodia-ui/ui/globals/onboarding.slint` | the `Onboarding` global |
| `crates/melodia-ui/ui/components/onboarding/` | `OnboardingOverlay` and the three panels |
| `crates/melodia-views/src/ui/onboarding/` | `mod.rs`, `callbacks/` |
| `crates/melodia-app/src/library/settings/onboarding.rs` | the two revision setters |

**Not a `Dialog` kind.** `Dialog` is a one-shot accept/cancel primitive with backdrop-click
cancel and a two-button footer. A stepped card needs a step index and a footer that does not
close on Next, both of which would land on a global twenty other flows read.

**Mounted only while it is up.** `DialogOverlay` and `QueueSheet` are permanently rendered so
their close animation can play, which is right for surfaces opened many times a session. This
one opens once ever, so it takes `NowPlayingView`'s shape: `if Onboarding.mounted`.

### The constraint that decides the shape

Because the overlay is `if`-mounted, **nothing in its subtree may carry a `changed` handler.**
A `ChangeTracker` inside a dropped branch stays registered in `CHANGED_NODES` with nothing to
upgrade to and panics the next time its property is re-dirtied — which the Settings ▸ About
re-run does directly, by writing `Onboarding.open` again.

Banned from the card: `TabBar`, `SearchBar`, `MetaChipStrip`, `GridColumnsSync`,
`FilterThrottle`, `CompositeScrollbars`, `FocusLossWatcher`. Verified tracker-free and safe:
`ChipGroup`, `ColorDotGrid`, `Dropdown`, `SettingRow`, `SettingRowStacked`, `ToggleSwitch`,
`SectionButton`, `PillButton`, `MaterialIcon`, `ProgressBar`, `SectionDivider`,
`GridEmptyState`.

**So Rust owns the mount timing**, rather than the `DialogOverlay` idiom of a `changed t`
handler firing `closed()` at t≈0:

- open: `mounted = true`, then a 1 ms `Timer::single_shot` setting `open = true`, because
  `init` sets the *initial* value and `animate` would never run.
- close: `open = false`, then a `dur-fast` `Timer::single_shot` clearing `mounted`.

### Where it mounts

Root-level sibling in `app-window.slint`, between `ResizeRing` and `DialogOverlay` — **below**
the dialog layer. Step 2's *Add another folder* can raise a real `Dialog` on failure, which has
to paint above the card and take Escape first. Paint order and Escape order then agree.

## Phases

Each phase leaves the tree building and the clippy gate clean.

### Phase 1 — Persistence and the decision · no visible change

1. `OnboardingFlags { onboarding_version: u32 }` + `ONBOARDING_VERSION`, flattened into
   `SettingsData`.
2. `library/settings/onboarding.rs` — `set_onboarding_seen`, `reset_onboarding`.
3. `TrayFlags` gives up its derived `Default` for a hand-written one with `tray_enabled: true`.

**Exit:** clippy and `cargo test --locked --workspace` clean. A fresh `MELODIA_DATA_DIR` puts a
tray icon up; a directory whose `settings.json` says `"tray_enabled": false` still has none.

### Phase 2 — The shell: global, overlay, mount, keys

Global, overlay component with the three panels stubbed, the `app-window.slint` mount, the
`shortcut-scope.slint` Escape arm and widened non-Escape early-out, the eleventh focus-regrab
mirror, the `melodia-views` slice, `CALLBACK_HOMES` enrolment, and the `main.rs` install.

**Exit:** the card opens on a fresh data dir, every dismissal path closes it, it never returns.

### Phase 3 — Step 1, Appearance

**Exit:** all four controls apply live with the card on screen; a language pick re-renders the
card in place.

### Phase 4 — Step 2, Your music

**Exit:** the count is live against a running scan; *Add another folder* opens a parented picker.

### Phase 5 — Step 3, What Melodia can reach

A table (`OnboardingFeature` in `models.slint`), not a hand-written column. The deep link out
reuses the two existing `IndexPersist` writers — `crates/melodia/tests/index_persist.rs` pins
the writer count at exactly six.

**Exit:** each toggle takes effect with the card up; the scrobbling button lands on
Settings ▸ Services and the tab survives a restart.

### Phase 6 — Deferral, and the re-run row

The update toast and the crash notice must not stack under the card. There is no suppression
mechanism in the tree and building one is more machinery than the problem needs, so **defer
rather than suppress**, through a single `Onboarding.closed()` handler. Suppressing the crash
notice outright would lose the report — `crash_report::take_unseen` consumes the marker.

Settings ▸ About gains a *Show the welcome guide again* row.

**Exit:** a fresh run shows the card alone; closing it lets the update check start.

### Phase 7 — i18n, tests, docs · after manual testing

Every new `@tr()` literal owes the same `msgid` in all six catalogues. Tests: the step-count
pin, the revision gate, the deep-link ordering, and a walk asserting no `changed ` inside
`components/onboarding/` — the one regression that builds, reviews clean, and panics on the
second open.

## Cross-cutting

**Memory.** The overlay's models and handles drop with the close rather than parking for a
second open a settled install never makes. The tray flip adds a D-Bus connection and a ksni
thread to every new install's baseline, so the closing RSS reading is compared against a run
with `tray_enabled` forced off, not just against the ceiling.

**Threading.** Everything here is UI-thread. Disk writes go through `persist_blocking`; the
mount and unmount timers are `slint::Timer::single_shot`, the shape `shell/notifications.rs`
and `tray_bridge.rs` already use.

**Errors.** The two setters return `Result<(), AppError>` and their callers log through
`error::describe`. No `Result<_, String>`.

**i18n.** Every label is an `@tr` literal in the `.slint`. The feature table carries a routing
`kind` and a live `bool`, never Rust-pushed copy — `@tr()` only translates literals at codegen.

**Logging.** The two settings writes take the `persist_blocking` label; nothing else logs.

## Open questions

None outstanding. The two the issue left open — whether an early close counts as seen, and
whether the update-check default flips — were settled before Phase 0.
