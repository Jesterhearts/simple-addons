//! Filesystem helpers shared by the staging and install steps.
//!
//! These wrap [`std::fs`] so that every failure names the path it happened
//! on, and so that directory listings arrive as plain data in a stable order.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// What a directory entry is, as reported without following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: EntryKind,
}

/// Lists a directory, sorted by name so that output and tests are repeatable.
pub fn list_dir(dir: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(Error::io("list", dir))? {
        let entry = entry.map_err(Error::io("list", dir))?;
        let kind = entry.file_type().map_err(Error::io("read", entry.path()))?;
        entries.push(Entry {
            path: entry.path(),
            kind: entry_kind(kind),
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn entry_kind(kind: fs::FileType) -> EntryKind {
    if kind.is_symlink() {
        EntryKind::Symlink
    } else if kind.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    }
}

/// Keeps only the subdirectories of a listing.
pub fn dirs_in(entries: Vec<Entry>) -> Vec<PathBuf> {
    entries
        .into_iter()
        .filter(|entry| entry.kind == EntryKind::Dir)
        .map(|entry| entry.path)
        .collect()
}

/// Keeps only the files of a listing. Symlinks count as files, because the
/// steps that read files are happy to follow them.
pub fn files_in(entries: Vec<Entry>) -> Vec<PathBuf> {
    entries
        .into_iter()
        .filter(|entry| entry.kind != EntryKind::Dir)
        .map(|entry| entry.path)
        .collect()
}

pub fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(Error::io("create", path))
}

/// Deletes a file or a whole directory tree. A path that is already gone is
/// not an error, since the point is to leave nothing behind.
pub fn remove_tree(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(Error::io("delete", path))
    } else {
        fs::remove_file(path).map_err(Error::io("delete", path))
    }
}

pub fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).map_err(Error::io("move", from))
}

/// Copies a directory tree, creating `to` and any missing parents.
pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    create_dir_all(to)?;
    for entry in list_dir(from)? {
        let target = to.join(file_name(&entry.path));
        match entry.kind {
            EntryKind::Dir => copy_tree(&entry.path, &target)?,
            EntryKind::File => copy_file(&entry.path, &target)?,
            EntryKind::Symlink => copy_link_target(&entry.path, &target)?,
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to).map_err(Error::io("copy", from))?;
    Ok(())
}

/// Copies what a symlink points at, as a plain file.
///
/// A link to a directory is skipped: following it could walk a cycle, and an
/// addon has no use for one. A broken link has nothing to copy.
fn copy_link_target(from: &Path, to: &Path) -> Result<()> {
    match fs::metadata(from) {
        Ok(metadata) if metadata.is_file() => copy_file(from, to),
        _ => Ok(()),
    }
}

/// The last component of a path, or an empty name for a path that has none.
pub fn file_name(path: &Path) -> &std::ffi::OsStr {
    path.file_name().unwrap_or_default()
}

/// The last component of a path as text, lossily converted so that a name
/// which is not valid UTF-8 still shows up in messages.
pub fn file_name_lossy(path: &Path) -> String {
    file_name(path).to_string_lossy().into_owned()
}

/// True when a path ends in the given extension, ignoring case.
pub fn has_extension(path: &Path, wanted: &str) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn lists_entries_sorted_with_their_kind() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("b.txt"), "b");
        write(&root.path().join("a/inner.txt"), "inner");

        let entries = list_dir(root.path()).unwrap();

        assert_eq!(
            entries,
            vec![
                Entry {
                    path: root.path().join("a"),
                    kind: EntryKind::Dir
                },
                Entry {
                    path: root.path().join("b.txt"),
                    kind: EntryKind::File
                },
            ]
        );
    }

    #[test]
    fn copies_a_tree_including_nested_files() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("from/top.txt"), "top");
        write(&root.path().join("from/deep/nested/leaf.txt"), "leaf");

        copy_tree(&root.path().join("from"), &root.path().join("to")).unwrap();

        assert_eq!(
            fs::read_to_string(root.path().join("to/top.txt")).unwrap(),
            "top"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("to/deep/nested/leaf.txt")).unwrap(),
            "leaf"
        );
    }

    #[test]
    fn removing_a_missing_path_succeeds() {
        let root = tempfile::tempdir().unwrap();

        assert!(remove_tree(&root.path().join("never-existed")).is_ok());
    }

    #[test]
    fn matches_extensions_ignoring_case() {
        assert!(has_extension(Path::new("Addon.TOC"), "toc"));
        assert!(!has_extension(Path::new("Addon.lua"), "toc"));
        assert!(!has_extension(Path::new("toc"), "toc"));
    }
}
