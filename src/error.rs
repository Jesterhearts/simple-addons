//! The error type every fallible step in the installer reports.
//!
//! Errors carry the path they happened on. The message a user sees is the
//! only debugging aid this tool offers, so every variant has to name what it
//! was working on.

use std::io;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("found no World of Warcraft directory; choose one by hand")]
    NoInstallFound,

    #[error(
        "{0} is not a World of Warcraft directory: \
         it holds no game version folder such as _retail_"
    )]
    NotAnInstall(PathBuf),

    #[error("{install} has no game version called \"{requested}\"; it has: {available}")]
    UnknownGameVersion {
        install: PathBuf,
        requested: String,
        available: String,
    },

    #[error("{install} holds several game versions ({available}); pick one with --flavor")]
    AmbiguousGameVersion { install: PathBuf, available: String },

    #[error("found several World of Warcraft directories ({found}); choose one with --wow-dir")]
    SeveralInstallsFound { found: String },

    #[error(
        "{path} is inside {install} but not inside any game version found there \
         ({available}); name the game version with --flavor"
    )]
    NotInGameVersion {
        path: PathBuf,
        install: PathBuf,
        available: String,
    },

    #[error("{0} does not exist")]
    MissingSource(PathBuf),

    #[error("{0} is neither a folder nor a zip file")]
    UnsupportedSource(PathBuf),

    #[error(
        "found no addon in {0}; \
         an addon is a folder holding a .toc file named after it"
    )]
    NoAddonFound(PathBuf),

    #[error("{addon} is not installed in {addons_dir}")]
    NotInstalled { addon: String, addons_dir: PathBuf },

    #[error(
        "\"{requested}\" could mean any of {matches} in {addons_dir}; \
         give the name with the same capital letters as the folder"
    )]
    AmbiguousAddon {
        requested: String,
        matches: String,
        addons_dir: PathBuf,
    },

    #[error(
        "{0} holds an old copy of an addon that could not be moved to safety; \
         move it somewhere else, then try again"
    )]
    OldCopyInTheWay(PathBuf),

    #[error(
        "cannot put the old copy back in {destination} ({reason}); it is kept in {kept}, \
         and the next run puts it back if nothing has taken its place"
    )]
    NotRestored {
        destination: PathBuf,
        kept: PathBuf,
        reason: String,
    },

    #[error(
        "another copy of simple-addons is installing or removing addons in {0}; \
         try again when it has finished"
    )]
    Busy(PathBuf),

    #[error("{archive} holds an entry that would write outside the install: {entry}")]
    UnsafeArchiveEntry { archive: PathBuf, entry: String },

    #[error("cannot read the zip file {archive}: {source}")]
    Archive {
        archive: PathBuf,
        #[source]
        source: zip::result::ZipError,
    },

    #[error("cannot {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl Error {
    /// Builds a mapper for [`io::Error`] that records what was being done and
    /// to which path, for use as `.map_err(Error::io("read", path))`.
    pub fn io(action: &'static str, path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io {
            action,
            path,
            source,
        }
    }
}
