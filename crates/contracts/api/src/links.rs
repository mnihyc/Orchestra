//! A document's Links section: what refers to it, what it refers to, the graph around it, and which tabs it shows.
//!
//! Every list here is filtered for the reader before it is paged: a source or a target that the reader may not
//! read is left out whole, and nothing counts it, names it, draws it or leaves its identity in a cursor.

use crate::common::Page;
use crate::documents::Author;
use crate::nodes::NodeKind;
use crate::routes::Access::Permission;
use crate::search::{RelatedAncestor, RelatedKind, RelatedMeaning};
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The tabs of the Links section, in the order the section shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinksTab {
    /// The things that refer to this document, with the line around each mention.
    ReferencedBy,
    /// The things this document refers to.
    References,
    /// Documents, chats and threads related by meaning, place and references.
    Related,
    /// The references around this document, drawn.
    Graph,
}

/// Which tabs a document shows, and where that comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentLinks {
    /// The tabs every reader of the document sees, in the section's order; empty hides the section.
    pub shown: Vec<LinksTab>,
    /// What was set for this document alone; `null` follows the place's setting.
    pub own: Option<Vec<LinksTab>>,
    /// What the place's setting gives where nothing is set for the document.
    pub inherited: Vec<LinksTab>,
    /// Whether a lock above the document keeps a choice of its own from applying.
    pub locked: bool,
    /// Whether the caller may change `own`: they manage the document and nothing locks it.
    pub writable: bool,
}

/// The tabs one document shows: a list sets them, `null` goes back to the place's setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentLinksSetBody {
    /// The tabs to show for this document alone, or `null` to follow the place's setting again.
    pub shown: Option<Vec<LinksTab>>,
}

/// Which way a list of references reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkDirection {
    /// What refers to this document.
    In,
    /// What this document refers to.
    Out,
}

/// The kinds of thing a reference can lead to or come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkedKind {
    /// A document.
    Document,
    /// A chat.
    Chat,
    /// A thread.
    Thread,
    /// An org, a project or a topic.
    Place,
    /// A file attached to a document or a chat message.
    File,
    /// A file in an environment's workspace.
    EnvironmentFile,
}

/// What a page of references reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentReferencesListQuery {
    /// Which way: `in` for what refers to the document, `out` for what it refers to.
    pub direction: LinkDirection,
    /// How many sources or targets, at most; the place's page size when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before; it belongs to the caller and this list alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Only the mentions of one source, as `<kind>:<id>` with `document`, `chat` or `thread`: the page that follows
    /// the mentions already shown under it. The reply holds that source alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
}

/// The line around one mention, cut where the text is longer; the three parts read in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkLine {
    /// The words before the reference, beginning with an ellipsis where text was cut.
    pub before: String,
    /// The reference itself, under the name the target has now; empty where the part names no single place.
    pub reference: String,
    /// The words after it, ending with an ellipsis where text was cut.
    pub after: String,
}

/// Where a mention stands, which is where a click goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LinkAnchor {
    /// A block of a document.
    Block {
        /// The block.
        block_id: Id,
    },
    /// A comment in a discussion beside a document.
    Comment {
        /// The comment.
        comment_id: Id,
        /// The discussion it is in.
        discussion_id: Id,
        /// The block the discussion is about; `null` where that block is gone.
        block_id: Option<Id>,
    },
    /// A message of a chat.
    Message {
        /// The message.
        message_id: Id,
    },
    /// An item of a thread's conversation.
    Turn {
        /// The item.
        item_id: Id,
        /// The number of the turn it is in; `null` for an item that lies in none.
        turn: Option<i64>,
    },
}

/// One mention of the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkOccurrence {
    /// Where it stands.
    pub anchor: LinkAnchor,
    /// The line around it, as this reader reads it.
    pub line: LinkLine,
    /// Who wrote it, where that is known.
    pub by: Option<Author>,
    /// When it was written or last changed.
    pub at: Time,
}

/// A thing that refers to the document, or that the document refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkedThing {
    /// What kind of thing.
    pub kind: LinkedKind,
    /// Its stable identity.
    pub id: Id,
    /// For an environment file, its path from the workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// For a place, which kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_kind: Option<NodeKind>,
    /// Its name now where the reader may read it; `null` where they may not.
    pub name: Option<String>,
    /// What the author of the reference wrote for it. Only for what this document refers to: the reader already
    /// reads that label in the text.
    pub label: Option<String>,
    /// Whether the reader may read it.
    pub readable: bool,
    /// The places it lies in that the reader may know, outermost first.
    #[serde(default)]
    pub context: Vec<RelatedAncestor>,
    /// How many places in the source refer to the document, or in the document refer to this target; counted only up
    /// to `documents.links.mentions_counted` for a source.
    pub mentions: i64,
    /// Whether more places refer to the document than `mentions` counts: the count reads "or more".
    #[serde(default)]
    pub mentions_more: bool,
    /// The first mentions, each with its line; empty for what this document refers to.
    #[serde(default)]
    pub occurrences: Vec<LinkOccurrence>,
    /// What to send as `cursor`, with `of` naming this source, for the mentions that follow those listed; `null`
    /// when all are listed.
    pub next_mentions: Option<String>,
}

/// What a graph reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentGraphGetQuery {
    /// How many steps from the centre, from 1 to 3; 1 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// The kinds drawn, comma separated from `document`, `chat` and `thread`; all when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    /// Whether to add dashed edges to what is related by meaning; off when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meaning: Option<bool>,
    /// A node to centre on instead of the document, as `<kind>:<id>`; it must be readable like any other node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<String>,
}

/// One node of the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GraphNode {
    /// What kind of thing.
    pub kind: RelatedKind,
    /// Its stable identity.
    pub id: Id,
    /// Its name now, as the reader may read it.
    pub name: String,
    /// The places it lies in that the reader may know, outermost first.
    #[serde(default)]
    pub context: Vec<RelatedAncestor>,
    /// How many steps it is from the centre.
    pub distance: u32,
    /// Whether it is the document the section belongs to.
    pub current: bool,
}

/// Why two nodes are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GraphEdgeKind {
    /// One refers to the other.
    Reference,
    /// Their meaning is alike.
    Meaning,
}

/// One edge: from the thing that refers to the thing referred to, or between two alike in meaning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GraphEdge {
    /// The node that refers; for a meaning edge, the centre.
    pub from: Id,
    /// The node referred to.
    pub to: Id,
    /// Why they are joined.
    pub kind: GraphEdgeKind,
}

/// The readable references around a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkGraph {
    /// The node the graph is centred on.
    pub center: Id,
    /// Readable nodes only, the centre first.
    pub nodes: Vec<GraphNode>,
    /// Edges between nodes in `nodes`; none passes through a node the reader may not read.
    pub edges: Vec<GraphEdge>,
    /// Whether a readable node was left out because the graph reached its size.
    pub truncated: bool,
    /// Whether meaning could join the graph, when it was asked for.
    pub meaning: RelatedMeaning,
}

crate::endpoints! {
    /// Says which tabs of the Links section a document shows, and whether the caller may change them.
    DocumentLinksGet: GET "/documents/{id}/links", Permission("doc.view"), none, none, json DocumentLinks, 200;
    /// Chooses the tabs of the Links section for one document, or goes back to the place's setting.
    DocumentLinksSet: PUT "/documents/{id}/links", Permission("doc.manage"), none, json DocumentLinksSetBody, json DocumentLinks, 200;
    /// Lists what refers to a document, with the line around each mention, or what it refers to; the reader's own view, paged.
    DocumentReferencesList: GET "/documents/{id}/references", Permission("doc.view"), query DocumentReferencesListQuery, none, json Page<LinkedThing>, 200;
    /// Draws the readable documents, chats and threads that refer to each other around a document.
    DocumentGraphGet: GET "/documents/{id}/graph", Permission("doc.view"), query DocumentGraphGetQuery, none, json LinkGraph, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tabs_are_written_as_their_words_and_a_body_may_clear_its_choice() {
        let tabs: Vec<LinksTab> =
            serde_json::from_value(json!(["referenced_by", "references", "related", "graph"]))
                .unwrap();
        assert_eq!(tabs.len(), 4);
        let clear: DocumentLinksSetBody = serde_json::from_value(json!({"shown": null})).unwrap();
        assert_eq!(clear.shown, None);
        assert!(
            serde_json::from_value::<DocumentLinksSetBody>(json!({"shown": ["nowhere"]})).is_err()
        );
    }

    #[test]
    fn a_mention_keeps_its_anchor_and_three_part_line_on_the_wire() {
        let wire = json!({
            "anchor": {"type": "turn", "item_id": "019a0000-0000-7000-8000-000000000001", "turn": 4},
            "line": {"before": "… see ", "reference": "Plan", "after": " for the dates"},
            "by": null,
            "at": "2026-10-29T10:00:00.000Z"
        });
        let read: LinkOccurrence = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(read).unwrap(), wire);
    }

    #[test]
    fn a_locked_target_is_a_label_without_a_name() {
        let wire = json!({
            "kind": "document", "id": "019a0000-0000-7000-8000-000000000002",
            "name": null, "label": "Budget", "readable": false, "context": [],
            "mentions": 1, "mentions_more": false, "occurrences": [], "next_mentions": null
        });
        let read: LinkedThing = serde_json::from_value(wire.clone()).unwrap();
        assert!(!read.readable);
        assert_eq!(serde_json::to_value(read).unwrap(), wire);
    }
}
