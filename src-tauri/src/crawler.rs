use std::collections::HashSet;
use std::path::Path;

use walkdir::WalkDir;

use crate::error::AppError;
use crate::platform::normalize_path;

/// A discovered file from directory crawling.
#[derive(Debug, Clone)]
pub struct CrawlEntry {
    pub file_path: String,
    pub file_name: String,
    pub file_ext: String,
    pub file_size: u64,
    pub modified_at_unix: i64,
}

/// Compute a blake3 hash of the file at the given path.
pub fn hash_file(path: &Path) -> Result<String, AppError> {
    let data = std::fs::read(path).map_err(|e| AppError::FileIo {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(blake3::hash(&data).to_hex().to_string())
}

/// Recursively crawl a directory and return all supported files.
///
/// - `root`: directory to start crawling from
/// - `exclude_dirs`: directory names/paths to skip (e.g., ".git", "node_modules")
/// - `supported_extensions`: file extensions to include (e.g., ".pdf", ".docx")
pub fn crawl_directory(
    root: &Path,
    exclude_dirs: &[&str],
    supported_extensions: &[String],
) -> Vec<CrawlEntry> {
    let ext_set: HashSet<String> = supported_extensions
        .iter()
        .map(|e| e.trim_start_matches('.').to_lowercase())
        .collect();

    let exclude_set: HashSet<&str> = exclude_dirs.iter().copied().collect();

    let mut entries = Vec::new();

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !should_exclude(e.path(), e.file_name().to_str().unwrap_or(""), &exclude_set)
        })
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        if !ext_set.contains(&ext) {
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        let modified_at_unix = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        entries.push(CrawlEntry {
            file_path: normalize_path(path),
            file_name,
            file_ext: format!(".{ext}"),
            file_size: metadata.len(),
            modified_at_unix,
        });
    }

    entries
}

/// Check if a path/directory name should be excluded from crawling.
fn should_exclude(path: &Path, name: &str, exclude_set: &HashSet<&str>) -> bool {
    // Check directory name
    if exclude_set.contains(name) {
        return true;
    }

    // Check if path starts with any excluded absolute path
    let path_str = path.to_string_lossy();
    for excluded in exclude_set {
        if (excluded.starts_with('/') || excluded.starts_with('\\'))
            && path_str.starts_with(excluded)
        {
            return true;
        }
    }

    false
}

/// Return the list of all supported file extensions (convertible + gemini upload).
pub fn all_supported_extensions() -> Vec<String> {
    vec![
        // Documents
        ".pdf".into(),
        ".docx".into(),
        ".pptx".into(),
        ".xlsx".into(),
        ".xls".into(),
        ".csv".into(),
        ".json".into(),
        ".txt".into(),
        ".md".into(),
        ".html".into(),
        ".xml".into(),
        // Images
        ".jpg".into(),
        ".jpeg".into(),
        ".png".into(),
        ".gif".into(),
        ".webp".into(),
        // Source code
        ".c".into(),
        ".h".into(),
        ".cpp".into(),
        ".cc".into(),
        ".cxx".into(),
        ".hpp".into(),
        ".hxx".into(),
        ".hh".into(),
        ".py".into(),
        ".pyw".into(),
        ".js".into(),
        ".mjs".into(),
        ".cjs".into(),
        ".jsx".into(),
        ".ts".into(),
        ".mts".into(),
        ".cts".into(),
        ".tsx".into(),
        ".rs".into(),
        ".go".into(),
        ".java".into(),
        ".kt".into(),
        ".kts".into(),
        ".rb".into(),
        ".swift".into(),
        ".cs".into(),
        ".php".into(),
        ".sh".into(),
        ".bash".into(),
        ".zsh".into(),
        ".fish".into(),
        ".pl".into(),
        ".pm".into(),
        ".lua".into(),
        ".r".into(),
        ".scala".into(),
        ".dart".into(),
        ".ex".into(),
        ".exs".into(),
        ".erl".into(),
        ".hs".into(),
        ".ml".into(),
        ".mli".into(),
        ".sql".into(),
        ".m".into(),
        ".mm".into(),
        ".zig".into(),
        ".nim".into(),
        ".v".into(),
        ".groovy".into(),
        ".ps1".into(),
        ".bat".into(),
        ".cmd".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn create_test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sftest_crawler_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_crawl_finds_supported_files() {
        let dir = create_test_dir("finds_supported");
        fs::write(dir.join("doc.pdf"), b"pdf content").unwrap();
        fs::write(dir.join("notes.txt"), b"text content").unwrap();
        fs::write(dir.join("image.png"), b"png content").unwrap();
        fs::write(dir.join("script.py"), b"python code").unwrap(); // not in test extension list

        let extensions = vec![".pdf".into(), ".txt".into(), ".png".into()];
        let entries = crawl_directory(&dir, &[], &extensions);

        assert_eq!(entries.len(), 3);
        let names: Vec<&str> = entries.iter().map(|e| e.file_name.as_str()).collect();
        assert!(names.contains(&"doc.pdf"));
        assert!(names.contains(&"notes.txt"));
        assert!(names.contains(&"image.png"));
        assert!(!names.contains(&"script.py"));

        cleanup(&dir);
    }

    #[test]
    fn test_crawl_recurses_subdirectories() {
        let dir = create_test_dir("recurses");
        let sub = dir.join("subdir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.join("root.txt"), b"root").unwrap();
        fs::write(sub.join("nested.txt"), b"nested").unwrap();

        let extensions = vec![".txt".into()];
        let entries = crawl_directory(&dir, &[], &extensions);

        assert_eq!(entries.len(), 2);

        cleanup(&dir);
    }

    #[test]
    fn test_crawl_excludes_directories() {
        let dir = create_test_dir("excludes");
        let git = dir.join(".git");
        let node = dir.join("node_modules");
        fs::create_dir_all(&git).unwrap();
        fs::create_dir_all(&node).unwrap();
        fs::write(git.join("config.txt"), b"git config").unwrap();
        fs::write(node.join("package.txt"), b"package").unwrap();
        fs::write(dir.join("readme.txt"), b"readme").unwrap();

        let extensions = vec![".txt".into()];
        let entries = crawl_directory(&dir, &[".git", "node_modules"], &extensions);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name, "readme.txt");

        cleanup(&dir);
    }

    #[test]
    fn test_crawl_empty_directory() {
        let dir = create_test_dir("empty");
        let extensions = vec![".txt".into()];
        let entries = crawl_directory(&dir, &[], &extensions);
        assert!(entries.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn test_crawl_entry_has_correct_metadata() {
        let dir = create_test_dir("metadata");
        let content = b"hello world";
        fs::write(dir.join("test.txt"), content).unwrap();

        let extensions = vec![".txt".into()];
        let entries = crawl_directory(&dir, &[], &extensions);

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.file_name, "test.txt");
        assert_eq!(entry.file_ext, ".txt");
        assert_eq!(entry.file_size, content.len() as u64);
        assert!(entry.modified_at_unix > 0);

        cleanup(&dir);
    }

    #[test]
    fn test_crawl_case_insensitive_extension() {
        let dir = create_test_dir("case_insensitive");
        fs::write(dir.join("doc.PDF"), b"pdf").unwrap();
        fs::write(dir.join("notes.TXT"), b"txt").unwrap();

        let extensions = vec![".pdf".into(), ".txt".into()];
        let entries = crawl_directory(&dir, &[], &extensions);

        assert_eq!(entries.len(), 2);

        cleanup(&dir);
    }

    #[test]
    fn test_hash_file_deterministic() {
        let dir = create_test_dir("hash_det");
        let path = dir.join("hash_test.txt");
        fs::write(&path, b"deterministic content").unwrap();

        let hash1 = hash_file(&path).unwrap();
        let hash2 = hash_file(&path).unwrap();
        assert_eq!(hash1, hash2);
        assert!(!hash1.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn test_hash_file_different_content() {
        let dir = create_test_dir("hash_diff");
        let path1 = dir.join("file1.txt");
        let path2 = dir.join("file2.txt");
        fs::write(&path1, b"content A").unwrap();
        fs::write(&path2, b"content B").unwrap();

        let hash1 = hash_file(&path1).unwrap();
        let hash2 = hash_file(&path2).unwrap();
        assert_ne!(hash1, hash2);

        cleanup(&dir);
    }

    #[test]
    fn test_hash_file_nonexistent_returns_error() {
        let result = hash_file(Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_file_empty_file() {
        let dir = create_test_dir("hash_empty");
        let path = dir.join("empty.txt");
        fs::write(&path, b"").unwrap();

        let hash = hash_file(&path).unwrap();
        assert!(!hash.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn test_crawl_normalizes_paths() {
        let dir = create_test_dir("normalize");
        fs::write(dir.join("test.txt"), b"content").unwrap();

        let extensions = vec![".txt".into()];
        let entries = crawl_directory(&dir, &[], &extensions);

        assert_eq!(entries.len(), 1);
        // On Unix, path should not contain backslashes
        assert!(!entries[0].file_path.contains('\\'));

        cleanup(&dir);
    }

    #[test]
    fn test_all_supported_extensions_includes_common_formats() {
        let exts = all_supported_extensions();
        assert!(exts.contains(&".pdf".to_string()));
        assert!(exts.contains(&".docx".to_string()));
        assert!(exts.contains(&".txt".to_string()));
        assert!(exts.contains(&".png".to_string()));
    }

    #[test]
    fn test_all_supported_extensions_includes_code_formats() {
        let exts = all_supported_extensions();
        assert!(exts.contains(&".py".to_string()));
        assert!(exts.contains(&".rs".to_string()));
        assert!(exts.contains(&".js".to_string()));
        assert!(exts.contains(&".ts".to_string()));
        assert!(exts.contains(&".go".to_string()));
        assert!(exts.contains(&".java".to_string()));
        assert!(exts.contains(&".cpp".to_string()));
    }

    #[test]
    fn test_should_exclude_by_name() {
        let exclude: HashSet<&str> = [".git", "node_modules"].iter().copied().collect();
        assert!(should_exclude(Path::new("/repo/.git"), ".git", &exclude));
        assert!(should_exclude(
            Path::new("/repo/node_modules"),
            "node_modules",
            &exclude
        ));
        assert!(!should_exclude(Path::new("/repo/src"), "src", &exclude));
    }
}
