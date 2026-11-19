//! A document's feed: its reference, the blocks that changed by sequence, and its discussions as they are opened, answered and resolved.
//!
//! A document is rows with a sequence, as a thread and a chat are: its
//! blocks are stamped from the document's own sequence, in the order in
//! which the changes were committed, and everybody who has the document
//! open is sent the blocks of each accepted change in that order. Whoever
//! wrote them, a person or an agent, a block says so itself.
//!
//! A block that was removed arrives as a row with `removed: true`, and
//! stays in the store: rows are never taken away on the client's own
//! initiative. A discussion arrives whole, with all its comments, when
//! anything in it changed: it was opened, somebody answered, it was
//! resolved or opened again.
//!
//! Removed blocks are purged for good after a while, and the document
//! keeps the sequence up to which that was done. A reader who would
//! continue from below it cannot be told what was removed and loads the
//! document again ([`FeedHead::reload_cause`](crate::sync::FeedHead::reload_cause)).
//!
//! What a person types in a block is not here: it is written over HTTP
//! and comes back as a row. Who is writing where is told by
//! [`TypingNotice`](crate::typing::TypingNotice), and who has the document
//! open by [`PresenceUser`](crate::presence::PresenceUser).

use crate::foreign;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a `sync` frame of a document's feed carries. Every member is there
/// only when rows of its kind changed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DocData {
    /// The document itself, without its blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<foreign::DocumentRef>,
    /// The blocks that a batch of changes touched, as they now stand;
    /// those that were removed say `removed: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<foreign::DocumentBlock>>,
    /// The discussions beside the document in which something changed,
    /// each with all its comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discussions: Option<Vec<foreign::DocumentDiscussion>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const DOC: &str = "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a00";

    fn person() -> Value {
        json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a10", "kind": "human", "username": "bobby",
            "display_name": "Bobby", "avatar": { "text": "B", "color": 2 }
        })
    }

    fn block(id: &str, seq: i64, removed: bool) -> Value {
        json!({
            "id": id, "document_id": DOC, "parent_id": null, "position": "1", "type": "paragraph",
            "props": {}, "content": [{ "type": "text", "text": "Hello" }], "removed": removed,
            "seq_created": 1, "seq_updated": seq, "updated_by": person(),
            "updated_at": "2026-10-08T17:04:05.123Z"
        })
    }

    fn document(seq: i64) -> Value {
        json!({
            "id": DOC, "title": "Handbook", "description": "How we ship", "description_source": "user",
            "description_line": "How we ship",
            "node": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a20", "kind": "topic", "name": "Plans", "redacted": false },
            "owner": person(), "owners": 1, "created_by": person(), "page": "follows", "seq": seq,
            "block_count": 2, "bytes": 120, "files_bytes": 0, "version": 0, "open_discussions": 1,
            "updated_at": "2026-10-08T17:04:05.123Z", "updated_by": person(), "seen_seq": null,
            "discussion_level": "involved", "archived": false, "deleted": false
        })
    }

    fn discussion() -> Value {
        json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03", "document_id": DOC,
            "block_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "quote": "", "anchored": true,
            "resolved": { "at": "2026-10-08T17:06:00.000Z", "by": person() },
            "created_by": person(), "created_at": "2026-10-08T17:05:00.000Z", "comments": [],
            "seq_updated": 32
        })
    }

    #[test]
    fn a_batch_of_changes_arrives_as_the_blocks_it_touched_and_a_removed_one_says_so() {
        let written = json!({
            "blocks": [
                block("0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", 31, false),
                block("0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02", 31, true)
            ]
        });
        let data: DocData = serde_json::from_value(written.clone()).unwrap();
        let blocks = data.blocks.as_ref().unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(!blocks[0].removed && blocks[1].removed);
        assert_eq!(data.document, None);
        assert_eq!(data.discussions, None);
        assert_eq!(serde_json::to_value(&data).unwrap(), written);
    }

    #[test]
    fn a_discussion_arrives_with_the_document_it_is_beside_and_nothing_is_there_that_did_not_change()
     {
        let written = json!({ "document": document(32), "discussions": [discussion()] });
        let data: DocData = serde_json::from_value(written.clone()).unwrap();
        assert!(data.blocks.is_none());
        assert_eq!(data.document.as_ref().map(|each| each.seq), Some(32));
        let discussions = data.discussions.as_ref().unwrap();
        assert!(discussions[0].resolved.is_some());
        assert_eq!(serde_json::to_value(&data).unwrap(), written);
        assert_eq!(serde_json::to_value(DocData::default()).unwrap(), json!({}));
    }

    #[test]
    fn a_row_that_is_no_row_of_a_document_is_refused() {
        let written = json!({ "blocks": [{ "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "removed": false }] });
        assert!(serde_json::from_value::<DocData>(written).is_err());
    }
}
