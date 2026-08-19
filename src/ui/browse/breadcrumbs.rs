//! Breadcrumb construction, folder basename, in-memory sort.

use std::path::{Path, PathBuf};

use slint::SharedString;

use crate::BreadcrumbRow as UiBreadcrumbRow;
use crate::entities::browse::BrowseFile;
use crate::entities::folder::Folder;
use crate::ui::track_sort::sort_track_rows_by;

/// Build a breadcrumb trail that starts at the **library folder root**
/// and walks down to `path`. The leading filesystem prefix is hidden so
/// the user sees `Music › sport` rather than `home › kenan › Music ›
/// sport` — matches Tauri's behaviour, where the library folder is
/// presented as the conceptual root.
///
/// When `path` is empty (root view) returns an empty Vec. When `path`
/// isn't under any enabled library folder (defensive — shouldn't
/// happen since `browse_directory` validates this), falls back to a
/// path-component walk so the user has *something* to click.
pub(super) fn build_breadcrumbs(path: &str, library_folders: &[Folder]) -> Vec<UiBreadcrumbRow> {
    if path.is_empty() {
        return Vec::new();
    }
    let p = Path::new(path);

    // Pick the deepest enabled library folder that is an ancestor of
    // (or equal to) `path`.
    let lib_root: Option<&Path> = library_folders
        .iter()
        .filter(|f| f.is_enabled)
        .filter_map(|f| {
            let fp = Path::new(&f.path);
            if p.starts_with(fp) { Some(fp) } else { None }
        })
        .max_by_key(|fp| fp.as_os_str().len());

    if let Some(lib_root) = lib_root {
        let mut out: Vec<UiBreadcrumbRow> = Vec::new();
        let root_name = lib_root.file_name().map_or_else(
            || lib_root.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut acc = lib_root.to_path_buf();
        out.push(UiBreadcrumbRow {
            label: SharedString::from(root_name.as_str()),
            path: SharedString::from(lib_root.to_string_lossy().as_ref()),
        });

        if let Ok(rel) = p.strip_prefix(lib_root) {
            for component in rel.components() {
                if let std::path::Component::Normal(seg) = component {
                    acc.push(seg);
                    out.push(UiBreadcrumbRow {
                        label: SharedString::from(seg.to_string_lossy().as_ref()),
                        path: SharedString::from(acc.to_string_lossy().as_ref()),
                    });
                }
            }
        }
        return out;
    }

    // Fallback: no library-folder ancestor matched. Walk the full path
    // so the user still has clickable segments. Defensive only.
    let mut acc = PathBuf::new();
    let mut out: Vec<UiBreadcrumbRow> = Vec::new();
    for component in p.components() {
        match component {
            std::path::Component::RootDir => {
                acc.push("/");
            }
            std::path::Component::Normal(seg) => {
                acc.push(seg);
                out.push(UiBreadcrumbRow {
                    label: SharedString::from(seg.to_string_lossy().as_ref()),
                    path: SharedString::from(acc.to_string_lossy().as_ref()),
                });
            }
            _ => {}
        }
    }
    out
}

/// Basename of a library-folder path (e.g. `/home/user/Music` → `Music`).
/// Used to label root-view folder rows. Falls back to the full path when
/// the basename can't be extracted (root paths, etc.).
pub(super) fn folder_basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Sort `files` in place by `field` / `dir`, using the file name as the
/// deterministic tie-breaker. Disk-only rows carry sparse `TrackListRow`s
/// (most fields empty / `0`), so they cluster but stay in a consistent
/// order. Delegates the shared `match field` shape to
/// [`crate::ui::track_sort::sort_track_rows_by`].
pub(super) fn sort_browse_files(files: &mut [BrowseFile], field: &str, dir: &str) {
    sort_track_rows_by(files, field, dir, |f| &f.row, |f| f.row.file_name.to_lowercase());
}
