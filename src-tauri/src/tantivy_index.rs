use std::path::Path;

use lindera::dictionary::{DictionaryKind, load_embedded_dictionary};
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera_tantivy::tokenizer::LinderaTokenizer;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    DateOptions, Field, INDEXED, NumericOptions, STORED, STRING, Schema, TextFieldIndexing,
    TextOptions, Value,
};
use tantivy::{DateTime, Index, IndexReader, IndexWriter, TantivyDocument, Term};

use crate::error::AppError;

/// Name of the registered Korean tokenizer for text fields.
const KOREAN_TOKENIZER_NAME: &str = "korean";

/// Holds the Tantivy schema field handles for convenient access.
#[derive(Clone, Debug)]
pub struct SchemaFields {
    pub summary: Field,
    pub keywords: Field,
    pub file_name: Field,
    pub file_path: Field,
    pub file_ext: Field,
    pub modified_at: Field,
    pub file_size: Field,
    pub db_id: Field,
}

/// A single search result returned from Tantivy.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub db_id: u64,
    pub file_path: String,
    pub file_name: String,
    pub summary: String,
    pub keywords: String,
    pub score: f32,
}

/// Input data for upserting a document into the Tantivy index.
pub struct DocumentData<'a> {
    pub db_id: u64,
    pub file_path: &'a str,
    pub file_name: &'a str,
    pub file_ext: &'a str,
    pub summary: &'a str,
    pub keywords: &'a str,
    pub file_size: u64,
    pub modified_at_unix: i64,
}

/// Wrapper around a Tantivy index with Korean tokenizer support.
pub struct TantivyIndex {
    index: Index,
    reader: IndexReader,
    writer: IndexWriter,
    pub fields: SchemaFields,
}

/// Build the Tantivy schema with Korean tokenizer on text fields.
fn build_schema() -> (Schema, SchemaFields) {
    let mut schema_builder = Schema::builder();

    // Text fields use the Korean tokenizer for morphological analysis
    let korean_text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(KOREAN_TOKENIZER_NAME)
                .set_index_option(tantivy::schema::IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();

    let summary = schema_builder.add_text_field("summary", korean_text_options.clone());
    let keywords = schema_builder.add_text_field("keywords", korean_text_options.clone());
    let file_name = schema_builder.add_text_field("file_name", korean_text_options);
    let file_path = schema_builder.add_text_field("file_path", STRING | STORED);
    let file_ext = schema_builder.add_text_field("file_ext", STRING | STORED);
    let modified_at =
        schema_builder.add_date_field("modified_at", DateOptions::default() | INDEXED | STORED);
    let file_size =
        schema_builder.add_u64_field("file_size", NumericOptions::default() | INDEXED | STORED);
    let db_id = schema_builder.add_u64_field("db_id", INDEXED | STORED);

    let schema = schema_builder.build();
    let fields = SchemaFields {
        summary,
        keywords,
        file_name,
        file_path,
        file_ext,
        modified_at,
        file_size,
        db_id,
    };

    (schema, fields)
}

/// Register the Korean (ko-dic) tokenizer with the given index.
fn register_korean_tokenizer(index: &Index) -> Result<(), AppError> {
    let dictionary = load_embedded_dictionary(DictionaryKind::KoDic)
        .map_err(|e| AppError::SearchIndex(format!("Failed to load Korean dictionary: {e}")))?;
    let segmenter = Segmenter::new(Mode::Normal, dictionary, None);
    let tokenizer = LinderaTokenizer::from_segmenter(segmenter);
    index
        .tokenizers()
        .register(KOREAN_TOKENIZER_NAME, tokenizer);
    Ok(())
}

impl TantivyIndex {
    /// Open or create a Tantivy index at the given directory path.
    pub fn open(index_dir: &Path) -> Result<Self, AppError> {
        std::fs::create_dir_all(index_dir).map_err(|e| {
            AppError::SearchIndex(format!(
                "Failed to create index directory {}: {e}",
                index_dir.display()
            ))
        })?;

        let (schema, fields) = build_schema();

        let index = Index::open_or_create(
            tantivy::directory::MmapDirectory::open(index_dir).map_err(|e| {
                AppError::SearchIndex(format!("Failed to open index directory: {e}"))
            })?,
            schema,
        )
        .map_err(|e| AppError::SearchIndex(format!("Failed to open or create index: {e}")))?;

        register_korean_tokenizer(&index)?;

        let reader = index
            .reader()
            .map_err(|e| AppError::SearchIndex(format!("Failed to create index reader: {e}")))?;

        let writer = index
            .writer(50_000_000) // 50 MB writer heap
            .map_err(|e| AppError::SearchIndex(format!("Failed to create index writer: {e}")))?;

        Ok(Self {
            index,
            reader,
            writer,
            fields,
        })
    }

    /// Open or create a Tantivy index using an in-memory directory (for testing).
    pub fn open_in_ram() -> Result<Self, AppError> {
        let (schema, fields) = build_schema();

        let index = Index::create_in_ram(schema);

        register_korean_tokenizer(&index)?;

        let reader = index
            .reader()
            .map_err(|e| AppError::SearchIndex(format!("Failed to create index reader: {e}")))?;

        let writer = index
            .writer(15_000_000) // 15 MB for testing
            .map_err(|e| AppError::SearchIndex(format!("Failed to create index writer: {e}")))?;

        Ok(Self {
            index,
            reader,
            writer,
            fields,
        })
    }

    /// Insert or update a document in the index. Deletes any existing document
    /// with the same `db_id` before inserting.
    pub fn upsert_document(&mut self, data: &DocumentData<'_>) -> Result<(), AppError> {
        // Delete existing document with the same db_id
        self.writer
            .delete_term(Term::from_field_u64(self.fields.db_id, data.db_id));

        let mut doc = TantivyDocument::new();
        doc.add_u64(self.fields.db_id, data.db_id);
        doc.add_text(self.fields.file_path, data.file_path);
        doc.add_text(self.fields.file_name, data.file_name);
        doc.add_text(self.fields.file_ext, data.file_ext);
        doc.add_text(self.fields.summary, data.summary);
        doc.add_text(self.fields.keywords, data.keywords);
        doc.add_u64(self.fields.file_size, data.file_size);
        doc.add_date(
            self.fields.modified_at,
            DateTime::from_timestamp_secs(data.modified_at_unix),
        );

        self.writer
            .add_document(doc)
            .map_err(|e| AppError::SearchIndex(format!("Failed to add document to index: {e}")))?;

        self.writer
            .commit()
            .map_err(|e| AppError::SearchIndex(format!("Failed to commit index: {e}")))?;

        self.reader
            .reload()
            .map_err(|e| AppError::SearchIndex(format!("Failed to reload index reader: {e}")))?;

        Ok(())
    }

    /// Delete a document by its `db_id`.
    pub fn delete_document(&mut self, db_id: u64) -> Result<(), AppError> {
        self.writer
            .delete_term(Term::from_field_u64(self.fields.db_id, db_id));

        self.writer
            .commit()
            .map_err(|e| AppError::SearchIndex(format!("Failed to commit index: {e}")))?;

        self.reader
            .reload()
            .map_err(|e| AppError::SearchIndex(format!("Failed to reload index reader: {e}")))?;

        Ok(())
    }

    /// Search the index with a text query (BM25 ranking).
    /// Returns up to `limit` results ordered by relevance score.
    pub fn search(&self, query_text: &str, limit: usize) -> Result<Vec<SearchResult>, AppError> {
        let searcher = self.reader.searcher();

        let query_parser = QueryParser::for_index(
            &self.index,
            vec![
                self.fields.summary,
                self.fields.keywords,
                self.fields.file_name,
            ],
        );

        let query = query_parser
            .parse_query(query_text)
            .map_err(|e| AppError::SearchIndex(format!("Failed to parse query: {e}")))?;

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit))
            .map_err(|e| AppError::SearchIndex(format!("Search execution failed: {e}")))?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| AppError::SearchIndex(format!("Failed to retrieve document: {e}")))?;

            let db_id = doc
                .get_first(self.fields.db_id)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let file_path = doc
                .get_first(self.fields.file_path)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let file_name = doc
                .get_first(self.fields.file_name)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let summary = doc
                .get_first(self.fields.summary)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let keywords = doc
                .get_first(self.fields.keywords)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            results.push(SearchResult {
                db_id,
                file_path,
                file_name,
                summary,
                keywords,
                score,
            });
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_index() -> TantivyIndex {
        TantivyIndex::open_in_ram().expect("Failed to create in-RAM index")
    }

    #[test]
    fn test_open_in_ram() {
        let idx = create_test_index();
        // Verify schema fields exist
        assert_eq!(
            idx.index.schema().get_field("summary").unwrap(),
            idx.fields.summary
        );
        assert_eq!(
            idx.index.schema().get_field("db_id").unwrap(),
            idx.fields.db_id
        );
    }

    #[test]
    fn test_upsert_and_search_english() {
        let mut idx = create_test_index();

        idx.upsert_document(&DocumentData {
            db_id: 1,
            file_path: "/docs/report.docx",
            file_name: "report.docx",
            file_ext: "docx",
            summary: "Quarterly revenue report for Q3 2024 with financial analysis",
            keywords: "revenue, quarterly, finance, Q3",
            file_size: 1024,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        let results = idx.search("revenue report", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].db_id, 1);
        assert_eq!(results[0].file_path, "/docs/report.docx");
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn test_upsert_and_search_korean() {
        let mut idx = create_test_index();

        idx.upsert_document(&DocumentData {
            db_id: 2,
            file_path: "/docs/계약서.docx",
            file_name: "계약서.docx",
            file_ext: "docx",
            summary: "김철수 대리와의 업무 협약 계약서입니다",
            keywords: "계약서, 협약, 김철수",
            file_size: 2048,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        let results = idx.search("계약서", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].db_id, 2);
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn test_search_mixed_korean_english() {
        let mut idx = create_test_index();

        idx.upsert_document(&DocumentData {
            db_id: 3,
            file_path: "/docs/Q3_매출보고서.xlsx",
            file_name: "Q3_매출보고서.xlsx",
            file_ext: "xlsx",
            summary: "Q3 분기 매출 보고서 - 2024년 3분기 실적 분석",
            keywords: "매출, Q3, 분기, 실적",
            file_size: 4096,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        let results = idx.search("Q3 매출", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].db_id, 3);
    }

    #[test]
    fn test_upsert_replaces_existing() {
        let mut idx = create_test_index();

        idx.upsert_document(&DocumentData {
            db_id: 10,
            file_path: "/docs/old.txt",
            file_name: "old.txt",
            file_ext: "txt",
            summary: "Old content that should be replaced",
            keywords: "old",
            file_size: 100,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        idx.upsert_document(&DocumentData {
            db_id: 10,
            file_path: "/docs/updated.txt",
            file_name: "updated.txt",
            file_ext: "txt",
            summary: "Updated content with new information",
            keywords: "updated, new",
            file_size: 200,
            modified_at_unix: 1700100000,
        })
        .unwrap();

        // Searching for old content should not find it
        let results = idx.search("Old content replaced", 10).unwrap();
        let has_old = results.iter().any(|r| r.file_path == "/docs/old.txt");
        assert!(!has_old, "Old document should have been replaced");

        // Searching for new content should find it
        let results = idx.search("Updated new information", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].db_id, 10);
        assert_eq!(results[0].file_path, "/docs/updated.txt");
    }

    #[test]
    fn test_delete_document() {
        let mut idx = create_test_index();

        idx.upsert_document(&DocumentData {
            db_id: 20,
            file_path: "/docs/delete_me.txt",
            file_name: "delete_me.txt",
            file_ext: "txt",
            summary: "This document will be deleted soon",
            keywords: "delete, temporary",
            file_size: 50,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        // Verify it exists
        let results = idx.search("deleted soon", 10).unwrap();
        assert_eq!(results.len(), 1);

        // Delete it
        idx.delete_document(20).unwrap();

        // Verify it's gone
        let results = idx.search("deleted soon", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_empty_index() {
        let idx = create_test_index();
        let results = idx.search("anything", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_no_match() {
        let mut idx = create_test_index();

        idx.upsert_document(&DocumentData {
            db_id: 30,
            file_path: "/docs/test.txt",
            file_name: "test.txt",
            file_ext: "txt",
            summary: "A document about machine learning algorithms",
            keywords: "machine learning, AI",
            file_size: 300,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        let results = idx.search("basketball championship", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_multiple_documents_ranked() {
        let mut idx = create_test_index();

        idx.upsert_document(&DocumentData {
            db_id: 40,
            file_path: "/docs/a.txt",
            file_name: "a.txt",
            file_ext: "txt",
            summary: "Brief mention of budget",
            keywords: "budget",
            file_size: 100,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        idx.upsert_document(&DocumentData {
            db_id: 41,
            file_path: "/docs/b.txt",
            file_name: "b.txt",
            file_ext: "txt",
            summary: "Detailed budget analysis for the annual budget review of the budget committee",
            keywords: "budget, analysis, review, committee",
            file_size: 200,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        let results = idx.search("budget", 10).unwrap();
        assert_eq!(results.len(), 2);
        // The document with more occurrences of "budget" should rank higher
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn test_search_limit() {
        let mut idx = create_test_index();

        for i in 0..5u64 {
            let path = format!("/docs/doc_{i}.txt");
            let name = format!("doc_{i}.txt");
            idx.upsert_document(&DocumentData {
                db_id: 100 + i,
                file_path: &path,
                file_name: &name,
                file_ext: "txt",
                summary: "Common search term across all documents",
                keywords: "common",
                file_size: 100,
                modified_at_unix: 1700000000,
            })
            .unwrap();
        }

        let results = idx.search("common search term", 3).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_open_on_disk() {
        let temp_dir = std::env::temp_dir().join("tantivy_test_open_on_disk");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let mut idx = TantivyIndex::open(&temp_dir).expect("Failed to open on-disk index");

        idx.upsert_document(&DocumentData {
            db_id: 1,
            file_path: "/docs/test.txt",
            file_name: "test.txt",
            file_ext: "txt",
            summary: "Persistent document for disk test",
            keywords: "persistent, disk",
            file_size: 100,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        let results = idx.search("persistent disk", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].db_id, 1);

        // Clean up
        drop(idx);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
