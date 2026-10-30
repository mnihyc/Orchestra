//! What an Orch package says of the things in it besides their records: who made each place, folder and document and when, what a document is about, where it lies, and who the people are that a package names, with the address of those whose address its maker may read.
//!
//! These are read when an archive is written as a package, in batches of the identifiers the archive holds. Nothing here
//! decides who may see what: the archive holds only what its person may read, and the callers pass identifiers that
//! the access engine has already let through.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// What a place says of itself beyond what the layout has: who made it and what it is about.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct NodeMeta {
    /// Its identifier.
    pub id: Id,
    /// `org`, `project`, `topic` or `thread`.
    pub kind: String,
    /// The node it lies in.
    pub parent_id: Option<Id>,
    /// Its name.
    pub name: String,
    /// What it is about.
    pub description: String,
    /// Who made it.
    pub created_by: Id,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
}

/// The places `ids`, as stored, deleted or not.
pub async fn node_meta(conn: &mut Conn, ids: &[Id]) -> Result<Vec<NodeMeta>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, parent_id, name, description, created_by, created_at, updated_at
           FROM nodes WHERE id = ANY($1) ORDER BY created_at, id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// What a document says of itself beyond its blocks.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DocumentMeta {
    /// Its identifier.
    pub id: Id,
    /// The node it is written beside.
    pub node_id: Id,
    /// The folder it lies in, if it lies in one.
    pub folder_id: Option<Id>,
    /// Its title.
    pub title: String,
    /// What it is about, as it says.
    pub description: String,
    /// Its maker, if they are still known.
    pub created_by: Option<Id>,
    /// Its first owner.
    pub owner_id: Id,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
    /// How many comments it holds.
    pub comments: i64,
}

/// The documents `ids` that were not deleted.
pub async fn document_meta(conn: &mut Conn, ids: &[Id]) -> Result<Vec<DocumentMeta>, DbError> {
    Ok(sqlx::query_as(
        "SELECT d.id, d.node_id, d.parent_container_id AS folder_id, d.title, d.description,
                d.created_by, d.owner_id, d.created_at, d.updated_at,
                (SELECT count(*) FROM document_comments c
                  WHERE c.document_id = d.id AND c.deleted_at IS NULL)::bigint AS comments
           FROM documents d WHERE d.id = ANY($1) AND d.deleted_at IS NULL
          ORDER BY d.created_at, d.id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A document folder, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct FolderMeta {
    /// Its identifier.
    pub id: Id,
    /// The node it lies beside.
    pub node_id: Id,
    /// The folder it lies in.
    pub parent_id: Option<Id>,
    /// Its name.
    pub name: String,
    /// Its maker, if they are still known.
    pub created_by: Option<Id>,
    /// Its first owner.
    pub owner_id: Id,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
}

/// The folders `ids` that were not deleted, and the folders they lie in, up to the top.
pub async fn folders_with_ancestors(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<Vec<FolderMeta>, DbError> {
    Ok(sqlx::query_as(
        "WITH RECURSIVE up AS (
            SELECT c.* FROM document_containers c WHERE c.id = ANY($1) AND c.deleted_at IS NULL
          UNION
            SELECT p.* FROM document_containers p JOIN up ON p.id = up.parent_container_id
             WHERE p.deleted_at IS NULL)
         SELECT id, node_id, parent_container_id AS parent_id, name, created_by, owner_id, created_at, updated_at
           FROM up ORDER BY created_at, id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A chat of a place or a thread, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ChatMeta {
    /// Its identifier.
    pub id: Id,
    /// The node it belongs to.
    pub node_id: Id,
    /// Who made it.
    pub created_by: Option<Id>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// The time of its newest message.
    pub last_message_at: Option<DateTime<Utc>>,
    /// How many messages it holds that were not deleted.
    pub messages: i64,
}

/// The chats `ids` of nodes.
pub async fn chat_meta(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ChatMeta>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id, c.node_id, c.created_by, c.created_at, c.last_message_at,
                (SELECT count(*) FROM chat_messages m WHERE m.chat_id = c.id AND m.deleted_at IS NULL)::bigint AS messages
           FROM chats c WHERE c.id = ANY($1) AND c.kind = 'node' ORDER BY c.created_at, c.id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A person or an identity a package names.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PersonMeta {
    /// Its identifier.
    pub id: Id,
    /// `human` or `service`.
    pub kind: String,
    /// The username.
    pub username: String,
    /// The name it is shown by.
    pub display_name: String,
    /// The address, as the person gave it; only read where the caller may know it.
    pub email: Option<String>,
    /// Whether the person confirmed the address.
    pub email_verified: bool,
}

/// The people `ids`, with their address; whether the caller may pass it on is the caller's to decide.
pub async fn people_meta(conn: &mut Conn, ids: &[Id]) -> Result<Vec<PersonMeta>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, username, display_name, email, (email_verified_at IS NOT NULL) AS email_verified
           FROM users WHERE id = ANY($1) ORDER BY username, id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The nodes `ids` that exist and were not deleted, with their kind and parent: what an import checks a place
/// against before it makes anything inside it.
pub async fn live_nodes(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<Vec<(Id, String, Option<Id>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, parent_id FROM nodes WHERE id = ANY($1) AND deleted_at IS NULL",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A person named in the page of a thing: what one entry gives.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct SharedWith {
    /// The thing whose page it is on.
    pub object_id: Id,
    /// The person.
    pub holder_id: Id,
    /// The permission it gives, by its key.
    pub permission: String,
    /// When it ends by itself.
    pub expires_at: Option<DateTime<Utc>>,
    /// Why it was written.
    pub note: String,
}

/// The people named by an entry of the live pages of the things `ids` of the kind `object_kind` (`node`, `document` or
/// `document_container`), apart from the Owners row, which a thing's maker always holds, and from what ended by `now`.
pub async fn shared_with(
    conn: &mut Conn,
    object_kind: &str,
    ids: &[Id],
    now: DateTime<Utc>,
) -> Result<Vec<SharedWith>, DbError> {
    Ok(sqlx::query_as(
        "SELECT object_id, holder_id, permission, expires_at, note
           FROM permission_entries
          WHERE object_kind = $1 AND object_id = ANY($2) AND page = 'live' AND removed_at IS NULL
            AND holder_kind = 'user' AND holder_id IS NOT NULL AND permission <> 'access.own'
            AND (expires_at IS NULL OR expires_at > $3)
          ORDER BY object_id, permission, holder_id",
    )
    .bind(object_kind)
    .bind(ids)
    .bind(now)
    .fetch_all(conn)
    .await?)
}
