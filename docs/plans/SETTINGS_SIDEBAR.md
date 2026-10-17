# Plan: Settings category panel

Working doc. Delete when the feature ships.

Status: **in progress** · Branch: `feat/settings-sidebar` (local) · Created: 2026-10-17

> Checked 2026-10-17 against the tree at `5946f2e9`: `settings-view.slint`, `settings-tabs.slint`,
> the five pages, `globals/settings-page.slint`, `layout/sidebar.slint`, `SearchBar`,
> `TooltipFrame`, `ui::settings::settings_page`, `view_state.rs` and the tests that pin them.

## What ships

Settings drops its pinned header row (five tabs and a search box). In its place is a category panel
card on the left, beside the cards:

- **Rows and search:** ten icon + label rows with the search box at the top. On wide windows the
  panel and the cards are centred together, and the panel stays put while the cards scroll.
- **Narrow windows:** the panel compacts into an icon rail. Each rail icon shows its name in a
  tooltip to the right of the card. The search box becomes a search button, and clicking it opens a
  search field to the right of the rail, above the cards.
- **Body:** one centred column of cards capped at 720 px, down from 800. The two-column flip is gone.

**What keeps working:**
- Every card and setting, unchanged and only regrouped.
- Search across every category, and "No matching settings".
- Ctrl+, and the deep links: the welcome card's Set Up lands on Integrations, and the signal path
  lands on Output.
- The restored category on restart. An upgraded install opens on Library once, since the old
  `settings_tab` key indexed the old five.

| # | Category | Cards | Icon |
|---|---|---|---|
| 0 | Library | Manage Folders, File Watching, Ratings | `library_music` |
| 1 | Playback | Playback | `play_circle` |
| 2 | Output | Output | `volume_up` |
| 3 | Appearance | Appearance, Language | `palette` |
| 4 | Window | Window, Player Bar, Miniplayer | `crop_square` |
| 5 | Radio | Radio | `radio` |
| 6 | Lyrics | Lyrics | `lyrics` |
| 7 | Integrations | Scrobbling, Discord | `link` |
| 8 | Updates | Updates | `system_update` |
| 9 | About | About, Diagnostics | `info` |

Every icon is already subset. "Integrations" is the one new msgid; "Interface" and "Services" leave
the catalogs.

## Structure

- `globals/settings-page.slint` owns:
  - the filter state and its predicate;
  - the category constants, count, index and the pick callback;
  - the body geometry: `body-avail` written by the view, then `card-cap`, `card-w`, and
    `nav-collapse-body-w`, the narrowest body the expanded panel keeps beside it.
- `views/settings-view.slint` is page chrome:
  - the width mirror and its seed `Timer`, and the lead margin that centres the pair;
  - the compact-mode search row and its focus hand-off across the threshold;
  - the body scroller and its bar;
  - the rail's `TooltipFrame`.
- `views/settings/settings-nav.slint` (new) is the panel:
  - the search slot, the rail's search button and the rows;
  - the measured expanded width, the compact verdict (the `TabBar` contract: the host hands over
    `avail-w` and reads `compact` back) and the morph;
  - the tooltip rect.
- `views/settings/settings-router.slint` (was `settings-tabs.slint`) mounts one category, or all ten
  while searching.
- `views/settings/pages/*.slint` hold which cards live in each category, one plain column each.
- `components/nav-row.slint` (new) is the row visual shared with the main sidebar's `SidebarItem`,
  which keeps only its `Nav` coupling.

**Collapsed or deleted:**
- the sidebar row, extracted rather than copied;
- the two-column machinery (`body-cols`, `body-w`, `page-w`, `grid-row`/`grid-col`, the latch);
- Settings' `TabSearchHeader` mount;
- the Rust if-chain over tab getters, now one `match` plus `ALL.find`;
- the testkit's `declared_tab_count`, now `declared_count(src, property)`.

**Grown:**
- `SearchBar` publishes `has-focus`.
- `TooltipFrame` takes a `gap`, defaulting to the value it had.

**Vocabulary:** "tab" becomes "category" throughout Settings (Slint, Rust, the `views.json` field and
its setter).

## Phases

1. ✅ Extract `NavRow`; `SidebarItem` wraps it, pixel-identical.
2. ✅ `SettingsPage` categories + geometry; `tab-name` → `category-name` on the 17 sections.
3. ✅ Ten pages + `SettingsRouter`; drop `interface-page`, `services-page`, `settings-tabs`.
   `SectionCard` carries the gap under it, so a search hiding most cards leaves no holes.
4. ✅ `SettingsNav` (rows, search slot, rail button, ruler, compact verdict, morph, tip rect).
5. ✅ `settings-view.slint` chrome; `SearchBar.has-focus`; `TooltipFrame.gap`.
6. ✅ Rust: `SettingsCategory`, deep links, view tag, boot seed, `views.json` field and setter.
7. ✅ Translations: the new "Integrations" entry in all 15 catalogs.
8. ✅ Fix the tests the change breaks; fmt, clippy, test. Static gates green 2026-10-17
   (4067 tests).
9. ⏳ Manual pass (Kenan).

**Waits for a go:**
- new pins:
  - the rail scroller in `CARD_SHAPED_LISTS`;
  - the rail-first seed;
  - the retired `settings_tab` key;
  - the sidebar's `NavRow` mount;
  - the rail tip's mount and gate;
- doc updates: `CLAUDE.md`, `ui-patterns.md`, `slint-pitfalls.md` and the README's Settings paths,
  plus the screenshot caption;
- a retaken `assets/screenshots/settings.png`.

## Rail tooltips

- **When:** compact state only, on all ten category icons and the search button.
- **Placement:** a pill to the right of the card, `pad-md` off its edge.
- **One hover area over the list,** with the index taken from the pointer's `y`. So the gaps between
  rows can't drop it, and moving between icons moves the pill without a second delay.
- **The rect subtracts the rail's scroll offset.**
- **Mounted through a `TooltipFrame` declared last in the view,** so it paints over the cards.
