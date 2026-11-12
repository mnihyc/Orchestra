//! Existing upload custody and partial transfers, with actual retained references.
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id};

/// A native inventory row, before names and administrative controls are projected.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, sqlx::FromRow)]
pub struct Row {
    /// Stable source key, never a filesystem path.
    pub key: String,
    /// completed, session or machine.
    pub kind: String,
    /// The actual immutable-custody identity.
    pub content_id: Option<Id>,
    /// An unfinished browser session.
    pub session_id: Option<Id>,
    /// Raw content identity where known.
    pub hash: Option<Vec<u8>>,
    /// Recorded sender; legacy unknown senders remain absent.
    pub user_id: Option<Id>,
    /// Actual sending machine, not its owner.
    pub machine_id: Option<Id>,
    /// Supplied original filename, where known.
    pub name: Option<String>,
    /// Supplied media type, where known.
    pub media_type: Option<String>,
    /// Declared or verified bytes.
    pub size: i64,
    /// Accepted prefix or verified size.
    pub received: i64,
    /// Original source time.
    pub created_at: DateTime<Utc>,
    /// Last recorded progress time.
    pub updated_at: DateTime<Utc>,
    /// Existing browser completion deadline.
    pub expires_at: Option<DateTime<Utc>>,
}
impl Row {
    /// Decode only a stored byte identity; a partial browser upload has none yet.
    pub fn blob(&self) -> Result<Option<Hash>, DbError> {
        self.hash
            .as_deref()
            .map(|raw| {
                Hash::from_slice(raw).map_err(|_| {
                    DbError::Other(sqlx::Error::Decode("an upload hash is not 32 bytes".into()))
                })
            })
            .transpose()
    }
}

const ROWS: &str = "
SELECT 'completed:'||c.id::text AS key,'completed'::text AS kind,c.id AS content_id,
  NULL::uuid AS session_id,c.hash,c.user_id,NULL::uuid AS machine_id,c.name,c.media_type,
  b.size,b.size AS received,c.created_at,c.created_at AS updated_at,NULL::timestamptz AS expires_at
 FROM completed_uploads c JOIN blobs b ON b.hash=c.hash
UNION ALL
SELECT 'upload:'||s.id::text,'session',NULL::uuid,s.id,NULL::bytea,s.user_id,NULL::uuid,
 s.name,s.media_type,s.size,s.received,s.created_at,s.updated_at,s.expires_at
 FROM upload_sessions s WHERE s.cancelled_at IS NULL AND s.completed_at IS NULL
UNION ALL
SELECT 'machine-upload:'||encode(u.hash,'hex'),'machine',NULL::uuid,NULL::uuid,u.hash,u.user_id,u.machine_id,
 NULL::text,NULL::text,u.size,u.received,u.started_at,u.updated_at,NULL::timestamptz
 FROM blob_uploads u";

/// Native source order shared by the personal cursor and system inventory.
pub async fn list(
    conn: &mut Conn,
    owner: Option<Id>,
    after: Option<(DateTime<Utc>, String)>,
    take: Option<i64>,
) -> Result<Vec<Row>, DbError> {
    let (at, key) = after
        .map(|(at, key)| (Some(at), Some(key)))
        .unwrap_or_default();
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH inventory AS ({ROWS}) SELECT * FROM inventory
         WHERE ($1::uuid IS NULL OR user_id=$1)
           AND ($2::timestamptz IS NULL OR (created_at,key)<($2,$3::text))
         ORDER BY created_at DESC,key DESC LIMIT $4"
    )))
    .bind(owner)
    .bind(at)
    .bind(key)
    .bind(take)
    .fetch_all(conn)
    .await?)
}

/// One actual completed custody row; callers establish its native read authority separately.
pub async fn completed(conn: &mut Conn, id: Id) -> Result<Option<Row>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH inventory AS ({ROWS}) SELECT * FROM inventory WHERE content_id=$1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

// $2 restricts personal attachment discovery to recorded authorship/custody. Other people's
// same-hash attachments cannot hide this person's unused upload or disclose their activity.
const REFERENCES: &str = "
SELECT 'thread'::text AS kind,thread_id AS id FROM thread_attachment_uploads
 WHERE blob=$1 AND ($2::uuid IS NULL OR created_by=$2)
UNION SELECT 'thread',i.thread_id FROM thread_input_blob_refs r JOIN thread_inputs i ON i.id=r.input_id
 WHERE r.blob=$1 AND ($2::uuid IS NULL OR i.author_id=$2)
UNION SELECT 'thread_draft',thread_id FROM drafts
 WHERE ($2::uuid IS NULL OR user_id=$2) AND (content @> $3 OR content @> $4 OR content @> $5)
UNION SELECT 'thread_draft',thread_id FROM thread_draft_blob_refs WHERE blob=$1 AND ($2::uuid IS NULL OR user_id=$2)
UNION SELECT 'chat',chat_id FROM chat_messages
 WHERE deleted_at IS NULL AND ($2::uuid IS NULL OR author_id=$2) AND (content @> $3 OR content @> $4 OR content @> $5)
UNION SELECT 'chat_draft',chat_id FROM chat_states
 WHERE ($2::uuid IS NULL OR user_id=$2) AND (draft @> $3 OR draft @> $4 OR draft @> $5)
UNION SELECT 'document',document_id FROM document_files WHERE blob=$1 AND ($2::uuid IS NULL OR added_by=$2)
UNION SELECT 'skill',skill_id FROM skill_draft_files WHERE blob=$1 AND ($2::uuid IS NULL OR updated_by=$2)
UNION SELECT 'skill_version',id FROM skill_versions WHERE archive=$1 AND ($2::uuid IS NULL OR created_by=$2)
UNION SELECT 'snapshot',s.id FROM snapshots s LEFT JOIN snapshot_blob_refs r ON r.snapshot_id=s.id
 WHERE (r.blob=$1 OR s.manifest_blob=$1) AND ($2::uuid IS NULL OR s.created_by=$2)
UNION SELECT 'export',id FROM exports WHERE blob=encode($1,'hex') AND state='ready' AND ($2::uuid IS NULL OR user_id=$2)
UNION SELECT 'import',id FROM imports WHERE encode($1,'hex')=ANY(source_blobs) AND ($2::uuid IS NULL OR user_id=$2)
UNION SELECT 'input_schedule',s.id FROM input_schedule_blob_refs r JOIN input_schedules s ON s.id=r.schedule_id
 WHERE r.blob=$1 AND ($2::uuid IS NULL OR s.author_id=$2)
UNION SELECT 'environment',environment_id FROM file_cache WHERE hash=$1 AND $2::uuid IS NULL
UNION SELECT 'thread',i.thread_id FROM thread_item_blob_refs r JOIN items i ON i.id=r.item_id
 WHERE r.blob=$1 AND ($2::uuid IS NULL OR i.author_id=$2)
UNION SELECT 'thread',i.thread_id FROM thread_item_revision_blob_refs r JOIN items i ON i.id=r.item_id
 WHERE r.blob=$1 AND ($2::uuid IS NULL OR i.author_id=$2)
UNION SELECT 'history_cut',cut_id FROM thread_history_cut_blobs WHERE blob=$1 AND $2::uuid IS NULL
UNION SELECT 'command',command_id FROM thread_tool_command_blobs WHERE blob=$1 AND $2::uuid IS NULL
UNION SELECT 'turn',id FROM turns WHERE end_manifest_blob=$1 AND $2::uuid IS NULL
UNION SELECT 'agent_share',id FROM agent_shares WHERE manifest_blob=$1 AND $2::uuid IS NULL
UNION SELECT 'terminal',id FROM terminal_sessions WHERE recording_blob=$1 AND $2::uuid IS NULL";

/// Actual distinct attached-to identities and their full count, with a bounded preview.
pub async fn attachments(
    conn: &mut Conn,
    hash: &Hash,
    owner: Option<Id>,
    take: i64,
) -> Result<(i64, Vec<(String, Id)>), DbError> {
    let raw = hash.to_string();
    // Both list producers use PageQuery's nonzero bound; keep the count observable
    // even if another trusted caller asks for a zero-length preview.
    let take = take.max(1);
    let direct = serde_json::json!([{"blob":raw}]);
    let images = serde_json::json!([{"images":[{"blob":raw}]}]);
    let files = serde_json::json!([{"attachments":[{"blob":raw}]}]);
    let rows:Vec<(String,Id,i64)>=sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH refs AS ({REFERENCES}) SELECT kind,id,count(*) OVER() AS total FROM refs ORDER BY kind,id LIMIT $6"
    ))).bind(hash.as_bytes().as_slice()).bind(owner).bind(direct).bind(images).bind(files).bind(take)
        .fetch_all(conn).await?;
    let total = rows.first().map_or(0, |row| row.2);
    Ok((
        total,
        rows.into_iter().map(|(kind, id, _)| (kind, id)).collect(),
    ))
}
