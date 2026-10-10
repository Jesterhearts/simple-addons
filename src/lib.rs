//! Install World of Warcraft addons from a folder or a zip file.
//!
//! The work is one pipeline:
//!
//! ```text
//! folder or zip file
//!   -> directory tree          (package::stage)
//!   -> addon folders           (package::detect)
//!   -> plan                    (install::plan)
//!   -> Interface/AddOns        (install::execute)
//! ```
//!
//! Where the addons go is worked out separately, by [`discovery`], and the
//! two meet in [`install_source`].

pub mod cli;
pub mod discovery;
pub mod error;
pub mod fs_util;
pub mod gui;
pub mod install;
pub mod package;

use std::path::Path;

pub use error::{Error, Result};

use install::{Outcome, Plan};
use package::{Source, Staged};

/// Reads a source and works out what installing it would do.
///
/// The [`Staged`] value comes back with the plan because the plan points into
/// it. Dropping it would delete the files an extracted archive was unpacked
/// to, so it has to live until the plan is carried out.
pub fn plan_source(source: &Path, addons_dir: &Path) -> Result<(Staged, Plan)> {
    let source = Source::open(source)?;
    let (staged, addons) = package::read_addons(&source)?;
    Ok((staged, install::plan(addons_dir, &addons)))
}

/// Installs every addon found in one folder or zip file.
pub fn install_source(source: &Path, addons_dir: &Path) -> Result<Vec<Outcome>> {
    let (staged, plan) = plan_source(source, addons_dir)?;
    let outcomes = install::execute(&plan)?;
    drop(staged);
    Ok(outcomes)
}

/// How many addons are installed in a folder.
///
/// Used to tell the user what is already there, so a count is not worth an
/// error when the folder cannot be read.
pub fn count_installed(addons_dir: &Path) -> Option<usize> {
    let entries = fs_util::list_dir(addons_dir).ok()?;
    Some(fs_util::dirs_in(entries).len())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    use super::*;

    fn addons_dir(root: &Path) -> PathBuf {
        let dir = root.join("_retail_").join("Interface").join("AddOns");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_zip(path: &Path, entries: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, contents) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn installs_a_zip_file_end_to_end() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let archive = root.path().join("MyAddon-1.2.3.zip");
        write_zip(
            &archive,
            &[
                ("MyAddon-1.2.3/MyAddon/MyAddon.toc", "## Version: 1.2.3\n"),
                ("MyAddon-1.2.3/MyAddon/Core.lua", "-- core\n"),
                ("MyAddon-1.2.3/README.md", "# read me\n"),
            ],
        );

        let outcomes = install_source(&archive, &addons).unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].addon, "MyAddon");
        assert_eq!(outcomes[0].toc.version, Some("1.2.3".to_string()));
        assert_eq!(
            fs::read_to_string(addons.join("MyAddon/Core.lua")).unwrap(),
            "-- core\n"
        );
        assert!(
            !addons.join("MyAddon/README.md").exists(),
            "only the addon folder belongs in AddOns"
        );
    }

    #[test]
    fn installs_a_folder_end_to_end() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let source = root.path().join("MyAddon");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("MyAddon.toc"), "## Title: My Addon\n").unwrap();

        let outcomes = install_source(&source, &addons).unwrap();

        assert_eq!(outcomes[0].addon, "MyAddon");
        assert!(addons.join("MyAddon/MyAddon.toc").is_file());
    }

    #[test]
    fn a_plan_says_what_would_happen_without_writing_anything() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        let source = root.path().join("MyAddon");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("MyAddon.toc"), "").unwrap();

        let (_staged, plan) = plan_source(&source, &addons).unwrap();

        assert_eq!(plan.addons[0].destination, addons.join("MyAddon"));
        assert!(!addons.join("MyAddon").exists());
    }

    #[test]
    fn counts_the_addons_already_installed() {
        let root = tempfile::tempdir().unwrap();
        let addons = addons_dir(root.path());
        fs::create_dir_all(addons.join("One")).unwrap();
        fs::create_dir_all(addons.join("Two")).unwrap();
        fs::write(addons.join("notes.txt"), "").unwrap();

        assert_eq!(count_installed(&addons), Some(2));
        assert_eq!(count_installed(&root.path().join("nowhere")), None);
    }
}
