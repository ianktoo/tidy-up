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

/// Like [`resolve_root`], but the folder may not exist yet: the nearest existing parent is
/// resolved and the missing tail is appended. Used for destinations that will be created.
pub fn resolve_maybe_new(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return resolve_root(path);
    }
    let mut tail = Vec::new();
    let mut current = path;
    let base = loop {
        let Some(name) = current.file_name() else {
            return Err(Error::Invalid(format!(
                "{} has no existing parent folder",
                path.display()
            )));
        };
        tail.push(name.to_owned());
        current = current.parent().unwrap_or(Path::new(""));
        let probe = if current.as_os_str().is_empty() {
            Path::new(".")
        } else {
            current
        };
        if probe.exists() {
            break resolve_root(probe)?;
        }
    };
    Ok(tail
        .into_iter()
        .rev()
        .fold(base, |acc, part| acc.join(part)))
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
    move_path_with(from, to, &|a, b| fs::rename(a, b))
}

/// [`move_path`] with the rename step injected, so tests can simulate a rename that fails the
/// way it does between partitions without needing a second partition.
fn move_path_with(
    from: &Path,
    to: &Path,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    if to.exists() {
        return Err(Error::Invalid(format!(
            "refusing to overwrite existing {}",
            to.display()
        )));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    match rename(from, to) {
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
    let modified = fs::metadata(from).and_then(|m| m.modified()).ok();
    fs::copy(from, to).at(to)?;
    // A rename keeps timestamps, so a copy must too, or moving between partitions would
    // reset every file's modified date. Best effort: some file systems refuse it.
    if let Some(modified) = modified {
        let _ = fs::File::options()
            .write(true)
            .open(to)
            .and_then(|f| f.set_modified(modified));
    }
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

    /// What `fs::rename` does between partitions: it fails and the caller must copy instead.
    fn cross_device(_: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::CrossesDevices.into())
    }

    #[test]
    fn a_rename_that_fails_across_devices_falls_back_to_copy_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.bin"), dir.path().join("sub/b.bin"));
        fs::write(&a, b"payload").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_400_000_000);
        fs::File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(old)
            .unwrap();

        move_path_with(&a, &b, &cross_device).unwrap();

        assert!(
            !a.exists(),
            "the source is removed only after the copy is verified"
        );
        assert_eq!(fs::read(&b).unwrap(), b"payload");
        let drift = fs::metadata(&b)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(old)
            .unwrap_or_else(|e| e.duration())
            .as_secs();
        assert!(drift <= 2, "modified time must survive, drifted {drift}s");
    }

    #[test]
    fn a_directory_cannot_be_copied_so_a_failed_rename_leaves_it_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("proj");
        fs::create_dir_all(src.join("inner")).unwrap();
        fs::write(src.join("inner/f.txt"), "1").unwrap();
        let dst = dir.path().join("elsewhere/proj");
        assert!(move_path_with(&src, &dst, &cross_device).is_err());
        assert!(src.join("inner/f.txt").exists(), "nothing lost");
        assert!(!dst.exists(), "nothing half-created");
    }

    #[test]
    fn a_failed_rename_never_overwrites_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&a, "new").unwrap();
        fs::write(&b, "existing").unwrap();
        assert!(move_path_with(&a, &b, &cross_device).is_err());
        assert_eq!(fs::read_to_string(&b).unwrap(), "existing");
        assert_eq!(fs::read_to_string(&a).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn the_copy_fallback_preserves_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("run.sh"), dir.path().join("moved.sh"));
        fs::write(&a, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&a, fs::Permissions::from_mode(0o750)).unwrap();
        move_path_with(&a, &b, &cross_device).unwrap();
        assert_eq!(
            fs::metadata(&b).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }

    /// File systems differ (Linux is case sensitive, Windows and default macOS are not), so
    /// the property is stated in terms of what *this* file system says exists.
    #[test]
    fn unique_names_never_collide_on_case_sensitive_or_insensitive_file_systems() {
        let dir = tempfile::tempdir().unwrap();
        let lower = dir.path().join("photo.jpg");
        fs::write(&lower, "x").unwrap();
        let insensitive = dir.path().join("PHOTO.JPG").exists();

        let wanted = dir.path().join("PHOTO.JPG");
        let got = unique_path(&wanted, &HashSet::new());
        assert!(
            !got.exists(),
            "the returned name must be free on this file system"
        );
        if insensitive {
            assert_eq!(
                got,
                dir.path().join("PHOTO (1).JPG"),
                "case-insensitive: it clashes"
            );
        } else {
            assert_eq!(got, wanted, "case-sensitive: it is a different file");
        }
    }

    #[test]
    fn copy_fallback_preserves_the_modified_time() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.bin"), dir.path().join("b.bin"));
        fs::write(&a, [1u8, 2, 3]).unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
        fs::File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(old)
            .unwrap();
        copy_verify_remove(&a, &b, 3).unwrap();
        let after = fs::metadata(&b).unwrap().modified().unwrap();
        let drift = after
            .duration_since(old)
            .unwrap_or_else(|e| e.duration())
            .as_secs();
        assert!(
            drift <= 2,
            "modified time should survive the copy, drifted {drift}s"
        );
    }

    #[test]
    fn resolve_maybe_new_handles_existing_and_missing_tails() {
        let dir = tempfile::tempdir().unwrap();
        let base = resolve_root(dir.path()).unwrap();
        assert_eq!(resolve_maybe_new(dir.path()).unwrap(), base);
        let future = dir.path().join("a").join("b");
        assert_eq!(
            resolve_maybe_new(&future).unwrap(),
            base.join("a").join("b")
        );
        assert!(!future.exists(), "resolving must not create anything");
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
