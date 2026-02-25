use std::ops::Bound;
use std::path::Path;

use lindera::dictionary::{DictionaryKind, load_embedded_dictionary};
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera_tantivy::tokenizer::LinderaTokenizer;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::{
    DateOptions, Field, INDEXED, IndexRecordOption, NumericOptions, STORED, STRING, Schema,
    TextFieldIndexing, TextOptions, Value,
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

/// Filters to constrain search results.
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    /// Only include files with these extensions (e.g., ["pdf", "docx"]).
    /// Empty means no extension filter.
    pub file_extensions: Vec<String>,
    /// Only include files modified at or after this Unix timestamp.
    pub date_after: Option<i64>,
    /// Only include files modified at or before this Unix timestamp.
    pub date_before: Option<i64>,
    /// Only include files under these directory prefixes.
    /// Empty means no directory filter.
    pub directories: Vec<String>,
}

impl SearchFilters {
    /// Returns true if any filter is active.
    pub fn has_any_filter(&self) -> bool {
        !self.file_extensions.is_empty()
            || self.date_after.is_some()
            || self.date_before.is_some()
            || !self.directories.is_empty()
    }
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

    /// Search the index with a text query and optional filters.
    ///
    /// Combines the text query (BM25) with extension and date range filters using
    /// `BooleanQuery`. Directory prefixes are applied as a post-filter since Tantivy
    /// STRING fields don't support prefix matching.
    ///
    /// If `query_text` is empty but filters are present, uses `AllQuery` to return
    /// all documents matching the filters (browse mode).
    pub fn search_with_filters(
        &self,
        query_text: &str,
        limit: usize,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, AppError> {
        // No text and no filters → empty results
        if query_text.is_empty() && !filters.has_any_filter() {
            return Ok(vec![]);
        }

        let searcher = self.reader.searcher();
        let mut must_clauses: Vec<(tantivy::query::Occur, Box<dyn tantivy::query::Query>)> =
            Vec::new();

        // Text query or AllQuery for browse mode
        if !query_text.is_empty() {
            let query_parser = QueryParser::for_index(
                &self.index,
                vec![
                    self.fields.summary,
                    self.fields.keywords,
                    self.fields.file_name,
                ],
            );
            let text_query = query_parser
                .parse_query(query_text)
                .map_err(|e| AppError::SearchIndex(format!("Failed to parse query: {e}")))?;
            must_clauses.push((tantivy::query::Occur::Must, text_query));
        } else {
            must_clauses.push((
                tantivy::query::Occur::Must,
                Box::new(AllQuery) as Box<dyn tantivy::query::Query>,
            ));
        }

        // Extension filter: OR across extensions, wrapped in MUST
        if !filters.file_extensions.is_empty() {
            let ext_clauses: Vec<(tantivy::query::Occur, Box<dyn tantivy::query::Query>)> = filters
                .file_extensions
                .iter()
                .map(|ext| {
                    let term = Term::from_field_text(self.fields.file_ext, ext);
                    (
                        tantivy::query::Occur::Should,
                        Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                            as Box<dyn tantivy::query::Query>,
                    )
                })
                .collect();
            let ext_query = BooleanQuery::new(ext_clauses);
            must_clauses.push((
                tantivy::query::Occur::Must,
                Box::new(ext_query) as Box<dyn tantivy::query::Query>,
            ));
        }

        // Date range filter
        if filters.date_after.is_some() || filters.date_before.is_some() {
            let lower = match filters.date_after {
                Some(ts) => Bound::Included(Term::from_field_date(
                    self.fields.modified_at,
                    DateTime::from_timestamp_secs(ts),
                )),
                None => Bound::Unbounded,
            };
            let upper = match filters.date_before {
                Some(ts) => Bound::Included(Term::from_field_date(
                    self.fields.modified_at,
                    DateTime::from_timestamp_secs(ts),
                )),
                None => Bound::Unbounded,
            };
            let date_query = RangeQuery::new(lower, upper);
            must_clauses.push((
                tantivy::query::Occur::Must,
                Box::new(date_query) as Box<dyn tantivy::query::Query>,
            ));
        }

        let combined_query = BooleanQuery::new(must_clauses);

        // Fetch extra results to account for directory post-filtering
        let fetch_limit = if filters.directories.is_empty() {
            limit
        } else {
            limit * 4
        };

        let top_docs = searcher
            .search(&combined_query, &TopDocs::with_limit(fetch_limit))
            .map_err(|e| AppError::SearchIndex(format!("Search execution failed: {e}")))?;

        let mut results = Vec::with_capacity(top_docs.len().min(limit));
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| AppError::SearchIndex(format!("Failed to retrieve document: {e}")))?;

            let file_path = doc
                .get_first(self.fields.file_path)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Directory prefix post-filter
            if !filters.directories.is_empty()
                && !filters
                    .directories
                    .iter()
                    .any(|dir| file_path.starts_with(dir))
            {
                continue;
            }

            let db_id = doc
                .get_first(self.fields.db_id)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

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

            if results.len() >= limit {
                break;
            }
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

    // ---- search_with_filters tests ----

    fn create_filter_test_index() -> TantivyIndex {
        let mut idx = create_test_index();

        idx.upsert_document(&DocumentData {
            db_id: 1,
            file_path: "/docs/reports/revenue.pdf",
            file_name: "revenue.pdf",
            file_ext: "pdf",
            summary: "Quarterly revenue report for Q3 2024",
            keywords: "revenue, quarterly, finance",
            file_size: 1024,
            modified_at_unix: 1700000000, // 2023-11-14
        })
        .unwrap();

        idx.upsert_document(&DocumentData {
            db_id: 2,
            file_path: "/docs/contracts/employment.docx",
            file_name: "employment.docx",
            file_ext: "docx",
            summary: "Employment contract for new hire",
            keywords: "contract, employment, hire",
            file_size: 2048,
            modified_at_unix: 1710000000, // 2024-03-09
        })
        .unwrap();

        idx.upsert_document(&DocumentData {
            db_id: 3,
            file_path: "/docs/reports/budget.txt",
            file_name: "budget.txt",
            file_ext: "txt",
            summary: "Annual budget planning document",
            keywords: "budget, planning, annual",
            file_size: 512,
            modified_at_unix: 1720000000, // 2024-07-03
        })
        .unwrap();

        idx
    }

    #[test]
    fn test_search_filter_by_extension_single() {
        let idx = create_filter_test_index();

        let filters = SearchFilters {
            file_extensions: vec!["pdf".to_string()],
            ..Default::default()
        };

        let results = idx
            .search_with_filters("revenue report", 10, &filters)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].db_id, 1);
    }

    #[test]
    fn test_search_filter_by_extension_multiple() {
        let idx = create_filter_test_index();

        let filters = SearchFilters {
            file_extensions: vec!["pdf".to_string(), "docx".to_string()],
            ..Default::default()
        };

        // Browse mode (no text query) with extension filter
        let results = idx.search_with_filters("", 10, &filters).unwrap();
        assert_eq!(results.len(), 2);
        let ext_set: Vec<u64> = results.iter().map(|r| r.db_id).collect();
        assert!(ext_set.contains(&1)); // pdf
        assert!(ext_set.contains(&2)); // docx
    }

    #[test]
    fn test_search_filter_by_date_range() {
        let idx = create_filter_test_index();

        // Only files modified after 2024-01-01 (1704067200)
        let filters = SearchFilters {
            date_after: Some(1704067200),
            ..Default::default()
        };

        let results = idx.search_with_filters("", 10, &filters).unwrap();
        assert_eq!(results.len(), 2);
        let ids: Vec<u64> = results.iter().map(|r| r.db_id).collect();
        assert!(ids.contains(&2)); // 2024-03
        assert!(ids.contains(&3)); // 2024-07
    }

    #[test]
    fn test_search_filter_by_date_range_upper_bound() {
        let idx = create_filter_test_index();

        // Only files modified before 2024-06-01 (1717200000)
        let filters = SearchFilters {
            date_before: Some(1717200000),
            ..Default::default()
        };

        let results = idx.search_with_filters("", 10, &filters).unwrap();
        assert_eq!(results.len(), 2);
        let ids: Vec<u64> = results.iter().map(|r| r.db_id).collect();
        assert!(ids.contains(&1)); // 2023-11
        assert!(ids.contains(&2)); // 2024-03
    }

    #[test]
    fn test_search_filter_by_directory() {
        let idx = create_filter_test_index();

        let filters = SearchFilters {
            directories: vec!["/docs/reports".to_string()],
            ..Default::default()
        };

        let results = idx.search_with_filters("", 10, &filters).unwrap();
        assert_eq!(results.len(), 2);
        let ids: Vec<u64> = results.iter().map(|r| r.db_id).collect();
        assert!(ids.contains(&1)); // /docs/reports/revenue.pdf
        assert!(ids.contains(&3)); // /docs/reports/budget.txt
    }

    #[test]
    fn test_search_filter_combined_text_and_filters() {
        let idx = create_filter_test_index();

        let filters = SearchFilters {
            file_extensions: vec!["pdf".to_string(), "txt".to_string()],
            date_after: Some(1704067200), // after 2024-01-01
            directories: vec!["/docs/reports".to_string()],
            ..Default::default()
        };

        // Text + extension + date + directory
        let results = idx
            .search_with_filters("budget planning", 10, &filters)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].db_id, 3); // budget.txt, after 2024-01, in /docs/reports
    }

    #[test]
    fn test_search_filter_no_text_query_browse_mode() {
        let idx = create_filter_test_index();

        let filters = SearchFilters {
            file_extensions: vec!["txt".to_string()],
            ..Default::default()
        };

        let results = idx.search_with_filters("", 10, &filters).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].db_id, 3);
    }

    #[test]
    fn test_search_filter_no_text_no_filters_returns_empty() {
        let idx = create_filter_test_index();
        let results = idx
            .search_with_filters("", 10, &SearchFilters::default())
            .unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_filter_default_matches_unfiltered_search() {
        let idx = create_filter_test_index();

        let unfiltered = idx.search("revenue report", 10).unwrap();
        let with_default_filters = idx
            .search_with_filters("revenue report", 10, &SearchFilters::default())
            .unwrap();

        assert_eq!(unfiltered.len(), with_default_filters.len());
        for (a, b) in unfiltered.iter().zip(with_default_filters.iter()) {
            assert_eq!(a.db_id, b.db_id);
        }
    }
}
