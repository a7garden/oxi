//! Cosine similarity and vector math — ported from omp `vector-math.ts`.

/// Cosine similarity between two vectors.
///
/// Handles length mismatch by zero-padding the shorter vector,
/// matching omp's `cosineSimilarity` behavior exactly.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().max(b.len());
    if len == 0 {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for i in 0..len {
        let av = if i < a.len() && a[i].is_finite() {
            a[i]
        } else {
            0.0
        };
        let bv = if i < b.len() && b[i].is_finite() {
            b[i]
        } else {
            0.0
        };
        dot += av * bv;
        norm_a += av * av;
        norm_b += bv * bv;
    }

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// Dot product of two equal-length vectors.
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// L2 norm of a vector.
pub fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn test_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 2.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_length_mismatch() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0]; // zero-padded to [1, 2, 0]
        let expected =
            (1.0f32 + 4.0f32) / ((1.0f32 + 4.0f32 + 9.0f32).sqrt() * (1.0f32 + 4.0f32).sqrt());
        assert!((cosine_similarity(&a, &b) - expected).abs() < 1e-6);
    }
}
