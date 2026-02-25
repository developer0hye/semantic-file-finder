use rayon::prelude::*;

/// A single vector search result.
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub db_id: i64,
    pub file_path: String,
    pub score: f32,
}

/// Compute cosine similarity between two vectors.
///
/// Returns 0.0 if either vector is zero-length or all zeros.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;

    for (x, y) in a.iter().zip(b.iter()) {
        let x = f64::from(*x);
        let y = f64::from(*y);
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        return 0.0;
    }

    (dot / denom) as f32
}

/// Brute-force vector search over all document embeddings.
///
/// Each embedding entry is `(db_id, file_path, embedding_vector)`.
/// Returns up to `limit` results sorted by descending cosine similarity.
pub fn vector_search(
    query_embedding: &[f32],
    embeddings: &[(i64, String, Vec<f32>)],
    limit: usize,
) -> Vec<VectorSearchResult> {
    let mut scored: Vec<VectorSearchResult> = embeddings
        .par_iter()
        .map(|(db_id, file_path, emb)| VectorSearchResult {
            db_id: *db_id,
            file_path: file_path.clone(),
            score: cosine_similarity(query_embedding, emb),
        })
        .collect();

    // Sort by descending score
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6, "Expected ~1.0, got {sim}");
    }

    #[test]
    fn test_cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6, "Expected ~0.0, got {sim}");
    }

    #[test]
    fn test_cosine_similarity_opposite_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6, "Expected ~-1.0, got {sim}");
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_different_lengths() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_empty_vectors() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_high_dimensional() {
        // Two similar vectors in high dimensions
        let a: Vec<f32> = (0..1536).map(|i| (i as f32) * 0.001).collect();
        let b: Vec<f32> = (0..1536).map(|i| (i as f32) * 0.001 + 0.0001).collect();
        let sim = cosine_similarity(&a, &b);
        assert!(sim > 0.99, "Expected high similarity, got {sim}");
    }

    #[test]
    fn test_vector_search_returns_top_results() {
        let query = vec![1.0, 0.0, 0.0];
        let embeddings = vec![
            (1, "/a.txt".to_string(), vec![1.0, 0.0, 0.0]), // perfect match
            (2, "/b.txt".to_string(), vec![0.0, 1.0, 0.0]), // orthogonal
            (3, "/c.txt".to_string(), vec![0.7, 0.7, 0.0]), // partial match
        ];

        let results = vector_search(&query, &embeddings, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].db_id, 1); // highest similarity
        assert_eq!(results[1].db_id, 3); // second highest
    }

    #[test]
    fn test_vector_search_empty_embeddings() {
        let query = vec![1.0, 0.0];
        let embeddings: Vec<(i64, String, Vec<f32>)> = vec![];
        let results = vector_search(&query, &embeddings, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_vector_search_limit_respected() {
        let query = vec![1.0, 0.0];
        let embeddings: Vec<(i64, String, Vec<f32>)> = (0..10)
            .map(|i| (i, format!("/doc_{i}.txt"), vec![1.0, 0.0]))
            .collect();

        let results = vector_search(&query, &embeddings, 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_vector_search_large_dataset_correctness() {
        const NUM_DOCS: usize = 10_000;
        const DIM: usize = 1536;

        // Create a query vector
        let query: Vec<f32> = (0..DIM).map(|i| ((i % 7) as f32) * 0.1).collect();

        // Create NUM_DOCS embeddings with one known-best match at index 42
        let mut embeddings: Vec<(i64, String, Vec<f32>)> = (0..NUM_DOCS)
            .map(|i| {
                let emb: Vec<f32> = (0..DIM).map(|d| ((d + i) % 11) as f32 * 0.05).collect();
                (i as i64, format!("/doc_{i}.txt"), emb)
            })
            .collect();

        // Plant a near-identical vector at index 42
        embeddings[42].2 = query.iter().map(|v| v + 0.0001).collect();

        let limit = 5;
        let results = vector_search(&query, &embeddings, limit);

        assert_eq!(results.len(), limit);
        // The planted near-identical vector must be the top result
        assert_eq!(results[0].db_id, 42, "Expected db_id 42 as top result");
        assert!(
            results[0].score > 0.99,
            "Expected score > 0.99, got {}",
            results[0].score
        );
        // Results must be sorted by descending score
        for i in 0..results.len() - 1 {
            assert!(
                results[i].score >= results[i + 1].score,
                "Results not sorted at position {i}: {} >= {} failed",
                results[i].score,
                results[i + 1].score
            );
        }
    }

    #[test]
    fn test_vector_search_sorted_by_descending_score() {
        let query = vec![1.0, 0.0, 0.0];
        let embeddings = vec![
            (1, "/a.txt".to_string(), vec![0.0, 1.0, 0.0]), // low
            (2, "/b.txt".to_string(), vec![0.5, 0.5, 0.0]), // medium
            (3, "/c.txt".to_string(), vec![0.9, 0.1, 0.0]), // high
            (4, "/d.txt".to_string(), vec![1.0, 0.0, 0.0]), // highest
        ];

        let results = vector_search(&query, &embeddings, 10);
        for i in 0..results.len() - 1 {
            assert!(
                results[i].score >= results[i + 1].score,
                "Results not sorted: {} >= {} failed",
                results[i].score,
                results[i + 1].score
            );
        }
    }
}
