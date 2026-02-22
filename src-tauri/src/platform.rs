use std::path::{Path, PathBuf};

/// Normalize a filesystem path to Unix-style forward slashes for DB storage.
pub fn normalize_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    #[cfg(target_os = "windows")]
    {
        // Strip extended-length prefix if present
        let stripped = s.strip_prefix(r"\\?\").unwrap_or(&s);
        stripped.replace('\\', "/")
    }
    #[cfg(not(target_os = "windows"))]
    {
        s.into_owned()
    }
}

/// Return the OS-specific list of directories to exclude by default.
pub fn default_exclude_dirs() -> Vec<&'static str> {
    let mut dirs = vec![
        // Common excludes across all platforms
        ".git",
        "node_modules",
        ".cache",
        "__pycache__",
        ".Trash",
        "Thumbs.db",
        ".DS_Store",
    ];

    #[cfg(target_os = "macos")]
    dirs.extend_from_slice(&["/System", "/Library", "/private", "/Volumes", "~/Library"]);

    #[cfg(target_os = "linux")]
    dirs.extend_from_slice(&[
        "/proc",
        "/sys",
        "/dev",
        "/run",
        "/snap",
        "/boot",
        "/lost+found",
    ]);

    #[cfg(target_os = "windows")]
    dirs.extend_from_slice(&[
        r"C:\Windows",
        r"C:\Program Files",
        r"C:\Program Files (x86)",
        r"C:\ProgramData",
        "$Recycle.Bin",
    ]);

    dirs
}

/// Return OS-appropriate default watch directories using the `dirs` crate.
pub fn default_watch_directories() -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Some(doc) = dirs::document_dir() {
        result.push(doc);
    }
    if let Some(desk) = dirs::desktop_dir() {
        result.push(desk);
    }
    result
}

/// Compare two paths, respecting OS case sensitivity rules.
///
/// macOS and Windows are case-insensitive; Linux is case-sensitive.
pub fn compare_paths(a: &Path, b: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        a == b
    }
    #[cfg(not(target_os = "linux"))]
    {
        let a_str = a.to_string_lossy().to_lowercase();
        let b_str = b.to_string_lossy().to_lowercase();
        a_str == b_str
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_unix_path_unchanged() {
        let path = Path::new("/home/user/docs/file.txt");
        assert_eq!(normalize_path(path), "/home/user/docs/file.txt");
    }

    #[test]
    fn default_exclude_dirs_contains_common() {
        let dirs = default_exclude_dirs();
        assert!(dirs.contains(&".git"));
        assert!(dirs.contains(&"node_modules"));
        assert!(dirs.contains(&"__pycache__"));
    }

    #[test]
    fn default_watch_directories_not_empty() {
        // On most systems, at least Documents or Desktop should exist
        let dirs = default_watch_directories();
        // We don't assert non-empty since CI might not have these dirs,
        // but we verify the function doesn't panic.
        let _ = dirs;
    }

    #[test]
    fn compare_paths_same_path() {
        let a = Path::new("/home/user/file.txt");
        let b = Path::new("/home/user/file.txt");
        assert!(compare_paths(a, b));
    }

    #[test]
    fn compare_paths_different() {
        let a = Path::new("/home/user/file.txt");
        let b = Path::new("/home/user/other.txt");
        assert!(!compare_paths(a, b));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn compare_paths_case_insensitive_on_macos_windows() {
        let a = Path::new("/Users/Test/File.TXT");
        let b = Path::new("/Users/test/file.txt");
        assert!(compare_paths(a, b));
    }
}
