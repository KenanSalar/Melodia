# Smart / Dynamic Playlists

Smart playlists are playlists whose membership is **derived from user-defined rules**
(genre, rating, play count, favorite, last-played, date-added, …) instead of a manually
curated track list — the pattern iTunes / Apple Music / MusicBee / foobar2000 all ship.
Membership is re-evaluated live as the library changes; it is never stored.

The `playlists.is_smart` + `smart_criteria TEXT` columns have existed since the initial
migration but were an inert stub (every insert hardcoded `FALSE` / `NULL`, nothing read
them). This feature makes them live — **no new migration was needed**.

## How it works

A smart playlist stores a JSON [`SmartCriteria`](src/entities/smart_criteria.rs) rule set
in `smart_criteria` and **no `playlist_items` rows**. When opened, its tracks are resolved
by evaluating the criteria against `tracks` at read time — exactly how Favorites
(`WHERE is_favorite = TRUE`) and Recently-Played are query-derived virtual lists.

### Criteria model — `src/entities/smart_criteria.rs`

- `SmartCriteria { version, match_mode, rules, limit }` — `#[serde(default)]` + a `version`
  field for forward-compat; `from_json_opt` logs-and-defaults on a malformed blob (never
  panics).
- `match_mode`: `All` (AND / intersection) or `Any` (OR / union).
- `Rule { field, op, value }` — `RuleValue` is adjacently tagged (`Text` / `Number` /
  `Days`) so JSON is type-explicit and can't be coerced.
- `RuleField` (comprehensive, iTunes-like): Title, Artist, Album Artist, Album, Genre,
  Composer, Comment, Label, Year, BPM, Duration, Plays, Skips, Rating, Bitrate, Sample
  rate, File size, Favorite, Last played, Date added.
- `RuleOp`: contains / does-not-contain / is / is-not / starts-with / ends-with (text);
  the six comparators (numeric); in-the-last / not-in-the-last N days (relative date);
  is-true / is-false (boolean); is-set / is-not-set (presence, e.g. "never played").
- Optional `SmartLimit { count, order }` — cap to N tracks by a `LimitOrder` (most/least
  recently added, most/least played, most/least recently played, highest rated, random).

### Evaluation engine — `src/database/queries/smart_playlist.rs`

`get_smart_playlist_tracks` / `count_smart_playlist` build the query with
`sqlx::QueryBuilder`. **Safety contract:** only enum-derived `&'static str` fragments
(the column from `column_for`, the SQL operator token, the enum-bounded ORDER BY) are
pushed as raw SQL; every user value goes through `push_bind` — the same guarantee as
`AssertSqlSafe(sql) + .bind()`. Relative-date operators compare against an RFC-3339 cutoff
computed in Rust. Incoherent / incomplete rules are silently skipped; an empty rule set
matches the whole library.

### Backend — `src/library/smart_playlists.rs` + `src/database/queries/playlist.rs`

`create_smart_playlist` / `update_smart_criteria` persist `is_smart = TRUE` + the JSON and
bump `library_changed_tx`. `evaluate` / `count` wrap the engine. The detail open
(`src/ui/playlists/detail.rs::fetch_playlist_detail`) branches on `is_smart` — evaluator
vs. the existing junction query — and derives the header count/duration from the resolved
set (the `playlist_items` triggers can't fire for a virtual playlist). `fetch_grid`
overwrites smart-row counts via `count_smart_playlist`.

## UI — same grid & detail view, plus a rule builder

Smart playlists live in the **same Playlists grid and detail view** as manual ones (nav 7),
marked with an `auto_awesome` badge (grid `EntityCard.badge-icon` + detail header). For a
smart playlist the detail view gates off reorder, remove-from-playlist, and file-drop, and
swaps the empty state to "No tracks match these rules." An **Edit Rules** pill sits beside
Rename; a **New Smart Playlist** header pill creates one.

The rule builder is a modal (`Dialog.kind == "smart-playlist-editor"`,
`ui/components/dialog/smart-playlist-editor-body.slint`) driven by the `SmartEditor`
global. Rust owns the `VecModel<SmartRuleRow>` and reconstructs the criteria on commit
(`src/ui/callbacks/playlists/smart.rs`). Each row = field dropdown + operator dropdown +
a value widget chosen by the operator's input kind.

**Live updates:** the existing `library_changed_tx` subscriber (scans / watcher / CRUD /
favorite / rating) plus a new `stats_changed_tx` subscriber (`lifecycle.rs`, gated on
`has_smart_playlists()`) so play-count / last-played rules ("most played", "never played")
refresh without churning every view on each played song.

## Gotchas / load-bearing details

- **Index alignment:** the editor's inline `@tr` dropdown arrays (field names, the four
  per-value-type operator arrays, limit orders) mirror `entities::smart_criteria::{FIELDS,
  ops_for, LimitOrder}` **by position**. Reorder one side → reorder both.
- **New Slint globals must be re-exported from `app-window.slint`** (the `import` line *and*
  the `export { … }` block) or Slint's dead-code pass prunes them from the Rust API — that
  is why `SmartEditor` / `SmartRuleRow` are listed there.
- Entry points set `Dialog.*` chrome inline (so `@tr` applies) then call
  `SmartEditor.request-{new,edit}`, which populate state and open the dialog on a fresh
  event-loop tick — a synchronous `Dialog.open = true` from a click callback trips Slint's
  property-recursion guard.
- Smart playlists never write `playlist_items`; opening one registers **no** file-drop
  target.

## Tests

`src/database/queries/tests/smart_playlist_tests.rs` (membership per operator, match-all vs
match-any, limit + each order, relative-date cutoffs, count) and serde/forward-compat tests
in `src/entities/tests/smart_criteria_tests.rs`, all on `DbPool::test_pool()`.
