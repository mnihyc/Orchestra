//! Reciprocal-rank fusion: one order made of the order by words and the order by meaning.
//!
//! Each result scores, for every list it is in, one over the fusion constant plus its rank there (ranks counted
//! from one), and the sum orders them. The constant (`fusion_k` of `search.semantic.tuning`, 60 as shipped, the
//! value the method was published with) keeps the first few ranks of either list from outweighing a result
//! that both lists rank well. Ties keep the order of the first list, then of the next, so that the same lists
//! always give the same order.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// The lists, best first in each, fused into one order, best first, with each result's score.
#[must_use]
pub fn fuse<K: Clone + Eq + Hash>(lists: &[Vec<K>], k: u32) -> Vec<(K, f64)> {
    let mut scores: HashMap<K, (f64, usize)> = HashMap::new();
    let mut seen = 0_usize;
    for list in lists {
        let mut distinct = HashSet::new();
        for (rank, key) in list.iter().filter(|key| distinct.insert(*key)).enumerate() {
            let add = 1.0 / (f64::from(k) + rank_of(rank));
            let entry = scores.entry(key.clone()).or_insert_with(|| {
                seen += 1;
                (0.0, seen)
            });
            entry.0 += add;
        }
    }
    let mut fused: Vec<(K, f64, usize)> = scores
        .into_iter()
        .map(|(key, (score, first))| (key, score, first))
        .collect();
    fused.sort_by(|one, other| other.1.total_cmp(&one.1).then(one.2.cmp(&other.2)));
    fused
        .into_iter()
        .map(|(key, score, _)| (key, score))
        .collect()
}

/// A place in a list as the rank fusion counts it, from one.
fn rank_of(index: usize) -> f64 {
    // A list is never longer than a few thousand; the conversion is exact far beyond that.
    f64::from(u32::try_from(index).unwrap_or(u32::MAX)) + 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_both_orders_rank_well_comes_before_one_that_only_one_ranks_first() {
        let words = vec!["a", "b", "c", "d"];
        let meaning = vec!["e", "c", "b", "f"];
        let fused: Vec<&str> = fuse(&[words, meaning], 60)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(fused[..2], ["b", "c"], "in both lists, near the top");
        assert_eq!(fused.len(), 6, "each result once");
        assert!(fused.contains(&"f"));
    }

    #[test]
    fn scores_follow_the_formula_and_ties_keep_the_first_lists_order() {
        let fused = fuse(&[vec![1, 2], vec![2, 1]], 60);
        assert!((fused[0].1 - (1.0 / 61.0 + 1.0 / 62.0)).abs() < 1e-12);
        assert_eq!(
            fused[0].0, 1,
            "an exact tie keeps the order of the first list"
        );
        assert_eq!(fused[1].0, 2);
        let alone = fuse(&[vec!["x"], Vec::new()], 60);
        assert_eq!(alone, vec![("x", 1.0 / 61.0)]);
        assert!(fuse::<u8>(&[], 60).is_empty());
    }

    #[test]
    fn several_chunks_standing_for_one_part_contribute_once_per_source() {
        let fused = fuse(&[vec!["both"], vec!["fragment", "fragment", "both"]], 60);
        assert_eq!(fused[0].0, "both");
        assert_eq!(
            fused
                .iter()
                .find(|(key, _)| *key == "fragment")
                .map(|(_, score)| *score),
            Some(1.0 / 61.0)
        );
        assert_eq!(fused.len(), 2);
    }

    #[test]
    fn a_smaller_constant_lets_the_top_ranks_weigh_more() {
        let lists = [
            vec!["top", "both"],
            vec!["other", "x", "y", "z", "w", "both"],
        ];
        let gentle: Vec<&str> = fuse(&lists, 60).into_iter().map(|(key, _)| key).collect();
        let steep: Vec<&str> = fuse(&lists, 1).into_iter().map(|(key, _)| key).collect();
        assert_eq!(gentle[0], "both");
        assert_eq!(steep[0], "top");
    }
}
