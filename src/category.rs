//! File-type categories and the extension table that drives them.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Name of the folder that collects detected code/git projects.
pub const PROJECTS_DIR: &str = "Projects";

/// Broad buckets a file can be sorted into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Category {
    /// Photos and raster/vector pictures.
    Images,
    /// Video clips and movies.
    Videos,
    /// Music, recordings, samples.
    Audio,
    /// PDFs and word-processor documents.
    Documents,
    /// Plain text, notes, logs.
    Text,
    /// Spreadsheets and delimited tables.
    Spreadsheets,
    /// Slide decks.
    Presentations,
    /// E-books.
    Ebooks,
    /// Compressed archives.
    Archives,
    /// Source files and scripts.
    Code,
    /// 3D models and scenes.
    Models3D,
    /// Design-tool source files.
    Design,
    /// Font files.
    Fonts,
    /// Installers and disk images.
    Installers,
    /// Structured data, configs and databases.
    Data,
    /// Anything unrecognised.
    Other,
}

/// Single source of truth: category → lowercase extensions (without the dot).
const TABLE: &[(Category, &[&str])] = &[
    (
        Category::Images,
        &[
            "jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif", "heic", "heif", "svg",
            "ico", "raw", "cr2", "nef", "arw", "dng", "avif",
        ],
    ),
    (
        Category::Videos,
        &[
            "mp4", "mkv", "mov", "avi", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "3gp",
        ],
    ),
    (
        Category::Audio,
        &[
            "mp3", "wav", "flac", "aac", "ogg", "m4a", "wma", "aiff", "opus", "mid", "midi",
        ],
    ),
    (
        Category::Documents,
        &["pdf", "doc", "docx", "odt", "rtf", "pages", "tex"],
    ),
    (
        Category::Text,
        &["txt", "md", "markdown", "log", "rst", "nfo"],
    ),
    (
        Category::Spreadsheets,
        &["xls", "xlsx", "ods", "csv", "tsv", "numbers"],
    ),
    (Category::Presentations, &["ppt", "pptx", "odp", "key"]),
    (Category::Ebooks, &["epub", "mobi", "azw", "azw3", "djvu"]),
    (
        Category::Archives,
        &[
            "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "tgz", "zst", "cab",
        ],
    ),
    (
        Category::Code,
        &[
            "rs", "py", "js", "mjs", "ts", "tsx", "jsx", "java", "c", "h", "cpp", "hpp", "cc",
            "cs", "go", "rb", "php", "swift", "kt", "kts", "sh", "bash", "bat", "cmd", "ps1",
            "html", "htm", "css", "scss", "sass", "lua", "dart", "scala", "r", "vue", "ipynb",
        ],
    ),
    (
        Category::Models3D,
        &[
            "stl", "obj", "fbx", "blend", "gltf", "glb", "3mf", "dae", "ply", "step", "stp", "3ds",
            "skp", "iges", "igs", "usdz", "max", "c4d", "ma", "mb",
        ],
    ),
    (
        Category::Design,
        &[
            "psd", "ai", "xd", "fig", "sketch", "indd", "eps", "afdesign", "afphoto", "kra", "xcf",
        ],
    ),
    (Category::Fonts, &["ttf", "otf", "woff", "woff2", "fon"]),
    (
        Category::Installers,
        &[
            "exe", "msi", "dmg", "pkg", "deb", "rpm", "apk", "appimage", "iso", "img", "msix",
        ],
    ),
    (
        Category::Data,
        &[
            "json", "xml", "yaml", "yml", "toml", "ini", "sql", "db", "sqlite", "sqlite3", "dat",
            "conf", "cfg",
        ],
    ),
];

impl Category {
    /// Every category, in display order.
    pub const ALL: [Category; 16] = [
        Category::Images,
        Category::Videos,
        Category::Audio,
        Category::Documents,
        Category::Text,
        Category::Spreadsheets,
        Category::Presentations,
        Category::Ebooks,
        Category::Archives,
        Category::Code,
        Category::Models3D,
        Category::Design,
        Category::Fonts,
        Category::Installers,
        Category::Data,
        Category::Other,
    ];

    /// Name of the destination folder for this category.
    pub fn folder_name(self) -> &'static str {
        match self {
            Category::Images => "Images",
            Category::Videos => "Videos",
            Category::Audio => "Audio",
            Category::Documents => "Documents",
            Category::Text => "Text Files",
            Category::Spreadsheets => "Spreadsheets",
            Category::Presentations => "Presentations",
            Category::Ebooks => "Ebooks",
            Category::Archives => "Archives",
            Category::Code => "Code",
            Category::Models3D => "3D Models",
            Category::Design => "Design",
            Category::Fonts => "Fonts",
            Category::Installers => "Installers",
            Category::Data => "Data",
            Category::Other => "Other",
        }
    }

    /// Classifies a bare extension (with or without a leading dot, any case).
    pub fn from_extension(ext: &str) -> Category {
        let ext = ext.trim_start_matches('.').to_lowercase();
        TABLE
            .iter()
            .find(|(_, exts)| exts.contains(&ext.as_str()))
            .map_or(Category::Other, |(cat, _)| *cat)
    }

    /// Classifies a path by its extension; extension-less files are [`Category::Other`].
    pub fn from_path(path: &Path) -> Category {
        path.extension()
            .and_then(|e| e.to_str())
            .map_or(Category::Other, Category::from_extension)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn classifies_common_extensions() {
        assert_eq!(Category::from_extension("PNG"), Category::Images);
        assert_eq!(Category::from_extension(".stl"), Category::Models3D);
        assert_eq!(Category::from_extension("rs"), Category::Code);
        assert_eq!(Category::from_extension("md"), Category::Text);
        assert_eq!(Category::from_extension("weird"), Category::Other);
    }

    #[test]
    fn classifies_paths() {
        assert_eq!(
            Category::from_path(Path::new("a/b/report.PDF")),
            Category::Documents
        );
        assert_eq!(Category::from_path(Path::new("Makefile")), Category::Other);
        assert_eq!(
            Category::from_path(Path::new(".gitignore")),
            Category::Other
        );
        assert_eq!(
            Category::from_path(Path::new("x.tar.gz")),
            Category::Archives
        );
    }

    #[test]
    fn no_extension_is_claimed_twice() {
        let mut seen = HashSet::new();
        for (_, exts) in TABLE {
            for e in *exts {
                assert!(seen.insert(*e), "extension `{e}` appears twice");
                assert_eq!(*e, e.to_lowercase(), "extensions must be lowercase");
            }
        }
    }

    #[test]
    fn folder_names_are_unique_and_all_is_complete() {
        let names: HashSet<_> = Category::ALL.iter().map(|c| c.folder_name()).collect();
        assert_eq!(names.len(), Category::ALL.len());
        for (cat, _) in TABLE {
            assert!(Category::ALL.contains(cat));
        }
    }
}
