//! Sharing with agents: copies, live shares by area to read or to write (for documents to edit, which on a place also makes them), of a thing, of a place with what is in it or of a place by itself, taking back, and an agent's reach as a tree whose entries say what they are and how they are counted.
//!
//! People are shared with on the page of what is shared. Agents are shared
//! with by [`ShareAgentCreate`], in one of two modes: a copy (`snapshot`),
//! written once as Markdown files into the thread's directory at its next
//! turn, and a live share, which is entries on the page of what is shared,
//! each with a holder of kind `agents`, so that the agents find it in their
//! tree and work in it as it is at each call. A share covers up to three
//! areas apart, the agents' conversations, people's chats and documents,
//! each to read or to write ([`ShareParts`]); for documents a third step, to
//! manage, adds deleting, moving and bringing back. [`SharedEntry`] is one entry of
//! an agent's reach as its own tools list it.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::Entry;
use crate::chats::ChatRef;
use crate::documents::DocumentRef;
use crate::nodes::NodeRef;
use crate::routes::Access::{Permission, Rule};
use crate::threads::{ShareMode, ThreadInput};
use crate::users::UserRef;

/// The most threads or places one share names.
pub const SHARE_MAX_DESTINATIONS: u32 = 10;
/// The shallowest a tree of an agent's reach may be asked for.
pub const SHARED_TREE_DEPTH_MIN: u32 = 1;
/// The deepest a tree of an agent's reach may be asked for.
pub const SHARED_TREE_DEPTH_MAX: u32 = 4;
/// The path a tree of an agent's reach starts at when it is not told.
pub const SHARED_TREE_ROOT: &str = "/";

/// How far an area of conversations or of chats is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShareLevel {
    /// The agents read it.
    Read,
    /// They also write: send to those threads, or post in those chats.
    Write,
}

/// How far documents are shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShareDocumentsLevel {
    /// The agents read them, with their files and versions.
    Read,
    /// On a document: they change it and put files into it. On a node: the same for the documents
    /// beside it that follow its row, and they make new ones there, as writing a directory lets one
    /// make a file in it.
    Edit,
    /// What `edit` gives, and the documents themselves: they delete them, move them to another
    /// place, bring a deleted one back and put an earlier version back. The act is the agent's one
    /// tool for it, which first says what would happen and does it only when it is asked again.
    /// It adds nothing for conversations and chats: no agent removes a thread, a chat or a place.
    Manage,
}

/// The three areas of a share, shared apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ShareParts {
    /// The agents' conversations; with `write`, also sending to those threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<ShareLevel>,
    /// People's chats; with `write`, also posting there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<ShareLevel>,
    /// Documents, with their files and versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documents: Option<ShareDocumentsLevel>,
}

/// A place, which is an org, a project, a topic or a thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareWhatNode {
    /// The place.
    pub node_id: Id,
}

/// A chat of any kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareWhatChat {
    /// The chat.
    pub chat_id: Id,
}

/// One document.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareWhatDocument {
    /// The document.
    pub document_id: Id,
}

/// What is shared.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ShareWhat {
    /// A place: an org, a project, a topic or a thread. A whole project can be shared as knowledge
    /// with every agent of another project, a topic or an org.
    Node(ShareWhatNode),
    /// A chat.
    Chat(ShareWhatChat),
    /// A document.
    Document(ShareWhatDocument),
}

/// A thread whose agent is shared with.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareToThread {
    /// The thread.
    pub thread_id: Id,
}

/// A place whose agents are shared with: every agent thread under it, those created later included.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareToNode {
    /// The topic, project or org.
    pub node_id: Id,
}

/// Whom a share is for: at most [`SHARE_MAX_DESTINATIONS`] threads, or for a live share a topic, a
/// project or an org each. It may be the place that is shared: a topic's documents with that
/// topic's own agents.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ShareTo {
    /// Threads; a copy can only go to these.
    Threads(Vec<ShareToThread>),
    /// Places.
    Nodes(Vec<ShareToNode>),
}

/// Shares a thing with agents. Both modes are all or nothing over `to`, and both are audited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareWithAgents {
    /// Marks included document material for first reading; never grants additional access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_first: Option<bool>,
    /// What is shared.
    pub what: ShareWhat,
    /// For a live share of a place: `true` unless given, and the share is of the place with what
    /// is in it. `false`: the place itself, its own chat and the documents beside it, and nothing
    /// inside; then `parts.timeline` is refused, a place having no conversation of its own. For
    /// one thing it means nothing and is left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<bool>,
    /// Whether the agents may also copy what is shared into their workspace (`download_shared`); `true`
    /// unless given. It names them in the Export rows of each area it covers as well, and so takes the
    /// caller's own Export; a copy (`snapshot`) takes it in any case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<bool>,
    /// Which areas, and each to read or to write. For a node, `{ timeline: 'read' }` unless given.
    /// A chat is `chat`, a document `documents`: only that member counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<ShareParts>,
    /// A copy, which holds no `write`, or a live share.
    pub mode: ShareMode,
    /// Whom it is for.
    pub to: ShareTo,
    /// For a copy: also sent to the agent as an input by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// For a live share: when it ends by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// A client-generated value that makes a repeated request safe.
    pub client_id: String,
}

/// What an agent share is of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[expect(
    clippy::large_enum_variant,
    reason = "a share is built once to be written or read; boxing the document would put a layer in every use"
)]
pub enum AgentShareWhat {
    /// A document.
    Document(DocumentRef),
    /// A chat.
    Chat(ChatRef),
    /// A place.
    Node(NodeRef),
}

/// Where a copy stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentShareSnapshotState {
    /// It waits for the thread's next turn.
    Pending,
    /// It was written into the thread's directory.
    Delivered,
    /// It was taken back.
    Revoked,
}

/// A copy that was made of what was shared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentShareSnapshot {
    /// Where it is written, in the thread's directory.
    pub path: String,
    /// How many files it holds.
    pub files: i64,
    /// Their size in bytes.
    pub bytes: i64,
    /// What moment it shows.
    pub as_of: Time,
    /// Where it stands.
    pub state: AgentShareSnapshotState,
}

/// A copy or a live share of something with the agent of a thread or of a place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentShare {
    /// Current first-reading intent carried by the share's document material.
    #[serde(default)]
    pub read_first: bool,
    /// The share's identifier.
    pub id: Id,
    /// What is shared.
    pub what: AgentShareWhat,
    /// The thread or place whose agents it is for.
    pub to: NodeRef,
    /// A copy or a live share.
    pub mode: ShareMode,
    /// For a live share of a place: whether it reaches what lies inside it. `false` for one thing,
    /// for a copy and for a place shared by itself.
    pub inside: bool,
    /// Whether the agents may copy what is shared into their workspace: they are named in the Export rows
    /// of the areas it covers.
    pub download: bool,
    /// The areas.
    pub parts: ShareParts,
    /// For a live share: the entries on the page that carry it.
    pub entries: Vec<Entry>,
    /// For a copy: what was written.
    pub snapshot: Option<AgentShareSnapshot>,
    /// For a copy with a comment: the input that was queued.
    pub input: Option<ThreadInput>,
    /// Who shared it.
    pub by: UserRef,
    /// When.
    pub at: Time,
}

/// What an entry of an agent's reach is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SharedEntryKind {
    /// An org.
    Org,
    /// A project.
    Project,
    /// A topic.
    Topic,
    /// A thread.
    Thread,
    /// A thread's conversation.
    Conversation,
    /// A chat.
    Chat,
    /// A folder of files.
    Folder,
    /// A document.
    Document,
    /// A version of a document.
    Version,
    /// A file.
    File,
}

/// What an entry's parts are counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SharedEntryUnit {
    /// Turns, for a conversation.
    Turn,
    /// Messages, for a chat.
    Message,
    /// Blocks, for a document.
    Block,
    /// Lines, for a text file.
    Line,
}

/// Why an agent reaches an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SharedEntryVia {
    /// The policy of its topic or project lets it.
    Policy,
    /// A share names it.
    Share,
    /// It lies in the agent's own topic.
    Topic,
}

/// One entry of an agent's reach, as its tools list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharedEntry {
    /// A readable thread's icon and safe particulars; absent for other kinds or without thread View.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_display: Option<crate::threads::ThreadDisplay>,
    /// Where it stands in the tree.
    pub path: String,
    /// Its name; `null` when the caller may not see it.
    pub name: Option<String>,
    /// Its identifier.
    pub id: Id,
    /// What it is.
    pub kind: SharedEntryKind,
    /// For a place, a thread and a document: the opening of its description; for a document that has
    /// none, of its first paragraph.
    pub description: Option<String>,
    /// What it is counted in, for a conversation, a chat, a document and a text file.
    pub unit: Option<SharedEntryUnit>,
    /// How many of that unit it has.
    pub parts: Option<i64>,
    /// Its size in bytes.
    pub size: Option<i64>,
    /// When it last changed.
    pub updated_at: Option<Time>,
    /// How many entries lie directly under it.
    pub children: Option<i64>,
    /// Whether the agent can read it.
    pub readable: bool,
    /// Whether the agent can write it.
    pub writable: bool,
    /// For a document: whether the agent may delete it, move it and bring it back. For one of its
    /// earlier versions: whether the agent may put it back. For the `docs` directory of a place:
    /// whether it may do that to every document that stands directly in it.
    pub manageable: bool,
    /// Whether it is a document that was deleted and not yet purged, listed to an agent that may
    /// bring it back. Such a document can be neither read nor written.
    pub deleted: bool,
    /// Why the agent reaches it.
    pub via: SharedEntryVia,
    /// Who shared it, where a share names it.
    pub shared_by: Option<UserRef>,
    /// Whether the caller may not see it themself: no name, no path below it.
    pub redacted: bool,
}

/// Which side of a share is asked about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentShareSide {
    /// The live shares and the copies of that node, document or chat.
    #[default]
    Of,
    /// For a node: what was shared with the agents of that place, or of a place above it, as far as
    /// the caller may see each thing.
    To,
}

/// Which shares of a thing to list. `{object}` is `nodes`, `documents` or `chats`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentShareListQuery {
    /// `of` unless given, or for a node `to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<AgentShareSide>,
}

/// What was shared, one share for each thread or place named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShareAgentCreateReply {
    /// The shares.
    pub shares: Vec<AgentShare>,
}

/// How far the agent of a thread reaches beyond its own thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadSharedPolicy {
    /// Nothing but its own thread.
    None,
    /// Its topic.
    Topic,
    /// Its project.
    Project,
}

/// What a thread's agent may read beyond its own thread, and the copies it was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSharedGetReply {
    /// The independent public topic-document policy.
    pub documents_policy: ThreadDocumentPolicy,
    /// Current first-reading paths, with no document bodies.
    pub read_first_paths: Vec<ReadFirstPath>,
    /// How far it reaches by policy.
    pub policy: ThreadSharedPolicy,
    /// The shares that name it.
    pub shares: Vec<AgentShare>,
}

/// Public topic/main-thread document reach, independently of conversation sharing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadDocumentPolicy {
    /// No implicit document access.
    None,
    /// Read public material.
    Read,
    /// Also edit and create public material.
    Write,
}

/// Material resolved from stable source and recipient identities at the current moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadFirstPath {
    /// The live source or delivered copy identifier.
    pub id: Id,
    /// The current tool path, or verified delivered copy index.
    pub path: String,
}

/// Where in the tree of an agent's reach to look.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadSharedTreeGetQuery {
    /// The path to start at; [`SHARED_TREE_ROOT`] unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How many levels, from [`SHARED_TREE_DEPTH_MIN`] to [`SHARED_TREE_DEPTH_MAX`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
}

/// An agent's reach as the tree its own tools list, so that a person sees exactly what the agent can
/// read and write. An entry the caller may not see themself comes without its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadSharedTreeGetReply {
    /// Where it starts.
    pub path: String,
    /// The entries.
    pub entries: Vec<SharedEntry>,
    /// Whether there are more than were listed.
    pub truncated: bool,
}

/// Changes a live share; what is left out stays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareAgentUpdateBody {
    /// Changes the material hint on included document material.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_first: Option<bool>,
    /// New areas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<ShareParts>,
    /// When it ends by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
}

crate::endpoints! {
    /// Shares a thing with agents, as a copy or as a live share, all or nothing over the threads or places named.
    ShareAgentCreate: POST "/share/agents", Rule("for a copy, read access to everything included and thread.interact on each thread named; for a live share, holding oneself what each area is shared for and being an owner of what is shared or a manager of its page, or the place letting holders share, or being in the chat, and thread.interact on each thread named or node.interact on the place named"), none, json ShareWithAgents, json ShareAgentCreateReply, 201;
    /// Lists the live shares and the copies of a node, a document or a chat, or what was shared with the agents of a place.
    AgentShareList: GET "/{object}/{id}/agent-shares", Rule("node.view on a node, doc.view on a document, or being in the chat"), query AgentShareListQuery, none, json Vec<AgentShare>, 200;
    /// Says what a thread's agent may read beyond its own thread, and the copies it was given.
    ThreadSharedGet: GET "/threads/{id}/shared", Permission("thread.view"), none, none, json ThreadSharedGetReply, 200;
    /// Lists an agent's reach as the tree its own tools list, so that a person sees exactly what the agent can read and write.
    ThreadSharedTreeGet: GET "/threads/{id}/shared/tree", Permission("thread.view"), query ThreadSharedTreeGetQuery, none, json ThreadSharedTreeGetReply, 200;
    /// Changes the areas or the end of a live share.
    ShareAgentUpdate: PATCH "/share/agents/{share_id}", Rule("as for creating it"), none, json ShareAgentUpdateBody, json AgentShare, 200;
    /// Takes a share back: a live share ends at the agent's next call, a copy not yet delivered is cancelled and one delivered is removed from the workspace.
    ShareAgentDelete: DELETE "/share/agents/{share_id}", Rule("whoever shared it, or whoever could share the same thing now"), none, none, none, 204;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user() -> serde_json::Value {
        json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "human", "username": "bo",
            "display_name": "Bo", "avatar": { "text": "B", "color": 2 }
        })
    }

    #[test]
    fn a_whole_project_is_shared_live_with_every_agent_of_another_place() {
        let body: ShareWithAgents = serde_json::from_value(json!({
            "what": { "node_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01" },
            "parts": { "timeline": "read", "documents": "edit" },
            "mode": "live",
            "to": [{ "node_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d02" }],
            "expires_at": "2026-12-01T00:00:00.000Z",
            "client_id": "7b7a4c1e-0f0e-4d57-9d5e-6f3a9a8b2c11"
        }))
        .unwrap();
        assert!(matches!(body.what, ShareWhat::Node(_)));
        assert!(matches!(&body.to, ShareTo::Nodes(places) if places.len() == 1));
        assert_eq!(body.mode, ShareMode::Live);
        assert_eq!(
            body.parts,
            Some(ShareParts {
                timeline: Some(ShareLevel::Read),
                chat: None,
                documents: Some(ShareDocumentsLevel::Edit)
            })
        );
    }

    #[test]
    fn a_place_is_shared_by_itself_when_the_share_says_it_is_not_inside() {
        let alone: ShareWithAgents = serde_json::from_value(json!({
            "what": { "node_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01" },
            "inside": false,
            "parts": { "chat": "read", "documents": "read" },
            "mode": "live",
            "to": [{ "node_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d02" }],
            "client_id": "c"
        }))
        .unwrap();
        assert_eq!(alone.inside, Some(false));
        let with_what_is_in_it: ShareWithAgents = serde_json::from_value(json!({
            "what": { "node_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01" },
            "mode": "live",
            "to": [{ "node_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d02" }],
            "client_id": "c"
        }))
        .unwrap();
        assert_eq!(
            with_what_is_in_it.inside, None,
            "left out, it is of the place with what is in it"
        );
        assert!(
            serde_json::to_value(&with_what_is_in_it)
                .unwrap()
                .get("inside")
                .is_none(),
            "what was left out is not written"
        );
    }

    #[test]
    fn a_copy_of_a_chat_goes_to_threads() {
        let body: ShareWithAgents = serde_json::from_value(json!({
            "what": { "chat_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01" },
            "mode": "snapshot",
            "to": [{ "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d03" }],
            "comment": "Please read this first",
            "client_id": "c"
        }))
        .unwrap();
        assert!(matches!(body.what, ShareWhat::Chat(_)));
        assert!(matches!(body.to, ShareTo::Threads(_)));
        assert!(
            serde_json::from_value::<ShareWithAgents>(json!({
                "what": { "chat_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01", "node_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d02" },
                "mode": "live", "to": [], "client_id": "c"
            }))
            .is_err(),
            "a share is of one thing"
        );
    }

    #[test]
    fn a_share_says_what_it_is_of_and_where_its_copy_stands() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e01",
            "what": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e02", "kind": "group", "org_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e03",
                      "node": null, "name": "Release" },
            "to": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e04", "kind": "thread", "name": "Fix the build", "redacted": false },
            "mode": "snapshot",
            "inside": false,
            "download": false,
            "read_first": false,
            "parts": { "chat": "read" },
            "entries": [],
            "snapshot": { "path": ".orch/shared/0199d1a8/", "files": 3, "bytes": 2048,
                          "as_of": "2026-10-08T17:04:05.123Z", "state": "pending" },
            "input": null,
            "by": user(),
            "at": "2026-10-08T17:04:05.123Z"
        });
        let share: AgentShare = serde_json::from_value(written.clone()).unwrap();
        assert!(matches!(share.what, AgentShareWhat::Chat(_)));
        assert_eq!(
            share.snapshot.as_ref().map(|copy| copy.state),
            Some(AgentShareSnapshotState::Pending)
        );
        assert_eq!(serde_json::to_value(&share).unwrap(), written);
    }

    #[test]
    fn documents_are_shared_to_read_to_edit_or_to_manage_and_only_documents_go_so_far() {
        for (word, level) in [
            ("read", ShareDocumentsLevel::Read),
            ("edit", ShareDocumentsLevel::Edit),
            ("manage", ShareDocumentsLevel::Manage),
        ] {
            let parts: ShareParts =
                serde_json::from_value(json!({ "documents": word, "chat": "write" })).unwrap();
            assert_eq!(parts.documents, Some(level));
            assert_eq!(
                serde_json::to_value(parts).unwrap()["documents"],
                json!(word)
            );
        }
        assert!(
            serde_json::from_value::<ShareParts>(json!({ "chat": "manage" })).is_err(),
            "a chat is read or written, never managed"
        );
        assert!(
            serde_json::from_value::<ShareParts>(json!({ "timeline": "manage" })).is_err(),
            "nor is a conversation"
        );
    }

    #[test]
    fn a_deleted_document_is_listed_as_manageable_and_deleted_to_the_agent_that_may_bring_it_back()
    {
        let entry: SharedEntry = serde_json::from_value(json!({
            "path": "/acme/Web/Fix login/docs/Plan.md", "name": "Plan.md",
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5f02", "kind": "document", "description": null,
            "unit": null, "parts": null, "size": null, "updated_at": null, "children": null,
            "readable": false, "writable": false, "manageable": true, "deleted": true,
            "via": "share", "shared_by": null, "redacted": false
        }))
        .unwrap();
        assert!(entry.manageable && entry.deleted && !entry.readable);
        assert_eq!(entry.kind, SharedEntryKind::Document);
    }

    #[test]
    fn an_entry_the_caller_may_not_see_comes_without_its_name() {
        let entry: SharedEntry = serde_json::from_value(json!({
            "path": "/project/topic", "name": null, "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5f01",
            "kind": "topic", "description": null, "unit": null, "parts": null, "size": null,
            "updated_at": null, "children": 4, "readable": true, "writable": false,
            "manageable": false, "deleted": false,
            "via": "share", "shared_by": null, "redacted": true
        }))
        .unwrap();
        assert!(entry.redacted && entry.name.is_none());
        assert_eq!(entry.kind, SharedEntryKind::Topic);
    }
}
