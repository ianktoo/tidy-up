//! Partition capacity and free space, plus parsing of human sizes like `200GiB` and `90%`.
//!
//! Space queries go through the [`DiskProbe`] trait so the planning code can be tested with
//! [`StaticDisks`] instead of depending on the machine it runs on.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::error::{Error, IoContext, Result};

/// Capacity of the partition a path lives on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskSpace {
    /// Total size of the partition in bytes.
    pub total: u64,
    /// Bytes available to this user (what a new file could actually use).
    pub available: u64,
    /// Identifies the partition; two paths with equal ids share the same free space.
    pub volume: String,
}

impl DiskSpace {
    /// Bytes in use (`total - available`).
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Fraction of the partition in use, `0.0` to `1.0`.
    pub fn used_fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.used() as f64 / self.total as f64
        }
    }
}

/// Something that can report partition space for a path.
pub trait DiskProbe {
    /// Space on the partition holding `path`.
    fn space(&self, path: &Path) -> Result<DiskSpace>;
}

/// Asks the operating system (Windows, macOS and Linux).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDisks;

impl DiskProbe for SystemDisks {
    /// A folder that does not exist yet is measured through its nearest existing parent,
    /// which is where it would be created.
    fn space(&self, path: &Path) -> Result<DiskSpace> {
        let existing = nearest_existing(path);
        let stats = fs4::statvfs(existing).at(existing)?;
        Ok(DiskSpace {
            total: stats.total_space(),
            available: stats.available_space(),
            volume: volume_id(existing)?,
        })
    }
}

fn nearest_existing(path: &Path) -> &Path {
    path.ancestors()
        .find(|p| !p.as_os_str().is_empty() && p.exists())
        .unwrap_or(Path::new("."))
}

/// A stable identifier for the partition holding `path`.
///
/// Unix: the device number. Windows: the drive prefix (a folder mounted from another
/// volume into a drive is not distinguished from that drive).
pub fn volume_id(path: &Path) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!("dev:{}", std::fs::metadata(path).at(path)?.dev()))
    }
    #[cfg(windows)]
    {
        use std::path::Component;
        match path.components().next() {
            Some(Component::Prefix(prefix)) => {
                Ok(prefix.as_os_str().to_string_lossy().to_uppercase())
            }
            _ => Err(Error::Invalid(format!(
                "cannot determine the drive of {}",
                path.display()
            ))),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = std::fs::metadata(path).at(path)?;
        Ok("default".to_string())
    }
}

/// A fixed table of partitions for tests and simulations. The longest matching path prefix wins.
#[derive(Debug, Clone, Default)]
pub struct StaticDisks(HashMap<PathBuf, DiskSpace>);

impl StaticDisks {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares the partition that `path` (and everything under it) lives on.
    pub fn with(
        mut self,
        path: impl Into<PathBuf>,
        total: u64,
        available: u64,
        volume: &str,
    ) -> Self {
        self.0.insert(
            path.into(),
            DiskSpace {
                total,
                available,
                volume: volume.to_string(),
            },
        );
        self
    }
}

impl DiskProbe for StaticDisks {
    fn space(&self, path: &Path) -> Result<DiskSpace> {
        self.0
            .iter()
            .filter(|(prefix, _)| path.starts_with(prefix))
            .max_by_key(|(prefix, _)| prefix.components().count())
            .map(|(_, space)| space.clone())
            .ok_or_else(|| Error::Invalid(format!("no disk info for {}", path.display())))
    }
}

/// Parses a size such as `1024`, `500M`, `1.5GB` or `200 GiB`.
///
/// All units are binary (1 KB = 1 KiB = 1024 bytes), matching how tidy-up prints sizes.
pub fn parse_size(text: &str) -> Result<u64> {
    let bad = || {
        Error::Invalid(format!(
            "`{}` is not a size; try 500M, 1.5G or 200GiB",
            text.trim()
        ))
    };
    let cleaned: String = text
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_')
        .collect();
    let split = cleaned
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(cleaned.len());
    let (number, unit) = cleaned.split_at(split);
    let value: f64 = number.parse().map_err(|_| bad())?;
    let multiplier = match unit {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024f64.powi(2),
        "g" | "gb" | "gib" => 1024f64.powi(3),
        "t" | "tb" | "tib" => 1024f64.powi(4),
        _ => return Err(bad()),
    };
    let bytes = value * multiplier;
    if !bytes.is_finite() || bytes >= u64::MAX as f64 {
        return Err(bad());
    }
    Ok(bytes as u64)
}

/// Parses `90`, `90%` or `87.5` into a fraction in `(0, 1]`.
pub fn parse_percent(text: &str) -> Result<f64> {
    let bad = || {
        Error::Invalid(format!(
            "`{}` is not a percentage between 1 and 100",
            text.trim()
        ))
    };
    let value: f64 = text
        .trim()
        .trim_end_matches('%')
        .trim()
        .parse()
        .map_err(|_| bad())?;
    if !(value > 0.0 && value <= 100.0) {
        return Err(bad());
    }
    Ok(value / 100.0)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn sizes_parse_with_all_common_spellings() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1k").unwrap(), 1024);
        assert_eq!(parse_size("2KiB").unwrap(), 2048);
        assert_eq!(parse_size("500M").unwrap(), 500 * 1024 * 1024);
        assert_eq!(parse_size("1.5G").unwrap(), 1_610_612_736);
        assert_eq!(parse_size(" 200 gib ").unwrap(), 200 * 1024u64.pow(3));
        assert_eq!(parse_size("1_000MB").unwrap(), 1000 * 1024 * 1024);
        assert_eq!(parse_size("2T").unwrap(), 2 * 1024u64.pow(4));
        assert_eq!(parse_size("0").unwrap(), 0);
    }

    #[test]
    fn bad_sizes_are_rejected() {
        for bad in [
            "",
            "abc",
            "-5G",
            "10 parsecs",
            "1..5G",
            "G",
            "99999999999999999999TB",
        ] {
            assert!(parse_size(bad).is_err(), "`{bad}` should be rejected");
        }
    }

    #[test]
    fn percentages_parse_to_fractions() {
        assert_eq!(parse_percent("90").unwrap(), 0.9);
        assert_eq!(parse_percent("90%").unwrap(), 0.9);
        assert_eq!(parse_percent(" 87.5 % ").unwrap(), 0.875);
        assert_eq!(parse_percent("100").unwrap(), 1.0);
        for bad in ["0", "-1", "101", "x", ""] {
            assert!(parse_percent(bad).is_err(), "`{bad}`");
        }
    }

    #[test]
    fn used_and_fraction() {
        let s = DiskSpace {
            total: 200,
            available: 50,
            volume: "v".into(),
        };
        assert_eq!(s.used(), 150);
        assert_eq!(s.used_fraction(), 0.75);
        let empty = DiskSpace {
            total: 0,
            available: 0,
            volume: "v".into(),
        };
        assert_eq!(empty.used_fraction(), 0.0);
        let odd = DiskSpace {
            total: 10,
            available: 20,
            volume: "v".into(),
        };
        assert_eq!(odd.used(), 0, "available above total must not underflow");
    }

    #[test]
    fn static_disks_pick_the_longest_matching_prefix() {
        let disks =
            StaticDisks::new()
                .with("/data", 1000, 500, "a")
                .with("/data/big", 9000, 100, "b");
        assert_eq!(disks.space(Path::new("/data/x")).unwrap().volume, "a");
        assert_eq!(disks.space(Path::new("/data/big/y")).unwrap().volume, "b");
        assert!(disks.space(Path::new("/elsewhere")).is_err());
    }

    #[test]
    fn real_disks_report_sane_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let space = SystemDisks.space(dir.path()).unwrap();
        assert!(space.total > 0);
        assert!(space.available <= space.total);
        assert!((0.0..=1.0).contains(&space.used_fraction()));
    }

    #[test]
    fn folders_that_do_not_exist_yet_are_measured_through_their_parent() {
        let dir = tempfile::tempdir().unwrap();
        let future = dir.path().join("not").join("created").join("yet");
        let space = SystemDisks.space(&future).unwrap();
        let parent = SystemDisks.space(dir.path()).unwrap();
        // free space changes between two live probes, so compare only the stable facts
        assert_eq!(space.total, parent.total);
        assert_eq!(space.volume, parent.volume);
    }

    #[test]
    fn folders_on_one_partition_share_a_volume_id() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        fs::create_dir(dir.path().join("b")).unwrap();
        assert_eq!(
            volume_id(&dir.path().join("a")).unwrap(),
            volume_id(&dir.path().join("b")).unwrap()
        );
    }
}
