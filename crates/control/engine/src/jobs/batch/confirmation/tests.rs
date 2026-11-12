//! Count full comparisons, independently of machine timing or business batch limits.
use super::indexed_positions;
use orch_types::Id;
use std::cell::Cell;

#[derive(Clone, PartialEq, Eq)]
struct Stored {
    thread: Id,
    source: u32,
    request: u32,
}
#[derive(Clone)]
struct Member {
    stored: Stored,
    selected: u32,
}
fn member(index: u32) -> Member {
    Member {
        stored: Stored {
            thread: Id::from_u128(u128::from(index) + 1),
            source: index,
            request: index,
        },
        selected: 3,
    }
}
fn positions(old: &[Member], fresh: &[Member], comparisons: &Cell<usize>) -> Option<Vec<usize>> {
    indexed_positions(
        old,
        fresh,
        |member| member.stored.thread,
        |old, fresh| {
            comparisons.set(comparisons.get() + 1);
            old.stored == fresh.stored && (old.selected & fresh.selected) == fresh.selected
        },
    )
}

#[test]
fn large_unchanged_reordered_and_skipped_batches_compare_full_members_once_per_selected_thread() {
    // Thousands of unique threads expose the former triangular scan without relying on elapsed time.
    let old: Vec<_> = (0..4096).map(member).collect();
    let comparisons = Cell::new(0);
    assert_eq!(
        positions(&old, &old, &comparisons),
        Some((0..old.len()).collect())
    );
    assert_eq!(comparisons.get(), old.len());
    comparisons.set(0);
    let reversed: Vec<_> = old.iter().rev().cloned().collect();
    assert_eq!(
        positions(&old, &reversed, &comparisons),
        Some((0..old.len()).rev().collect())
    );
    assert_eq!(comparisons.get(), old.len());
    comparisons.set(0);
    let skipped: Vec<_> = old.iter().step_by(3).cloned().collect();
    assert_eq!(
        positions(&old, &skipped, &comparisons),
        Some((0..old.len()).step_by(3).collect())
    );
    assert_eq!(comparisons.get(), skipped.len());
}

#[test]
fn thread_identity_never_replaces_complete_stored_or_selected_source_comparisons() {
    let old = [member(0), member(1)];
    for change in ["thread", "source", "request", "selected"] {
        let mut fresh = old.to_vec();
        match change {
            "thread" => fresh[1].stored.thread = Id::from_u128(3),
            "source" => fresh[1].stored.source += 1,
            "request" => fresh[1].stored.request += 1,
            "selected" => fresh[1].selected = 4,
            _ => unreachable!(),
        }
        assert!(positions(&old, &fresh, &Cell::new(0)).is_none(), "{change}");
    }
    let mut subset = old.to_vec();
    subset[0].selected = 1;
    assert_eq!(positions(&old, &subset, &Cell::new(0)), Some(vec![0, 1]));
}

#[test]
fn duplicate_thread_buckets_keep_candidate_order_and_reuse_instead_of_consuming_matches() {
    let first = Member {
        selected: 1,
        ..member(0)
    };
    let mut different = first.clone();
    different.stored.request += 1;
    different.selected = 15;
    let later = Member {
        selected: 3,
        ..first.clone()
    };
    let old = [first, different, later];
    let fresh = Member {
        selected: 2,
        ..member(0)
    };
    assert_eq!(
        positions(&old, &[fresh.clone(), fresh], &Cell::new(0)),
        Some(vec![2, 2])
    );
    let weaker = Member {
        selected: 0,
        ..member(0)
    };
    assert_eq!(positions(&old, &[weaker], &Cell::new(0)), Some(vec![0]));
    assert_eq!(positions(&old, &[], &Cell::new(0)), Some(Vec::new()));
    assert_eq!(positions(&[], &[], &Cell::new(0)), Some(Vec::new()));
    assert!(positions(&[], &[member(0)], &Cell::new(0)).is_none());
}
