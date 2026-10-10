//! Extracting a zip file into a staging folder.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::error::{Error, Result};
use crate::fs_util;

/// The bytes every zip file starts with.
const ZIP_MAGIC: [u8; 4] = [b'P', b'K', 0x03, 0x04];

/// True when a file is a zip file.
///
/// The contents decide rather than the name, so an archive saved without its
/// `.zip` extension still installs.
pub fn looks_like_zip(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; ZIP_MAGIC.len()];
    file.read_exact(&mut magic).is_ok() && magic == ZIP_MAGIC
}

/// Extracts `archive` into `target`, which must already exist.
///
/// Entry names are checked before anything is written. An entry naming a path
/// outside the target would let an archive write wherever it liked, so such
/// an archive is rejected rather than partly extracted.
pub fn extract(archive: &Path, target: &Path) -> Result<()> {
    let reader = io::BufReader::new(File::open(archive).map_err(Error::io("read", archive))?);
    let mut zip = zip::ZipArchive::new(reader).map_err(archive_error(archive))?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(archive_error(archive))?;
        // A symlink in an addon archive serves no purpose and could point
        // anywhere, so it is left out.
        if entry.is_symlink() {
            continue;
        }
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| Error::UnsafeArchiveEntry {
                archive: archive.to_path_buf(),
                entry: entry_name(&entry, index),
            })?;
        let path = target.join(relative);
        if entry.is_dir() {
            fs_util::create_dir_all(&path)?;
        } else {
            write_entry(&mut entry, &path)?;
        }
    }
    Ok(())
}

fn archive_error(archive: &Path) -> impl FnOnce(zip::result::ZipError) -> Error {
    let archive = archive.to_path_buf();
    move |source| Error::Archive { archive, source }
}

/// The name of an entry, for a message about a bad archive. An entry whose
/// name cannot even be decoded is reported by position.
fn entry_name<R: io::Read>(entry: &zip::read::ZipFile<'_, R>, index: usize) -> String {
    match entry.name() {
        Ok(name) => name.to_string(),
        Err(_) => format!("entry {index}"),
    }
}

/// Writes one archive entry, creating the folders it sits in. Archives are
/// not required to list a folder before the files inside it.
fn write_entry(entry: &mut impl io::Read, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_util::create_dir_all(parent)?;
    }
    let mut file = File::create(path).map_err(Error::io("create", path))?;
    io::copy(entry, &mut file).map_err(Error::io("write", path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::*;

    /// Writes a zip file holding the given entries, using the names exactly as
    /// given so that unsafe names can be tested.
    fn write_zip(path: &Path, entries: &[(&str, &str)]) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for (name, contents) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn extracts_nested_entries() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("addon.zip");
        write_zip(
            &archive,
            &[
                ("MyAddon/MyAddon.toc", "## Title: My Addon\n"),
                ("MyAddon/Modules/Thing.lua", "-- thing\n"),
            ],
        );
        let target = root.path().join("out");
        fs::create_dir_all(&target).unwrap();

        extract(&archive, &target).unwrap();

        assert_eq!(
            fs::read_to_string(target.join("MyAddon/MyAddon.toc")).unwrap(),
            "## Title: My Addon\n"
        );
        assert_eq!(
            fs::read_to_string(target.join("MyAddon/Modules/Thing.lua")).unwrap(),
            "-- thing\n"
        );
    }

    #[test]
    fn refuses_an_entry_that_climbs_out_of_the_target() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("evil.zip");
        write_zip(&archive, &[("../escaped.txt", "nope")]);
        let target = root.path().join("out");
        fs::create_dir_all(&target).unwrap();

        let error = extract(&archive, &target).unwrap_err();

        assert!(matches!(error, Error::UnsafeArchiveEntry { .. }));
        assert!(!root.path().join("escaped.txt").exists());
    }

    #[test]
    fn recognises_a_zip_file_by_its_contents() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("addon-without-extension");
        write_zip(&archive, &[("MyAddon/MyAddon.toc", "")]);

        assert!(looks_like_zip(&archive));
    }

    #[test]
    fn does_not_mistake_other_files_for_zip_files() {
        let root = tempfile::tempdir().unwrap();
        let text = root.path().join("notes.txt");
        fs::write(&text, "just text").unwrap();

        assert!(!looks_like_zip(&text));
        assert!(!looks_like_zip(&root.path().join("missing")));
    }
}
