//! Chats, messages, states, attachments and forwarding.
//!
//! A chat is people's own conversation: it has its own history and its own
//! sequence, and nothing in it reaches an agent unless a person forwards it
//! there or shares the chat with that agent. A node has a chat of its own
//! ([`ChatKind::Node`]); two people have one direct chat in a root org; a
//! group chat is made by a member for the people they name. Messages are
//! [`ChatPart`]s; a client reads a chat as a [`ChatSync`] and then follows
//! its feed. [`ForwardCreate`] copies the messages somebody selected, or
//! the items of a thread, into chats and into the threads of agents.

use orch_types::{Hash, Id, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::Role;
use crate::common::Page;
use crate::documents::Author;
use crate::nodes::NodeRef;
use crate::routes::Access::{self, Permission, Rule};
use crate::threads::{Forwarded, ImageMediaType, ThreadInput};
use crate::users::UserRef;

/// A request to join a known group chat.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatJoinRequestBody {
    /// Optional words for the people who may add a member.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// The name as shared with the requester.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// The most parts a chat message holds.
pub const CHAT_MESSAGE_MAX_PARTS: u32 = 32;
/// The most characters across the text parts of a chat message.
pub const CHAT_MESSAGE_MAX_TEXT_CHARS: u32 = 8_000;
/// The most different emoji in the reactions of one message.
pub const CHAT_MESSAGE_MAX_EMOJI: u32 = 20;
/// How many characters of a message are shown beside a reply or in a chat-list preview.
pub const CHAT_REPLY_EXCERPT_CHARS: u32 = 120;
/// The most characters of the name of a group chat; the least is one.
pub const CHAT_NAME_MAX_CHARS: u32 = 80;
/// How many messages a sync or a read of messages returns when it is not told.
pub const CHAT_MESSAGES_DEFAULT_LIMIT: u32 = 100;
/// The most messages a sync or a read of messages returns at once.
pub const CHAT_MESSAGES_MAX_LIMIT: u32 = 500;
/// The most messages a sync from a position holds; with more changed, the client loads again
/// without a position.
pub const CHAT_SYNC_MAX_CHANGES: i64 = 2_000;
/// The most messages of one chat that one forward carries.
pub const FORWARD_MAX_MESSAGES: u32 = 20;
/// The most items of a thread that one forward carries.
pub const FORWARD_MAX_ITEMS: u32 = 200;
/// The most parts a forward of the items of a thread comes to.
pub const FORWARD_MAX_PARTS: u32 = 20;
/// The most destinations of one forward.
pub const FORWARD_MAX_DESTINATIONS: u32 = 10;
/// The most bytes of one part made of what an agent said and did in a turn.
pub const FORWARD_PART_MAX_BYTES: i64 = 256 * 1024;

/// Who may read a chat: `chat.view` on its node; for a direct or group chat, being in it or
/// holding `directs.view` on its root org, or on an org inside it that everybody in the chat
/// belongs to. A chat the caller may not read answers `404 not_found`.
const READ: Access = Rule(
    "chat.view on the chat's node; for a direct or group chat, being in it or holding directs.view on its root org, or on an org inside it that everybody in the chat belongs to",
);
/// Who may send in a chat: `chat.interact` on its node with `chat.enabled` resolving true, or
/// being in the direct or group chat.
const SEND: Access = Rule(
    "chat.interact on the chat's node with chat.enabled on; for a direct or group chat, being in it",
);

/// How much of a chat a person wants to be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatLevel {
    /// Every message.
    All,
    /// Only where they are mentioned.
    Mentions,
    /// Nothing.
    Off,
}

/// What kind of chat it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatKind {
    /// The chat of a node: of an org at any level, a project, a topic or a thread.
    Node,
    /// A chat between two people.
    Direct,
    /// A chat among several people.
    Group,
}

/// A chat, named wherever another thing points at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatRef {
    /// The chat's identifier.
    pub id: Id,
    /// What kind of chat it is.
    pub kind: ChatKind,
    /// Its root org.
    pub org_id: Id,
    /// The node it belongs to, for a node's chat.
    pub node: Option<NodeRef>,
    /// Its name, for a group chat.
    pub name: Option<String>,
}

/// A person's own state in one chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatState {
    /// The fence an automatic read must carry; deliberate unread advances it.
    #[serde(default)]
    pub read_revision: i64,
    /// The current default before the person's own chat bell, when projected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_level: Option<ChatLevel>,
    /// The current reason for that default, when projected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reason: Option<ChatDefaultReason>,
    /// The chat.
    pub chat_id: Id,
    /// What they set with the bell; `null` when they have not.
    pub level: Option<ChatLevel>,
    /// Whether they posted here or were mentioned here.
    pub engaged: bool,
    /// What applies to them; off means they do not follow the chat.
    pub effective_level: ChatLevel,
    /// The last sequence they read.
    pub last_read_seq: Seq,
    /// How many mentions of them are unread.
    pub mentions_unread: i64,
    /// Whether they hid the chat.
    pub hidden: bool,
}

/// Why the caller cannot send in a chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatCanReason {
    /// Its node is archived.
    Archived,
    /// Chats are switched off where it lies.
    Disabled,
    /// A direct chat whose other person is disabled or has left the org.
    Closed,
    /// The caller may not send in it.
    NoPermission,
    /// The caller reads it as an admin and is not in it.
    Inspecting,
}

/// What the caller may do in a chat, now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatCan {
    /// The actual source of native-only permissions, even when ordinary Read leaves no marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_place: Option<crate::nodes::NodeOrSystem>,
    /// Native reach alone gives Read; absent when ordinary pages or membership already do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<crate::access::AdminAccess>,
    /// Held chat permissions absent from ordinary access, in catalogue order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_only: Vec<String>,
    /// An explicit assignment at the node or a current follow, independently of Read and As admin.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub participating: bool,
    /// Whether they may send.
    pub send: bool,
    /// Whether they may moderate: delete the messages of others.
    pub moderate: bool,
    /// Whether they may take it away as a file: they hold the Export row of a node's chat (`chat.export`), or are in
    /// a direct or a group chat. Somebody who only inspects one as an admin may not.
    pub export: bool,
    /// Why `send` is false; `null` when it is true.
    pub reason: Option<ChatCanReason>,
}

/// A chat, as the caller reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Chat {
    /// The root's current explicit inspection holders, for a direct/group access description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection_holders: Option<Vec<crate::access::Holder>>,
    /// Current visible agent origins and their actual reach; omitted on compact lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<ChatAgentReach>>,
    /// What names it wherever it is pointed at.
    #[serde(flatten)]
    pub base: ChatRef,
    /// The people in a direct or group chat; `null` for a node's chat.
    pub members: Option<Vec<UserRef>>,
    /// Those of `members` who are not members of the root org; group chats only.
    pub guests: Vec<Id>,
    /// Deadlines of current timed members; absent entries are permanent.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub membership_expiries: std::collections::BTreeMap<Id, Time>,
    /// Who made it.
    pub created_by: Option<UserRef>,
    /// When it was made.
    pub created_at: Time,
    /// The highest chat sequence issued.
    pub seq: Seq,
    /// The sequence of the newest message.
    pub last_message_seq: Seq,
    /// When the newest message was written.
    pub last_message_at: Option<Time>,
    /// The newest ordinary message as a short preview, after permission checks;
    /// `null` where the chat has only system lines or no messages.
    #[serde(default)]
    pub preview: Option<ChatPreview>,
    /// What the caller may do in it, now.
    pub can: ChatCan,
    /// The caller's own state; `null` when they have none here.
    pub my: Option<ChatState>,
}

/// A chat list's newest ordinary message, without its parts, replies or reactions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatPreview {
    /// The message, to which a client may open the chat.
    pub id: Id,
    /// Its author with the same permission-filtered names as a message view.
    pub author: Option<Author>,
    /// Up to CHAT_REPLY_EXCERPT_CHARS characters of text, else the names of its
    /// files; empty for a deleted message. Live item quotes do not copy private text.
    pub text: String,
    /// Whether this newest message was deleted; older text is not substituted.
    pub deleted: bool,
}

/// One part of a chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatPart {
    /// A deliberate current reader, role or group audience, resolved when the message is accepted.
    Mention {
        /// The stable audience identity.
        target: ChatMentionTarget,
    },
    /// Text, as Markdown.
    Text {
        /// The text.
        text: String,
    },
    /// An image that was uploaded.
    Image {
        /// The image, by its hash.
        blob: Hash,
        /// Its type.
        media_type: ImageMediaType,
        /// Its name.
        name: String,
    },
    /// A file that was attached.
    Attachment {
        /// The file, by its hash.
        blob: Hash,
        /// Its name.
        name: String,
        /// Its size in bytes.
        size: i64,
        /// Its type.
        media_type: String,
    },
    /// An item of a thread, quoted; only in that thread's chat.
    ItemRef {
        /// The thread.
        thread_id: Id,
        /// The item.
        item_id: Id,
    },
    /// Pasted text kept as a folded, editable draft attachment.
    #[serde(untagged)]
    PastedText(crate::threads::PastedText),
    /// A message forwarded into this chat.
    #[serde(untagged)]
    Forwarded(Box<Forwarded>),
}

/// What a message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageKind {
    /// Something somebody wrote.
    Message,
    /// A line the system wrote about the chat.
    System,
}

/// What a system message tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatSystemEvent {
    /// The chat was made.
    Created,
    /// The chat was renamed.
    Renamed,
    /// Somebody was added.
    MemberAdded,
    /// Somebody left.
    MemberLeft,
    /// Somebody was removed.
    MemberRemoved,
    /// A timed membership ended.
    MemberExpired,
}

/// The event a system message tells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageSystem {
    /// What happened.
    pub event: ChatSystemEvent,
    /// Who did it.
    pub by: Option<UserRef>,
    /// To whom, where it was done to somebody.
    pub user: Option<UserRef>,
    /// The new name, for a rename.
    pub name: Option<String>,
}

/// The message that a message replies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageReply {
    /// The message.
    pub id: Id,
    /// Who wrote it.
    pub author: Option<Author>,
    /// Its first [`CHAT_REPLY_EXCERPT_CHARS`] characters.
    pub excerpt: String,
    /// Whether it was deleted.
    pub deleted: bool,
}

/// An emoji and the people who reacted with it, in the order they did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Reaction {
    /// The emoji.
    pub emoji: String,
    /// The people.
    pub users: Vec<Id>,
}

/// Who pinned a message, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatPin {
    /// When.
    pub at: Time,
    /// Who.
    pub by: UserRef,
}

/// One message of a chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessage {
    /// Current safe display labels for deliberate targets; editing still sends their stable identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mention_names: Vec<ChatMentionName>,
    /// Stable deliberate targets retained independently of their frozen recipient sets.
    #[serde(default)]
    pub mention_targets: Vec<ChatMentionTarget>,
    /// The ordinary inputs accepted atomically with this visible message, never an edit.
    #[serde(default)]
    pub asked_agents: Vec<ChatAskedAgent>,
    /// The message's identifier.
    pub id: Id,
    /// The chat.
    pub chat_id: Id,
    /// What it is.
    pub kind: ChatMessageKind,
    /// A person, or an agent that was let post here; `null` for a system message.
    pub author: Option<Author>,
    /// What it says; empty for a deleted message and for a system message.
    pub parts: Vec<ChatPart>,
    /// What happened, for a system message.
    pub system: Option<ChatMessageSystem>,
    /// The message it replies to.
    pub reply_to: Option<ChatMessageReply>,
    /// The people it names with `@`.
    pub mentions: Vec<UserRef>,
    /// Whether it says `@all`.
    pub mentions_all: bool,
    /// How people reacted.
    pub reactions: Vec<Reaction>,
    /// Whether and by whom it is pinned.
    pub pinned: Option<ChatPin>,
    /// When it was last edited.
    pub edited_at: Option<Time>,
    /// Whether it was deleted: the row stays, without parts, reactions or mentions.
    pub deleted: bool,
    /// The chat sequence at which it was written.
    pub seq_created: Seq,
    /// The chat sequence at which it last changed.
    pub seq_updated: Seq,
    /// When it was written.
    pub created_at: Time,
}

/// Everything a client needs to show a chat, or what changed since it last did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatSync {
    /// The chat, always.
    pub chat: Chat,
    /// The chat's highest issued sequence.
    pub seq: Seq,
    /// The sequence the caller asked from, if it did.
    pub since: Option<Seq>,
    /// False when more than [`CHAT_SYNC_MAX_CHANGES`] messages changed; the client loads again
    /// without a position.
    pub complete: bool,
    /// The messages.
    pub messages: Vec<ChatMessage>,
    /// Whether messages older than the returned ones exist; on a first load only.
    pub has_older: bool,
}

/// A chat's admitted first window and this reader's private composer draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatOpening {
    /// The same admitted conversation as the initial sync reader.
    pub sync: ChatSync,
    /// The caller's draft, absent for an empty composer or an inspecting reader.
    pub draft: Option<ChatDraft>,
}

/// One readable chat offered beside a place, independently admitted from its ancestors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeChatScope {
    /// The readable place, with the same redaction rules as its native node view.
    pub node: NodeRef,
    /// The chat and its current message position.
    pub chat: crate::nodes::TreeChat,
}

/// The readable scopes of a place and the selected chat's first window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeChatOpening {
    /// Only scopes whose place and chat the current caller may read.
    pub scopes: Vec<NodeChatScope>,
    /// The selected scope; absent when none is readable.
    pub opening: Option<ChatOpening>,
}

/// Selects a familiar chat scope without exposing any unread conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeChatOpeningGetQuery {
    /// A preferred place kind; the nearest offered scope is used if unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<crate::nodes::NodeKind>,
    /// An explicitly requested chat, selected only among independently readable scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<Id>,
    /// The initial message page size, with the same bounds as chat sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A message that was begun in a chat's composer and not sent. Drafts are private to their
/// author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatDraft {
    /// Current safe labels used to reopen typed mentions on another device.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mention_names: Vec<ChatMentionName>,
    /// What was written.
    pub parts: Vec<ChatPart>,
    /// The message it replies to.
    pub reply_to: Option<Id>,
    /// When it was last saved.
    pub updated_at: Time,
}

/// A current label for a stable deliberate target, without any recipient enumeration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMentionName {
    /// Its stable identity.
    pub target: ChatMentionTarget,
    /// The visible name after @, filtered for this reader.
    pub name: String,
}

/// Which chats of a root org a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatListScope {
    /// The caller's direct and group chats there that are not hidden.
    Mine,
    /// The direct and group chats the caller is not in and may read through `directs.view`.
    Others,
    /// The chats of the tree's nodes that the caller can read and that have messages.
    Nodes,
}

/// The chats of a root org. `{id}` is a root org; an org inside one answers `404 not_found`,
/// because its people's chats are the tree's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgChatListQuery {
    /// Only ordinary membership or a node's ordinary chat page, independent of native reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participating: Option<bool>,
    /// Which chats; `mine` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ChatListScope>,
    /// With `others`: the org of the tree on which the caller holds `directs.view`. All the chats
    /// when it is the root org, and for an org inside it those all of whose people are its
    /// members. Without it, the union of every readable org, before paging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<Id>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Opens a chat among people. With one other person and no name it is the direct chat with them in
/// this org, which answers `200` when it exists already; otherwise a new group, `201`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgChatCreateBody {
    /// The other people.
    pub user_ids: Vec<Id>,
    /// A name for a group, one to [`CHAT_NAME_MAX_CHARS`] characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Renames a group chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatUpdateBody {
    /// Its new name; `null` names it after its members again.
    pub name: Option<String>,
}

/// What to read of a chat's messages from a sync.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatSyncGetQuery {
    /// Only what changed after this chat sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Seq>,
    /// How many messages, at most [`CHAT_MESSAGES_MAX_LIMIT`]; [`CHAT_MESSAGES_DEFAULT_LIMIT`] when
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The messages to read: exactly one of `before` or `after` (a `seq_created`), `around` (a
/// message), `pinned` or `item_id` (the messages that quote that item).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMessageListQuery {
    /// The messages written before this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Seq>,
    /// The messages written after this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Seq>,
    /// The messages around this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<Id>,
    /// With `true`: the pinned messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    /// The messages that quote this item of the thread whose chat it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<Id>,
    /// How many messages, at most [`CHAT_MESSAGES_MAX_LIMIT`]; [`CHAT_MESSAGES_DEFAULT_LIMIT`]
    /// when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Messages of a chat, ascending by `seq_created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageListReply {
    /// The messages.
    pub messages: Vec<ChatMessage>,
    /// Whether older messages exist.
    pub has_older: bool,
    /// Whether newer messages exist.
    pub has_newer: bool,
}

/// Sends a message. It has at least one part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMessageCreateBody {
    /// What it says: at most [`CHAT_MESSAGE_MAX_PARTS`] parts.
    pub parts: Vec<ChatPart>,
    /// The message it replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Id>,
    /// The current input preview accepted by the sender, required for deliberate agent targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_review: Option<Hash>,
    /// A client-generated UUID: a repeated pair of author and value returns the stored message.
    pub client_id: String,
}

/// The message that was sent, and the chat sequence it was written at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageCreateReply {
    /// The message.
    pub message: ChatMessage,
    /// The chat's highest issued sequence.
    pub seq: Seq,
}

/// Edits a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMessageUpdateBody {
    /// What it says now.
    pub parts: Vec<ChatPart>,
}

/// Says how far the caller has read a chat. The stored value only moves forward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatReadBody {
    /// The chat sequence read.
    pub seq: Seq,
    /// The state revision the browser observed; stale reads are refused.
    pub read_revision: i64,
    /// Rewinds to immediately before the selected live message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_from: Option<bool>,
}

/// Why the current default follows or leaves quiet this chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatDefaultReason {
    /// They own this chat's thread.
    Owner,
    /// They follow its thread wholly.
    Following,
    /// They posted or were mentioned here.
    Engaged,
    /// They are in the direct or group chat.
    Member,
    /// They have no following reason.
    Quiet,
}

/// Changes the caller's state in a chat; what is left out stays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatStateSetBody {
    /// The bell: a value follows or mutes the chat; `null` returns to the default.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<ChatLevel>")]
    pub level: Option<Option<ChatLevel>>,
    /// Whether the chat is off the caller's list until its next message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

/// Saves a draft. The chat keeps one for each person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatDraftSetBody {
    /// What was written.
    pub parts: Vec<ChatPart>,
    /// The message it replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Id>,
}

/// An attachment that is added to a chat from a finished upload session instead of a raw body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatAttachmentBlobBody {
    /// The upload's hash.
    pub blob: Hash,
    /// Its name.
    pub name: String,
}

/// A file stored for a chat, ready for an `attachment` part. Attachments are stored as blobs and
/// never in a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatAttachmentCreateReply {
    /// The file, by its hash.
    pub blob: Hash,
    /// Its name.
    pub name: String,
    /// Its size in bytes.
    pub size: i64,
    /// Its type.
    pub media_type: String,
}

/// How an image or an attachment of a chat is served.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatBlobGetQuery {
    /// Return a bounded static preview instead of the original file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<bool>,
    /// Whether to offer it as a download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<bool>,
}

/// Messages of one chat to forward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardSourceChat {
    /// The chat.
    pub chat_id: Id,
    /// The messages, at most [`FORWARD_MAX_MESSAGES`].
    pub message_ids: Vec<Id>,
}

/// Items of one thread to forward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardSourceThread {
    /// The thread.
    pub thread_id: Id,
    /// The items that the caller can read, of any kind but an approval and a history line: at
    /// most [`FORWARD_MAX_ITEMS`], coming to at most [`FORWARD_MAX_PARTS`] parts.
    pub item_ids: Vec<Id>,
}

/// What is forwarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ForwardSource {
    /// Messages of a chat.
    Chat(ForwardSourceChat),
    /// Items of a thread.
    Thread(ForwardSourceThread),
}

/// A chat the caller can send in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardDestinationChat {
    /// The chat.
    pub chat_id: Id,
}

/// A person: the direct chat with them in the org the source belongs to, opened if it does not
/// exist yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardDestinationUser {
    /// The person.
    pub user_id: Id,
}

/// An agent's thread on which the caller holds `thread.interact`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardDestinationThread {
    /// The thread.
    pub thread_id: Id,
}

/// Where a forward goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ForwardDestination {
    /// A chat.
    Chat(ForwardDestinationChat),
    /// A person.
    User(ForwardDestinationUser),
    /// An agent's thread.
    Thread(ForwardDestinationThread),
}

/// Forwards messages or items to up to [`FORWARD_MAX_DESTINATIONS`] destinations. Every
/// destination is checked and written in one transaction: if one refuses, nothing is written
/// anywhere, and the error names that destination in `object`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardCreateBody {
    /// What is forwarded.
    pub from: ForwardSource,
    /// Where it goes.
    pub to: Vec<ForwardDestination>,
    /// A text that goes first in each chat message and each input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// A client-generated UUID: a repeated value returns what was stored.
    pub client_id: String,
}

/// The message a forward wrote into a chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForwardChatMessage {
    /// The chat.
    pub chat_id: Id,
    /// The message, by the caller.
    pub message: ChatMessage,
}

/// The input a forward queued for an agent's thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForwardThreadInput {
    /// The thread.
    pub thread_id: Id,
    /// The input, by the caller, with `delivery: 'queue'`.
    pub input: ThreadInput,
}

/// What a forward wrote, by destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForwardCreateReply {
    /// The messages written into chats.
    pub chats: Vec<ForwardChatMessage>,
    /// The inputs queued for threads.
    pub threads: Vec<ForwardThreadInput>,
}

/// The stable target of a deliberate human chat mention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatMentionTarget {
    /// One current readable person.
    User {
        /// Their identifier.
        id: Id,
    },
    /// This chat place's named role or higher.
    Role {
        /// The node chat's place or the group chat's root org.
        node_id: Id,
        /// The role or higher.
        role: Role,
    },
    /// A visible group intersected with current readers.
    Group {
        /// Its identifier.
        id: Id,
    },
    /// An explicit ordinary queued input to this visible thread, accepted with the chat post.
    Agent {
        /// The originating thread's stable identity, independent of its displayed agent name.
        thread_id: Id,
    },
}

/// An accepted ask, projected through the reader's current thread visibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatAskedAgent {
    /// The thread, redacted if this reader may no longer see it.
    pub thread: NodeRef,
    /// The ordinary input written by the sender.
    pub input_id: Id,
}

/// A current readable choice in the mention picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MentionChoice {
    /// The person's current public identity for a human choice; null for a broad audience.
    pub user: Option<UserRef>,
    /// The stable target to name.
    pub target: ChatMentionTarget,
    /// Its human name in its visible place.
    pub name: String,
    /// Distinct current recipients, excluding the sender.
    pub count: i64,
    /// Whether it may be chosen now.
    pub available: bool,
    /// A reason when it cannot.
    pub reason: Option<String>,
    /// An offered agent's visible thread, otherwise null.
    pub thread: Option<NodeRef>,
    /// The offered agent's name, otherwise null.
    pub agent: Option<String>,
    /// The offered thread's state, otherwise null.
    pub state: Option<String>,
    /// When the offered thread began, otherwise null.
    pub started_at: Option<Time>,
}

/// Current mention choices in reader-first order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMentionListQuery {
    /// A name fragment; empty already lists readers, @all lists current followers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Requested page size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The previous page's cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A page of choices and the count a human @all currently names, without its roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMentionPage {
    /// Choices after the current visibility and policy cuts.
    pub items: Vec<MentionChoice>,
    /// The next page, if any.
    pub next_cursor: Option<String>,
    /// Current readable followers, excluding this sender.
    pub followers: i64,
}

/// The visible message whose current audience is to be counted before sending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatMentionPreviewBody {
    /// The same parts the sender intends to post.
    pub parts: Vec<ChatPart>,
    /// The original readable reply that will be forwarded before the new message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Id>,
}

/// The actual ordinary queued input and current receiver facts reviewed before sending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatAgentAskPreview {
    /// Its current visible thread.
    pub thread: NodeRef,
    /// Its current agent, when bound.
    pub agent: Option<String>,
    /// Its current resolved model.
    pub model: Option<String>,
    /// Its current resolved effort, when set.
    pub effort: Option<String>,
    /// The ordinary input, made in a transaction rolled back after this preview.
    pub input: ThreadInput,
}

/// The distinct current human audience, never a sum of overlapping group counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMentionPreviewReply {
    /// Recipients, excluding the sender.
    pub count: i64,
    /// Actual guarded input previews; none for ordinary human-only mentions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_asks: Vec<ChatAgentAskPreview>,
    /// Optimistic review of the proposed message and current receivers; never an authority token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_review: Option<Hash>,
}

/// Stops or allows one originating thread's agent posting in this chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatAgentPostingSetBody {
    /// Clearing the stop grants no writing share.
    pub stopped: bool,
}

/// One currently visible originating agent's actual standing towards this chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatAgentReach {
    /// Its thread, only where the current reader may view it.
    pub thread: NodeRef,
    /// Its current agent name.
    pub agent: String,
    /// Whether a current share opens reading.
    pub readable: bool,
    /// Whether a current write share survives the local posting stop.
    pub writable: bool,
    /// Whether a person stopped this origin here.
    pub stopped: bool,
    /// The release time of a currently full rolling-hour bound, otherwise null.
    pub next_allowed_at: Option<Time>,
}

crate::endpoints! {
    /// Opens a place's independently readable chat scopes and the selected conversation.
    NodeChatOpeningGet: GET "/nodes/{id}/chat-opening", Permission("node.view"), query NodeChatOpeningGetQuery, none, json NodeChatOpening, 200;
    /// Opens a chat and the current reader's draft under their native checks.
    ChatOpeningGet: GET "/chats/{id}/opening", READ, query ChatSyncGetQuery, none, json ChatOpening, 200;
    /// Stops or allows one origin here under current chat management.
    ChatAgentPostingSet: PUT "/chats/{id}/agents/{thread_id}/posting", Rule("chat.manage on a node chat; the creator of a group chat"), none, json ChatAgentPostingSetBody, none, 204;
    /// Lists this chat's current readable people and permitted role and group audiences.
    ChatMentionList: GET "/chats/{id}/mentions", SEND, query ChatMentionListQuery, none, json ChatMentionPage, 200;
    /// Counts the message's distinct current human audience before Send.
    ChatMentionPreview: POST "/chats/{id}/mentions/preview", SEND, none, json ChatMentionPreviewBody, json ChatMentionPreviewReply, 200;
    /// Lists the chats of a root org that the caller may see, newest activity first.
    OrgChatList: GET "/orgs/{id}/chats", Rule("a member of that root org, or a guest of one of its group chats, who gets those chats only; directs.view for the chats of others"), query OrgChatListQuery, none, json Page<Chat>, 200;
    /// Opens a chat among people: the direct chat with one other person (`200` when it exists already), otherwise a new group (`201`).
    OrgChatCreate: POST "/orgs/{id}/chats", Rule("a member of the org, with chat.direct resolving to members there"), none, json OrgChatCreateBody, json Chat, 201;
    /// Reads a chat.
    ChatGet: GET "/chats/{id}", READ, none, none, json Chat, 200;
    /// Asks the people who may add members to join a known group chat.
    ChatJoinRequest: POST "/chats/{id}/join-requests", Access::User, none, json ChatJoinRequestBody, json crate::access::AccessRequestCreateReply, 201;
    /// Renames a group chat, or names it after its members again.
    ChatUpdate: PATCH "/chats/{id}", Rule("someone in the group chat"), none, json ChatUpdateBody, json Chat, 200;
    /// Adds a person to a group chat; somebody who is no member of the root org joins as a guest when the caller may bring an outsider in.
    ChatMemberAdd: PUT "/chats/{id}/members/{user_id}", Rule("someone in the group chat who is a member of the root org"), none, none, json Chat, 200;
    /// Takes a person out of a group chat, or lets them leave.
    ChatMemberRemove: DELETE "/chats/{id}/members/{user_id}", Rule("that person, to leave; or the creator of the group"), none, none, none, 204;
    /// Reads a chat and its latest messages, or what changed since a chat sequence.
    ChatSyncGet: GET "/chats/{id}/sync", READ, query ChatSyncGetQuery, none, json ChatSync, 200;
    /// Reads messages of a chat: before or after a sequence, around a message, the pinned ones, or those that quote an item.
    ChatMessageList: GET "/chats/{id}/messages", READ, query ChatMessageListQuery, none, json ChatMessageListReply, 200;
    /// Sends a message; a repeated `client_id` of the same author answers `200` with the stored message instead of `201`.
    ChatMessageCreate: POST "/chats/{id}/messages", SEND, none, json ChatMessageCreateBody, json ChatMessageCreateReply, 201;
    /// Edits a message.
    ChatMessageUpdate: PATCH "/chats/{id}/messages/{message_id}", Rule("the message's author, who can still send"), none, json ChatMessageUpdateBody, json ChatMessage, 200;
    /// Deletes a message, which stays as a row with `deleted: true`.
    ChatMessageDelete: DELETE "/chats/{id}/messages/{message_id}", Rule("the message's author; or chat.manage in a node's chat"), none, none, json ChatMessage, 200;
    /// Adds the caller's own reaction to a message.
    ChatMessageReactionAdd: PUT "/chats/{id}/messages/{message_id}/reactions/{emoji}", SEND, none, none, json ChatMessage, 200;
    /// Removes the caller's own reaction from a message.
    ChatMessageReactionRemove: DELETE "/chats/{id}/messages/{message_id}/reactions/{emoji}", SEND, none, none, json ChatMessage, 200;
    /// Pins a message.
    ChatMessagePinSet: PUT "/chats/{id}/messages/{message_id}/pin", SEND, none, none, json ChatMessage, 200;
    /// Unpins a message.
    ChatMessagePinDelete: DELETE "/chats/{id}/messages/{message_id}/pin", SEND, none, none, json ChatMessage, 200;
    /// Moves the caller's read position forward, clears their unread mentions and marks the chat's notifications read.
    ChatRead: POST "/chats/{id}/read", Rule("read, as someone who is in the chat or holds chat.view"), none, json ChatReadBody, none, 204;
    /// Marks every chat of the org that the caller follows as read up to now.
    OrgChatsRead: POST "/orgs/{id}/chats/read", Permission("node.view"), none, none, none, 204;
    /// Sets the bell and the hiding of a chat for the caller.
    ChatStateSet: PUT "/chats/{id}/state", Rule("the same as for marking it read"), none, json ChatStateSetBody, json ChatState, 200;
    /// Reads the caller's draft in a chat, if they have one.
    ChatDraftGet: GET "/chats/{id}/draft", SEND, none, none, json Option<ChatDraft>, 200;
    /// Saves the caller's draft in a chat.
    ChatDraftSet: PUT "/chats/{id}/draft", SEND, none, json ChatDraftSetBody, json ChatDraft, 200;
    /// Discards the caller's draft in a chat.
    ChatDraftDelete: DELETE "/chats/{id}/draft", SEND, none, none, none, 204;
    /// Stores a file for an `attachment` part, from a raw body with a filename header or from a finished upload.
    ChatAttachmentCreate: POST "/chats/{id}/attachments", SEND, none, bytes, json ChatAttachmentCreateReply, 200;
    /// Serves an image or an attachment carried by a message of this chat, forwarded copies included.
    ChatBlobGet: GET "/chats/{id}/blobs/{hash}", READ, query ChatBlobGetQuery, none, raw, 200;
    /// Forwards messages of a chat, or items of a thread, into chats and the threads of agents, all or nothing.
    ForwardCreate: POST "/forward", Rule("read what is forwarded, and send in each destination chat or hold thread.interact on each destination thread"), none, json ForwardCreateBody, json ForwardCreateReply, 201;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn joining_a_group_needs_no_reason_or_shared_label() {
        let empty: ChatJoinRequestBody = serde_json::from_value(json!({})).unwrap();
        assert!(empty.reason.is_empty());
        assert_eq!(empty.label, None);
        assert_eq!(serde_json::to_value(empty).unwrap(), json!({}));
        let written = json!({"reason":"Working together","label":"Shared group"});
        let request: ChatJoinRequestBody = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), written);
        assert!(serde_json::from_value::<ChatJoinRequestBody>(json!({"members":[]})).is_err());
    }

    #[test]
    fn a_deliberate_unread_choice_carries_its_observed_fence() {
        let value = json!({"seq":7,"read_revision":2,"unread_from":true});
        let read: ChatReadBody = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(read).unwrap(), value);
        assert!(serde_json::from_value::<ChatReadBody>(json!({"seq":7})).is_err());
    }

    fn user(n: u8) -> serde_json::Value {
        json!({
            "id": format!("0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a{n:02x}"), "kind": "human", "username": format!("user{n}"),
            "display_name": format!("User {n}"), "avatar": { "text": "U", "color": 3 }
        })
    }

    #[test]
    fn a_message_is_read_with_its_parts_its_author_and_its_reactions() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b01",
            "chat_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b02",
            "kind": "message",
            "author": { "agent": "claude-code", "thread": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b03", "kind": "thread", "name": "Fix the build", "redacted": false
            } },
            "parts": [
                { "type": "text", "text": "The build is green @user1" },
                { "type": "image", "blob": "a".repeat(64), "media_type": "image/png", "name": "chart.png" },
                { "type": "attachment", "blob": "b".repeat(64), "name": "report.pdf", "size": 120000, "media_type": "application/pdf" },
                { "type": "item_ref", "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b03", "item_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b04" },
                { "type": "forwarded",
                  "from": { "kind": "chat", "chat": {
                      "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b05", "kind": "group", "org_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b06",
                      "node": null, "name": "Release" } },
                  "source_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b07", "author": user(1),
                  "at": "2026-10-08T17:04:05.123Z", "text": "Ship it", "images": [], "attachments": [] }
            ],
            "system": null,
            "reply_to": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5b08", "author": user(2), "excerpt": "Is it green?", "deleted": false },
            "mentions": [user(1)],
            "mentions_all": false,
            "mention_targets": [],
            "asked_agents": [],
            "reactions": [{ "emoji": "\u{1f44d}", "users": ["0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01"] }],
            "pinned": { "at": "2026-10-08T17:05:00.000Z", "by": user(2) },
            "edited_at": null,
            "deleted": false,
            "seq_created": 41,
            "seq_updated": 43,
            "created_at": "2026-10-08T17:04:05.123Z"
        });
        let message: ChatMessage = serde_json::from_value(written.clone()).unwrap();
        assert!(matches!(message.author, Some(Author::Agent(_))));
        assert_eq!(message.parts.len(), 5);
        assert!(matches!(message.parts[4], ChatPart::Forwarded(_)));
        assert_eq!(serde_json::to_value(&message).unwrap(), written);
    }

    #[test]
    fn a_chat_is_a_reference_with_what_the_caller_may_do_in_it() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5c01", "kind": "direct", "org_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5c02",
            "node": null, "name": null,
            "members": [user(1), user(2)], "guests": [],
            "created_by": user(1), "created_at": "2026-10-08T17:04:05.123Z",
            "seq": 12, "last_message_seq": 12, "last_message_at": "2026-10-08T18:00:00.000Z",
            "can": { "send": false, "moderate": false, "export": false, "reason": "inspecting" },
            "my": null, "preview": null
        });
        let chat: Chat = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(chat.base.kind, ChatKind::Direct);
        assert_eq!(chat.can.reason, Some(ChatCanReason::Inspecting));
        assert_eq!(serde_json::to_value(&chat).unwrap(), written);
    }

    #[test]
    fn the_bell_returns_to_the_default_with_null_and_stays_when_left_out() {
        let default: ChatStateSetBody = serde_json::from_value(json!({ "level": null })).unwrap();
        let muted: ChatStateSetBody = serde_json::from_value(json!({ "level": "off" })).unwrap();
        let hidden: ChatStateSetBody = serde_json::from_value(json!({ "hidden": true })).unwrap();
        assert_eq!(default.level, Some(None));
        assert_eq!(muted.level, Some(Some(ChatLevel::Off)));
        assert_eq!(hidden.level, None);
    }

    #[test]
    fn a_forward_names_a_source_and_destinations_of_three_kinds() {
        let body: ForwardCreateBody = serde_json::from_value(json!({
            "from": { "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01", "item_ids": ["0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d02"] },
            "to": [
                { "chat_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d03" },
                { "user_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d04" },
                { "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d05" }
            ],
            "comment": "Look at this",
            "client_id": "7b7a4c1e-0f0e-4d57-9d5e-6f3a9a8b2c11"
        }))
        .unwrap();
        assert!(matches!(body.from, ForwardSource::Thread(_)));
        assert!(matches!(body.to[0], ForwardDestination::Chat(_)));
        assert!(matches!(body.to[1], ForwardDestination::User(_)));
        assert!(matches!(body.to[2], ForwardDestination::Thread(_)));
        assert!(
            serde_json::from_value::<ForwardCreateBody>(json!({
                "from": { "chat_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5d01", "message_ids": [], "thread_id": "x" },
                "to": [], "client_id": "c"
            }))
            .is_err()
        );
    }
}
