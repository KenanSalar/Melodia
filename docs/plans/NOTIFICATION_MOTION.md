# Plan: Notification motion (cards that arrive, leave and make room)

Status: **in progress** (Phase 0 done, Phase 1 awaiting Kenan's hand test) · Owner: Kenan ·
Created: 2026-10-31

> **Validated 2026-10-31** against `0b6d11b5` (`main`): direct reads of `notification-stack.slint`,
> `globals/notifications.slint`, `models.slint`, `notifications.rs` and its tests, `undo/mod.rs`,
> `view-transition.slint`, `mount_fade_tests.rs`, `frameless_tests.rs`,
> `onboarding/callbacks/mod.rs` and `slint-pitfalls.md`. Checked in Slint 1.18.1's sources:
> `model/repeater.rs` (`row_changed` updates an instance in place; `row_added` and `row_removed`
> leave the others standing), `timers.rs` (`single_shot` without a platform is queued and never
> fires, so model unit tests are safe), `animations.rs` with winit's `event_loop.rs`
> (`animation-tick()` is refreshed in `new_events`, before a pointer event is dispatched). Guidance:
> the Material 3 snackbar guidelines and accessibility pages, the GNOME HIG toasts page,
> libadwaita's `AdwToast`, Sonner's docs and its author's write-up, and WCAG 2.2.1.

Working doc. Delete when every phase has shipped and passed Kenan's hand test.

## Context

**What you see today.**

- A notice appears at full strength in one frame, with no fade and no movement.
- Closing one (✕, Undo, Ctrl+Z, its clock running out) removes it in one frame, and every card
  above it drops by a card's height plus the gap in that same frame.
- A sixth notice pushes the oldest out the same way.
- A run of the same removal changes the count in place, with nothing to draw the eye to it.
- The undo notice wears the info glyph, which reads as an announcement rather than an offer.

**Why.**

- **The fade-in is written but never runs.** `init => { root.shown = true; }`
  (`notification-stack.slint:17`) sets the starting value, and `slint-pitfalls.md`'s `init` entry
  says a value set there never animates. `mount_fade_tests.rs` checks the 1 ms `Timer` fix for
  three files, and this one isn't among them.
- **Nothing can animate a card out.** `remove_at` (`notifications.rs:280`) takes the row out of
  the `VecModel`, and Slint destroys its instance at once. A repeated instance has no exit hook.
- **The column jumps.** The cards sit in a `VerticalLayout` with `spacing: 8px` (`:181`), each
  card's height fixed at its content (`height: layout.preferred-height`), so the layout re-flows in
  one step.

**Outcome.**

- A card rises from the stack's bottom edge as it fades in, and the cards above glide up to make
  room.
- A leaving card fades while its slot closes, and the cards above glide down. Every way out takes
  this path: ✕, Undo, Ctrl+Z, the clock, the undo slot replacing its notice, a dismiss by kind, and
  the cap evicting the oldest.
- When a run of removals updates the undo notice's count, the card briefly lifts.
- The undo notice and its over-cap sibling carry no glyph. The accent bar stays.
- A card can be dragged right and released to close it.
- **Unchanged:** what raises a notice and its text and colours, every timing (the 6 s undo
  default and its 4 to 10 s range, 3 s confirmations, sticky errors), the hover and unfocused
  pauses, Ctrl+Z, placement (bottom-right above the now-playing bar, 360 px wide, 16 px insets),
  the cap of five live cards, and the `@tr` catalogs (no string changes).

## Decisions settled here

1. **Keep the `VerticalLayout` and animate each card's own height.** A card in a `for` can't read
   its siblings' heights, so a hand-placed stack would have nowhere to get its offsets. A card whose
   height eases from 0 to full makes the layout re-flow every frame, and that re-flow is the glide.
   The 8 px gap moves inside each card, above its body, so it eases too, and the column's `spacing`
   goes to 0.
2. **The card rises from the bottom edge, clipped only while it is shorter than full.** The body is
   anchored to the card's top. In a bottom-anchored column, a growing card's top moves up, so the
   body rises from the stack's bottom edge, revealed by the clip, and never overlaps the card above.
   The clip has no radius, so it is a plain scissor with no layer. At rest it is off, so nothing the
   body paints past its edge is cut, including the drag in Phase 4.
3. **Leaving happens in two steps: Rust marks the row `leaving`, the card fades and collapses, and
   then Rust removes the row.** Rust schedules the removal with `slint::Timer::single_shot` for
   `Theme.dur-med`, read through the getter. The onboarding overlay uses the same shape
   (`onboarding/callbacks/mod.rs:63`). The duration has one home, and no card is destroyed from
   inside its own `Timer`.
4. **A row stops being live the moment it starts leaving.** At that moment the undo slot's listener
   fires, so Ctrl+Z can't reach a card on its way out, and its recipe is dropped. After it,
   `dismiss`, `relabel`, `restart` and the action button all ignore the row. The cap counts live
   rows only.
5. **Motion tokens.** The enter runs on `Theme.dur-med` with `cubic-bezier(0.2, 0, 0, 1)`, the
   curve `ViewTransition` uses. The exit fades on `Theme.dur-fast` with `ease-in` and collapses on
   `Theme.dur-med` with the enter curve. Something leaving should go faster than it came. The hand
   test tunes the curves, while the tokens stay.
6. **One opacity, on the card.** An `opacity` below 1 renders the subtree into a texture sized to
   its children, which crops marks above a line box while the fade runs (`slint-pitfalls.md`,
   Text). `ViewTransition` accepts this for a whole page. A card fades for at most a quarter
   second, so it accepts it too.
7. **No pause on keyboard focus.** Nothing in the card can take focus, because `ShortcutScope` is
   the only `FocusScope`. Ctrl+Z is the keyboard path.
8. **The undo family gets a `neutral` variant rather than a kind test in the card.** `timed_notice`
   (`undo/mod.rs:139`) builds both the undo notice and the over-cap one, so changing one line
   covers both. `neutral` paints the accent bar and no glyph. It isn't called `plain`, which already
   means "no button" (`NotificationParams::plain`, `RowText::plain`).
9. **Drag works rightwards only, toward the stack's own edge.** A release past 45 px, or faster
   than 0.11 px/ms over the whole drag, closes the card. Anything less eases back, and a leftward
   drag stays at 0. The speed rule exists because a short, quick flick would fail on distance alone.
10. **The count cue is a 4 px rise and settle of the body, not a scale or a flash.**
    `transform-scale` draws glyphs as paths (`mini-captions.slint:120`), and a brush flash would
    need the recoloring gate plus a second rounded layer over the fill. A 4 px rise stays inside
    the 8 px gap, so the clip never cuts it.

## Prior art

What the guidance says, and how this plan answers it. Only libraries and design systems are named
here.

- **Material 3 snackbars** appear without blocking anything, and updated information replaces an
  outdated snackbar in place. They auto-dismiss after 4 to 10 s. They enter on a decelerating curve
  and exit on a shorter, accelerating one. Decision 5 follows this.
- **libadwaita's `AdwToast`** holds its timer while hovered. Re-adding a shown toast resets its
  clock, and the title carries the count. The undo slot's merge already works this way.
- **Sonner** raises a toast from below its resting place and animates with interruptible
  transitions, so a burst of toasts can change course mid-flight. It unmounts a toast only after
  the exit duration. It dismisses on a 45 px or 0.11 px/ms swipe and pauses timers while the page
  is hidden. Decisions 2, 3 and 9 follow this.
- **WCAG 2.2.1**: the existing hover and unfocused pauses already cover adjustable timing. The
  motion added here is decorative and short, and no information depends on it.

## Structure

| File | Owns after the change |
|---|---|
| `crates/melodia-ui/ui/components/dialog/notification-card.slint` **(new)** | One card: its lifecycle (enter, leave, clock, count cue, drag) and its content |
| `crates/melodia-ui/ui/components/dialog/notification-stack.slint` | Placement only: the column, its insets, the `for` |
| `crates/melodia-ui/ui/models.slint` | `NotificationRow.leaving`, `NotificationVariant.neutral` |
| `crates/melodia-views/src/ui/shell/notifications.rs` | The row lifecycle: push, retire, remove after the exit, the cap over live rows |
| `crates/melodia-views/src/ui/undo/mod.rs` | `timed_notice` picks `neutral`, and the listener is renamed |
| `crates/melodia-views/src/ui/appearance/mod.rs` | `token_duration`, the one conversion from a `Theme` duration |

- **Why the card gets its own file.** The card gains a lifecycle (the enter flip, the two-step
  exit, the cue, the drag) on top of its content. The stack's one reason to change, placement, is
  one the card doesn't share. Both files stay well under 800 lines.
- **Vocabulary in `notifications.rs`.** *Retire* means the row stops being live and its card
  starts leaving. *Remove* means the model drops the row. So `remove_at` becomes `retire_at`,
  `on_row_removed` becomes `on_row_retired`, the `row_removed` field and undo's `row_removed`
  become `row_retired`, and `find` becomes `find_live`.
- **Duplication inside the blast radius.** Three sites convert a `Theme` duration to
  `std::time::Duration` by hand, as `u64::try_from(…get_dur_*()).unwrap_or(0)`:
  `onboarding/callbacks/mod.rs:63`, `now_playing/mod.rs:366` and `appearance/theme_paint.rs:96`.
  Notifications would be the fourth. Phase 0 lands one helper first, with no behaviour change.
- **No constant crosses the Slint/Rust boundary.** Rust reads `Theme.dur-med` through its getter,
  and the fling thresholds and the cue height live in the card only.

## Phases

Each phase ends at the static gates and a build, and Kenan runs it. An existing test that a phase
breaks is fixed in that phase. New tests and doc files wait for Phase 5's go.

### Phase 0: one `Theme` duration conversion (no behaviour change)

Status: done (2026-10-31). `token_duration` is `pub(crate)`, all its callers being in the crate.
fmt, clippy, the full suite and the build are green.

1. Add `pub fn token_duration(ms: i64) -> Duration` to `ui/appearance/mod.rs`, which clamps a
   negative value to zero, as the three copies do.
2. Replace the copies at `onboarding/callbacks/mod.rs:63`, `now_playing/mod.rs:366` and
   `appearance/theme_paint.rs:96` with it.

### Phase 1: cards rise in and the stack makes room

Status: implemented (2026-10-31), awaiting the hand test below. fmt, clippy, the full suite and the
build are green. The generated card's vertical layout info is `min 0`, with `slot-h` as both its
preferred and max values and no body term folded in, and its clip has no radius. The stack drops
its `spacing` line, since 0 is `VerticalLayout`'s default. The note on the top-anchored body sits
above `body`, its anchor, rather than in the header.

1. **Pure move:** `NotificationCard` moves verbatim into `notification-card.slint`, exported, and
   the stack imports it. No edits in this step.
2. **Card geometry:**
   - A private `gap` (8px), `content-h: layout.preferred-height` and `full-h: gap + content-h`.
   - `body := Rectangle { x: 0; y: root.gap; width: 100%; height: root.content-h; }` takes the
     fill, the bar `Path`, the `sentinel` and the border. The `Path`'s `h` and `height` read
     `body.height` instead of `root.height` (`notification-stack.slint:72,79`), because the root
     now animates.
   - The root drops `height: layout.preferred-height`. Following the pitfall's split for an
     animated height, it gets `min-height: 0px; preferred-height: root.slot-h; max-height:
     root.slot-h;`, with `slot-h: root.shown ? root.full-h : 0px` carrying the one
     `animate slot-h` on the enter tokens, and `clip: root.slot-h < root.full-h`.
3. **Card enter:** `opacity: root.shown ? 1 : 0`, animated on the enter tokens. `init` only reads
   `root.opacity` and `root.slot-h`, and a 1 ms one-shot `Timer` flips `shown`, the same shape as
   `view-transition.slint:20-37`.
4. **Stack:** `spacing: 0px`, since the gap now lives in each card. The topmost card's gap sits
   above the column, where it is invisible, and the bottom inset stays 16 px.
5. **Comments:** the card's header loses "No exit animation" in Phase 2. Here it gains one line on
   why the body is top-anchored under a clip.

**Hand test** (`cargo run -p melodia`, dev data folder):
- Remove a queue row. The undo card rises from the stack's bottom edge as it fades in.
- With that card up, raise a different notice (for example, an import's completion). The first
  card glides up while the new one rises in.
- A long message that wraps to two lines rises at its full height.
- Exits are still instant until Phase 2.

### Phase 2: every way out leaves the same way

1. **`models.slint`:** add `NotificationRow.leaving: bool`, with a one-line why. `show` builds it
   `false`.
2. **`notifications.rs`:**
   - Add an `exit: Duration` field, read in `install` as `token_duration(Theme.get_dur_med())`.
   - `retire_at(pos)` replaces `remove_at` and stays the one path out. It drops the recipe, writes
     `leaving: true`, tells the listener, then schedules `single_shot(self.exit, …)` with a clone
     of `rows`, which calls `remove_row`.
   - `fn remove_row(rows: &VecModel<NotificationRow>, id: i32)` is a free function, so a test can
     run the removal the timer would.
   - `find_live` skips leaving rows and serves `dismiss`, `relabel` and `restart`.
     `is_live(id)` guards `on_action` before `run_action`, so a press on a leaving card does
     nothing.
   - `dismiss_by_kind` retires the live matches in a forward walk. Its back-to-front reason goes,
     because nothing is removed synchronously any more.
   - The cap becomes `while self.live_count() >= MAX_VISIBLE`, retiring the oldest live row.
     `MAX_VISIBLE`'s doc says it counts the cards still up.
3. **`undo/mod.rs`:** rename `row_removed` to `row_retired` along with the listener call. The
   module doc still holds ("the hold goes with its notice, however that leaves").
4. **Card:**
   - `up: root.shown && !root.data.leaving` drives `opacity` and `slot-h`.
   - The opacity animation takes `Theme.dur-fast` with `ease-in` while leaving, and the enter
     tokens otherwise.
   - The clock's `running` gains `&& !root.data.leaving`.
   - The header comment says in one line that Rust finishes a card's exit.
5. **Existing tests this breaks:**
   - In `notifications_tests.rs`, `make_ui` gains `exit: Duration::ZERO`, and the listener
     helper follows the rename.
   - `dismiss_removes_only_the_matching_row`, `dismiss_by_kind_removes_every_matching_row` and
     `max_visible_evicts_oldest_on_overflow` assert on live rows.
   - The recipe and listener tests hold as written.

**Hand test:**
- Each way out (✕, Undo, Ctrl+Z, the clock, a removal of another kind replacing the undo notice,
  a sixth notice evicting the oldest) fades the card, and its neighbours glide down.
- Clicking Undo or ✕ on a card that is already leaving does nothing.
- Ctrl+Z after the card starts leaving does nothing.
- A language switch with a card up still relabels it.

### Phase 3: the undo notice, with no glyph and a cue when its count moves

1. **`neutral` variant:**
   - Append `NotificationVariant.neutral` in `models.slint`.
   - The card's `look` gains `{ accent: Theme.accent, icon: "" }` for it, and `MaterialIcon`
     mounts behind `if root.look.icon != ""`.
   - `timed_notice` returns `NotificationVariant::Neutral`.
   - No Rust code matches on the variant, so nothing else moves.
2. **Count cue:**
   - A private `rise` length, with the body at `y: root.gap - root.rise`, and
     `animate rise { duration: Theme.dur-fast; easing: cubic-bezier(0.2, 0, 0, 1); }`.
   - The existing `changed watched-restarts` sets `rise` to a named `bump-rise` (4px) and starts a
     one-shot `settle` `Timer` on `Theme.dur-fast`, which sets it back to 0.
   - `init` also reads `root.rise`.
   - The undo merge is the only caller of `restart` (`undo/mod.rs:101`), so only the undo notice
     lifts.

**Hand test:**
- The undo notice has no glyph, while success, warning and error cards keep theirs.
- Removing three queue rows one at a time lifts the card on each removal as the count climbs.
- The cue plays under the pointer too, and the clock behaves as before.

### Phase 4: drag a card away

1. **State:** private `drag-x`, `dragging`, `flung`, `press-x` and `press-tick` properties, with
   `fling-distance: 45px` and `fling-speed: 0.11` (px per ms) named with a one-line why. The body
   gets `x: root.drag-x`.
2. **Pointer, on `sentinel`:**
   - Measure in window coordinates (`self.absolute-position.x + self.mouse-x`), because the
     sentinel moves with the body.
   - A left-button `down` records `press-x`, records `press-tick` from `animation-tick()`, and sets
     `dragging`. Other buttons are filtered out, because a `TouchArea` grabs on any button.
   - `moved` while `self.pressed` sets `drag-x` to `max(0px, now - press-x)`.
   - On `up`: past either threshold, set `flung` and call `Notifications.dismiss`. Otherwise set
     `drag-x` back to 0. On `cancel`, ease back.
   - Undo and ✕ keep their own presses, because a child takes the press first.
3. **Motion:**
   - `animate drag-x { enabled: !root.dragging; … }`, since a dragged property may not ease the
     drag.
   - A flung leaving card sets `drag-x` to the body's width, so it travels on rightward as it
     fades, and `clip` gains `&& !root.flung`, so it isn't cut at the column's edge while its slot
     collapses beneath it.
   - The clock's `running` gains `&& !root.dragging`.
   - `init` also reads `root.drag-x`.
4. **Check before closing the phase:** a card retired mid-drag (by eviction) is removed while
   pressed, and per the pitfalls file its release then reaches an ancestor as a fresh event.
   Confirm that nothing above `NotificationStack` in `app-window.slint` reads `PointerEventKind.up`
   over that corner.

**Hand test:**
- A slow drag short of 45 px eases back when released, and one past it closes and travels off to
  the right.
- A quick flick closes the card, and a leftward drag doesn't move it.
- A drag starting on Undo or ✕ behaves exactly as a click does today, and a right-button drag
  does nothing.
- The clock never fires mid-drag.

### Phase 5: tests and docs (each waits for Kenan's go)

1. **New pins:**
   - `mount_fade_tests.rs`: the card reads every animated layer at mount
     (`root.opacity`, `root.slot-h`, `root.rise`, `root.drag-x`, in whatever order its `init`
     settles on).
   - `notifications_tests.rs`, covering all of the following:
     - A retired row is no longer live: dismissing it twice tells the listener once, and
       `relabel` and `restart` don't reach it.
     - The cap keeps exactly `MAX_VISIBLE` live rows with leaving rows present.
     - `remove_row` removes only the named row after positions have moved.
     - `is_live` refuses a retired row.
   - Each pin is shown to fail by breaking its code deliberately.
2. **Docs:** sweep `README.md` and `.claude/rules/` for wording this makes stale.
   `ui-patterns.md`'s Notifications line is still true. Then delete this plan once Kenan's hand
   test passes, after the root `CLAUDE.md` check that what it built is argued at its anchors.

## Cross-cutting

- **Gates every phase:** `cargo fmt --all`, then
  `cargo clippy --all-targets --locked --workspace -- -D warnings`, then
  `cargo test --locked --workspace`, then `cargo build -p melodia`. After that, the comment audit
  from `code-style.md`.
- **The Slint pitfalls this plan steers around:**
  - No new `animate` reads a brush, so none owes `enabled: !Theme.recoloring`.
  - The card's only `changed` handler stays on `restarts`, never on a layout property.
  - No `Timer` interval reaches a layout property (1 ms and `Theme.dur-fast`).
  - Every animated property is read in `init`.
- **Unaffected:** no migration, no settings field, no `@tr` change, and no RSS check, since the
  change adds one bool per row.
- **`frameless_tests.rs`** pins only the `NotificationStack {` mount, which stays.

## Open questions

- The exit curve, the cue height and the fling thresholds are first values for the hand test, not
  settled numbers.
