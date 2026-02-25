use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::tantivy_index::SearchFilters;

/// A file record stored in the `files` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: i64,
    pub file_path: String,
    pub file_name: String,
    pub file_ext: String,
    pub file_size: i64,
    pub file_hash: String,
    pub modified_at: String,
    pub indexed_at: String,
    pub summary: String,
    pub keywords: String,
    pub embedding: Vec<u8>,
    pub embedding_dim: i32,
    pub index_state: String,
    pub last_error: Option<String>,
}

/// A pending file record from the `pending_files` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingFile {
    pub file_path: String,
    pub reason: String,
    pub enqueued_at: String,
    pub retry_count: i32,
    pub next_retry_at: Option<String>,
    pub last_error: Option<String>,
}

/// A row from get_all_embeddings: (id, file_path, embedding_bytes, embedding_dim).
pub type EmbeddingRow = (i64, String, Vec<u8>, i32);

/// SQLite database handle for the application.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) a database at the given path with WAL mode and schema init.
    pub fn open(db_path: &Path) -> Result<Self, AppError> {
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, AppError> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<(), AppError> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS files (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path     TEXT NOT NULL UNIQUE,
                file_name     TEXT NOT NULL,
                file_ext      TEXT NOT NULL,
                file_size     INTEGER NOT NULL,
                file_hash     TEXT NOT NULL,
                modified_at   TEXT NOT NULL,
                indexed_at    TEXT NOT NULL,
                summary       TEXT NOT NULL,
                keywords      TEXT NOT NULL,
                embedding     BLOB NOT NULL,
                embedding_dim INTEGER NOT NULL,
                index_state   TEXT NOT NULL DEFAULT 'indexed',
                last_error    TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_files_hash ON files(file_hash);
            CREATE INDEX IF NOT EXISTS idx_files_ext ON files(file_ext);
            CREATE INDEX IF NOT EXISTS idx_files_path ON files(file_path);
            CREATE INDEX IF NOT EXISTS idx_files_index_state ON files(index_state);

            CREATE TABLE IF NOT EXISTS pending_files (
                file_path      TEXT PRIMARY KEY,
                reason         TEXT NOT NULL,
                enqueued_at    TEXT NOT NULL,
                retry_count    INTEGER NOT NULL DEFAULT 0,
                next_retry_at  TEXT,
                last_error     TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_pending_next_retry ON pending_files(next_retry_at);
            ",
        )?;
        Ok(())
    }

    // ---- files CRUD ----

    /// Insert or update a file record. Returns the row id.
    pub fn upsert_file(&self, record: &FileRecord) -> Result<i64, AppError> {
        self.conn.execute(
            "INSERT INTO files (file_path, file_name, file_ext, file_size, file_hash,
                                modified_at, indexed_at, summary, keywords, embedding,
                                embedding_dim, index_state, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(file_path) DO UPDATE SET
                file_name=excluded.file_name, file_ext=excluded.file_ext,
                file_size=excluded.file_size, file_hash=excluded.file_hash,
                modified_at=excluded.modified_at, indexed_at=excluded.indexed_at,
                summary=excluded.summary, keywords=excluded.keywords,
                embedding=excluded.embedding, embedding_dim=excluded.embedding_dim,
                index_state=excluded.index_state, last_error=excluded.last_error",
            params![
                record.file_path,
                record.file_name,
                record.file_ext,
                record.file_size,
                record.file_hash,
                record.modified_at,
                record.indexed_at,
                record.summary,
                record.keywords,
                record.embedding,
                record.embedding_dim,
                record.index_state,
                record.last_error,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Get a file record by its normalized path.
    pub fn get_file_by_path(&self, file_path: &str) -> Result<Option<FileRecord>, AppError> {
        let result = self
            .conn
            .query_row(
                "SELECT id, file_path, file_name, file_ext, file_size, file_hash,
                        modified_at, indexed_at, summary, keywords, embedding,
                        embedding_dim, index_state, last_error
                 FROM files WHERE file_path = ?1",
                params![file_path],
                |row| {
                    Ok(FileRecord {
                        id: row.get(0)?,
                        file_path: row.get(1)?,
                        file_name: row.get(2)?,
                        file_ext: row.get(3)?,
                        file_size: row.get(4)?,
                        file_hash: row.get(5)?,
                        modified_at: row.get(6)?,
                        indexed_at: row.get(7)?,
                        summary: row.get(8)?,
                        keywords: row.get(9)?,
                        embedding: row.get(10)?,
                        embedding_dim: row.get(11)?,
                        index_state: row.get(12)?,
                        last_error: row.get(13)?,
                    })
                },
            )
            .optional()?;
        Ok(result)
    }

    /// Delete a file record by path. Returns true if a row was deleted.
    pub fn delete_file(&self, file_path: &str) -> Result<bool, AppError> {
        let deleted = self
            .conn
            .execute("DELETE FROM files WHERE file_path = ?1", params![file_path])?;
        Ok(deleted > 0)
    }

    /// Get all file embeddings for vector search.
    /// Returns (id, file_path, embedding bytes, embedding_dim).
    pub fn get_all_embeddings(&self) -> Result<Vec<EmbeddingRow>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, embedding, embedding_dim FROM files WHERE index_state = 'indexed'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i32>(3)?,
            ))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get files with a specific index_state (e.g., 'pending_tantivy').
    pub fn get_files_by_state(&self, state: &str) -> Result<Vec<FileRecord>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, file_name, file_ext, file_size, file_hash,
                    modified_at, indexed_at, summary, keywords, embedding,
                    embedding_dim, index_state, last_error
             FROM files WHERE index_state = ?1",
        )?;
        let rows = stmt.query_map(params![state], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                file_path: row.get(1)?,
                file_name: row.get(2)?,
                file_ext: row.get(3)?,
                file_size: row.get(4)?,
                file_hash: row.get(5)?,
                modified_at: row.get(6)?,
                indexed_at: row.get(7)?,
                summary: row.get(8)?,
                keywords: row.get(9)?,
                embedding: row.get(10)?,
                embedding_dim: row.get(11)?,
                index_state: row.get(12)?,
                last_error: row.get(13)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Update the index_state of a file by id.
    pub fn update_index_state(&self, id: i64, state: &str) -> Result<(), AppError> {
        self.conn.execute(
            "UPDATE files SET index_state = ?1 WHERE id = ?2",
            params![state, id],
        )?;
        Ok(())
    }

    // ---- filter queries ----

    /// Get IDs of indexed files matching the given search filters.
    ///
    /// Builds a dynamic SQL query with WHERE clauses for extension, date range,
    /// and directory prefix filters. Returns a set of matching file IDs that can
    /// be used to pre-filter embeddings for vector search.
    pub fn get_filtered_db_ids(&self, filters: &SearchFilters) -> Result<HashSet<i64>, AppError> {
        let mut sql = String::from("SELECT id FROM files WHERE index_state = 'indexed'");
        let mut bind_values: Vec<rusqlite::types::Value> = Vec::new();

        // Extension filter: DB stores with dot prefix (e.g., ".pdf"), SearchFilters without
        if !filters.file_extensions.is_empty() {
            let placeholders: Vec<String> = (0..filters.file_extensions.len())
                .map(|i| format!("?{}", bind_values.len() + i + 1))
                .collect();
            sql.push_str(&format!(" AND file_ext IN ({})", placeholders.join(", ")));
            for ext in &filters.file_extensions {
                bind_values.push(rusqlite::types::Value::Text(format!(".{ext}")));
            }
        }

        // Date range filter: convert stored ISO text to Unix timestamp for comparison
        if let Some(after) = filters.date_after {
            bind_values.push(rusqlite::types::Value::Integer(after));
            sql.push_str(&format!(
                " AND CAST(strftime('%s', modified_at) AS INTEGER) >= ?{}",
                bind_values.len()
            ));
        }
        if let Some(before) = filters.date_before {
            bind_values.push(rusqlite::types::Value::Integer(before));
            sql.push_str(&format!(
                " AND CAST(strftime('%s', modified_at) AS INTEGER) <= ?{}",
                bind_values.len()
            ));
        }

        // Directory prefix filter: OR across multiple directory prefixes
        if !filters.directories.is_empty() {
            let dir_clauses: Vec<String> = (0..filters.directories.len())
                .map(|i| format!("file_path LIKE ?{}", bind_values.len() + i + 1))
                .collect();
            sql.push_str(&format!(" AND ({})", dir_clauses.join(" OR ")));
            for dir in &filters.directories {
                bind_values.push(rusqlite::types::Value::Text(format!("{dir}%")));
            }
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = bind_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| row.get::<_, i64>(0))?;

        let mut ids = HashSet::new();
        for row in rows {
            ids.insert(row?);
        }
        Ok(ids)
    }

    // ---- pending_files CRUD ----

    /// Enqueue a pending file for processing.
    pub fn enqueue_pending(&self, pending: &PendingFile) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO pending_files (file_path, reason, enqueued_at, retry_count, next_retry_at, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(file_path) DO UPDATE SET
                reason=excluded.reason, enqueued_at=excluded.enqueued_at,
                retry_count=excluded.retry_count, next_retry_at=excluded.next_retry_at,
                last_error=excluded.last_error",
            params![
                pending.file_path,
                pending.reason,
                pending.enqueued_at,
                pending.retry_count,
                pending.next_retry_at,
                pending.last_error,
            ],
        )?;
        Ok(())
    }

    /// Remove a pending file from the queue.
    pub fn dequeue_pending(&self, file_path: &str) -> Result<bool, AppError> {
        let deleted = self.conn.execute(
            "DELETE FROM pending_files WHERE file_path = ?1",
            params![file_path],
        )?;
        Ok(deleted > 0)
    }

    /// Get all pending files, ordered by next_retry_at.
    pub fn get_pending_files(&self) -> Result<Vec<PendingFile>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, reason, enqueued_at, retry_count, next_retry_at, last_error
             FROM pending_files ORDER BY next_retry_at ASC NULLS FIRST",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PendingFile {
                file_path: row.get(0)?,
                reason: row.get(1)?,
                enqueued_at: row.get(2)?,
                retry_count: row.get(3)?,
                next_retry_at: row.get(4)?,
                last_error: row.get(5)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get total count of indexed files.
    pub fn get_indexed_count(&self) -> Result<usize, AppError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE index_state = 'indexed'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Get file count grouped by extension.
    pub fn get_count_by_extension(&self) -> Result<Vec<(String, usize)>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT file_ext, COUNT(*) FROM files WHERE index_state = 'indexed' GROUP BY file_ext",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get total size of all indexed files.
    pub fn get_total_size(&self) -> Result<u64, AppError> {
        let size: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(file_size), 0) FROM files WHERE index_state = 'indexed'",
            [],
            |row| row.get(0),
        )?;
        Ok(size as u64)
    }
}

/// Convert f32 slice to little-endian bytes for BLOB storage.
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Convert little-endian BLOB bytes back to f32 slice.
pub fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_record(path: &str) -> FileRecord {
        let embedding = embedding_to_bytes(&[0.1, 0.2, 0.3]);
        FileRecord {
            id: 0,
            file_path: path.to_string(),
            file_name: "test.pdf".to_string(),
            file_ext: ".pdf".to_string(),
            file_size: 1024,
            file_hash: "abc123".to_string(),
            modified_at: "2024-01-01T00:00:00Z".to_string(),
            indexed_at: "2024-01-01T00:00:00Z".to_string(),
            summary: "Test summary".to_string(),
            keywords: r#"["test","keyword"]"#.to_string(),
            embedding,
            embedding_dim: 3,
            index_state: "indexed".to_string(),
            last_error: None,
        }
    }

    #[test]
    fn test_open_in_memory_succeeds() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.get_indexed_count().unwrap(), 0);
    }

    #[test]
    fn test_upsert_and_get_file() {
        let db = Database::open_in_memory().unwrap();
        let record = make_test_record("/test/file.pdf");

        db.upsert_file(&record).unwrap();

        let loaded = db.get_file_by_path("/test/file.pdf").unwrap().unwrap();
        assert_eq!(loaded.file_name, "test.pdf");
        assert_eq!(loaded.file_ext, ".pdf");
        assert_eq!(loaded.file_size, 1024);
        assert_eq!(loaded.summary, "Test summary");
    }

    #[test]
    fn test_upsert_updates_existing_record() {
        let db = Database::open_in_memory().unwrap();
        let mut record = make_test_record("/test/file.pdf");
        db.upsert_file(&record).unwrap();

        record.summary = "Updated summary".to_string();
        db.upsert_file(&record).unwrap();

        let loaded = db.get_file_by_path("/test/file.pdf").unwrap().unwrap();
        assert_eq!(loaded.summary, "Updated summary");
        assert_eq!(db.get_indexed_count().unwrap(), 1);
    }

    #[test]
    fn test_get_file_by_path_returns_none_for_missing() {
        let db = Database::open_in_memory().unwrap();
        let result = db.get_file_by_path("/nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_file() {
        let db = Database::open_in_memory().unwrap();
        let record = make_test_record("/test/file.pdf");
        db.upsert_file(&record).unwrap();

        assert!(db.delete_file("/test/file.pdf").unwrap());
        assert!(db.get_file_by_path("/test/file.pdf").unwrap().is_none());
    }

    #[test]
    fn test_delete_file_returns_false_for_missing() {
        let db = Database::open_in_memory().unwrap();
        assert!(!db.delete_file("/nonexistent").unwrap());
    }

    #[test]
    fn test_embedding_roundtrip() {
        let original = vec![1.0_f32, -0.5, 0.0, 0.12345];
        let bytes = embedding_to_bytes(&original);
        let restored = bytes_to_embedding(&bytes);
        assert_eq!(original.len(), restored.len());
        for (a, b) in original.iter().zip(restored.iter()) {
            assert!((a - b).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_get_all_embeddings() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_file(&make_test_record("/a.pdf")).unwrap();
        db.upsert_file(&make_test_record("/b.pdf")).unwrap();

        let embeddings = db.get_all_embeddings().unwrap();
        assert_eq!(embeddings.len(), 2);
    }

    #[test]
    fn test_get_files_by_state() {
        let db = Database::open_in_memory().unwrap();
        let mut record = make_test_record("/a.pdf");
        record.index_state = "pending_tantivy".to_string();
        db.upsert_file(&record).unwrap();

        db.upsert_file(&make_test_record("/b.pdf")).unwrap();

        let pending = db.get_files_by_state("pending_tantivy").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].file_path, "/a.pdf");

        let indexed = db.get_files_by_state("indexed").unwrap();
        assert_eq!(indexed.len(), 1);
    }

    #[test]
    fn test_update_index_state() {
        let db = Database::open_in_memory().unwrap();
        let record = make_test_record("/a.pdf");
        db.upsert_file(&record).unwrap();
        let loaded = db.get_file_by_path("/a.pdf").unwrap().unwrap();

        db.update_index_state(loaded.id, "pending_tantivy").unwrap();

        let updated = db.get_file_by_path("/a.pdf").unwrap().unwrap();
        assert_eq!(updated.index_state, "pending_tantivy");
    }

    #[test]
    fn test_enqueue_and_get_pending_files() {
        let db = Database::open_in_memory().unwrap();
        let pending = PendingFile {
            file_path: "/test/pending.pdf".to_string(),
            reason: "startup_resume".to_string(),
            enqueued_at: "2024-01-01T00:00:00Z".to_string(),
            retry_count: 0,
            next_retry_at: None,
            last_error: None,
        };
        db.enqueue_pending(&pending).unwrap();

        let all = db.get_pending_files().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].file_path, "/test/pending.pdf");
        assert_eq!(all[0].reason, "startup_resume");
    }

    #[test]
    fn test_dequeue_pending() {
        let db = Database::open_in_memory().unwrap();
        let pending = PendingFile {
            file_path: "/test/pending.pdf".to_string(),
            reason: "retry".to_string(),
            enqueued_at: "2024-01-01T00:00:00Z".to_string(),
            retry_count: 1,
            next_retry_at: Some("2024-01-01T00:01:00Z".to_string()),
            last_error: Some("timeout".to_string()),
        };
        db.enqueue_pending(&pending).unwrap();

        assert!(db.dequeue_pending("/test/pending.pdf").unwrap());
        assert_eq!(db.get_pending_files().unwrap().len(), 0);
    }

    #[test]
    fn test_dequeue_pending_returns_false_for_missing() {
        let db = Database::open_in_memory().unwrap();
        assert!(!db.dequeue_pending("/nonexistent").unwrap());
    }

    #[test]
    fn test_enqueue_pending_upsert_on_conflict() {
        let db = Database::open_in_memory().unwrap();
        let pending1 = PendingFile {
            file_path: "/test/file.pdf".to_string(),
            reason: "startup_resume".to_string(),
            enqueued_at: "2024-01-01T00:00:00Z".to_string(),
            retry_count: 0,
            next_retry_at: None,
            last_error: None,
        };
        db.enqueue_pending(&pending1).unwrap();

        let pending2 = PendingFile {
            file_path: "/test/file.pdf".to_string(),
            reason: "retry".to_string(),
            enqueued_at: "2024-01-01T00:01:00Z".to_string(),
            retry_count: 1,
            next_retry_at: Some("2024-01-01T00:02:00Z".to_string()),
            last_error: Some("rate limit".to_string()),
        };
        db.enqueue_pending(&pending2).unwrap();

        let all = db.get_pending_files().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].reason, "retry");
        assert_eq!(all[0].retry_count, 1);
    }

    #[test]
    fn test_get_count_by_extension() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_file(&make_test_record("/a.pdf")).unwrap();
        let mut docx_record = make_test_record("/b.docx");
        docx_record.file_ext = ".docx".to_string();
        db.upsert_file(&docx_record).unwrap();

        let counts = db.get_count_by_extension().unwrap();
        assert_eq!(counts.len(), 2);
    }

    #[test]
    fn test_get_total_size() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_file(&make_test_record("/a.pdf")).unwrap();
        db.upsert_file(&make_test_record("/b.pdf")).unwrap();

        let total = db.get_total_size().unwrap();
        assert_eq!(total, 2048); // 2 files × 1024 bytes
    }

    #[test]
    fn test_embedding_to_bytes_empty() {
        let bytes = embedding_to_bytes(&[]);
        assert!(bytes.is_empty());
        let restored = bytes_to_embedding(&bytes);
        assert!(restored.is_empty());
    }

    // ---- get_filtered_db_ids tests ----

    fn insert_filter_test_records(db: &Database) {
        let embedding = embedding_to_bytes(&[0.1, 0.2, 0.3]);

        // PDF in /docs/reports, modified 2024-03-01
        db.upsert_file(&FileRecord {
            id: 0,
            file_path: "/docs/reports/revenue.pdf".to_string(),
            file_name: "revenue.pdf".to_string(),
            file_ext: ".pdf".to_string(),
            file_size: 1024,
            file_hash: "hash1".to_string(),
            modified_at: "2024-03-01T00:00:00Z".to_string(),
            indexed_at: "2024-03-01T00:00:00Z".to_string(),
            summary: "Revenue report".to_string(),
            keywords: "revenue".to_string(),
            embedding: embedding.clone(),
            embedding_dim: 3,
            index_state: "indexed".to_string(),
            last_error: None,
        })
        .unwrap();

        // DOCX in /docs/contracts, modified 2024-07-15
        db.upsert_file(&FileRecord {
            id: 0,
            file_path: "/docs/contracts/employment.docx".to_string(),
            file_name: "employment.docx".to_string(),
            file_ext: ".docx".to_string(),
            file_size: 2048,
            file_hash: "hash2".to_string(),
            modified_at: "2024-07-15T00:00:00Z".to_string(),
            indexed_at: "2024-07-15T00:00:00Z".to_string(),
            summary: "Employment contract".to_string(),
            keywords: "contract".to_string(),
            embedding: embedding.clone(),
            embedding_dim: 3,
            index_state: "indexed".to_string(),
            last_error: None,
        })
        .unwrap();

        // TXT in /docs/reports, modified 2023-11-01
        db.upsert_file(&FileRecord {
            id: 0,
            file_path: "/docs/reports/notes.txt".to_string(),
            file_name: "notes.txt".to_string(),
            file_ext: ".txt".to_string(),
            file_size: 512,
            file_hash: "hash3".to_string(),
            modified_at: "2023-11-01T00:00:00Z".to_string(),
            indexed_at: "2023-11-01T00:00:00Z".to_string(),
            summary: "Meeting notes".to_string(),
            keywords: "notes".to_string(),
            embedding,
            embedding_dim: 3,
            index_state: "indexed".to_string(),
            last_error: None,
        })
        .unwrap();
    }

    #[test]
    fn test_get_filtered_db_ids_by_extension() {
        let db = Database::open_in_memory().unwrap();
        insert_filter_test_records(&db);

        let filters = SearchFilters {
            file_extensions: vec!["pdf".to_string()],
            ..Default::default()
        };
        let ids = db.get_filtered_db_ids(&filters).unwrap();
        assert_eq!(ids.len(), 1);

        // Multiple extensions
        let filters = SearchFilters {
            file_extensions: vec!["pdf".to_string(), "docx".to_string()],
            ..Default::default()
        };
        let ids = db.get_filtered_db_ids(&filters).unwrap();
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_get_filtered_db_ids_by_date_range() {
        let db = Database::open_in_memory().unwrap();
        insert_filter_test_records(&db);

        // After 2024-01-01 (1704067200)
        let filters = SearchFilters {
            date_after: Some(1704067200),
            ..Default::default()
        };
        let ids = db.get_filtered_db_ids(&filters).unwrap();
        assert_eq!(ids.len(), 2); // revenue.pdf (2024-03) + employment.docx (2024-07)

        // Before 2024-06-01 (1717200000)
        let filters = SearchFilters {
            date_before: Some(1717200000),
            ..Default::default()
        };
        let ids = db.get_filtered_db_ids(&filters).unwrap();
        assert_eq!(ids.len(), 2); // revenue.pdf (2024-03) + notes.txt (2023-11)
    }

    #[test]
    fn test_get_filtered_db_ids_combined() {
        let db = Database::open_in_memory().unwrap();
        insert_filter_test_records(&db);

        // PDF files modified after 2024-01-01 in /docs/reports
        let filters = SearchFilters {
            file_extensions: vec!["pdf".to_string()],
            date_after: Some(1704067200),
            directories: vec!["/docs/reports".to_string()],
            ..Default::default()
        };
        let ids = db.get_filtered_db_ids(&filters).unwrap();
        assert_eq!(ids.len(), 1); // only revenue.pdf
    }

    #[test]
    fn test_get_filtered_db_ids_no_filters_returns_all() {
        let db = Database::open_in_memory().unwrap();
        insert_filter_test_records(&db);

        let ids = db.get_filtered_db_ids(&SearchFilters::default()).unwrap();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn test_get_filtered_db_ids_directory_filter() {
        let db = Database::open_in_memory().unwrap();
        insert_filter_test_records(&db);

        let filters = SearchFilters {
            directories: vec!["/docs/contracts".to_string()],
            ..Default::default()
        };
        let ids = db.get_filtered_db_ids(&filters).unwrap();
        assert_eq!(ids.len(), 1);
    }
}
