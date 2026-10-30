//! Typed document-folder navigation and atomic document-tree organization.

use crate::routes::Access::Rule;
use chrono::{DateTime, Utc};
use orch_types::{Id, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An existing directory in the document domain, independent of hierarchy folders.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum DocumentDirectory {
    /// The virtual docs root beside a node.
    NodeDocs(Id),
    /// A real document-only folder.
    Container(Id),
}

/// A text document or a distinct document-only folder.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum DocumentTreeItem {
    /// Editable text with its blocks, files and history.
    Document(Id),
    /// A navigable folder without a text editor.
    Container(Id),
}

/// The fixed kind of a document-folder access source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentContainerMark {
    /// A document-only folder.
    DocumentContainer,
}

/// A folder named by permission provenance; an unreadable folder has no label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct DocumentContainerRef {
    /// Native folder identity.
    pub id: Id,
    /// Its distinct domain kind.
    pub kind: DocumentContainerMark,
    /// Current label, when readable.
    pub name: Option<String>,
    /// Whether the label was withheld.
    pub redacted: bool,
}

/// One document namespace entry, without editable content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreeEntry {
    /// Type and stable identity.
    pub item: DocumentTreeItem,
    /// Immediate directory.
    pub parent: DocumentDirectory,
    /// Actual hierarchy anchor.
    pub node: Id,
    /// Root organization.
    pub root: Id,
    /// Exact path component.
    pub basename: String,
    /// Human label.
    pub title: String,
    /// Content/container revision.
    pub revision: i64,
    /// Accountable first owner.
    pub owner: Id,
    /// Exact retained deletion timestamp, preserved in confirmation comparisons.
    #[schemars(with = "Option<Time>")]
    pub deleted_at: Option<DateTime<Utc>>,
    /// Recursive deletion cohort.
    pub deletion: Option<Id>,
    /// Attached-file count; zero for a folder.
    pub files: i64,
    /// Retained version count; zero for a folder.
    pub versions: i64,
}

/// Current authority on one document entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreeStanding {
    /// May open or navigate.
    pub read: bool,
    /// May edit text or create entries in the folder.
    pub write: bool,
    /// May manage this item; descendants are checked separately.
    pub manage: bool,
}

/// A caller-filtered namespace row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreeListed {
    /// Current metadata.
    pub entry: DocumentTreeEntry,
    /// Actual authority.
    pub standing: DocumentTreeStanding,
    /// Read-only retained metadata, admitted by current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection: Option<crate::nodes::DeletedInspection>,
    /// The ordinary person's list projection for live readable text, never folder or retained content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<crate::documents::DocumentRef>,
    /// Ordinary participation from the same native decision as document; admin reach alone is not participation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participating: Option<bool>,
}

/// Structural ancestor with no hidden name disclosure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreeAncestor {
    /// Folder identity.
    pub id: Id,
    /// Immediate directory.
    pub parent: DocumentDirectory,
    /// Hierarchy anchor.
    pub node: Id,
    /// Root organization.
    pub root: Id,
    /// Exact component only when readable.
    pub basename: Option<String>,
    /// Label only when readable.
    pub title: Option<String>,
    /// Current authority.
    pub standing: DocumentTreeStanding,
}

/// A selected entry's current hierarchy anchor and safe path, without document content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreePlace {
    /// The actual containing place, named only as this person may know it.
    pub node: crate::nodes::NodeRef,
    /// Current hierarchy ancestry, root first and including the containing place.
    pub path: Vec<crate::nodes::NodeRef>,
}

/// One content-free navigation read, including its redacted breadcrumbs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreeRead {
    /// Readable selected entries.
    pub entries: Vec<DocumentTreeListed>,
    /// Required folder ancestors.
    pub ancestors: Vec<DocumentTreeAncestor>,
    /// Current containing place for one selected document/folder; absent for multi-entry receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<DocumentTreePlace>,
    /// Selected-folder context; absent for a document or multi-entry receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<DocumentDirectoryContext>,
    /// The exact ephemeral entry metadata view, when one entry was selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed: Option<String>,
}

/// The selected readable directory and its actual current creation decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentDirectoryContext {
    /// The selected root or distinct folder.
    pub at: DocumentDirectory,
    /// Person-redacted hierarchy anchor.
    pub node: crate::nodes::NodeRef,
    /// Person-redacted hierarchy ancestry.
    pub path: Vec<crate::nodes::NodeRef>,
    /// The selected folder; null for a virtual node docs root.
    pub folder: Option<DocumentTreeListed>,
    /// Required folder ancestors; link by parent identity, not array order.
    pub ancestors: Vec<DocumentTreeAncestor>,
    /// Native Create, docs.enabled, archive and actual actor scope all permit creation.
    pub can_create: bool,
    /// Always read-only when the selected directory is retained.
    pub inspection: Option<crate::nodes::DeletedInspection>,
    /// Structural scope revision at the same read cut; zero before its first entry.
    pub membership_seq: Seq,
}

/// An authorized bounded page, including context even when its candidate scan found no visible row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreePage {
    /// Only caller-visible entries.
    pub items: Vec<DocumentTreeListed>,
    /// Opaque candidate progress; a non-null cursor may follow an empty visible page.
    pub next_cursor: Option<String>,
    /// Subscribe to this exact requested page's current metadata.
    pub feed: String,
    /// Selected directory, safe ancestry and actual Create standing.
    pub directory: DocumentDirectoryContext,
    /// Required ancestors for this page's entries, including flat retained inventory paths.
    pub ancestors: Vec<DocumentTreeAncestor>,
}

/// Current document placement, independent of editable text and hierarchy-node ancestry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentLocation {
    /// Actual current immediate directory.
    pub parent: DocumentDirectory,
    /// Exact stored path component.
    pub basename: String,
    /// Each name independently projected for the person reading the document.
    pub ancestors: Vec<DocumentTreeAncestor>,
}

/// Exact complete deletion membership and authority, not a confirmation capability by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreeMembership {
    /// Every affected item, with no hidden or truncated tail.
    pub entries: Vec<DocumentTreeEntry>,
    /// Native authority/audience epoch.
    pub authority_epoch: i64,
    /// Hash of relevant page rows/entries.
    pub authority_fingerprint: String,
    /// Earliest expiring authority grant.
    #[schemars(with = "Option<Time>")]
    pub authority_until: Option<DateTime<Utc>>,
}

/// A complete authorized deletion preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreePlan {
    /// Exact current membership and authority.
    pub stamp: DocumentTreeMembership,
    /// Ordinary retention deadline.
    #[schemars(with = "Time")]
    pub restore_until: DateTime<Utc>,
    /// People gaining ordinary read access.
    pub gaining: i64,
    /// People losing ordinary read access.
    pub losing: i64,
}

/// A copied identity pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreeCopied {
    /// Original item.
    pub source: DocumentTreeItem,
    /// New item.
    pub destination: DocumentTreeItem,
}

/// The committed answer of a replay-safe document operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentTreeReceipt {
    /// Durable operation identity.
    pub operation: Id,
    /// Changed entries.
    pub entries: Vec<DocumentTreeEntry>,
    /// Whole-tree original-to-fresh map.
    pub copied: Vec<DocumentTreeCopied>,
    /// People gaining ordinary read access.
    pub gaining: i64,
    /// People losing ordinary read access.
    pub losing: i64,
    /// Retention deadline for deletion.
    #[schemars(with = "Option<Time>")]
    pub restore_until: Option<DateTime<Utc>>,
    /// Current person-visible rows and ancestry, never stored as personalized durable replay data.
    #[serde(default)]
    pub view: DocumentTreeRead,
}

/// Document organization commands; only Delete uses a separate preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentTreeCommand {
    /// Create one empty folder.
    CreateFolder,
    /// Copy current text/files and folder structure.
    Copy,
    /// Move identities with their history and files.
    Move,
    /// Rename a text document.
    Rename,
    /// Soft-delete a complete tree.
    Delete,
    /// Restore an exact deletion cohort.
    Restore,
    /// Restore one earlier text version.
    RestoreVersion,
}

/// One direct document operation or a deletion carrying its exact preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentTreeOperationBody {
    /// Command.
    pub command: DocumentTreeCommand,
    /// Typed selections; empty for CreateFolder.
    #[serde(default)]
    pub items: Vec<DocumentTreeItem>,
    /// Existing destination.
    pub to: Option<DocumentDirectory>,
    /// One explicit new basename/title.
    pub name: Option<String>,
    /// Actor-scoped request identity.
    pub client_id: String,
    /// Earlier version only for RestoreVersion.
    pub version: Option<i32>,
    /// Complete current preview only for Delete.
    pub deletion_plan: Option<DocumentTreeMembership>,
}

/// Folder creation in a node's document area; omission keeps the ordinary root placement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentContainerCreateBody {
    /// Existing folder in this node's document area, otherwise its docs root.
    pub parent_container_id: Option<Id>,
    /// New single component.
    pub name: String,
    /// Replay identity.
    pub client_id: String,
}

/// A typed entry read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentTreeKind {
    /// Text document.
    Document,
    /// Document-only folder.
    Container,
}

/// Content-free entry metadata and breadcrumbs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentTreeEntryGetQuery {
    /// Required typed kind.
    pub kind: DocumentTreeKind,
    /// Explicit read-only metadata inspection under current restoration authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_deleted: Option<bool>,
}

/// Immediate folder/root children, paged only after each entry's access check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentTreeListQuery {
    /// Existing folder in the node's document area; omitted for its docs root.
    pub parent_container_id: Option<Id>,
    /// Flat retained inventory within this document ancestry, not hierarchy descendants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<crate::documents::DeletedFilter>,
    /// Opaque internal candidate progress, including through empty visible pages.
    pub cursor: Option<String>,
    /// Number of visible entries; defaults to fifty and is clamped to one through five hundred.
    pub limit: Option<u16>,
}

crate::endpoints! {
    /// Lists the readable document and folder children beside a node or inside one folder.
    DocumentTreeList: GET "/nodes/{id}/document-tree", Rule("node.view on the root or doc.view on the specified folder; retained rows require current restore authority"), query DocumentTreeListQuery, none, json DocumentTreePage, 200;
    /// Reads a document folder's metadata and redacted ancestry without opening an editor.
    DocumentContainerGet: GET "/document-containers/{id}", Rule("doc.view on this document container; explicit retained inspection requires current restore authority"), query crate::nodes::DeletedInspectionQuery, none, json DocumentTreeRead, 200;
    /// Reads typed document metadata and redacted folder ancestry.
    DocumentTreeEntryGet: GET "/document-tree/entries/{id}", Rule("doc.view on the exact typed entry"), query DocumentTreeEntryGetQuery, none, json DocumentTreeRead, 200;
    /// Creates one empty document-only folder in an existing writable directory.
    DocumentContainerCreate: POST "/nodes/{id}/document-containers", Rule("destination doc.create with docs.enabled"), none, json DocumentContainerCreateBody, json DocumentTreeReceipt, 201;
    /// Creates ordinary text directly inside an existing writable document folder.
    DocumentContainerDocumentCreate: POST "/document-containers/{id}/documents", Rule("doc.create on this container with docs.enabled"), none, json crate::documents::NodeDocumentCreateBody, json crate::documents::Document, 201;
    /// Computes a complete authorized deletion preview without changing any document.
    DocumentTreeDeletePreview: POST "/document-tree/delete/preview", Rule("doc.manage on every affected entry"), none, json DocumentTreeOperationBody, json DocumentTreePlan, 200;
    /// Executes an atomic document operation, requiring a complete preview only for deletion.
    DocumentTreeOperate: POST "/document-tree/operations", Rule("current complete source and destination document authority"), none, json DocumentTreeOperationBody, json DocumentTreeReceipt, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entry_place_is_additive_redacted_metadata_without_document_content()
    -> Result<(), Box<dyn std::error::Error>> {
        let old = json!({"entries":[],"ancestors":[]});
        let mut read: DocumentTreeRead = serde_json::from_value(old.clone())?;
        assert!(read.place.is_none());
        assert_eq!(serde_json::to_value(&read)?, old);
        let node = crate::nodes::NodeRef {
            thread_display: None,
            id: Id::from_u128(1),
            kind: crate::nodes::NodeKind::Topic,
            name: None,
            redacted: true,
        };
        read.place = Some(DocumentTreePlace {
            node: node.clone(),
            path: vec![node],
        });
        let value = serde_json::to_value(&read)?;
        assert_eq!(value["place"].as_object().ok_or("place object")?.len(), 2);
        assert_eq!(value["place"]["node"]["redacted"], true);
        assert!(value["place"]["node"]["name"].is_null());
        assert_eq!(serde_json::from_value::<DocumentTreeRead>(value)?, read);
        Ok(())
    }

    #[test]
    fn folders_are_typed_and_complete_deletion_stamps_retain_database_timestamp_precision()
    -> Result<(), Box<dyn std::error::Error>> {
        let id = Id::from_u128(1);
        let literal = json!({"entries":[{"item":{"kind":"container","id":id},
            "parent":{"kind":"node_docs","id":Id::from_u128(2)},"node":Id::from_u128(2),"root":Id::from_u128(3),
            "basename":"Research","title":"Research","revision":7,"owner":Id::from_u128(4),
            "deleted_at":"2026-01-01T00:00:00.123456Z","deletion":Id::from_u128(5),"files":0,"versions":0}],
            "authority_epoch":12,"authority_fingerprint":"synthetic-state-fingerprint",
            "authority_until":"2026-01-02T00:00:00.654321Z"});
        let stamp: DocumentTreeMembership = serde_json::from_value(literal.clone())?;
        assert_eq!(serde_json::to_value(stamp)?, literal);
        assert!(
            serde_json::from_value::<DocumentDirectory>(json!({"kind":"document","id":id}))
                .is_err(),
            "editable text cannot be a directory"
        );
        assert!(
            serde_json::from_value::<DocumentTreeItem>(json!({"kind":"project","id":id})).is_err(),
            "a hierarchy node is never a document folder"
        );
        Ok(())
    }
}
