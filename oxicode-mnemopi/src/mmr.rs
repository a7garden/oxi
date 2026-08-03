//! MMR (Maximal Marginal Relevance) diversity reranking — ported from
//! omp `beam/mmr.ts`.
//!
//! Re-ranks recall results to balance relevance and diversity, avoiding
//! a cluster of near-identical results dominating the top-k.

use crate::vector_math::cosine_similarity;

/// An item that can be reranked by MMR.
#[derive(Debug, Clone)]
pub struct MmrItem {
    pub content: String,
    pub score: f32,
    pub embedding: Option<Vec<f32>>,
    /// Index into the original results list.
    pub original_index: usize,
}

/// Rerank items using Maximal Marginal Relevance.
///
/// - `lambda`: 0 = pure diversity, 1 = pure relevance (default: 0.7).
/// - `k`: number of items to return.
///
/// The algorithm iteratively selects the item with the highest MMR score:
/// ```text
/// MMR(i) = λ * relevance(i) - (1 - λ) * max(similarity(i, selected))
/// ```
///
/// Falls back to score-sorted order when embeddings are unavailable.
pub fn rerank(items: Vec<MmrItem>, lambda: f32, k: usize) -> Vec<MmrItem> {
    if items.is_empty() {
        return Vec::new();
    }
    if items.len() == 1 {
        return items;
    }

    let lambda = lambda.clamp(0.0, 1.0);
    let has_embeddings = items.iter().all(|i| i.embedding.is_some());

    if !has_embeddings {
        let mut sorted = items;
        sorted.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        sorted.truncate(k);
        return sorted;
    }

    let n = items.len();
    let mut selected: Vec<usize> = Vec::with_capacity(k);
    let mut remaining: Vec<usize> = (0..n).collect();

    // Precompute pairwise similarities
    let mut sim_matrix = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine_similarity(
                items[i].embedding.as_ref().unwrap(),
                items[j].embedding.as_ref().unwrap(),
            );
            sim_matrix[i][j] = sim;
            sim_matrix[j][i] = sim;
        }
    }

    // Select first item (highest relevance)
    let first = remaining
        .iter()
        .copied()
        .max_by(|&a, &b| {
            items[a]
                .score
                .partial_cmp(&items[b].score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    selected.push(first);
    remaining.retain(|&x| x != first);

    // Iteratively select items with highest MMR score
    while selected.len() < k && !remaining.is_empty() {
        let mut best_idx = 0;
        let mut best_mmr = f32::MIN;

        for (idx, &candidate) in remaining.iter().enumerate() {
            let relevance = items[candidate].score;
            let max_sim = selected
                .iter()
                .map(|&s| sim_matrix[candidate][s])
                .fold(0.0f32, f32::max);

            let mmr = lambda * relevance - (1.0 - lambda) * max_sim;

            if mmr > best_mmr {
                best_mmr = mmr;
                best_idx = idx;
            }
        }

        let chosen = remaining.swap_remove(best_idx);
        selected.push(chosen);
    }

    selected.into_iter().map(|i| items[i].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerank_favors_diversity() {
        let items = vec![
            MmrItem {
                content: "a".to_string(),
                score: 1.0,
                embedding: Some(vec![1.0, 0.0]),
                original_index: 0,
            },
            MmrItem {
                content: "b".to_string(),
                score: 0.9,
                embedding: Some(vec![0.99, 0.01]),
                original_index: 1,
            },
            MmrItem {
                content: "c".to_string(),
                score: 0.8,
                embedding: Some(vec![0.0, 1.0]),
                original_index: 2,
            },
        ];

        let result = rerank(items, 0.5, 3);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content, "a");
        // "c" (diverse) should come before "b" (similar to "a")
        assert_eq!(result[1].content, "c");
    }

    #[test]
    fn rerank_without_embeddings_sorts_by_score() {
        let items = vec![
            MmrItem {
                content: "low".to_string(),
                score: 0.3,
                embedding: None,
                original_index: 0,
            },
            MmrItem {
                content: "high".to_string(),
                score: 0.9,
                embedding: None,
                original_index: 1,
            },
        ];

        let result = rerank(items, 0.7, 2);
        assert_eq!(result[0].content, "high");
        assert_eq!(result[1].content, "low");
    }

    #[test]
    fn rerank_single_item_passthrough() {
        let items = vec![MmrItem {
            content: "only".to_string(),
            score: 1.0,
            embedding: Some(vec![1.0]),
            original_index: 0,
        }];
        let result = rerank(items, 0.7, 5);
        assert_eq!(result.len(), 1);
    }
}
