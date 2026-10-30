//! Exports: a chat taken away as a file, and one archive of what a person may read in a place, with the plan that says beforehand what it would hold and what stands in its way, the job that writes it, and the archive fetched until it is removed.
//!
//! A thread (`GET /threads/{id}/export`) and a document (`GET /documents/{id}/export`) are exported by
//! requests of their own, and so is a chat ([`ChatExportGet`]). Several things, or a whole place, are exported
//! as one archive: an [`ExportRequest`] is first asked what it would hold ([`NodeExportPreflight`], an
//! [`ExportPlan`]), then made ([`NodeExportCreate`], a job), and its [`Export`] is read, listed and fetched
//! until it is removed. An archive holds what its person could export one by one and names nothing else.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Page, PageQuery};
use crate::nodes::NodeRef;
use crate::routes::Access::{Permission, Rule, User};
use crate::threads::{ExportDetail, ExportFormat};
use crate::workspace::Job;

/// The most identifiers one list of an [`ExportChosen`] names: what a tree of ticks comes to in a large place,
/// and what the body of a request, a megabyte of JSON, holds.
pub const EXPORT_CHOSEN_MAX: u32 = 10_000;

/// The word that asks for everything in a place that the caller may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportEverything {
    /// Everything in the place that the caller may read.
    All,
}

/// Things chosen in the tree of a place, each with what is in it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportChosen {
    /// Places and threads inside the place, each with what is in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<Id>>,
    /// Documents written beside any of the nodes of the place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documents: Option<Vec<Id>>,
    /// Chats of the nodes of the place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chats: Option<Vec<Id>>,
}

/// What an archive is of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ExportWhat {
    /// Everything in the place that the caller may read.
    Everything(ExportEverything),
    /// These, each with what is in it.
    Chosen(ExportChosen),
}

/// Which kinds of thing an archive holds; all three unless given.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportParts {
    /// The conversations of threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversations: Option<bool>,
    /// The chats of places and of threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chats: Option<bool>,
    /// The documents written beside them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documents: Option<bool>,
}

/// An archive asked for: what it is of and how it is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    /// What it is of.
    pub what: ExportWhat,
    /// Which kinds; all three unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<ExportParts>,
    /// `markdown` unless given: the written forms. `json`: the forms the single exports give.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ExportFormat>,
    /// How much of a conversation; `normal` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<ExportDetail>,
    /// The files that inputs, messages and documents carry; `false` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<bool>,
    /// The earlier versions of documents; `false` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<bool>,
    /// The discussions of documents; `false` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<bool>,
    /// A UTC day, `YYYY-MM-DD`: only the turns and the messages of that day and after; documents are written as
    /// they stand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// A UTC day, `YYYY-MM-DD`: only the turns and the messages of that day and before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// An Orch package, which can be imported again: beside the readable form of each thing its record as JSON,
    /// the files and the discussions of documents, and at the root a manifest of everything in it
    /// ([`crate::package`]); `false` unless given. `format` has no say then, because the records are JSON and the
    /// readable forms Markdown; files and discussions are in it unless they are asked off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<bool>,
}

/// How many of each kind an archive holds, or would.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportCounts {
    /// Places that have a directory in it.
    pub places: i64,
    /// Conversations.
    pub conversations: i64,
    /// Chats.
    pub chats: i64,
    /// Documents.
    pub documents: i64,
    /// Files that inputs, messages and documents carry.
    pub files: i64,
}

/// What the place allows of one archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportLimits {
    /// The most bytes: `export.max_bytes` as it resolves at the place.
    pub max_bytes: i64,
    /// The most conversations, chats and documents together: `export.max_items`.
    pub max_items: i64,
    /// For how many hours a finished archive is kept: `export.keep_hours`.
    pub keep_hours: i64,
}

/// Why something that was asked for is not in an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportLeftOutWhat {
    /// The caller may read it and holds no Export row for it.
    NotExportable,
    /// A file larger than the largest the system takes into an export.
    TooLarge,
}

/// Things that are not in an archive, by the place they lie in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportLeftOut {
    /// Why.
    pub what: ExportLeftOutWhat,
    /// How many.
    pub count: i64,
    /// The place they lie in; `null` where they lie in none that is named. What the caller may not read is
    /// no part of any figure here.
    #[serde(rename = "where")]
    pub place: Option<NodeRef>,
}

/// Something that would stop an archive as it is asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportBlocker {
    /// The code of the refusal that asking would get: `export.too_large`, `export.too_many` or
    /// `export.nothing`.
    pub code: String,
    /// What it comes to, in a sentence.
    pub message: String,
}

/// What an archive would hold, as far as the caller may read it, and what stands in its way. Nothing is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportPlan {
    /// The place it is asked of.
    pub node: NodeRef,
    /// How many of each kind it would hold.
    pub counts: ExportCounts,
    /// An estimate of its size in bytes, from what is stored.
    pub bytes: i64,
    /// What the place allows.
    pub limits: ExportLimits,
    /// What the caller may read and not take away, and the files past a size: by the place they lie in.
    pub left_out: Vec<ExportLeftOut>,
    /// Not empty: it would be refused as it is asked.
    pub blockers: Vec<ExportBlocker>,
}

/// Where an archive stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportState {
    /// A job is writing it.
    Pending,
    /// It can be fetched.
    Ready,
    /// The job failed, or was cancelled; nothing was kept.
    Failed,
    /// It was removed, by its person or when its time ran out; the row stays for a month.
    Expired,
}

/// An archive of what a person may read in a place: theirs alone to fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Export {
    /// The archive's identifier.
    pub id: Id,
    /// The place it was made of; `null` once the place was purged, when the archive stays its person's until its
    /// time.
    pub node: Option<NodeRef>,
    /// As it was asked.
    pub request: ExportRequest,
    /// Where it stands.
    pub state: ExportState,
    /// The job that writes it.
    pub job_id: Option<Id>,
    /// Its size once it is ready.
    pub bytes: Option<i64>,
    /// How many of each kind it holds, once it is ready.
    pub counts: Option<ExportCounts>,
    /// What was asked for and is not in it, each with why.
    pub left_out: Vec<ExportLeftOut>,
    /// When it was asked for.
    pub created_at: Time,
    /// When it became ready.
    pub ready_at: Option<Time>,
    /// When it is removed.
    pub expires_at: Option<Time>,
    /// When it was first fetched.
    pub downloaded_at: Option<Time>,
}

/// What `result` is on the job that writes an archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportJobResult {
    /// The archive.
    pub export_id: Id,
}

/// Whether the files that a chat's messages carry go with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatExportFiles {
    /// The chat alone, the files named where they stood.
    Omit,
    /// An archive of the chat and the files beside it.
    Include,
}

/// How a chat is exported.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatExportGetQuery {
    /// `markdown` unless given: the chat in its written form. `json`: its messages as records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ExportFormat>,
    /// The first message, by its number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    /// The last message, by its number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
    /// `omit` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<ChatExportFiles>,
}

crate::endpoints! {
    /// Exports a chat as a file: its written form or its messages as records, and with its files an archive of them.
    ChatExportGet: GET "/chats/{id}/export", Rule("chat.export on its node, with chat.view; or being in the direct or group chat"), query ChatExportGetQuery, none, raw, 200;
    /// Says what an archive of a place would hold as far as the caller may read it, and what stands in its way. Nothing is made.
    NodeExportPreflight: POST "/nodes/{id}/exports/preflight", Permission("node.view"), none, json ExportRequest, json ExportPlan, 200;
    /// Asks for an archive of what the caller may read in a place, written by a job; each thing in it is written by the caller's Export row.
    NodeExportCreate: POST "/nodes/{id}/exports", Permission("node.view"), none, json ExportRequest, json Job, 202;
    /// Lists the caller's own archives, newest first.
    MeExportList: GET "/me/exports", User, query PageQuery, none, json Page<Export>, 200;
    /// Reads one of the caller's archives.
    ExportGet: GET "/exports/{id}", Rule("its person"), none, none, json Export, 200;
    /// Fetches an archive, as a zip named for the place and the day; it can be fetched again until it is removed.
    ExportDownload: GET "/exports/{id}/download", Rule("its person, in a browser session or with a token of theirs"), none, none, raw, 200;
    /// Removes an archive now; a job still writing it is cancelled.
    ExportDelete: DELETE "/exports/{id}", Rule("its person"), none, none, none, 204;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn everything_in_a_place_is_asked_with_one_word_and_a_choice_with_the_ids() {
        let all: ExportRequest = serde_json::from_value(json!({ "what": "all" })).unwrap();
        assert_eq!(all.what, ExportWhat::Everything(ExportEverything::All));
        assert_eq!(all.parts, None);
        assert_eq!(all.format, None, "markdown, unless given");
        let chosen: ExportRequest = serde_json::from_value(json!({
            "what": { "nodes": ["0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01"], "documents": [] },
            "parts": { "chats": false },
            "format": "json",
            "detail": "full",
            "files": true,
            "versions": true,
            "comments": true,
            "from": "2026-10-01",
            "to": "2026-10-09",
            "package": true
        }))
        .unwrap();
        let ExportWhat::Chosen(what) = &chosen.what else {
            panic!("ids are a choice");
        };
        assert_eq!(what.nodes.as_ref().map(Vec::len), Some(1));
        assert_eq!(what.chats, None);
        assert_eq!(
            chosen.parts,
            Some(ExportParts {
                conversations: None,
                chats: Some(false),
                documents: None
            })
        );
        assert_eq!(chosen.format, Some(ExportFormat::Json));
        assert_eq!(chosen.detail, Some(ExportDetail::Full));
        assert_eq!(chosen.from.as_deref(), Some("2026-10-01"));
        assert_eq!(chosen.package, Some(true));
        assert_eq!(all.package, None, "a package only where it is asked for");
        assert_eq!(
            serde_json::to_value(&all).unwrap(),
            json!({ "what": "all" }),
            "what was left out is not written"
        );
    }

    #[test]
    fn a_request_with_a_member_nobody_defined_or_a_word_that_is_no_choice_is_refused() {
        assert!(
            serde_json::from_value::<ExportRequest>(json!({ "what": "all", "zip": true })).is_err()
        );
        assert!(serde_json::from_value::<ExportRequest>(json!({ "what": "everything" })).is_err());
        assert!(
            serde_json::from_value::<ExportRequest>(json!({ "what": { "places": [] } })).is_err()
        );
        assert!(serde_json::from_value::<ExportRequest>(json!({})).is_err());
    }

    #[test]
    fn what_was_left_out_says_where_in_a_member_that_is_a_word_of_the_language() {
        let left: ExportLeftOut = serde_json::from_value(json!({
            "what": "not_exportable", "count": 9,
            "where": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "topic", "name": "Fix login", "redacted": false }
        }))
        .unwrap();
        assert_eq!(left.what, ExportLeftOutWhat::NotExportable);
        assert_eq!(left.count, 9);
        assert_eq!(
            left.place.as_ref().and_then(|place| place.name.as_deref()),
            Some("Fix login")
        );
        let written = serde_json::to_value(&left).unwrap();
        assert!(written.get("where").is_some() && written.get("place").is_none());
        let nowhere: ExportLeftOut =
            serde_json::from_value(json!({ "what": "too_large", "count": 1, "where": null }))
                .unwrap();
        assert!(nowhere.place.is_none());
    }

    #[test]
    fn an_archive_that_is_not_ready_yet_has_no_size_and_no_counts() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b01",
            "node": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "project", "name": "Web", "redacted": false },
            "request": { "what": "all" },
            "state": "pending",
            "job_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5c01",
            "bytes": null, "counts": null, "left_out": [],
            "created_at": "2026-10-12T09:00:00.000Z",
            "ready_at": null, "expires_at": null, "downloaded_at": null
        });
        let export: Export = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(export.state, ExportState::Pending);
        assert!(export.bytes.is_none() && export.counts.is_none());
        assert_eq!(serde_json::to_value(&export).unwrap(), written);
    }
}
