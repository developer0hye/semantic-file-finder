use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify_debouncer_full::{
    DebounceEventResult, Debouncer, RecommendedCache, new_debouncer,
    notify::{self, RecursiveMode},
};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::crawler::{CrawlEntry, hash_file};
use crate::db::{Database, PendingFile};
use crate::error::AppError;
use crate::gemini::GeminiClient;
use crate::pipeline::{self, chrono_now_iso8601};
use crate::platform::normalize_path;
use crate::tantivy_index::TantivyIndex;

/// File system watcher that detects changes and auto-indexes files.
pub struct FileWatcher {
    debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    watched_directories: Vec<PathBuf>,
}

impl FileWatcher {
    /// Start watching the given directories for file changes.
    ///
    /// Returns `None` if `directories` is empty. The watcher uses a 2-second
    /// debounce window to coalesce rapid changes before dispatching events.
    pub fn start(
        directories: Vec<PathBuf>,
        extensions: Vec<String>,
        exclude_dirs: Vec<String>,
        db: Arc<Mutex<Database>>,
        tantivy: Arc<Mutex<TantivyIndex>>,
        gemini: Arc<Mutex<Option<GeminiClient>>>,
    ) -> Result<Option<Self>, AppError> {
        if directories.is_empty() {
            return Ok(None);
        }

        let ext_set: Arc<HashSet<String>> = Arc::new(
            extensions
                .iter()
                .map(|e| e.trim_start_matches('.').to_lowercase())
                .collect(),
        );
        let exclude_set: Arc<HashSet<String>> =
            Arc::new(exclude_dirs.iter().map(|d| d.to_string()).collect());

        let rt_handle = tokio::runtime::Handle::current();

        let mut debouncer = new_debouncer(
            Duration::from_secs(2),
            None,
            move |results: DebounceEventResult| {
                let events = match results {
                    Ok(events) => events,
                    Err(errors) => {
                        for e in &errors {
                            if is_inotify_limit_error(&e.to_string()) {
                                error!(
                                    error = %e,
                                    "inotify watch limit exceeded — increase fs.inotify.max_user_watches"
                                );
                            } else {
                                warn!(error = %e, "file watcher error");
                            }
                        }
                        return;
                    }
                };

                let db = db.clone();
                let tantivy = tantivy.clone();
                let gemini = gemini.clone();
                let ext_set = ext_set.clone();
                let exclude_set = exclude_set.clone();

                rt_handle.spawn(async move {
                    for event in events {
                        use notify::EventKind;

                        let paths: Vec<PathBuf> = event
                            .event
                            .paths
                            .iter()
                            .filter(|p| is_relevant_file(p, &ext_set, &exclude_set))
                            .cloned()
                            .collect();

                        match event.event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) => {
                                for path in &paths {
                                    if let Err(e) = handle_file_created_or_modified(
                                        path,
                                        &db,
                                        &tantivy,
                                        &gemini,
                                    )
                                    .await
                                    {
                                        warn!(
                                            file = %path.display(),
                                            error = %e,
                                            "watcher: failed to index file"
                                        );
                                    }
                                }
                            }
                            EventKind::Remove(_) => {
                                for path in &paths {
                                    if let Err(e) =
                                        handle_file_removed(path, &db, &tantivy).await
                                    {
                                        warn!(
                                            file = %path.display(),
                                            error = %e,
                                            "watcher: failed to remove file"
                                        );
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                });
            },
        )
        .map_err(|e| AppError::Internal(format!("failed to create file watcher: {e}")))?;

        let mut watched = Vec::new();
        for dir in &directories {
            match debouncer.watch(dir, RecursiveMode::Recursive) {
                Ok(()) => {
                    info!(directory = %dir.display(), "watching directory");
                    watched.push(dir.clone());
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if is_inotify_limit_error(&err_str) {
                        error!(
                            directory = %dir.display(),
                            error = %e,
                            "inotify watch limit exceeded"
                        );
                    } else {
                        error!(directory = %dir.display(), error = %e, "failed to watch directory");
                    }
                }
            }
        }

        if watched.is_empty() {
            return Ok(None);
        }

        Ok(Some(Self {
            debouncer,
            watched_directories: watched,
        }))
    }

    /// Add a directory to the watch set.
    pub fn watch_directory(&mut self, dir: &Path) -> Result<(), AppError> {
        self.debouncer
            .watch(dir, RecursiveMode::Recursive)
            .map_err(|e| {
                let err_str = e.to_string();
                if is_inotify_limit_error(&err_str) {
                    AppError::WatcherLimitExceeded {
                        directory: dir.display().to_string(),
                    }
                } else {
                    AppError::Internal(format!("failed to watch directory {}: {e}", dir.display()))
                }
            })?;
        if !self.watched_directories.contains(&dir.to_path_buf()) {
            self.watched_directories.push(dir.to_path_buf());
        }
        Ok(())
    }

    /// Remove a directory from the watch set.
    pub fn unwatch_directory(&mut self, dir: &Path) -> Result<(), AppError> {
        self.debouncer.unwatch(dir).map_err(|e| {
            AppError::Internal(format!(
                "failed to unwatch directory {}: {e}",
                dir.display()
            ))
        })?;
        self.watched_directories.retain(|d| d != dir);
        Ok(())
    }

    /// Return the currently watched directories.
    pub fn watched_directories(&self) -> &[PathBuf] {
        &self.watched_directories
    }
}

/// Handle a file creation or modification event.
///
/// Uses hash-based change detection to skip unchanged files.
/// On failure, the file is enqueued as pending for later retry.
async fn handle_file_created_or_modified(
    path: &Path,
    db: &Arc<Mutex<Database>>,
    tantivy: &Arc<Mutex<TantivyIndex>>,
    gemini: &Arc<Mutex<Option<GeminiClient>>>,
) -> Result<(), AppError> {
    if !path.exists() || !path.is_file() {
        return Ok(());
    }

    let normalized = normalize_path(path);
    let file_hash = hash_file(path)?;

    // Check if file has changed
    {
        let db_guard = db.lock().await;
        if let Ok(Some(existing)) = db_guard.get_file_by_path(&normalized)
            && existing.file_hash == file_hash
        {
            return Ok(());
        }
    }

    let entry = build_crawl_entry(path)?;

    // Process the file
    let gemini_guard = gemini.lock().await;
    let process_result = match gemini_guard.as_ref() {
        Some(client) => pipeline::process_single_file(path, client).await,
        None => pipeline::process_single_file_without_gemini(path).await,
    };
    drop(gemini_guard);

    match process_result {
        Ok(processed) => {
            let db_guard = db.lock().await;
            let mut tantivy_guard = tantivy.lock().await;
            pipeline::store_result(
                &db_guard,
                &mut tantivy_guard,
                &entry,
                &file_hash,
                &processed,
            )?;
            info!(file_path = %normalized, "watcher: file indexed");
        }
        Err(e) => {
            let db_guard = db.lock().await;
            let pending = PendingFile {
                file_path: normalized.clone(),
                reason: "retry".to_string(),
                enqueued_at: chrono_now_iso8601(),
                retry_count: 0,
                next_retry_at: None,
                last_error: Some(e.to_string()),
            };
            if let Err(enqueue_err) = db_guard.enqueue_pending(&pending) {
                error!(
                    file_path = %normalized,
                    error = %enqueue_err,
                    "watcher: failed to enqueue pending file"
                );
            }
            return Err(e);
        }
    }

    Ok(())
}

/// Handle a file removal event.
///
/// Looks up the DB record, deletes from Tantivy and DB, and removes
/// any pending file entry.
async fn handle_file_removed(
    path: &Path,
    db: &Arc<Mutex<Database>>,
    tantivy: &Arc<Mutex<TantivyIndex>>,
) -> Result<(), AppError> {
    let normalized = normalize_path(path);

    let db_guard = db.lock().await;
    let record = db_guard.get_file_by_path(&normalized)?;

    if let Some(record) = record {
        let mut tantivy_guard = tantivy.lock().await;
        tantivy_guard.delete_document(record.id as u64)?;
        drop(tantivy_guard);

        db_guard.delete_file(&normalized)?;
        let _ = db_guard.dequeue_pending(&normalized);

        info!(file_path = %normalized, "watcher: file removed from index");
    }

    Ok(())
}

/// Build a CrawlEntry from filesystem metadata.
fn build_crawl_entry(path: &Path) -> Result<CrawlEntry, AppError> {
    let metadata = std::fs::metadata(path).map_err(|e| AppError::FileIo {
        path: path.display().to_string(),
        source: e,
    })?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let modified_at_unix = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    Ok(CrawlEntry {
        file_path: normalize_path(path),
        file_name,
        file_ext: format!(".{ext}"),
        file_size: metadata.len(),
        modified_at_unix,
    })
}

/// Check whether a path is a relevant file for indexing.
///
/// Returns true if the path is a file (not a directory), its extension is
/// in the supported set, and it is not inside an excluded directory.
fn is_relevant_file(
    path: &Path,
    extensions: &HashSet<String>,
    exclude_dirs: &HashSet<String>,
) -> bool {
    // Directories are never relevant
    if path.is_dir() {
        return false;
    }

    // Check extension
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e.to_lowercase(),
        None => return false,
    };
    if !extensions.contains(&ext) {
        return false;
    }

    // Check if any parent directory is excluded
    for component in path.components() {
        if let std::path::Component::Normal(name) = component
            && let Some(name_str) = name.to_str()
            && exclude_dirs.contains(name_str)
        {
            return false;
        }
    }

    true
}

/// Check whether an error message indicates a Linux inotify watch limit error.
fn is_inotify_limit_error(error_msg: &str) -> bool {
    error_msg.contains("inotify")
        && (error_msg.contains("limit") || error_msg.contains("No space left on device"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::gemini::DocumentAnalysis;
    use crate::tantivy_index::TantivyIndex;
    use std::fs;

    fn create_test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sftest_watcher_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn make_ext_set() -> HashSet<String> {
        ["txt", "pdf", "docx", "png"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn make_exclude_set() -> HashSet<String> {
        [".git", "node_modules", "__pycache__"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    // === build_crawl_entry tests ===

    #[test]
    fn test_build_crawl_entry_valid_file() {
        let dir = create_test_dir("build_entry_valid");
        let path = dir.join("document.txt");
        fs::write(&path, "hello world").unwrap();

        let entry = build_crawl_entry(&path).unwrap();

        assert_eq!(entry.file_name, "document.txt");
        assert_eq!(entry.file_ext, ".txt");
        assert_eq!(entry.file_size, 11);
        assert!(entry.modified_at_unix > 0);
        assert!(entry.file_path.ends_with("document.txt"));

        cleanup(&dir);
    }

    #[test]
    fn test_build_crawl_entry_nonexistent_returns_error() {
        let result = build_crawl_entry(Path::new("/nonexistent/file.txt"));
        assert!(result.is_err());
    }

    // === is_relevant_file tests ===

    #[test]
    fn test_is_relevant_file_matching_extension() {
        let dir = create_test_dir("relevant_match");
        let path = dir.join("doc.txt");
        fs::write(&path, "content").unwrap();

        assert!(is_relevant_file(
            &path,
            &make_ext_set(),
            &make_exclude_set()
        ));

        cleanup(&dir);
    }

    #[test]
    fn test_is_relevant_file_excluded_extension() {
        let dir = create_test_dir("relevant_excluded_ext");
        let path = dir.join("script.xyz");
        fs::write(&path, "content").unwrap();

        assert!(!is_relevant_file(
            &path,
            &make_ext_set(),
            &make_exclude_set()
        ));

        cleanup(&dir);
    }

    #[test]
    fn test_is_relevant_file_in_excluded_directory() {
        let dir = create_test_dir("relevant_excluded_dir");
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let path = git_dir.join("config.txt");
        fs::write(&path, "content").unwrap();

        assert!(!is_relevant_file(
            &path,
            &make_ext_set(),
            &make_exclude_set()
        ));

        cleanup(&dir);
    }

    #[test]
    fn test_is_relevant_file_directory_path_returns_false() {
        let dir = create_test_dir("relevant_dir_path");
        // Directories are not relevant files
        assert!(!is_relevant_file(
            &dir,
            &make_ext_set(),
            &make_exclude_set()
        ));

        cleanup(&dir);
    }

    #[test]
    fn test_is_relevant_file_no_extension() {
        let dir = create_test_dir("relevant_no_ext");
        let path = dir.join("Makefile");
        fs::write(&path, "content").unwrap();

        assert!(!is_relevant_file(
            &path,
            &make_ext_set(),
            &make_exclude_set()
        ));

        cleanup(&dir);
    }

    // === is_inotify_limit_error tests ===

    #[test]
    fn test_is_inotify_limit_error_positive_limit() {
        assert!(is_inotify_limit_error("inotify watch limit reached"));
    }

    #[test]
    fn test_is_inotify_limit_error_positive_no_space() {
        assert!(is_inotify_limit_error("inotify: No space left on device"));
    }

    #[test]
    fn test_is_inotify_limit_error_negative() {
        assert!(!is_inotify_limit_error("file not found"));
        assert!(!is_inotify_limit_error("permission denied"));
        assert!(!is_inotify_limit_error(""));
    }

    // === handle_file_removed tests ===

    #[tokio::test]
    async fn test_handle_file_removed_deletes_from_db_and_tantivy() {
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));

        // Insert a file record into DB and Tantivy
        let dir = create_test_dir("remove_deletes");
        let path = dir.join("test.txt");

        let entry = CrawlEntry {
            file_path: normalize_path(&path),
            file_name: "test.txt".to_string(),
            file_ext: ".txt".to_string(),
            file_size: 100,
            modified_at_unix: 1700000000,
        };

        let processed = pipeline::ProcessedFile {
            file_path: normalize_path(&path),
            analysis: DocumentAnalysis {
                summary: "Test summary".to_string(),
                keywords: vec!["test".to_string()],
                language: "en".to_string(),
                doc_type: "report".to_string(),
            },
            embedding: vec![0.1, 0.2, 0.3],
        };

        {
            let db_guard = db.lock().await;
            let mut tantivy_guard = tantivy.lock().await;
            pipeline::store_result(&db_guard, &mut tantivy_guard, &entry, "hash123", &processed)
                .unwrap();
        }

        // Verify it exists
        {
            let db_guard = db.lock().await;
            assert!(
                db_guard
                    .get_file_by_path(&normalize_path(&path))
                    .unwrap()
                    .is_some()
            );
        }

        // Remove it via watcher handler
        handle_file_removed(&path, &db, &tantivy).await.unwrap();

        // Verify it's gone
        {
            let db_guard = db.lock().await;
            assert!(
                db_guard
                    .get_file_by_path(&normalize_path(&path))
                    .unwrap()
                    .is_none()
            );
        }

        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_handle_file_removed_nonexistent_is_noop() {
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));

        let result = handle_file_removed(Path::new("/nonexistent/file.txt"), &db, &tantivy).await;
        assert!(result.is_ok());
    }

    // === handle_file_created_or_modified tests ===

    #[tokio::test]
    async fn test_handle_file_created_indexes_new_file() {
        let dir = create_test_dir("created_indexes");
        let path = dir.join("new_doc.txt");
        fs::write(&path, "This is a new document for indexing").unwrap();

        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));
        let gemini: Arc<Mutex<Option<GeminiClient>>> = Arc::new(Mutex::new(None));

        handle_file_created_or_modified(&path, &db, &tantivy, &gemini)
            .await
            .unwrap();

        // Verify it was indexed
        let db_guard = db.lock().await;
        let record = db_guard.get_file_by_path(&normalize_path(&path)).unwrap();
        assert!(record.is_some());
        let record = record.unwrap();
        assert_eq!(record.index_state, "indexed");
        assert_eq!(record.file_name, "new_doc.txt");

        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_handle_file_modified_skips_unchanged() {
        let dir = create_test_dir("modified_skips");
        let path = dir.join("stable.txt");
        fs::write(&path, "Stable content").unwrap();

        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));
        let gemini: Arc<Mutex<Option<GeminiClient>>> = Arc::new(Mutex::new(None));

        // Index the file first
        handle_file_created_or_modified(&path, &db, &tantivy, &gemini)
            .await
            .unwrap();

        // Get the initial indexed_at
        let initial_indexed_at = {
            let db_guard = db.lock().await;
            let record = db_guard
                .get_file_by_path(&normalize_path(&path))
                .unwrap()
                .unwrap();
            record.indexed_at.clone()
        };

        // Call again without changing the file — should be skipped (hash matches)
        handle_file_created_or_modified(&path, &db, &tantivy, &gemini)
            .await
            .unwrap();

        // indexed_at should remain the same since the file was skipped
        let db_guard = db.lock().await;
        let record = db_guard
            .get_file_by_path(&normalize_path(&path))
            .unwrap()
            .unwrap();
        assert_eq!(record.indexed_at, initial_indexed_at);

        cleanup(&dir);
    }

    // === lifecycle tests ===

    #[tokio::test]
    async fn test_start_with_empty_directories_returns_none() {
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));
        let gemini: Arc<Mutex<Option<GeminiClient>>> = Arc::new(Mutex::new(None));

        let result = FileWatcher::start(
            vec![],
            vec![".txt".to_string()],
            vec![],
            db,
            tantivy,
            gemini,
        )
        .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_start_with_valid_directory_returns_some() {
        let dir = create_test_dir("start_valid");

        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));
        let gemini: Arc<Mutex<Option<GeminiClient>>> = Arc::new(Mutex::new(None));

        let result = FileWatcher::start(
            vec![dir.clone()],
            vec![".txt".to_string()],
            vec![],
            db,
            tantivy,
            gemini,
        )
        .unwrap();

        assert!(result.is_some());
        let watcher = result.unwrap();
        assert_eq!(watcher.watched_directories().len(), 1);
        assert_eq!(watcher.watched_directories()[0], dir);

        cleanup(&dir);
    }
}
