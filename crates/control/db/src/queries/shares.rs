//! What agents were given: copies with their delivery and revocation, and live shares by area.
//!
//! A live share is entries on pages whose holder is the agents of a place: the rows of `permission_entries`
//! are the source of truth and nothing is kept beside them, so that a share taken back is gone from the next
//! statement that reads them. A copy is a row of `agent_shares`. Beside those, this module holds the statements
//! the shared tree is made of: the places of the hierarchy by name, the documents and the chats beside them, and
//! a conversation, a chat and a document read by the numbers of their parts.

use crate::queries::threads::{ItemRow, TurnRow};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;
use std::collections::HashMap;

// ---------------------------------------------------------------- live shares

/// An entry whose holder is the agents of a place, as it stands on its page.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct GrantRow {
    /// A first-reading hint on agents/doc.view only.
    pub read_first: bool,
    /// The entry.
    pub entry_id: Id,
    /// What the page is of: `node`, `document` or `chat`.
    pub object_kind: String,
    /// Which one.
    pub object_id: Id,
    /// `live`, or `defaults` for what the place gives to what is made inside it.
    pub page: String,
    /// The row's profile; `any` except on a defaults page of a permission that has two.
    pub profile: String,
    /// The permission of the row, by its name.
    pub permission: String,
    /// The place whose agents are named.
    pub holder_id: Id,
    /// The retained reach of a place share; None for ordinary or older entries.
    pub share_inside: Option<bool>,
    /// Who wrote the entry.
    pub created_by: Option<Id>,
    /// When.
    pub created_at: DateTime<Utc>,
    /// When it ends by itself.
    pub expires_at: Option<DateTime<Utc>>,
    /// Its note.
    pub note: String,
    /// How it came about.
    pub source: String,
    /// How many times it was changed.
    pub revision: i64,
    /// For an entry on the page of a node: what kind of node it is (`org`, `project`, `topic` or
    /// `thread`), which tells a place, whose share can be of the place alone, from a thread.
    pub node_kind: Option<String>,
}

const GRANT_COLUMNS: &str =
    "e.id AS entry_id, e.read_first, e.object_kind, e.object_id, e.page, e.profile, e.permission, e.holder_id, e.share_inside,
    e.created_by, e.created_at, e.expires_at, e.note, e.source, e.revision, n.kind AS node_kind";

/// Where the entries are read from: the entries, each with the node its page is of.
const GRANT_FROM: &str =
    "permission_entries e LEFT JOIN nodes n ON e.object_kind = 'node' AND n.id = e.object_id";

/// The entries that name the agents of any of `places` in a row of one of `permissions`, as they count at `now`.
pub async fn grants_to(
    conn: &mut Conn,
    places: &[Id],
    permissions: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<GrantRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {GRANT_COLUMNS} FROM {GRANT_FROM}
          WHERE e.holder_kind = 'agents' AND e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > $2)
            AND e.holder_id = ANY($1) AND e.permission = ANY($3)
          ORDER BY e.created_at, e.id"
    )))
    .bind(places)
    .bind(now)
    .bind(permissions)
    .fetch_all(conn)
    .await?)
}

/// The entries that name agents, of any place, on the pages of one thing (`live`, and for a place `defaults`), as
/// they count at `now`.
pub async fn grants_on(
    conn: &mut Conn,
    object_kind: &str,
    object_id: Id,
    permissions: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<GrantRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {GRANT_COLUMNS} FROM {GRANT_FROM}
          WHERE e.holder_kind = 'agents' AND e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > $4)
            AND e.object_kind = $1 AND e.object_id = $2 AND e.permission = ANY($3)
          ORDER BY e.created_at, e.id"
    )))
    .bind(object_kind)
    .bind(object_id)
    .bind(permissions)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// Every entry that names agents, wherever it stands and whatever it names, that counts at `now`: where shares are
/// looked for by their identifier.
pub async fn all_grants(
    conn: &mut Conn,
    permissions: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<GrantRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {GRANT_COLUMNS} FROM {GRANT_FROM}
          WHERE e.holder_kind = 'agents' AND e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > $2)
            AND e.permission = ANY($1)
          ORDER BY e.created_at, e.id"
    )))
    .bind(permissions)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- the places of the tree

/// A node as the tree shows it, with the facts of the kind it is.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PlaceRow {
    /// Its identifier.
    pub id: Id,
    /// `org`, `project`, `topic` or `thread`.
    pub kind: String,
    /// The node it lies in.
    pub parent_id: Option<Id>,
    /// Its root org.
    pub org_id: Id,
    /// The nodes from its root org down to it.
    pub path: Vec<Id>,
    /// Its name.
    pub name: String,
    /// What it is about.
    pub description: String,
    /// For an org, its short name.
    pub slug: Option<String>,
    /// Whether it is archived itself.
    pub archived: bool,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
    /// Its first owner.
    pub owner_id: Id,
    /// For a thread: the thread it is a side of.
    pub side_of: Option<Id>,
    /// For a thread: what it is doing.
    pub state: Option<String>,
    /// For a thread: its agent profile.
    pub agent_id: Option<Id>,
    /// For a thread: its environment.
    pub environment_id: Option<Id>,
    /// For a thread: its directory there.
    pub workspace_dir: Option<String>,
    /// For a topic: `open` or `done`.
    pub status: Option<String>,
}

const PLACE_SELECT: &str =
    "SELECT n.id, n.kind, n.parent_id, n.org_id, n.path, n.name, n.description, o.slug,
        (n.archived_at IS NOT NULL) AS archived, n.created_at, n.updated_at, n.owner_id,
        t.side_of, t.state, t.agent_id, t.environment_id, t.workspace_dir, tp.status
   FROM nodes n
   LEFT JOIN orgs o ON o.node_id = n.id
   LEFT JOIN threads t ON t.node_id = n.id
   LEFT JOIN topics tp ON tp.node_id = n.id";

/// The nodes `ids` that are not deleted.
pub async fn places(conn: &mut Conn, ids: &[Id]) -> Result<Vec<PlaceRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PLACE_SELECT} WHERE n.id = ANY($1) AND n.deleted_at IS NULL ORDER BY n.created_at, n.id"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Selected path metadata without any descriptions or descendant inventory.
pub async fn header_places(conn: &mut Conn, ids: &[Id]) -> Result<Vec<PlaceRow>, DbError> {
    let select = PLACE_SELECT.replace("n.description", "''::text AS description");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{select} WHERE n.id=ANY($1) AND n.deleted_at IS NULL ORDER BY n.created_at,n.id"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// What lies directly in a node, oldest first, without the side threads that are asides of a thread.
pub async fn children_of(conn: &mut Conn, parent: Id) -> Result<Vec<PlaceRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PLACE_SELECT} WHERE n.parent_id = $1 AND n.deleted_at IS NULL AND t.side_of IS NULL
          ORDER BY n.created_at, n.id"
    )))
    .bind(parent)
    .fetch_all(conn)
    .await?)
}

/// Live document names beside these nodes, including unreadable siblings, solely for stable tree naming.
pub async fn document_names_beside(
    conn: &mut Conn,
    nodes: &[Id],
) -> Result<Vec<(Id, Id, String)>, DbError> {
    Ok(sqlx::query_as("SELECT id,node_id,title FROM documents WHERE node_id=ANY($1) AND deleted_at IS NULL ORDER BY created_at,id")
        .bind(nodes).fetch_all(conn).await?)
}

/// Everything below a node, at any depth, newest first, without side threads; at most `limit` of them.
/// Bounded legacy consumers report their own cut. Browsing consumers use
/// [`below_page`] so that every older candidate has a continuation.
pub async fn below(conn: &mut Conn, place: Id, limit: i64) -> Result<Vec<PlaceRow>, DbError> {
    Ok(below_page(conn, place, None, limit).await?.items)
}

/// One newest-first candidate page below a place. The position does not grant
/// access; the caller applies the ordinary reach policy to every loaded row.
#[derive(Debug)]
pub struct BelowPage {
    /// The scanned candidates.
    pub items: Vec<PlaceRow>,
    /// More candidates remain below the place.
    pub partial: bool,
    /// The exclusive timestamp/id key for the next page.
    pub next_cursor: Option<(DateTime<Utc>, Id)>,
}

/// Reads one keyset page. Limit is positive and supplied by a bounded consumer.
pub async fn below_page(
    conn: &mut Conn,
    place: Id,
    before: Option<(DateTime<Utc>, Id)>,
    limit: i64,
) -> Result<BelowPage, DbError> {
    let mut items: Vec<PlaceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PLACE_SELECT} WHERE $1 = ANY(n.path) AND n.id <> $1 AND n.deleted_at IS NULL AND t.side_of IS NULL
          AND ($3::timestamptz IS NULL OR (n.created_at, n.id) < ($3, $4::uuid))
          ORDER BY n.created_at DESC, n.id DESC LIMIT $2"
    )))
    .bind(place)
    .bind(limit.saturating_add(1))
    .bind(before.map(|(at, _)| at))
    .bind(before.map(|(_, id)| id))
    .fetch_all(conn)
    .await?;
    let count = usize::try_from(limit).unwrap_or(0);
    let partial = items.len() > count;
    items.truncate(count);
    let next_cursor = if partial {
        items.last().map(|row| (row.created_at, row.id))
    } else {
        None
    };
    Ok(BelowPage {
        items,
        partial,
        next_cursor,
    })
}

/// The root orgs `ids` among the nodes, whatever lies in them, without those that are deleted.
pub async fn root_orgs(conn: &mut Conn, ids: &[Id]) -> Result<Vec<PlaceRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PLACE_SELECT} WHERE n.id = ANY($1) AND n.parent_id IS NULL AND n.deleted_at IS NULL
          ORDER BY n.created_at, n.id"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// For each of `ids`, the nodes that are not deleted and whose path holds it: the nodes the agents of a place
/// reach. Side threads are not among them.
pub async fn live_threads_among(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.node_id = ANY($1) AND n.deleted_at IS NULL AND t.side_of IS NULL",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The nodes (or documents' nodes) among `ids` that have a deleted node on their way up: they and what they hold
/// are not there.
pub async fn under_deleted(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT n.id FROM nodes n
          WHERE n.id = ANY($1)
            AND EXISTS (SELECT 1 FROM nodes a WHERE a.id = ANY(n.path) AND a.deleted_at IS NOT NULL)",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The profile names of agents, by identifier.
pub async fn agent_names(conn: &mut Conn, ids: &[Id]) -> Result<HashMap<Id, String>, DbError> {
    let rows: Vec<(Id, String)> = sqlx::query_as("SELECT id, name FROM agents WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(conn)
        .await?;
    Ok(rows.into_iter().collect())
}

/// The names of environments, by identifier.
pub async fn environment_names(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<HashMap<Id, String>, DbError> {
    let rows: Vec<(Id, String)> =
        sqlx::query_as("SELECT id, name FROM environments WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(conn)
            .await?;
    Ok(rows.into_iter().collect())
}

/// The names of tags of a topic.
pub async fn topic_tag_names(conn: &mut Conn, topic: Id) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT g.name FROM node_tags t JOIN tags g ON g.id = t.tag_id WHERE t.node_id = $1 ORDER BY g.name",
    )
    .bind(topic)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- documents and chats beside a node

/// The documents written beside any of `nodes` that are not deleted, oldest first.
pub async fn documents_beside(conn: &mut Conn, nodes: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT d.id FROM documents d WHERE d.node_id = ANY($1) AND d.deleted_at IS NULL
          ORDER BY d.created_at, d.id",
    )
    .bind(nodes)
    .fetch_all(conn)
    .await?)
}

/// How many top-level blocks each document has: what a document is counted in.
pub async fn top_level_blocks(
    conn: &mut Conn,
    documents: &[Id],
) -> Result<HashMap<Id, i64>, DbError> {
    let rows: Vec<(Id, i64)> = sqlx::query_as(
        "SELECT document_id, count(*) FROM document_blocks
          WHERE document_id = ANY($1) AND parent_id IS NULL AND removed_at IS NULL GROUP BY document_id",
    )
    .bind(documents)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().collect())
}

/// A chat of a node, with what it is counted in.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct NodeChatRow {
    /// The chat.
    pub chat_id: Id,
    /// Its node.
    pub node_id: Id,
    /// When the last message was written.
    pub last_message_at: Option<DateTime<Utc>>,
    /// How many messages it holds, those deleted among them: the highest number.
    pub messages: i64,
}

/// The chats of `nodes`, with how many messages each holds.
pub async fn chats_of_nodes(conn: &mut Conn, nodes: &[Id]) -> Result<Vec<NodeChatRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id AS chat_id, c.node_id, c.last_message_at,
                (SELECT count(*) FROM chat_messages m WHERE m.chat_id = c.id) AS messages
           FROM chats c WHERE c.node_id = ANY($1)",
    )
    .bind(nodes)
    .fetch_all(conn)
    .await?)
}

/// A direct or group chat as the tree shows it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PeopleChatRow {
    /// The chat.
    pub chat_id: Id,
    /// Its root org.
    pub org_id: Id,
    /// `direct` or `group`.
    pub kind: String,
    /// A group's name.
    pub name: Option<String>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When the last message was written.
    pub last_message_at: Option<DateTime<Utc>>,
    /// How many messages it holds.
    pub messages: i64,
}

/// The direct and group chats `ids`.
pub async fn people_chats(conn: &mut Conn, ids: &[Id]) -> Result<Vec<PeopleChatRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id AS chat_id, c.org_id, c.kind, c.name, c.created_at, c.last_message_at,
                (SELECT count(*) FROM chat_messages m WHERE m.chat_id = c.id) AS messages
           FROM chats c WHERE c.id = ANY($1) AND c.kind <> 'node' ORDER BY c.created_at, c.id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The display names of the people who are in a chat now, by chat.
pub async fn member_names(
    conn: &mut Conn,
    chats: &[Id],
) -> Result<HashMap<Id, Vec<String>>, DbError> {
    let rows: Vec<(Id, String)> = sqlx::query_as(
        "SELECT m.chat_id, u.display_name FROM chat_members m JOIN users u ON u.id = m.user_id
          WHERE m.chat_id = ANY($1) AND m.left_at IS NULL ORDER BY m.joined_at, m.user_id",
    )
    .bind(chats)
    .fetch_all(conn)
    .await?;
    let mut by_chat: HashMap<Id, Vec<String>> = HashMap::new();
    for (chat, name) in rows {
        by_chat.entry(chat).or_default().push(name);
    }
    Ok(by_chat)
}

// ---------------------------------------------------------------- a conversation, by turns

/// The number of the last turn of a thread; none while it has none.
pub async fn last_turn(conn: &mut Conn, thread: Id) -> Result<i64, DbError> {
    match super::thread_history::active_view(conn, thread).await? {
        Some(view) => super::thread_history::last_turn(conn, thread, view).await,
        None => Ok(0),
    }
}

/// When the thread last changed: the moment of its newest item.
pub async fn last_item_at(conn: &mut Conn, thread: Id) -> Result<Option<DateTime<Utc>>, DbError> {
    match super::thread_history::active_view(conn, thread).await? {
        Some(view) => super::thread_history::last_item_at(conn, thread, view).await,
        None => Ok(None),
    }
}

/// The turns numbered `from` to `to`, both counted in, oldest first.
pub async fn turns_between(
    conn: &mut Conn,
    thread: Id,
    from: i64,
    to: i64,
) -> Result<Vec<TurnRow>, DbError> {
    match super::thread_history::active_view(conn, thread).await? {
        Some(view) => super::thread_history::turns_between(conn, thread, view, from, to).await,
        None => Ok(Vec::new()),
    }
}

const ITEM_COLUMNS: &str = "id, thread_id, continuation_id, segment_id, turn_id, model_input_id, stream, parent_item_id, kind, status, author_id,
        payload, native_ref, before_orch, search_text, pinned_at, pinned_by, chat_refs, seq_created, seq_updated, created_at, updated_at";

/// Live explicit item identities, oldest first; frozen range readers use thread_history instead.
pub async fn items_by_ids(
    conn: &mut Conn,
    thread: Id,
    ids: &[Id],
) -> Result<Vec<ItemRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ITEM_COLUMNS} FROM items WHERE thread_id = $1 AND id = ANY($2) ORDER BY seq_created"
    )))
    .bind(thread)
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The items of the main stream that were made in these turns, oldest first.
pub async fn items_of_turns(
    conn: &mut Conn,
    thread: Id,
    turns: &[Id],
) -> Result<Vec<ItemRow>, DbError> {
    match super::thread_history::active_view(conn, thread).await? {
        Some(view) => super::thread_history::items_of_turns(conn, thread, view, turns).await,
        None => Ok(Vec::new()),
    }
}

/// The items of the main stream that belong to no turn (the lines of the thread's history, mostly), made after
/// the sequence `after` and not after `up_to`, oldest first.
pub async fn items_between(
    conn: &mut Conn,
    thread: Id,
    after: i64,
    up_to: i64,
) -> Result<Vec<ItemRow>, DbError> {
    let Some(view) = super::thread_history::active_view(conn, thread).await? else {
        return Ok(Vec::new());
    };
    let mut items =
        super::thread_history::items_between(conn, thread, view, after, up_to, Some("main"))
            .await?;
    items.retain(|item| item.turn_id.is_none() && item.kind != "reasoning");
    Ok(items)
}

/// The lowest and the highest sequence at which an item of this turn was made.
pub async fn turn_span(conn: &mut Conn, turn: Id) -> Result<Option<(i64, i64)>, DbError> {
    let row: (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT min(seq_created), max(seq_created) FROM items WHERE turn_id = $1")
            .bind(turn)
            .fetch_one(conn)
            .await?;
    Ok(match row {
        (Some(low), Some(high)) => Some((low, high)),
        _ => None,
    })
}

/// What an approval asked, and who decided it, by approval.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ApprovalLine {
    /// The approval.
    pub id: Id,
    /// What was asked.
    pub title: String,
    /// The name and mention handle of who decided it, for an agent's reading.
    pub decided_by: Option<String>,
}

/// The approvals `ids`.
pub async fn approval_lines(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ApprovalLine>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.title, u.display_name || ' (@' || u.username || ')' AS decided_by FROM approvals a
           LEFT JOIN users u ON u.id = a.decided_by WHERE a.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The last thing each sub-agent said, by the name of its stream.
pub async fn last_sub_messages(
    conn: &mut Conn,
    thread: Id,
    streams: &[String],
) -> Result<HashMap<String, String>, DbError> {
    match super::thread_history::active_view(conn, thread).await? {
        Some(view) => super::thread_history::last_sub_messages(conn, thread, view, streams).await,
        None => Ok(HashMap::new()),
    }
}

/// The profile name of the agent of each segment, by segment.
pub async fn segment_agents(
    conn: &mut Conn,
    segments: &[Id],
) -> Result<HashMap<Id, String>, DbError> {
    let rows: Vec<(Id, String)> = sqlx::query_as(
        "SELECT s.id, a.name FROM segments s JOIN agents a ON a.id = s.agent_id WHERE s.id = ANY($1)",
    )
    .bind(segments)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().collect())
}

/// For each thread: its selected history's last permanent turn number and maintained activity.
pub async fn turn_totals(
    conn: &mut Conn,
    threads: &[Id],
) -> Result<HashMap<Id, (i64, Option<DateTime<Utc>>)>, DbError> {
    // Owners are one batched relation. A correlated scan of that whole relation
    // for every thread makes a large, mostly empty shared subtree quadratic.
    let sql = format!(
        "{} SELECT t.node_id,max(last.ordinal),t.last_activity_at
        FROM threads t LEFT JOIN owners o ON o.thread_id=t.node_id
        LEFT JOIN LATERAL (
          SELECT u.ordinal FROM turns u WHERE u.thread_id=o.thread_id AND u.continuation_id=o.owner
            AND u.ordinal BETWEEN o.first_turn AND o.last_turn ORDER BY u.ordinal DESC LIMIT 1
        ) last ON true WHERE t.node_id=ANY($1)
        GROUP BY t.node_id,t.last_activity_at",
        super::thread_continuations::reading::SELECTED_OWNERS
    );
    let rows: Vec<(Id, Option<i32>, Option<DateTime<Utc>>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(threads)
            .fetch_all(conn)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(thread, turns, at)| (thread, (turns.map_or(0, i64::from), at)))
        .collect())
}

/// The root org with this short name.
pub async fn root_org_named(conn: &mut Conn, slug: &str) -> Result<Option<PlaceRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PLACE_SELECT} WHERE n.parent_id IS NULL AND n.deleted_at IS NULL AND lower(o.slug) = lower($1)"
    )))
    .bind(slug)
    .fetch_optional(conn)
    .await?)
}

/// Where a chat is read from: the node of a node's chat, the root org of a direct or group chat. Nothing for an
/// identifier that is no chat.
pub async fn chat_home(conn: &mut Conn, chat: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT CASE WHEN kind = 'node' THEN node_id ELSE org_id END FROM chats WHERE id = $1",
    )
    .bind(chat)
    .fetch_optional(conn)
    .await?)
}

// ---------------------------------------------------------------- a chat, by the numbers of its messages

/// How many messages a chat holds, those deleted and the lines the system wrote among them: its last number.
pub async fn chat_total(conn: &mut Conn, chat: Id) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id = $1")
            .bind(chat)
            .fetch_one(conn)
            .await?,
    )
}

/// A message with its number in the chat.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct NumberedMessage {
    /// The message.
    #[sqlx(flatten)]
    pub row: crate::queries::chats::MessageRow,
    /// Its number: its place among all the chat's messages, counted from 1 in the order they were written.
    pub number: i64,
}

/// The messages numbered `from` to `to`, both counted in, oldest first.
pub async fn chat_messages_numbered(
    conn: &mut Conn,
    chat: Id,
    from: i64,
    to: i64,
) -> Result<Vec<NumberedMessage>, DbError> {
    Ok(sqlx::query_as(
        "SELECT * FROM (SELECT m.*, row_number() OVER (ORDER BY m.seq_created) AS number
                          FROM chat_messages m WHERE m.chat_id = $1) numbered
          WHERE numbered.number BETWEEN $2 AND $3 ORDER BY numbered.number",
    )
    .bind(chat)
    .bind(from)
    .bind(to)
    .fetch_all(conn)
    .await?)
}

/// The numbers of these messages of a chat.
pub async fn chat_numbers(
    conn: &mut Conn,
    chat: Id,
    ids: &[Id],
) -> Result<HashMap<Id, i64>, DbError> {
    let rows: Vec<(Id, i64)> = sqlx::query_as(
        "SELECT id, number FROM (SELECT m.id, row_number() OVER (ORDER BY m.seq_created) AS number
                                   FROM chat_messages m WHERE m.chat_id = $1) numbered
          WHERE id = ANY($2)",
    )
    .bind(chat)
    .bind(ids)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The highest written main-stream sequence through this turn number: empty
/// turns and sub-agent/reasoning rows do not move a written window's boundary.
pub async fn turn_high(conn: &mut Conn, thread: Id, ordinal: i64) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT max(i.seq_created) FROM items i JOIN turns t ON t.id = i.turn_id
          WHERE t.thread_id = $1 AND t.ordinal <= $2
            AND i.stream = 'main' AND i.kind <> 'reasoning'",
    )
    .bind(thread)
    .bind(ordinal)
    .fetch_one(conn)
    .await?)
}

// ---------------------------------------------------------------- the earlier versions of a document

/// An earlier version of a document, as the tree lists it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct VersionLine {
    /// Its number.
    pub version: i32,
    /// The size of the text then.
    pub bytes: i64,
    /// When its last change was made: the moment it is named by.
    pub to_at: DateTime<Utc>,
    /// How many top-level blocks it holds.
    pub blocks: i64,
}

/// The versions of a document, newest first.
pub async fn versions_of(
    conn: &mut Conn,
    document: Id,
    limit: i64,
) -> Result<Vec<VersionLine>, DbError> {
    Ok(sqlx::query_as(
        "SELECT v.version, v.bytes, v.to_at, v.top_blocks AS blocks
           FROM document_versions v WHERE v.document_id = $1 ORDER BY v.version DESC LIMIT $2",
    )
    .bind(document)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- looking through the text the database holds

/// Escapes what `LIKE` takes for a pattern, so that a text is looked for as it is.
#[must_use]
pub fn like_escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('%');
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// Reads bounded openings only for the already-authorized thread/document identifiers supplied by the caller.
pub async fn entry_openings(
    conn: &mut Conn,
    threads: &[Id],
    documents: &[Id],
    chars: i32,
) -> Result<HashMap<Id, String>, DbError> {
    let mut out: Vec<(Id, String)> = Vec::new();
    if !threads.is_empty() {
        out.extend(
            sqlx::query_as::<_, (Id, String)>(
                "SELECT owner.id, opening.text FROM unnest($1::uuid[]) AS owner(id)
             CROSS JOIN LATERAL (SELECT left(i.search_text,$2) AS text FROM items i
               WHERE i.thread_id=owner.id AND i.stream='main' AND i.kind<>'reasoning'
                 AND i.search_text IS NOT NULL AND btrim(i.search_text)<>''
               ORDER BY i.seq_created,i.id LIMIT 1) opening",
            )
            .bind(threads)
            .bind(chars)
            .fetch_all(&mut *conn)
            .await?,
        );
    }
    if !documents.is_empty() {
        out.extend(sqlx::query_as::<_, (Id, String)>(
            "SELECT owner.id, opening.text FROM unnest($1::uuid[]) AS owner(id)
             CROSS JOIN LATERAL (SELECT left(b.text,$2) AS text FROM document_blocks b
               WHERE b.document_id=owner.id AND b.parent_id IS NULL AND b.removed_at IS NULL AND btrim(b.text)<>''
               ORDER BY b.position,b.id LIMIT 1) opening")
            .bind(documents).bind(chars).fetch_all(&mut *conn).await?);
    }
    Ok(out.into_iter().collect())
}

/// A text that may hold what is looked for, with the entry it is in and its place there.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct TextRow {
    /// The row: an item, a message or a block.
    pub id: Id,
    /// The thread, the chat or the document it is in.
    pub owner: Id,
    /// Its number in that: a turn's, a message's; none for a block, which is placed by its ancestors.
    pub position: Option<i64>,
    /// The block it is under, for a block.
    pub parent: Option<Id>,
    /// The change time used with the identifier for newest-first continuation.
    pub at: DateTime<Utc>,
    /// The text.
    pub text: String,
    /// Whether it is a heading of a document, which ranks above the rest of its text.
    pub heading: bool,
}

/// The main-stream, non-reasoning items matching every literal pattern, optionally restricted to indexed parts.
pub async fn search_items(
    conn: &mut Conn,
    threads: &[Id],
    patterns: &[String],
    before: Option<(DateTime<Utc>, Id)>,
    limit: i64,
    selected: Option<&[Id]>,
) -> Result<Vec<TextRow>, DbError> {
    let predicate = super::search::matching_sql("i.search_text", "$2");
    let chosen = if selected.is_some() {
        "i.id=ANY($6)"
    } else {
        "$6::uuid[] IS NULL"
    };
    let sql = format!("SELECT i.id, i.thread_id AS owner, t.ordinal::bigint AS position, NULL::uuid AS parent, i.updated_at AS at, i.search_text AS text,
                false AS heading
           FROM items i LEFT JOIN turns t ON t.id = i.turn_id
          WHERE i.thread_id = ANY($1) AND i.stream = 'main' AND i.kind <> 'reasoning'
            AND i.search_text IS NOT NULL
            AND {predicate} AND {chosen}
            AND ($3::timestamptz IS NULL OR (i.updated_at,i.id)<($3,$4))
          ORDER BY i.updated_at DESC,i.id DESC LIMIT $5");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(threads)
        .bind(patterns)
        .bind(before.map(|(at, _)| at))
        .bind(before.map(|(_, id)| id))
        .bind(limit)
        .bind(selected)
        .fetch_all(conn)
        .await?)
}

/// Live chat messages matching every literal pattern, optionally restricted to indexed parts, with native sequence.
pub async fn search_messages(
    conn: &mut Conn,
    chats: &[Id],
    patterns: &[String],
    before: Option<(DateTime<Utc>, Id)>,
    limit: i64,
    selected: Option<&[Id]>,
) -> Result<Vec<TextRow>, DbError> {
    let predicate = super::search::matching_sql("m.search_text", "$2");
    let chosen = if selected.is_some() {
        "m.id=ANY($6)"
    } else {
        "$6::uuid[] IS NULL"
    };
    // The engine translates only the bounded first displayed positions; do not rank every message in a chat.
    let sql = format!(
        "SELECT m.id, m.chat_id AS owner,
           m.seq_created AS position,
           NULL::uuid AS parent, m.created_at AS at, m.search_text AS text, false AS heading
          FROM chat_messages m
         WHERE m.chat_id=ANY($1) AND m.deleted_at IS NULL AND m.search_text IS NOT NULL
           AND {predicate} AND {chosen} AND ($3::timestamptz IS NULL OR (m.created_at,m.id)<($3,$4))
         ORDER BY m.created_at DESC,m.id DESC LIMIT $5"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(chats)
        .bind(patterns)
        .bind(before.map(|(at, _)| at))
        .bind(before.map(|(_, id)| id))
        .bind(limit)
        .bind(selected)
        .fetch_all(conn)
        .await?)
}

/// Native numbers for just the selected first matching messages, including removed messages in their numbering.
pub async fn search_chat_numbers(
    conn: &mut Conn,
    messages: &[Id],
) -> Result<HashMap<Id, i64>, DbError> {
    if messages.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Id, i64)> = sqlx::query_as(
        "SELECT m.id,(SELECT count(*) FROM chat_messages earlier WHERE earlier.chat_id=m.chat_id AND earlier.seq_created<=m.seq_created)
           FROM chat_messages m WHERE m.id=ANY($1)")
        .bind(messages).fetch_all(conn).await?;
    Ok(rows.into_iter().collect())
}

/// The live blocks of these documents matching every literal pattern, optionally restricted to indexed parts.
pub async fn search_blocks(
    conn: &mut Conn,
    documents: &[Id],
    patterns: &[String],
    before: Option<(DateTime<Utc>, Id)>,
    limit: i64,
    selected: Option<&[Id]>,
) -> Result<Vec<TextRow>, DbError> {
    let predicate = super::search::matching_sql("b.text", "$2");
    let chosen = if selected.is_some() {
        "b.id=ANY($6)"
    } else {
        "$6::uuid[] IS NULL"
    };
    let sql = format!("SELECT b.id, b.document_id AS owner, NULL::bigint AS position, b.parent_id AS parent, b.updated_at AS at, b.text,
                (b.type = 'heading') AS heading
           FROM document_blocks b
          WHERE b.document_id = ANY($1) AND b.removed_at IS NULL AND b.text <> ''
            AND {predicate} AND {chosen}
            AND ($3::timestamptz IS NULL OR (b.updated_at,b.id)<($3,$4))
          ORDER BY b.updated_at DESC,b.id DESC LIMIT $5");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(documents)
        .bind(patterns)
        .bind(before.map(|(at, _)| at))
        .bind(before.map(|(_, id)| id))
        .bind(limit)
        .bind(selected)
        .fetch_all(conn)
        .await?)
}

/// The top-level block each of these blocks lies under (itself, for a top-level one), and the place of that block
/// among the top-level blocks of its document, counted from 1.
pub async fn block_places(
    conn: &mut Conn,
    documents: &[Id],
) -> Result<Vec<(Id, Id, Option<Id>, i64)>, DbError> {
    // (document, block, parent, number among the top-level blocks when it is one)
    Ok(sqlx::query_as(
        "SELECT b.document_id, b.id, b.parent_id,
                CASE WHEN b.parent_id IS NULL
                     THEN row_number() OVER (PARTITION BY b.document_id, (b.parent_id IS NULL)
                                             ORDER BY b.position, b.id)
                     ELSE 0 END AS number
           FROM document_blocks b
          WHERE b.document_id = ANY($1) AND b.removed_at IS NULL",
    )
    .bind(documents)
    .fetch_all(conn)
    .await?)
}

/// The names of the files documents hold: document, name, size, whether its bytes are text.
pub async fn document_file_names(
    conn: &mut Conn,
    documents: &[Id],
) -> Result<Vec<(Id, String, i64, bool)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT document_id, name, size, is_text FROM document_files
          WHERE document_id = ANY($1) ORDER BY document_id, name",
    )
    .bind(documents)
    .fetch_all(conn)
    .await?)
}

/// How many blocks of a document were removed after the sequence `seq`.
pub async fn blocks_removed_since(conn: &mut Conn, document: Id, seq: i64) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM document_blocks
          WHERE document_id = $1 AND removed_at IS NOT NULL AND seq_updated > $2",
    )
    .bind(document)
    .bind(seq)
    .fetch_one(conn)
    .await?)
}

// ---------------------------------------------------------------- copies

/// A copy given to one agent, as it is stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CopyRow {
    /// Whether its verified delivered index is first-reading material.
    pub read_first: bool,
    /// Its identifier.
    pub id: Id,
    /// The thread whose agent receives it.
    pub thread_id: Id,
    /// What it is of: `node`, `chat` or `document`.
    pub object_kind: String,
    /// Which one.
    pub object_id: Id,
    /// For a node: `timeline`, `chat`, `documents`.
    pub include: Vec<String>,
    /// Where it is written, relative to the thread's directory.
    pub path: String,
    /// How many files it holds.
    pub file_count: i32,
    /// Their size in bytes.
    pub total_bytes: i64,
    /// The moment it is a copy of.
    pub as_of: DateTime<Utc>,
    /// `pending`, `delivered` or `revoked`.
    pub state: String,
    /// Who made it.
    pub created_by: Id,
    /// When.
    pub created_at: DateTime<Utc>,
    /// When it was written into the workspace.
    pub delivered_at: Option<DateTime<Utc>>,
}

const COPY_COLUMNS: &str =
    "id, read_first, thread_id, object_kind, object_id, include, path, file_count, total_bytes, as_of,
    state, created_by, created_at, delivered_at";

/// A copy that is made.
#[derive(Debug, Clone)]
pub struct NewCopy<'a> {
    /// Whether included document material is offered for first reading.
    pub read_first: bool,
    /// Its identifier.
    pub id: Id,
    /// The thread that receives it.
    pub thread: Id,
    /// What it is of.
    pub object_kind: &'a str,
    /// Which one.
    pub object_id: Id,
    /// The areas it holds.
    pub include: &'a [String],
    /// Where it is written.
    pub path: &'a str,
    /// The hash of the list of its files, once they are kept; it names a blob that the store holds.
    pub manifest: Option<&'a [u8]>,
    /// How many files.
    pub file_count: i32,
    /// Their size.
    pub total_bytes: i64,
    /// The moment it shows.
    pub as_of: DateTime<Utc>,
    /// Who makes it.
    pub by: Id,
}

/// Records a copy, waiting for the thread's next turn.
pub async fn insert_copy(conn: &mut Conn, new: &NewCopy<'_>) -> Result<CopyRow, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO agent_shares (id, thread_id, object_kind, object_id, include, path, manifest_blob, file_count,
                total_bytes, as_of, created_by, read_first)
         VALUES ($1, $2, $3, $4, $5, $6, $11, $7, $8, $9, $10, $12) RETURNING {COPY_COLUMNS}"
    )))
    .bind(new.id)
    .bind(new.thread)
    .bind(new.object_kind)
    .bind(new.object_id)
    .bind(new.include)
    .bind(new.path)
    .bind(new.file_count)
    .bind(new.total_bytes)
    .bind(new.as_of)
    .bind(new.by)
    .bind(new.manifest)
    .bind(new.read_first)
    .fetch_one(conn)
    .await?)
}

/// Changes only the hint on the actual current document-reading entries of a guarded live share.
pub async fn read_first(conn: &mut Conn, entries: &[Id], value: bool) -> Result<(), DbError> {
    sqlx::query("UPDATE permission_entries SET read_first=$2, revision=revision+1 WHERE id=ANY($1) AND holder_kind='agents' AND permission='doc.view' AND removed_at IS NULL AND read_first IS DISTINCT FROM $2")
        .bind(entries).bind(value).execute(conn).await?;
    Ok(())
}

/// The copy `id`.
pub async fn copy(conn: &mut Conn, id: Id) -> Result<Option<CopyRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COPY_COLUMNS} FROM agent_shares WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The copies of a thing that were not taken back.
pub async fn copies_of(
    conn: &mut Conn,
    object_kind: &str,
    object: Id,
) -> Result<Vec<CopyRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COPY_COLUMNS} FROM agent_shares
          WHERE object_kind = $1 AND object_id = $2 AND state <> 'revoked' ORDER BY created_at, id"
    )))
    .bind(object_kind)
    .bind(object)
    .fetch_all(conn)
    .await?)
}

/// The copies given to these threads that were not taken back.
pub async fn copies_to(conn: &mut Conn, threads: &[Id]) -> Result<Vec<CopyRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COPY_COLUMNS} FROM agent_shares
          WHERE thread_id = ANY($1) AND state <> 'revoked' ORDER BY created_at, id"
    )))
    .bind(threads)
    .fetch_all(conn)
    .await?)
}

/// Takes a copy back. False where it is none, or was taken back already.
pub async fn revoke_copy(
    conn: &mut Conn,
    id: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE agent_shares SET state = 'revoked', revoked_by = $2, revoked_at = $3
          WHERE id = $1 AND state <> 'revoked'",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Where the files of a copy are: the list of them in the store, and the environment they were written
/// into once the copy was delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyParcel {
    /// The copy.
    pub id: Id,
    /// Where it is written, relative to the thread's directory.
    pub path: String,
    /// The list of its files, by the hash of the blob that holds it; none where the copy was made without one.
    pub manifest: Option<orch_types::Hash>,
    /// The environment it was written into; none while it waits for the thread's next turn.
    pub environment_id: Option<Id>,
    /// `pending`, `delivered` or `revoked`.
    pub state: String,
}

/// A copy's parcel as a statement reads it: identifier, path, the list's hash, environment, state.
type RawCopyParcel = (Id, String, Option<Vec<u8>>, Option<Id>, String);

fn copy_parcel(raw: RawCopyParcel) -> Result<CopyParcel, DbError> {
    let (id, path, manifest, environment_id, state) = raw;
    let manifest = manifest
        .map(|bytes| {
            orch_types::Hash::from_slice(&bytes).map_err(|_| {
                DbError::Other(sqlx::Error::Decode("a hash that is not 32 bytes".into()))
            })
        })
        .transpose()?;
    Ok(CopyParcel {
        id,
        path,
        manifest,
        environment_id,
        state,
    })
}

/// The files of the copy `id`, wherever it stands.
pub async fn copy_parcel_of(conn: &mut Conn, id: Id) -> Result<Option<CopyParcel>, DbError> {
    let raw: Option<RawCopyParcel> = sqlx::query_as(
        "SELECT id, path, manifest_blob, environment_id, state FROM agent_shares WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?;
    raw.map(copy_parcel).transpose()
}

/// The copies given to `thread` that wait to be written into its workspace, oldest first.
pub async fn copies_waiting(conn: &mut Conn, thread: Id) -> Result<Vec<CopyParcel>, DbError> {
    let raw: Vec<RawCopyParcel> = sqlx::query_as(
        "SELECT id, path, manifest_blob, environment_id, state FROM agent_shares
          WHERE thread_id = $1 AND state = 'pending' ORDER BY created_at, id",
    )
    .bind(thread)
    .fetch_all(conn)
    .await?;
    raw.into_iter().map(copy_parcel).collect()
}

/// Records that a copy waiting for its thread was written into `environment`. False where it was taken back
/// meanwhile, or written already.
pub async fn copy_delivered(
    conn: &mut Conn,
    id: Id,
    environment: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE agent_shares SET state = 'delivered', delivered_at = $3, environment_id = $2
          WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(environment)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The topic a thread lies in and the agent behind it: whether the harness can be given tools.
pub async fn harness_takes_tools(conn: &mut Conn, thread: Id) -> Result<Option<bool>, DbError> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT a.descriptor->'capabilities'->>'orch_tools' FROM threads t
           JOIN agents a ON a.id = t.agent_id WHERE t.node_id = $1",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(word,)| word.as_deref() == Some("mcp")))
}

// ---------------------------------------------------------------- documents an agent may bring back, and what goes with one

/// The documents written beside any of `nodes` that were deleted and not yet purged, oldest first: what an agent
/// that may bring documents back is listed beside the ones that are there.
pub async fn deleted_documents_beside(conn: &mut Conn, nodes: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT d.id FROM documents d WHERE d.node_id = ANY($1) AND d.deleted_at IS NOT NULL
          ORDER BY d.created_at, d.id",
    )
    .bind(nodes)
    .fetch_all(conn)
    .await?)
}

/// What goes with a document besides its text when it is deleted or moved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WhatGoesWith {
    /// The files it holds.
    pub files: i64,
    /// The earlier versions that are kept.
    pub versions: i64,
}

/// How many files each of the documents holds and how many earlier versions are kept of it: two counts for any
/// number of documents in one statement. A document that is none is not among them.
pub async fn what_goes_with(
    conn: &mut Conn,
    documents: &[Id],
) -> Result<HashMap<Id, WhatGoesWith>, DbError> {
    let rows: Vec<(Id, i64, i64)> = sqlx::query_as(
        "SELECT d.id,
                (SELECT count(*) FROM document_files f WHERE f.document_id = d.id),
                (SELECT count(*) FROM document_versions v WHERE v.document_id = d.id)
           FROM documents d WHERE d.id = ANY($1)",
    )
    .bind(documents)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, files, versions)| (id, WhatGoesWith { files, versions }))
        .collect())
}
