//! Typed UUID identifiers.
//!
//! An identifier is a UUID, written lowercase and hyphenated. New ones are
//! version 7: they sort by the moment they were made, which keeps indexes
//! of freshly inserted rows local and lets a list page by identifier.

/// The identifier of anything Orch stores.
pub type Id = uuid::Uuid;

/// A new identifier, ordered after every one made earlier on this process.
#[must_use]
pub fn new_id() -> Id {
    uuid::Uuid::now_v7()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_identifiers_sort_later() {
        let first = new_id();
        let second = new_id();
        assert!(first < second);
    }

    #[test]
    fn written_lowercase_and_hyphenated() {
        let id = new_id();
        let text = serde_json::to_string(&id).unwrap();
        assert_eq!(text.len(), 38);
        assert_eq!(text, text.to_lowercase());
    }
}
