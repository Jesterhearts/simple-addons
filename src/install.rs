//! Writing addon folders into the game's `Interface/AddOns` folder.
//!
//! Each addon is first copied next to its destination and then swapped in
//! with renames. A rename within one filesystem either happens or does not,
//! so the game never finds a half written addon where a working one used to
//! be, however the installer is interrupted.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fs_util;
use crate::package::{AddonName, AddonSource, Toc};
use crate::staging::{self, LeftBehind, Staging};

/// What installing one addon will do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonPlan {
    pub name: AddonName,
    pub source: PathBuf,
    pub destination: PathBuf,
    /// True when an addon of this name is already installed and will be
    /// replaced.
    pub replaces_existing: bool,
    pub toc: Toc,
}

/// Everything one run will write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub addons_dir: PathBuf,
    pub addons: Vec<AddonPlan>,
}

/// What installing one addon did. Addons are reported one by one, so that one
/// failure does not hide the rest of the run.
#[derive(Debug)]
pub struct Outcome {
    pub addon: String,
    pub toc: Toc,
    pub result: Result<Installed>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub destination: PathBuf,
    /// True when this replaced an addon that was already installed.
    pub replaced: bool,
    /// Files of the replaced copy that could not be deleted, and where they
    /// were moved instead.
    pub left_behind: Option<LeftBehind>,
}

/// Works out what installing these addons would do.
pub fn plan(addons_dir: &Path, addons: &[AddonSource]) -> Plan {
    Plan {
        addons_dir: addons_dir.to_path_buf(),
        addons: addons
            .iter()
            .map(|addon| plan_addon(addons_dir, addon))
            .collect(),
    }
}

fn plan_addon(addons_dir: &Path, addon: &AddonSource) -> AddonPlan {
    let destination = addons_dir.join(&addon.name);
    AddonPlan {
        name: addon.name.clone(),
        source: addon.path.clone(),
        replaces_existing: destination.exists(),
        destination,
        toc: addon.toc.clone(),
    }
}

/// Carries out a plan.
///
/// The error type covers only the setup that has to succeed before anything
/// is written. Once that is done, every addon is attempted and reports for
/// itself.
pub fn execute(plan: &Plan) -> Result<Vec<Outcome>> {
    fs_util::create_dir_all(&plan.addons_dir)?;
    let staging = staging::prepare(&plan.addons_dir)?;

    let outcomes = plan
        .addons
        .iter()
        .map(|addon| Outcome {
            addon: addon.name.to_string(),
            toc: addon.toc.clone(),
            result: install_addon(&staging, addon),
        })
        .collect();

    // Every addon has reported for itself by now, so tidying up must not
    // turn into an error that would hide those reports.
    staging::finish(staging);
    Ok(outcomes)
}

fn install_addon(staging: &Staging, plan: &AddonPlan) -> Result<Installed> {
    let incoming = staging.path().join(&plan.name);
    fs_util::remove_tree(&incoming)?;
    fs_util::copy_tree(&plan.source, &incoming)?;

    let replaced = plan.destination.exists();
    let left_behind = if replaced {
        swap_in(&incoming, &plan.destination, staging, &plan.name)?
    } else {
        fs_util::rename(&incoming, &plan.destination)?;
        None
    };

    Ok(Installed {
        destination: plan.destination.clone(),
        replaced,
        left_behind,
    })
}

/// Replaces an installed addon with the one just copied, and says where any
/// files of the old one that could not be deleted are now.
///
/// The old folder is moved aside rather than deleted, so that a failed swap
/// can put it back instead of leaving the user with no addon at all.
fn swap_in(
    incoming: &Path,
    destination: &Path,
    staging: &Staging,
    name: &AddonName,
) -> Result<Option<LeftBehind>> {
    let previous = staging::old_copy(staging, name);
    // Every run starts by putting back or setting aside each old copy, so
    // one still here could be neither. Moving over it could lose the only
    // copy of that addon.
    if fs::symlink_metadata(&previous).is_ok() {
        return Err(Error::OldCopyInTheWay(previous));
    }
    fs_util::rename(destination, &previous)?;
    if let Err(error) = fs_util::rename(incoming, destination) {
        // Putting the old addon back matters more than the original failure,
        // so a failure to restore is what gets reported. The old copy stays
        // where nothing deletes it, and the next run puts it back if nothing
        // has taken its place.
        if let Err(restore) = fs_util::rename(&previous, destination) {
            return Err(Error::NotRestored {
                destination: destination.to_path_buf(),
                kept: previous,
                reason: staging::reason(restore),
            });
        }
        return Err(error);
    }
    Ok(retire_old_copy(staging, &previous, incoming))
}

/// Deletes the old copy once the update is done, and says where any of it
/// that could not be deleted is now.
///
/// It goes back through the staging folder first, to where the new copy was
/// assembled, so that whatever of it cannot be deleted is never taken for an
/// old copy to put back.
fn retire_old_copy(staging: &Staging, previous: &Path, assembled_at: &Path) -> Option<LeftBehind> {
    let doomed = match fs_util::rename(previous, assembled_at) {
        Ok(()) => assembled_at,
        Err(_) => previous,
    };
    staging::discard(staging, doomed).err()
}

/// What a report says after the name of an addon it installed: the version,
/// and where any files of the copy it replaced that could not be deleted
/// are now.
pub fn describe_installed(toc: &Toc, installed: &Installed) -> String {
    let mut parts = vec![toc.version.clone().unwrap_or_default()];
    parts.extend(
        installed
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

    use super::*;
    use crate::fs_util::test_support::{make_read_only, make_writable};

    /// Builds the `Interface/AddOns` folder of a game version.
    fn addons_dir(root: &Path) -> PathBuf {
        let dir = root.join("_retail_").join("Interface").join("AddOns");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn addon_source(root: &Path, name: &str, version: &str) -> AddonSource {
        let path = root.join("sources").join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join(format!("{name}.toc")),
            format!("## Version: {version}\n"),
        )
        .unwrap();
        fs::write(path.join("Core.lua"), format!("-- {version}\n")).unwrap();
        AddonSource {
            name: AddonName::new(name).unwrap(),
            path,
            toc: crate::package::toc::parse(&format!("## Version: {version}\n")),
        }
    }

    #[test]
    fn installs_an_addon_that_is_not_there_yet() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let source = addon_source(root.path(), "MyAddon", "1.0");

        let outcomes = execute(&plan(&addons, &[source])).unwrap();

        let installed = outcomes[0].result.as_ref().unwrap();
        assert_eq!(installed.destination, addons.join("MyAddon"));
        assert!(!installed.replaced);
        assert_eq!(
            fs::read_to_string(addons.join("MyAddon/Core.lua")).unwrap(),
            "-- 1.0\n"
        );
    }

    #[test]
    fn replaces_an_addon_that_is_already_installed() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        execute(&plan(
            &addons,
            &[addon_source(root.path(), "MyAddon", "1.0")],
        ))
        .unwrap();
        fs::write(addons.join("MyAddon/Stale.lua"), "-- old file\n").unwrap();

        let outcomes = execute(&plan(
            &addons,
            &[addon_source(root.path(), "MyAddon", "2.0")],
        ))
        .unwrap();

        assert!(outcomes[0].result.as_ref().unwrap().replaced);
        assert_eq!(
            fs::read_to_string(addons.join("MyAddon/Core.lua")).unwrap(),
            "-- 2.0\n"
        );
        assert!(
            !addons.join("MyAddon/Stale.lua").exists(),
            "files dropped by the new version should not survive"
        );
    }

    #[test]
    fn an_old_copy_that_cannot_be_deleted_does_not_fail_the_update() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        execute(&plan(
            &addons,
            &[addon_source(root.path(), "MyAddon", "1.0")],
        ))
        .unwrap();
        fs::create_dir_all(addons.join("MyAddon/ReadOnly")).unwrap();
        fs::write(addons.join("MyAddon/ReadOnly/Old.lua"), "").unwrap();
        if !make_read_only(&addons.join("MyAddon/ReadOnly")) {
            return;
        }

        let outcomes = execute(&plan(
            &addons,
            &[addon_source(root.path(), "MyAddon", "2.0")],
        ))
        .unwrap();
        let again = execute(&plan(
            &addons,
            &[addon_source(root.path(), "MyAddon", "3.0")],
        ))
        .unwrap();

        let installed = outcomes[0].result.as_ref().unwrap();
        let left = installed.left_behind.as_ref().unwrap();
        assert!(left.path.join("ReadOnly/Old.lua").is_file());
        assert!(
            describe_installed(&outcomes[0].toc, installed)
                .starts_with("2.0; some files could not")
        );
        assert!(again[0].result.is_ok(), "the next update is not blocked");
        assert_eq!(
            fs::read_to_string(addons.join("MyAddon/Core.lua")).unwrap(),
            "-- 3.0\n"
        );
        make_writable(root.path());
    }

    #[test]
    fn an_update_does_not_delete_an_old_copy_it_could_not_keep_safe() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        execute(&plan(
            &addons,
            &[addon_source(root.path(), "MyAddon", "1.0")],
        ))
        .unwrap();
        let interface = addons.parent().unwrap();
        let old = interface.join(".simple-addons-old-copies/MyAddon");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("Only.lua"), "").unwrap();
        fs::create_dir_all(interface.join(".simple-addons-staging")).unwrap();
        // MyAddon is installed, so the old copy cannot go back, and with
        // Interface read-only it cannot be set aside either.
        if !make_read_only(interface) {
            return;
        }

        let outcomes = execute(&plan(
            &addons,
            &[addon_source(root.path(), "MyAddon", "2.0")],
        ))
        .unwrap();

        assert!(matches!(outcomes[0].result, Err(Error::OldCopyInTheWay(_))));
        assert!(old.join("Only.lua").is_file());
        assert_eq!(
            fs::read_to_string(addons.join("MyAddon/Core.lua")).unwrap(),
            "-- 1.0\n"
        );
        make_writable(root.path());
    }

    #[test]
    fn records_in_the_plan_which_addons_are_already_installed() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        fs::create_dir_all(addons.join("Known")).unwrap();
        let sources = [
            addon_source(root.path(), "Known", "2.0"),
            addon_source(root.path(), "New", "1.0"),
        ];

        let plan = plan(&addons, &sources);

        assert!(plan.addons[0].replaces_existing);
        assert!(!plan.addons[1].replaces_existing);
    }

    #[test]
    fn creates_the_addons_folder_when_the_game_version_has_none() {
        let root = tempfile::tempdir().unwrap();
        let addons = root
            .path()
            .join("_retail_")
            .join("Interface")
            .join("AddOns");
        let source = addon_source(root.path(), "MyAddon", "1.0");

        execute(&plan(&addons, &[source])).unwrap();

        assert!(addons.join("MyAddon/MyAddon.toc").is_file());
    }

    #[test]
    fn leaves_nothing_behind_in_the_game_folder() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let source = addon_source(root.path(), "MyAddon", "1.0");

        execute(&plan(&addons, &[source])).unwrap();

        let interface = addons.parent().unwrap();
        let left_over = fs_util::list_dir(interface).unwrap();
        assert_eq!(
            left_over.len(),
            1,
            "only AddOns should remain: {left_over:?}"
        );
    }

    #[test]
    fn installs_the_rest_when_one_addon_fails() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let good = addon_source(root.path(), "Good", "1.0");
        let mut missing = addon_source(root.path(), "Missing", "1.0");
        missing.path = root.path().join("sources").join("not-there");

        let outcomes = execute(&plan(&addons, &[good, missing])).unwrap();

        assert!(outcomes[0].result.is_ok());
        assert!(outcomes[1].result.is_err());
        assert!(addons.join("Good/Core.lua").is_file());
    }
}
