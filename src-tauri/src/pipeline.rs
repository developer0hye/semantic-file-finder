use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;
use tokio::sync::{Mutex, Semaphore, mpsc};
use tracing::{error, info, warn};

use crate::converter;
use crate::crawler::{CrawlEntry, hash_file};
use crate::db::{Database, FileRecord, PendingFile, embedding_to_bytes};
use crate::error::AppError;
use crate::gemini::DocumentAnalysis;
use crate::platform::normalize_path;
use crate::tantivy_index::{DocumentData, TantivyIndex};

/// Trait for Gemini API operations, enabling test mocking.
pub trait GeminiService: Send + Sync {
    fn analyze_text(
        &self,
        text: &str,
    ) -> impl std::future::Future<Output = Result<DocumentAnalysis, AppError>> + Send;

    fn analyze_text_with_images(
        &self,
        text: &str,
        images: &[(String, Vec<u8>)],
    ) -> impl std::future::Future<Output = Result<DocumentAnalysis, AppError>> + Send;

    fn upload_file(
        &self,
        data: &[u8],
        mime_type: &str,
        display_name: &str,
    ) -> impl std::future::Future<Output = Result<String, AppError>> + Send;

    fn analyze_uploaded_file(
        &self,
        file_uri: &str,
        mime_type: &str,
    ) -> impl std::future::Future<Output = Result<DocumentAnalysis, AppError>> + Send;

    fn embed_text(
        &self,
        text: &str,
        task_type: &str,
    ) -> impl std::future::Future<Output = Result<Vec<f32>, AppError>> + Send;
}

/// Indexing pipeline state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IndexingState {
    Idle,
    Running,
    Paused,
}

/// Current status of the indexing pipeline.
#[derive(Debug, Clone, Serialize)]
pub struct IndexingStatus {
    pub state: IndexingState,
    pub total_files: usize,
    pub indexed_files: usize,
    pub failed_files: usize,
    pub current_file: Option<String>,
}

/// Result of processing a single file through the pipeline.
#[derive(Debug)]
pub struct ProcessedFile {
    pub file_path: String,
    pub analysis: DocumentAnalysis,
    pub embedding: Vec<f32>,
}

/// Process a single file without Gemini (keyword-only mode).
///
/// - Convertible formats: anytomd → raw markdown as summary, filename-based keywords, empty embedding
/// - Gemini-upload formats (PDF, images): returns error — caller should enqueue as pending
pub async fn process_single_file_without_gemini(path: &Path) -> Result<ProcessedFile, AppError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let normalized = normalize_path(path);

    if converter::is_convertible(&ext) {
        let output = tokio::task::spawn_blocking({
            let path = path.to_path_buf();
            move || converter::convert_file(&path)
        })
        .await
        .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))??;

        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        let keywords = extract_filename_keywords(&file_stem);

        Ok(ProcessedFile {
            file_path: normalized,
            analysis: DocumentAnalysis {
                summary: output.markdown,
                keywords,
                language: String::new(),
                doc_type: String::new(),
            },
            embedding: vec![],
        })
    } else if converter::needs_gemini_upload(&ext) {
        Err(AppError::Internal(format!(
            "file requires Gemini API for processing: {}",
            normalized
        )))
    } else {
        Err(AppError::UnsupportedFormat {
            extension: ext.to_string(),
        })
    }
}

/// Extract keyword tokens from a filename stem.
///
/// Splits on common delimiters (underscore, hyphen, space, dot, CamelCase boundaries)
/// and returns lowercased, non-empty tokens.
fn extract_filename_keywords(stem: &str) -> Vec<String> {
    let mut tokens = Vec::new();

    // Split on common delimiters first
    for part in stem.split(['_', '-', ' ', '.']) {
        // Further split on CamelCase boundaries
        let mut current = String::new();
        for ch in part.chars() {
            if ch.is_uppercase() && !current.is_empty() {
                let lower = current.to_lowercase();
                if !lower.is_empty() {
                    tokens.push(lower);
                }
                current.clear();
            }
            current.push(ch);
        }
        if !current.is_empty() {
            let lower = current.to_lowercase();
            if !lower.is_empty() {
                tokens.push(lower);
            }
        }
    }

    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    tokens.retain(|t| seen.insert(t.clone()));

    tokens
}

/// Analyze and embed a single file using the Gemini service.
///
/// This is the core processing function that handles format-specific logic:
/// - Convertible formats (DOCX, PPTX, etc.) → anytomd → Gemini text analysis
/// - Gemini upload formats (PDF, images) → Files API upload → Gemini analysis
/// - Plain text (TXT, MD) → direct Gemini text analysis
pub async fn process_single_file<G: GeminiService>(
    path: &Path,
    gemini: &G,
) -> Result<ProcessedFile, AppError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let normalized = normalize_path(path);
    let analysis: DocumentAnalysis;

    if converter::is_convertible(&ext) {
        // Convert locally with anytomd, then analyze text
        let output = tokio::task::spawn_blocking({
            let path = path.to_path_buf();
            move || converter::convert_file(&path)
        })
        .await
        .map_err(|e| AppError::Internal(format!("spawn_blocking failed: {e}")))??;

        if output.images.is_empty() {
            analysis = gemini.analyze_text(&output.markdown).await?;
        } else {
            let images: Vec<(String, Vec<u8>)> = output
                .images
                .into_iter()
                .map(|img| (img.filename, img.data))
                .collect();
            analysis = gemini
                .analyze_text_with_images(&output.markdown, &images)
                .await?;
        }
    } else if converter::needs_gemini_upload(&ext) {
        // Upload to Gemini Files API, then analyze
        let data = std::fs::read(path).map_err(|e| AppError::FileIo {
            path: normalized.clone(),
            source: e,
        })?;
        let mime = mime_for_extension(&ext);
        let display_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        let file_uri = gemini.upload_file(&data, mime, &display_name).await?;
        analysis = gemini.analyze_uploaded_file(&file_uri, mime).await?;
    } else {
        return Err(AppError::UnsupportedFormat {
            extension: ext.to_string(),
        });
    }

    // Generate embedding from the summary
    let embedding = gemini
        .embed_text(&analysis.summary, "RETRIEVAL_DOCUMENT")
        .await?;

    Ok(ProcessedFile {
        file_path: normalized,
        analysis,
        embedding,
    })
}

/// Store a processed file result in SQLite and Tantivy.
///
/// 1. Upsert to SQLite with index_state = 'pending_tantivy'
/// 2. Upsert to Tantivy
/// 3. Update SQLite index_state to 'indexed'
pub fn store_result(
    db: &Database,
    tantivy: &mut TantivyIndex,
    entry: &CrawlEntry,
    file_hash: &str,
    processed: &ProcessedFile,
) -> Result<(), AppError> {
    let now = chrono_now_iso8601();
    let keywords_json =
        serde_json::to_string(&processed.analysis.keywords).unwrap_or_else(|_| "[]".to_string());

    let record = FileRecord {
        id: 0,
        file_path: processed.file_path.clone(),
        file_name: entry.file_name.clone(),
        file_ext: entry.file_ext.clone(),
        file_size: entry.file_size as i64,
        file_hash: file_hash.to_string(),
        modified_at: format_unix_timestamp(entry.modified_at_unix),
        indexed_at: now,
        summary: processed.analysis.summary.clone(),
        keywords: keywords_json.clone(),
        embedding: embedding_to_bytes(&processed.embedding),
        embedding_dim: processed.embedding.len() as i32,
        index_state: "pending_tantivy".to_string(),
        last_error: None,
    };

    let db_id = db.upsert_file(&record)?;

    let tantivy_result = tantivy.upsert_document(&DocumentData {
        db_id: db_id as u64,
        file_path: &processed.file_path,
        file_name: &entry.file_name,
        file_ext: &entry.file_ext,
        summary: &processed.analysis.summary,
        keywords: &keywords_json,
        file_size: entry.file_size,
        modified_at_unix: entry.modified_at_unix,
    });

    match tantivy_result {
        Ok(()) => {
            db.update_index_state(db_id, "indexed")?;
        }
        Err(e) => {
            warn!(file_path = %processed.file_path, error = %e, "Tantivy upsert failed, keeping pending_tantivy state");
            return Err(e);
        }
    }

    Ok(())
}

/// Run the indexing pipeline over discovered files.
///
/// Processes files concurrently using a semaphore to limit Gemini API calls.
/// Files that fail are recorded in the pending_files table for retry.
pub async fn run_pipeline<G: GeminiService + 'static>(
    gemini: Option<Arc<G>>,
    db: Arc<Mutex<Database>>,
    tantivy: Arc<Mutex<TantivyIndex>>,
    entries: Vec<CrawlEntry>,
    concurrency: usize,
    status: Arc<Mutex<IndexingStatus>>,
    pause_flag: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), AppError> {
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let indexed_count = Arc::new(AtomicUsize::new(0));
    let failed_count = Arc::new(AtomicUsize::new(0));

    {
        let mut s = status.lock().await;
        s.state = IndexingState::Running;
        s.total_files = entries.len();
        s.indexed_files = 0;
        s.failed_files = 0;
    }

    let (result_tx, mut result_rx) = mpsc::channel::<
        Result<(CrawlEntry, String, ProcessedFile), (CrawlEntry, AppError)>,
    >(concurrency * 2);

    // Spawn file processing tasks
    for entry in entries {
        // Check pause flag
        while pause_flag.load(Ordering::Relaxed) {
            {
                let mut s = status.lock().await;
                s.state = IndexingState::Paused;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AppError::Internal(format!("semaphore closed: {e}")))?;

        let gemini = gemini.clone();
        let db = db.clone();
        let status = status.clone();
        let result_tx = result_tx.clone();

        tokio::spawn(async move {
            {
                let mut s = status.lock().await;
                s.state = IndexingState::Running;
                s.current_file = Some(entry.file_path.clone());
            }

            // Check if file needs processing (hash-based change detection)
            let file_path_clone = entry.file_path.clone();
            let needs_processing = {
                let db_guard = db.lock().await;
                match db_guard.get_file_by_path(&file_path_clone) {
                    Ok(Some(existing)) => {
                        // Compute hash and compare
                        match hash_file(Path::new(&file_path_clone)) {
                            Ok(hash) => hash != existing.file_hash,
                            Err(_) => true, // re-process on hash error
                        }
                    }
                    Ok(None) => true, // new file
                    Err(_) => true,   // re-process on DB error
                }
            };

            if !needs_processing {
                drop(permit);
                return;
            }

            let file_hash = match hash_file(Path::new(&entry.file_path)) {
                Ok(h) => h,
                Err(e) => {
                    let _ = result_tx.send(Err((entry, e))).await;
                    drop(permit);
                    return;
                }
            };

            let process_result = match gemini.as_deref() {
                Some(g) => process_single_file(Path::new(&entry.file_path), g).await,
                None => process_single_file_without_gemini(Path::new(&entry.file_path)).await,
            };

            match process_result {
                Ok(processed) => {
                    let _ = result_tx.send(Ok((entry, file_hash, processed))).await;
                }
                Err(e) => {
                    let _ = result_tx.send(Err((entry, e))).await;
                }
            }

            drop(permit);
        });
    }

    // Drop the sender so the receiver knows when all tasks are done
    drop(result_tx);

    // Collect results and store them
    while let Some(result) = result_rx.recv().await {
        match result {
            Ok((entry, file_hash, processed)) => {
                let db_guard = db.lock().await;
                let mut tantivy_guard = tantivy.lock().await;
                match store_result(
                    &db_guard,
                    &mut tantivy_guard,
                    &entry,
                    &file_hash,
                    &processed,
                ) {
                    Ok(()) => {
                        let count = indexed_count.fetch_add(1, Ordering::Relaxed) + 1;
                        let mut s = status.lock().await;
                        s.indexed_files = count;
                        info!(file_path = %processed.file_path, "file indexed successfully");
                    }
                    Err(e) => {
                        error!(file_path = %entry.file_path, error = %e, "failed to store result");
                        let count = failed_count.fetch_add(1, Ordering::Relaxed) + 1;
                        let mut s = status.lock().await;
                        s.failed_files = count;
                        enqueue_failed_file(&db_guard, &entry.file_path, &e.to_string());
                    }
                }
            }
            Err((entry, e)) => {
                error!(file_path = %entry.file_path, error = %e, "failed to process file");
                let count = failed_count.fetch_add(1, Ordering::Relaxed) + 1;
                let mut s = status.lock().await;
                s.failed_files = count;
                let db_guard = db.lock().await;
                enqueue_failed_file(&db_guard, &entry.file_path, &e.to_string());
            }
        }
    }

    {
        let mut s = status.lock().await;
        s.state = IndexingState::Idle;
        s.current_file = None;
    }

    Ok(())
}

/// Reconcile pending_tantivy files on startup.
///
/// For any files with index_state='pending_tantivy', attempt to re-upsert
/// their data into Tantivy and mark them as 'indexed'.
pub fn reconcile_pending_tantivy(
    db: &Database,
    tantivy: &mut TantivyIndex,
) -> Result<usize, AppError> {
    let pending = db.get_files_by_state("pending_tantivy")?;
    let mut reconciled = 0;

    for record in &pending {
        let keywords_str = &record.keywords;
        let result = tantivy.upsert_document(&DocumentData {
            db_id: record.id as u64,
            file_path: &record.file_path,
            file_name: &record.file_name,
            file_ext: &record.file_ext,
            summary: &record.summary,
            keywords: keywords_str,
            file_size: record.file_size as u64,
            modified_at_unix: record.modified_at.parse::<i64>().unwrap_or(0),
        });

        match result {
            Ok(()) => {
                db.update_index_state(record.id, "indexed")?;
                reconciled += 1;
                info!(file_path = %record.file_path, "reconciled pending_tantivy → indexed");
            }
            Err(e) => {
                warn!(
                    file_path = %record.file_path,
                    error = %e,
                    "reconciliation failed, keeping pending_tantivy"
                );
            }
        }
    }

    Ok(reconciled)
}

fn enqueue_failed_file(db: &Database, file_path: &str, error_msg: &str) {
    let pending = PendingFile {
        file_path: file_path.to_string(),
        reason: "retry".to_string(),
        enqueued_at: chrono_now_iso8601(),
        retry_count: 0,
        next_retry_at: None,
        last_error: Some(error_msg.to_string()),
    };
    if let Err(e) = db.enqueue_pending(&pending) {
        error!(file_path = %file_path, error = %e, "failed to enqueue pending file");
    }
}

fn mime_for_extension(ext: &str) -> &'static str {
    match ext {
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

fn chrono_now_iso8601() -> String {
    // Use system time for ISO 8601 timestamp
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_unix_timestamp(now as i64)
}

fn format_unix_timestamp(unix_secs: i64) -> String {
    // Simple ISO 8601 formatting without external chrono dependency
    let secs = unix_secs;
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    // Simple days-since-epoch to date conversion
    let mut y = 1970;
    let mut remaining_days = days;

    loop {
        let days_in_year = if is_leap_year(y) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        y += 1;
    }

    let month_days = if is_leap_year(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m = 0;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining_days < md {
            m = i;
            break;
        }
        remaining_days -= md;
    }

    format!(
        "{y:04}-{:02}-{:02}T{hours:02}:{minutes:02}:{seconds:02}Z",
        m + 1,
        remaining_days + 1
    )
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crawler::crawl_directory;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    // Mock Gemini service for testing
    struct MockGemini {
        analysis: DocumentAnalysis,
        embedding: Vec<f32>,
    }

    impl MockGemini {
        fn new() -> Self {
            Self {
                analysis: DocumentAnalysis {
                    summary: "Test document summary".to_string(),
                    keywords: vec!["test".to_string(), "mock".to_string()],
                    language: "en".to_string(),
                    doc_type: "report".to_string(),
                },
                embedding: vec![0.1, 0.2, 0.3],
            }
        }
    }

    impl GeminiService for MockGemini {
        async fn analyze_text(&self, _text: &str) -> Result<DocumentAnalysis, AppError> {
            Ok(self.analysis.clone())
        }

        async fn analyze_text_with_images(
            &self,
            _text: &str,
            _images: &[(String, Vec<u8>)],
        ) -> Result<DocumentAnalysis, AppError> {
            Ok(self.analysis.clone())
        }

        async fn upload_file(
            &self,
            _data: &[u8],
            _mime_type: &str,
            _display_name: &str,
        ) -> Result<String, AppError> {
            Ok("gs://mock-uri".to_string())
        }

        async fn analyze_uploaded_file(
            &self,
            _file_uri: &str,
            _mime_type: &str,
        ) -> Result<DocumentAnalysis, AppError> {
            Ok(self.analysis.clone())
        }

        async fn embed_text(&self, _text: &str, _task_type: &str) -> Result<Vec<f32>, AppError> {
            Ok(self.embedding.clone())
        }
    }

    fn create_test_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sftest_pipeline_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_format_unix_timestamp() {
        let ts = format_unix_timestamp(0);
        assert_eq!(ts, "1970-01-01T00:00:00Z");

        let ts = format_unix_timestamp(1700000000);
        assert_eq!(ts, "2023-11-14T22:13:20Z");
    }

    #[test]
    fn test_mime_for_extension() {
        assert_eq!(mime_for_extension("pdf"), "application/pdf");
        assert_eq!(mime_for_extension("jpg"), "image/jpeg");
        assert_eq!(mime_for_extension("jpeg"), "image/jpeg");
        assert_eq!(mime_for_extension("png"), "image/png");
        assert_eq!(mime_for_extension("gif"), "image/gif");
        assert_eq!(mime_for_extension("webp"), "image/webp");
        assert_eq!(mime_for_extension("xyz"), "application/octet-stream");
    }

    #[test]
    fn test_store_result_creates_record() {
        let db = Database::open_in_memory().unwrap();
        let mut tantivy = TantivyIndex::open_in_ram().unwrap();

        let entry = CrawlEntry {
            file_path: "/test/doc.txt".to_string(),
            file_name: "doc.txt".to_string(),
            file_ext: ".txt".to_string(),
            file_size: 100,
            modified_at_unix: 1700000000,
        };

        let processed = ProcessedFile {
            file_path: "/test/doc.txt".to_string(),
            analysis: DocumentAnalysis {
                summary: "Test summary".to_string(),
                keywords: vec!["test".to_string()],
                language: "en".to_string(),
                doc_type: "report".to_string(),
            },
            embedding: vec![0.1, 0.2, 0.3],
        };

        store_result(&db, &mut tantivy, &entry, "abc123", &processed).unwrap();

        let record = db.get_file_by_path("/test/doc.txt").unwrap().unwrap();
        assert_eq!(record.summary, "Test summary");
        assert_eq!(record.index_state, "indexed");
        assert_eq!(record.file_hash, "abc123");
    }

    #[test]
    fn test_store_result_updates_existing() {
        let db = Database::open_in_memory().unwrap();
        let mut tantivy = TantivyIndex::open_in_ram().unwrap();

        let entry = CrawlEntry {
            file_path: "/test/doc.txt".to_string(),
            file_name: "doc.txt".to_string(),
            file_ext: ".txt".to_string(),
            file_size: 100,
            modified_at_unix: 1700000000,
        };

        let processed1 = ProcessedFile {
            file_path: "/test/doc.txt".to_string(),
            analysis: DocumentAnalysis {
                summary: "First summary".to_string(),
                keywords: vec!["first".to_string()],
                language: "en".to_string(),
                doc_type: "report".to_string(),
            },
            embedding: vec![0.1, 0.2, 0.3],
        };

        store_result(&db, &mut tantivy, &entry, "hash1", &processed1).unwrap();

        let processed2 = ProcessedFile {
            file_path: "/test/doc.txt".to_string(),
            analysis: DocumentAnalysis {
                summary: "Updated summary".to_string(),
                keywords: vec!["updated".to_string()],
                language: "en".to_string(),
                doc_type: "report".to_string(),
            },
            embedding: vec![0.4, 0.5, 0.6],
        };

        store_result(&db, &mut tantivy, &entry, "hash2", &processed2).unwrap();

        let record = db.get_file_by_path("/test/doc.txt").unwrap().unwrap();
        assert_eq!(record.summary, "Updated summary");
        assert_eq!(record.file_hash, "hash2");
        assert_eq!(db.get_indexed_count().unwrap(), 1);
    }

    #[test]
    fn test_reconcile_pending_tantivy() {
        let db = Database::open_in_memory().unwrap();
        let mut tantivy = TantivyIndex::open_in_ram().unwrap();

        // Insert a record with pending_tantivy state
        let record = FileRecord {
            id: 0,
            file_path: "/test/pending.txt".to_string(),
            file_name: "pending.txt".to_string(),
            file_ext: ".txt".to_string(),
            file_size: 100,
            file_hash: "hash".to_string(),
            modified_at: "1700000000".to_string(),
            indexed_at: "2024-01-01T00:00:00Z".to_string(),
            summary: "Pending summary".to_string(),
            keywords: r#"["pending"]"#.to_string(),
            embedding: crate::db::embedding_to_bytes(&[0.1, 0.2, 0.3]),
            embedding_dim: 3,
            index_state: "pending_tantivy".to_string(),
            last_error: None,
        };
        db.upsert_file(&record).unwrap();

        let reconciled = reconcile_pending_tantivy(&db, &mut tantivy).unwrap();
        assert_eq!(reconciled, 1);

        let updated = db.get_file_by_path("/test/pending.txt").unwrap().unwrap();
        assert_eq!(updated.index_state, "indexed");
    }

    #[test]
    fn test_reconcile_no_pending_files() {
        let db = Database::open_in_memory().unwrap();
        let mut tantivy = TantivyIndex::open_in_ram().unwrap();

        let reconciled = reconcile_pending_tantivy(&db, &mut tantivy).unwrap();
        assert_eq!(reconciled, 0);
    }

    #[tokio::test]
    async fn test_process_single_file_txt() {
        let dir = create_test_dir("single_txt");
        let path = dir.join("test.txt");
        fs::write(&path, "Hello, this is a test document for analysis.").unwrap();

        let gemini = MockGemini::new();
        let result = process_single_file(&path, &gemini).await.unwrap();

        assert_eq!(result.analysis.summary, "Test document summary");
        assert_eq!(result.embedding, vec![0.1, 0.2, 0.3]);
        assert!(result.file_path.ends_with("test.txt"));

        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_process_single_file_unsupported() {
        let dir = create_test_dir("unsupported");
        let path = dir.join("test.xyz");
        fs::write(&path, "unsupported format").unwrap();

        let gemini = MockGemini::new();
        let result = process_single_file(&path, &gemini).await;

        assert!(result.is_err());

        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_run_pipeline_processes_files() {
        let dir = create_test_dir("processes");
        fs::write(dir.join("a.txt"), "Document A content").unwrap();
        fs::write(dir.join("b.txt"), "Document B content").unwrap();

        let gemini = Arc::new(MockGemini::new());
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));

        let entries = crawl_directory(&dir, &[], &[".txt".into()]);
        assert_eq!(entries.len(), 2);

        let status = Arc::new(Mutex::new(IndexingStatus {
            state: IndexingState::Idle,
            total_files: 0,
            indexed_files: 0,
            failed_files: 0,
            current_file: None,
        }));
        let pause_flag = Arc::new(AtomicBool::new(false));

        run_pipeline(
            Some(gemini),
            db.clone(),
            tantivy,
            entries,
            2,
            status.clone(),
            pause_flag,
        )
        .await
        .unwrap();

        let s = status.lock().await;
        assert_eq!(s.state, IndexingState::Idle);
        assert_eq!(s.indexed_files, 2);
        assert_eq!(s.failed_files, 0);

        let db_guard = db.lock().await;
        assert_eq!(db_guard.get_indexed_count().unwrap(), 2);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_run_pipeline_skips_unchanged_files() {
        let dir = create_test_dir("skips_unchanged");
        fs::write(dir.join("a.txt"), "Document A content").unwrap();

        let gemini = Arc::new(MockGemini::new());
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));

        let entries = crawl_directory(&dir, &[], &[".txt".into()]);
        let status = Arc::new(Mutex::new(IndexingStatus {
            state: IndexingState::Idle,
            total_files: 0,
            indexed_files: 0,
            failed_files: 0,
            current_file: None,
        }));
        let pause_flag = Arc::new(AtomicBool::new(false));

        // First run
        run_pipeline(
            Some(gemini.clone()),
            db.clone(),
            tantivy.clone(),
            entries,
            2,
            status.clone(),
            pause_flag.clone(),
        )
        .await
        .unwrap();

        {
            let s = status.lock().await;
            assert_eq!(s.indexed_files, 1);
        }

        // Second run (file unchanged)
        let entries2 = crawl_directory(&dir, &[], &[".txt".into()]);
        let status2 = Arc::new(Mutex::new(IndexingStatus {
            state: IndexingState::Idle,
            total_files: 0,
            indexed_files: 0,
            failed_files: 0,
            current_file: None,
        }));

        run_pipeline(
            Some(gemini),
            db.clone(),
            tantivy,
            entries2,
            2,
            status2.clone(),
            pause_flag,
        )
        .await
        .unwrap();

        let s = status2.lock().await;
        // File unchanged, should be skipped (not counted as indexed in this run)
        assert_eq!(s.indexed_files, 0);

        // But DB still has the file
        let db_guard = db.lock().await;
        assert_eq!(db_guard.get_indexed_count().unwrap(), 1);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_run_pipeline_empty_entries() {
        let gemini = Arc::new(MockGemini::new());
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));

        let status = Arc::new(Mutex::new(IndexingStatus {
            state: IndexingState::Idle,
            total_files: 0,
            indexed_files: 0,
            failed_files: 0,
            current_file: None,
        }));
        let pause_flag = Arc::new(AtomicBool::new(false));

        run_pipeline(
            Some(gemini),
            db,
            tantivy,
            vec![],
            2,
            status.clone(),
            pause_flag,
        )
        .await
        .unwrap();

        let s = status.lock().await;
        assert_eq!(s.state, IndexingState::Idle);
        assert_eq!(s.total_files, 0);
    }

    #[test]
    fn test_indexing_status_initial_state() {
        let status = IndexingStatus {
            state: IndexingState::Idle,
            total_files: 0,
            indexed_files: 0,
            failed_files: 0,
            current_file: None,
        };
        assert_eq!(status.state, IndexingState::Idle);
        assert!(status.current_file.is_none());
    }

    #[test]
    fn test_is_leap_year() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2023));
    }

    #[test]
    fn test_extract_filename_keywords_snake_case() {
        let keywords = extract_filename_keywords("quarterly_report_2024");
        assert!(keywords.contains(&"quarterly".to_string()));
        assert!(keywords.contains(&"report".to_string()));
        assert!(keywords.contains(&"2024".to_string()));
    }

    #[test]
    fn test_extract_filename_keywords_camel_case() {
        let keywords = extract_filename_keywords("QuarterlyReport");
        assert!(keywords.contains(&"quarterly".to_string()));
        assert!(keywords.contains(&"report".to_string()));
    }

    #[test]
    fn test_extract_filename_keywords_mixed_delimiters() {
        let keywords = extract_filename_keywords("my-project_v2.final");
        assert!(keywords.contains(&"my".to_string()));
        assert!(keywords.contains(&"project".to_string()));
        assert!(keywords.contains(&"v2".to_string()));
        assert!(keywords.contains(&"final".to_string()));
    }

    #[test]
    fn test_extract_filename_keywords_deduplication() {
        let keywords = extract_filename_keywords("test_test_test");
        assert_eq!(keywords.iter().filter(|k| *k == "test").count(), 1);
    }

    #[tokio::test]
    async fn test_process_single_file_without_gemini_txt() {
        let dir = create_test_dir("no_gemini_txt");
        let path = dir.join("hello_world.txt");
        fs::write(&path, "Hello, this is a test document.").unwrap();

        let result = process_single_file_without_gemini(&path).await.unwrap();

        assert!(result.file_path.ends_with("hello_world.txt"));
        // Summary is the raw markdown from anytomd
        assert!(result.analysis.summary.contains("Hello"));
        // Keywords extracted from filename
        assert!(result.analysis.keywords.contains(&"hello".to_string()));
        assert!(result.analysis.keywords.contains(&"world".to_string()));
        // No embedding in keyword-only mode
        assert!(result.embedding.is_empty());

        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_process_single_file_without_gemini_pdf_skipped() {
        let dir = create_test_dir("no_gemini_pdf");
        let path = dir.join("document.pdf");
        fs::write(&path, b"%PDF-1.4 fake pdf content").unwrap();

        let result = process_single_file_without_gemini(&path).await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("requires Gemini API"));

        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_run_pipeline_without_gemini_indexes_convertible_files() {
        let dir = create_test_dir("no_gemini_pipeline");
        fs::write(dir.join("a.txt"), "Document A content").unwrap();
        fs::write(dir.join("b.txt"), "Document B content").unwrap();

        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));

        let entries = crawl_directory(&dir, &[], &[".txt".into()]);
        assert_eq!(entries.len(), 2);

        let status = Arc::new(Mutex::new(IndexingStatus {
            state: IndexingState::Idle,
            total_files: 0,
            indexed_files: 0,
            failed_files: 0,
            current_file: None,
        }));
        let pause_flag = Arc::new(AtomicBool::new(false));

        run_pipeline(
            None::<Arc<MockGemini>>,
            db.clone(),
            tantivy,
            entries,
            2,
            status.clone(),
            pause_flag,
        )
        .await
        .unwrap();

        let s = status.lock().await;
        assert_eq!(s.state, IndexingState::Idle);
        assert_eq!(s.indexed_files, 2);
        assert_eq!(s.failed_files, 0);

        let db_guard = db.lock().await;
        assert_eq!(db_guard.get_indexed_count().unwrap(), 2);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn test_run_pipeline_without_gemini_skips_upload_formats() {
        let dir = create_test_dir("no_gemini_skip_upload");
        fs::write(dir.join("doc.txt"), "Text document").unwrap();
        fs::write(dir.join("photo.png"), b"fake png data").unwrap();
        fs::write(dir.join("report.pdf"), b"%PDF-1.4 fake").unwrap();

        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let tantivy = Arc::new(Mutex::new(TantivyIndex::open_in_ram().unwrap()));

        let entries = crawl_directory(&dir, &[], &[".txt".into(), ".png".into(), ".pdf".into()]);
        assert_eq!(entries.len(), 3);

        let status = Arc::new(Mutex::new(IndexingStatus {
            state: IndexingState::Idle,
            total_files: 0,
            indexed_files: 0,
            failed_files: 0,
            current_file: None,
        }));
        let pause_flag = Arc::new(AtomicBool::new(false));

        run_pipeline(
            None::<Arc<MockGemini>>,
            db.clone(),
            tantivy,
            entries,
            2,
            status.clone(),
            pause_flag,
        )
        .await
        .unwrap();

        let s = status.lock().await;
        // Only txt should be indexed, pdf and png should fail (enqueued as pending)
        assert_eq!(s.indexed_files, 1);
        assert_eq!(s.failed_files, 2);

        let db_guard = db.lock().await;
        assert_eq!(db_guard.get_indexed_count().unwrap(), 1);

        cleanup(&dir);
    }
}
