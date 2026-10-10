//! Remembering the window's choice of game folder and version between runs.
//!
//! Only the command line takes an explicit target, so without this the
//! window would go back to its default every time it opened. The default is
//! retail, which is the wrong guess for anyone who plays something else.
//!
//! What is stored is a path and a folder name, not the install itself, since
//! a game folder can be moved or removed while the program is not running.
//! What comes back is always checked against what is really there.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::discovery::{self, Flavor, WowInstall};

/// The window's choices, as stored between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub install_root: Option<PathBuf>,
    pub flavor_dir: Option<String>,
}

/// The key the settings are stored under.
const KEY: &str = "target";

/// Reads the stored choices. Anything missing or unreadable, such as settings
/// written by an older version, falls back to no choice at all.
pub fn load(storage: Option<&dyn eframe::Storage>) -> Settings {
    storage
        .and_then(|storage| eframe::get_value(storage, KEY))
        .unwrap_or_default()
}

pub fn save(storage: &mut dyn eframe::Storage, settings: &Settings) {
    eframe::set_value(storage, KEY, settings);
}

/// Records a choice so that it can be offered again next time.
pub fn of(install: &WowInstall, flavor: &Flavor) -> Settings {
    Settings {
        install_root: Some(install.root().to_path_buf()),
        flavor_dir: Some(flavor.dir_name().to_string()),
    }
}

/// The game folder chosen last time, if it is still a game folder.
///
/// Searching usually finds it already, and that copy is used. Otherwise it is
/// opened directly, so that a folder chosen by hand stays on offer even
/// though the search does not reach it.
pub fn remembered_install(found: &[WowInstall], settings: &Settings) -> Option<WowInstall> {
    let root = settings.install_root.as_deref()?;
    match found.iter().find(|install| install.root() == root) {
        Some(known) => Some(known.clone()),
        None => discovery::open(root).ok(),
    }
}

/// The game version chosen last time, if that install still has it.
pub fn remembered_flavor(install: &WowInstall, settings: &Settings) -> Option<Flavor> {
    let dir_name = settings.flavor_dir.as_deref()?;
    install
        .flavors()
        .iter()
        .find(|flavor| flavor.dir_name() == dir_name)
        .cloned()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    use eframe::Storage as _;

    use super::*;

    /// A store that keeps the strings in memory, so that [`load`] and [`save`]
    /// can be tested over the real encoding without touching the disk.
    #[derive(Default)]
    struct MemoryStorage {
        values: HashMap<String, String>,
    }

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.values.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.values.insert(key.to_string(), value);
        }

        fn remove_string(&mut self, key: &str) {
            self.values.remove(key);
        }

        fn flush(&mut self) {}
    }

    fn fake_install(root: &Path, flavors: &[&str]) -> WowInstall {
        for flavor in flavors {
            fs::create_dir_all(root.join(flavor).join("Interface").join("AddOns")).unwrap();
        }
        discovery::read_install(root).unwrap()
    }

    #[test]
    fn a_choice_round_trips_through_the_stored_form() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_", "_classic_era_"]);
        let era = install.flavors()[1].clone();

        let settings = of(&install, &era);

        assert_eq!(remembered_install(&[], &settings).unwrap(), install);
        assert_eq!(remembered_flavor(&install, &settings), Some(era));
    }

    #[test]
    fn a_remembered_folder_already_found_is_used_as_found() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_"]);
        let settings = of(&install, &install.flavors()[0]);

        let found = vec![install.clone()];

        assert_eq!(remembered_install(&found, &settings), Some(install));
    }

    #[test]
    fn a_remembered_folder_that_is_gone_is_forgotten() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_"]);
        let settings = of(&install, &install.flavors()[0]);
        drop(root);

        assert_eq!(remembered_install(&[], &settings), None);
    }

    #[test]
    fn a_remembered_game_version_that_is_gone_is_forgotten() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_"]);
        let settings = Settings {
            install_root: Some(install.root().to_path_buf()),
            flavor_dir: Some("_classic_era_".to_string()),
        };

        assert_eq!(remembered_flavor(&install, &settings), None);
    }

    #[test]
    fn a_choice_survives_being_stored_and_read_back() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_", "_classic_era_"]);
        let era = install.flavors()[1].clone();
        let mut storage = MemoryStorage::default();

        save(&mut storage, &of(&install, &era));
        let restored = load(Some(&storage));

        assert_eq!(remembered_install(&[], &restored).unwrap(), install);
        assert_eq!(remembered_flavor(&install, &restored), Some(era));
    }

    #[test]
    fn an_empty_store_means_no_choice() {
        assert_eq!(load(Some(&MemoryStorage::default())), Settings::default());
        assert_eq!(load(None), Settings::default());
    }

    #[test]
    fn a_stored_value_that_cannot_be_read_means_no_choice() {
        let mut storage = MemoryStorage::default();
        storage.set_string(KEY, "not ron at all".to_string());

        assert_eq!(load(Some(&storage)), Settings::default());
    }

    #[test]
    fn nothing_remembered_means_no_choice() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_"]);

        assert_eq!(remembered_install(&[], &Settings::default()), None);
        assert_eq!(remembered_flavor(&install, &Settings::default()), None);
    }
}
