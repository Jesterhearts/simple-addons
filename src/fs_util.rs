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

/// True for a directory, and for a symlink that leads to one. The game
/// follows links, so where it looks for folders a link to a folder is one.
pub fn is_folder(entry: &Entry) -> bool {
    match entry.kind {
        EntryKind::Dir => true,
        EntryKind::Symlink => entry.path.is_dir(),
        EntryKind::File => false,
    }
}

/// Keeps the folders of a listing, counting links to folders.
pub fn folders_in(entries: Vec<Entry>) -> Vec<PathBuf> {
    entries
        .into_iter()
        .filter(is_folder)
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

/// Helpers for tests that need something which cannot be deleted.
#[cfg(test)]
pub mod test_support {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// Makes a folder read-only, so that nothing in it can be deleted, and
    /// says whether that worked. It does not work for root, whom the mode
    /// does not bind, so a test that relies on it has to be skipped there.
    pub fn make_read_only(dir: &Path) -> bool {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
        let probe = dir.join(".probe");
        if fs::write(&probe, "").is_ok() {
            fs::remove_file(&probe).unwrap();
            eprintln!("skipped: a read-only folder does not stop this user");
            return false;
        }
        true
    }

    /// Makes every folder under `root` writable again, so that the test can
    /// tidy up after itself.
    pub fn make_writable(root: &Path) {
        for entry in super::list_dir(root).unwrap() {
            if entry.kind == super::EntryKind::Dir {
                fs::set_permissions(&entry.path, fs::Permissions::from_mode(0o755)).unwrap();
                make_writable(&entry.path);
            }
        }
    }
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
