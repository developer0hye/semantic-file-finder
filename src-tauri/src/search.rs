use std::collections::HashMap;

use rayon::prelude::*;

use crate::db::bytes_to_embedding;
use crate::error::AppError;
use crate::tantivy_index::{SearchFilters, TantivyIndex};
use crate::vector_search::{VectorSearchResult, vector_search};

/// Search mode determines which search strategies are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Both keyword (Tantivy BM25) and vector (cosine similarity) search.
    Hybrid,
    /// Keyword search only (Tantivy BM25). Used as offline fallback.
    KeywordOnly,
    /// Vector search only (cosine similarity).
    VectorOnly,
}

/// Configuration parameters for a hybrid search query.
pub struct SearchParams<'a> {
    pub mode: SearchMode,
    pub alpha: f32,
    pub limit: usize,
    pub filters: &'a SearchFilters,
}

/// A merged search result with scores from both search strategies.
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    pub db_id: i64,
    pub file_path: String,
    pub file_name: String,
    pub summary: String,
    pub keywords: String,
    pub keyword_score: f32,
    pub vector_score: f32,
    pub final_score: f32,
}

/// Normalize BM25 scores to [0, 1] range using min-max normalization.
///
/// If all scores are identical, returns 1.0 for all entries.
fn normalize_scores(scores: &[f32]) -> Vec<f32> {
    if scores.is_empty() {
        return vec![];
    }

    let min = scores.iter().cloned().reduce(f32::min).unwrap_or(0.0);
    let max = scores.iter().cloned().reduce(f32::max).unwrap_or(0.0);
    let range = max - min;

    if range == 0.0 {
        // All scores identical — assign 1.0 so they don't vanish
        return vec![1.0; scores.len()];
    }

    scores.iter().map(|s| (s - min) / range).collect()
}

/// Execute a hybrid search combining keyword and vector results.
///
/// - `tantivy_index`: the full-text search index
/// - `query_text`: the user's search query
/// - `query_embedding`: the embedded query vector (None triggers KeywordOnly fallback)
/// - `document_embeddings`: all indexed embeddings as `(db_id, file_path, embedding_bytes, embedding_dim)`.
///   The caller should pre-filter these when search filters are active.
/// - `params`: search configuration (mode, alpha, limit, filters)
pub fn hybrid_search(
    tantivy_index: &TantivyIndex,
    query_text: &str,
    query_embedding: Option<&[f32]>,
    document_embeddings: &[(i64, String, Vec<u8>, i32)],
    params: &SearchParams<'_>,
) -> Result<Vec<HybridSearchResult>, AppError> {
    let SearchParams {
        mode,
        alpha,
        limit,
        filters,
    } = params;
    let mode = *mode;
    let alpha = *alpha;
    let limit = *limit;

    // Determine effective mode: degrade to KeywordOnly if embedding is missing
    let effective_mode = match mode {
        SearchMode::Hybrid if query_embedding.is_none() => SearchMode::KeywordOnly,
        SearchMode::VectorOnly if query_embedding.is_none() => {
            return Err(AppError::Internal(
                "VectorOnly search requires a query embedding".into(),
            ));
        }
        other => other,
    };

    // Collect keyword results (with filters applied at the Tantivy level)
    let keyword_results = if effective_mode != SearchMode::VectorOnly {
        tantivy_index.search_with_filters(query_text, limit * 2, filters)?
    } else {
        vec![]
    };

    // Collect vector results, post-filtering by directory prefix
    let vector_results: Vec<VectorSearchResult> = if effective_mode != SearchMode::KeywordOnly {
        if let Some(qe) = query_embedding {
            let decoded: Vec<(i64, String, Vec<f32>)> = document_embeddings
                .par_iter()
                .map(|(id, path, bytes, _dim)| (*id, path.clone(), bytes_to_embedding(bytes)))
                .collect();
            let mut vr = vector_search(qe, &decoded, limit * 2);
            // Post-filter vector results by directory prefix (Tantivy handles this
            // for keyword results, but vector search bypasses Tantivy)
            if !filters.directories.is_empty() {
                vr.retain(|r| {
                    filters
                        .directories
                        .iter()
                        .any(|dir| r.file_path.starts_with(dir))
                });
            }
            vr
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    // Merge results into a unified map keyed by db_id
    let mut result_map: HashMap<i64, HybridSearchResult> = HashMap::new();

    // Normalize keyword scores
    let raw_keyword_scores: Vec<f32> = keyword_results.iter().map(|r| r.score).collect();
    let normalized_keyword_scores = normalize_scores(&raw_keyword_scores);

    for (i, kr) in keyword_results.iter().enumerate() {
        let db_id = kr.db_id as i64;
        let entry = result_map
            .entry(db_id)
            .or_insert_with(|| HybridSearchResult {
                db_id,
                file_path: kr.file_path.clone(),
                file_name: kr.file_name.clone(),
                summary: kr.summary.clone(),
                keywords: kr.keywords.clone(),
                keyword_score: 0.0,
                vector_score: 0.0,
                final_score: 0.0,
            });
        entry.keyword_score = normalized_keyword_scores[i];
    }

    for vr in &vector_results {
        let entry = result_map
            .entry(vr.db_id)
            .or_insert_with(|| HybridSearchResult {
                db_id: vr.db_id,
                file_path: vr.file_path.clone(),
                file_name: String::new(),
                summary: String::new(),
                keywords: String::new(),
                keyword_score: 0.0,
                vector_score: 0.0,
                final_score: 0.0,
            });
        entry.vector_score = vr.score;
    }

    // Compute final scores
    let alpha = alpha.clamp(0.0, 1.0);
    for result in result_map.values_mut() {
        result.final_score = match effective_mode {
            SearchMode::Hybrid => {
                alpha * result.keyword_score + (1.0 - alpha) * result.vector_score
            }
            SearchMode::KeywordOnly => result.keyword_score,
            SearchMode::VectorOnly => result.vector_score,
        };
    }

    // Sort by final_score descending and truncate
    let mut results: Vec<HybridSearchResult> = result_map.into_values().collect();
    results.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::embedding_to_bytes;
    use crate::tantivy_index::{DocumentData, SearchFilters, TantivyIndex};

    fn default_params(mode: SearchMode, alpha: f32, limit: usize) -> SearchParams<'static> {
        static DEFAULT_FILTERS: SearchFilters = SearchFilters {
            file_extensions: Vec::new(),
            date_after: None,
            date_before: None,
            directories: Vec::new(),
        };
        SearchParams {
            mode,
            alpha,
            limit,
            filters: &DEFAULT_FILTERS,
        }
    }

    fn create_test_index_with_docs() -> TantivyIndex {
        let mut idx = TantivyIndex::open_in_ram().expect("Failed to create in-RAM index");

        idx.upsert_document(&DocumentData {
            db_id: 1,
            file_path: "/docs/revenue.docx",
            file_name: "revenue.docx",
            file_ext: "docx",
            summary: "Quarterly revenue report for Q3 2024 with detailed financial analysis",
            keywords: "revenue, quarterly, finance, Q3, 2024",
            file_size: 1024,
            modified_at_unix: 1700000000,
        })
        .unwrap();

        idx.upsert_document(&DocumentData {
            db_id: 2,
            file_path: "/docs/contract.pdf",
            file_name: "contract.pdf",
            file_ext: "pdf",
            summary: "Employment contract for new hire starting January 2025",
            keywords: "contract, employment, hire, January",
            file_size: 2048,
            modified_at_unix: 1700100000,
        })
        .unwrap();

        idx.upsert_document(&DocumentData {
            db_id: 3,
            file_path: "/docs/budget.xlsx",
            file_name: "budget.xlsx",
            file_ext: "xlsx",
            summary: "Annual budget planning spreadsheet for fiscal year 2025",
            keywords: "budget, planning, fiscal, annual",
            file_size: 4096,
            modified_at_unix: 1700200000,
        })
        .unwrap();

        idx
    }

    fn make_test_embeddings() -> Vec<(i64, String, Vec<u8>, i32)> {
        // Create embeddings that simulate semantic meaning
        // doc 1 (revenue) — close to "financial report" query
        let emb1: Vec<f32> = vec![0.9, 0.1, 0.0];
        // doc 2 (contract) — close to "employment" query
        let emb2: Vec<f32> = vec![0.1, 0.9, 0.0];
        // doc 3 (budget) — medium similarity to "financial report"
        let emb3: Vec<f32> = vec![0.6, 0.3, 0.1];

        vec![
            (1, "/docs/revenue.docx".into(), embedding_to_bytes(&emb1), 3),
            (2, "/docs/contract.pdf".into(), embedding_to_bytes(&emb2), 3),
            (3, "/docs/budget.xlsx".into(), embedding_to_bytes(&emb3), 3),
        ]
    }

    #[test]
    fn test_normalize_scores_basic() {
        let scores = vec![1.0, 3.0, 5.0];
        let normalized = normalize_scores(&scores);
        assert!((normalized[0] - 0.0).abs() < 1e-6);
        assert!((normalized[1] - 0.5).abs() < 1e-6);
        assert!((normalized[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_normalize_scores_identical() {
        let scores = vec![2.0, 2.0, 2.0];
        let normalized = normalize_scores(&scores);
        assert!(normalized.iter().all(|&s| (s - 1.0).abs() < 1e-6));
    }

    #[test]
    fn test_normalize_scores_empty() {
        let scores: Vec<f32> = vec![];
        let normalized = normalize_scores(&scores);
        assert!(normalized.is_empty());
    }

    #[test]
    fn test_normalize_scores_single() {
        let scores = vec![5.0];
        let normalized = normalize_scores(&scores);
        assert_eq!(normalized.len(), 1);
        assert!((normalized[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_keyword_only_search() {
        let idx = create_test_index_with_docs();
        let embeddings = make_test_embeddings();

        let results = hybrid_search(
            &idx,
            "revenue report",
            None, // no embedding
            &embeddings,
            &default_params(SearchMode::KeywordOnly, 0.4, 10),
        )
        .unwrap();

        assert!(!results.is_empty());
        // Revenue doc should rank first
        assert_eq!(results[0].db_id, 1);
        // All vector scores should be 0.0 in KeywordOnly mode
        assert!(results.iter().all(|r| r.vector_score == 0.0));
    }

    #[test]
    fn test_vector_only_search() {
        let idx = create_test_index_with_docs();
        let embeddings = make_test_embeddings();

        // Query embedding close to "financial report" (doc 1)
        let query_emb: Vec<f32> = vec![0.85, 0.15, 0.0];

        let results = hybrid_search(
            &idx,
            "",
            Some(&query_emb),
            &embeddings,
            &default_params(SearchMode::VectorOnly, 0.4, 10),
        )
        .unwrap();

        assert_eq!(results.len(), 3);
        // Doc 1 should rank first (highest cosine similarity)
        assert_eq!(results[0].db_id, 1);
        // All keyword scores should be 0.0 in VectorOnly mode
        assert!(results.iter().all(|r| r.keyword_score == 0.0));
    }

    #[test]
    fn test_hybrid_search_combines_scores() {
        let idx = create_test_index_with_docs();
        let embeddings = make_test_embeddings();

        // Query embedding close to "financial report" (doc 1)
        let query_emb: Vec<f32> = vec![0.85, 0.15, 0.0];

        let results = hybrid_search(
            &idx,
            "revenue report",
            Some(&query_emb),
            &embeddings,
            &default_params(SearchMode::Hybrid, 0.4, 10),
        )
        .unwrap();

        assert!(!results.is_empty());
        // Doc 1 should rank first — it matches both keyword and vector
        assert_eq!(results[0].db_id, 1);
        // Final score should be a combination
        let top = &results[0];
        let expected = 0.4 * top.keyword_score + 0.6 * top.vector_score;
        assert!(
            (top.final_score - expected).abs() < 1e-6,
            "Expected {expected}, got {}",
            top.final_score
        );
    }

    #[test]
    fn test_hybrid_degrades_to_keyword_only_when_no_embedding() {
        let idx = create_test_index_with_docs();
        let embeddings = make_test_embeddings();

        // Hybrid mode but no query embedding — should degrade to KeywordOnly
        let results = hybrid_search(
            &idx,
            "budget planning",
            None,
            &embeddings,
            &default_params(SearchMode::Hybrid, 0.4, 10),
        )
        .unwrap();

        assert!(!results.is_empty());
        // All vector scores should be 0.0 (degraded to keyword only)
        assert!(results.iter().all(|r| r.vector_score == 0.0));
    }

    #[test]
    fn test_vector_only_without_embedding_returns_error() {
        let idx = create_test_index_with_docs();
        let embeddings = make_test_embeddings();

        let result = hybrid_search(
            &idx,
            "query",
            None, // no embedding
            &embeddings,
            &default_params(SearchMode::VectorOnly, 0.4, 10),
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_hybrid_search_alpha_zero_is_vector_only_behavior() {
        let idx = create_test_index_with_docs();
        let embeddings = make_test_embeddings();
        let query_emb: Vec<f32> = vec![0.85, 0.15, 0.0];

        let results = hybrid_search(
            &idx,
            "revenue",
            Some(&query_emb),
            &embeddings,
            &default_params(SearchMode::Hybrid, 0.0, 10),
        )
        .unwrap();

        // With alpha=0, final_score == vector_score
        for r in &results {
            assert!(
                (r.final_score - r.vector_score).abs() < 1e-6,
                "With alpha=0, final_score should equal vector_score"
            );
        }
    }

    #[test]
    fn test_hybrid_search_alpha_one_is_keyword_only_behavior() {
        let idx = create_test_index_with_docs();
        let embeddings = make_test_embeddings();
        let query_emb: Vec<f32> = vec![0.85, 0.15, 0.0];

        let results = hybrid_search(
            &idx,
            "revenue",
            Some(&query_emb),
            &embeddings,
            &default_params(SearchMode::Hybrid, 1.0, 10),
        )
        .unwrap();

        // With alpha=1, final_score == keyword_score
        for r in &results {
            assert!(
                (r.final_score - r.keyword_score).abs() < 1e-6,
                "With alpha=1, final_score should equal keyword_score"
            );
        }
    }

    #[test]
    fn test_hybrid_search_respects_limit() {
        let idx = create_test_index_with_docs();
        let embeddings = make_test_embeddings();
        let query_emb: Vec<f32> = vec![0.5, 0.5, 0.0];

        let results = hybrid_search(
            &idx,
            "report",
            Some(&query_emb),
            &embeddings,
            &default_params(SearchMode::Hybrid, 0.4, 1),
        )
        .unwrap();

        assert!(results.len() <= 1);
    }

    #[test]
    fn test_keyword_only_search_empty_query() {
        let idx = create_test_index_with_docs();
        let embeddings = make_test_embeddings();

        let results = hybrid_search(
            &idx,
            "",
            None,
            &embeddings,
            &default_params(SearchMode::KeywordOnly, 0.4, 10),
        )
        .unwrap();

        // Empty query should return no keyword matches
        assert!(results.is_empty());
    }
}
