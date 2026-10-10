//! Writing addon folders into the game's `Interface/AddOns` folder.
//!
//! Each addon is first copied next to its destination and then swapped in
//! with renames. A rename within one filesystem either happens or does not,
//! so the game never finds a half written addon where a working one used to
//! be, however the installer is interrupted.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::fs_util;
use crate::package::{AddonSource, Toc};

/// What installing one addon will do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonPlan {
    pub name: String,
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
}

/// The folder used to assemble addons before they are moved into place.
///
/// It sits beside `AddOns` rather than inside it, so that the game never
/// scans it, and on the same filesystem, so that moving out of it is a
/// rename rather than a copy.
const STAGING_DIR: &str = ".simple-addons-staging";

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
    let staging = staging_dir(&plan.addons_dir);
    fs_util::create_dir_all(&plan.addons_dir)?;
    // An earlier run may have been killed before it could tidy up.
    fs_util::remove_tree(&staging)?;
    fs_util::create_dir_all(&staging)?;

    let outcomes = plan
        .addons
        .iter()
        .map(|addon| Outcome {
            addon: addon.name.clone(),
            toc: addon.toc.clone(),
            result: install_addon(&staging, addon),
        })
        .collect();

    fs_util::remove_tree(&staging)?;
    Ok(outcomes)
}

fn staging_dir(addons_dir: &Path) -> PathBuf {
    match addons_dir.parent() {
        Some(parent) => parent.join(STAGING_DIR),
        None => addons_dir.join(STAGING_DIR),
    }
}

fn install_addon(staging: &Path, plan: &AddonPlan) -> Result<Installed> {
    let incoming = staging.join(&plan.name);
    fs_util::remove_tree(&incoming)?;
    fs_util::copy_tree(&plan.source, &incoming)?;

    let replaced = plan.destination.exists();
    if replaced {
        swap_in(&incoming, &plan.destination, staging, &plan.name)?;
    } else {
        fs_util::rename(&incoming, &plan.destination)?;
    }

    Ok(Installed {
        destination: plan.destination.clone(),
        replaced,
    })
}

/// Replaces an installed addon with the one just copied.
///
/// The old folder is moved aside rather than deleted, so that a failed swap
/// can put it back instead of leaving the user with no addon at all.
fn swap_in(incoming: &Path, destination: &Path, staging: &Path, name: &str) -> Result<()> {
    let previous = staging.join(format!("{name}.previous"));
    fs_util::remove_tree(&previous)?;
    fs_util::rename(destination, &previous)?;
    if let Err(error) = fs_util::rename(incoming, destination) {
        // Putting the old addon back matters more than the original failure,
        // so a failure to restore is what gets reported.
        fs_util::rename(&previous, destination)?;
        return Err(error);
    }
    fs_util::remove_tree(&previous)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

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
            name: name.to_string(),
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
