//! Error type shared by every layer of the library.

use std::{
    io,
    path::{Path, PathBuf},
};

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong while scanning, moving, journaling or restoring.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An I/O failure, annotated with the path that caused it.
    #[error("{}: {source}", .path.display())]
    Io {
        /// Path the operation was acting on.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },
    /// A journal file could not be understood.
    #[error("invalid journal {}: {reason}", .path.display())]
    Journal {
        /// Journal file.
        path: PathBuf,
        /// Human readable explanation.
        reason: String,
    },
    /// The caller supplied something unusable (bad path, unknown id, ...).
    #[error("{0}")]
    Invalid(String),
    /// JSON (de)serialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Attaches a path to a bare [`io::Result`].
pub trait IoContext<T> {
    /// Converts the I/O error into [`Error::Io`] tagged with `path`.
    fn at(self, path: &Path) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn at(self, path: &Path) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_context_includes_path_in_message() {
        let err = std::fs::read("definitely/not/here.txt")
            .at(Path::new("definitely/not/here.txt"))
            .unwrap_err();
        assert!(err.to_string().contains("here.txt"));
    }
}
