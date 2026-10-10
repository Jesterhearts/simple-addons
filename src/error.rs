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

    #[error("{0} does not exist")]
    MissingSource(PathBuf),

    #[error("{0} is neither a folder nor a zip file")]
    UnsupportedSource(PathBuf),

    #[error(
        "found no addon in {0}; \
         an addon is a folder holding a .toc file named after it"
    )]
    NoAddonFound(PathBuf),

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
