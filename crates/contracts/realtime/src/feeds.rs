//! Durable and ephemeral feed identifiers, an environment's own feed and a document's among them.
//!
//! A client says what it wants to follow by naming a feed. A durable
//! feed has a sequence: the client loads its state over HTTP, says how
//! far it has it, and is sent every row that changed after that, none
//! skipped. An ephemeral feed has none: it delivers what happens while
//! somebody listens, and nothing of what happened before.
//!
//! What a feed carries is said here with it: the members of a durable
//! feed's rows, and the events of the others.

use crate::documents::DocData;
use crate::foreign;
use crate::typing::TypingNotice;
use base64::Engine;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use orch_api_contract::common::Listed;
use orch_api_contract::common::SubscriptionLevel;
use orch_api_contract::environment::{Environment, EnvironmentSyncJob};
use orch_api_contract::everything::EverythingResource;
use orch_api_contract::nodes::{Topic, TreeNode};
use orch_api_contract::personal::{ChatState, Draft, PersonalStateKey};
use orch_api_contract::processes::{ProcessInfo, ResourceSample};
use orch_api_contract::resources::LoginState;
use orch_api_contract::threads::{Item, RawEvent, Segment, Thread, ThreadInput, Turn};
use orch_api_contract::topics::Announcement;
use orch_api_contract::workspace::{DirEntry, Job};
use orch_types::{Id, Json, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Map;

/// The most bytes of a directory's path in a feed's name: the longest path
/// a Linux file system takes, which is where workspaces live. Written in
/// base64 it is still far under what a client may send in one frame.
pub const MAX_PATH_BYTES: usize = 4_096;
/// How long a server keeps watching for an ephemeral feed after its
/// last subscriber left, so that a screen that is left and opened again
/// does not stop and start the watch.
pub const WATCH_LINGER_SECS: u32 = 10;
/// How often what an environment uses is reported, in seconds.
pub const STATS_EVERY_SECS: u32 = 5;
/// How often an environment's processes are reported, in seconds; a
/// signal is followed by a report at once.
pub const PROCS_EVERY_SECS: u32 = 5;
/// Changes in a watched directory are gathered for at least this long,
/// so that a build that writes a thousand files sends a few events.
pub const DIR_BATCH_MILLIS: u32 = 500;
/// At most this often is a resource reported as changed, and a job of
/// it as moved on.
pub const RESOURCE_MIN_INTERVAL_SECS: u32 = 1;

/// A directory's path in a feed's name: base64url, written without
/// padding and read with or without it.
const PATH_CODING: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Whether a feed has a sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeedKind {
    /// Rows by sequence: nothing is missed across a reconnect.
    Durable,
    /// Events while subscribed: nothing is replayed.
    Ephemeral,
}

/// A kind of resource that has a feed of its own.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// A provider.
    Provider,
    /// An agent profile.
    Agent,
    /// A host.
    Host,
    /// A physical machine's resource page and roles.
    Machine,
    /// A template.
    Template,
    /// A tool server.
    McpServer,
    /// A mount.
    Mount,
    /// A skill.
    Skill,
}

impl ResourceKind {
    /// Every kind.
    pub const ALL: [ResourceKind; 8] = [
        Self::Provider,
        Self::Agent,
        Self::Host,
        Self::Machine,
        Self::Template,
        Self::McpServer,
        Self::Mount,
        Self::Skill,
    ];

    /// The kind as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Agent => "agent",
            Self::Host => "host",
            Self::Machine => "machine",
            Self::Template => "template",
            Self::McpServer => "mcp_server",
            Self::Mount => "mount",
            Self::Skill => "skill",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// Something a client can follow.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feed {
    /// One authorized job's current progress and result.
    Job(Id),
    /// A fork/switch batch's current authorized members and aggregate progress.
    Fork(Id),
    /// Live model requests visible under `me`, `node:<id>` or `provider:<id>` usage scope.
    Usage(String),
    /// A thread: its rows, the growing text of its items, and who is
    /// looking.
    Thread(Id),
    /// The raw protocol traffic of a thread's harness.
    ThreadRaw(Id),
    /// Current native commands of existing retained sessions, never replayed.
    ThreadBackground(Id),
    /// Git status in a thread's current directory, requiring its environment's Files View.
    ThreadGit(Id),
    /// A topic: its threads, its jobs and its announcements.
    Topic(Id),
    /// The tree of a root organization.
    Org(Id),
    /// The caller's own: notifications, and what they changed elsewhere.
    User,
    /// A chat: its messages, who is looking and who is typing.
    Chat(Id),
    /// A document: its blocks and discussions, who has it open and who
    /// is writing where.
    Doc(Id),
    /// One bounded document-directory or entry metadata projection for the current person.
    DocumentTree(crate::document_tree::DocumentTreeView),
    /// An environment and the jobs about it.
    Env(Id),
    /// What an environment uses.
    EnvStats(Id),
    /// The processes of an environment, with no thread named.
    EnvProcs(Id),
    /// The reader's terminal sessions, scoped like their terminal GET.
    EnvTerminals(Id),
    /// Current port observations and openings, projected like their environment GET.
    EnvPorts(Id),
    /// Git status in a selected workspace directory.
    EnvGit {
        /// The environment.
        environment_id: Id,
        /// The directory relative to its workspace; empty for the root.
        path: String,
    },
    /// Browsers and profiles, as a whole unretained list.
    EnvBrowsers(Id),
    /// Displays, as a whole unretained list.
    EnvDisplays(Id),
    /// A directory of an environment's workspace, with no thread named.
    EnvDir {
        /// The environment.
        environment_id: Id,
        /// The directory, from the root of the workspace; empty for the
        /// root itself.
        path: String,
    },
    /// A resource: that it changed, and for a provider its sign-in and
    /// the making of its runtime.
    Resource {
        /// The kind of resource.
        kind: ResourceKind,
        /// The resource.
        id: Id,
    },
}

impl Feed {
    /// Whether the feed has a sequence.
    #[must_use]
    pub const fn kind(&self) -> FeedKind {
        match self {
            Self::Thread(_)
            | Self::Topic(_)
            | Self::Org(_)
            | Self::User
            | Self::Chat(_)
            | Self::Doc(_)
            | Self::Env(_) => FeedKind::Durable,
            Self::ThreadRaw(_)
            | Self::Job(_)
            | Self::Fork(_)
            | Self::Usage(_)
            | Self::DocumentTree(_)
            | Self::ThreadBackground(_)
            | Self::ThreadGit(_)
            | Self::EnvStats(_)
            | Self::EnvProcs(_)
            | Self::EnvTerminals(_)
            | Self::EnvPorts(_)
            | Self::EnvGit { .. }
            | Self::EnvBrowsers(_)
            | Self::EnvDisplays(_)
            | Self::EnvDir { .. }
            | Self::Resource { .. } => FeedKind::Ephemeral,
        }
    }

    /// Whether the growing text of items travels on the feed between
    /// their rows.
    #[must_use]
    pub const fn has_deltas(&self) -> bool {
        matches!(self, Self::Thread(_))
    }

    /// Whether the feed says who is looking.
    #[must_use]
    pub const fn has_presence(&self) -> bool {
        matches!(self, Self::Thread(_) | Self::Chat(_) | Self::Doc(_))
    }

    /// Whether the feed says who is typing. A thread's does not: what is
    /// typed there is addressed to an agent.
    #[must_use]
    pub const fn has_typing(&self) -> bool {
        matches!(self, Self::Chat(_) | Self::Doc(_))
    }

    /// The requests, below the API's base path, that load a durable
    /// feed's state and repair it; none for an ephemeral feed.
    #[must_use]
    pub fn sync_paths(&self) -> Vec<String> {
        match self {
            Self::Thread(id) => vec![format!("/threads/{id}/sync")],
            Self::Topic(id) => vec![format!("/topics/{id}/sync")],
            Self::Env(id) => vec![format!("/environments/{id}/sync")],
            Self::Org(id) => vec![format!("/orgs/{id}/tree")],
            Self::User => vec![
                "/notifications".to_owned(),
                "/notifications/summary".to_owned(),
            ],
            Self::Chat(id) => vec![format!("/chats/{id}/sync")],
            Self::Doc(id) => vec![format!("/documents/{id}/sync")],
            _ => Vec::new(),
        }
    }
}

impl std::fmt::Display for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Job(id) => write!(f, "job:{id}"),
            Self::Fork(id) => write!(f, "fork:{id}"),
            Self::Usage(scope) => write!(f, "usage:{scope}"),
            Self::Thread(id) => write!(f, "thread:{id}"),
            Self::ThreadRaw(id) => write!(f, "thread:{id}:raw"),
            Self::ThreadBackground(id) => write!(f, "thread:{id}:background"),
            Self::ThreadGit(id) => write!(f, "thread:{id}:git"),
            Self::Topic(id) => write!(f, "topic:{id}"),
            Self::Org(id) => write!(f, "org:{id}"),
            Self::User => f.write_str("user"),
            Self::Chat(id) => write!(f, "chat:{id}"),
            Self::Doc(id) => write!(f, "doc:{id}"),
            Self::DocumentTree(view) => write!(
                f,
                "document-tree:{}",
                PATH_CODING.encode(serde_json::to_vec(view).map_err(|_| std::fmt::Error)?)
            ),
            Self::Env(id) => write!(f, "env:{id}"),
            Self::EnvStats(id) => write!(f, "env:{id}:stats"),
            Self::EnvProcs(id) => write!(f, "env:{id}:procs"),
            Self::EnvTerminals(id) => write!(f, "env:{id}:terminals"),
            Self::EnvPorts(id) => write!(f, "env:{id}:ports"),
            Self::EnvGit {
                environment_id,
                path,
            } => write!(f, "env:{environment_id}:git:{}", PATH_CODING.encode(path)),
            Self::EnvBrowsers(id) => write!(f, "env:{id}:browsers"),
            Self::EnvDisplays(id) => write!(f, "env:{id}:displays"),
            Self::EnvDir {
                environment_id,
                path,
            } => {
                write!(f, "env:{environment_id}:dir:{}", PATH_CODING.encode(path))
            }
            Self::Resource { kind, id } => write!(f, "resource:{}:{id}", kind.as_str()),
        }
    }
}

/// A text that names no feed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not the name of a feed")]
pub struct BadFeed(pub String);

/// An identifier written the one way identifiers are written: lowercase,
/// with hyphens.
fn canonical_id(text: &str) -> Option<Id> {
    text.parse::<Id>().ok().filter(|id| id.to_string() == text)
}

/// Whether `path` names a directory of a workspace: parts joined by
/// slashes, none of them empty or a step up or in place, no byte zero
/// (no file system has one in a name), and the empty path for the root.
#[must_use]
pub fn is_workspace_path(path: &str) -> bool {
    path.len() <= MAX_PATH_BYTES
        && !path.contains('\0')
        && (path.is_empty()
            || path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."))
}

fn decode_path(coded: &str) -> Option<String> {
    let bytes = PATH_CODING.decode(coded).ok()?;
    String::from_utf8(bytes)
        .ok()
        .filter(|path| is_workspace_path(path))
}

impl std::str::FromStr for Feed {
    type Err = BadFeed;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || BadFeed(text.to_owned());
        let mut parts = text.split(':');
        let family = parts.next().ok_or_else(bad)?;
        let id = |part: Option<&str>| part.and_then(canonical_id).ok_or_else(bad);
        let feed = match family {
            "usage" => {
                let mut scope = match parts.next() {
                    Some("me") => "me".to_owned(),
                    Some(kind @ ("node" | "provider")) => format!("{kind}:{}", id(parts.next())?),
                    _ => return Err(bad()),
                };
                match parts.next() {
                    None => {}
                    Some("org") => scope.push_str(&format!(":org:{}", id(parts.next())?)),
                    // A provider's health alone, without its records: what its status bar follows.
                    Some("health") if scope.starts_with("provider:") => scope.push_str(":health"),
                    Some(_) => return Err(bad()),
                }
                Self::Usage(scope)
            }
            "user" => Self::User,
            "job" => Self::Job(id(parts.next())?),
            "fork" => Self::Fork(id(parts.next())?),
            "topic" => Self::Topic(id(parts.next())?),
            "org" => Self::Org(id(parts.next())?),
            "chat" => Self::Chat(id(parts.next())?),
            "doc" => Self::Doc(id(parts.next())?),
            "document-tree" => {
                let coded = parts.next().ok_or_else(bad)?;
                if coded.len() > crate::version::MAX_CLIENT_FRAME_BYTES {
                    return Err(bad());
                }
                let bytes = PATH_CODING.decode(coded).map_err(|_| bad())?;
                let view: crate::document_tree::DocumentTreeView =
                    serde_json::from_slice(&bytes).map_err(|_| bad())?;
                if matches!(&view,crate::document_tree::DocumentTreeView::Directory {limit:Some(limit),..} if !(1..=500).contains(limit))
                {
                    return Err(bad());
                }
                Self::DocumentTree(view)
            }
            "thread" => {
                let thread_id = id(parts.next())?;
                match parts.next() {
                    None => Self::Thread(thread_id),
                    Some("raw") => Self::ThreadRaw(thread_id),
                    Some("background") => Self::ThreadBackground(thread_id),
                    Some("git") => Self::ThreadGit(thread_id),
                    Some(_) => return Err(bad()),
                }
            }
            "env" => {
                let environment_id = id(parts.next())?;
                match parts.next() {
                    None => Self::Env(environment_id),
                    Some("stats") => Self::EnvStats(environment_id),
                    Some("procs") => Self::EnvProcs(environment_id),
                    Some("terminals") => Self::EnvTerminals(environment_id),
                    Some("ports") => Self::EnvPorts(environment_id),
                    Some("git") => Self::EnvGit {
                        environment_id,
                        path: parts.next().and_then(decode_path).ok_or_else(bad)?,
                    },
                    Some("browsers") => Self::EnvBrowsers(environment_id),
                    Some("displays") => Self::EnvDisplays(environment_id),
                    Some("dir") => {
                        let path = parts.next().and_then(decode_path).ok_or_else(bad)?;
                        Self::EnvDir {
                            environment_id,
                            path,
                        }
                    }
                    Some(_) => return Err(bad()),
                }
            }
            "resource" => {
                let kind = parts.next().and_then(ResourceKind::parse).ok_or_else(bad)?;
                Self::Resource {
                    kind,
                    id: id(parts.next())?,
                }
            }
            _ => return Err(bad()),
        };
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(feed)
    }
}

impl Serialize for Feed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Feed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Feed {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Feed".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string" })
    }
}

// ---------------------------------------------------------------- durable feeds

/// What a `sync` frame of a thread's feed carries. Every member is there
/// only when rows of its kind changed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadData {
    /// Exact read projection; a different follow-active scope requires replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<orch_api_contract::threads::ThreadContinuationProjection>,
    /// Bounded current-reader metadata for returned or affected actual prompt inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_summaries: Vec<orch_api_contract::threads::ThreadPromptSummary>,
    /// This view's own edited prompt/status, independent of the active execution's job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewed_prompt: Option<orch_api_contract::threads::ThreadPromptVersion>,
    /// Together with inputs_queue_revision, replaces the bounded queue page cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs_next_after: Option<Id>,
    /// Presence replaces queue-page metadata; absent next_after then means its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs_queue_revision: Option<i64>,
    /// False requires a fresh bounded HTTP load instead of merging this cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<bool>,
    /// The thread itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Thread>,
    /// Items of its conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<Item>>,
    /// Turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<Vec<Turn>>,
    /// What was sent to its agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<ThreadInput>>,
    /// Its stretches on one agent, provider and environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<Segment>>,
}

/// What a `sync` frame of a topic's feed carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TopicData {
    /// The topic itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<Topic>,
    /// Its threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<Vec<Thread>>,
    /// The jobs about its threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs: Option<Vec<Job>>,
    /// What was announced on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announcements: Option<Vec<Announcement>>,
}

/// What a `sync` frame of an environment's feed carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvData {
    /// The environment itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Environment>,
    /// The jobs about it: each whole where its reader may read it, or the current job's safe
    /// progress alone for a reader who sees the environment only through a thread, as its HTTP
    /// state gives them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs: Option<Vec<EnvironmentSyncJob>>,
}

/// What a `sync` frame of an organization's feed carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OrgData {
    /// The nodes of its tree that the caller may view. One that was
    /// deleted arrives as a row that says so; a private one the caller
    /// administers and may not view arrives as its minimal row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<Listed<TreeNode>>>,
}

/// The two counters of a person's inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UserDataSummary {
    /// How many notifications are unread.
    pub unread: i64,
    /// How many approvals wait for this person.
    pub approvals_pending: i64,
}

/// What a `sync` frame of the caller's own feed carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UserData {
    /// Notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<Vec<foreign::Notification>>,
    /// The counters of the inbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<UserDataSummary>,
}

/// What a `sync` frame of a chat's feed carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChatData {
    /// The chat itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<foreign::Chat>,
    /// Messages. An edit, a reaction, a pin and a deletion each arrive
    /// as the message's row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<foreign::ChatMessage>>,
}

/// What a `sync` frame carries, by the feed it is of.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "the data of a frame is read once and handed on; boxing a thread's would put an indirection in every use"
)]
pub enum SyncData {
    /// Of a thread.
    Thread(ThreadData),
    /// Of a topic.
    Topic(TopicData),
    /// Of an environment.
    Env(EnvData),
    /// Of an organization.
    Org(OrgData),
    /// Of the caller's own feed.
    User(UserData),
    /// Of a chat.
    Chat(ChatData),
    /// Of a document.
    Doc(DocData),
}

/// Why the data of a frame is not what its feed carries.
#[derive(Debug, thiserror::Error)]
pub enum DataError {
    /// The feed carries no frames of that sort.
    #[error("the feed `{0}` carries no such frames")]
    NotCarried(String),
    /// The data does not have the members the feed defines.
    #[error("the data is not what the feed carries: {0}")]
    Shape(#[source] serde_json::Error),
    /// What was to be read or written as the data of a frame is not a JSON object.
    #[error("the data of a frame is a JSON object")]
    NotAnObject,
}

/// Writes `value` as the data of a frame, which is a JSON object.
fn object_of<T: Serialize>(value: &T) -> Result<Map<String, Json>, DataError> {
    match serde_json::to_value(value).map_err(DataError::Shape)? {
        Json::Object(object) => Ok(object),
        _ => Err(DataError::NotAnObject),
    }
}

impl SyncData {
    /// The data of a `sync` frame of `feed`.
    pub fn decode(feed: &Feed, data: Json) -> Result<Self, DataError> {
        fn read<T: serde::de::DeserializeOwned>(data: Json) -> Result<T, DataError> {
            if !data.is_object() {
                return Err(DataError::NotAnObject);
            }
            serde_json::from_value(data).map_err(DataError::Shape)
        }
        match feed {
            Feed::Thread(_) => read(data).map(Self::Thread),
            Feed::Topic(_) => read(data).map(Self::Topic),
            Feed::Env(_) => read(data).map(Self::Env),
            Feed::Org(_) => read(data).map(Self::Org),
            Feed::User => read(data).map(Self::User),
            Feed::Chat(_) => read(data).map(Self::Chat),
            Feed::Doc(_) => read(data).map(Self::Doc),
            _ => Err(DataError::NotCarried(feed.to_string())),
        }
    }

    /// The data as the JSON object a `sync` frame carries.
    pub fn to_object(&self) -> Result<Map<String, Json>, DataError> {
        match self {
            Self::Thread(data) => object_of(data),
            Self::Topic(data) => object_of(data),
            Self::Env(data) => object_of(data),
            Self::Org(data) => object_of(data),
            Self::User(data) => object_of(data),
            Self::Chat(data) => object_of(data),
            Self::Doc(data) => object_of(data),
        }
    }
}

// ---------------------------------------------------------------- events

/// News on the caller's own feed about their own state and about what they
/// own: a change they made on another of their connections, or something
/// that happened which concerns them. These are not replayed: after every
/// connect a client loads that state again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "what", rename_all = "snake_case")]
pub enum UserEvent {
    /// This browser session changed its global administrative mode. Other sessions keep theirs.
    AdminMode {
        /// The browser session to which the switch belongs.
        session_id: Id,
        /// Its new switch value.
        as_admin: bool,
    },
    /// How far they have read a thread.
    ReadState {
        /// The thread.
        thread_id: Id,
        /// The last sequence number they have read.
        last_read_seq: Seq,
    },
    /// A read cursor for one retained history, independent of selected execution.
    ContinuationReadState {
        /// The current execution's reader-specific icon after the stored read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_display: Option<orch_api_contract::threads::ThreadDisplay>,
        thread_id: Id,
        continuation_id: Id,
        last_read_seq: Seq,
    },
    /// How much of a place they want to be told.
    Subscription {
        /// The place.
        node_id: Id,
        /// The level; `null` when they took their choice back.
        level: Option<SubscriptionLevel>,
    },
    /// This person's invitation to create an org changed; reread Meta as this caller.
    OrgCreation,
    /// Public metadata inputs changed; reread Meta without borrowing another caller's permission.
    MetaChanged {
        /// The native mutation request, so its actor can join the receipt and its echo.
        /// Recovery and older/internal writers omit it and require a conservative refresh.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
    /// Their favourites changed: load them again.
    Favorites,
    /// What they typed to an agent and did not send.
    Draft {
        /// The thread.
        thread_id: Id,
        /// The draft; `null` when it was sent or cleared.
        draft: Option<Draft>,
    },
    /// They read a chat, changed their level in it, hid it, or were
    /// mentioned in it.
    ChatState {
        /// Their state in that chat.
        state: ChatState,
    },
    /// Their discussion bell on a document changed.
    DocumentState {
        /// The document whose caller projection should be reloaded.
        document_id: Id,
        /// Which discussions they want to be told of.
        discussion_level: orch_api_contract::documents::DocumentDiscussionLevel,
    },
    /// What they typed in a chat and did not send.
    ChatDraft {
        /// The chat.
        chat_id: Id,
        /// The draft; `null` when it was sent or cleared.
        draft: Option<foreign::ChatDraft>,
    },
    /// A new message in a direct or group chat they are in.
    Chat {
        /// The chat.
        chat_id: Id,
        /// The chat's sequence with that message.
        seq: Seq,
        /// When it was written.
        at: Time,
    },
    /// An approval they may decide was made or settled. Counters are projected for this session
    /// after commit, including when its preferences produced no Inbox notification.
    Approvals {
        /// Current native counters and outside-notification status, absent on older senders.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<Box<orch_api_contract::notifications::NotificationSummaryGetReply>>,
    },
    /// One of their machines enrolled, came online or went offline.
    Machines {
        /// The machine.
        machine_id: Id,
    },
    /// This person's enrollment was consumed; progress is projected for the receiving session.
    Enrollment {
        /// Tracking reference, never the enrollment secret or installer command.
        receipt: String,
        /// Current authorized native resource view; omitted by older/internal notices.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<Box<orch_api_contract::resources::EnrollmentProgress>>,
    },
    /// A resource they own was made, changed state or was removed.
    Resources {
        /// The kind of resource, such as `host`, `provider` or `environment`.
        kind: String,
        /// The resource.
        id: Id,
    },
    /// One of their routes out of the application changed without them.
    Route {
        /// The channel the route goes through.
        channel_id: Id,
    },
    /// What the interface keeps for them was changed on another device.
    State {
        /// Which part of it.
        key: PersonalStateKey,
        /// Current authorized value, projected after commit rather than put in PostgreSQL's notice.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<Json>,
    },
}

/// What a resource's feed says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "what", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "an event is read once and handed on; boxing the job of one variant would put an indirection in every use"
)]
pub enum ResourceEvent {
    /// The resource changed. The native view is projected for this recipient; older sources may omit it.
    Changed {
        /// The same safe resource view as its GET, including the recipient's current access.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        row: Option<EverythingResource>,
        /// View was lost while another right still admits this feed (for example private sign-in progress).
        #[serde(default, skip_serializing_if = "view_not_revoked")]
        view_revoked: bool,
    },
    /// A step of a provider's sign-in, for those who manage it.
    Login {
        /// How far the sign-in is.
        state: LoginState,
    },
    /// The making of a provider's runtime, for those who manage it.
    Job {
        /// The job.
        job: Job,
    },
}

const fn view_not_revoked(value: &bool) -> bool {
    !*value
}

/// The processes of an environment at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProcsData {
    /// The moment the list was taken.
    pub as_of: Time,
    /// Every process of the environment, each with the thread it
    /// belongs to where that is known.
    pub processes: Vec<ProcessInfo>,
}

/// Why a requested directory watch could not become live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DirectoryWatchError {
    /// The same normalized code as the directory's HTTP read.
    pub code: orch_api_contract::common::ErrorCode,
    /// A stable native cause where the command supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Safe words about the failed read, never a credential or command body.
    pub detail: String,
    /// Whether explicit fresh demand can try again.
    pub retryable: bool,
}

/// What changed among the direct children of a watched directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DirData {
    /// A failed activation is not an empty successful directory or an offline machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DirectoryWatchError>,
    /// Immutable observed metadata and a continuation, bound like the HTTP directory read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// The next HTTP page; absent for the actual end or an older connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// The selector's idle expiry, subject to earlier metadata-cache eviction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// Start of an initial metadata observation; absent for deltas, failures or an older peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_from: Option<Time>,
    /// Keep displayed rows, but restart before appending another page of this directory.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invalidated: bool,
    /// True replaces the directory's previous entries; false applies only the named changes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
    /// Whether the directory exceeded the native list bound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Initial observation completion, or when a later change batch was gathered.
    pub as_of: Time,
    /// The entries that are new or changed.
    pub changed: Vec<DirEntry>,
    /// The names of those that are gone.
    pub removed: Vec<String>,
}

/// A current Git observation, tied to the environment and directory it actually describes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GitData {
    /// The environment whose connector read the repository.
    pub environment_id: Id,
    /// The selected workspace directory; empty for the root.
    pub path: String,
    /// The same status as the repository read returns, including unsupported reasons.
    pub status: orch_api_contract::workspace::GitStatus,
}

/// The live-shell page and the recent closed-shell page, with truthful continuations for both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalsData {
    /// At most two hundred opening/running shells, scoped like state=open on the GET.
    pub open: orch_api_contract::common::Page<orch_api_contract::terminals::TerminalSession>,
    /// The ten most recently opened shells that have ended, scoped like state=closed on the GET.
    pub closed: orch_api_contract::common::Page<orch_api_contract::terminals::TerminalSession>,
}

/// One current authorized job, including the matching GET's progress-only cut where needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JobData {
    /// Its current row.
    pub job: Job,
}

/// One current batch, with exactly the matching GET's member visibility and hidden count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ForkData {
    /// Its current members and progress.
    pub batch: orch_api_contract::forks::ForkBatch,
}

/// The whole list of an environment's browsers and profiles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BrowsersData {
    /// Every browser the environment knows of.
    pub browsers: Vec<crate::foreign::Browser>,
    /// The profiles it keeps.
    pub profiles: Vec<crate::foreign::BrowserProfile>,
}

/// The whole list of an environment's displays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DisplaysData {
    /// Every display the environment knows of.
    pub displays: Vec<crate::foreign::Display>,
}

/// The word that the environment behind a feed is offline. It is sent
/// once, and the feed then stays silent until the environment is back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "OfflineWire", into = "OfflineWire")]
pub struct Offline;

/// [`Offline`] as it is written.
#[derive(Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "Offline")]
struct OfflineWire {
    offline: bool,
}

impl From<Offline> for OfflineWire {
    fn from(_: Offline) -> Self {
        Self { offline: true }
    }
}

impl TryFrom<OfflineWire> for Offline {
    type Error = &'static str;

    fn try_from(wire: OfflineWire) -> Result<Self, Self::Error> {
        if wire.offline {
            Ok(Self)
        } else {
            Err("`offline` is only ever sent as true")
        }
    }
}

/// What a feed that watches an environment says: what it sees, or that
/// the environment is offline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "{T}OrOffline")]
pub enum Watched<T> {
    /// The environment is offline.
    Offline(Offline),
    /// What the watch sees.
    Seen(T),
}

/// Bounded provider health, without model-request rows or names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HealthData {
    /// The exact status API projection for this reader.
    pub health: orch_api_contract::usage::UsageDegradation,
}

/// One masked raw observation from an explicitly admitted native segment/history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RawData {
    /// Exact native segment; raw numbers are unique only within this source.
    pub segment_id: Id,
    /// The history admitted for this recipient, including whether it follows active.
    pub projection: orch_api_contract::threads::ThreadContinuationProjection,
    /// The retained native observation, masked again before publication.
    pub event: RawEvent,
}

/// What an `event` frame carries, by the feed it is of.
#[derive(Debug, Clone, PartialEq)]
pub enum EventData {
    /// Current person-authorized document-directory or entry metadata.
    DocumentTree(crate::document_tree::DocumentTreeData),
    /// A job's current visible progress/result.
    Job(JobData),
    /// A batch's current visible members and progress.
    Fork(ForkData),
    /// A bounded newest-first page of model requests, rechecked for this subscriber.
    Usage(orch_api_contract::common::Page<orch_api_contract::usage::UsageEvent>),
    /// Of usage:provider:<id>:health, which carries no request log.
    UsageHealth(HealthData),
    /// Of the caller's own feed.
    User(UserEvent),
    /// Of a chat's or a document's: somebody is typing.
    Typing(TypingNotice),
    /// Of the feed of what an environment uses.
    Stats(Watched<ResourceSample>),
    /// Of a feed of processes.
    Procs(Watched<ProcsData>),
    /// The same bounded terminal page the matching GET returns, scoped for this reader.
    Terminals(Watched<TerminalsData>),
    /// The current port view, with names and actions projected for this reader.
    Ports(Watched<orch_api_contract::ports::EnvironmentPorts>),
    /// Git status for a watched workspace or thread directory.
    Git(Watched<GitData>),
    /// Of a feed of a directory.
    Dir(Watched<DirData>),
    /// Of the list of browsers and profiles.
    Browsers(Watched<BrowsersData>),
    /// Of the list of displays.
    Displays(Watched<DisplaysData>),
    /// Of the feed of a harness's raw traffic.
    Raw(RawData),
    /// A reader's current native roster and exact End authority.
    Background(orch_api_contract::background::ThreadBackground),
    /// Of a resource's.
    Resource(ResourceEvent),
}

impl EventData {
    /// The data of an `event` frame of `feed`.
    pub fn decode(feed: &Feed, data: Json) -> Result<Self, DataError> {
        fn read<T: serde::de::DeserializeOwned>(data: Json) -> Result<T, DataError> {
            if !data.is_object() {
                return Err(DataError::NotAnObject);
            }
            serde_json::from_value(data).map_err(DataError::Shape)
        }
        match feed {
            Feed::DocumentTree(_) => read(data).map(Self::DocumentTree),
            Feed::Job(_) => read(data).map(Self::Job),
            Feed::Fork(_) => read(data).map(Self::Fork),
            Feed::Usage(scope) if scope.ends_with(":health") => read(data).map(Self::UsageHealth),
            Feed::Usage(_) => read(data).map(Self::Usage),
            Feed::User => read(data).map(Self::User),
            Feed::Chat(_) | Feed::Doc(_) => read(data).map(Self::Typing),
            Feed::EnvStats(_) => read(data).map(Self::Stats),
            Feed::EnvProcs(_) => read(data).map(Self::Procs),
            Feed::EnvTerminals(_) => read(data).map(Self::Terminals),
            Feed::EnvPorts(_) => read(data).map(Self::Ports),
            Feed::EnvGit { .. } | Feed::ThreadGit(_) => read(data).map(Self::Git),
            Feed::EnvDir { .. } => read(data).map(Self::Dir),
            Feed::EnvBrowsers(_) => read(data).map(Self::Browsers),
            Feed::EnvDisplays(_) => read(data).map(Self::Displays),
            Feed::ThreadRaw(_) => read(data).map(Self::Raw),
            Feed::ThreadBackground(_) => read(data).map(Self::Background),
            Feed::Resource { .. } => read(data).map(Self::Resource),
            Feed::Thread(_) | Feed::Topic(_) | Feed::Org(_) | Feed::Env(_) => {
                Err(DataError::NotCarried(feed.to_string()))
            }
        }
    }

    /// The data as the JSON object an `event` frame carries.
    pub fn to_object(&self) -> Result<Map<String, Json>, DataError> {
        match self {
            Self::DocumentTree(data) => object_of(data),
            Self::Job(data) => object_of(data),
            Self::Fork(data) => object_of(data),
            Self::Usage(data) => object_of(data),
            Self::UsageHealth(data) => object_of(data),
            Self::User(data) => object_of(data),
            Self::Typing(data) => object_of(data),
            Self::Stats(data) => object_of(data),
            Self::Procs(data) => object_of(data),
            Self::Terminals(data) => object_of(data),
            Self::Ports(data) => object_of(data),
            Self::Git(data) => object_of(data),
            Self::Dir(data) => object_of(data),
            Self::Browsers(data) => object_of(data),
            Self::Displays(data) => object_of(data),
            Self::Raw(data) => object_of(data),
            Self::Background(data) => object_of(data),
            Self::Resource(data) => object_of(data),
        }
    }
}

/// Rows and identifiers shared by the tests of this package.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use serde_json::json;

    pub(crate) const A: &str = "0199c1a0-0000-7000-8000-00000000000a";
    pub(crate) const B: &str = "0199c1a0-0000-7000-8000-00000000000b";
    pub(crate) const AT: &str = "2026-10-08T17:04:05.123Z";

    pub(crate) fn id() -> Id {
        A.parse().unwrap()
    }

    pub(crate) fn user() -> Json {
        json!({ "id": B, "kind": "human", "username": "bo", "display_name": "Bo", "avatar": { "text": "B", "color": 2 } })
    }

    pub(crate) fn agent_message(status: &str, text: &str) -> Json {
        json!({
            "id": "0199c1a0-0000-7000-8000-0000000000a9",
            "thread_id": A,
            "segment_id": "0199c1a0-0000-7000-8000-0000000000a3",
            "turn_id": "0199c1a0-0000-7000-8000-0000000000a4",
            "stream": "main", "parent_item_id": null,
            "kind": "agent_message",
            "payload": { "text": text, "text_len": text.chars().count(), "phase": null },
            "status": status, "author": null, "pinned": null, "chat_refs": 0, "before_orch": false,
            "seq_created": 414, "seq_updated": 415, "created_at": AT, "updated_at": AT
        })
    }

    /// The members of every node, then those of a thread: written in parts, because a literal this
    /// long runs a macro out of its recursion limit.
    pub(crate) fn thread() -> Json {
        let node = json!({
            "id": A, "kind": "thread", "parent_id": B, "org_id": B,
            "path": [{ "id": B, "kind": "org", "name": "Acme", "redacted": false }],
            "name": "Fix the login", "description": "", "color": null, "readme": null, "owner": user(), "owners": 1,
            "title_source": "user", "description_source": "auto", "auto_updated_at": null,
            "tags": [], "tags_source": "auto",
            "page": "follows", "chat_id": B, "archived": false, "archived_at": null, "archived_by": null,
            "revision": 3, "created_at": AT, "updated_at": AT
        });
        let access = json!({
            "access": {
                "permissions": ["node.view", "thread.view"], "role": "admin", "role_from": "system",
                "administers": true, "owns": true, "subscription": null, "effective_subscription": "off"
            }
        });
        let placing = json!({
            "topic_id": B, "state": "running",
            "display": { "seq": 415, "read_seq": 0, "state": "running", "queued_inputs": 0,
                "held_inputs": 0, "waiting_inputs": 0, "waiting_for_answer": false, "blocked_reason": null, "finished_at": null },
            "side_of": null, "side_from_turn": null, "sides": null,
            "side_reading": null,
            "starting_step": null, "blocked": null,
            "queue": { "held": false, "by": null, "at": null, "held_inputs": 0, "revision": 1 },
            "binding": null, "environment": null, "environment_permissions": [],
            "directory": "", "source": null, "links": [], "suggested_links": [], "work": null,
            "attention": { "uncommitted": null, "unpushed": null, "failed": false, "as_of": null, "stale": false, "reason": null },
            "model": null, "model_display": null, "effort": null, "cwd": "/work", "directory_threads": 1,
            "memory": { "kept_by": "agent", "native": false, "why": null }, "lock": null
        });
        let standing = json!({
            "input_access": { "allowed": true, "missing": [] },
            "can": { "fork": true, "archive": true, "export": true, "place": false },
            "current_turn_id": null, "current_segment_id": null, "pending_approvals": 0, "pending_inputs": 0,
            "usage": null, "last_activity_at": AT, "seq": 415, "activity_seq": 415, "my_last_read_seq": null
        });
        let mut written = Json::Object(Map::new());
        for part in [node, access, placing, standing] {
            if let (Some(whole), Json::Object(more)) = (written.as_object_mut(), part) {
                whole.extend(more);
            }
        }
        written
    }

    pub(crate) fn input(state: &str) -> Json {
        json!({
            "id": "0199c1a0-0000-7000-8000-0000000000a5", "thread_id": A, "author": user(), "run_as": null,
            "plan": false, "source": "user",
            "content": [{ "type": "text", "text": "Read the handler" }], "delivery": "queue", "state": state,
            "state_detail": null, "attachment_issues": [], "queue_held": false,
            "limit_bypass": [], "position": "1", "turn_id": null, "client_id": "k1",
            "created_at": AT, "delivered_at": null, "seq_updated": 415
        })
    }

    pub(crate) fn job() -> Json {
        json!({
            "id": A, "kind": "provider_provision", "node_id": null, "environment_id": null, "provider_id": B,
            "batch_id": null, "state": "running",
            "progress": {
                "phase": "build", "phases": [{ "name": "build", "weight": 100, "state": "running" }], "percent": 40,
                "files_done": 0, "files_total": null, "bytes_done": 0, "bytes_total": null, "rate_bps": null,
                "eta_seconds": null, "message": "Step 3 of 7"
            },
            "result": null, "error": null, "log": "Step 3/7 : RUN make", "created_by": null,
            "created_at": AT, "started_at": AT, "finished_at": null
        })
    }

    pub(crate) fn process() -> Json {
        json!({
            "pid": 4711, "ppid": 1, "name": "cargo", "command": "cargo test", "user": "agent", "state": "S",
            "cpu_percent": 12.5, "memory_bytes": 1_048_576, "started_at": AT, "thread_id": A, "item_id": null
        })
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{A, AT, B, agent_message, id, input, job, process, thread, user};
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_hints_round_trip_without_a_caller_permission_or_user_payload() {
        for raw in [
            json!({"what":"org_creation"}),
            json!({"what":"meta_changed"}),
            json!({"what":"meta_changed","request_id":"committed-request"}),
        ] {
            let event: UserEvent = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(serde_json::to_value(event).unwrap(), raw);
            let EventData::User(event) = EventData::decode(&Feed::User, raw.clone()).unwrap()
            else {
                panic!("a user hint must survive native event decoding");
            };
            assert_eq!(serde_json::to_value(event).unwrap(), raw);
        }
    }

    #[test]
    fn an_initial_directory_reset_retains_its_observation_interval_on_the_wire() {
        let raw = json!({"as_of":AT,"observed_from":"2026-10-08T17:04:00.000Z","reset":true,
            "revision":"live:observed","next_cursor":"opaque-next","changed":[],"removed":[]});
        let feed = Feed::EnvDir {
            environment_id: id(),
            path: "src".to_owned(),
        };
        let EventData::Dir(Watched::Seen(read)) = EventData::decode(&feed, raw.clone()).unwrap()
        else {
            panic!("not a directory reset")
        };
        assert!(read.observed_from.is_some());
        assert_eq!(serde_json::to_value(read).unwrap(), raw);
    }

    fn every_feed() -> Vec<(Feed, String)> {
        vec![
            (Feed::Usage("me".to_owned()), "usage:me".to_owned()),
            (
                Feed::Usage(format!("me:org:{A}")),
                format!("usage:me:org:{A}"),
            ),
            (Feed::Usage(format!("node:{A}")), format!("usage:node:{A}")),
            (
                Feed::Usage(format!("provider:{A}")),
                format!("usage:provider:{A}"),
            ),
            (
                Feed::Usage(format!("provider:{A}:health")),
                format!("usage:provider:{A}:health"),
            ),
            (Feed::Thread(id()), format!("thread:{A}")),
            (Feed::ThreadRaw(id()), format!("thread:{A}:raw")),
            (Feed::ThreadGit(id()), format!("thread:{A}:git")),
            (Feed::Job(id()), format!("job:{A}")),
            (Feed::Fork(id()), format!("fork:{A}")),
            (Feed::Topic(id()), format!("topic:{A}")),
            (Feed::Org(id()), format!("org:{A}")),
            (Feed::User, "user".to_owned()),
            (Feed::Chat(id()), format!("chat:{A}")),
            (Feed::Doc(id()), format!("doc:{A}")),
            (Feed::Env(id()), format!("env:{A}")),
            (Feed::EnvStats(id()), format!("env:{A}:stats")),
            (Feed::EnvProcs(id()), format!("env:{A}:procs")),
            (Feed::EnvTerminals(id()), format!("env:{A}:terminals")),
            (Feed::EnvPorts(id()), format!("env:{A}:ports")),
            (
                Feed::EnvGit {
                    environment_id: id(),
                    path: "src".into(),
                },
                format!("env:{A}:git:c3Jj"),
            ),
            (
                Feed::EnvDir {
                    environment_id: id(),
                    path: String::new(),
                },
                format!("env:{A}:dir:"),
            ),
            (
                Feed::Resource {
                    kind: ResourceKind::McpServer,
                    id: id(),
                },
                format!("resource:mcp_server:{A}"),
            ),
        ]
    }

    // ------------------------------------------------------------ names

    #[test]
    fn every_feed_is_written_and_read_by_its_name() {
        for (feed, name) in every_feed() {
            assert_eq!(feed.to_string(), name);
            assert_eq!(name.parse::<Feed>(), Ok(feed.clone()));
            assert_eq!(serde_json::to_value(&feed).unwrap(), name);
            assert_eq!(serde_json::from_value::<Feed>(json!(name)).unwrap(), feed);
        }
    }

    #[test]
    fn every_kind_of_resource_has_a_feed_of_its_own() {
        let kinds = [
            "provider",
            "agent",
            "host",
            "machine",
            "template",
            "mcp_server",
            "mount",
            "skill",
        ];
        assert_eq!(
            ResourceKind::ALL.len(),
            kinds.len(),
            "zip must cover every resource kind"
        );
        for (kind, name) in ResourceKind::ALL.into_iter().zip(kinds) {
            assert_eq!(kind.as_str(), name);
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
            let feed: Feed = format!("resource:{name}:{A}").parse().unwrap();
            assert_eq!(feed, Feed::Resource { kind, id: id() });
        }
    }

    #[test]
    fn input_omission_and_draft_null_keep_their_distinct_old_reader_contracts()
    -> Result<(), Box<dyn std::error::Error>> {
        let canonical = input("cancelled");
        let mut legacy = canonical.clone();
        legacy["native_action"] = Json::Null;
        let read: orch_api_contract::threads::ThreadInput = serde_json::from_value(legacy)?;
        assert!(read.native_action.is_none());
        assert_eq!(serde_json::to_value(read)?, canonical);
        let canonical = json!({"native_action":null,"content":[],"delivery":"queue","review_comments":[],"updated_at":AT});
        let mut legacy = canonical.clone();
        legacy
            .as_object_mut()
            .ok_or("draft fixture is not an object")?
            .remove("native_action");
        let read: orch_api_contract::personal::Draft = serde_json::from_value(legacy)?;
        assert!(read.native_action.is_none());
        assert_eq!(serde_json::to_value(read)?, canonical);
        Ok(())
    }

    #[test]
    fn a_directory_is_named_by_its_path_whatever_characters_it_has() {
        let feed = Feed::EnvDir {
            environment_id: id(),
            path: "docs/übersicht/a b".into(),
        };
        let name = feed.to_string();
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":-_".contains(&byte)),
            "{name}"
        );
        assert_eq!(name.parse::<Feed>(), Ok(feed));
    }

    #[test]
    fn a_path_is_read_with_or_without_padding_and_written_without() {
        let padded = format!("env:{A}:dir:c3JjL2FwaQ==");
        let feed: Feed = padded.parse().unwrap();
        assert_eq!(
            feed,
            Feed::EnvDir {
                environment_id: id(),
                path: "src/api".into()
            }
        );
        assert_eq!(feed.to_string(), format!("env:{A}:dir:c3JjL2FwaQ"));
    }

    #[test]
    fn a_name_that_is_no_feeds_is_refused() {
        let upper = format!("thread:{}", A.to_uppercase());
        let unknown_part = format!("thread:{A}:logs");
        let no_path = format!("env:{A}:dir");
        let not_coded = format!("env:{A}:dir:src/api");
        let through_a_thread = [
            format!("thread:{A}:procs"),
            format!("thread:{A}:dir:c3JjL2FwaQ"),
        ];
        let steps_up = format!("env:{A}:dir:{}", PATH_CODING.encode("../etc"));
        let absolute = format!("env:{A}:dir:{}", PATH_CODING.encode("/etc"));
        let nul = format!("env:{A}:dir:{}", PATH_CODING.encode("a\0b"));
        let trailing = format!("topic:{A}:");
        let unknown_kind = format!("resource:secret:{A}");
        let stats_of_a_thread = format!("thread:{A}:stats");
        let braces = format!("thread:{{{A}}}");
        let names = [
            "",
            "users",
            "user:",
            "thread",
            "thread:",
            "thread:not-an-id",
            &upper,
            &unknown_part,
            &no_path,
            &not_coded,
            &steps_up,
            &absolute,
            &nul,
            &trailing,
            &unknown_kind,
            &stats_of_a_thread,
            &braces,
        ];
        for name in names {
            assert!(name.parse::<Feed>().is_err(), "{name}");
        }
        for name in &through_a_thread {
            assert!(
                name.parse::<Feed>().is_err(),
                "{name}: no feed of an environment goes through a thread"
            );
        }
    }

    #[test]
    fn a_workspace_path_has_no_empty_parts_and_no_steps_up_or_in_place() {
        for path in ["", "src", "src/api", "a b/ü"] {
            assert!(is_workspace_path(path), "{path}");
        }
        for path in [
            "/",
            "/src",
            "src/",
            "src//api",
            ".",
            "src/./api",
            "..",
            "src/../etc",
            "a\0b",
        ] {
            assert!(!is_workspace_path(path), "{path:?}");
        }
        assert!(is_workspace_path(&"a".repeat(MAX_PATH_BYTES)));
        assert!(!is_workspace_path(&"a".repeat(MAX_PATH_BYTES + 1)));
    }

    #[test]
    fn seven_feeds_have_a_sequence_and_the_others_replay_nothing() {
        let durable: Vec<String> = every_feed()
            .into_iter()
            .filter(|(feed, _)| feed.kind() == FeedKind::Durable)
            .map(|(_, name)| name.split(':').next().unwrap().to_owned())
            .collect();
        assert_eq!(
            durable,
            ["thread", "topic", "org", "user", "chat", "doc", "env"]
        );
        for (feed, name) in every_feed() {
            assert_eq!(
                feed.sync_paths().is_empty(),
                feed.kind() == FeedKind::Ephemeral,
                "{name}"
            );
        }
    }

    #[test]
    fn each_durable_feed_is_loaded_and_repaired_by_its_own_request() {
        assert_eq!(
            Feed::Thread(id()).sync_paths(),
            [format!("/threads/{A}/sync")]
        );
        assert_eq!(
            Feed::Topic(id()).sync_paths(),
            [format!("/topics/{A}/sync")]
        );
        assert_eq!(
            Feed::Env(id()).sync_paths(),
            [format!("/environments/{A}/sync")]
        );
        assert_eq!(Feed::Org(id()).sync_paths(), [format!("/orgs/{A}/tree")]);
        assert_eq!(Feed::Chat(id()).sync_paths(), [format!("/chats/{A}/sync")]);
        assert_eq!(
            Feed::Doc(id()).sync_paths(),
            [format!("/documents/{A}/sync")]
        );
        assert_eq!(
            Feed::User.sync_paths(),
            ["/notifications", "/notifications/summary"]
        );
    }

    #[test]
    fn only_a_thread_streams_text_and_a_thread_never_says_who_types() {
        assert!(Feed::Thread(id()).has_deltas());
        assert!(!Feed::Chat(id()).has_deltas());
        assert!(
            Feed::Thread(id()).has_presence()
                && Feed::Chat(id()).has_presence()
                && Feed::Doc(id()).has_presence()
        );
        assert!(!Feed::Topic(id()).has_presence());
        assert!(Feed::Chat(id()).has_typing() && Feed::Doc(id()).has_typing());
        assert!(!Feed::Thread(id()).has_typing());
    }

    // ------------------------------------------------------------ rows

    #[test]
    fn the_rows_of_a_thread_arrive_under_the_names_of_their_kinds() {
        let data = json!({
            "thread": thread(),
            "items": [agent_message("in_progress", "")],
            "inputs": [input("cancelled")],
            "turns": [],
            "segments": []
        });
        let SyncData::Thread(read) = SyncData::decode(&Feed::Thread(id()), data.clone()).unwrap()
        else {
            panic!("not a thread's data")
        };
        assert_eq!(read.items.as_ref().map(Vec::len), Some(1));
        assert_eq!(read.thread.as_ref().map(|thread| thread.seq), Some(415));
        // A cancelled input is how the withdrawal of one arrives; the row stays.
        assert_eq!(
            read.inputs.as_ref().map(|inputs| inputs[0].seq_updated),
            Some(415)
        );
        assert_eq!(serde_json::to_value(&read).unwrap(), data);
        // What did not change is not there at all.
        assert_eq!(
            serde_json::to_value(ThreadData::default()).unwrap(),
            json!({})
        );
        let only_items = SyncData::decode(&Feed::Thread(id()), json!({ "items": [] })).unwrap();
        assert!(matches!(
            only_items,
            SyncData::Thread(ThreadData {
                thread: None,
                inputs: None,
                ..
            })
        ));
    }

    #[test]
    fn a_row_that_is_not_a_row_of_the_feed_is_refused() {
        let wrong = json!({ "items": [ { "id": "i9" } ] });
        assert!(matches!(
            SyncData::decode(&Feed::Thread(id()), wrong),
            Err(DataError::Shape(_))
        ));
        // Data is an object, whatever else a struct could be read from.
        assert!(matches!(
            SyncData::decode(&Feed::Thread(id()), json!([])),
            Err(DataError::NotAnObject)
        ));
        assert!(matches!(
            EventData::decode(&Feed::User, json!("favorites")),
            Err(DataError::NotAnObject)
        ));
    }

    #[test]
    fn the_other_durable_feeds_carry_their_own_rows() {
        let node = json!({
            "id": A, "kind": "thread", "parent_id": B, "name": "Fix the login", "archived": false,
            "address": null, "color": null, "docs": 0,
            "restricted": false, "deleted": true, "tree_seq": 77, "topic": null,
            "thread": { "state": "idle",
                "display": { "seq": 412, "read_seq": 0, "state": "idle", "queued_inputs": 0,
                    "held_inputs": 0, "waiting_inputs": 0, "waiting_for_answer": false, "blocked_reason": null, "finished_at": null },
                "blocked_reason": null, "pending_approvals": 0, "activity_seq": 412, "last_activity_at": null,
                "attention": { "uncommitted": null, "unpushed": null, "failed": false, "as_of": null, "stale": false, "reason": null } },
            "chat": { "id": B, "seq": 31 },
            "content_hidden": false
        });
        let hidden = json!({
            "id": B, "parent_id": A, "kind": "topic", "name": "Payroll", "owner": null,
            "content_hidden": true, "private": true
        });
        let org = json!({ "nodes": [node, hidden] });
        let SyncData::Org(read) = SyncData::decode(&Feed::Org(id()), org.clone()).unwrap() else {
            panic!("not a tree")
        };
        // A node that was deleted arrives as a row that says so, and the tree sequence is its stamp.
        assert_eq!(
            read.nodes.as_ref().and_then(|nodes| nodes[0]
                .as_visible()
                .map(|node| (node.deleted, node.tree_seq))),
            Some((true, 77))
        );
        // A private node its reader administers is its minimal row, and nothing else.
        assert_eq!(
            read.nodes
                .as_ref()
                .and_then(|nodes| nodes[1].as_hidden().map(|row| row.name.clone())),
            Some("Payroll".to_owned())
        );
        assert_eq!(serde_json::to_value(&read).unwrap(), org);

        let environment = json!({ "jobs": [job()] });
        let SyncData::Env(read) = SyncData::decode(&Feed::Env(id()), environment.clone()).unwrap()
        else {
            panic!("not an environment")
        };
        assert_eq!(read.jobs.as_ref().map(Vec::len), Some(1));
        assert_eq!(serde_json::to_value(&read).unwrap(), environment);

        let topic = json!({ "jobs": [job()], "threads": [], "announcements": [] });
        let SyncData::Topic(read) = SyncData::decode(&Feed::Topic(id()), topic.clone()).unwrap()
        else {
            panic!("not a topic")
        };
        assert_eq!(read.threads.as_ref().map(Vec::len), Some(0));
        assert_eq!(serde_json::to_value(&read).unwrap(), topic);

        let chat = json!({
            "chat": {
                "id": A, "kind": "direct", "org_id": B, "node": null, "name": null, "members": [], "guests": [],
                "created_by": null, "created_at": AT, "seq": 12, "last_message_seq": 12, "last_message_at": AT,
                "can": { "send": true, "moderate": false, "export": true, "reason": null }, "my": null, "preview": null
            },
            "messages": [ {
                "id": B, "chat_id": A, "kind": "message", "author": null, "parts": [], "system": null,
                "reply_to": null, "mentions": [], "mentions_all": false, "mention_targets": [], "asked_agents": [], "reactions": [], "pinned": null,
                "edited_at": null, "deleted": true, "seq_created": 4, "seq_updated": 6, "created_at": AT
            } ]
        });
        let SyncData::Chat(read) = SyncData::decode(&Feed::Chat(id()), chat.clone()).unwrap()
        else {
            panic!("not a chat")
        };
        assert_eq!(read.messages.as_ref().map(Vec::len), Some(1));
        assert_eq!(serde_json::to_value(&read).unwrap(), chat);
    }

    #[test]
    fn the_callers_own_feed_carries_notifications_and_the_two_counters() {
        let user = json!({ "notifications": [ {
            "id": A, "kind": "milestone", "title": "Ready", "body": "The release is ready",
            "node": null, "thread_id": null, "approval_id": null, "chat_id": null,
            "url": "/notifications", "data": {}, "created_at": "2026-10-14T10:00:00.000Z",
            "read_at": null, "inbox_seq": 7, "group": null, "waiting_for_me": false
        } ], "summary": { "unread": 3, "approvals_pending": 1 } });
        let SyncData::User(read) = SyncData::decode(&Feed::User, user.clone()).unwrap() else {
            panic!("not the caller's data")
        };
        assert_eq!(
            read.summary,
            Some(UserDataSummary {
                unread: 3,
                approvals_pending: 1
            })
        );
        assert_eq!(serde_json::to_value(&read).unwrap(), user);
        assert_eq!(read.notifications.as_ref().unwrap()[0].title, "Ready");
        assert!(matches!(
            SyncData::decode(
                &Feed::User,
                json!({"notifications": [{"op": "ping", "n": 1}]})
            ),
            Err(DataError::Shape(_))
        ));
        assert!(matches!(
            SyncData::decode(&Feed::User, json!({ "summary": { "unread": 3 } })),
            Err(DataError::Shape(_))
        ));
    }

    #[test]
    fn a_feed_without_a_sequence_has_no_sync_data() {
        for (feed, name) in every_feed() {
            let read = SyncData::decode(&feed, json!({}));
            assert_eq!(read.is_ok(), feed.kind() == FeedKind::Durable, "{name}");
            if let Err(error) = read {
                assert!(matches!(error, DataError::NotCarried(_)), "{name}");
            }
        }
    }

    #[test]
    fn data_is_written_as_the_object_it_was_read_from() {
        let data =
            json!({ "items": [agent_message("completed", "I'll start by reading the handler.")] });
        let read = SyncData::decode(&Feed::Thread(id()), data.clone()).unwrap();
        assert_eq!(Json::Object(read.to_object().unwrap()), data);
    }

    // ------------------------------------------------------------ events

    #[test]
    fn what_a_person_changed_elsewhere_reads_and_writes_as_the_protocol_has_it() {
        let events = [
            json!({ "what": "read_state", "thread_id": A, "last_read_seq": 412 }),
            json!({ "what": "subscription", "node_id": A, "level": null }),
            json!({ "what": "subscription", "node_id": A, "level": "important" }),
            json!({ "what": "favorites" }),
            json!({ "what": "document_state", "document_id": A, "discussion_level": "all" }),
            json!({ "what": "document_state", "document_id": A, "discussion_level": "involved" }),
            json!({ "what": "draft", "thread_id": A, "draft": null }),
            json!({ "what": "draft", "thread_id": A, "draft": {
                "native_action": null,
                "content": [{ "type": "text", "text": "Read the handler" }], "delivery": "queue", "review_comments": [],
                "updated_at": AT } }),
            json!({ "what": "chat_state", "state": {
                "chat_id": A, "level": null, "engaged": true, "effective_level": "all",
                "last_read_seq": 9, "read_revision": 0, "mentions_unread": 0, "hidden": false } }),
            json!({ "what": "chat_draft", "chat_id": A, "draft": { "parts": [], "reply_to": null, "updated_at": AT } }),
            json!({ "what": "chat", "chat_id": A, "seq": 31, "at": AT }),
            json!({ "what": "approvals" }),
            json!({ "what": "machines", "machine_id": A }),
            json!({ "what": "resources", "kind": "host", "id": A }),
            json!({ "what": "resources", "kind": "environment", "id": A }),
            json!({ "what": "route", "channel_id": A }),
            json!({ "what": "state", "key": "layouts" }),
        ];
        for event in events {
            let EventData::User(read) = EventData::decode(&Feed::User, event.clone()).unwrap()
            else {
                panic!("not an event of the caller's feed")
            };
            assert_eq!(serde_json::to_value(&read).unwrap(), event);
        }
        assert!(matches!(
            EventData::decode(&Feed::User, json!({ "what": "weather" })),
            Err(DataError::Shape(_))
        ));
        // A key of the interface that nothing keeps is refused, not passed on.
        assert!(matches!(
            EventData::decode(
                &Feed::User,
                json!({ "what": "state", "key": "arrangements" })
            ),
            Err(DataError::Shape(_))
        ));
    }

    #[test]
    fn what_a_watch_sees_reads_and_writes_as_the_protocol_has_it() {
        let procs = json!({ "as_of": AT, "processes": [process()] });
        let EventData::Procs(Watched::Seen(seen)) =
            EventData::decode(&Feed::EnvProcs(id()), procs.clone()).unwrap()
        else {
            panic!("not a list of processes")
        };
        assert_eq!(seen.processes.len(), 1);
        assert_eq!(serde_json::to_value(&seen).unwrap(), procs);

        let dir = json!({
            "as_of": AT,
            "changed": [{
                "name": "main.rs", "path": "src/main.rs", "type": "file", "size": 120, "mtime": AT,
                "mode": 420, "link_target": null, "cached": false
            }],
            "removed": ["old.rs"]
        });
        let feed = Feed::EnvDir {
            environment_id: id(),
            path: "src".into(),
        };
        let EventData::Dir(Watched::Seen(seen)) = EventData::decode(&feed, dir.clone()).unwrap()
        else {
            panic!("not a directory's changes")
        };
        assert_eq!(seen.removed, ["old.rs"]);
        assert_eq!(serde_json::to_value(&seen).unwrap(), dir);

        let stats = json!({
            "at": AT, "cpu_percent": 12.5, "cpu_limit_millis": 2_000, "memory_bytes": 1_048_576, "memory_limit_bytes": null,
            "disk_used_bytes": null, "disk_limit_bytes": null, "net_rx_bps": 0, "net_tx_bps": 0, "pids": 12
        });
        let EventData::Stats(Watched::Seen(seen)) =
            EventData::decode(&Feed::EnvStats(id()), stats.clone()).unwrap()
        else {
            panic!("not a sample")
        };
        assert_eq!(serde_json::to_value(&seen).unwrap(), stats);

        assert!(
            EventData::decode(
                &Feed::ThreadRaw(id()),
                json!({"n":7,"at":AT,
            "direction":"from_harness","payload":{"method":"x"}})
            )
            .is_err(),
            "bare segment-local numbers can never decode as a source-bound raw event"
        );
        let raw = json!({"segment_id":id(),"projection":{
            "viewed_continuation_id":id(),"active_continuation_id":id(),"execution_revision":1,
            "projection_revision":1,"follows_active":false},
            "event":{ "n": 7, "at": AT, "direction": "from_harness", "payload": { "method": "x" } }});
        let EventData::Raw(seen) = EventData::decode(&Feed::ThreadRaw(id()), raw.clone()).unwrap()
        else {
            panic!("not raw traffic")
        };
        assert_eq!(seen.event.n, 7);
        assert_eq!(serde_json::to_value(&seen).unwrap(), raw);
    }

    #[test]
    fn a_feed_whose_environment_is_offline_says_so_once() {
        let offline = json!({ "offline": true });
        let feeds = [
            Feed::EnvStats(id()),
            Feed::EnvProcs(id()),
            Feed::EnvDir {
                environment_id: id(),
                path: String::new(),
            },
        ];
        for feed in feeds {
            let read = EventData::decode(&feed, offline.clone()).unwrap();
            assert!(
                matches!(
                    read,
                    EventData::Stats(Watched::Offline(_))
                        | EventData::Procs(Watched::Offline(_))
                        | EventData::Dir(Watched::Offline(_))
                ),
                "{feed}"
            );
            assert_eq!(Json::Object(read.to_object().unwrap()), offline);
        }
        assert_eq!(
            serde_json::to_value(Watched::<ProcsData>::Offline(Offline)).unwrap(),
            offline
        );
        // Only ever sent as true.
        assert!(serde_json::from_value::<Offline>(json!({ "offline": false })).is_err());
        assert!(EventData::decode(&Feed::EnvStats(id()), json!({ "offline": false })).is_err());
    }

    #[test]
    fn a_resource_says_that_it_changed_and_a_provider_how_its_sign_in_goes() {
        let events = [
            json!({ "what": "changed" }),
            json!({ "what": "login", "state": {
                "state": "awaiting_user", "method": "device_code", "shows": [], "asks": null, "expires_at": null, "error": null } }),
            json!({ "what": "job", "job": job() }),
        ];
        let feed = Feed::Resource {
            kind: ResourceKind::Provider,
            id: id(),
        };
        for event in events {
            let EventData::Resource(read) = EventData::decode(&feed, event.clone()).unwrap() else {
                panic!("not a resource's event")
            };
            assert_eq!(serde_json::to_value(&read).unwrap(), event);
        }
        assert!(matches!(
            EventData::decode(&Feed::Thread(id()), json!({})),
            Err(DataError::NotCarried(_))
        ));
    }

    #[test]
    fn typing_is_the_event_of_a_chat_and_of_a_document() {
        let typing = json!({ "what": "typing", "user": user() });
        for feed in [Feed::Chat(id()), Feed::Doc(id())] {
            let read = EventData::decode(&feed, typing.clone()).unwrap();
            assert!(matches!(read, EventData::Typing(_)));
            assert_eq!(Json::Object(read.to_object().unwrap()), typing);
        }
    }
}
