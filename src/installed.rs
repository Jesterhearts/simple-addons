//! What is installed in a game version's `Interface/AddOns` folder.
//!
//! No record is kept of what this program installed: the folder is the
//! record. Every folder in it is an installed addon, whichever program put it
//! there, and its name is the name the game knows it by.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fs_util::{self, Entry, EntryKind};
use crate::package::{Toc, detect};

/// One folder in `Interface/AddOns`.
///
/// Only [`list`] makes these, from what is really in the folder. Removing an
/// addon deletes what its path points at, so a path built from a name the
/// user typed must never be able to pose as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAddon {
    name: String,
    path: PathBuf,
    toc: Option<Toc>,
    link: bool,
}

impl InstalledAddon {
    /// The folder name, which is the name the game knows the addon by.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the addon says about itself, or `None` for a folder holding no
    /// toc file named after it, which the game does not load.
    pub fn toc(&self) -> Option<&Toc> {
        self.toc.as_ref()
    }

    pub fn version(&self) -> Option<&str> {
        self.toc.as_ref()?.version.as_deref()
    }

    /// True when the folder is a symlink to an addon kept somewhere else, as
    /// a developer's working copy often is. Removing it removes only the
    /// link.
    pub fn is_link(&self) -> bool {
        self.link
    }

    fn dependencies(&self) -> &[String] {
        match &self.toc {
            Some(toc) => &toc.dependencies,
            None => &[],
        }
    }
}

/// What a list says about an addon after its version: its title, or why the
/// game does not load it, and whether it is a link.
pub fn describe(addon: &InstalledAddon) -> String {
    let mut note = match addon.toc() {
        Some(toc) => toc.title.clone().unwrap_or_default(),
        None => NOT_LOADED.to_string(),
    };
    if addon.is_link() {
        note.push_str(" (link)");
    }
    note.trim_start().to_string()
}

/// What a list says about a folder that holds no toc file named after it.
pub const NOT_LOADED: &str = "not loaded by the game: no toc file is named after the folder";

/// Lists the addons in an `AddOns` folder in alphabetical order, ignoring
/// case, since that is how a person looks for a name in a list.
pub fn list(addons_dir: &Path) -> Result<Vec<InstalledAddon>> {
    let mut addons: Vec<InstalledAddon> = addon_folders(addons_dir)?
        .into_iter()
        .map(read_addon)
        .collect();
    addons.sort_by_cached_key(|addon| (addon.name.to_lowercase(), addon.name.clone()));
    Ok(addons)
}

/// How many addons an `AddOns` folder holds, found without reading their toc
/// files.
pub fn count(addons_dir: &Path) -> Result<usize> {
    Ok(addon_folders(addons_dir)?.len())
}

/// The entries of an `AddOns` folder that are addon folders. A symlink to a
/// folder counts, because the game follows it.
fn addon_folders(addons_dir: &Path) -> Result<Vec<Entry>> {
    Ok(fs_util::list_dir(addons_dir)?
        .into_iter()
        .filter(fs_util::is_folder)
        .collect())
}

fn read_addon(entry: Entry) -> InstalledAddon {
    let name = fs_util::file_name_lossy(&entry.path);
    InstalledAddon {
        toc: detect::installed_toc(&entry.path, &name),
        link: entry.kind == EntryKind::Symlink,
        name,
        path: entry.path,
    }
}

/// Finds the installed addon a user asked for by name.
///
/// Capital letters are easy to get wrong when typing a name, so they only
/// count when they are needed to tell two folders apart.
pub fn find<'a>(
    installed: &'a [InstalledAddon],
    requested: &str,
    addons_dir: &Path,
) -> Result<&'a InstalledAddon> {
    if let Some(exact) = installed.iter().find(|addon| addon.name == requested) {
        return Ok(exact);
    }
    let matches: Vec<&InstalledAddon> = installed
        .iter()
        .filter(|addon| addon.name.eq_ignore_ascii_case(requested))
        .collect();
    match matches.as_slice() {
        [only] => Ok(only),
        [] => Err(Error::NotInstalled {
            addon: requested.to_string(),
            addons_dir: addons_dir.to_path_buf(),
        }),
        several => Err(Error::AmbiguousAddon {
            requested: requested.to_string(),
            matches: several
                .iter()
                .map(|addon| addon.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            addons_dir: addons_dir.to_path_buf(),
        }),
    }
}

/// The installed addons that stop loading once `removing` is gone.
///
/// An addon stops loading when it names a removed addon as a required
/// dependency. An addon that needs one of those stops loading too, so the
/// search repeats until it finds no more. The answer leaves out `removing`
/// itself and keeps the order of `installed`.
pub fn dependents(
    installed: &[InstalledAddon],
    removing: &[InstalledAddon],
) -> Vec<InstalledAddon> {
    let removed: HashSet<String> = removing.iter().map(|addon| folded(&addon.name)).collect();
    let mut gone = removed.clone();
    loop {
        let newly: Vec<String> = installed
            .iter()
            .filter(|addon| !gone.contains(&folded(&addon.name)) && needs_any(addon, &gone))
            .map(|addon| folded(&addon.name))
            .collect();
        if newly.is_empty() {
            break;
        }
        gone.extend(newly);
    }
    installed
        .iter()
        .filter(|addon| {
            let name = folded(&addon.name);
            gone.contains(&name) && !removed.contains(&name)
        })
        .cloned()
        .collect()
}

fn needs_any(addon: &InstalledAddon, gone: &HashSet<String>) -> bool {
    addon
        .dependencies()
        .iter()
        .any(|dependency| gone.contains(&folded(dependency)))
}

/// A name in the form names are compared in. The game runs on filesystems
/// that ignore case, so a dependency written `weakauras` means `WeakAuras`.
fn folded(name: &str) -> String {
    name.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;

    /// Installs an addon by hand: a folder holding a toc file named after it.
    fn put_addon(addons: &Path, name: &str, toc: &str) {
        fs::create_dir_all(addons.join(name)).unwrap();
        fs::write(addons.join(name).join(format!("{name}.toc")), toc).unwrap();
    }

    fn names(addons: &[InstalledAddon]) -> Vec<&str> {
        addons.iter().map(InstalledAddon::name).collect()
    }

    fn named(installed: &[InstalledAddon], name: &str) -> Vec<InstalledAddon> {
        installed
            .iter()
            .filter(|addon| addon.name() == name)
            .cloned()
            .collect()
    }

    #[test]
    fn lists_addon_folders_alphabetically_whatever_their_case() {
        let root = tempfile::tempdir().unwrap();
        put_addon(root.path(), "gamma", "");
        put_addon(root.path(), "Beta", "## Version: 2.0\n");
        put_addon(root.path(), "alpha", "");
        fs::write(root.path().join("notes.txt"), "").unwrap();

        let addons = list(root.path()).unwrap();

        assert_eq!(names(&addons), vec!["alpha", "Beta", "gamma"]);
        assert_eq!(addons[1].version(), Some("2.0"));
        assert_eq!(addons[1].path(), root.path().join("Beta"));
        assert!(!addons[1].is_link());
    }

    #[test]
    fn a_folder_the_game_does_not_load_is_still_listed() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("WeakAuras-5.19.1")).unwrap();
        fs::write(root.path().join("WeakAuras-5.19.1/WeakAuras.toc"), "").unwrap();

        let addons = list(root.path()).unwrap();

        assert_eq!(names(&addons), vec!["WeakAuras-5.19.1"]);
        assert_eq!(addons[0].toc(), None);
    }

    #[test]
    fn a_link_to_an_addon_folder_is_listed_as_a_link() {
        let root = tempfile::tempdir().unwrap();
        let addons = root.path().join("AddOns");
        put_addon(
            &root.path().join("checkout"),
            "MyAddon",
            "## Version: dev\n",
        );
        fs::create_dir_all(&addons).unwrap();
        symlink(root.path().join("checkout/MyAddon"), addons.join("MyAddon")).unwrap();

        let listed = list(&addons).unwrap();

        assert_eq!(names(&listed), vec!["MyAddon"]);
        assert!(listed[0].is_link());
        assert_eq!(listed[0].version(), Some("dev"));
    }

    #[test]
    fn links_that_lead_to_no_folder_are_not_addons() {
        let root = tempfile::tempdir().unwrap();
        let addons = root.path().join("AddOns");
        fs::create_dir_all(&addons).unwrap();
        fs::write(root.path().join("file.txt"), "").unwrap();
        symlink(root.path().join("file.txt"), addons.join("ToAFile")).unwrap();
        symlink(root.path().join("nowhere"), addons.join("Broken")).unwrap();

        assert!(list(&addons).unwrap().is_empty());
        assert_eq!(count(&addons).unwrap(), 0);
    }

    #[test]
    fn the_count_agrees_with_the_list() {
        let root = tempfile::tempdir().unwrap();
        put_addon(root.path(), "One", "");
        put_addon(root.path(), "Two", "");
        fs::create_dir_all(root.path().join("NotAnAddon")).unwrap();

        assert_eq!(count(root.path()).unwrap(), 3);
        assert_eq!(list(root.path()).unwrap().len(), 3);
    }

    #[test]
    fn a_list_names_the_title_or_why_the_game_does_not_load_a_folder() {
        let root = tempfile::tempdir().unwrap();
        let addons = root.path().join("AddOns");
        put_addon(&addons, "Details", "## Title: Details! Damage Meter\n");
        put_addon(&addons, "Untitled", "");
        fs::create_dir_all(addons.join("old-folder")).unwrap();
        put_addon(
            &root.path().join("checkout"),
            "DevAddon",
            "## Title: Dev Addon\n",
        );
        symlink(
            root.path().join("checkout/DevAddon"),
            addons.join("DevAddon"),
        )
        .unwrap();

        let notes: Vec<String> = list(&addons).unwrap().iter().map(describe).collect();

        assert_eq!(
            notes,
            vec![
                "Details! Damage Meter".to_string(),
                "Dev Addon (link)".to_string(),
                NOT_LOADED.to_string(),
                String::new(),
            ]
        );
    }

    #[test]
    fn a_folder_that_is_not_there_cannot_be_listed() {
        let root = tempfile::tempdir().unwrap();

        assert!(list(&root.path().join("nowhere")).is_err());
    }

    #[test]
    fn finds_an_addon_by_name_ignoring_case() {
        let root = tempfile::tempdir().unwrap();
        put_addon(root.path(), "WeakAuras", "");
        let installed = list(root.path()).unwrap();

        for requested in ["WeakAuras", "weakauras", "WEAKAURAS"] {
            let found = find(&installed, requested, root.path()).unwrap();
            assert_eq!(found.name(), "WeakAuras", "for {requested}");
        }
    }

    #[test]
    fn an_addon_that_is_not_there_is_reported_with_the_folder_looked_in() {
        let root = tempfile::tempdir().unwrap();
        put_addon(root.path(), "WeakAuras", "");
        let installed = list(root.path()).unwrap();

        let error = find(&installed, "Details", root.path()).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("Details is not installed in {}", root.path().display())
        );
    }

    #[test]
    fn capital_letters_decide_between_folders_that_differ_only_in_them() {
        let root = tempfile::tempdir().unwrap();
        put_addon(root.path(), "MyAddon", "");
        put_addon(root.path(), "MYADDON", "");
        let installed = list(root.path()).unwrap();

        let exact = find(&installed, "MYADDON", root.path()).unwrap();
        let error = find(&installed, "myaddon", root.path()).unwrap_err();

        assert_eq!(exact.name(), "MYADDON");
        assert!(matches!(error, Error::AmbiguousAddon { .. }));
    }

    #[test]
    fn finds_the_addons_that_need_one_being_removed() {
        let root = tempfile::tempdir().unwrap();
        put_addon(root.path(), "WeakAuras", "");
        put_addon(
            root.path(),
            "WeakAurasOptions",
            "## Dependencies: WeakAuras\n",
        );
        put_addon(
            root.path(),
            "WeakAurasArchive",
            "## RequiredDeps: weakauras\n",
        );
        put_addon(root.path(), "Masque", "## OptionalDeps: WeakAuras\n");
        let installed = list(root.path()).unwrap();

        let needing = dependents(&installed, &named(&installed, "WeakAuras"));

        assert_eq!(
            names(&needing),
            vec!["WeakAurasArchive", "WeakAurasOptions"]
        );
    }

    #[test]
    fn an_addon_that_needs_a_dependent_stops_loading_too() {
        let root = tempfile::tempdir().unwrap();
        put_addon(root.path(), "ElvUI", "");
        put_addon(root.path(), "ElvUI_Plugin", "## Dependencies: ElvUI\n");
        put_addon(
            root.path(),
            "ElvUI_PluginSkin",
            "## Dependencies: ElvUI_Plugin\n",
        );
        put_addon(root.path(), "Details", "");
        let installed = list(root.path()).unwrap();

        let needing = dependents(&installed, &named(&installed, "ElvUI"));

        assert_eq!(names(&needing), vec!["ElvUI_Plugin", "ElvUI_PluginSkin"]);
    }

    #[test]
    fn what_is_being_removed_is_not_its_own_dependent() {
        let root = tempfile::tempdir().unwrap();
        put_addon(root.path(), "WeakAuras", "");
        put_addon(
            root.path(),
            "WeakAurasOptions",
            "## Dependencies: WeakAuras\n",
        );
        let installed = list(root.path()).unwrap();

        assert!(dependents(&installed, &installed).is_empty());
    }
}
