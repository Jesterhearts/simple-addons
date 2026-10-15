//! Removing addons from the game's `Interface/AddOns` folder.
//!
//! Each addon is moved out of `AddOns` with a rename before it is deleted. A
//! rename within one filesystem either happens or does not, so the game never
//! finds part of an addon where a whole one used to be, however the removal
//! is interrupted.
//!
//! The settings the game saved for an addon live in `WTF/`, not in its
//! folder, so they are kept for when the addon is installed again.

use std::path::Path;

use crate::error::{Error, Result};
use crate::installed::InstalledAddon;
use crate::staging::{self, LeftBehind, Staging};

/// What removing one addon did. Addons are reported one by one, so that one
/// failure does not hide the rest of the run.
#[derive(Debug)]
pub struct Outcome {
    pub addon: InstalledAddon,
    pub result: Result<Removed>,
}

/// An addon that is out of `AddOns`, so the game no longer loads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// Files of the addon that could not be deleted, and where they were
    /// moved instead. `None` when the addon is gone entirely.
    pub left_behind: Option<LeftBehind>,
}

/// Removes addons that were listed from `addons_dir`.
///
/// The error type covers only the setup that has to succeed before anything
/// is moved. Once that is done, every addon is attempted and reports for
/// itself.
pub fn execute(addons_dir: &Path, addons: &[InstalledAddon]) -> Result<Vec<Outcome>> {
    let staging = staging::prepare(addons_dir)?;
    let outcomes = addons
        .iter()
        .map(|addon| Outcome {
            addon: addon.clone(),
            result: remove_addon(&staging, addons_dir, addon),
        })
        .collect();
    staging::finish(staging);
    Ok(outcomes)
}

/// Moves one addon out of `AddOns`, then deletes it. A link is moved and
/// deleted as a link, so the folder it leads to is left alone.
fn remove_addon(staging: &Staging, addons_dir: &Path, addon: &InstalledAddon) -> Result<Removed> {
    // The run's lock covers the staging folder beside `addons_dir` and no
    // other, so an addon listed from some other folder must not use it.
    if addon.path().parent() != Some(addons_dir) {
        return Err(Error::NotInstalled {
            addon: addon.name().to_string(),
            addons_dir: addons_dir.to_path_buf(),
        });
    }
    let outgoing = staging.path().join(crate::fs_util::file_name(addon.path()));
    crate::fs_util::rename(addon.path(), &outgoing)?;
    Ok(Removed {
        left_behind: staging::discard(staging, &outgoing).err(),
    })
}

/// What a report says after the name of an addon it is about to remove: the
/// version and, for a link, that the folder it leads to stays.
pub fn describe(addon: &InstalledAddon) -> String {
    let version = addon.version().unwrap_or_default();
    let link = if addon.is_link() {
        " (only the link; the folder it leads to stays)"
    } else {
        ""
    };
    format!("{version}{link}").trim_start().to_string()
}

/// What a report says after the name of an addon it removed, which adds
/// where any files that could not be deleted are now.
pub fn describe_removed(addon: &InstalledAddon, removed: &Removed) -> String {
    let mut parts = vec![describe(addon)];
    parts.extend(
        removed
            .left_behind
            .as_ref()
            .map(staging::describe_left_behind),
    );
    parts.retain(|part| !part.is_empty());
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    use super::*;
    use crate::fs_util::test_support::{make_read_only, make_writable};
    use crate::{fs_util, installed};

    /// Builds the `Interface/AddOns` folder of a game version.
    fn addons_dir(root: &Path) -> PathBuf {
        let dir = root.join("_retail_").join("Interface").join("AddOns");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put_addon(addons: &Path, name: &str) {
        fs::create_dir_all(addons.join(name).join("Libs")).unwrap();
        fs::write(addons.join(name).join(format!("{name}.toc")), "").unwrap();
        fs::write(addons.join(name).join("Libs").join("Lib.lua"), "").unwrap();
    }

    fn named(addons: &Path, wanted: &[&str]) -> Vec<InstalledAddon> {
        installed::list(addons)
            .unwrap()
            .into_iter()
            .filter(|addon| wanted.contains(&addon.name()))
            .collect()
    }

    fn left(addons: &Path) -> Vec<String> {
        installed::list(addons)
            .unwrap()
            .iter()
            .map(|addon| addon.name().to_string())
            .collect()
    }

    #[test]
    fn removes_the_chosen_addons_and_nothing_else() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        put_addon(&addons, "Details");
        put_addon(&addons, "WeakAuras");
        put_addon(&addons, "WeakAurasOptions");

        let outcomes =
            execute(&addons, &named(&addons, &["WeakAuras", "WeakAurasOptions"])).unwrap();

        for outcome in &outcomes {
            assert_eq!(outcome.result.as_ref().unwrap().left_behind, None);
        }
        assert_eq!(left(&addons), vec!["Details"]);
    }

    #[test]
    fn leaves_nothing_behind_in_the_game_folder() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        put_addon(&addons, "MyAddon");

        execute(&addons, &named(&addons, &["MyAddon"])).unwrap();

        let interface = addons.parent().unwrap();
        let left_over = fs_util::list_dir(interface).unwrap();
        assert_eq!(
            left_over.len(),
            1,
            "only AddOns should remain: {left_over:?}"
        );
    }

    #[test]
    fn keeps_the_settings_the_game_saved() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        put_addon(&addons, "MyAddon");
        let saved = root
            .path()
            .join("_retail_/WTF/Account/ME/SavedVariables/MyAddon.lua");
        fs::create_dir_all(saved.parent().unwrap()).unwrap();
        fs::write(&saved, "MyAddonDB = {}\n").unwrap();

        execute(&addons, &named(&addons, &["MyAddon"])).unwrap();

        assert!(!addons.join("MyAddon").exists());
        assert!(saved.is_file());
    }

    #[test]
    fn removing_a_link_keeps_the_folder_it_leads_to() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let checkout = root.path().join("checkout");
        put_addon(&checkout, "MyAddon");
        symlink(checkout.join("MyAddon"), addons.join("MyAddon")).unwrap();

        let outcomes = execute(&addons, &named(&addons, &["MyAddon"])).unwrap();

        assert!(outcomes[0].result.is_ok());
        assert!(fs::symlink_metadata(addons.join("MyAddon")).is_err());
        assert!(checkout.join("MyAddon/Libs/Lib.lua").is_file());
    }

    #[test]
    fn links_inside_an_addon_are_removed_without_following_them() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("keep.txt"), "").unwrap();
        put_addon(&addons, "MyAddon");
        symlink(&elsewhere, addons.join("MyAddon/Libs/Absolute")).unwrap();
        symlink(
            "../../../../../elsewhere",
            addons.join("MyAddon/Libs/Relative"),
        )
        .unwrap();

        execute(&addons, &named(&addons, &["MyAddon"])).unwrap();

        assert!(!addons.join("MyAddon").exists());
        assert!(elsewhere.join("keep.txt").is_file());
    }

    #[test]
    fn removes_the_rest_when_one_addon_fails() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        put_addon(&addons, "Gone");
        put_addon(&addons, "Here");
        let listed = named(&addons, &["Gone", "Here"]);
        // Something else removes it between listing and removing.
        fs::remove_dir_all(addons.join("Gone")).unwrap();

        let outcomes = execute(&addons, &listed).unwrap();

        assert_eq!(outcomes[0].addon.name(), "Gone");
        assert!(outcomes[0].result.is_err());
        assert!(outcomes[1].result.is_ok());
        assert!(!addons.join("Here").exists());
    }

    #[test]
    fn files_that_cannot_be_deleted_are_set_aside_and_later_runs_still_work() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        put_addon(&addons, "Locked");
        put_addon(&addons, "Other");
        if !make_read_only(&addons.join("Locked/Libs")) {
            return;
        }

        let outcomes = execute(&addons, &named(&addons, &["Locked"])).unwrap();
        let again = execute(&addons, &named(&addons, &["Other"])).unwrap();

        let removed = outcomes[0].result.as_ref().unwrap();
        let left_behind = removed.left_behind.as_ref().unwrap();
        assert!(left_behind.path.join("Libs/Lib.lua").is_file());
        assert!(describe_removed(&outcomes[0].addon, removed).contains("could not be deleted"));
        assert!(again[0].result.is_ok(), "the next run is not blocked");
        assert!(left(&addons).is_empty());
        make_writable(root.path());
    }

    #[test]
    fn an_addon_listed_from_another_folder_is_not_removed() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let other = root.path().join("_classic_era_/Interface/AddOns");
        fs::create_dir_all(&other).unwrap();
        put_addon(&other, "MyAddon");

        let outcomes = execute(&addons, &named(&other, &["MyAddon"])).unwrap();

        assert!(matches!(
            outcomes[0].result,
            Err(Error::NotInstalled { .. })
        ));
        assert!(other.join("MyAddon").exists());
    }

    #[test]
    fn a_report_names_the_version_and_whether_only_a_link_goes() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        fs::create_dir_all(addons.join("Plain")).unwrap();
        fs::write(addons.join("Plain/Plain.toc"), "## Version: 1.0\n").unwrap();
        put_addon(&root.path().join("checkout"), "Linked");
        symlink(root.path().join("checkout/Linked"), addons.join("Linked")).unwrap();
        let listed = installed::list(&addons).unwrap();

        let described: Vec<String> = listed.iter().map(describe).collect();

        assert_eq!(
            described,
            vec![
                "(only the link; the folder it leads to stays)".to_string(),
                "1.0".to_string()
            ]
        );
    }
}
