-- Files the duplicate finder took out of the library while leaving them on disk. No scan or watcher
-- event brings one back; an import of the file does, and lifts its row.
--
-- A path here never has a track row. Each row follows its file as a track row would: a rename
-- re-points it, and a new path carrying its file_hash, where its own path is gone, is the same file
-- moved. file_size and date_modified are the stamp a scan compares, as a track's are, so a file
-- rewritten in place is read again and its file_hash kept current.
CREATE TABLE IF NOT EXISTS excluded_files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    file_hash TEXT,
    file_size INTEGER,
    date_modified TEXT,
    folder_id INTEGER NOT NULL REFERENCES folders (id) ON DELETE CASCADE,
    excluded_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_excluded_files_folder ON excluded_files (folder_id);

CREATE INDEX IF NOT EXISTS idx_excluded_files_hash ON excluded_files (file_hash)
WHERE file_hash IS NOT NULL;
