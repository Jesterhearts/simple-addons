//! Turning what the user handed over into addon folders ready to install.
//!
//! ```text
//! folder or zip file -> directory tree -> addon folders
//! ```

pub mod archive;
pub mod detect;
pub mod toc;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::error::{Error, Result};

pub use detect::AddonSource;
pub use toc::Toc;

/// Something to install, after checking that it exists and can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Folder(PathBuf),
    Archive(PathBuf),
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => Error::MissingSource(path.to_path_buf()),
            _ => Error::io("read", path)(error),
        })?;
        if metadata.is_dir() {
            return Ok(Self::Folder(path.to_path_buf()));
        }
        if archive::looks_like_zip(path) {
            return Ok(Self::Archive(path.to_path_buf()));
        }
        Err(Error::UnsupportedSource(path.to_path_buf()))
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Folder(path) | Self::Archive(path) => path,
        }
    }
}

/// A source laid out as a directory tree.
///
/// An extracted archive keeps its temporary folder alive until this value is
/// dropped, so it has to outlive anything pointing into the tree.
#[derive(Debug)]
pub enum Staged {
    Folder(PathBuf),
    Extracted(TempDir),
}

impl Staged {
    pub fn root(&self) -> &Path {
        match self {
            Self::Folder(path) => path,
            Self::Extracted(dir) => dir.path(),
        }
    }
}

/// Lays a source out as a directory tree. A folder is read where it is; an
/// archive is extracted to a temporary folder.
pub fn stage(source: &Source) -> Result<Staged> {
    match source {
        Source::Folder(path) => Ok(Staged::Folder(path.clone())),
        Source::Archive(path) => {
            let dir = tempfile::tempdir().map_err(Error::io("create", std::env::temp_dir()))?;
            archive::extract(path, dir.path())?;
            Ok(Staged::Extracted(dir))
        }
    }
}

/// Reads a source and lists the addons in it.
///
/// The [`Staged`] value comes back with the list because the addon paths
/// point into it, and dropping it would delete an extracted archive.
pub fn read_addons(source: &Source) -> Result<(Staged, Vec<AddonSource>)> {
    let staged = stage(source)?;
    let addons = detect::find_addons(staged.root())?;
    Ok((staged, addons))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

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
    fn opens_a_folder_as_a_folder() {
        let root = tempfile::tempdir().unwrap();

        assert_eq!(
            Source::open(root.path()).unwrap(),
            Source::Folder(root.path().to_path_buf())
        );
    }

    #[test]
    fn opens_a_zip_file_as_an_archive() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("addon.zip");
        write_zip(&archive, &[("MyAddon/MyAddon.toc", "")]);

        assert_eq!(
            Source::open(&archive).unwrap(),
            Source::Archive(archive.clone())
        );
    }

    #[test]
    fn refuses_a_file_that_is_not_an_archive() {
        let root = tempfile::tempdir().unwrap();
        let text = root.path().join("notes.txt");
        fs::write(&text, "just text").unwrap();

        assert!(matches!(
            Source::open(&text).unwrap_err(),
            Error::UnsupportedSource(_)
        ));
    }

    #[test]
    fn reports_a_source_that_is_not_there() {
        let root = tempfile::tempdir().unwrap();

        assert!(matches!(
            Source::open(&root.path().join("missing.zip")).unwrap_err(),
            Error::MissingSource(_)
        ));
    }

    #[test]
    fn reads_the_addons_of_an_archive() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("bundle.zip");
        write_zip(
            &archive,
            &[
                ("Bundle-1.0/Details/Details.toc", "## Version: 1.0\n"),
                ("Bundle-1.0/Details_Streamer/Details_Streamer.toc", ""),
            ],
        );

        let (staged, addons) = read_addons(&Source::open(&archive).unwrap()).unwrap();

        let names: Vec<&str> = addons.iter().map(|addon| addon.name.as_str()).collect();
        assert_eq!(names, vec!["Details", "Details_Streamer"]);
        assert!(addons[0].path.starts_with(staged.root()));
    }

    #[test]
    fn reads_the_addons_of_a_folder_where_it_is() {
        let root = tempfile::tempdir().unwrap();
        let addon_dir = root.path().join("MyAddon");
        fs::create_dir_all(&addon_dir).unwrap();
        fs::write(addon_dir.join("MyAddon.toc"), "").unwrap();

        let (staged, addons) = read_addons(&Source::open(&addon_dir).unwrap()).unwrap();

        assert_eq!(staged.root(), addon_dir);
        assert_eq!(addons[0].path, addon_dir);
    }
}
