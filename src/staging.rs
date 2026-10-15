//! The folders addons pass through on their way into or out of `AddOns`.
//!
//! They sit beside `AddOns` rather than inside it, so that the game never
//! scans them, and on the same filesystem, so that moving into and out of
//! them is a rename rather than a copy.
//!
//! - The staging folder holds copies on their way in and addons on their way
//!   out. Everything in it can be thrown away, and every run starts by
//!   emptying it.
//! - The old copies folder holds an installed addon while an update swaps
//!   the new copy in. That may be the only copy of the addon there is, so
//!   nothing ever deletes it: an old copy is only put back or set aside.
//!
//! A run holds a lock on the folder these sit in for as long as it uses
//! them, so that a second copy of the program, such as one started by
//! dropping files on the launcher icon, cannot empty them under the first.
//! Some network filesystems cannot lock a folder; there a run goes ahead
//! without the lock, as every run did before there was one, rather than not
//! at all.

use std::ffi::OsStr;
use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fs_util;
use crate::package::AddonName;

const STAGING_DIR: &str = ".simple-addons-staging";

const OLD_COPIES_DIR: &str = ".simple-addons-old-copies";

/// Where files that could not be deleted are moved, so that they cannot
/// stop later runs.
pub const LEFT_OVER_DIR: &str = ".simple-addons-left-over";

/// The staging folder, held by one run.
#[derive(Debug)]
pub struct Staging {
    path: PathBuf,
    /// The folder the staging folder sits in, opened to hold the lock. The
    /// lock goes when this is dropped. `None` where the filesystem cannot
    /// lock a folder.
    _lock: Option<File>,
}

impl Staging {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Files that could not be deleted, and where they are now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftBehind {
    pub path: PathBuf,
    /// Why they could not be deleted, such as "Permission denied".
    pub reason: String,
}

/// Takes the staging folder of an `AddOns` folder for one run, empty.
///
/// Fails with [`Error::Busy`] while another run holds it. Old copies an
/// unfinished update left are dealt with first, and whatever an earlier run
/// left in the staging folder is cleared or set aside.
pub fn prepare(addons_dir: &Path) -> Result<Staging> {
    let parent = parent_dir(addons_dir);
    let lock = lock(&parent)?;
    let old_copies = parent.join(OLD_COPIES_DIR);
    tidy_old_copies(&parent, &old_copies, addons_dir);
    fs_util::create_dir_all(&old_copies)?;
    let path = parent.join(STAGING_DIR);
    clear(&parent, &path);
    fs_util::create_dir_all(&path)?;
    Ok(Staging { path, _lock: lock })
}

/// Ends a run: clears the staging folder and lets the lock go.
///
/// This cannot fail. What cannot be deleted is set aside, and anything that
/// cannot even be moved is tried again by the next run.
pub fn finish(staging: Staging) {
    let parent = parent_dir(&staging.path);
    clear(&parent, &staging.path);
    // An old copy still here could not be put back, and has to stay. An
    // empty folder has no reason to.
    let _ = fs::remove_dir(parent.join(OLD_COPIES_DIR));
}

/// Where the old copy of an addon waits while an update swaps the new one in.
pub fn old_copy(staging: &Staging, name: &AddonName) -> PathBuf {
    parent_dir(&staging.path).join(OLD_COPIES_DIR).join(name)
}

/// Deletes a tree that was moved into the staging folder.
///
/// A tree can hold something this program may not delete, such as a
/// read-only folder an addon brought with it. What is left of it is then
/// moved out of the staging folder, where it would stop every later run, to
/// a folder of its own, and the error says where.
pub fn discard(staging: &Staging, tree: &Path) -> std::result::Result<(), LeftBehind> {
    let Err(error) = fs_util::remove_tree(tree) else {
        return Ok(());
    };
    let parent = parent_dir(&staging.path);
    Err(LeftBehind {
        path: set_aside(&parent, tree, fs_util::file_name(tree)),
        reason: reason(error),
    })
}

/// What a report says about files that could not be deleted.
pub fn describe_left_behind(left: &LeftBehind) -> String {
    format!(
        "some files could not be deleted ({}); they are in {}",
        left.reason,
        left.path.display()
    )
}

/// Why something could not be done to a tree, without the path, which no
/// longer holds once the tree has been moved.
pub fn reason(error: Error) -> String {
    match error {
        Error::Io { source, .. } => source.to_string(),
        other => other.to_string(),
    }
}

fn parent_dir(addons_dir: &Path) -> PathBuf {
    match addons_dir.parent() {
        Some(parent) => parent.to_path_buf(),
        None => addons_dir.to_path_buf(),
    }
}

/// Locks the folder the staging folder sits in, for one run. `None` when the
/// filesystem cannot lock a folder at all, which is no reason to stop.
fn lock(parent: &Path) -> Result<Option<File>> {
    let folder = File::open(parent).map_err(Error::io("open", parent))?;
    match folder.try_lock() {
        Ok(()) => Ok(Some(folder)),
        Err(TryLockError::WouldBlock) => Err(Error::Busy(parent.to_path_buf())),
        Err(TryLockError::Error(_)) => Ok(None),
    }
}

/// Deals with the old copies an earlier run left, which only happens when an
/// update did not finish.
///
/// When the addon is missing from `AddOns`, the update stopped between moving
/// the old copy out and moving the new one in, and putting the old copy back
/// undoes it. Otherwise something has taken its place since, and the old
/// copy is set aside. One that can be neither stays where it is, and an
/// update of that addon then says so.
fn tidy_old_copies(parent: &Path, old_copies: &Path, addons_dir: &Path) {
    if !is_real_folder(old_copies) {
        // Nothing there, or a link or file made by hand. Removing a link
        // removes only the link, and leaves what it leads to alone.
        let _ = fs_util::remove_tree(old_copies);
        return;
    }
    let Ok(entries) = fs_util::list_dir(old_copies) else {
        return;
    };
    for entry in entries {
        let name = fs_util::file_name(&entry.path);
        let home = addons_dir.join(name);
        let nothing_there = fs::symlink_metadata(&home).is_err();
        if !(nothing_there && fs_util::rename(&entry.path, &home).is_ok()) {
            set_aside(parent, &entry.path, name);
        }
    }
}

/// Deletes the staging folder, or sets it aside when that fails. A link put
/// in its place by hand is removed as a link, so what it leads to is never
/// emptied.
fn clear(parent: &Path, staging: &Path) {
    if fs_util::remove_tree(staging).is_err() {
        set_aside(parent, staging, OsStr::new("staging"));
    }
}

fn is_real_folder(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Moves a folder that could not be deleted into a numbered folder of its
/// own in [`LEFT_OVER_DIR`], under `name`, and says where it is now. A folder
/// that cannot be moved either stays where it is.
fn set_aside(parent: &Path, stuck: &Path, name: &OsStr) -> PathBuf {
    let Some(slot) = claim_slot(&parent.join(LEFT_OVER_DIR)) else {
        return stuck.to_path_buf();
    };
    let destination = slot.join(name);
    match fs_util::rename(stuck, &destination) {
        Ok(()) => destination,
        Err(_) => {
            let _ = fs::remove_dir(&slot);
            stuck.to_path_buf()
        }
    }
}

/// Makes a new numbered folder in `left_over`. Making the folder is what
/// claims the number, so two things set aside never share one.
fn claim_slot(left_over: &Path) -> Option<PathBuf> {
    fs_util::create_dir_all(left_over).ok()?;
    let mut number = 1u32;
    loop {
        let slot = left_over.join(number.to_string());
        match fs::create_dir(&slot) {
            Ok(()) => return Some(slot),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => number += 1,
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_util::test_support::{make_read_only, make_writable};

    fn addons_dir(root: &Path) -> PathBuf {
        let addons = root.join("Interface").join("AddOns");
        fs::create_dir_all(&addons).unwrap();
        addons
    }

    /// A folder holding a read-only folder that holds a file, which cannot be
    /// deleted until the mode changes back. `false` where the mode does not
    /// stop this user, so the test has to be skipped.
    fn undeletable(path: &Path) -> bool {
        fs::create_dir_all(path.join("ReadOnly")).unwrap();
        fs::write(path.join("ReadOnly").join("file.lua"), "").unwrap();
        make_read_only(&path.join("ReadOnly"))
    }

    #[test]
    fn sits_beside_the_addons_folder() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());

        let staging = prepare(&addons).unwrap();

        assert_eq!(
            staging.path(),
            root.path().join("Interface").join(STAGING_DIR)
        );
        assert!(staging.path().is_dir());
    }

    #[test]
    fn starts_empty_whatever_an_earlier_run_left() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let staging = prepare(&addons).unwrap();
        let path = staging.path().to_path_buf();
        fs::write(path.join("left-over.txt"), "").unwrap();
        drop(staging);

        let _staging = prepare(&addons).unwrap();

        assert!(fs_util::list_dir(&path).unwrap().is_empty());
    }

    #[test]
    fn a_second_run_cannot_take_it_while_the_first_holds_it() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let first = prepare(&addons).unwrap();

        let second = prepare(&addons);

        assert!(matches!(second, Err(Error::Busy(_))));
        finish(first);
        assert!(prepare(&addons).is_ok());
    }

    /// Leaves an old copy as an update killed between its two renames does:
    /// out of `AddOns`, waiting in the old copies folder.
    fn interrupted_update(addons: &Path, name: &str) -> PathBuf {
        let staging = prepare(addons).unwrap();
        let old = old_copy(&staging, &AddonName::new(name).unwrap());
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join(format!("{name}.toc")), "## Version: old\n").unwrap();
        // Dropped without finishing, as a killed run would.
        drop(staging);
        old
    }

    #[test]
    fn an_update_that_did_not_finish_is_undone_by_the_next_run() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        interrupted_update(&addons, "MyAddon");

        let _staging = prepare(&addons).unwrap();

        assert_eq!(
            fs::read_to_string(addons.join("MyAddon/MyAddon.toc")).unwrap(),
            "## Version: old\n"
        );
    }

    #[test]
    fn an_old_copy_whose_addon_is_back_in_place_is_set_aside_not_deleted() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        interrupted_update(&addons, "MyAddon");
        fs::create_dir_all(addons.join("MyAddon")).unwrap();
        fs::write(addons.join("MyAddon/MyAddon.toc"), "## Version: new\n").unwrap();

        let _staging = prepare(&addons).unwrap();

        let set_aside = root
            .path()
            .join("Interface")
            .join(LEFT_OVER_DIR)
            .join("1")
            .join("MyAddon");
        assert!(set_aside.join("MyAddon.toc").is_file());
        assert_eq!(
            fs::read_to_string(addons.join("MyAddon/MyAddon.toc")).unwrap(),
            "## Version: new\n"
        );
    }

    #[test]
    fn an_old_copy_that_can_be_neither_put_back_nor_set_aside_stays() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let old = interrupted_update(&addons, "MyAddon");
        fs::create_dir_all(addons.join("MyAddon")).unwrap();
        // With Interface read-only, no left-over folder can be made in it.
        if !make_read_only(&root.path().join("Interface")) {
            return;
        }

        let staging = prepare(&addons).unwrap();
        finish(staging);

        assert!(old.join("MyAddon.toc").is_file());
        make_writable(root.path());
    }

    #[test]
    fn a_staging_folder_made_into_a_link_is_unlinked_not_emptied() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir_all(elsewhere.join("MyAddon")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.path().join("Interface").join(STAGING_DIR))
            .unwrap();

        let staging = prepare(&addons).unwrap();

        assert!(elsewhere.join("MyAddon").is_dir());
        assert!(is_real_folder(staging.path()));
    }

    #[test]
    fn finishing_leaves_nothing_behind() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let staging = prepare(&addons).unwrap();
        fs::write(staging.path().join("copy.lua"), "").unwrap();

        finish(staging);

        let interface = addons.parent().unwrap();
        assert_eq!(fs_util::list_dir(interface).unwrap().len(), 1);
    }

    #[test]
    fn what_cannot_be_deleted_is_set_aside_and_named() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let staging = prepare(&addons).unwrap();
        let tree = staging.path().join("Locked");
        if !undeletable(&tree) {
            return;
        }

        let left = discard(&staging, &tree).unwrap_err();

        let left_over = root.path().join("Interface").join(LEFT_OVER_DIR);
        assert_eq!(left.path, left_over.join("1").join("Locked"));
        assert!(left.path.join("ReadOnly/file.lua").is_file());
        assert!(left.reason.contains("ermission denied"), "{}", left.reason);
        assert!(!tree.exists());
        make_writable(root.path());
    }

    #[test]
    fn a_stuck_staging_folder_does_not_stop_the_next_run() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let staging = prepare(&addons).unwrap();
        if !undeletable(&staging.path().join("Locked")) {
            return;
        }
        drop(staging);

        let staging = prepare(&addons).unwrap();

        assert!(fs_util::list_dir(staging.path()).unwrap().is_empty());
        let set_aside = root
            .path()
            .join("Interface")
            .join(LEFT_OVER_DIR)
            .join("1")
            .join("staging");
        assert!(set_aside.join("Locked/ReadOnly/file.lua").is_file());
        make_writable(root.path());
    }
}
