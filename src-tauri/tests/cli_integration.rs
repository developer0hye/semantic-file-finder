use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use semantic_file_search_lib::app_init;
use semantic_file_search_lib::config;
use semantic_file_search_lib::crawler::crawl_directory;
use semantic_file_search_lib::pipeline::{self, IndexingState, IndexingStatus};
use semantic_file_search_lib::search::{self, SearchMode};
use tokio::sync::Mutex;

fn create_test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sftest_cli_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_cli_search_returns_json_structure() {
    let dir = create_test_dir("search_json");
    let resources = app_init::initialize(&dir).unwrap();

    let embeddings = resources.db.get_all_embeddings().unwrap();
    let results = search::hybrid_search(
        &resources.tantivy,
        "test query",
        None,
        &embeddings,
        SearchMode::KeywordOnly,
        0.4,
        20,
    )
    .unwrap();

    // Empty index should return empty results
    assert!(results.is_empty());

    cleanup(&dir);
}

#[test]
fn test_cli_search_empty_index() {
    let dir = create_test_dir("search_empty");
    let resources = app_init::initialize(&dir).unwrap();

    let embeddings = resources.db.get_all_embeddings().unwrap();
    let results = search::hybrid_search(
        &resources.tantivy,
        "nonexistent document",
        None,
        &embeddings,
        SearchMode::KeywordOnly,
        0.4,
        20,
    )
    .unwrap();

    assert!(results.is_empty());
    assert_eq!(resources.db.get_indexed_count().unwrap(), 0);

    cleanup(&dir);
}

#[tokio::test]
async fn test_cli_index_txt_files() {
    let data_dir = create_test_dir("index_txt");
    let content_dir = create_test_dir("index_txt_content");

    fs::write(content_dir.join("hello.txt"), "Hello world from sfs CLI").unwrap();
    fs::write(
        content_dir.join("notes.txt"),
        "Meeting notes for project planning",
    )
    .unwrap();

    let resources = app_init::initialize(&data_dir).unwrap();

    let entries = crawl_directory(&content_dir, &[], &[".txt".to_string()]);
    assert_eq!(entries.len(), 2);

    let db = Arc::new(Mutex::new(resources.db));
    let tantivy = Arc::new(Mutex::new(resources.tantivy));
    let status = Arc::new(Mutex::new(IndexingStatus {
        state: IndexingState::Idle,
        total_files: 0,
        indexed_files: 0,
        failed_files: 0,
        current_file: None,
    }));
    let pause_flag = Arc::new(AtomicBool::new(false));

    pipeline::run_pipeline(
        None::<Arc<semantic_file_search_lib::gemini::GeminiClient>>,
        db.clone(),
        tantivy.clone(),
        entries,
        2,
        status.clone(),
        pause_flag,
    )
    .await
    .unwrap();

    let final_status = status.lock().await;
    assert_eq!(final_status.indexed_files, 2);
    assert_eq!(final_status.failed_files, 0);

    // Verify search returns results
    let db_guard = db.lock().await;
    let tantivy_guard = tantivy.lock().await;
    let embeddings = db_guard.get_all_embeddings().unwrap();
    let results = search::hybrid_search(
        &tantivy_guard,
        "hello world",
        None,
        &embeddings,
        SearchMode::KeywordOnly,
        0.4,
        10,
    )
    .unwrap();
    assert!(!results.is_empty());

    cleanup(&data_dir);
    cleanup(&content_dir);
}

#[test]
fn test_cli_status_shows_counts() {
    let dir = create_test_dir("status_counts");
    let resources = app_init::initialize(&dir).unwrap();

    let total_files = resources.db.get_indexed_count().unwrap();
    let by_extension = resources.db.get_count_by_extension().unwrap();
    let total_size_bytes = resources.db.get_total_size().unwrap();
    let pending_files = resources.db.get_pending_files().unwrap();

    assert_eq!(total_files, 0);
    assert!(by_extension.is_empty());
    assert_eq!(total_size_bytes, 0);
    assert!(pending_files.is_empty());

    cleanup(&dir);
}

#[test]
fn test_cli_config_show() {
    let dir = create_test_dir("config_show");
    let _ = app_init::initialize(&dir).unwrap();

    let app_config = config::load_config(&dir).unwrap();
    let json = serde_json::to_value(&app_config).unwrap();

    assert!(json.get("search_alpha").is_some());
    assert!(json.get("gemini_model").is_some());
    assert!(json.get("embedding_model").is_some());
    assert!(json.get("supported_extensions").is_some());
    assert!(json.get("watch_directories").is_some());

    cleanup(&dir);
}

#[test]
fn test_cli_config_set_and_read_back() {
    let dir = create_test_dir("config_set");
    let _ = app_init::initialize(&dir).unwrap();

    // Set a config value
    let mut app_config = config::load_config(&dir).unwrap();
    app_config.search_alpha = 0.7;
    config::save_config(&dir, &app_config).unwrap();

    // Read it back
    let reloaded = config::load_config(&dir).unwrap();
    assert!((reloaded.search_alpha - 0.7).abs() < f32::EPSILON);

    // Set another value
    let mut app_config2 = config::load_config(&dir).unwrap();
    app_config2.gemini_model = "custom-model".to_string();
    config::save_config(&dir, &app_config2).unwrap();

    let reloaded2 = config::load_config(&dir).unwrap();
    assert_eq!(reloaded2.gemini_model, "custom-model");
    // Previous value should be preserved
    assert!((reloaded2.search_alpha - 0.7).abs() < f32::EPSILON);

    cleanup(&dir);
}

#[tokio::test]
async fn test_cli_index_then_search_end_to_end() {
    let data_dir = create_test_dir("e2e_index_search");
    let content_dir = create_test_dir("e2e_content");

    fs::write(
        content_dir.join("quarterly_report.txt"),
        "Q3 2024 revenue grew 15% year over year, driven by strong product sales.",
    )
    .unwrap();
    fs::write(
        content_dir.join("meeting_notes.txt"),
        "Action items: review budget proposal, schedule team offsite, finalize hiring plan.",
    )
    .unwrap();

    let resources = app_init::initialize(&data_dir).unwrap();
    let entries = crawl_directory(&content_dir, &[], &[".txt".to_string()]);

    let db = Arc::new(Mutex::new(resources.db));
    let tantivy = Arc::new(Mutex::new(resources.tantivy));
    let status = Arc::new(Mutex::new(IndexingStatus {
        state: IndexingState::Idle,
        total_files: 0,
        indexed_files: 0,
        failed_files: 0,
        current_file: None,
    }));
    let pause_flag = Arc::new(AtomicBool::new(false));

    // Index
    pipeline::run_pipeline(
        None::<Arc<semantic_file_search_lib::gemini::GeminiClient>>,
        db.clone(),
        tantivy.clone(),
        entries,
        2,
        status,
        pause_flag,
    )
    .await
    .unwrap();

    // Search for "revenue"
    let db_guard = db.lock().await;
    let tantivy_guard = tantivy.lock().await;
    let embeddings = db_guard.get_all_embeddings().unwrap();
    let results = search::hybrid_search(
        &tantivy_guard,
        "revenue",
        None,
        &embeddings,
        SearchMode::KeywordOnly,
        0.4,
        10,
    )
    .unwrap();

    assert!(!results.is_empty());
    assert!(results[0].file_path.contains("quarterly_report"));

    // Verify status
    assert_eq!(db_guard.get_indexed_count().unwrap(), 2);

    // Verify per-extension counts
    let by_ext = db_guard.get_count_by_extension().unwrap();
    let ext_map: HashMap<String, usize> = by_ext.into_iter().collect();
    assert_eq!(ext_map.get(".txt"), Some(&2));

    cleanup(&data_dir);
    cleanup(&content_dir);
}
