//! Transcript and node search; and the finder's names: places, people, resources, documents and threads from a few letters.
//!
//! [`SearchList`] searches what was written: the items of the threads the
//! caller may view, the messages of the chats they may read, and the
//! documents they may read, newest first by default, each [`SearchHit`] with a snippet
//! around the first match. [`NameSearchList`] is the quick way to anything
//! by its name: a [`NameHit`] finds a thing and says where it lies; what is
//! written in it is for [`SearchList`].

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::chats::{ChatMessage, ChatRef};
use crate::common::Page;
use crate::documents::{Author, DocumentRef};
use crate::nodes::{NodeRef, Tag};
use crate::routes::Access::User;
use crate::threads::Item;
use crate::users::{Avatar, UserRef};

/// The fewest characters of a search of what was written.
pub const SEARCH_QUERY_MIN_CHARS: u32 = 2;
/// The fewest characters of a search that can use the index; a shorter one is accepted only with a
/// scope that is a topic or a thread.
pub const SEARCH_INDEXED_MIN_CHARS: u32 = 3;
/// The most characters of the snippet around the first match.
pub const SEARCH_SNIPPET_MAX_CHARS: u32 = 240;
/// How many names a search for names returns when it is not told.
pub const NAME_SEARCH_LIMIT_DEFAULT: u32 = 20;
/// The most names a search for names returns.
pub const NAME_SEARCH_LIMIT_MAX: u32 = 50;

/// What a search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "a hit is built once to be written; boxing the item would put a layer in every use"
)]
pub enum SearchFound {
    /// A readable place's name, offered by relevance search before matching content.
    Name {
        /// The named place.
        node: NodeRef,
        /// Its authorized path, including itself.
        path: Vec<NodeRef>,
        /// Its address from the same builder as Names and Everything.
        href: String,
        /// Its recorded activity/update time.
        at: Time,
    },
    /// An item of a thread's timeline.
    Item {
        /// Exact history containing the returned frozen body; opening it does not select execution.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projection: Option<crate::threads::ThreadContinuationProjection>,
        /// The item.
        item: Item,
        /// The thread.
        thread: NodeRef,
        /// The topic it lies in.
        topic: NodeRef,
        /// The project it lies in.
        project: NodeRef,
    },
    /// A message of a chat.
    ChatMessage {
        /// The message.
        message: ChatMessage,
        /// The chat.
        chat: ChatRef,
        /// The way down from the root org to the chat's node; empty for a direct and a group chat.
        path: Vec<NodeRef>,
    },
    /// A document: its title, the name of a file it holds, or one block.
    Document {
        /// The matched file, or a current file selected by a file filter; its normal owner route opens it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<crate::documents::DocumentFile>,
        /// The document.
        document: DocumentRef,
        /// The block that matched; `null` when the title or a file's name did.
        block_id: Option<Id>,
        /// The actual matching block or file's writer; header attribution for a title match.
        #[serde(default)]
        by: Option<Author>,
        /// The way down from the root org to the document's node.
        path: Vec<NodeRef>,
    },
}

/// The text field represented by a result's excerpt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchField {
    /// A place's name.
    Name,
    /// A document's title.
    DocumentTitle,
    /// A place's or document's description.
    Description,
    /// A document heading.
    DocumentHeading,
    /// The name of a file held by a document.
    FileName,
    /// A paragraph, message or item; its result kind identifies the text.
    Body,
}

/// One hit of a search, with the words around the first match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    /// Native reach alone opens the containing object; the same marker as its page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<crate::access::AdminAccess>,
    /// The server's source field for the excerpt, independent of display wording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_field: Option<SearchMatchField>,
    /// What was found.
    #[serde(flatten)]
    pub found: SearchFound,
    /// At most [`SEARCH_SNIPPET_MAX_CHARS`] characters around the first match.
    pub snippet: String,
    /// The character ranges of the matches inside the snippet, as start and end.
    pub matches: Vec<(u32, u32)>,
}

/// What a name can be of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NameKind {
    /// An org, at any level.
    Org,
    /// A project.
    Project,
    /// A topic.
    Topic,
    /// A thread.
    Thread,
    /// A document.
    Document,
    /// A distinct document-only folder, never a hierarchy project or text document.
    DocumentContainer,
    /// A chat.
    Chat,
    /// A readable topic label, with its defining place.
    Tag,
    /// A person.
    User,
    /// A group of people.
    Group,
    /// A provider.
    Provider,
    /// An agent profile.
    Agent,
    /// An environment.
    Environment,
    /// A host.
    Host,
    /// A template.
    Template,
    /// An MCP server.
    McpServer,
    /// A mount.
    Mount,
    /// A machine.
    Machine,
    /// A skill.
    Skill,
}

impl NameKind {
    /// The stable kind used by saved identities and the name index.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Project => "project",
            Self::Topic => "topic",
            Self::Thread => "thread",
            Self::Document => "document",
            Self::DocumentContainer => "document_container",
            Self::Chat => "chat",
            Self::Tag => "tag",
            Self::User => "user",
            Self::Group => "group",
            Self::Provider => "provider",
            Self::Agent => "agent",
            Self::Environment => "environment",
            Self::Host => "host",
            Self::Template => "template",
            Self::McpServer => "mcp_server",
            Self::Mount => "mount",
            Self::Machine => "machine",
            Self::Skill => "skill",
        }
    }
}

/// A thing found by its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NameHit {
    /// A readable thread's icon and safe particulars; absent for other kinds or without thread View.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_display: Option<crate::threads::ThreadDisplay>,
    /// Its canonical web address, shared with Everything; absent only for names that need a chooser/card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// The thing's current state, where its kind has one; never inferred from a name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Last recorded activity, using the existing update time where the kind has no activity clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<Time>,
    /// An ordinary assignment here or below, or a follow, from the visibility frame.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub participating: bool,
    /// This exact kind/id is starred by the caller.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub starred: bool,
    /// Native reach alone opens this result; the same marker as its page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<crate::access::AdminAccess>,
    /// A thread's observed nullable flags; null for every other kind of result.
    #[serde(default)]
    pub thread_attention: Option<crate::source::ThreadAttention>,
    /// What it is.
    pub kind: NameKind,
    /// Its identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// A tag's current definition; null for every other kind of name.
    #[serde(default)]
    pub tag: Option<Tag>,
    /// The palette colour of a place's icon; null for anything that is not a place or uses its kind's colour.
    #[serde(default)]
    pub color: Option<u32>,
    /// A place's own icon colour, in place of the palette; null for other kinds of result.
    #[serde(default)]
    pub custom_color: Option<String>,
    /// A person's username; absent for other kinds of thing.
    pub username: Option<String>,
    /// A person's avatar; absent for other kinds of thing.
    pub avatar: Option<Avatar>,
    /// A person's, an org's or a resource's address.
    pub address: Option<String>,
    /// For a node, a document and a node's chat: the way down from the root org to its place.
    pub path: Vec<NodeRef>,
    /// For a resource and a document: its first owner.
    pub owner: Option<UserRef>,
    /// Whether it is archived.
    pub archived: bool,
    /// Whether the caller went there lately.
    pub recent: bool,
}

/// A bounded name lookup; a partial answer asks the reader to narrow the words rather than claiming completeness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NameSearchPage {
    /// Currently readable names, in rank order.
    pub items: Vec<NameHit>,
    /// More candidates may match beyond the answer or its bounded access-check budget.
    pub partial: bool,
}

impl std::ops::Deref for NameSearchPage {
    type Target = [NameHit];
    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

impl IntoIterator for NameSearchPage {
    type Item = NameHit;
    type IntoIter = std::vec::IntoIter<NameHit>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a> IntoIterator for &'a NameSearchPage {
    type Item = &'a NameHit;
    type IntoIter = std::slice::Iter<'a, NameHit>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

/// Content order; the existing default remains chronological.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchSort {
    /// Most recently written first.
    #[default]
    Newest,
    /// Best matches first: by the field that matched (a name or a title, a description, a heading, the rest),
    /// then by how often the words occur there, then the newest.
    Relevance,
}

/// Which clock of the containing thing a search date range selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchDateField {
    /// The thing's creation time.
    Created,
    /// The thing's most recent edit time.
    Edited,
}

/// What to search for in what was written. A hit is returned only for what the caller may read: the
/// items of a thread they hold `thread.view` on, the messages of a chat they can read (deleted ones
/// never match), and the documents they hold `doc.view` on. Matching requires every literal word,
/// in any order, and keeps double-quoted phrases together, without regard to case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchListQuery {
    /// With scope naming one thread, search this retained history; absence uses its selected history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    /// Optional scope identity for a thread search; opaque page cursors retain it automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// Newest by default; relevance also offers readable place names when no content-kind filter is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<SearchSort>,
    /// Only ordinary page access, independent of native administrator reach; applied before paging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participating: Option<bool>,
    /// Only entries carrying a current file or attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_file: Option<bool>,
    /// Only current stored mentions of the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentions_me: Option<bool>,
    /// Only messages of this currently readable chat, subject also to scope and kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<Id>,
    /// Only this currently readable document's title, files and blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<Id>,
    /// What to look for: at least [`SEARCH_QUERY_MIN_CHARS`] characters, and at least
    /// [`SEARCH_INDEXED_MIN_CHARS`] unless a readable topic, thread, chat or document is selected.
    pub q: String,
    /// A node: only what lies in it and below it. Everything the caller can read when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Id>,
    /// A comma list of item kinds, `chat_message` and `document`; empty selects none, absence all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    /// Only what this person wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_id: Option<Id>,
    /// Only things created by this person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by_id: Option<Id>,
    /// Only things last edited by this person, where that fact is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_by_id: Option<Id>,
    /// Only things with this person as their first owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<Id>,
    /// Only names and titles, not descriptions or body text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_only: Option<bool>,
    /// Only things shared with the caller under their ordinary access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_with_me: Option<bool>,
    /// Only things with a current file of this MIME type or family ending in `/*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    /// Select a containing thing's creation or edit time; absence retains the matched part's time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_field: Option<SearchDateField>,
    /// Inclusive lower bound on the selected time; absence of `date_field` uses the matched part's time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Time>,
    /// Inclusive upper bound on the same time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Time>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// On a stream, use the shared regular-expression engine and words alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<bool>,
    /// On a stream, ask meaning immediately as on Enter; ignored by the lexical JSON endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immediate: Option<bool>,
    /// On a stream, milliseconds already elapsed since the last keystroke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typing_age_ms: Option<u32>,
}

/// Which names to find, among whatever the caller may see: nodes they may view, documents they may
/// read, the chats they are in, the people and groups of the trees they belong to, and the resources
/// listed for them. Recents come first, then a match at the beginning of a name, then by
/// `updated_at`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameSearchListQuery {
    /// Only this caller's starred kind/id identities; empty words list them in saved order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starred: Option<bool>,
    /// Only ordinary page access, independent of native administrator reach; applied before the limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participating: Option<bool>,
    /// Part of a name, a title, a username, a display name or an address, without regard to case.
    /// Empty for the caller's recents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// A comma list of [`NameKind`]s; every kind unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    /// Keeps to one tree of orgs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
    /// How many names, at most [`NAME_SEARCH_LIMIT_MAX`]; [`NAME_SEARCH_LIMIT_DEFAULT`] when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The same search selection on the streamed endpoint.
pub type SearchStreamListQuery = SearchListQuery;

/// Which source found a part; membership may become both when the other source answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchFoundBy {
    /// Literal words or a phrase.
    Words,
    /// Similar meaning alone.
    Meaning,
    /// The two sources found the same part.
    Both,
}

/// One part in the server's current reciprocal-rank order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchOrderEntry {
    /// Stable part identity, shared with its payload event.
    pub id: String,
    /// Its reciprocal-rank score, calculated only on the server.
    pub rank: f64,
    /// Which sources found it so far.
    pub found_by: SearchFoundBy,
    /// The latest known word field, which can arrive after a meaning-first payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_field: Option<SearchMatchField>,
}

/// A newline-delimited event of one search; each payload appears once in that search segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchStreamEvent {
    /// The settings that govern this opening, resolved at the selected scope.
    Options {
        /// The initial number of visible rows.
        page_size: u32,
        /// How long an untouched first screen can reorder.
        reorder_window_secs: u32,
        /// The pause before spending a query embedding.
        meaning_pause_ms: u32,
    },
    /// One hydrated and currently authorized part.
    Hit {
        /// Stable identity of the represented part.
        id: String,
        /// Its payload, sent once.
        hit: Box<SearchHit>,
        /// What found it when it first arrived.
        found_by: SearchFoundBy,
    },
    /// The authoritative order after a batch, including updated source membership.
    Order {
        /// Current ranked part identities, without resending payloads.
        items: Vec<SearchOrderEntry>,
        /// Number of distinct parts received so far.
        received: i64,
    },
    /// The progress or final state of one independent retrieval.
    Source {
        /// `words` or `meaning`.
        source: String,
        /// `running`, `done` or `unavailable`.
        state: String,
        /// Why the source is unavailable, deliberately omitted, or using a paused index.
        reason: Option<String>,
        /// Parts contributed by this source so far.
        received: i64,
    },
    /// A segment ended; continuing it resumes both sources with current access checks.
    Cursor {
        /// Opaque continuation, or null when neither source has more.
        cursor: Option<String>,
    },
}

/// One of the content kinds that can be related to a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelatedKind {
    /// A document.
    Document,
    /// A chat.
    Chat,
    /// A thread's conversation.
    Thread,
}

/// A positive, enabled signal contributing to the item's rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelatedBy {
    /// Similar whole-item embeddings in the same generation.
    Meaning,
    /// A shared place.
    Place,
    /// A direct or shared-target reference.
    References,
}

/// The closest shared place that contributed to a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelatedPlace {
    /// The same document folder or nested project.
    Folder,
    /// The same topic.
    Topic,
    /// A common project.
    Project,
    /// The same root org.
    Org,
}

/// Why references contributed to a result, without naming their other targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelatedReference {
    /// The source document references the item.
    Outgoing,
    /// The item references the source document.
    Incoming,
    /// Both reference at least one common thing.
    Shared,
}

/// Whether the source document has a usable whole-item vector in its org's present generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelatedMeaning {
    /// Stored meaning vectors contributed; no model request was needed.
    Ready,
    /// Semantic search or its related-item weight is off.
    Off,
    /// The present generation has not indexed the current source yet.
    NotReady,
    /// Vector storage or retrieval is unavailable.
    Unavailable,
}

/// One containing place or document folder, with its name redacted independently of the result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RelatedAncestor {
    /// A hierarchy node or document folder in the result's current ancestry.
    pub id: Id,
    /// Its current name, or null when that ancestor is only structural.
    pub name: Option<String>,
}

/// A currently readable whole item in the server's weighted order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RelatedItem {
    /// The kind of the item.
    pub kind: RelatedKind,
    /// Its stable identity.
    pub id: Id,
    /// Its current permitted name; empty for an unnamed chat.
    pub name: String,
    /// Permitted containing places and document folders, outermost first, for shared name qualification.
    #[serde(default)]
    pub context: Vec<RelatedAncestor>,
    /// Positive enabled signals contributing to this item.
    pub related_by: Vec<RelatedBy>,
    /// Its shared place, when place contributed.
    pub place: Option<RelatedPlace>,
    /// Its reference relationship, when references contributed.
    pub references: Option<RelatedReference>,
}

/// A bounded related list for the source document's current revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RelatedItems {
    /// The source revision against which this answer was read.
    pub seq: i64,
    /// Readable items, already ranked and bounded by the resolved settings.
    pub items: Vec<RelatedItem>,
    /// Whether meaning was available alongside place and references.
    pub meaning: RelatedMeaning,
    /// A safe reason when meaning is unavailable, otherwise null.
    pub meaning_reason: Option<String>,
    /// What to send as `cursor` for the readable items that follow these; absent at the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

crate::endpoints! {
    /// Finds related readable documents, chats and threads using the document's resolved settings.
    DocumentRelatedList: GET "/documents/{id}/related", User, query crate::common::PageQuery, none, json RelatedItems, 200;
    /// Streams words and meaning concurrently, with hydrated parts, server orders, progress and a shared cursor.
    SearchStreamList: GET "/search/stream", User, query SearchStreamListQuery, none, raw, 200;
    /// Searches the items of threads, the messages of chats and the documents that the caller may read, newest first.
    SearchList: GET "/search", User, query SearchListQuery, none, json Page<SearchHit>, 200;
    /// Finds whatever the caller may see by its name: the quick way to a place, a person, a resource, a document or a thread.
    NameSearchList: GET "/search/names", User, query NameSearchListQuery, none, json NameSearchPage, 200;
}

#[cfg(test)]
mod related_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn related_entries_keep_their_signal_names_and_unavailable_meaning_state() {
        let wire = json!({"seq":7,"items":[{
            "kind":"document","id":"019a0000-0000-7000-8000-000000000001","name":"Plan",
            "context":[],"related_by":["place","references"],"place":"topic","references":"outgoing"
        }],"meaning":"unavailable","meaning_reason":"absent"});
        let reply: RelatedItems = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(reply).unwrap(), wire);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(kind: &str, n: u8, name: &str) -> serde_json::Value {
        json!({ "id": format!("0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a{n:02x}"), "kind": kind, "name": name, "redacted": false })
    }

    fn user() -> serde_json::Value {
        json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "human", "username": "bo",
            "display_name": "Bo", "avatar": { "text": "B", "color": 2 }
        })
    }

    #[test]
    fn a_hit_in_a_document_names_the_block_and_the_way_down_to_it() {
        let written = json!({
            "kind": "document",
            "document": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b01", "title": "Handbook", "node": node("project", 2, "Docs"),
                "description": "", "description_source": "auto", "description_line": "How we ship",
                "owner": user(), "owners": 1, "created_by": user(), "page": "follows", "seq": 12,
                "block_count": 40, "bytes": 9000, "files_bytes": 0, "version": 3, "open_discussions": 0,
                "updated_at": "2026-10-08T17:04:05.123Z", "updated_by": null, "seen_seq": null,
                "discussion_level": "involved",
                "archived": false, "deleted": false
            },
            "block_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b02",
            "by": null,
            "path": [node("org", 3, "Acme"), node("project", 2, "Docs")],
            "snippet": "...the release process starts...",
            "matches": [[7, 14]]
        });
        let hit: SearchHit = serde_json::from_value(written.clone()).unwrap();
        match &hit.found {
            SearchFound::Document { block_id, path, .. } => {
                assert!(block_id.is_some());
                assert_eq!(path.len(), 2);
            }
            other => panic!("not a document hit: {other:?}"),
        }
        assert_eq!(hit.matches, vec![(7, 14)]);
        assert_eq!(serde_json::to_value(&hit).unwrap(), written);
    }

    #[test]
    fn a_hit_on_an_item_carries_the_item_with_its_thread_its_topic_and_its_project() {
        let written = json!({
            "kind": "item",
            "item": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01", "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d02",
                "segment_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d03", "turn_id": null, "stream": "main",
                "parent_item_id": null, "kind": "user_message",
                "payload": {
                    "parts": [{ "type": "text", "text": "Please fix the build" }],
                    "input_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d04", "delivery": "queue", "source": "user"
                },
                "status": "completed", "author": user(), "before_orch": false, "pinned": null, "chat_refs": 0,
                "seq_created": 4, "seq_updated": 4,
                "created_at": "2026-10-08T17:04:05.123Z", "updated_at": "2026-10-08T17:04:05.123Z"
            },
            "thread": node("thread", 4, "Fix the build"),
            "topic": node("topic", 5, "CI"),
            "project": node("project", 6, "Billing"),
            "snippet": "Please fix the build",
            "matches": [[11, 14]]
        });
        let hit: SearchHit = serde_json::from_value(written.clone()).unwrap();
        assert!(matches!(&hit.found, SearchFound::Item { item, .. } if item.stream == "main"));
        assert_eq!(serde_json::to_value(&hit).unwrap(), written);
    }

    #[test]
    fn a_search_selector_requires_no_new_field_from_an_older_reader_and_keeps_a_modern_history_identity()
     {
        let old = json!({"q":"retained words","scope":node("thread",4,"Work")["id"]});
        let old_query: SearchListQuery = serde_json::from_value(old.clone()).unwrap();
        assert!(old_query.continuation_id.is_none() && old_query.projection_revision.is_none());
        assert_eq!(serde_json::to_value(old_query).unwrap(), old);
        let mut modern = old;
        modern["continuation_id"] = node("thread", 5, "Retained")["id"].clone();
        modern["projection_revision"] = json!(9);
        let query: SearchListQuery = serde_json::from_value(modern.clone()).unwrap();
        assert_eq!(query.projection_revision, Some(9));
        assert_eq!(serde_json::to_value(query).unwrap(), modern);
        modern["unknown_history"] = json!(true);
        assert!(serde_json::from_value::<SearchListQuery>(modern).is_err());
        assert!(
            serde_json::from_value::<SearchListQuery>(
                json!({"scope":node("thread",4,"Work")["id"]})
            )
            .is_err()
        );
    }

    #[test]
    fn an_advanced_selection_round_trips_and_rejects_unknown_date_choices() {
        let selection = json!({
            "q": "release", "created_by_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01",
            "edited_by_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d02",
            "owner_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d03",
            "title_only": true, "shared_with_me": true, "file_type": "image/*", "date_field": "edited"
        });
        let query: SearchListQuery = serde_json::from_value(selection.clone()).unwrap();
        assert_eq!(query.date_field, Some(SearchDateField::Edited));
        assert_eq!(serde_json::to_value(query).unwrap(), selection);
        assert!(
            serde_json::from_value::<SearchListQuery>(
                json!({"q":"release", "date_field":"activity"})
            )
            .is_err()
        );
    }

    #[test]
    fn a_hit_on_a_title_has_no_block() {
        let written = json!({
            "kind": "document",
            "document": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b01", "title": "Handbook", "node": node("project", 2, "Docs"),
                "description": "", "description_source": "auto", "description_line": "How we ship",
                "owner": user(), "owners": 1, "created_by": user(), "page": "restricted", "seq": 12,
                "block_count": 40, "bytes": 9000, "files_bytes": 0, "version": 3, "open_discussions": 0,
                "updated_at": "2026-10-08T17:04:05.123Z", "updated_by": null, "seen_seq": 7,
                "archived": false, "deleted": false
            },
            "block_id": null, "path": [], "snippet": "Handbook", "matches": [[0, 4]]
        });
        let hit: SearchHit = serde_json::from_value(written).unwrap();
        assert!(matches!(
            hit.found,
            SearchFound::Document { block_id: None, .. }
        ));
    }

    #[test]
    fn a_name_is_found_with_its_kind_and_whether_it_was_visited_lately() {
        let hit: NameHit = serde_json::from_value(json!({
            "kind": "mcp_server", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5c01", "name": "Issue tracker",
            "address": "acme.research.bo.issues", "path": [], "owner": user(), "archived": false, "recent": true
        }))
        .unwrap();
        assert_eq!(hit.kind, NameKind::McpServer);
        assert!(hit.recent);
        assert!(
            serde_json::from_value::<NameHit>(json!({
                "kind": "command", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5c01", "name": "x", "address": null,
                "path": [], "owner": null, "archived": false, "recent": false
            }))
            .is_err(),
            "commands are found by the interface itself"
        );
    }
}
