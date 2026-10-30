//! The arithmetic of vectors: their length, how alike two are, and a vector shortened to fewer numbers.
//!
//! Vectors are compared by the cosine of their angle. A model trained to keep its meaning in its first numbers
//! (Matryoshka representation learning) gives a shorter vector by keeping those and scaling the rest away; a
//! shortened vector is made unit length again, as such models ask. A vector of fewer numbers than asked is kept
//! as it is: only the model's own length can be shortened.

/// The vector scaled to unit length; a vector of zeros stays as it is.
#[must_use]
pub fn unit(vector: &[f32]) -> Vec<f32> {
    let length = norm(vector);
    if length == 0.0 || !length.is_finite() {
        return vector.to_vec();
    }
    vector
        .iter()
        .map(|value| (f64::from(*value) / length) as f32)
        .collect()
}

/// Its first `dimensions` numbers made unit length, or all of them made so when it has no more.
#[must_use]
pub fn shortened(vector: &[f32], dimensions: usize) -> Vec<f32> {
    unit(&vector[..dimensions.min(vector.len())])
}

/// The cosine of the angle between two vectors of one length; 0 when either is all zeros or their lengths
/// differ.
#[must_use]
pub fn cosine(one: &[f32], other: &[f32]) -> f64 {
    if one.len() != other.len() {
        return 0.0;
    }
    let lengths = norm(one) * norm(other);
    if lengths == 0.0 {
        return 0.0;
    }
    let dot: f64 = one
        .iter()
        .zip(other)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum();
    dot / lengths
}

/// The Euclidean length.
fn norm(vector: &[f32]) -> f64 {
    vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortened_vector_keeps_its_first_numbers_at_unit_length() {
        let made = shortened(&[3.0, 4.0, 12.0], 2);
        assert_eq!(made.len(), 2);
        assert!((made[0] - 0.6).abs() < 1e-6 && (made[1] - 0.8).abs() < 1e-6);
        assert_eq!(
            shortened(&[3.0, 4.0], 8).len(),
            2,
            "a vector is never lengthened"
        );
        assert_eq!(unit(&[0.0, 0.0]), vec![0.0, 0.0]);
    }

    #[test]
    fn the_same_direction_is_alike_and_a_right_angle_is_not() {
        assert!((cosine(&[1.0, 2.0], &[2.0, 4.0]) - 1.0).abs() < 1e-9);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-9);
        assert!((cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-9);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0, "lengths differ");
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }
}
