//! The breadcrumb trail, the root's folder labels and the in-memory sort.

use std::path::{Path, PathBuf};

use slint::SharedString;

use crate::ui::track_sort::sort_track_rows_by;
use melodia_core::entities::browse::BrowseFile;
use melodia_core::entities::folder::Folder;
use melodia_ui::BreadcrumbRow as UiBreadcrumbRow;

/// Builds the trail from the library folder down to `path`, hiding the filesystem above it, the
/// library folder being the user's root. Empty at the root. A `path` under no enabled folder,
/// which `browse_directory` already refuses, falls back to every path component, so there is
/// still something to click.
pub(super) fn build_breadcrumbs(path: &str, library_folders: &[Folder]) -> Vec<UiBreadcrumbRow> {
    if path.is_empty() {
        return Vec::new();
    }
    let p = Path::new(path);

    // The deepest, for a library folder nested inside another.
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

/// A library folder's label at the root: its basename, or the whole path where it has none.
pub(super) fn folder_basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Sorts `files` in place, ties broken by file name, so the sparse disk-only rows cluster in a
/// stable order.
pub(super) fn sort_browse_files(files: &mut [BrowseFile], field: &str, dir: &str) {
    sort_track_rows_by(files, field, dir, |f| &f.row, |f| f.file_name.to_lowercase());
}
