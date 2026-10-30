//! The shared tree as paths: how a place, a thread, a conversation, a chat, a document and its versions are named, how an agent's reach is laid out as directories and files, and how a path is found again.
//!
//! The reach of an agent (`orch_authz::reach`) is presented as one tree, laid out as the hierarchy is, so that
//! an agent finds its way in it with what it knows about file systems: `/<root org>/<org>…/<project>/<topic>/
//! <thread>/`, and in each directory, as far as each was shared, `conversation.md`, `chat.md` and `docs/` (the
//! documents, each `<title>.md`, with `<title>.history/` for its earlier versions). `/<root org>/messages/` holds the
//! direct and group chats that were shared. A path without a leading `/` is read from the agent's own topic.
//!
//! **Names.** A place and a thread go by their names, an org by its short name, a document by its title with
//! `.md`; a `/` in a name is written `-`. Where two entries of one directory would get one name, or a name is
//! one the layout reserves, every one of them but the oldest has `~` and the last four hex characters of its identifier
//! appended (more of the tail where those collide). Names
//! are given among **all** the entries of a directory, whether or not the agent may see them, so that a name does
//! not change when something is shared; the cost is that such a suffix tells that an older entry of that name exists.
//!
//! **What is there.** A directory is in the tree when the agent may read something in it or under it (or it is on
//! the agent's own way, or a thread of its own topic, which is always listed); the places on the way to a thing
//! appear as directories with nothing else in them. What cannot be read is not listed, and is answered, when
//! asked for, as what is not there. One thing is listed that cannot be read: a document that was deleted, to an agent
//! that may bring it back (it may manage it), marked as deleted, beside the ones that are there and under a name of
//! its own that takes none of theirs.
//!
//! The tree is read for a scope at a moment, in a few statements for any number of entries (the rows of every page
//! are read together), and not kept: a share that was removed is gone from the next call.

use crate::document_tree as organization;
use crate::error::EngineError;
use crate::written::{FILES_SUFFIX, HISTORY_SUFFIX};
use globset::{Glob, GlobMatcher};
use orch_api_contract::share::{SharedEntryKind, SharedEntryUnit, SharedEntryVia};
use orch_authz::documents::Document;
use orch_authz::reach::{self, Agent, AgentReach, Reach, Standing, Things, Via};
use orch_db::Conn;
use orch_db::queries::documents as document_rows;
use orch_db::queries::shares::{self as rows, PlaceRow};
use orch_types::{Id, Time};
#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::{HashMap, HashSet, VecDeque};

pub(crate) mod attachments;
mod folders;
mod loading;

/// The most entries one listing gives.
pub const ENTRIES_MAX: usize = 500;

/// The most nodes read in one database page of the agent's selected reach; every continuation is followed.
pub const NODES_MAX: i64 = 20_000;

/// The most characters of the opening of a description.
pub const DESCRIPTION_CHARS: usize = 200;

/// How many characters of an identifier make the suffix that tells entries of one name apart, at the least.
pub const SUFFIX_CHARS: usize = 4;

/// The names the layout reserves in a directory.
const CONVERSATION: &str = "conversation.md";
const CHAT: &str = "chat.md";
const DOCS: &str = "docs";
const MESSAGES: &str = "messages";
const DOCUMENT_SUFFIX: &str = ".md";

/// The most characters of the name of a chat made of the names of its people.
const CHAT_NAME_CHARS: usize = 80;

// ---------------------------------------------------------------- names

/// What the tree calls a name: a `/` written `-`, and nothing that would be a path itself.
#[must_use]
pub fn tree_name(name: &str) -> String {
    let name = name.replace('/', "-");
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return format!("_{name}_");
    }
    name.to_owned()
}

/// Whether the layout reserves a name in every directory.
fn reserved(name: &str) -> bool {
    matches!(name, CONVERSATION | CHAT | DOCS | MESSAGES)
        || name.ends_with(FILES_SUFFIX)
        || name.ends_with(HISTORY_SUFFIX)
}

/// The names of entries of one directory, oldest first: each its own, and where an entry before it (or the layout)
/// has the name already, `~` and as many trailing hex characters as it takes to differ from every name
/// before it, at least [`SUFFIX_CHARS`]. `extension` is kept after the suffix (`.md` for a document).
#[must_use]
pub fn assign_names(entries: &[(Id, String)], extension: &str, keep_reserved: bool) -> Vec<String> {
    assign_names_after(HashSet::new(), entries, extension, keep_reserved)
}

/// The same, among entries that come after some that have their names already: the names of the later ones differ
/// from `taken` too, and the earlier ones are left as they are. A document that was deleted is named so, after those
/// that are there, so that it takes no name away from them.
#[must_use]
pub fn assign_names_after(
    mut taken: HashSet<String>,
    entries: &[(Id, String)],
    extension: &str,
    keep_reserved: bool,
) -> Vec<String> {
    let mut out = Vec::with_capacity(entries.len());
    for (id, name) in entries {
        let bare = tree_name(name);
        let whole = format!("{bare}{extension}");
        let clash = taken.contains(&whole) || (keep_reserved && reserved(&whole));
        let chosen = if clash {
            let digits = id.simple().to_string();
            (SUFFIX_CHARS..=digits.len())
                .map(|length| format!("{bare}~{}{extension}", &digits[digits.len() - length..]))
                .find(|candidate| {
                    !taken.contains(candidate) && !(keep_reserved && reserved(candidate))
                })
                .unwrap_or_else(|| format!("{bare}~{digits}{extension}"))
        } else {
            whole
        };
        taken.insert(chosen.clone());
        out.push(chosen);
    }
    out
}

/// The name of a twin without its tilde suffix, and the identifier tail that suffix spells.
fn twin_of(name: &str, extension: &str) -> Option<(String, String)> {
    let stem = name.strip_suffix(extension).unwrap_or(name);
    let (bare, tail) = stem.rsplit_once('~')?;
    (tail.len() >= SUFFIX_CHARS && tail.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| (bare.to_owned(), tail.to_lowercase()))
}

/// The entries a name can mean; a bare name retains all twins rather than silently picking the oldest.
fn matching_names(
    entries: &[(Id, String)],
    names: &[String],
    part: &str,
    extension: &str,
) -> Vec<usize> {
    // A case-insensitive alias must never shadow a literal virtual directory/file.
    if extension.is_empty() && reserved(part) {
        return Vec::new();
    }
    let bare: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(at, (_, name))| {
            (!(extension.is_empty() && reserved(part))
                && !(extension.is_empty() && reserved(&tree_name(name)))
                && format!("{}{extension}", tree_name(name)) == part)
                .then_some(at)
        })
        .collect();
    if bare.len() > 1 {
        return bare;
    }
    if let Some(at) = names.iter().position(|name| name == part) {
        return vec![at];
    }
    if !bare.is_empty() {
        return bare;
    }
    if let Some((bare, tail)) = twin_of(part, extension) {
        return entries
            .iter()
            .enumerate()
            .filter_map(|(at, (id, name))| {
                (tree_name(name).eq_ignore_ascii_case(&bare)
                    && id.simple().to_string().ends_with(&tail))
                .then_some(at)
            })
            .collect();
    }
    entries
        .iter()
        .enumerate()
        .filter_map(|(at, (_, name))| {
            (names
                .get(at)
                .is_some_and(|name| name.eq_ignore_ascii_case(part))
                || (!(extension.is_empty() && reserved(&tree_name(name)))
                    && format!("{}{extension}", tree_name(name)).eq_ignore_ascii_case(part)))
            .then_some(at)
        })
        .collect()
}

/// The opening of a text in one line.
#[must_use]
pub fn opening(text: &str, chars: usize) -> String {
    crate::written::opening(
        text.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or(""),
        chars,
    )
}

// ---------------------------------------------------------------- the paths an agent writes

/// A path as an agent writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Named {
    /// `id:` and an identifier.
    Id(Id),
    /// Names, from the top (`/…`), or from the agent's own topic.
    Path {
        /// Whether it began with `/`.
        absolute: bool,
        /// Its parts, none of them empty.
        parts: Vec<String>,
    },
}

/// Reads what an agent wrote as a path. Nothing for an `id:` that is no identifier.
#[must_use]
pub fn parse_path(text: &str) -> Option<Named> {
    let text = text.trim();
    if let Some(id) = text.strip_prefix("id:") {
        return id.trim().parse::<Id>().ok().map(Named::Id);
    }
    let absolute = text.starts_with('/');
    let parts: Vec<String> = text
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect();
    Some(Named::Path { absolute, parts })
}

/// A pattern over the names of entries, as a shell has it: `*`, `?` and `[…]`.
#[derive(Debug, Clone)]
pub struct Pattern(GlobMatcher);

impl Pattern {
    /// Reads a pattern; nothing where it is none.
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        Glob::new(text.trim())
            .ok()
            .map(|glob| Self(glob.compile_matcher()))
    }

    /// Whether a name matches.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        self.0.is_match(name)
    }
}

// ---------------------------------------------------------------- the entries

/// Where an entry of the tree stands, which says what is read to give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Where {
    /// The top of everything.
    Root,
    /// A directory that is a node: an org, a project, a topic or a thread.
    Place(Id),
    /// A thread's conversation.
    Conversation(Id),
    /// The chat of a node.
    NodeChat(Id),
    /// A direct or group chat.
    PeopleChat(Id),
    /// The `docs` directory beside a node.
    Docs(Id),
    /// A real document-only folder; hierarchy nodes and virtual file/history directories are distinct.
    DocumentFolder(Id),
    /// A document.
    Document(Id),
    /// The immutable virtual directory of one readable document's attached files.
    DocumentFiles(Id),
    /// One current attached file, addressed through its owning document.
    DocumentFile { document: Id, file: Id },
    /// The immutable virtual directory of one readable chat's carried files.
    ChatFiles(Id),
    /// A carried file in the existing written-chat order (one-based, including forwarded files).
    ChatFile {
        chat: Id,
        message: Id,
        carried: usize,
    },
    /// The directory of a document's earlier versions.
    History(Id),
    /// One of them.
    Version(Id, i32),
    /// The `messages` directory of a root org.
    Messages(Id),
    /// The directory of one shared direct or group chat.
    PeopleChatDir(Id),
}

/// One entry of a listing.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Where it stands in the tree.
    pub path: String,
    /// Its name.
    pub name: String,
    /// What it is.
    pub kind: SharedEntryKind,
    /// Its identifier.
    pub id: Id,
    /// What reads it.
    pub at: Where,
    /// For a place, a thread and a document: the opening of its description; for a document that has none, of
    /// its first paragraph.
    pub description: Option<String>,
    /// What it is counted in.
    pub unit: Option<SharedEntryUnit>,
    /// How many of that unit it has.
    pub parts: Option<i64>,
    /// Its size in bytes.
    pub size: Option<i64>,
    /// When it last changed.
    pub updated_at: Option<Time>,
    /// For a thread: what it is doing.
    pub state: Option<String>,
    /// For a topic: open or done.
    pub status: Option<String>,
    /// Archived itself or below an archived place.
    pub archived: bool,
    /// For a thread: its agent profile.
    pub agent: Option<String>,
    /// For a thread: its environment.
    pub environment: Option<String>,
    /// For a thread in the asking agent's own environment: its directory there.
    pub directory: Option<String>,
    /// Whether it is the asking agent's own thread.
    pub is_self: bool,
    /// For a directory: how many entries it has.
    pub children: Option<i64>,
    /// Whether the agent may read it.
    pub readable: bool,
    /// Whether the agent may write it.
    pub writable: bool,
    /// Whether the agent may delete, move and bring back a document, put an earlier version of it back, or do
    /// that to every document that stands in a `docs` directory.
    pub manageable: bool,
    /// Whether it is a document that was deleted, listed because the agent may bring it back.
    pub deleted: bool,
    /// Why the agent has it.
    pub via: SharedEntryVia,
    /// Who shared it, where a share names it.
    pub shared_by: Option<Id>,
}

impl Entry {
    fn bare(path: String, name: String, kind: SharedEntryKind, id: Id, at: Where) -> Self {
        Self {
            path,
            name,
            kind,
            id,
            at,
            description: None,
            unit: None,
            parts: None,
            size: None,
            updated_at: None,
            state: None,
            status: None,
            archived: false,
            agent: None,
            environment: None,
            directory: None,
            is_self: false,
            children: None,
            readable: true,
            writable: false,
            manageable: false,
            deleted: false,
            via: SharedEntryVia::Share,
            shared_by: None,
        }
    }

    /// Whether it is a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        matches!(
            self.kind,
            SharedEntryKind::Org
                | SharedEntryKind::Project
                | SharedEntryKind::Topic
                | SharedEntryKind::Thread
                | SharedEntryKind::Folder
        ) || matches!(self.at, Where::PeopleChatDir(_))
    }
}

fn via_of(via: Via) -> SharedEntryVia {
    match via {
        Via::Policy => SharedEntryVia::Policy,
        Via::Share => SharedEntryVia::Share,
        Via::Topic => SharedEntryVia::Topic,
    }
}

fn kind_of(word: &str) -> SharedEntryKind {
    match word {
        "org" => SharedEntryKind::Org,
        "project" => SharedEntryKind::Project,
        "topic" => SharedEntryKind::Topic,
        _ => SharedEntryKind::Thread,
    }
}

// ---------------------------------------------------------------- the tree in memory

/// What a thread's conversation is, read.
#[derive(Debug, Clone)]
struct Conversation {
    standing: Standing,
    turns: i64,
    updated: Option<chrono::DateTime<chrono::Utc>>,
}

/// What a chat is, read.
#[derive(Debug, Clone)]
struct ChatNode {
    id: Id,
    standing: Standing,
    messages: i64,
    updated: Option<chrono::DateTime<chrono::Utc>>,
}

/// A document the agent may read, or one that was deleted and that it may bring back.
#[derive(Debug, Clone)]
pub struct DocEntry {
    /// The document as stored.
    pub row: document_rows::DocumentRow,
    /// The document as the access engine knows it.
    pub document: Document,
    /// Its file name in `docs`.
    pub name: String,
    /// What the agent may do with it.
    pub standing: Standing,
    /// How many top-level blocks it has.
    pub top_blocks: i64,
}

/// A direct or group chat the agent may read.
#[derive(Debug, Clone)]
pub struct PeopleEntry {
    /// The chat.
    pub chat: Id,
    /// What it is called: its name, or the names of its people.
    pub label: String,
    /// Its name in `messages`.
    pub name: String,
    /// What the agent may do with it.
    pub standing: Standing,
    /// How many messages it has.
    pub messages: i64,
    /// When the last one was written.
    pub updated: Option<chrono::DateTime<chrono::Utc>>,
}

/// A directory that is a node.
#[derive(Debug)]
struct Dir {
    place: PlaceRow,
    name: String,
    parent: Option<usize>,
    children: Vec<usize>,
    /// On the agent's own way, or a thread of its own topic: always listed.
    on_way: bool,
    conversation: Option<Conversation>,
    chat: Option<ChatNode>,
    docs: Vec<DocEntry>,
    /// The documents beside it that were deleted and that the agent may bring back; none where it is archived.
    deleted: Vec<DocEntry>,
    docs_standing: Standing,
    present: bool,
    /// The most that can be made out of it for a person who asks: whether it is a place the agent holds a share on.
    standing: Standing,
}

/// A directory in the tree: where a listing begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Its implicit and granted subtrees, with the ancestor directories that make their paths.
    Everything,
    /// A node and what lies below it.
    Under(Id),
}

/// The tree an agent reaches, read for a scope.
#[derive(Debug)]
pub struct Tree {
    reach: Reach,
    agent: Agent,
    now: Time,
    dirs: Vec<Dir>,
    by_id: HashMap<Id, usize>,
    /// The top: root orgs, by name.
    roots: Vec<usize>,
    root_names: HashMap<usize, String>,
    messages: HashMap<Id, Vec<PeopleEntry>>,
    attachment_dirs: HashSet<Where>,
    organized: HashMap<organization::Item, organization::Listed>,
    folder_ancestors: HashMap<Id, organization::Ancestor>,
    document_names: HashMap<organization::Item, String>,
    agent_names: HashMap<Id, String>,
    environment_names: HashMap<Id, String>,
    own_environment: Option<Id>,
    capped: bool,
    scope: Scope,
    /// The path of the place the scope begins at, up to and not including it: `/` for the top.
    base: String,
    /// The directory the scope begins at, when it is one node.
    scope_at: Option<usize>,
}

/// A directory found by its path: where it stands, and the path to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    /// What the directory is.
    pub at: Where,
    /// Its path, from the top, ending in `/` for a directory.
    pub path: String,
}

/// What a listing is of, besides its entries.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The directory listed.
    pub path: String,
    /// What it is, where it is a place.
    pub about: Option<About>,
    /// What is in it.
    pub entries: Vec<Entry>,
    /// Whether there were more than are given.
    pub truncated: bool,
}

/// The order of a tool listing; names and path identities do not depend on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    /// Active work, then most recently changed, then name and stable path.
    #[default]
    Changed,
    /// Directories and files by name, as a file-system listing.
    Name,
}

/// One page of an agent's directory listing, with a continuation and a counted archive fold.
pub struct ListingPage {
    /// The existing listing shape.
    pub listing: Listing,
    /// The last path of this page when another page remains.
    pub next: Option<String>,
    /// Readable archived entries folded without traversing their subtrees.
    pub archived_count: i64,
}

/// A tool page's order, archive choice, bound and last returned path.
pub struct PageOptions<'a> {
    /// How rows are ordered within each directory.
    pub sort: Sort,
    /// Whether archived entries are included rather than counted.
    pub include_archived: bool,
    /// The already-validated page bound.
    pub limit: usize,
    /// The last returned path, from a caller/query-bound cursor.
    pub after: Option<&'a str>,
}

/// What a listed place is.
#[derive(Debug, Clone, PartialEq)]
pub struct About {
    /// Its kind.
    pub kind: SharedEntryKind,
    /// Its name.
    pub name: String,
    /// What it is about.
    pub description: Option<String>,
    /// For a topic: `open` or `done`.
    pub status: Option<String>,
    /// For a topic: its tags.
    pub tags: Vec<String>,
}

/// The nodes on the way down to `node`, root org first, itself last, each with the name the tree gives it among
/// its siblings, so that they are the names a listing shows.
pub async fn names_along(conn: &mut Conn, node: Id) -> Result<Vec<(Id, String)>, EngineError> {
    let Some(place) = rows::places(conn, &[node]).await?.into_iter().next() else {
        return Ok(Vec::new());
    };
    let way = rows::places(conn, &place.path).await?;
    let mut names = Vec::with_capacity(place.path.len());
    for id in &place.path {
        let Some(on_the_way) = way.iter().find(|candidate| candidate.id == *id) else {
            return Ok(Vec::new());
        };
        let Some(parent) = on_the_way.parent_id else {
            names.push((
                *id,
                on_the_way
                    .slug
                    .clone()
                    .unwrap_or_else(|| tree_name(&on_the_way.name)),
            ));
            continue;
        };
        let siblings = rows::children_of(conn, parent).await?;
        let named: Vec<(Id, String)> = siblings
            .iter()
            .map(|sibling| {
                (
                    sibling.id,
                    sibling.slug.clone().unwrap_or_else(|| sibling.name.clone()),
                )
            })
            .collect();
        let assigned = assign_names(&named, "", true);
        let name = siblings
            .iter()
            .zip(assigned)
            .find(|(sibling, _)| sibling.id == *id)
            .map(|(_, name)| name)
            .unwrap_or_else(|| tree_name(&on_the_way.name));
        names.push((*id, name));
    }
    Ok(names)
}

/// Where following a path begins, and how far the places on it go: reading the whole tree for every call would
/// make each call cost as much as everything an agent reaches, so the places are followed first, by name, and the
/// tree is read under the deepest one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walked {
    /// What to read the tree for.
    pub scope: Scope,
    /// How many parts of the path the places took.
    pub consumed: usize,
    /// The places the walk passed, from the root org down.
    pub chain: Vec<Id>,
}

/// A path written from the agent's topic, written from the top: the names of the nodes on the way to the topic
/// first, `..` taken out. An identifier stays one.
pub async fn absolute(conn: &mut Conn, agent: &Agent, named: &Named) -> Result<Named, EngineError> {
    let Named::Path { absolute, parts } = named else {
        return Ok(named.clone());
    };
    let mut done: Vec<String> = if *absolute {
        Vec::new()
    } else {
        names_along(conn, agent.topic)
            .await?
            .into_iter()
            .map(|(_, name)| name)
            .collect()
    };
    for part in parts {
        if part == ".." {
            done.pop();
        } else {
            done.push(part.clone());
        }
    }
    Ok(Named::Path {
        absolute: true,
        parts: done,
    })
}

/// A full identifier tail is an existing unambiguous path alias; no sibling inventory is needed.
fn header_name(place: &PlaceRow) -> String {
    if place.parent_id.is_none() {
        place.slug.clone().unwrap_or_else(|| tree_name(&place.name))
    } else {
        format!(
            "{}~{}",
            tree_name(place.slug.as_deref().unwrap_or(&place.name)),
            place.id.simple()
        )
    }
}

async fn header_names_along(conn: &mut Conn, node: Id) -> Result<Vec<(Id, String)>, EngineError> {
    let Some(place) = rows::header_places(conn, &[node]).await?.into_iter().next() else {
        return Ok(Vec::new());
    };
    let path = rows::header_places(conn, &place.path).await?;
    Ok(place
        .path
        .iter()
        .filter_map(|id| {
            path.iter()
                .find(|row| row.id == *id)
                .map(|row| (*id, header_name(row)))
        })
        .collect())
}

/// Resolves a relative metadata path without reading its topic's sibling inventory.
pub(crate) async fn absolute_headers(
    conn: &mut Conn,
    agent: &Agent,
    named: &Named,
) -> Result<Named, EngineError> {
    let Named::Path { absolute, parts } = named else {
        return Ok(named.clone());
    };
    let mut done: Vec<_> = if *absolute {
        Vec::new()
    } else {
        header_names_along(conn, agent.topic)
            .await?
            .into_iter()
            .map(|(_, name)| name)
            .collect()
    };
    for part in parts {
        if part == ".." {
            done.pop();
        } else {
            done.push(part.clone());
        }
    }
    Ok(Named::Path {
        absolute: true,
        parts: done,
    })
}

async fn header_child(
    conn: &mut Conn,
    parent: Id,
    part: &str,
) -> Result<Option<PlaceRow>, EngineError> {
    if reserved(part) {
        return Ok(None);
    }
    for folded in [false, true] {
        let value = if folded {
            "translate(orch_related_tree_name(n.name),'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')"
        } else {
            "orch_related_tree_name(n.name)"
        };
        let asked = if folded {
            "translate($2,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')"
        } else {
            "$2"
        };
        let ids: Vec<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT n.id FROM nodes n WHERE n.parent_id=$1 AND n.deleted_at IS NULL
             AND orch_related_name_key({value})=orch_related_name_key({asked}) AND {value}={asked}
             AND NOT EXISTS(SELECT 1 FROM threads t WHERE t.node_id=n.id AND t.side_of IS NOT NULL) LIMIT 2"
        ))).bind(parent).bind(part).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if !ids.is_empty() {
            return if ids.len() == 1 {
                Ok(rows::header_places(conn, &ids).await?.into_iter().next())
            } else {
                Ok(None)
            };
        }
    }
    if let Some((bare, tail)) = twin_of(part, "") {
        if tail.len() == 32
            && let Ok(id) = tail.parse::<Id>()
        {
            return Ok(rows::header_places(conn, &[id])
                .await?
                .into_iter()
                .find(|row| {
                    row.parent_id == Some(parent)
                        && tree_name(row.slug.as_deref().unwrap_or(&row.name))
                            .eq_ignore_ascii_case(&bare)
                }));
        }
        // Older short aliases are still bounded; a full id tail remains the unambiguous spelling.
        let ids:Vec<Id>=sqlx::query_scalar("SELECT id FROM nodes WHERE parent_id=$1 AND deleted_at IS NULL
          AND orch_related_name_key(translate(orch_related_tree_name(name),'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))=
              orch_related_name_key(translate($2,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
          AND translate(orch_related_tree_name(name),'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')=translate($2,'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')
          AND right(replace(id::text,'-',''),length($3))=$3 LIMIT 2")
          .bind(parent).bind(bare).bind(tail).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if ids.len() == 1 {
            return Ok(rows::header_places(conn, &ids).await?.into_iter().next());
        }
    }
    Ok(None)
}

/// Follows only the named metadata path, with at most two candidate identities at every component.
pub(crate) async fn walk_headers(conn: &mut Conn, named: &Named) -> Result<Walked, EngineError> {
    let parts = match named {
        Named::Id(id) => return scope_of_id(conn, *id).await,
        Named::Path { parts, .. } => parts,
    };
    let mut current: Option<PlaceRow> = None;
    let mut chain = Vec::new();
    for part in parts {
        let next = match &current {
            None => rows::root_org_named(conn, part).await?,
            Some(row) => header_child(conn, row.id, part).await?,
        };
        let Some(next) = next else {
            break;
        };
        chain.push(next.id);
        current = Some(next);
    }
    Ok(Walked {
        scope: current.map_or(Scope::Everything, |row| Scope::Under(row.id)),
        consumed: chain.len(),
        chain,
    })
}

/// A child of a place that a name means, among `children` (oldest first) named `names`: its own name, a twin's
/// name with its identifier tail, or the one name that differs only in case.
fn pick<'a>(children: &'a [PlaceRow], names: &[String], part: &str) -> Option<&'a PlaceRow> {
    let entries: Vec<_> = children
        .iter()
        .map(|child| {
            (
                child.id,
                child.slug.clone().unwrap_or_else(|| child.name.clone()),
            )
        })
        .collect();
    let matches = matching_names(&entries, names, part, "");
    (matches.len() == 1)
        .then(|| children.get(matches[0]))
        .flatten()
}

/// Follows a path (written from the top) down the places, by name, and says where it stopped.
pub async fn walk(conn: &mut Conn, named: &Named) -> Result<Walked, EngineError> {
    let parts = match named {
        Named::Path { parts, .. } => parts,
        Named::Id(id) => return scope_of_id(conn, *id).await,
    };
    let mut current: Option<PlaceRow> = None;
    let mut chain: Vec<Id> = Vec::new();
    for part in parts {
        let next = match &current {
            None => rows::root_org_named(conn, part).await?,
            Some(place) => {
                let children = rows::children_of(conn, place.id).await?;
                let named: Vec<(Id, String)> = children
                    .iter()
                    .map(|child| {
                        (
                            child.id,
                            child.slug.clone().unwrap_or_else(|| child.name.clone()),
                        )
                    })
                    .collect();
                let names = assign_names(&named, "", true);
                pick(&children, &names, part).cloned()
            }
        };
        match next {
            Some(place) => {
                chain.push(place.id);
                current = Some(place);
            }
            None => break,
        }
    }
    Ok(Walked {
        scope: current.map_or(Scope::Everything, |place| Scope::Under(place.id)),
        consumed: chain.len(),
        chain,
    })
}

/// The scope to read for an identifier: the node it is, the node a document or a chat is beside, or the root
/// org of a direct or group chat.
async fn scope_of_id(conn: &mut Conn, id: Id) -> Result<Walked, EngineError> {
    let scope = if let Some(place) = rows::places(conn, &[id]).await?.into_iter().next() {
        Scope::Under(place.id)
    } else if let Some(row) = document_rows::get(conn, id).await? {
        Scope::Under(row.node_id)
    } else if let Some(chat) = rows::chat_home(conn, id).await? {
        Scope::Under(chat)
    } else {
        Scope::Everything
    };
    Ok(Walked {
        scope,
        consumed: 0,
        chain: Vec::new(),
    })
}

/// The identifier of an earlier version of a document, which no document and no other version has.
#[must_use]
pub fn version_id(document: Id, number: i32) -> Id {
    crate::ids::derived(
        "version",
        &[
            document.as_bytes().as_slice(),
            number.to_le_bytes().as_slice(),
        ],
    )
}

fn path_of(prefix: &str, name: &str, directory: bool) -> String {
    if directory {
        format!("{prefix}{name}/")
    } else {
        format!("{prefix}{name}")
    }
}

impl Tree {
    /// Reads the tree for `scope`: the agent's reach, a few statements for any number of things.
    pub async fn load(
        conn: &mut Conn,
        agent: Agent,
        policy: AgentReach,
        documents_policy: reach::DocumentPolicy,
        now: Time,
        scope: Scope,
    ) -> Result<Self, EngineError> {
        Self::load_selected(conn, agent, policy, documents_policy, now, scope, None).await
    }

    /// Reads indexed source candidates and file directories, preserving ordinary reach and canonical names.
    pub async fn load_search(
        conn: &mut Conn,
        agent: Agent,
        policy: AgentReach,
        documents_policy: reach::DocumentPolicy,
        now: Time,
        scope: Scope,
        selected: &orch_db::queries::search::CandidateSet,
    ) -> Result<Self, EngineError> {
        Self::load_selected(
            conn,
            agent,
            policy,
            documents_policy,
            now,
            scope,
            Some(selected),
        )
        .await
    }

    async fn load_selected(
        conn: &mut Conn,
        agent: Agent,
        policy: AgentReach,
        documents_policy: reach::DocumentPolicy,
        now: Time,
        scope: Scope,
        search_parts: Option<&orch_db::queries::search::CandidateSet>,
    ) -> Result<Self, EngineError> {
        let selected_documents = search_parts.map(|parts| parts.documents.as_slice());
        let headers_only = search_parts.is_some_and(|parts| parts.headers_only);
        let grants = if headers_only {
            Vec::new()
        } else {
            reach::grants(conn, &agent, now).await?
        };
        let loaded = if let Some(parts) = search_parts.filter(|_| headers_only) {
            loading::headers(conn, &agent, parts, scope).await?
        } else {
            loading::load(conn, &agent, policy, scope, &grants).await?
        };
        let places = loaded.places;
        let capped = loaded.capped;
        let content_nodes: HashSet<_> = loaded.content.iter().copied().collect();
        let ids: Vec<Id> = places.iter().map(|place| place.id).collect();
        // Their documents and chats, and the direct and group chats that were shared.
        let mut selected: Vec<_> = if let Some(documents) = selected_documents {
            documents
                .iter()
                .copied()
                .map(organization::Item::Document)
                .collect()
        } else {
            loaded
                .documents
                .iter()
                .copied()
                .map(organization::Item::Document)
                .chain(
                    loaded
                        .containers
                        .iter()
                        .copied()
                        .map(organization::Item::Container),
                )
                .collect()
        };
        if selected_documents.is_some() && !headers_only {
            // Blob contents keep the existing bounded file scan even when their parent text does not match.
            let files: Vec<Id>=sqlx::query_scalar("SELECT DISTINCT f.document_id FROM document_files f JOIN documents d ON d.id=f.document_id
              WHERE d.deleted_at IS NULL AND (d.node_id=ANY($1) OR d.id=ANY($2))")
              .bind(&loaded.content).bind(&loaded.documents).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
            selected.extend(files.into_iter().map(organization::Item::Document));
        }
        let organized = organization::load_entries_in(
            conn,
            organization::Actor::Agent {
                thread: agent.thread,
            },
            if selected_documents.is_some() {
                &[]
            } else {
                &loaded.content
            },
            &selected,
            now,
        )
        .await
        .map_err(folders::read_error)?;
        let folder_ancestors: HashMap<_, _> = organized
            .ancestors
            .into_iter()
            .map(|ancestor| (ancestor.id, ancestor))
            .collect();
        let organized: HashMap<_, _> = organized
            .entries
            .into_iter()
            .map(|entry| (entry.entry.item, entry))
            .collect();
        let document_names = folders::presentation_names(&organized, &folder_ancestors);
        let mut doc_ids: Vec<_> = organized
            .keys()
            .filter_map(|item| match item {
                organization::Item::Document(id) => Some(*id),
                _ => None,
            })
            .collect();
        doc_ids.sort_unstable();
        let all_doc_rows = document_rows::get_many(conn, &doc_ids).await?;
        let doc_rows: Vec<_> = all_doc_rows
            .iter()
            .filter(|row| row.deleted_at.is_none())
            .cloned()
            .collect();
        let doc_names: Vec<_> = doc_rows
            .iter()
            .filter_map(|row| {
                document_names
                    .get(&organization::Item::Document(row.id))
                    .map(|name| (row.id, row.node_id, name.clone()))
            })
            .collect();
        let documents: Vec<Document> = doc_rows.iter().map(Document::of_row).collect();
        // The ones that were deleted and not yet purged: listed to whoever may bring them back, which is an agent
        // that was shared documents to manage and no other, so that nothing is read for one that was not.
        let deleted_rows: Vec<_> = all_doc_rows
            .iter()
            .filter(|row| row.deleted_at.is_some())
            .cloned()
            .collect();
        let deleted_documents: Vec<Document> = deleted_rows.iter().map(Document::of_row).collect();
        let node_chats = if search_parts.is_none() {
            rows::chats_of_nodes(conn, &loaded.content).await?
        } else {
            Vec::new()
        };
        let node_chat_locations: Vec<(Id, Id)> = if search_parts.is_some() {
            // File discovery needs every reachable chat identity, not the size of each chat history.
            sqlx::query_as("SELECT id,node_id FROM chats WHERE node_id=ANY($1) AND ($2::uuid[] IS NULL OR id=ANY($2))")
                .bind(&loaded.content).bind(search_parts.filter(|_| headers_only).map(|parts| &parts.chats))
                .fetch_all(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
        } else {
            node_chats
                .iter()
                .map(|chat| (chat.chat_id, chat.node_id))
                .collect()
        };
        let mut people_ids: Vec<Id> = grants
            .iter()
            .filter(|grant| grant.page.kind == orch_access_contract::pages::StoredKind::Chat)
            .map(|grant| grant.page.id)
            .collect();
        let attachment_chats: Vec<_> = node_chat_locations
            .iter()
            .map(|(chat, _)| *chat)
            .chain(people_ids.iter().copied())
            .collect();
        if headers_only {
            people_ids = search_parts.map_or_else(Vec::new, |parts| parts.chats.clone());
        }
        let attachment_dirs = if headers_only {
            HashSet::new()
        } else {
            attachments::directories(conn, &doc_ids, &attachment_chats).await?
        };
        let node_chats = if let Some(parts) = search_parts {
            let nodes: Vec<_> = node_chat_locations
                .iter()
                .filter_map(|(chat, node)| {
                    (parts.chats.contains(chat)
                        || attachment_dirs.contains(&Where::ChatFiles(*chat)))
                    .then_some(*node)
                })
                .collect();
            people_ids.retain(|chat| {
                parts.chats.contains(chat) || attachment_dirs.contains(&Where::ChatFiles(*chat))
            });
            if nodes.is_empty() {
                Vec::new()
            } else {
                if headers_only {
                    sqlx::query_as("SELECT id chat_id,node_id,last_message_at,0::bigint messages FROM chats WHERE node_id=ANY($1)")
                        .bind(&nodes).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
                } else {
                    rows::chats_of_nodes(conn, &nodes).await?
                }
            }
        } else {
            node_chats
        };
        let people = if people_ids.is_empty() {
            Vec::new()
        } else {
            if headers_only {
                sqlx::query_as("SELECT id chat_id,org_id,kind,name,created_at,last_message_at,0::bigint messages FROM chats WHERE id=ANY($1) AND kind<>'node' ORDER BY created_at,id")
                    .bind(&people_ids).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
            } else {
                rows::people_chats(conn, &people_ids).await?
            }
        };
        let threads: Vec<Id> = places
            .iter()
            .filter(|place| {
                place.kind == "thread"
                    && search_parts.is_none_or(|parts| parts.nodes.contains(&place.id))
            })
            .map(|place| place.id)
            .collect();
        let totals = if headers_only || threads.is_empty() {
            HashMap::new()
        } else {
            rows::turn_totals(conn, &threads).await?
        };
        let blocks = if headers_only {
            HashMap::new()
        } else {
            rows::top_level_blocks(conn, &doc_ids).await?
        };
        let agent_ids: Vec<Id> = places.iter().filter_map(|place| place.agent_id).collect();
        let environment_ids: Vec<Id> = places
            .iter()
            .filter_map(|place| place.environment_id)
            .collect();
        let agent_names = if headers_only {
            HashMap::new()
        } else {
            rows::agent_names(conn, &agent_ids).await?
        };
        let environment_names = if headers_only {
            HashMap::new()
        } else {
            rows::environment_names(conn, &environment_ids).await?
        };
        let own_environment = places
            .iter()
            .find(|place| place.id == agent.thread)
            .and_then(|place| place.environment_id);
        let reach = Reach::load(
            conn,
            agent.clone(),
            policy,
            now,
            &Things {
                nodes: ids.clone(),
                documents: documents
                    .iter()
                    .chain(deleted_documents.iter())
                    .cloned()
                    .collect(),
                people_chats: people_ids.clone(),
            },
        )
        .await?
        .with_document_policy(documents_policy);
        let member_names = if headers_only {
            HashMap::new()
        } else {
            rows::member_names(
                conn,
                &people.iter().map(|chat| chat.chat_id).collect::<Vec<_>>(),
            )
            .await?
        };

        // The directories, in the order the nodes were made.
        let mut dirs: Vec<Dir> = Vec::with_capacity(places.len());
        let mut by_id: HashMap<Id, usize> = HashMap::new();
        for place in places {
            by_id.insert(place.id, dirs.len());
            dirs.push(Dir {
                place,
                name: String::new(),
                parent: None,
                children: Vec::new(),
                on_way: false,
                conversation: None,
                chat: None,
                docs: Vec::new(),
                deleted: Vec::new(),
                docs_standing: Standing::default(),
                present: false,
                standing: Standing::default(),
            });
        }
        let order: Vec<usize> = (0..dirs.len()).collect();
        let mut by_parent: HashMap<Option<Id>, Vec<usize>> = HashMap::new();
        for at in &order {
            let parent = dirs[*at].place.parent_id;
            let parent_at = parent.and_then(|id| by_id.get(&id).copied());
            dirs[*at].parent = parent_at;
            by_parent.entry(parent).or_default().push(*at);
        }
        for (parent, mut children) in by_parent {
            children.sort_by_key(|at| (dirs[*at].place.created_at, dirs[*at].place.id));
            let named: Vec<(Id, String)> = children
                .iter()
                .map(|at| {
                    let place = &dirs[*at].place;
                    (
                        place.id,
                        place.slug.clone().unwrap_or_else(|| place.name.clone()),
                    )
                })
                .collect();
            // Siblings contribute only their stable names, never content or additional reach.
            let siblings = match parent {
                Some(parent) if !headers_only && !content_nodes.contains(&parent) => {
                    rows::children_of(conn, parent).await?
                }
                _ => Vec::new(),
            };
            let named = if siblings.is_empty() {
                named
            } else {
                siblings
                    .iter()
                    .map(|sibling| {
                        (
                            sibling.id,
                            sibling.slug.clone().unwrap_or_else(|| sibling.name.clone()),
                        )
                    })
                    .collect()
            };
            let names: HashMap<_, _> = named
                .iter()
                .map(|(id, _)| *id)
                .zip(assign_names(&named, "", true))
                .collect();
            for at in &children {
                dirs[*at].name = names
                    .get(&dirs[*at].place.id)
                    .cloned()
                    .unwrap_or_else(|| tree_name(&dirs[*at].place.name));
                if headers_only && dirs[*at].place.parent_id.is_some() {
                    dirs[*at].name = header_name(&dirs[*at].place);
                }
            }
            if let Some(parent_at) = parent.and_then(|id| by_id.get(&id).copied()) {
                dirs[parent_at].children = children;
            }
        }
        let mut roots: Vec<usize> = dirs
            .iter()
            .enumerate()
            .filter(|(_, dir)| dir.place.parent_id.is_none())
            .map(|(at, _)| at)
            .collect();
        roots.sort_by_key(|at| (dirs[*at].place.created_at, dirs[*at].place.id));
        let root_names: HashMap<usize, String> = roots
            .iter()
            .map(|at| {
                (
                    *at,
                    dirs[*at]
                        .place
                        .slug
                        .clone()
                        .unwrap_or_else(|| tree_name(&dirs[*at].place.name)),
                )
            })
            .collect();
        for at in &roots {
            if let Some(name) = root_names.get(at) {
                dirs[*at].name.clone_from(name);
            }
        }
        // What the agent may do with each.
        let chat_of: HashMap<Id, &rows::NodeChatRow> =
            node_chats.iter().map(|chat| (chat.node_id, chat)).collect();
        let docs_by_id: HashMap<_, _> = doc_rows.iter().map(|row| (row.id, row)).collect();
        let mut names_by_node: HashMap<Id, Vec<(Id, String)>> = HashMap::new();
        for (id, node, title) in doc_names {
            names_by_node.entry(node).or_default().push((id, title));
        }
        let mut deleted_by_node: HashMap<Id, Vec<usize>> = HashMap::new();
        for (at, row) in deleted_rows.iter().enumerate() {
            deleted_by_node.entry(row.node_id).or_default().push(at);
        }
        let own_topic_threads: HashSet<Id> = dirs
            .iter()
            .filter(|dir| {
                dir.place.kind == "thread"
                    && dir.place.side_of.is_none()
                    && dir.place.path.contains(&agent.topic)
            })
            .map(|dir| dir.place.id)
            .collect();
        let way: HashSet<Id> = agent.path.iter().copied().collect();
        for dir in &mut dirs {
            let id = dir.place.id;
            dir.on_way = way.contains(&id) || own_topic_threads.contains(&id);
            if dir.place.kind == "thread" {
                let standing = reach.conversation(id, dir.place.side_of.is_some());
                let total = totals.get(&id).copied().unwrap_or((0, None));
                dir.standing = standing;
                dir.conversation = search_parts
                    .is_none_or(|parts| parts.nodes.contains(&id))
                    .then_some(Conversation {
                        standing,
                        turns: total.0,
                        updated: total.1,
                    });
            }
            if let Some(chat) = chat_of.get(&id) {
                let standing = reach.node_chat(id);
                dir.chat = Some(ChatNode {
                    id: chat.chat_id,
                    standing,
                    messages: chat.messages,
                    updated: chat.last_message_at,
                });
            }
            dir.docs_standing = reach.documents_of(id);
            // Every name a document that is there has, whether or not the agent may read it.
            if let Some(mine) = names_by_node.get(&id) {
                let names: Vec<_> = mine.iter().map(|(_, basename)| basename.clone()).collect();
                for ((doc_id, _), name) in mine.iter().zip(names) {
                    let Some(row) = docs_by_id.get(doc_id).copied() else {
                        continue;
                    };
                    let document = Document::of_row(row);
                    let standing = folders::standing(
                        organized
                            .get(&organization::Item::Document(row.id))
                            .map(|entry| entry.standing)
                            .unwrap_or_default(),
                        reach.document(&document),
                    );
                    if standing.any() {
                        dir.docs.push(DocEntry {
                            row: row.clone(),
                            document,
                            name,
                            standing,
                            top_blocks: blocks.get(doc_id).copied().unwrap_or(0),
                        });
                    }
                }
            }
            // Those that were deleted come after, under names that take none of the others': they are listed to
            // an agent that may bring them back, and to nobody else; nothing is under a place that is archived.
            if let Some(list) = deleted_by_node.get(&id)
                && !reach.archived(id)
            {
                let mut mine: Vec<&document_rows::DocumentRow> =
                    list.iter().map(|at| &deleted_rows[*at]).collect();
                mine.sort_by_key(|row| (row.created_at, row.id));
                let names: Vec<_> = mine
                    .iter()
                    .map(|row| {
                        document_names
                            .get(&organization::Item::Document(row.id))
                            .cloned()
                            .unwrap_or_else(|| {
                                format!("{}{DOCUMENT_SUFFIX}", tree_name(&row.title))
                            })
                    })
                    .collect();
                for (row, name) in mine.into_iter().zip(names) {
                    let document = Document::of_row(row);
                    let standing = folders::standing(
                        organized
                            .get(&organization::Item::Document(row.id))
                            .map(|entry| entry.standing)
                            .unwrap_or_default(),
                        reach.document(&document),
                    );
                    if standing.manages() {
                        dir.deleted.push(DocEntry {
                            row: row.clone(),
                            document,
                            name,
                            standing,
                            top_blocks: 0,
                        });
                    }
                }
            }
        }
        // The direct and group chats that were shared, by root org.
        let mut messages: HashMap<Id, Vec<PeopleEntry>> = HashMap::new();
        for chat in &people {
            let standing = reach.people_chat(chat.chat_id);
            if !standing.any() {
                continue;
            }
            let label = chat
                .name
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| {
                    member_names
                        .get(&chat.chat_id)
                        .map(|names| names.join(", "))
                        .filter(|names| !names.is_empty())
                        .unwrap_or_else(|| "chat".to_owned())
                });
            let label: String = label.chars().take(CHAT_NAME_CHARS).collect();
            messages.entry(chat.org_id).or_default().push(PeopleEntry {
                chat: chat.chat_id,
                label: label.clone(),
                name: label,
                standing,
                messages: chat.messages,
                updated: chat.last_message_at,
            });
        }
        for chats in messages.values_mut() {
            let named: Vec<(Id, String)> = chats
                .iter()
                .map(|chat| (chat.chat, chat.name.clone()))
                .collect();
            let names = assign_names(&named, "", false);
            for (chat, name) in chats.iter_mut().zip(names) {
                chat.name = if headers_only {
                    format!("{}~{}", tree_name(&chat.label), chat.chat.simple())
                } else {
                    name
                };
            }
        }
        let (base, scope_at) = match scope {
            Scope::Everything => (String::from("/"), None),
            Scope::Under(node) => {
                let along = if headers_only {
                    header_names_along(conn, node).await?
                } else {
                    names_along(conn, node).await?
                };
                let at = by_id.get(&node).copied();
                if let (Some(at), Some((_, name))) = (at, along.last()) {
                    dirs[at].name.clone_from(name);
                    dirs[at].parent = None;
                }
                let mut base = String::from("/");
                for (_, name) in along.iter().take(along.len().saturating_sub(1)) {
                    base.push_str(name);
                    base.push('/');
                }
                (base, at)
            }
        };
        let mut tree = Self {
            reach,
            agent,
            now,
            dirs,
            by_id,
            roots,
            root_names,
            messages,
            attachment_dirs,
            organized,
            folder_ancestors,
            document_names,
            agent_names,
            environment_names,
            own_environment,
            capped,
            scope,
            base,
            scope_at,
        };
        tree.work_out_presence();
        Ok(tree)
    }

    /// A directory is there when the agent may read something in it or under it, or it is on the agent's own way.
    fn work_out_presence(&mut self) {
        // Children are made after their parents: walk from the end.
        let order: Vec<usize> = (0..self.dirs.len()).rev().collect();
        // Deepest first, so that a child's answer is known to its parent.
        let mut by_depth: Vec<usize> = order;
        by_depth.sort_by_key(|at| std::cmp::Reverse(self.dirs[*at].place.path.len()));
        for at in by_depth {
            let own = {
                let dir = &self.dirs[at];
                dir.on_way
                    // A search window can omit history counts while the thread's name remains readable.
                    || (dir.place.kind == "thread" && dir.standing.reads())
                    || dir.chat.as_ref().is_some_and(|chat| chat.standing.any())
                    || !dir.docs.is_empty()
                    || !dir.deleted.is_empty()
                    || !self
                        .folder_children(organization::Directory::NodeDocs(dir.place.id))
                        .is_empty()
                    || dir.docs_standing.reads()
                    || dir.docs_standing.writes()
                    || (dir.place.parent_id.is_none() && self.messages.contains_key(&dir.place.id))
            };
            let below = self.dirs[at]
                .children
                .iter()
                .any(|child| self.dirs[*child].present);
            self.dirs[at].present = own || below;
        }
    }

    /// The agent the tree is read for.
    #[must_use]
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// The reach the tree was made from.
    #[must_use]
    pub fn reach(&self) -> &Reach {
        &self.reach
    }

    /// The moment it was read at.
    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    /// Whether more nodes lay in scope than were looked through.
    #[must_use]
    pub fn capped(&self) -> bool {
        self.capped
    }

    fn dir_of(&self, id: Id) -> Option<&Dir> {
        self.by_id.get(&id).map(|at| &self.dirs[*at])
    }

    /// The documents of a node that the agent may read.
    #[must_use]
    pub fn documents_of(&self, node: Id) -> &[DocEntry] {
        self.dir_of(node).map_or(&[], |dir| dir.docs.as_slice())
    }

    /// The documents beside a node that were deleted and that the agent may bring back.
    #[must_use]
    pub fn deleted_of(&self, node: Id) -> &[DocEntry] {
        self.dir_of(node).map_or(&[], |dir| dir.deleted.as_slice())
    }

    /// The path of the `docs` directory beside a node, ending in `/`.
    #[must_use]
    pub fn docs_path_of(&self, node: Id) -> Option<String> {
        self.path_of_node(node)
            .map(|path| path_of(&path, DOCS, true))
    }

    /// The path of a node, from the top, with its name at each step; ends in `/`.
    fn path_to(&self, at: usize) -> String {
        let mut names: Vec<&str> = Vec::new();
        let mut current = Some(at);
        let mut top = at;
        while let Some(index) = current {
            names.push(self.dirs[index].name.as_str());
            top = index;
            current = self.dirs[index].parent;
        }
        names.reverse();
        let mut path = if Some(top) == self.scope_at {
            self.base.clone()
        } else {
            String::from("/")
        };
        for name in names {
            path.push_str(name);
            path.push('/');
        }
        path
    }

    /// The path of a node: the names of the nodes on its way, then `/`.
    #[must_use]
    pub fn path_of_node(&self, node: Id) -> Option<String> {
        self.by_id.get(&node).map(|at| self.path_to(*at))
    }

    // ------------------------------------------------------------ making the entries

    fn place_entry(&self, at: usize, prefix: &str) -> Entry {
        let dir = &self.dirs[at];
        let place = &dir.place;
        let kind = kind_of(&place.kind);
        let mut entry = Entry::bare(
            path_of(prefix, &dir.name, true),
            dir.name.clone(),
            kind,
            place.id,
            Where::Place(place.id),
        );
        entry.description =
            Some(opening(&place.description, DESCRIPTION_CHARS)).filter(|text| !text.is_empty());
        entry.updated_at = Some(Time::new(place.updated_at));
        entry.status.clone_from(&place.status);
        entry.archived = self.reach.archived(place.id);
        entry.children = Some(self.count_entries(at));
        if place.kind == "thread" {
            entry.updated_at = dir
                .conversation
                .as_ref()
                .and_then(|conversation| conversation.updated)
                .map(Time::new)
                .or(entry.updated_at);
            entry.state.clone_from(&place.state);
            entry.agent = place
                .agent_id
                .and_then(|id| self.agent_names.get(&id).cloned());
            entry.environment = place
                .environment_id
                .and_then(|id| self.environment_names.get(&id).cloned());
            if place.environment_id.is_some() && place.environment_id == self.own_environment {
                entry.directory.clone_from(&place.workspace_dir);
            }
            entry.is_self = place.id == self.agent.thread;
            let standing = dir
                .conversation
                .as_ref()
                .map(|conversation| conversation.standing)
                .unwrap_or_default();
            let sendable = !place.archived && standing.write.is_some() && !entry.is_self;
            entry.readable = standing.reads();
            entry.writable = sendable;
            entry.via = standing
                .read
                .or(standing.write)
                .map_or(SharedEntryVia::Topic, via_of);
            entry.shared_by = standing.shared_by;
        } else {
            // A place is read through what lies in it.
            entry.readable = true;
            entry.writable = false;
            entry.via = if dir.on_way {
                SharedEntryVia::Topic
            } else {
                dir.chat
                    .as_ref()
                    .and_then(|chat| chat.standing.read)
                    .or(dir.docs.first().and_then(|doc| doc.standing.read))
                    .map_or(SharedEntryVia::Share, via_of)
            };
        }
        entry
    }

    /// The things in a node's directory that are files and folders of its own.
    fn own_entries(&self, at: usize, prefix: &str) -> Vec<Entry> {
        let dir = &self.dirs[at];
        let mut out: Vec<Entry> = Vec::new();
        if let Some(conversation) = &dir.conversation
            && conversation.standing.reads()
        {
            let mut entry = Entry::bare(
                path_of(prefix, CONVERSATION, false),
                CONVERSATION.to_owned(),
                SharedEntryKind::Conversation,
                dir.place.id,
                Where::Conversation(dir.place.id),
            );
            entry.unit = Some(SharedEntryUnit::Turn);
            entry.parts = Some(conversation.turns);
            entry.updated_at = conversation.updated.map(Time::new);
            // What is written to a conversation is a message to its thread, so it can be written where the thread
            // can be sent to.
            entry.writable = !dir.place.archived
                && conversation.standing.write.is_some()
                && dir.place.id != self.agent.thread;
            entry.via = conversation
                .standing
                .read
                .map_or(SharedEntryVia::Share, via_of);
            entry.shared_by = conversation.standing.shared_by;
            out.push(entry);
        }
        if let Some(chat) = &dir.chat
            && chat.standing.any()
        {
            out.push(self.chat_entry(
                prefix,
                CHAT,
                chat.id,
                &chat.standing,
                chat.messages,
                chat.updated,
                Where::NodeChat(dir.place.id),
                dir.place.archived,
            ));
            if chat.standing.reads() && self.attachment_dirs.contains(&Where::ChatFiles(chat.id)) {
                out.push(self.attachment_directory(
                    prefix,
                    "chat.files",
                    Where::ChatFiles(chat.id),
                    &chat.standing,
                ));
            }
        }
        if !dir.docs.is_empty()
            || !dir.deleted.is_empty()
            || dir.docs_standing.reads()
            || dir.docs_standing.writes()
            || !self
                .folder_children(organization::Directory::NodeDocs(dir.place.id))
                .is_empty()
        {
            let mut entry = Entry::bare(
                path_of(prefix, DOCS, true),
                DOCS.to_owned(),
                SharedEntryKind::Folder,
                ids_of("docs", dir.place.id),
                Where::Docs(dir.place.id),
            );
            entry.children = Some(
                i64::try_from(
                    self.document_entries(at, "", organization::Directory::NodeDocs(dir.place.id))
                        .len(),
                )
                .unwrap_or(i64::MAX),
            );
            entry.readable = dir.docs_standing.reads()
                || !dir.docs.is_empty()
                || !self
                    .folder_children(organization::Directory::NodeDocs(dir.place.id))
                    .is_empty();
            entry.writable = dir.docs_standing.writes() && !self.reach.archived(dir.place.id);
            // This is a virtual docs area, not a mutable document container.
            entry.manageable = false;
            entry.via = dir
                .docs_standing
                .read
                .or(dir.docs_standing.write)
                .map_or(SharedEntryVia::Share, via_of);
            entry.shared_by = dir.docs_standing.shared_by;
            out.push(entry);
        }
        if dir.place.parent_id.is_none()
            && let Some(chats) = self.messages.get(&dir.place.id)
            && !chats.is_empty()
        {
            let mut entry = Entry::bare(
                path_of(prefix, MESSAGES, true),
                MESSAGES.to_owned(),
                SharedEntryKind::Folder,
                ids_of("messages", dir.place.id),
                Where::Messages(dir.place.id),
            );
            entry.children = Some(i64::try_from(chats.len()).unwrap_or(i64::MAX));
            out.push(entry);
        }
        out
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "a chat is told by what it is counted in and what may be done with it"
    )]
    fn chat_entry(
        &self,
        prefix: &str,
        name: &str,
        chat: Id,
        standing: &Standing,
        messages: i64,
        updated: Option<chrono::DateTime<chrono::Utc>>,
        at: Where,
        archived: bool,
    ) -> Entry {
        let mut entry = Entry::bare(
            path_of(prefix, name, false),
            name.to_owned(),
            SharedEntryKind::Chat,
            chat,
            at,
        );
        entry.unit = Some(SharedEntryUnit::Message);
        entry.parts = Some(messages);
        entry.updated_at = updated.map(Time::new);
        entry.readable = standing.reads();
        entry.writable = standing.writes() && !archived;
        entry.via = standing
            .read
            .or(standing.write)
            .map_or(SharedEntryVia::Share, via_of);
        entry.shared_by = standing.shared_by;
        entry
    }

    fn document_entries(
        &self,
        node: usize,
        prefix: &str,
        parent: organization::Directory,
    ) -> Vec<Entry> {
        let dir = &self.dirs[node];
        let archived = self.reach.archived(dir.place.id);
        let mut out = Vec::new();
        for doc in dir
            .docs
            .iter()
            .filter(|doc| self.document_parent(doc.row.id) == Some(parent))
        {
            let mut entry = Entry::bare(
                path_of(prefix, &doc.name, false),
                doc.name.clone(),
                SharedEntryKind::Document,
                doc.row.id,
                Where::Document(doc.row.id),
            );
            entry.unit = Some(SharedEntryUnit::Block);
            entry.parts = Some(doc.top_blocks);
            // The opening of its description; one that has none is given its first paragraph's by the caller,
            // which reads the blocks of the documents it lists.
            entry.description = Some(opening(&doc.row.description, DESCRIPTION_CHARS))
                .filter(|text| !text.is_empty());
            entry.size = Some(doc.row.bytes);
            entry.updated_at = Some(Time::new(doc.row.updated_at));
            entry.readable = doc.standing.reads();
            entry.writable = doc.standing.writes() && !archived;
            entry.manageable = doc.standing.manages() && !archived;
            entry.via = doc
                .standing
                .read
                .or(doc.standing.write)
                .map_or(SharedEntryVia::Share, via_of);
            entry.shared_by = doc.standing.shared_by;
            out.push(entry);
            if doc.standing.reads()
                && self
                    .attachment_dirs
                    .contains(&Where::DocumentFiles(doc.row.id))
            {
                let stem = doc.name.strip_suffix(DOCUMENT_SUFFIX).unwrap_or(&doc.name);
                out.push(self.attachment_directory(
                    prefix,
                    &format!("{stem}{FILES_SUFFIX}"),
                    Where::DocumentFiles(doc.row.id),
                    &doc.standing,
                ));
            }
            if doc.row.version > 0 {
                let stem = doc.name.strip_suffix(DOCUMENT_SUFFIX).unwrap_or(&doc.name);
                let mut history = Entry::bare(
                    path_of(prefix, &format!("{stem}{HISTORY_SUFFIX}"), true),
                    format!("{stem}{HISTORY_SUFFIX}"),
                    SharedEntryKind::Folder,
                    ids_of("history", doc.row.id),
                    Where::History(doc.row.id),
                );
                history.children = Some(i64::from(doc.row.version));
                history.via = entry_via(&doc.standing);
                out.push(history);
            }
        }
        // The ones that were deleted can be neither read nor written: they can be brought back.
        for doc in dir
            .deleted
            .iter()
            .filter(|doc| self.document_parent(doc.row.id) == Some(parent))
        {
            let mut entry = Entry::bare(
                path_of(prefix, &doc.name, false),
                doc.name.clone(),
                SharedEntryKind::Document,
                doc.row.id,
                Where::Document(doc.row.id),
            );
            entry.size = Some(doc.row.bytes);
            entry.updated_at = Some(Time::new(doc.row.updated_at));
            entry.readable = false;
            entry.manageable = true;
            entry.deleted = true;
            entry.via = entry_via(&doc.standing);
            entry.shared_by = doc.standing.shared_by;
            out.push(entry);
        }
        out.extend(
            self.folder_children(parent)
                .into_iter()
                .filter_map(|folder| self.folder_entry(folder, prefix)),
        );
        out
    }

    fn count_entries(&self, at: usize) -> i64 {
        let dir = &self.dirs[at];
        let places = dir
            .children
            .iter()
            .filter(|child| self.dirs[**child].present)
            .count();
        let own = self.own_entries(at, "").len();
        i64::try_from(places + own).unwrap_or(i64::MAX)
    }

    /// The entries directly in a directory: the places that are there, then what the directory holds itself.
    fn direct_entries(&self, where_: Where, prefix: &str) -> Vec<Entry> {
        match where_ {
            Where::Root => self
                .roots
                .iter()
                .filter(|at| self.dirs[**at].present)
                .map(|at| {
                    let mut entry = self.place_entry(*at, prefix);
                    if let Some(name) = self.root_names.get(at) {
                        entry.name.clone_from(name);
                        entry.path = path_of(prefix, name, true);
                    }
                    entry
                })
                .collect(),
            Where::Place(id) => {
                let Some(at) = self.by_id.get(&id).copied() else {
                    return Vec::new();
                };
                let mut out: Vec<Entry> = self.dirs[at]
                    .children
                    .iter()
                    .filter(|child| self.dirs[**child].present)
                    .map(|child| self.place_entry(*child, prefix))
                    .collect();
                out.extend(self.own_entries(at, prefix));
                out
            }
            Where::Docs(node) => {
                let Some(at) = self.by_id.get(&node).copied() else {
                    return Vec::new();
                };
                self.document_entries(at, prefix, organization::Directory::NodeDocs(node))
            }
            Where::DocumentFolder(id) => {
                let Some(node) = self.directory_node(organization::Directory::Container(id)) else {
                    return Vec::new();
                };
                let Some(at) = self.by_id.get(&node).copied() else {
                    return Vec::new();
                };
                self.document_entries(at, prefix, organization::Directory::Container(id))
            }
            Where::Messages(root) => self
                .messages
                .get(&root)
                .map(|chats| {
                    chats
                        .iter()
                        .map(|chat| {
                            let mut entry = Entry::bare(
                                path_of(prefix, &chat.name, true),
                                chat.name.clone(),
                                SharedEntryKind::Folder,
                                chat.chat,
                                Where::PeopleChatDir(chat.chat),
                            );
                            entry.children = Some(
                                if self.attachment_dirs.contains(&Where::ChatFiles(chat.chat)) {
                                    2
                                } else {
                                    1
                                },
                            );
                            entry.via = entry_via(&chat.standing);
                            entry.shared_by = chat.standing.shared_by;
                            entry
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Where::PeopleChatDir(chat) => self
                .messages
                .values()
                .flatten()
                .find(|entry| entry.chat == chat)
                .map(|entry| {
                    let mut entries = vec![self.chat_entry(
                        prefix,
                        CHAT,
                        entry.chat,
                        &entry.standing,
                        entry.messages,
                        entry.updated,
                        Where::PeopleChat(entry.chat),
                        false,
                    )];
                    if entry.standing.reads()
                        && self.attachment_dirs.contains(&Where::ChatFiles(entry.chat))
                    {
                        entries.push(self.attachment_directory(
                            prefix,
                            "chat.files",
                            Where::ChatFiles(entry.chat),
                            &entry.standing,
                        ));
                    }
                    entries
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// The earlier versions of a document, newest first: each named by its number and the moment it was cut.
    async fn version_entries(
        &self,
        conn: &mut Conn,
        document: Id,
        prefix: &str,
    ) -> Result<Vec<Entry>, EngineError> {
        if !self.directory_at(Where::History(document)) {
            return Ok(Vec::new());
        }
        let access = self
            .dirs
            .iter()
            .flat_map(|dir| dir.docs.iter())
            .find(|doc| doc.row.id == document)
            .map(|doc| {
                (
                    doc.standing,
                    doc.standing.manages() && !self.reach.archived(doc.row.node_id),
                )
            });
        let mut out = Vec::new();
        for version in rows::versions_of(conn, document, ENTRIES_MAX as i64).await? {
            let name = format!(
                "{:04} {}{DOCUMENT_SUFFIX}",
                version.version,
                version.to_at.format("%Y-%m-%dT%H-%MZ")
            );
            let mut entry = Entry::bare(
                path_of(prefix, &name, false),
                name,
                SharedEntryKind::Version,
                version_id(document, version.version),
                Where::Version(document, version.version),
            );
            entry.unit = Some(SharedEntryUnit::Block);
            entry.parts = Some(version.blocks);
            entry.size = Some(version.bytes);
            entry.updated_at = Some(Time::new(version.to_at));
            entry.via = access
                .as_ref()
                .map_or(SharedEntryVia::Share, |(standing, _)| entry_via(standing));
            entry.shared_by = access.and_then(|(standing, _)| standing.shared_by);
            entry.manageable = access.is_some_and(|(_, manageable)| manageable);
            out.push(entry);
        }
        Ok(out)
    }

    /// Sorted as a listing gives them: directories first, then files, each by name.
    fn sorted(mut entries: Vec<Entry>) -> Vec<Entry> {
        entries.sort_by(|a, b| {
            let rank = |entry: &Entry| match entry.kind {
                SharedEntryKind::Org
                | SharedEntryKind::Project
                | SharedEntryKind::Topic
                | SharedEntryKind::Thread => 0,
                SharedEntryKind::Folder => 1,
                _ => 2,
            };
            let by_name = a
                .name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name));
            rank(a).cmp(&rank(b)).then(
                if a.kind == SharedEntryKind::Version && b.kind == SharedEntryKind::Version {
                    by_name.reverse()
                } else {
                    by_name
                },
            )
        });
        entries
    }

    fn sorted_for(mut entries: Vec<Entry>, sort: Sort) -> Vec<Entry> {
        if sort == Sort::Name {
            return Self::sorted(entries);
        }
        let active = |entry: &Entry| {
            !entry.archived
                && (matches!(
                    entry.state.as_deref(),
                    Some("starting" | "running" | "waiting_approval" | "blocked" | "stopping")
                ) || entry.status.as_deref() == Some("open"))
        };
        entries.sort_by(|one, other| {
            one.archived
                .cmp(&other.archived)
                .then_with(|| active(other).cmp(&active(one)))
                .then_with(|| other.updated_at.cmp(&one.updated_at))
                .then_with(|| one.name.to_lowercase().cmp(&other.name.to_lowercase()))
                .then_with(|| one.path.cmp(&other.path))
        });
        entries
    }

    fn archived_at(&self, at: Where) -> bool {
        match at {
            Where::Place(id)
            | Where::Conversation(id)
            | Where::NodeChat(id)
            | Where::Docs(id)
            | Where::Messages(id) => self.reach.archived(id),
            Where::DocumentFolder(id) => self
                .directory_node(organization::Directory::Container(id))
                .is_some_and(|node| self.reach.archived(node)),
            Where::Document(id)
            | Where::DocumentFiles(id)
            | Where::DocumentFile { document: id, .. }
            | Where::History(id)
            | Where::Version(id, _) => self
                .document_either(id)
                .is_some_and(|doc| self.reach.archived(doc.row.node_id)),
            Where::ChatFiles(chat) | Where::ChatFile { chat, .. } => {
                self.chat_origin(chat).is_some_and(|node| {
                    node.map_or_else(
                        || {
                            self.messages.iter().any(|(root, entries)| {
                                self.reach.archived(*root)
                                    && entries.iter().any(|entry| entry.chat == chat)
                            })
                        },
                        |node| self.reach.archived(node),
                    )
                })
            }
            Where::PeopleChat(id) | Where::PeopleChatDir(id) => {
                self.messages.iter().any(|(root, chats)| {
                    self.reach.archived(*root) && chats.iter().any(|chat| chat.chat == id)
                })
            }
            Where::Root => false,
        }
    }

    /// The directory a path names, if it is one that is there.
    #[must_use]
    pub fn directory_at(&self, where_: Where) -> bool {
        match where_ {
            Where::Root => true,
            Where::Place(id) => self.dir_of(id).is_some_and(|dir| dir.present),
            Where::Docs(id) => self.dir_of(id).is_some_and(|dir| {
                !dir.docs.is_empty()
                    || !dir.deleted.is_empty()
                    || dir.docs_standing.reads()
                    || dir.docs_standing.writes()
                    || !self
                        .folder_children(organization::Directory::NodeDocs(id))
                        .is_empty()
            }),
            Where::DocumentFolder(id) => self.folder_exists(id),
            Where::Messages(id) => self
                .messages
                .get(&id)
                .is_some_and(|chats| !chats.is_empty()),
            Where::PeopleChatDir(id) => self
                .messages
                .values()
                .flatten()
                .any(|entry| entry.chat == id),
            Where::History(id) => self
                .dirs
                .iter()
                .flat_map(|dir| dir.docs.iter())
                .any(|doc| doc.row.id == id && doc.row.version > 0),
            Where::DocumentFiles(id) => {
                self.attachment_dirs.contains(&where_)
                    && self.document(id).is_some_and(|doc| doc.standing.reads())
            }
            Where::ChatFiles(chat) => {
                self.attachment_dirs.contains(&where_) && self.chat_origin(chat).is_some()
            }
            _ => false,
        }
    }

    /// What a listed place is.
    async fn about(&self, conn: &mut Conn, at: Where) -> Result<Option<About>, EngineError> {
        let Where::Place(id) = at else {
            return Ok(None);
        };
        let Some(dir) = self.dir_of(id) else {
            return Ok(None);
        };
        let place = &dir.place;
        let tags = if place.kind == "topic" {
            rows::topic_tag_names(conn, place.id).await?
        } else {
            Vec::new()
        };
        Ok(Some(About {
            kind: kind_of(&place.kind),
            name: place.name.clone(),
            description: Some(place.description.trim().to_owned()).filter(|text| !text.is_empty()),
            status: place.status.clone(),
            tags,
        }))
    }

    /// Lists a directory `depth` levels down, filtered by `kinds` and by a pattern over names, at most
    /// [`ENTRIES_MAX`] entries, directories before files. `path` is the directory's path.
    pub async fn list(
        &self,
        conn: &mut Conn,
        at: Where,
        path: &str,
        depth: u32,
        kinds: Option<&[SharedEntryKind]>,
        names: Option<&Pattern>,
    ) -> Result<Listing, EngineError> {
        self.list_page(
            conn,
            at,
            path,
            depth,
            kinds,
            names,
            PageOptions {
                sort: Sort::Name,
                include_archived: true,
                limit: ENTRIES_MAX,
                after: None,
            },
        )
        .await?
        .map(|page| page.listing)
        .ok_or_else(|| {
            EngineError::unreadable("an initial directory page has no continuation to lose")
        })
    }

    /// Lists one current page. A missing continuation path means the directory changed; callers start again.
    #[allow(
        clippy::too_many_arguments,
        reason = "the existing directory selection and its bound continuation share one traversal"
    )]
    pub async fn list_page(
        &self,
        conn: &mut Conn,
        at: Where,
        path: &str,
        depth: u32,
        kinds: Option<&[SharedEntryKind]>,
        names: Option<&Pattern>,
        options: PageOptions<'_>,
    ) -> Result<Option<ListingPage>, EngineError> {
        let mut out: Vec<Entry> = Vec::new();
        let mut archived_count = 0;
        let mut queue: VecDeque<(Where, String, u32)> = VecDeque::new();
        queue.push_back((at, path.to_owned(), 1));
        while let Some((where_, prefix, level)) = queue.pop_front() {
            let mut direct = match where_ {
                Where::History(document) => self.version_entries(conn, document, &prefix).await?,
                Where::DocumentFiles(_) | Where::ChatFiles(_) => {
                    self.attachment_entries(conn, where_, &prefix).await?
                }
                other => self.direct_entries(other, &prefix),
            };
            for entry in &mut direct {
                entry.archived = self.archived_at(entry.at);
            }
            for entry in Self::sorted_for(direct, options.sort) {
                if entry.archived && !options.include_archived {
                    archived_count += 1;
                    continue;
                }
                if level < depth && entry.is_directory() {
                    queue.push_back((entry.at, entry.path.clone(), level + 1));
                }
                if kinds.is_some_and(|kinds| !kinds.contains(&entry.kind))
                    || names.is_some_and(|pattern| !pattern.matches(&entry.name))
                {
                    continue;
                }
                out.push(entry);
            }
        }
        let start = match options.after {
            None => 0,
            Some(path) => match out.iter().position(|entry| entry.path == path) {
                Some(at) => at + 1,
                None => return Ok(None),
            },
        };
        let end = start.saturating_add(options.limit).min(out.len());
        let next = (end < out.len()).then(|| out[end - 1].path.clone());
        let entries = out.into_iter().skip(start).take(options.limit).collect();
        let about = self.about(conn, at).await?;
        Ok(Some(ListingPage {
            listing: Listing {
                path: path.to_owned(),
                about,
                entries,
                truncated: self.capped || next.is_some(),
            },
            next,
            archived_count,
        }))
    }

    // ------------------------------------------------------------ finding a path again

    /// Finds what a path names, without saying whether the agent may read it: the directory, or the file and the
    /// directory it is in. Nothing where no entry of the tree has that name.
    #[must_use]
    pub fn find(&self, named: &Named, consumed: usize) -> Option<Found> {
        self.find_all(named, consumed).into_iter().next()
    }

    /// Follows the same path for all visible twins; explicit identifiers and assigned suffixes stay exact.
    #[must_use]
    pub fn find_all(&self, named: &Named, consumed: usize) -> Vec<Found> {
        match named {
            Named::Id(id) => self.find_id(*id).into_iter().collect(),
            Named::Path { parts, .. } => {
                let Some((cursor, path, rest)) = self.start(parts, consumed) else {
                    return Vec::new();
                };
                let mut paths = vec![(cursor, path)];
                for part in rest {
                    paths = paths
                        .into_iter()
                        .flat_map(|(cursor, path)| {
                            self.steps(cursor, part)
                                .into_iter()
                                .map(move |(next, step)| (next, format!("{path}{step}")))
                        })
                        .collect();
                }
                paths
                    .into_iter()
                    .map(|(cursor, path)| Found {
                        entry: cursor.entry(self),
                        path,
                    })
                    .collect()
            }
        }
    }

    /// Reader-safe facts for a name candidate, using exactly the metadata already exposed by the shared tree.
    #[must_use]
    pub fn candidate(&self, found: &Found) -> Option<Entry> {
        let mut entry = match found.entry {
            FoundEntry::Directory(Where::Place(id)) | FoundEntry::File(Where::Conversation(id)) => {
                let at = *self.by_id.get(&id)?;
                if !self.dirs[at].present {
                    return None;
                }
                let mut entry = self.place_entry(at, "");
                if let Some(updated) = self.dirs[at]
                    .conversation
                    .as_ref()
                    .and_then(|conversation| conversation.updated)
                {
                    entry.updated_at = Some(Time::new(updated));
                }
                if self.dirs[at].place.archived {
                    entry.state = Some("archived".to_owned());
                }
                entry
            }
            FoundEntry::File(Where::Document(id) | Where::Version(id, _)) => {
                let doc = self.document_either(id)?;
                let mut entry = Entry::bare(
                    found.path.clone(),
                    doc.name.clone(),
                    SharedEntryKind::Document,
                    id,
                    Where::Document(id),
                );
                entry.updated_at = Some(Time::new(doc.row.updated_at));
                entry.deleted = doc.row.deleted_at.is_some();
                entry.state = Some(if entry.deleted { "deleted" } else { "live" }.to_owned());
                entry.readable = doc.standing.reads();
                entry
            }
            FoundEntry::Directory(Where::DocumentFolder(id)) => self.folder_entry(id, "")?,
            FoundEntry::File(at @ (Where::DocumentFile { .. } | Where::ChatFile { .. })) => {
                let id = match at {
                    Where::DocumentFile { file, .. } => file,
                    Where::ChatFile {
                        chat,
                        message,
                        carried,
                    } => attachments::chat_file_id(chat, message, carried),
                    _ => unreachable!("only attachment targets enter this arm"),
                };
                Entry::bare(
                    found.path.clone(),
                    found.path.rsplit('/').next().unwrap_or_default().to_owned(),
                    SharedEntryKind::File,
                    id,
                    at,
                )
            }
            _ => return None,
        };
        entry.path.clone_from(&found.path);
        Some(entry)
    }

    fn steps(&self, cursor: Cursor, part: &str) -> Vec<(Cursor, String)> {
        match cursor {
            Cursor::Place(at) if part != ".." => {
                let children = &self.dirs[at].children;
                let entries: Vec<_> = children
                    .iter()
                    .map(|child| {
                        let dir = &self.dirs[*child];
                        (
                            dir.place.id,
                            dir.place
                                .slug
                                .clone()
                                .unwrap_or_else(|| dir.place.name.clone()),
                        )
                    })
                    .collect();
                let names: Vec<_> = children
                    .iter()
                    .map(|child| self.dirs[*child].name.clone())
                    .collect();
                let choices = matching_names(&entries, &names, part, "");
                if !choices.is_empty() {
                    return choices
                        .into_iter()
                        .filter_map(|at| {
                            let child = children[at];
                            self.dirs[child].present.then(|| {
                                (Cursor::Place(child), format!("{}/", self.dirs[child].name))
                            })
                        })
                        .collect();
                }
            }
            Cursor::Directory(at @ (Where::Docs(_) | Where::DocumentFolder(_))) => {
                let parent = match at {
                    Where::Docs(node) => organization::Directory::NodeDocs(node),
                    Where::DocumentFolder(id) => organization::Directory::Container(id),
                    _ => unreachable!("document directories only"),
                };
                let Some(node) = self.directory_node(parent) else {
                    return Vec::new();
                };
                let folder_entries: Vec<_> = self
                    .folder_children(parent)
                    .into_iter()
                    .filter_map(|id| self.folder_entry(id, ""))
                    .collect();
                let folder_names: Vec<_> = folder_entries
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect();
                let folder_matches: Vec<_> = folders::matching_basenames(&folder_names, part)
                    .into_iter()
                    .map(|at| {
                        let entry = &folder_entries[at];
                        (Cursor::Directory(entry.at), format!("{}/", entry.name))
                    })
                    .collect();
                if !folder_matches.is_empty() {
                    return folder_matches;
                }
                if let Some(dir) = self.dir_of(node) {
                    let docs: Vec<_> = dir
                        .docs
                        .iter()
                        .chain(&dir.deleted)
                        .filter(|doc| self.document_parent(doc.row.id) == Some(parent))
                        .collect();
                    if let Some(stem) = part.strip_suffix(HISTORY_SUFFIX) {
                        let choices =
                            folders::matching_documents(&docs, &format!("{stem}{DOCUMENT_SUFFIX}"));
                        return choices
                            .into_iter()
                            .filter(|at| docs[*at].row.deleted_at.is_none())
                            .map(|at| {
                                let name = docs[at]
                                    .name
                                    .strip_suffix(DOCUMENT_SUFFIX)
                                    .unwrap_or(&docs[at].name);
                                (
                                    Cursor::Directory(Where::History(docs[at].row.id)),
                                    format!("{name}{HISTORY_SUFFIX}/"),
                                )
                            })
                            .collect();
                    }
                    if let Some(stem) = part.strip_suffix(FILES_SUFFIX) {
                        let choices =
                            folders::matching_documents(&docs, &format!("{stem}{DOCUMENT_SUFFIX}"));
                        return choices
                            .into_iter()
                            .filter(|at| {
                                docs[*at].row.deleted_at.is_none() && docs[*at].standing.reads()
                            })
                            .map(|at| {
                                let stem = docs[at]
                                    .name
                                    .strip_suffix(DOCUMENT_SUFFIX)
                                    .unwrap_or(&docs[at].name);
                                (
                                    Cursor::Directory(Where::DocumentFiles(docs[at].row.id)),
                                    format!("{stem}{FILES_SUFFIX}/"),
                                )
                            })
                            .collect();
                    }
                    let choices = folders::matching_documents(&docs, part);
                    if !choices.is_empty() {
                        return choices
                            .into_iter()
                            .map(|at| {
                                (
                                    Cursor::File(Where::Document(docs[at].row.id)),
                                    docs[at].name.clone(),
                                )
                            })
                            .collect();
                    }
                }
            }
            _ => {}
        }
        self.step(cursor, part).into_iter().collect()
    }

    /// Where following a path begins: the top, or the node the tree was read under, with the parts of the path
    /// that are left to follow from there.
    fn start<'a>(
        &self,
        parts: &'a [String],
        consumed: usize,
    ) -> Option<(Cursor, String, &'a [String])> {
        if let Some(at) = self.scope_at {
            return Some((Cursor::Place(at), self.path_to(at), parts.get(consumed..)?));
        }
        Some((Cursor::Root, String::from("/"), parts))
    }

    fn find_id(&self, id: Id) -> Option<Found> {
        if let Some(at) = self.by_id.get(&id) {
            return Some(Found {
                entry: FoundEntry::Directory(Where::Place(id)),
                path: self.path_to(*at),
            });
        }
        if self.folder_exists(id) {
            return Some(Found {
                entry: FoundEntry::Directory(Where::DocumentFolder(id)),
                path: self.path_of_directory(organization::Directory::Container(id))?,
            });
        }
        for dir in &self.dirs {
            if let Some(chat) = &dir.chat
                && chat.id == id
            {
                let prefix = self.path_of_node(dir.place.id)?;
                return Some(Found {
                    entry: FoundEntry::File(Where::NodeChat(dir.place.id)),
                    path: format!("{prefix}{CHAT}"),
                });
            }
            for doc in dir.docs.iter().chain(dir.deleted.iter()) {
                if doc.row.id == id {
                    return Some(Found {
                        entry: FoundEntry::File(Where::Document(id)),
                        path: self.document_path(id)?,
                    });
                }
            }
        }
        for (root, chats) in &self.messages {
            if let Some(chat) = chats.iter().find(|chat| chat.chat == id) {
                let at = self.by_id.get(root)?;
                return Some(Found {
                    entry: FoundEntry::File(Where::PeopleChat(id)),
                    path: format!("{}{MESSAGES}/{}/{CHAT}", self.path_to(*at), chat.name),
                });
            }
        }
        None
    }

    fn step(&self, cursor: Cursor, part: &str) -> Option<(Cursor, String)> {
        match cursor {
            Cursor::Root => self
                .roots
                .iter()
                .find(|at| self.root_names.get(at).is_some_and(|name| name == part))
                .map(|at| (Cursor::Place(*at), format!("{part}/"))),
            Cursor::Place(at) => {
                let dir = &self.dirs[at];
                if part == ".." {
                    return dir
                        .parent
                        .map(|parent| (Cursor::Place(parent), String::new()));
                }
                if let Some(child) = self.child_named(at, part) {
                    return Some((Cursor::Place(child), format!("{part}/")));
                }
                match part {
                    CONVERSATION => dir.conversation.as_ref().map(|_| {
                        (
                            Cursor::File(Where::Conversation(dir.place.id)),
                            part.to_owned(),
                        )
                    }),
                    CHAT => dir
                        .chat
                        .as_ref()
                        .map(|_| (Cursor::File(Where::NodeChat(dir.place.id)), part.to_owned())),
                    "chat.files" => dir
                        .chat
                        .as_ref()
                        .filter(|chat| {
                            chat.standing.reads()
                                && self.attachment_dirs.contains(&Where::ChatFiles(chat.id))
                        })
                        .map(|chat| {
                            (
                                Cursor::Directory(Where::ChatFiles(chat.id)),
                                format!("{part}/"),
                            )
                        }),
                    DOCS => Some((
                        Cursor::Directory(Where::Docs(dir.place.id)),
                        format!("{part}/"),
                    )),
                    MESSAGES if dir.place.parent_id.is_none() => {
                        self.messages.contains_key(&dir.place.id).then(|| {
                            (
                                Cursor::Directory(Where::Messages(dir.place.id)),
                                format!("{part}/"),
                            )
                        })
                    }
                    _ => None,
                }
            }
            Cursor::Directory(at @ (Where::Docs(_) | Where::DocumentFolder(_))) => {
                let parent = match at {
                    Where::Docs(node) => organization::Directory::NodeDocs(node),
                    Where::DocumentFolder(id) => organization::Directory::Container(id),
                    _ => unreachable!("document directories only"),
                };
                let node = self.directory_node(parent)?;
                let dir = self.dir_of(node)?;
                if let Some(folder) = self
                    .folder_children(parent)
                    .into_iter()
                    .filter_map(|id| self.folder_entry(id, ""))
                    .find(|entry| entry.name == part)
                {
                    return Some((Cursor::Directory(folder.at), format!("{part}/")));
                }
                if let Some(doc) = dir.docs.iter().chain(dir.deleted.iter()).find(|doc| {
                    self.document_parent(doc.row.id) == Some(parent) && doc.name == part
                }) {
                    return Some((Cursor::File(Where::Document(doc.row.id)), part.to_owned()));
                }
                if let Some(stem) = part.strip_suffix(HISTORY_SUFFIX) {
                    let doc = dir.docs.iter().find(|doc| {
                        self.document_parent(doc.row.id) == Some(parent)
                            && doc.name.strip_suffix(DOCUMENT_SUFFIX) == Some(stem)
                    })?;
                    return Some((
                        Cursor::Directory(Where::History(doc.row.id)),
                        format!("{part}/"),
                    ));
                }
                if let Some(stem) = part.strip_suffix(FILES_SUFFIX) {
                    let doc = dir.docs.iter().find(|doc| {
                        self.document_parent(doc.row.id) == Some(parent)
                            && doc.standing.reads()
                            && doc.name.strip_suffix(DOCUMENT_SUFFIX) == Some(stem)
                    })?;
                    return Some((
                        Cursor::Directory(Where::DocumentFiles(doc.row.id)),
                        format!("{part}/"),
                    ));
                }
                None
            }
            Cursor::Directory(Where::History(doc)) => {
                let number: i32 = part
                    .split_whitespace()
                    .next()
                    .and_then(|first| first.parse().ok())?;
                Some((Cursor::File(Where::Version(doc, number)), part.to_owned()))
            }
            Cursor::Directory(Where::Messages(root)) => {
                let chat = self
                    .messages
                    .get(&root)?
                    .iter()
                    .find(|chat| chat.name == part)?;
                Some((
                    Cursor::Directory(Where::PeopleChatDir(chat.chat)),
                    format!("{part}/"),
                ))
            }
            Cursor::Directory(Where::PeopleChatDir(chat)) => {
                if part == "chat.files" && self.chat_origin(chat).is_some() {
                    Some((
                        Cursor::Directory(Where::ChatFiles(chat)),
                        format!("{part}/"),
                    ))
                } else {
                    (part == CHAT).then(|| (Cursor::File(Where::PeopleChat(chat)), part.to_owned()))
                }
            }
            _ => None,
        }
    }

    /// The child of a node that a name means: its own name, a twin's name with its identifier tail, or
    /// the one name that differs only in case.
    fn child_named(&self, at: usize, part: &str) -> Option<usize> {
        let children = &self.dirs[at].children;
        let entries: Vec<_> = children
            .iter()
            .map(|child| {
                let dir = &self.dirs[*child];
                (
                    dir.place.id,
                    dir.place
                        .slug
                        .clone()
                        .unwrap_or_else(|| dir.place.name.clone()),
                )
            })
            .collect();
        let names: Vec<_> = children
            .iter()
            .map(|child| self.dirs[*child].name.clone())
            .collect();
        matching_names(&entries, &names, part, "")
            .first()
            .map(|at| children[*at])
    }

    /// The names an agent may have meant, for a path that names nothing: the entries of the deepest directory
    /// that is there, those that read like the last name first.
    #[must_use]
    pub fn closest(&self, named: &Named, consumed: usize, count: usize) -> (String, Vec<String>) {
        let top = (String::from("/"), Vec::new());
        let Named::Path { parts, .. } = named else {
            return top;
        };
        let Some((mut cursor, mut path, rest)) = self.start(parts, consumed) else {
            return top;
        };
        // The deepest directory that is there for the agent, and the first name that led out of it: a directory
        // that exists and is not the agent's to see answers as one that does not.
        let mut best: Option<(Where, String, Option<&str>)> = None;
        let mut led_out: Option<&str> = None;
        for part in rest.iter().map(String::as_str).chain(std::iter::once("")) {
            let here = match cursor {
                Cursor::Root => Some(Where::Root),
                Cursor::Place(at) => Some(Where::Place(self.dirs[at].place.id)),
                Cursor::Directory(where_) => Some(where_),
                Cursor::File(_) => None,
            };
            if let Some(where_) = here.filter(|where_| self.directory_at(*where_)) {
                best = Some((where_, path.clone(), None));
                led_out = None;
            }
            if part.is_empty() {
                break;
            }
            if led_out.is_none() {
                led_out = Some(part);
            }
            if let Some((_, _, wanted)) = best.as_mut() {
                *wanted = led_out;
            }
            match self.step(cursor, part) {
                Some((next, step)) => {
                    path.push_str(&step);
                    cursor = next;
                }
                None => break,
            }
        }
        let Some((where_, directory, wanted)) = best else {
            return top;
        };
        let here: Vec<String> = Self::sorted(self.direct_entries(where_, &directory))
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        let wanted = wanted.unwrap_or_default().to_lowercase();
        let mut scored: Vec<(bool, String)> = here
            .into_iter()
            .map(|name| {
                let lower = name.to_lowercase();
                let near =
                    !wanted.is_empty() && (lower.contains(&wanted) || wanted.contains(&lower));
                (!near, name)
            })
            .collect();
        scored.sort_by_key(|(far, _)| *far);
        (
            directory,
            scored
                .into_iter()
                .take(count)
                .map(|(_, name)| name)
                .collect(),
        )
    }

    /// What the agent may do with a thread's conversation, and how many turns it has; nothing where the thread
    /// is not in the tree.
    #[must_use]
    pub fn conversation_of(&self, thread: Id) -> Option<(Standing, i64)> {
        self.dir_of(thread)
            .and_then(|dir| dir.conversation.as_ref())
            .map(|conversation| (conversation.standing, conversation.turns))
    }

    /// The chat of a node, what the agent may do with it, and how many messages it has.
    #[must_use]
    pub fn node_chat_of(&self, node: Id) -> Option<(Id, Standing, i64)> {
        self.dir_of(node)
            .and_then(|dir| dir.chat.as_ref())
            .map(|chat| (chat.id, chat.standing, chat.messages))
    }

    /// A direct or group chat that was shared.
    #[must_use]
    pub fn people_chat_of(&self, chat: Id) -> Option<&PeopleEntry> {
        self.messages
            .values()
            .flatten()
            .find(|entry| entry.chat == chat)
    }

    /// A document the agent may read, and the node it is beside.
    #[must_use]
    pub fn document(&self, id: Id) -> Option<&DocEntry> {
        self.dirs
            .iter()
            .flat_map(|dir| dir.docs.iter())
            .find(|doc| doc.row.id == id)
    }

    /// A document that was deleted and that the agent may bring back.
    #[must_use]
    pub fn deleted_document(&self, id: Id) -> Option<&DocEntry> {
        self.dirs
            .iter()
            .flat_map(|dir| dir.deleted.iter())
            .find(|doc| doc.row.id == id)
    }

    /// A document the agent may read, or one that was deleted and that it may bring back.
    #[must_use]
    pub fn document_either(&self, id: Id) -> Option<&DocEntry> {
        self.document(id).or_else(|| self.deleted_document(id))
    }

    /// What the agent may do with the documents beside a node: its `write` is the right to make a document
    /// there.
    #[must_use]
    pub fn documents_standing(&self, node: Id) -> Standing {
        self.dir_of(node)
            .map(|dir| dir.docs_standing)
            .unwrap_or_default()
    }

    /// A node of the tree.
    #[must_use]
    pub fn place(&self, id: Id) -> Option<&PlaceRow> {
        self.dir_of(id).map(|dir| &dir.place)
    }

    /// The name the tree gives a node.
    #[must_use]
    pub fn name_of(&self, id: Id) -> Option<&str> {
        self.dir_of(id).map(|dir| dir.name.as_str())
    }

    /// The name of a chat as a heading says it: the node's name for the chat of a node, the label for a direct
    /// or group chat.
    #[must_use]
    pub fn chat_title(&self, chat: Id) -> Option<String> {
        if let Some(entry) = self.people_chat_of(chat) {
            return Some(format!("Chat of {}", entry.label));
        }
        self.dirs
            .iter()
            .find(|dir| {
                dir.chat
                    .as_ref()
                    .is_some_and(|node_chat| node_chat.id == chat)
            })
            .map(|dir| format!("Chat of {}", dir.place.name))
    }

    /// The scope the tree was read for.
    #[must_use]
    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// The thread directories the agent may read the conversations of, with their paths.
    #[must_use]
    pub fn conversations(&self) -> Vec<(Id, String, Entry)> {
        let mut out = Vec::new();
        for (at, dir) in self.dirs.iter().enumerate() {
            let Some(conversation) = &dir.conversation else {
                continue;
            };
            if !conversation.standing.reads() {
                continue;
            }
            let prefix = self.path_to(at);
            let mut entry = Entry::bare(
                format!("{prefix}{CONVERSATION}"),
                CONVERSATION.to_owned(),
                SharedEntryKind::Conversation,
                dir.place.id,
                Where::Conversation(dir.place.id),
            );
            entry.unit = Some(SharedEntryUnit::Turn);
            entry.parts = Some(conversation.turns);
            out.push((dir.place.id, prefix, entry));
        }
        out
    }

    /// The chats of nodes, and the shared direct and group chats, that the agent may read, with their paths.
    #[must_use]
    pub fn chats(&self) -> Vec<(Id, String, Option<Id>)> {
        let mut out = Vec::new();
        for (at, dir) in self.dirs.iter().enumerate() {
            if let Some(chat) = &dir.chat
                && chat.standing.reads()
            {
                out.push((
                    chat.id,
                    format!("{}{CHAT}", self.path_to(at)),
                    Some(dir.place.id),
                ));
            }
        }
        for (root, chats) in &self.messages {
            let Some(at) = self.by_id.get(root) else {
                continue;
            };
            for chat in chats.iter().filter(|chat| chat.standing.reads()) {
                out.push((
                    chat.chat,
                    format!("{}{MESSAGES}/{}/{CHAT}", self.path_to(*at), chat.name),
                    None,
                ));
            }
        }
        out
    }

    /// The documents the agent may read, with their paths.
    #[must_use]
    pub fn documents(&self) -> Vec<(&DocEntry, String)> {
        let mut out = Vec::new();
        for dir in &self.dirs {
            for doc in &dir.docs {
                if doc.standing.reads()
                    && let Some(path) = self.document_path(doc.row.id)
                {
                    out.push((doc, path));
                }
            }
        }
        out
    }

    /// The places and threads of the tree with their paths, for their names to be searched.
    #[must_use]
    pub fn places(&self) -> Vec<(Id, String, String, SharedEntryKind)> {
        self.dirs
            .iter()
            .enumerate()
            .filter(|(_, dir)| dir.present)
            .map(|(at, dir)| {
                (
                    dir.place.id,
                    self.path_to(at),
                    dir.place.name.clone(),
                    kind_of(&dir.place.kind),
                )
            })
            .collect()
    }
}

/// What a cursor walks through while a path is followed.
#[derive(Debug, Clone, Copy)]
enum Cursor {
    Root,
    Place(usize),
    Directory(Where),
    File(Where),
}

impl Cursor {
    fn entry(self, tree: &Tree) -> FoundEntry {
        match self {
            Self::Root => FoundEntry::Directory(Where::Root),
            Self::Place(at) => FoundEntry::Directory(Where::Place(tree.dirs[at].place.id)),
            Self::Directory(where_) => FoundEntry::Directory(where_),
            Self::File(where_) => FoundEntry::File(where_),
        }
    }
}

/// What a path names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundEntry {
    /// A directory.
    Directory(Where),
    /// A file.
    File(Where),
}

/// A path found: what it names and its normal form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// What it names.
    pub entry: FoundEntry,
    /// The path, from the top.
    pub path: String,
}

/// The identifier of a folder, which is no thing of its own: derived from what it holds.
fn ids_of(word: &str, id: Id) -> Id {
    crate::ids::derived(&format!("folder:{word}"), &[id.as_bytes().as_slice()])
}

fn entry_via(standing: &Standing) -> SharedEntryVia {
    standing
        .read
        .or(standing.write)
        .map_or(SharedEntryVia::Share, via_of)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn a_slash_in_a_name_is_written_as_a_dash_and_a_name_that_is_a_path_is_made_a_name() {
        assert_eq!(tree_name("Fix login / SSO"), "Fix login - SSO");
        assert_eq!(tree_name(".."), "_.._");
        assert_eq!(tree_name("  "), "__");
    }

    #[test]
    fn of_two_entries_of_one_name_the_oldest_keeps_it_and_the_others_have_the_tail_of_their_identifier()
     {
        let (a, b, c) = (new_id(), new_id(), new_id());
        let names = assign_names(
            &[
                (a, "Plan".to_owned()),
                (b, "Plan".to_owned()),
                (c, "Notes".to_owned()),
            ],
            ".md",
            false,
        );
        assert_eq!(names[0], "Plan.md");
        assert_eq!(names[2], "Notes.md");
        assert!(
            names[1].starts_with("Plan~") && names[1].ends_with(".md"),
            "{}",
            names[1]
        );
        let digits = b.simple().to_string();
        assert!(names[1].contains(&digits[digits.len() - SUFFIX_CHARS..]));
    }

    #[test]
    fn same_timestamp_names_take_four_random_tail_digits_and_extend_only_a_colliding_tail() {
        let entries = [
            (
                "0199c1a0-0000-7000-8000-000000000001".parse().unwrap(),
                "Plan".to_owned(),
            ),
            (
                "0199c1a0-0000-7000-8000-000000012345".parse().unwrap(),
                "Plan".to_owned(),
            ),
            (
                "0199c1a0-0000-7000-8000-0000000a2345".parse().unwrap(),
                "Plan".to_owned(),
            ),
        ];
        let names = assign_names(&entries, ".md", false);
        assert_eq!(names, ["Plan.md", "Plan~2345.md", "Plan~a2345.md"]);
        assert_eq!(
            matching_names(&entries, &names, "Plan.md", ".md"),
            [0, 1, 2]
        );
        assert_eq!(
            matching_names(&entries, &names, "Plan~a2345.md", ".md"),
            [2]
        );
    }

    #[test]
    fn a_name_the_layout_reserves_has_the_suffix_even_when_it_is_the_only_one() {
        let id = new_id();
        let names = assign_names(
            &[
                (id, "docs".to_owned()),
                (new_id(), "chat.md".to_owned()),
                (new_id(), "Plan.files".to_owned()),
                (new_id(), "Notes".to_owned()),
            ],
            "",
            true,
        );
        assert!(names[0].starts_with("docs~"));
        assert!(names[1].starts_with("chat.md~"));
        assert!(names[2].starts_with("Plan.files~"));
        assert_eq!(names[3], "Notes");
    }

    #[test]
    fn entries_made_together_still_get_names_that_differ() {
        // The start of an identifier is the time it was made: entries made together share their first characters.
        let ids: Vec<Id> = (0..5).map(|_| new_id()).collect();
        let entries: Vec<(Id, String)> = ids.iter().map(|id| (*id, "Docs".to_owned())).collect();
        let names = assign_names(&entries, "", false);
        let distinct: BTreeSet<&String> = names.iter().collect();
        assert_eq!(distinct.len(), 5, "{names:?}");
    }

    #[test]
    fn a_path_is_read_from_the_top_or_from_the_topic_and_an_id_is_taken_wherever_a_path_is() {
        assert_eq!(
            parse_path("/acme/Web/"),
            Some(Named::Path {
                absolute: true,
                parts: vec!["acme".to_owned(), "Web".to_owned()]
            })
        );
        assert_eq!(
            parse_path("./docs/Week 41.md"),
            Some(Named::Path {
                absolute: false,
                parts: vec!["docs".to_owned(), "Week 41.md".to_owned()]
            })
        );
        let id = new_id();
        assert_eq!(parse_path(&format!("id:{id}")), Some(Named::Id(id)));
        assert_eq!(parse_path("id:nonsense"), None);
    }

    #[test]
    fn a_pattern_over_names_is_a_shells() {
        let pattern = Pattern::new("*.md").unwrap();
        assert!(pattern.matches("Plan.md") && !pattern.matches("Plan.txt"));
        assert!(Pattern::new("[a").is_none());
    }
}
