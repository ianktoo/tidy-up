//! Low-level, non-destructive filesystem helpers.
//!
//! Nothing here ever overwrites an existing file: destinations are checked and
//! de-duplicated by name first.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::error::{Error, IoContext, Result};

/// Resolves `path` to an absolute directory, stripping the Windows `\\?\` prefix.
pub fn resolve_root(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path).at(path)?;
    if !canonical.is_dir() {
        return Err(Error::Invalid(format!(
            "{} is not a folder",
            path.display()
        )));
    }
    Ok(strip_verbatim(canonical))
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// Returns `desired`, or `name (1).ext`, `name (2).ext`, ... if it is taken.
///
/// A name is "taken" if it exists on disk or is in `reserved` (destinations
/// already promised to other pending moves).
pub fn unique_path(desired: &Path, reserved: &HashSet<PathBuf>) -> PathBuf {
    let free = |p: &Path| !p.exists() && !reserved.contains(p);
    if free(desired) {
        return desired.to_path_buf();
    }
    let stem = desired
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = desired
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    let parent = desired.parent().unwrap_or(Path::new(""));
    (1u32..)
        .map(|n| match &ext {
            Some(ext) => parent.join(format!("{stem} ({n}).{ext}")),
            None => parent.join(format!("{stem} ({n})")),
        })
        .find(|candidate| free(candidate))
        .expect("u32 range exhausted")
}

/// Creates `dir` (and parents) and returns the directories that did not exist before,
/// outermost first, so they can be journaled and cleaned up on restore.
pub fn create_dirs_tracked(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut missing = Vec::new();
    let mut current = dir;
    while !current.exists() {
        missing.push(current.to_path_buf());
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    fs::create_dir_all(dir).at(dir)?;
    missing.reverse();
    Ok(missing)
}

/// Moves a file or directory without ever overwriting `to`.
///
/// Uses an atomic rename when possible and falls back to copy + verify + delete
/// for files when the rename fails (for example across drives).
pub fn move_path(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        return Err(Error::Invalid(format!(
            "refusing to overwrite existing {}",
            to.display()
        )));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            let meta = fs::symlink_metadata(from).at(from)?;
            if meta.is_file() {
                copy_verify_remove(from, to, meta.len())
            } else {
                Err(Error::Io {
                    path: from.to_path_buf(),
                    source: rename_error,
                })
            }
        }
    }
}

fn copy_verify_remove(from: &Path, to: &Path, expected_len: u64) -> Result<()> {
    fs::copy(from, to).at(to)?;
    let copied_ok = fs::metadata(to).is_ok_and(|m| m.len() == expected_len);
    if !copied_ok {
        let _ = fs::remove_file(to);
        return Err(Error::Invalid(format!(
            "copy of {} was incomplete; original left in place",
            from.display()
        )));
    }
    if let Err(source) = fs::remove_file(from) {
        let _ = fs::remove_file(to);
        return Err(Error::Io {
            path: from.to_path_buf(),
            source,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_path_returns_desired_when_free() {
        let dir = tempfile::tempdir().unwrap();
        let want = dir.path().join("a.txt");
        assert_eq!(unique_path(&want, &HashSet::new()), want);
    }

    #[test]
    fn unique_path_counts_up_over_disk_and_reserved() {
        let dir = tempfile::tempdir().unwrap();
        let want = dir.path().join("a.txt");
        fs::write(&want, "x").unwrap();
        let mut reserved = HashSet::new();
        reserved.insert(dir.path().join("a (1).txt"));
        assert_eq!(unique_path(&want, &reserved), dir.path().join("a (2).txt"));
    }

    #[test]
    fn unique_path_handles_extensionless_names() {
        let dir = tempfile::tempdir().unwrap();
        let want = dir.path().join("Makefile");
        fs::write(&want, "x").unwrap();
        assert_eq!(
            unique_path(&want, &HashSet::new()),
            dir.path().join("Makefile (1)")
        );
    }

    #[test]
    fn tracked_dirs_report_only_new_ones_outermost_first() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("exists")).unwrap();
        let target = dir.path().join("exists").join("x").join("y");
        let created = create_dirs_tracked(&target).unwrap();
        assert_eq!(
            created,
            [dir.path().join("exists/x"), dir.path().join("exists/x/y")]
        );
        assert!(create_dirs_tracked(&target).unwrap().is_empty());
    }

    #[test]
    fn move_path_moves_and_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("sub/b.txt"));
        fs::write(&a, "hello").unwrap();
        move_path(&a, &b).unwrap();
        assert!(!a.exists());
        assert_eq!(fs::read_to_string(&b).unwrap(), "hello");

        fs::write(&a, "other").unwrap();
        assert!(move_path(&a, &b).is_err());
        assert_eq!(fs::read_to_string(&b).unwrap(), "hello");
        assert_eq!(fs::read_to_string(&a).unwrap(), "other");
    }

    #[test]
    fn move_path_moves_directories() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("proj");
        fs::create_dir_all(src.join("inner")).unwrap();
        fs::write(src.join("inner/f.txt"), "1").unwrap();
        let dst = dir.path().join("Projects/proj");
        move_path(&src, &dst).unwrap();
        assert!(dst.join("inner/f.txt").exists());
        assert!(!src.exists());
    }

    #[test]
    fn copy_fallback_verifies_and_removes_source() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.bin"), dir.path().join("b.bin"));
        fs::write(&a, [1u8, 2, 3]).unwrap();
        copy_verify_remove(&a, &b, 3).unwrap();
        assert!(!a.exists() && b.exists());

        let (c, d) = (dir.path().join("c.bin"), dir.path().join("d.bin"));
        fs::write(&c, [1u8, 2, 3]).unwrap();
        assert!(copy_verify_remove(&c, &d, 99).is_err());
        assert!(c.exists() && !d.exists());
    }

    #[test]
    fn resolve_root_rejects_files_and_strips_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, "x").unwrap();
        assert!(resolve_root(&file).is_err());
        assert!(resolve_root(dir.path()).unwrap().is_dir());
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
    }
}
