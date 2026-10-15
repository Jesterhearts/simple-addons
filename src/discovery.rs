//! Finding World of Warcraft directories and the game versions inside them.
//!
//! A World of Warcraft directory holds one folder per game version, named
//! like `_retail_` or `_classic_era_`. Addons for a version live in
//! `<install>/<version>/Interface/AddOns`.
//!
//! On Linux the game usually sits inside a Wine prefix, so the search covers
//! the prefix layouts that Lutris, Steam, Bottles and Heroic create as well
//! as plain install folders.

use std::env;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fs_util;

/// A game version folder, named the way the game names it: `_retail_`.
///
/// Addon authors call these flavors, which is also what the command line
/// option is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flavor {
    dir_name: String,
}

impl Flavor {
    /// Accepts only names shaped like `_retail_`, so that ordinary folders in
    /// the install directory are not mistaken for game versions.
    pub fn from_dir_name(name: &str) -> Option<Self> {
        let inner = name.strip_prefix('_')?.strip_suffix('_')?;
        if inner.is_empty() {
            return None;
        }
        Some(Self {
            dir_name: name.to_string(),
        })
    }

    pub fn dir_name(&self) -> &str {
        &self.dir_name
    }

    /// The name to show a user: "Classic Era" for `_classic_era_`.
    pub fn label(&self) -> String {
        self.dir_name
            .trim_matches('_')
            .split('_')
            .map(spell_out)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn is_retail(&self) -> bool {
        self.dir_name == "_retail_"
    }

    /// True when a name the user typed refers to this game version. Spelling
    /// is compared loosely, so `classic_era`, `Classic Era` and
    /// `_classic_era_` all match.
    pub fn matches(&self, requested: &str) -> bool {
        loosely(requested) == loosely(&self.dir_name)
    }
}

/// Words that read better as initials than as capitalised words.
const INITIALISMS: &[&str] = &["ptr", "xptr", "tbc", "bcc", "wotlk", "mop"];

fn spell_out(word: &str) -> String {
    if INITIALISMS.contains(&word) {
        return word.to_uppercase();
    }
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// Reduces a name to its letters and digits in lower case, so that the
/// punctuation people use for game versions stops mattering.
fn loosely(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A World of Warcraft directory and the game versions found in it.
///
/// Only built when at least one game version is present, so every value of
/// this type has somewhere to install to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WowInstall {
    root: PathBuf,
    flavors: Vec<Flavor>,
}

impl WowInstall {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The game versions present, retail first and the rest by name.
    pub fn flavors(&self) -> &[Flavor] {
        &self.flavors
    }

    pub fn addons_dir(&self, flavor: &Flavor) -> PathBuf {
        self.root
            .join(flavor.dir_name())
            .join("Interface")
            .join("AddOns")
    }
}

/// Decides whether one directory is a World of Warcraft directory.
///
/// A game version folder may be a link, as when one version is kept on
/// another disk, so links to folders count.
pub fn read_install(root: &Path) -> Option<WowInstall> {
    let mut flavors: Vec<Flavor> = fs_util::folders_in(fs_util::list_dir(root).ok()?)
        .iter()
        .filter_map(|dir| flavor_at(dir))
        .collect();
    if flavors.is_empty() {
        return None;
    }
    flavors.sort_by(|a, b| (!a.is_retail(), a.dir_name()).cmp(&(!b.is_retail(), b.dir_name())));
    Some(WowInstall {
        root: root.to_path_buf(),
        flavors,
    })
}

/// A game version folder is named like `_retail_` and holds the game itself,
/// which rules out folders a user happened to name the same way.
fn flavor_at(dir: &Path) -> Option<Flavor> {
    let flavor = Flavor::from_dir_name(dir.file_name()?.to_str()?)?;
    if dir.join("Interface").is_dir() || holds_game_executable(dir) {
        Some(flavor)
    } else {
        None
    }
}

fn holds_game_executable(dir: &Path) -> bool {
    let Ok(entries) = fs_util::list_dir(dir) else {
        return false;
    };
    fs_util::files_in(entries).iter().any(|file| {
        let name = fs_util::file_name_lossy(file).to_lowercase();
        name.starts_with("wow") && name.ends_with(".exe")
    })
}

/// Looks through the usual install locations and returns what is really there.
pub fn discover() -> Vec<WowInstall> {
    let mut installs: Vec<WowInstall> = Vec::new();
    for root in candidate_roots() {
        let Some(install) = read_install(&root) else {
            continue;
        };
        if !installs
            .iter()
            .any(|known| same_directory(known.root(), install.root()))
        {
            installs.push(install);
        }
    }
    installs
}

/// Opens a directory the user pointed at.
///
/// The install root, a game version folder, an `Interface` folder and an
/// `AddOns` folder are all accepted, because any of them is a reasonable
/// thing to have at hand.
pub fn open(path: &Path) -> Result<WowInstall> {
    walk_up(path, 3)
        .iter()
        .find_map(|dir| install_holding(dir).or_else(|| read_install(dir)))
        .ok_or_else(|| Error::NotAnInstall(path.to_path_buf()))
}

/// The install a game version folder belongs to, when `dir` is one.
///
/// A game version folder can itself hold a folder shaped like one, such as
/// a backup made by hand, which would make it look like an install of its
/// own. The install it belongs to is the one meant.
fn install_holding(dir: &Path) -> Option<WowInstall> {
    let install = read_install(dir.parent()?)?;
    let name = dir.file_name()?.to_str()?;
    let holds_dir = install
        .flavors()
        .iter()
        .any(|flavor| flavor.dir_name() == name);
    holds_dir.then_some(install)
}

/// The game version a path lies in, such as Classic Era for
/// `<install>/_classic_era_/Interface/AddOns`. `None` for the install root
/// itself, and for a path outside the install, since neither names one.
///
/// [`open`] accepts paths like these, and the path says which game version
/// was meant as plainly as the install it is in does. A path inside the
/// install but in no game version found there names something this program
/// cannot see, so that is an error rather than no answer: treating it as no
/// answer would quietly pick another game version.
pub fn flavor_containing<'a>(install: &'a WowInstall, path: &Path) -> Result<Option<&'a Flavor>> {
    let Ok(relative) = path.strip_prefix(install.root()) else {
        return Ok(None);
    };
    let Some(first) = relative.components().next() else {
        return Ok(None);
    };
    let first = first.as_os_str().to_string_lossy();
    let exact = install
        .flavors()
        .iter()
        .find(|flavor| flavor.dir_name() == first);
    // A filesystem that ignores case lets a path spell the folder its own way.
    let ignoring_case = || {
        install
            .flavors()
            .iter()
            .find(|flavor| flavor.dir_name().eq_ignore_ascii_case(&first))
    };
    match exact.or_else(ignoring_case) {
        Some(flavor) => Ok(Some(flavor)),
        None => Err(Error::NotInGameVersion {
            path: path.to_path_buf(),
            install: install.root().to_path_buf(),
            available: flavor_list(install),
        }),
    }
}

/// Picks the game version to install into.
pub fn select_flavor<'a>(install: &'a WowInstall, requested: Option<&str>) -> Result<&'a Flavor> {
    match requested {
        Some(name) => install
            .flavors()
            .iter()
            .find(|flavor| flavor.matches(name))
            .ok_or_else(|| Error::UnknownGameVersion {
                install: install.root().to_path_buf(),
                requested: name.to_string(),
                available: flavor_list(install),
            }),
        None => default_flavor(install),
    }
}

/// Retail is what most people mean. Failing that, a single game version is
/// unambiguous, and anything else needs the user to say which one.
fn default_flavor(install: &WowInstall) -> Result<&Flavor> {
    if let Some(retail) = install.flavors().iter().find(|flavor| flavor.is_retail()) {
        return Ok(retail);
    }
    match install.flavors() {
        [only] => Ok(only),
        _ => Err(Error::AmbiguousGameVersion {
            install: install.root().to_path_buf(),
            available: flavor_list(install),
        }),
    }
}

/// The game versions of an install, for a message to the user.
pub fn flavor_list(install: &WowInstall) -> String {
    install
        .flavors()
        .iter()
        .map(Flavor::label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Install folders, relative to a Wine prefix.
const WINE_SUBPATHS: &[&str] = &[
    "drive_c/Program Files (x86)/World of Warcraft",
    "drive_c/Program Files/World of Warcraft",
    "drive_c/World of Warcraft",
];

/// Home-relative directories that hold one Wine prefix per child folder.
const PREFIX_PARENTS: &[&str] = &[
    "Games",
    "Games/Heroic/Prefixes",
    ".local/share/wineprefixes",
    ".local/share/bottles/bottles",
    ".var/app/net.lutris.Lutris/data/lutris/prefixes",
];

/// Home-relative Wine prefixes at a fixed location.
const FIXED_PREFIXES: &[&str] = &[".wine", ".wine64"];

/// Home-relative Steam compatibility folders, which hold one prefix per game.
const STEAM_COMPATDATA: &[&str] = &[
    ".steam/steam/steamapps/compatdata",
    ".local/share/Steam/steamapps/compatdata",
];

/// Home-relative install folders outside any Wine prefix.
const PLAIN_SUBPATHS: &[&str] = &["World of Warcraft", "Games/World of Warcraft"];

/// Install folders at a fixed place on the system.
const ABSOLUTE_SUBPATHS: &[&str] = &[
    "/Applications/World of Warcraft",
    "/opt/World of Warcraft",
    "C:/Program Files (x86)/World of Warcraft",
];

/// Every directory worth testing, in the order they are tried.
pub fn candidate_roots() -> Vec<PathBuf> {
    let home = env::home_dir();
    let mut roots = Vec::new();
    for prefix in wine_prefixes(home.as_deref()) {
        roots.extend(WINE_SUBPATHS.iter().map(|subpath| prefix.join(subpath)));
    }
    if let Some(home) = home.as_deref() {
        roots.extend(PLAIN_SUBPATHS.iter().map(|subpath| home.join(subpath)));
    }
    roots.extend(ABSOLUTE_SUBPATHS.iter().map(PathBuf::from));
    roots
}

fn wine_prefixes(home: Option<&Path>) -> Vec<PathBuf> {
    let Some(home) = home else {
        return Vec::new();
    };
    let mut prefixes: Vec<PathBuf> = FIXED_PREFIXES
        .iter()
        .map(|prefix| home.join(prefix))
        .collect();
    for parent in PREFIX_PARENTS {
        prefixes.extend(child_dirs(&home.join(parent)));
    }
    for compatdata in STEAM_COMPATDATA {
        prefixes.extend(
            child_dirs(&home.join(compatdata))
                .into_iter()
                .map(|game| game.join("pfx")),
        );
    }
    prefixes
}

fn child_dirs(parent: &Path) -> Vec<PathBuf> {
    fs_util::dirs_in(fs_util::list_dir(parent).unwrap_or_default())
}

/// A path and its parents, nearest first, for finding the install root from
/// somewhere inside it.
fn walk_up(path: &Path, levels: usize) -> Vec<PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    let mut current = path;
    for _ in 0..levels {
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                paths.push(parent.to_path_buf());
                current = parent;
            }
            _ => break,
        }
    }
    paths
}

/// Different candidates can reach the same directory, through a symlink or a
/// bind mount, so they are compared by where they really lead.
fn same_directory(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Builds an install directory holding the given game versions, each with
    /// the `Interface` folder that marks it as real.
    fn fake_install(root: &Path, flavors: &[&str]) {
        for flavor in flavors {
            fs::create_dir_all(root.join(flavor).join("Interface").join("AddOns")).unwrap();
        }
    }

    #[test]
    fn reads_game_versions_with_retail_first() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_classic_era_", "_retail_", "_classic_"]);

        let install = read_install(root.path()).unwrap();

        let names: Vec<&str> = install.flavors().iter().map(Flavor::dir_name).collect();
        assert_eq!(names, vec!["_retail_", "_classic_", "_classic_era_"]);
    }

    #[test]
    fn ignores_folders_that_only_look_like_game_versions() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_"]);
        fs::create_dir_all(root.path().join("_backup_")).unwrap();
        fs::create_dir_all(root.path().join("Data")).unwrap();

        let install = read_install(root.path()).unwrap();

        assert_eq!(install.flavors().len(), 1);
    }

    #[test]
    fn a_directory_without_game_versions_is_not_an_install() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("Data")).unwrap();

        assert!(read_install(root.path()).is_none());
    }

    #[test]
    fn accepts_an_executable_instead_of_an_interface_folder() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("_retail_")).unwrap();
        fs::write(root.path().join("_retail_").join("Wow.exe"), "").unwrap();

        assert!(read_install(root.path()).is_some());
    }

    #[test]
    fn opens_an_install_from_inside_it() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_"]);
        let addons = root
            .path()
            .join("_retail_")
            .join("Interface")
            .join("AddOns");

        for start in [root.path(), &root.path().join("_retail_"), &addons] {
            assert_eq!(open(start).unwrap().root(), root.path(), "from {start:?}");
        }
    }

    #[test]
    fn a_game_version_holding_a_backup_shaped_like_one_still_opens_its_install() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_", "_classic_era_"]);
        let era = root.path().join("_classic_era_");
        fake_install(&era, &["_backup_"]);

        for start in [
            era.clone(),
            era.join("Interface"),
            era.join("Interface/AddOns"),
        ] {
            let install = open(&start).unwrap();
            assert_eq!(install.root(), root.path(), "from {start:?}");
            let flavor = flavor_containing(&install, &start).unwrap().unwrap();
            assert_eq!(flavor.dir_name(), "_classic_era_");
        }
    }

    #[test]
    fn opening_somewhere_else_reports_the_path_given() {
        let root = tempfile::tempdir().unwrap();

        let error = open(root.path()).unwrap_err();

        assert!(matches!(error, Error::NotAnInstall(path) if path == root.path()));
    }

    #[test]
    fn a_path_inside_a_game_version_names_that_game_version() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_", "_classic_era_"]);
        let era = root.path().join("_classic_era_");

        for start in [
            era.clone(),
            era.join("Interface"),
            era.join("Interface/AddOns"),
        ] {
            let install = open(&start).unwrap();
            let flavor = flavor_containing(&install, &start).unwrap().unwrap();
            assert_eq!(flavor.dir_name(), "_classic_era_", "from {start:?}");
        }
        let install = open(root.path()).unwrap();
        assert_eq!(flavor_containing(&install, root.path()).unwrap(), None);
    }

    #[test]
    fn a_path_in_no_game_version_found_there_is_refused_rather_than_guessed() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_"]);
        let unknown = root.path().join("_classic_era_/Interface/AddOns");
        fs::create_dir_all(&unknown).unwrap();
        fs::remove_dir_all(root.path().join("_classic_era_/Interface")).unwrap();
        fs::create_dir_all(root.path().join("Data")).unwrap();
        let install = open(root.path()).unwrap();

        for path in [root.path().join("_classic_era_"), root.path().join("Data")] {
            let error = flavor_containing(&install, &path).unwrap_err();
            assert!(
                matches!(error, Error::NotInGameVersion { .. }),
                "for {path:?}"
            );
        }
    }

    #[test]
    fn a_game_version_folder_may_be_a_link() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_"]);
        fake_install(elsewhere.path(), &["_classic_era_"]);
        std::os::unix::fs::symlink(
            elsewhere.path().join("_classic_era_"),
            root.path().join("_classic_era_"),
        )
        .unwrap();
        let addons = root.path().join("_classic_era_/Interface/AddOns");

        let install = open(&addons).unwrap();

        assert_eq!(install.root(), root.path());
        let flavor = flavor_containing(&install, &addons).unwrap().unwrap();
        assert_eq!(flavor.dir_name(), "_classic_era_");
    }

    #[test]
    fn a_game_version_folder_spelt_in_other_capitals_is_still_found() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_", "_classic_era_"]);
        let install = open(root.path()).unwrap();

        let flavor = flavor_containing(&install, &root.path().join("_Classic_Era_/Interface"))
            .unwrap()
            .unwrap();

        assert_eq!(flavor.dir_name(), "_classic_era_");
    }

    #[test]
    fn addons_live_under_the_chosen_game_version() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_"]);
        let install = read_install(root.path()).unwrap();

        let addons = install.addons_dir(&install.flavors()[0]);

        assert_eq!(
            addons,
            root.path()
                .join("_retail_")
                .join("Interface")
                .join("AddOns")
        );
    }

    #[test]
    fn retail_is_the_default_game_version() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_classic_era_", "_retail_"]);
        let install = read_install(root.path()).unwrap();

        assert!(select_flavor(&install, None).unwrap().is_retail());
    }

    #[test]
    fn a_single_game_version_is_the_default() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_classic_era_"]);
        let install = read_install(root.path()).unwrap();

        assert_eq!(
            select_flavor(&install, None).unwrap().dir_name(),
            "_classic_era_"
        );
    }

    #[test]
    fn several_non_retail_versions_need_a_choice() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_classic_era_", "_classic_"]);
        let install = read_install(root.path()).unwrap();

        let error = select_flavor(&install, None).unwrap_err();

        assert!(matches!(error, Error::AmbiguousGameVersion { .. }));
    }

    #[test]
    fn a_requested_game_version_is_matched_loosely() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_classic_era_", "_retail_"]);
        let install = read_install(root.path()).unwrap();

        for requested in ["classic_era", "Classic Era", "_classic_era_", "CLASSICERA"] {
            assert_eq!(
                select_flavor(&install, Some(requested)).unwrap().dir_name(),
                "_classic_era_",
                "for {requested}"
            );
        }
    }

    #[test]
    fn an_unknown_game_version_lists_what_is_there() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_"]);
        let install = read_install(root.path()).unwrap();

        let error = select_flavor(&install, Some("classic")).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "{} has no game version called \"classic\"; it has: Retail",
                root.path().display()
            )
        );
    }

    #[test]
    fn game_version_labels_read_as_words() {
        let labels: Vec<String> = ["_retail_", "_classic_era_", "_ptr_", "_classic_beta_"]
            .iter()
            .map(|name| Flavor::from_dir_name(name).unwrap().label())
            .collect();

        assert_eq!(labels, vec!["Retail", "Classic Era", "PTR", "Classic Beta"]);
    }

    #[test]
    fn only_underscore_wrapped_names_are_game_versions() {
        assert!(Flavor::from_dir_name("_retail_").is_some());
        assert!(Flavor::from_dir_name("retail").is_none());
        assert!(Flavor::from_dir_name("_retail").is_none());
        assert!(Flavor::from_dir_name("_").is_none());
        assert!(Flavor::from_dir_name("__").is_none());
    }
}
