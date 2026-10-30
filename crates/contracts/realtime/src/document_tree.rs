//! Current authorized document-directory and entry metadata, without editable content.

use orch_api_contract::document_tree::{
    DocumentDirectory, DocumentTreeItem, DocumentTreePage, DocumentTreeRead,
};
use orch_api_contract::documents::DeletedFilter;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One bounded active human metadata view, independent of an agent's shared-tree authority.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentTreeView {
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

/// A current page/entry replacement. No historical row replay is claimed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum DocumentTreeData {
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
}
