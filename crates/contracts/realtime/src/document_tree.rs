//! Current authorized document-directory and entry metadata, without editable content.

use orch_api_contract::document_tree::{
    DocumentDirectory, DocumentTreeItem, DocumentTreePage, DocumentTreeRead,
};
use orch_api_contract::documents::DeletedFilter;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An active human metadata source whose page registrations stay individually bounded.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentTreeView {
    /// The current reader's incrementally registered hierarchy pages, across their roots.
    Hierarchy {},

    /// One requested directory page, or its flat retained document-ancestry inventory.
    Directory {
        /// Exact root/folder identity.
        at: DocumentDirectory,
        /// Only retained entries when present.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deleted: Option<DeletedFilter>,
        /// Opaque internal candidate progress for this page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
        /// At most five hundred entries; fifty when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u16>,
    },
    /// One typed entry and its redacted structural ancestry.
    Entry {
        /// The actual document-domain identity.
        item: DocumentTreeItem,
        /// An explicit current-restoration-authorized, read-only retained view.
        #[serde(default)]
        inspect_deleted: bool,
    },
}

/// Change one page of an already admitted hierarchy metadata subscription.
/// Query JSON is UTF-8, split into consecutive URL-safe unpadded base64 fragments so
/// even a long selector fits the physical client frame boundary. A window is a
/// connection-local routing number and grants no authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HierarchyWindow {
    /// Add or replace one bounded selector when its last fragment arrives.
    Query {
        /// The actual root used by the same native producer as the HTTP page.
        root: orch_types::Id,
        /// Chosen by the client; stable only while this page is held.
        window: u64,
        /// The next consecutive fragment of UTF-8 query JSON.
        data: String,
        /// This fragment completes the selector.
        complete: bool,
    },
    /// Stop following this page and release its selector and publication fingerprint.
    Remove {
        /// The same client-chosen routing number.
        window: u64,
    },
}

impl HierarchyWindow {
    /// The page changed by this control frame.
    #[must_use]
    pub const fn window(&self) -> u64 {
        match self {
            Self::Query { window, .. } | Self::Remove { window } => *window,
        }
    }
}

/// Only scoped current pages belong on the metadata source; a full-tree/delta read does not.
#[must_use]
pub fn bounded_hierarchy_query(query: &orch_api_contract::nodes::OrgTreeGetQuery) -> bool {
    query.since.is_none()
        && query.limit != Some(0)
        && (query.parent_id.is_some()
            || query.path_to.is_some()
            || query.q.is_some()
            || query.kind.is_some()
            || query.below_id.is_some()
            || query.include_threads == Some(false)
            || query.sort.is_some())
}

/// A current page/entry replacement. No historical row replay is claimed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum DocumentTreeData {
    /// Replace one authorized hierarchy page; never replay a root's complete history.
    Hierarchy {
        /// The exact page replaced by this event, within the subscribed open branches.
        window: u64,
        /// A part can become unavailable while the other open branches stay live.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        problem: Option<orch_api_contract::common::ErrorCode>,
        hierarchy: Box<orch_api_contract::nodes::OrgTreeGetReply>,
    },

    /// Replace only this subscribed page.
    Directory {
        /// The same projection as the HTTP list.
        directory: Box<DocumentTreePage>,
        /// Its old cursor generation changed; this is the current first page.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        restart: bool,
    },
    /// Replace only this entry's authorized metadata/ancestry.
    Entry {
        /// The same projection as its typed metadata read.
        entry: Box<DocumentTreeRead>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feeds::{EventData, Feed, FeedKind};
    use orch_types::Id;
    use serde_json::json;

    type Proof = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn typed_metadata_feeds_roundtrip_without_content_presence_or_durable_replay() -> Proof {
        let view = DocumentTreeView::Directory {
            at: DocumentDirectory::NodeDocs(Id::from_u128(1)),
            deleted: Some(DeletedFilter::Only),
            cursor: Some("sealed-synthetic-progress".to_owned()),
            limit: Some(50),
        };
        let feed = Feed::DocumentTree(view);
        assert_eq!(feed.to_string().parse::<Feed>()?, feed);
        assert_eq!(feed.kind(), FeedKind::Ephemeral);
        assert!(!feed.has_presence() && !feed.has_typing() && !feed.has_deltas());
        assert!(feed.sync_paths().is_empty());
        let entry = Feed::DocumentTree(DocumentTreeView::Entry {
            item: DocumentTreeItem::Container(Id::from_u128(2)),
            inspect_deleted: true,
        });
        assert_eq!(entry.to_string().parse::<Feed>()?, entry);
        Ok(())
    }

    #[test]
    fn an_empty_visible_page_can_carry_real_scan_progress_and_a_named_restart() -> Proof {
        let node = Id::from_u128(1);
        let literal = json!({"directory":{"items":[],"next_cursor":"sealed-next","feed":"current-feed",
            "directory":{"at":{"kind":"node_docs","id":node},"node":{"id":node,"kind":"topic","name":"Plans","redacted":false},
                "path":[],"folder":null,"ancestors":[],"can_create":false,"inspection":null,"membership_seq":7},
            "ancestors":[]},"restart":true});
        let feed = Feed::DocumentTree(DocumentTreeView::Directory {
            at: DocumentDirectory::NodeDocs(node),
            deleted: None,
            cursor: None,
            limit: None,
        });
        let data = EventData::decode(&feed, literal.clone())?;
        assert_eq!(serde_json::Value::Object(data.to_object()?), literal);
        assert!(
            serde_json::from_value::<DocumentTreeData>(
                json!({"entry":{"entries":[],"ancestors":[]},"directory":{}})
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn selectors_refuse_unbounded_views_unknown_scopes_and_zero_page_limits() {
        let at = DocumentDirectory::NodeDocs(Id::from_u128(1));
        let invalid = DocumentTreeView::Directory {
            at,
            deleted: None,
            cursor: None,
            limit: Some(0),
        };
        assert!(
            Feed::DocumentTree(invalid)
                .to_string()
                .parse::<Feed>()
                .is_err()
        );
        assert!(
            serde_json::from_value::<DocumentTreeView>(
                json!({"kind":"directory","at":at,"all_orgs":true})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<DocumentTreeView>(
                json!({"kind":"entry","item":{"kind":"project","id":Id::from_u128(2)}})
            )
            .is_err()
        );
    }
    #[test]
    fn a_hundred_open_pages_each_cross_the_real_client_frame_boundary_without_a_growing_feed()
    -> Proof {
        use crate::frames::ClientFrame;
        use base64::Engine as _;
        let feed = Feed::DocumentTree(serde_json::from_value::<DocumentTreeView>(
            json!({"kind":"hierarchy"}),
        )?);
        let first = json!({"op":"sub","id":1,"feed":feed.to_string()}).to_string();
        assert!(ClientFrame::decode(&first).is_ok());
        for window in 1..=100 {
            let query = json!({"parent_id":Id::from_u128(1),"cursor":format!("root|parent|123|scope|{window}")});
            let data = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&query)?);
            let frame = json!({"op":"sub","id":1,"feed":feed.to_string(),
                "window":{"kind":"query","root":Id::from_u128(1),"window":window,"data":data,"complete":true}})
            .to_string();
            assert!(frame.len() <= crate::version::MAX_CLIENT_FRAME_BYTES);
            assert!(
                ClientFrame::decode(&frame).is_ok(),
                "page {window} must keep the socket open"
            );
        }
        Ok(())
    }
}
