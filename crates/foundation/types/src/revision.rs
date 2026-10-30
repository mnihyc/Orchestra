//! Revision and sequence primitives.
//!
//! A revision counts the changes of one object: a writer says which one it
//! saw, and is refused when the object has moved on. A sequence number
//! orders the entries of one feed without gaps, so that a reader knows what
//! it missed.

/// How many times an object was changed. The first is 1.
pub type Revision = i64;

/// The place of an entry in its feed. A feed begins at 1 and has no gaps.
pub type Seq = i64;

/// What a reader that has seen everything up to `seen` should do with an
/// entry numbered `seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrival {
    /// The next entry: apply it.
    Next,
    /// Seen before: drop it.
    Duplicate,
    /// Entries between were missed: read them before applying this one.
    Gap {
        /// The first sequence number that was missed.
        from: Seq,
    },
}

/// Classifies an entry numbered `seq` for a reader that has applied
/// everything up to and including `seen`.
#[must_use]
pub fn arrival(seen: Seq, seq: Seq) -> Arrival {
    if seq <= seen {
        Arrival::Duplicate
    } else if seq == seen + 1 {
        Arrival::Next
    } else {
        Arrival::Gap { from: seen + 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_applies_the_next_drops_the_old_and_notices_a_gap() {
        assert_eq!(arrival(4, 5), Arrival::Next);
        assert_eq!(arrival(4, 4), Arrival::Duplicate);
        assert_eq!(arrival(4, 2), Arrival::Duplicate);
        assert_eq!(arrival(4, 7), Arrival::Gap { from: 5 });
        assert_eq!(arrival(0, 1), Arrival::Next);
    }
}
