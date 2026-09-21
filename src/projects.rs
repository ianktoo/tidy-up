//! Detection of code / git project directories, which are never torn apart.

use std::{fs, path::Path};

/// Names whose presence marks a directory as a project root (compared case-insensitively).
const MARKERS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "cargo.toml",
    "package.json",
    "pyproject.toml",
    "setup.py",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "cmakelists.txt",
    "composer.json",
    "gemfile",
    "pubspec.yaml",
    "mix.exs",
    "deno.json",
    "package.swift",
    "project.godot",
];

/// Extensions of project files (`*.sln`, ...).
const MARKER_EXTENSIONS: &[&str] = &[
    "sln",
    "csproj",
    "fsproj",
    "vcxproj",
    "uproject",
    "xcodeproj",
];

/// Whether a directory entry named `name` marks its parent as a project root.
///
/// Cheap (no allocation), so scanners can check names while they iterate a directory.
pub fn is_marker(name: &str) -> bool {
    MARKERS.iter().any(|m| name.eq_ignore_ascii_case(m))
        || Path::new(name).extension().is_some_and(|ext| {
            MARKER_EXTENSIONS
                .iter()
                .any(|m| ext.eq_ignore_ascii_case(m))
        })
}

/// Returns `true` if `dir` looks like the root of a software project.
pub fn is_project_dir(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries
        .filter_map(|e| e.ok())
        .any(|entry| is_marker(&entry.file_name().to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for f in files {
            let path = dir.path().join(f);
            if f.ends_with('/') {
                fs::create_dir_all(path).unwrap();
            } else {
                fs::write(path, "x").unwrap();
            }
        }
        dir
    }

    #[test]
    fn detects_git_repos() {
        assert!(is_project_dir(dir_with(&[".git/"]).path()));
    }

    #[test]
    fn detects_manifests_case_insensitively() {
        assert!(is_project_dir(dir_with(&["Cargo.toml"]).path()));
        assert!(is_project_dir(dir_with(&["PACKAGE.JSON"]).path()));
    }

    #[test]
    fn detects_project_file_extensions() {
        assert!(is_project_dir(dir_with(&["App.sln"]).path()));
    }

    #[test]
    fn plain_folders_are_not_projects() {
        assert!(!is_project_dir(
            dir_with(&["holiday.jpg", "notes.txt"]).path()
        ));
        assert!(!is_project_dir(Path::new("/definitely/not/a/dir")));
    }

    #[test]
    fn marker_check_works_on_bare_names() {
        for name in ["Cargo.toml", ".GIT", "My.SLN", "go.mod"] {
            assert!(is_marker(name), "{name}");
        }
        for name in ["notes.txt", "sln", "cargo.txt", ""] {
            assert!(!is_marker(name), "{name}");
        }
    }
}
