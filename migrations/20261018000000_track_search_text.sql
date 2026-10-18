-- A track's five searchable fields, case- and accent-folded and joined, so the library-sized lists
-- can filter in SQL with a plain `instr`.
--
-- Stored rather than computed because the fold is Rust's (`SearchFields::pack`), and SQLite can
-- only call Rust here through a collation, never a scalar function. Every ingest path writes it,
-- and `tasks::search_text_backfill` fills the rows that predate it.
--
-- '' means not filled yet. A filled value never is, the separators being there whatever the fields
-- hold, so a change to the packing ships as a migration blanking the column and the backfill
-- refills it on the next launch.
ALTER TABLE tracks ADD COLUMN search_text TEXT NOT NULL DEFAULT '';

-- The rows the backfill still owes, so its check on every launch reads an index that is empty
-- once the library is filled rather than the whole table.
CREATE INDEX IF NOT EXISTS idx_tracks_search_text_unfilled ON tracks (id) WHERE search_text = '';
