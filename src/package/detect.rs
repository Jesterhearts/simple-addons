//! Finding the addon folders inside a staged directory tree.
//!
//! The game loads an addon from `Interface/AddOns/<Name>`, where the folder
//! holds `<Name>.toc`. What a user hands the installer is rarely in that
//! shape already: a download may wrap the addon in a release folder, hold
//! several addons side by side, or carry the toc file at the top with no
//! folder at all. This module reduces all of those to a list of folders and
//! the names they should install as.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fs_util;
use crate::package::toc::{self, Toc};

/// One addon folder ready to install. `path` holds the files and `name` is
/// the folder name the game expects under `Interface/AddOns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonSource {
    pub name: String,
    pub path: PathBuf,
    pub toc: Toc,
}

/// How far below the source root an addon folder may sit.
///
/// One level covers a release folder around the addon, and the rest covers
/// bundles that group their addons into subfolders.
const MAX_DEPTH: usize = 4;

/// Folders that never hold an addon, and that would otherwise be searched or
/// even installed. `__MACOSX` in particular shadows the real folder names of
/// an archive made on macOS.
const IGNORED_DIRS: &[&str] = &["__MACOSX", ".git", ".github", ".svn", ".hg", "node_modules"];

/// Suffixes addons add to a toc file name to support several game versions
/// from one download, as in `MyAddon_Vanilla.toc`.
const GAME_VERSION_SUFFIXES: &[&str] = &[
    "mainline",
    "standard",
    "vanilla",
    "classic",
    "era",
    "tbc",
    "bcc",
    "wrath",
    "wotlk",
    "wotlkc",
    "cata",
    "mop",
    "wod",
    "legion",
    "bfa",
    "shadowlands",
    "dragonflight",
    "thewarwithin",
    "midnight",
    "ptr",
    "beta",
];

/// Lists the addons in a staged directory tree.
pub fn find_addons(root: &Path) -> Result<Vec<AddonSource>> {
    let mut addons = Vec::new();
    collect_addons(root, 0, &mut addons)?;
    if addons.is_empty() {
        return Err(Error::NoAddonFound(root.to_path_buf()));
    }
    addons.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(addons)
}

/// Walks down from `dir` collecting addon folders.
///
/// A folder that is itself an addon is not searched any further. Addons embed
/// shared libraries as addon folders under `Libs/`, and those belong to their
/// parent rather than being installed on their own.
fn collect_addons(dir: &Path, depth: usize, addons: &mut Vec<AddonSource>) -> Result<()> {
    if let Some(addon) = read_addon(dir) {
        addons.push(addon);
        return Ok(());
    }
    if depth == MAX_DEPTH {
        return Ok(());
    }
    for child in child_dirs(dir)? {
        collect_addons(&child, depth + 1, addons)?;
    }
    Ok(())
}

/// Reads one folder as an addon, if it holds a toc file.
fn read_addon(dir: &Path) -> Option<AddonSource> {
    let tocs = toc_files(dir).ok()?;
    let stems: Vec<String> = tocs.iter().filter_map(|toc| file_stem(toc)).collect();
    let name = addon_name(&fs_util::file_name_lossy(dir), &stems)?;
    let primary = primary_toc(&tocs, &name)?;
    Some(AddonSource {
        name,
        path: dir.to_path_buf(),
        toc: toc::read(&primary),
    })
}

/// The folder name an addon has to install as.
///
/// Toc file names decide it, because the game only loads `<Name>/<Name>.toc`.
/// The folder keeps its own name when it already agrees, which leaves a
/// correctly packaged addon alone; otherwise the toc name wins, which strips
/// the version numbers that downloads add to their folders.
fn addon_name(folder: &str, toc_stems: &[String]) -> Option<String> {
    let names: Vec<&str> = toc_stems
        .iter()
        .map(|stem| strip_game_version_suffix(stem))
        .collect();
    if names.iter().any(|name| name.eq_ignore_ascii_case(folder)) {
        return Some(folder.to_string());
    }
    names.first().map(|name| (*name).to_string())
}

/// Removes the game version suffix from a toc file name, leaving the addon
/// name. Only known suffixes are removed, so an addon whose name happens to
/// hold a dash or an underscore keeps it.
fn strip_game_version_suffix(stem: &str) -> &str {
    for separator in ['_', '-'] {
        if let Some((name, suffix)) = stem.rsplit_once(separator)
            && !name.is_empty()
            && is_game_version_suffix(suffix)
        {
            return name;
        }
    }
    stem
}

fn is_game_version_suffix(suffix: &str) -> bool {
    GAME_VERSION_SUFFIXES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(suffix))
}

/// The toc file to read metadata from: the one named after the addon when it
/// is there, since that is the one the game loads for the current version.
fn primary_toc(tocs: &[PathBuf], name: &str) -> Option<PathBuf> {
    tocs.iter()
        .find(|toc| file_stem(toc).is_some_and(|stem| stem.eq_ignore_ascii_case(name)))
        .or_else(|| tocs.first())
        .cloned()
}

fn toc_files(dir: &Path) -> Result<Vec<PathBuf>> {
    Ok(fs_util::files_in(fs_util::list_dir(dir)?)
        .into_iter()
        .filter(|file| fs_util::has_extension(file, "toc"))
        .collect())
}

fn child_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    Ok(fs_util::dirs_in(fs_util::list_dir(dir)?)
        .into_iter()
        .filter(|child| !is_ignored(child))
        .collect())
}

fn is_ignored(dir: &Path) -> bool {
    let name = fs_util::file_name_lossy(dir);
    IGNORED_DIRS.iter().any(|ignored| *ignored == name)
}

fn file_stem(path: &Path) -> Option<String> {
    Some(path.file_stem()?.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn names(addons: &[AddonSource]) -> Vec<&str> {
        addons.iter().map(|addon| addon.name.as_str()).collect()
    }

    #[test]
    fn finds_an_addon_folder_handed_over_directly() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join("MyAddon/MyAddon.toc"),
            "## Title: My Addon\n## Version: 1.0\n",
        );

        let addons = find_addons(&root.path().join("MyAddon")).unwrap();

        assert_eq!(names(&addons), vec!["MyAddon"]);
        assert_eq!(addons[0].path, root.path().join("MyAddon"));
        assert_eq!(addons[0].toc.version, Some("1.0".to_string()));
    }

    #[test]
    fn finds_an_addon_inside_a_release_folder() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("MyAddon-v1.2.3/MyAddon/MyAddon.toc"), "");

        let addons = find_addons(root.path()).unwrap();

        assert_eq!(names(&addons), vec!["MyAddon"]);
        assert_eq!(
            addons[0].path,
            root.path().join("MyAddon-v1.2.3").join("MyAddon")
        );
    }

    #[test]
    fn names_an_addon_after_its_toc_when_the_folder_disagrees() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("WeakAuras-5.19.1/WeakAuras.toc"), "");

        let addons = find_addons(root.path()).unwrap();

        assert_eq!(names(&addons), vec!["WeakAuras"]);
        assert_eq!(addons[0].path, root.path().join("WeakAuras-5.19.1"));
    }

    #[test]
    fn names_a_top_level_toc_after_the_addon_not_the_folder() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("MyAddon.toc"), "");

        let addons = find_addons(root.path()).unwrap();

        assert_eq!(names(&addons), vec!["MyAddon"]);
        assert_eq!(addons[0].path, root.path());
    }

    #[test]
    fn finds_every_addon_of_a_bundle() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("WeakAuras/WeakAuras.toc"), "");
        write(
            &root.path().join("WeakAurasOptions/WeakAurasOptions.toc"),
            "",
        );
        write(
            &root.path().join("WeakAurasArchive/WeakAurasArchive.toc"),
            "",
        );

        let addons = find_addons(root.path()).unwrap();

        assert_eq!(
            names(&addons),
            vec!["WeakAuras", "WeakAurasArchive", "WeakAurasOptions"]
        );
    }

    #[test]
    fn leaves_embedded_libraries_inside_their_addon() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("MyAddon/MyAddon.toc"), "");
        write(&root.path().join("MyAddon/Libs/LibStub/LibStub.toc"), "");
        write(&root.path().join("MyAddon/Libs/AceGUI/AceGUI.toc"), "");

        let addons = find_addons(root.path()).unwrap();

        assert_eq!(names(&addons), vec!["MyAddon"]);
    }

    #[test]
    fn keeps_the_folder_name_when_tocs_carry_game_version_suffixes() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("MyAddon/MyAddon_Mainline.toc"), "");
        write(&root.path().join("MyAddon/MyAddon_Vanilla.toc"), "");

        let addons = find_addons(root.path()).unwrap();

        assert_eq!(names(&addons), vec!["MyAddon"]);
    }

    #[test]
    fn keeps_a_dash_that_belongs_to_the_addon_name() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("Dominos-Cast/Dominos-Cast.toc"), "");

        let addons = find_addons(root.path()).unwrap();

        assert_eq!(names(&addons), vec!["Dominos-Cast"]);
    }

    #[test]
    fn skips_the_folders_a_macos_archive_adds() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("MyAddon/MyAddon.toc"), "");
        write(&root.path().join("__MACOSX/MyAddon/MyAddon.toc"), "");

        let addons = find_addons(root.path()).unwrap();

        assert_eq!(names(&addons), vec!["MyAddon"]);
    }

    #[test]
    fn reads_metadata_from_the_toc_named_after_the_addon() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join("MyAddon/MyAddon.toc"),
            "## Version: current\n",
        );
        write(
            &root.path().join("MyAddon/MyAddon_Vanilla.toc"),
            "## Version: old\n",
        );

        let addons = find_addons(root.path()).unwrap();

        assert_eq!(addons[0].toc.version, Some("current".to_string()));
    }

    #[test]
    fn a_tree_without_a_toc_file_holds_no_addon() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("Screenshots/shot.tga"), "");

        let error = find_addons(root.path()).unwrap_err();

        assert!(matches!(error, Error::NoAddonFound(path) if path == root.path()));
    }

    #[test]
    fn stops_searching_below_the_depth_limit() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("a/b/c/d/e/Deep/Deep.toc"), "");

        assert!(find_addons(root.path()).is_err());
    }
}
