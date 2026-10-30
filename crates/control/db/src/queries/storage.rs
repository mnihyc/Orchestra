//! Blobs, uploads, file cache, snapshots, and shells with whether somebody is attached and the last of what each printed.
//!
//! **Blobs.** A row of `blobs` says that the store holds some content, how large it is, when it was
//! last asked for, and how many things point at it (`pin_count`): a snapshot, a manifest, a kept
//! output, an attachment. Content that nothing points at is cache, and goes after the days of its
//! grace (`unreferenced`, `forget`). The bytes are in the store; this module keeps only what is
//! known about them, and the store is the truth about whether a blob is there.
//!
//! **Uploads.** `blob_uploads` is a machine's upload in pieces that can go on where it stopped, one
//! row for each hash; `upload_sessions` is a person's, from a browser, one row for each session.
//! Both say how many bytes have arrived; the bytes themselves wait in files of the API.
//!
//! **The file cache.** `file_cache` says which version of which file of an environment has its
//! content in the store: a version is the path with the size and the time of last change. A time is
//! kept in nanoseconds as the machine read it, and compared in milliseconds, which is all that a
//! listing shows of it.
//!
//! **Snapshots.** A snapshot is a row from the moment it is asked for, `running` until the job that
//! takes it ends.
//!
//! Nothing here decides anything: who may read a snapshot or write a file is the access engine's.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id, Json};

/// How many hashes one statement pins or unpins: what a snapshot's last phase pins at a time.
pub const PIN_BATCH: usize = 1_000;

/// How many nanoseconds make a millisecond: the precision at which two times of last change are
/// the same version of a file.
pub const NANOS_PER_MILLI: i64 = 1_000_000;

/// A hash read from a column that is meant to hold 32 bytes.
fn hash_of(raw: &[u8]) -> Result<Hash, DbError> {
    Hash::from_slice(raw)
        .map_err(|_| DbError::Other(sqlx::Error::Decode("a hash that is not 32 bytes".into())))
}

fn hashes_of(raw: Vec<Vec<u8>>) -> Result<Vec<Hash>, DbError> {
    raw.iter().map(|each| hash_of(each)).collect()
}

fn raw_hashes(hashes: &[Hash]) -> Vec<Vec<u8>> {
    hashes.iter().map(|hash| hash.as_bytes().to_vec()).collect()
}

// ---- blobs ------------------------------------------------------------------------------------

/// What is known of a blob that the store holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobRow {
    /// Its hash.
    pub hash: Hash,
    /// Its size in bytes.
    pub size: i64,
    /// When it was first stored.
    pub stored_at: DateTime<Utc>,
    /// When it was last asked for.
    pub last_access_at: DateTime<Utc>,
    /// How many things point at it.
    pub pin_count: i32,
}

#[derive(sqlx::FromRow)]
struct RawBlob {
    hash: Vec<u8>,
    size: i64,
    stored_at: DateTime<Utc>,
    last_access_at: DateTime<Utc>,
    pin_count: i32,
}

impl TryFrom<RawBlob> for BlobRow {
    type Error = DbError;

    fn try_from(raw: RawBlob) -> Result<Self, DbError> {
        Ok(Self {
            hash: hash_of(&raw.hash)?,
            size: raw.size,
            stored_at: raw.stored_at,
            last_access_at: raw.last_access_at,
            pin_count: raw.pin_count,
        })
    }
}

/// Records that the store holds the blob `hash` of `size` bytes, or, when it is recorded already,
/// that it was asked for at `at`. Says whether this call made the row.
pub async fn register_blob(
    conn: &mut Conn,
    hash: &Hash,
    size: i64,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO blobs (hash, size, stored_at, last_access_at) VALUES ($1, $2, $3, $3)
         ON CONFLICT (hash) DO UPDATE SET last_access_at = GREATEST(blobs.last_access_at, EXCLUDED.last_access_at)
         RETURNING (xmax = 0)",
    )
    .bind(hash.as_bytes().as_slice())
    .bind(size)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// The row of a blob.
pub async fn blob(conn: &mut Conn, hash: &Hash) -> Result<Option<BlobRow>, DbError> {
    let raw: Option<RawBlob> = sqlx::query_as(
        "SELECT hash, size, stored_at, last_access_at, pin_count FROM blobs WHERE hash = $1",
    )
    .bind(hash.as_bytes().as_slice())
    .fetch_optional(conn)
    .await?;
    raw.map(BlobRow::try_from).transpose()
}

/// Which of `hashes` have a row.
pub async fn blobs_among(conn: &mut Conn, hashes: &[Hash]) -> Result<Vec<Hash>, DbError> {
    let raw: Vec<Vec<u8>> = sqlx::query_scalar("SELECT hash FROM blobs WHERE hash = ANY($1)")
        .bind(raw_hashes(hashes))
        .fetch_all(conn)
        .await?;
    hashes_of(raw)
}

/// Records that a blob was asked for at `at`, which starts its grace again.
pub async fn touch_blob(conn: &mut Conn, hash: &Hash, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE blobs SET last_access_at = GREATEST(last_access_at, $2) WHERE hash = $1")
        .bind(hash.as_bytes().as_slice())
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Adds one pin to each of `hashes` (each once, however often it is listed) and says how many had a
/// row to pin. A pinned blob is never evicted and never goes with the grace.
pub async fn pin_blobs(conn: &mut Conn, hashes: &[Hash]) -> Result<u64, DbError> {
    let mut pinned = 0;
    for batch in hashes.chunks(PIN_BATCH) {
        pinned += sqlx::query("UPDATE blobs SET pin_count = pin_count + 1 WHERE hash = ANY($1)")
            .bind(raw_hashes(batch))
            .execute(&mut *conn)
            .await?
            .rows_affected();
    }
    Ok(pinned)
}

/// The person's saved draft and pending attachments, locked with replacement bytes before pin changes.
pub async fn private_thread_blobs(
    conn: &mut Conn,
    user: Id,
    thread: Id,
) -> Result<Vec<Hash>, DbError> {
    let raw: Vec<Vec<u8>> = sqlx::query_scalar("SELECT blob FROM thread_draft_blob_refs WHERE user_id=$1 AND thread_id=$2
        UNION SELECT blob FROM thread_attachment_uploads WHERE created_by=$1 AND thread_id=$2 AND blob IS NOT NULL")
        .bind(user).bind(thread).fetch_all(conn).await?;
    hashes_of(raw)
}

/// Records exactly the blobs a snapshot owns, adding each association and its pin once.
pub async fn pin_snapshot(conn: &mut Conn, snapshot: Id, hashes: &[Hash]) -> Result<u64, DbError> {
    let unique: Vec<Hash> = hashes
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let mut present = 0;
    for batch in unique.chunks(PIN_BATCH) {
        sqlx::query("WITH added AS (INSERT INTO snapshot_blob_refs(snapshot_id,blob)
            SELECT $1,b.hash FROM blobs b WHERE b.hash=ANY($2) ON CONFLICT DO NOTHING RETURNING blob)
            UPDATE blobs b SET pin_count=pin_count+1 FROM added a WHERE b.hash=a.blob")
            .bind(snapshot).bind(raw_hashes(batch)).execute(&mut *conn).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM snapshot_blob_refs WHERE snapshot_id=$1 AND blob=ANY($2)",
        )
        .bind(snapshot)
        .bind(raw_hashes(batch))
        .fetch_one(&mut *conn)
        .await?;
        present += u64::try_from(count).unwrap_or_default();
    }
    Ok(present)
}

/// The exact logical pins of a snapshot, independent of whether its manifest can still be read.
pub async fn snapshot_pins(conn: &mut Conn, snapshot: Id) -> Result<Vec<Hash>, DbError> {
    let raw: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT blob FROM snapshot_blob_refs WHERE snapshot_id=$1 ORDER BY blob",
    )
    .bind(snapshot)
    .fetch_all(conn)
    .await?;
    hashes_of(raw)
}

/// Releases each logical snapshot association separately, preserving another snapshot's shared pin.
pub async fn release_snapshot_pins(
    conn: &mut Conn,
    snapshots: &[Id],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let raw: Vec<Vec<u8>> = sqlx::query_scalar(
        "DELETE FROM snapshot_blob_refs WHERE snapshot_id=ANY($1) RETURNING blob",
    )
    .bind(snapshots)
    .fetch_all(&mut *conn)
    .await?;
    for hash in hashes_of(raw)? {
        unpin_blobs(conn, &[hash], at).await?;
    }
    Ok(())
}

/// Replaces an accepted input's own attachment pins; another draft or snapshot keeps its own pins.
pub async fn input_pins(
    conn: &mut Conn,
    input: Id,
    hashes: &[Hash],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    replace_owned_pins(
        conn,
        "thread_input_blob_refs",
        "input_id",
        input,
        hashes,
        at,
    )
    .await
}

/// Replaces a frozen schedule's pins; accepted input pins are independent of future-work edits.
pub async fn schedule_pins(
    conn: &mut Conn,
    schedule: Id,
    hashes: &[Hash],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    replace_owned_pins(
        conn,
        "input_schedule_blob_refs",
        "schedule_id",
        schedule,
        hashes,
        at,
    )
    .await
}

async fn replace_owned_pins(
    conn: &mut Conn,
    table: &str,
    column: &str,
    id: Id,
    hashes: &[Hash],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let removed: Vec<Vec<u8>> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "DELETE FROM {table} WHERE {column}=$1 AND blob<>ALL($2) RETURNING blob"
    )))
    .bind(id)
    .bind(raw_hashes(hashes))
    .fetch_all(&mut *conn)
    .await?;
    for hash in hashes_of(removed)? {
        unpin_blobs(conn, &[hash], at).await?;
    }
    for batch in hashes.chunks(PIN_BATCH) {
        sqlx::query(sqlx::AssertSqlSafe(format!("WITH added AS (INSERT INTO {table}({column},blob) SELECT $1,b.hash FROM blobs b WHERE b.hash=ANY($2) ON CONFLICT DO NOTHING RETURNING blob) UPDATE blobs b SET pin_count=pin_count+1 FROM added a WHERE b.hash=a.blob")))
            .bind(id).bind(raw_hashes(batch)).execute(&mut *conn).await?;
    }
    Ok(())
}

/// Releases logical attachment owners before their parent's successful full-row collection.
pub async fn release_input_pins(
    conn: &mut Conn,
    threads: &[Id],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let raw:Vec<Vec<u8>>=sqlx::query_scalar("DELETE FROM thread_input_blob_refs WHERE input_id IN(SELECT id FROM thread_inputs WHERE thread_id=ANY($1)) RETURNING blob")
        .bind(threads).fetch_all(&mut *conn).await?;
    for hash in hashes_of(raw)? {
        unpin_blobs(conn, &[hash], at).await?;
    }
    Ok(())
}

/// Releases frozen drafts before a successfully collected topic takes their schedules with it.
pub async fn release_schedule_pins(
    conn: &mut Conn,
    topics: &[Id],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let raw:Vec<Vec<u8>>=sqlx::query_scalar("DELETE FROM input_schedule_blob_refs WHERE schedule_id IN(SELECT id FROM input_schedules WHERE topic_id=ANY($1)) RETURNING blob")
        .bind(topics).fetch_all(&mut *conn).await?;
    for hash in hashes_of(raw)? {
        unpin_blobs(conn, &[hash], at).await?;
    }
    Ok(())
}

/// Takes one pin off each of `hashes` that has one. A blob that has none left is cache again, and
/// its grace begins at `at`.
pub async fn unpin_blobs(
    conn: &mut Conn,
    hashes: &[Hash],
    at: DateTime<Utc>,
) -> Result<u64, DbError> {
    let mut unpinned = 0;
    for batch in hashes.chunks(PIN_BATCH) {
        unpinned += sqlx::query(
            "UPDATE blobs
                SET pin_count = pin_count - 1,
                    last_access_at = CASE WHEN pin_count = 1 THEN GREATEST(last_access_at, $2) ELSE last_access_at END
              WHERE hash = ANY($1) AND pin_count > 0",
        )
        .bind(raw_hashes(batch))
        .bind(at)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    }
    Ok(unpinned)
}

/// Blobs that nothing points at, no cached version of a file names, and nobody asked for since
/// `idle_since`: the ones the grace has run out for, oldest first.
pub async fn unreferenced(
    conn: &mut Conn,
    idle_since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<(Hash, i64)>, DbError> {
    let rows: Vec<(Vec<u8>, i64)> = sqlx::query_as(
        "SELECT b.hash, b.size FROM blobs b
          WHERE b.pin_count = 0 AND b.last_access_at < $1
            AND NOT EXISTS (SELECT 1 FROM file_cache f WHERE f.hash = b.hash)
            AND NOT EXISTS (SELECT 1 FROM model_inputs m CROSS JOIN LATERAL jsonb_array_elements(m.blocks) x WHERE x->>'blob'=encode(b.hash,'hex'))
          ORDER BY b.last_access_at LIMIT $2",
    )
    .bind(idle_since)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|(raw, size)| Ok((hash_of(&raw)?, size)))
        .collect()
}

/// Takes the row of a blob away if it is still one that nothing wants: the conditions are looked
/// at again in the statement that deletes, so a pin that came meanwhile keeps it. Says whether the
/// row was deleted; the store's file is then the caller's to remove.
pub async fn forget_blob(
    conn: &mut Conn,
    hash: &Hash,
    idle_since: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "DELETE FROM blobs b
          WHERE b.hash = $1 AND b.pin_count = 0 AND b.last_access_at < $2
            AND NOT EXISTS (SELECT 1 FROM file_cache f WHERE f.hash = b.hash)
            AND NOT EXISTS (SELECT 1 FROM model_inputs m CROSS JOIN LATERAL jsonb_array_elements(m.blocks) x WHERE x->>'blob'=encode(b.hash,'hex'))",
    )
    .bind(hash.as_bytes().as_slice())
    .bind(idle_since)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

// ---- uploads of machines, in pieces -----------------------------------------------------------

/// A machine's upload in pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadRow {
    /// The hash the content is to have.
    pub hash: Hash,
    /// How large the whole is.
    pub size: i64,
    /// How many bytes have arrived, from the first.
    pub received: i64,
    /// The machine that sends it.
    pub machine_id: Option<Id>,
    /// The person who sends it, for an upload that is a person's.
    pub user_id: Option<Id>,
    /// When it began.
    pub started_at: DateTime<Utc>,
    /// When a piece last arrived.
    pub updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct RawUpload {
    hash: Vec<u8>,
    size: i64,
    received: i64,
    machine_id: Option<Id>,
    user_id: Option<Id>,
    started_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<RawUpload> for UploadRow {
    type Error = DbError;

    fn try_from(raw: RawUpload) -> Result<Self, DbError> {
        Ok(Self {
            hash: hash_of(&raw.hash)?,
            size: raw.size,
            received: raw.received,
            machine_id: raw.machine_id,
            user_id: raw.user_id,
            started_at: raw.started_at,
            updated_at: raw.updated_at,
        })
    }
}

const UPLOAD_COLUMNS: &str = "hash, size, received, machine_id, user_id, started_at, updated_at";

/// The upload under way for `hash`, if there is one.
pub async fn upload(conn: &mut Conn, hash: &Hash) -> Result<Option<UploadRow>, DbError> {
    let raw: Option<RawUpload> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {UPLOAD_COLUMNS} FROM blob_uploads WHERE hash = $1"
    )))
    .bind(hash.as_bytes().as_slice())
    .fetch_optional(conn)
    .await?;
    raw.map(UploadRow::try_from).transpose()
}

/// The upload under way for `hash`, locked until the transaction ends: whoever holds it is the one
/// that adds the next piece.
pub async fn lock_upload(conn: &mut Conn, hash: &Hash) -> Result<Option<UploadRow>, DbError> {
    let raw: Option<RawUpload> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {UPLOAD_COLUMNS} FROM blob_uploads WHERE hash = $1 FOR UPDATE"
    )))
    .bind(hash.as_bytes().as_slice())
    .fetch_optional(conn)
    .await?;
    raw.map(UploadRow::try_from).transpose()
}

/// Begins the upload of `hash`, nothing received yet. Says whether this call began it: two machines
/// that send the same content at once share one upload.
pub async fn begin_upload(
    conn: &mut Conn,
    hash: &Hash,
    size: i64,
    machine: Option<Id>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO blob_uploads (hash, size, received, machine_id, started_at, updated_at)
         VALUES ($1, $2, 0, $3, $4, $4) ON CONFLICT (hash) DO NOTHING",
    )
    .bind(hash.as_bytes().as_slice())
    .bind(size)
    .bind(machine)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Records that `received` bytes of the upload of `hash` have arrived for the server-verified command. A new command
/// can continue an earlier upload; attribution changes only with an accepted piece. Standing transfers name no command.
pub async fn advance_upload(
    conn: &mut Conn,
    hash: &Hash,
    received: i64,
    command: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE blob_uploads SET received = $2, updated_at = $3, command_id = $4 WHERE hash = $1",
    )
    .bind(hash.as_bytes().as_slice())
    .bind(received)
    .bind(at)
    .bind(command)
    .execute(conn)
    .await?;
    Ok(())
}

/// The exact command's unfinished upload with `hash`, rechecked even after a watcher has found it. Another command
/// continuing those bytes no longer supplies this command's progress.
pub async fn upload_of_command(
    conn: &mut Conn,
    machine: Id,
    command: Id,
    hash: &Hash,
) -> Result<Option<UploadRow>, DbError> {
    let raw: Option<RawUpload> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {UPLOAD_COLUMNS} FROM blob_uploads WHERE hash = $1 AND machine_id = $2 AND command_id = $3"
    )))
    .bind(hash.as_bytes().as_slice())
    .bind(machine)
    .bind(command)
    .fetch_optional(conn)
    .await?;
    raw.map(UploadRow::try_from).transpose()
}

/// At most `limit` unfinished uploads attributed to this exact authenticated command. The watcher asks for two only
/// to distinguish a single archive from ambiguity; machine/time proximity is never evidence of command ownership.
/// Hash order gives a stable prefix and uses the matching index even when machine/command correlation is misestimated;
/// an unordered limit can otherwise choose a scan, expecting its matches to appear near the start of the table.
pub async fn uploads_of_command(
    conn: &mut Conn,
    machine: Id,
    command: Id,
    limit: i64,
) -> Result<Vec<UploadRow>, DbError> {
    let raw: Vec<RawUpload> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {UPLOAD_COLUMNS} FROM blob_uploads WHERE machine_id = $1 AND command_id = $2 ORDER BY hash LIMIT $3"
    )))
    .bind(machine)
    .bind(command)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    raw.into_iter().map(UploadRow::try_from).collect()
}

/// Ends the upload of `hash`: it is whole, or it was given up.
pub async fn end_upload(conn: &mut Conn, hash: &Hash) -> Result<(), DbError> {
    sqlx::query("DELETE FROM blob_uploads WHERE hash = $1")
        .bind(hash.as_bytes().as_slice())
        .execute(conn)
        .await?;
    Ok(())
}

/// The uploads of a machine to which no piece has come since `silent_since`.
pub async fn stale_uploads(
    conn: &mut Conn,
    silent_since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Hash>, DbError> {
    let raw: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT hash FROM blob_uploads WHERE updated_at < $1 ORDER BY updated_at LIMIT $2",
    )
    .bind(silent_since)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    hashes_of(raw)
}

/// The uploads of `machine` to which a piece has come since `since`, at most `limit` of them, oldest update first.
/// This reports machine activity, not command ownership; job progress uses [`uploads_of_command`].
/// An upload is gone from the table when it is whole, so these are the ones under way. Update-time order uses the
/// machine/time index even when correlated machine and time estimates would favor an unordered table scan.
pub async fn uploads_of_machine(
    conn: &mut Conn,
    machine: Id,
    since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<UploadRow>, DbError> {
    let raw: Vec<RawUpload> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {UPLOAD_COLUMNS} FROM blob_uploads
         WHERE machine_id = $1 AND updated_at >= $2 ORDER BY updated_at LIMIT $3"
    )))
    .bind(machine)
    .bind(since)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    raw.into_iter().map(UploadRow::try_from).collect()
}

/// The most bytes that arrived for an upload of a machine that is under way with exactly `size`
/// bytes and was fed lately: how far a file that is being fetched has come, as far as can be told
/// without knowing its hash.
pub async fn received_of_machine(
    conn: &mut Conn,
    machine: Id,
    size: i64,
    since: DateTime<Utc>,
) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT max(received) FROM blob_uploads WHERE machine_id = $1 AND size = $2 AND updated_at >= $3",
    )
    .bind(machine)
    .bind(size)
    .bind(since)
    .fetch_one(conn)
    .await?)
}

// ---- uploads of people, from a browser --------------------------------------------------------

/// A person's upload in pieces.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SessionRow {
    /// The session.
    pub id: Id,
    /// The person who began it.
    pub user_id: Id,
    /// The name of what is uploaded.
    pub name: String,
    /// Its type, where it was said.
    pub media_type: Option<String>,
    /// How large the whole is.
    pub size: i64,
    /// How many bytes have arrived.
    pub received: i64,
    /// When it began.
    pub created_at: DateTime<Utc>,
    /// When it ends.
    pub expires_at: DateTime<Utc>,
}

const SESSION_COLUMNS: &str =
    "id, user_id, name, media_type, size, received, created_at, expires_at";

/// Begins an upload session.
pub async fn begin_session(conn: &mut Conn, session: &SessionRow) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO upload_sessions (id, user_id, name, media_type, size, received, created_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(session.id)
    .bind(session.user_id)
    .bind(&session.name)
    .bind(&session.media_type)
    .bind(session.size)
    .bind(session.received)
    .bind(session.created_at)
    .bind(session.expires_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The session `id`, locked until the transaction ends.
pub async fn lock_session(conn: &mut Conn, id: Id) -> Result<Option<SessionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SESSION_COLUMNS} FROM upload_sessions WHERE id = $1 AND cancelled_at IS NULL AND completed_at IS NULL FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Records that `received` bytes of the session have arrived.
pub async fn advance_session(conn: &mut Conn, id: Id, received: i64) -> Result<(), DbError> {
    sqlx::query("UPDATE upload_sessions SET received = $2 WHERE id = $1")
        .bind(id)
        .bind(received)
        .execute(conn)
        .await?;
    Ok(())
}

/// Ends a session: it is whole, or it ran out.
pub async fn end_session(conn: &mut Conn, id: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM upload_sessions WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// The sessions that ran out before `now`.
pub async fn expired_sessions(
    conn: &mut Conn,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM upload_sessions WHERE expires_at < $1 ORDER BY expires_at LIMIT $2",
    )
    .bind(now)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

// ---- the file cache ---------------------------------------------------------------------------

/// A version of a file whose content the store holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedFile {
    /// Its path from the workspace root.
    pub path: String,
    /// Its size in bytes.
    pub size: i64,
    /// When it last changed, in nanoseconds since 1970.
    pub mtime_ns: i64,
    /// The hash of its content.
    pub hash: Hash,
    /// When the store got it.
    pub cached_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct RawCached {
    path: String,
    size: i64,
    mtime_ns: i64,
    hash: Vec<u8>,
    cached_at: DateTime<Utc>,
}

impl TryFrom<RawCached> for CachedFile {
    type Error = DbError;

    fn try_from(raw: RawCached) -> Result<Self, DbError> {
        Ok(Self {
            path: raw.path,
            size: raw.size,
            mtime_ns: raw.mtime_ns,
            hash: hash_of(&raw.hash)?,
            cached_at: raw.cached_at,
        })
    }
}

/// Records that the content of a version of a file is in the store. The blob has to have its row.
pub async fn record_cached(
    conn: &mut Conn,
    environment: Id,
    file: &CachedFile,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO file_cache (environment_id, path, size, mtime_ns, hash, cached_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (environment_id, path, size, mtime_ns)
         DO UPDATE SET hash = EXCLUDED.hash, cached_at = EXCLUDED.cached_at",
    )
    .bind(environment)
    .bind(&file.path)
    .bind(file.size)
    .bind(file.mtime_ns)
    .bind(file.hash.as_bytes().as_slice())
    .bind(file.cached_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The hash of the cached content of a version of a file, the time of last change being compared
/// in milliseconds.
pub async fn cached_version(
    conn: &mut Conn,
    environment: Id,
    path: &str,
    size: i64,
    mtime_ms: i64,
) -> Result<Option<Hash>, DbError> {
    let raw: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT hash FROM file_cache
          WHERE environment_id = $1 AND path = $2 AND size = $3 AND mtime_ns / $5 = $4
          ORDER BY cached_at DESC LIMIT 1",
    )
    .bind(environment)
    .bind(path)
    .bind(size)
    .bind(mtime_ms)
    .bind(NANOS_PER_MILLI)
    .fetch_optional(conn)
    .await?;
    raw.map(|raw| hash_of(&raw)).transpose()
}

/// The versions of the files `paths` that are cached, as `(path, size, time in milliseconds)`.
pub async fn cached_among(
    conn: &mut Conn,
    environment: Id,
    paths: &[String],
) -> Result<Vec<(String, i64, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT path, size, mtime_ns / $3 FROM file_cache
          WHERE environment_id = $1 AND path = ANY($2)",
    )
    .bind(environment)
    .bind(paths)
    .bind(NANOS_PER_MILLI)
    .fetch_all(conn)
    .await?)
}

/// Whether `hash` is a content that was cached for `path` of the environment, in any version.
pub async fn hash_belongs(
    conn: &mut Conn,
    environment: Id,
    path: &str,
    hash: &Hash,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM file_cache WHERE environment_id = $1 AND path = $2 AND hash = $3)",
    )
    .bind(environment)
    .bind(path)
    .bind(hash.as_bytes().as_slice())
    .fetch_one(conn)
    .await?)
}

/// The newest kept version at this exact path of an environment.
pub async fn cached_latest(
    conn: &mut Conn,
    environment: Id,
    path: &str,
) -> Result<Option<CachedFile>, DbError> {
    let raw: Option<RawCached> = sqlx::query_as("SELECT path,size,mtime_ns,hash,cached_at FROM file_cache WHERE environment_id=$1 AND path=$2 ORDER BY cached_at DESC LIMIT 1")
        .bind(environment).bind(path).fetch_optional(conn).await?;
    raw.map(CachedFile::try_from).transpose()
}

/// The cached versions below a directory, ordered newest first for each path.
pub async fn cached_below(
    conn: &mut Conn,
    environment: Id,
    directory: &str,
) -> Result<Vec<CachedFile>, DbError> {
    let prefix = if directory.is_empty() {
        String::new()
    } else {
        format!("{directory}/")
    };
    let raw: Vec<RawCached> = sqlx::query_as(
        "SELECT path, size, mtime_ns, hash, cached_at FROM file_cache
          WHERE environment_id = $1 AND starts_with(path, $2)
          ORDER BY path, cached_at DESC",
    )
    .bind(environment)
    .bind(prefix)
    .fetch_all(conn)
    .await?;
    raw.into_iter().map(CachedFile::try_from).collect()
}

/// Bounded newest-version metadata traversal for a cached-only directory listing.
/// Bytewise path order keeps each implied child together, independently of database locale.
pub async fn cached_latest_page(
    conn: &mut Conn,
    environment: Id,
    directory: &str,
    after: Option<&str>,
    limit: i64,
) -> Result<Vec<CachedFile>, DbError> {
    let prefix = if directory.is_empty() {
        String::new()
    } else {
        format!("{directory}/")
    };
    let raw: Vec<RawCached> = sqlx::query_as(
        r#"
        SELECT path,size,mtime_ns,hash,cached_at FROM (
            SELECT DISTINCT ON (path COLLATE "C") path,size,mtime_ns,hash,cached_at
              FROM file_cache WHERE environment_id=$1 AND starts_with(path,$2)
                AND ($3::text IS NULL OR path COLLATE "C" > $3 COLLATE "C")
              ORDER BY path COLLATE "C",cached_at DESC,mtime_ns DESC,hash
        ) latest ORDER BY path COLLATE "C" LIMIT $4"#,
    )
    .bind(environment)
    .bind(prefix)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    raw.into_iter().map(CachedFile::try_from).collect()
}

/// Forgets the cached versions of `path` and of everything below it: the file was renamed away or
/// deleted. Says how many versions went.
pub async fn forget_cached_under(
    conn: &mut Conn,
    environment: Id,
    path: &str,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM file_cache WHERE environment_id = $1 AND (path = $2 OR starts_with(path, $3))",
    )
    .bind(environment)
    .bind(path)
    .bind(format!("{path}/"))
    .execute(conn)
    .await?
    .rows_affected())
}

/// How many bytes of content each environment that has some cached holds, counting each content
/// once however many versions name it.
pub async fn cache_usage(conn: &mut Conn) -> Result<Vec<(Id, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT f.environment_id, COALESCE(sum(b.size), 0)::bigint
           FROM (SELECT DISTINCT environment_id, hash FROM file_cache) f
           JOIN blobs b ON b.hash = f.hash
          GROUP BY f.environment_id",
    )
    .fetch_all(conn)
    .await?)
}

/// The cached versions of an environment that may be evicted, the ones whose content was asked for
/// least lately first: those whose content something points at are not among them.
pub async fn eviction_candidates(
    conn: &mut Conn,
    environment: Id,
    limit: i64,
) -> Result<Vec<(CachedFile, i64)>, DbError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        path: String,
        size: i64,
        mtime_ns: i64,
        hash: Vec<u8>,
        cached_at: DateTime<Utc>,
        blob_size: i64,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT f.path, f.size, f.mtime_ns, f.hash, f.cached_at, b.size AS blob_size
           FROM file_cache f JOIN blobs b ON b.hash = f.hash
          WHERE f.environment_id = $1 AND b.pin_count = 0
          ORDER BY b.last_access_at, f.cached_at LIMIT $2",
    )
    .bind(environment)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                CachedFile::try_from(RawCached {
                    path: row.path,
                    size: row.size,
                    mtime_ns: row.mtime_ns,
                    hash: row.hash,
                    cached_at: row.cached_at,
                })?,
                row.blob_size,
            ))
        })
        .collect()
}

/// Forgets one cached version, and starts the grace of its content if nothing else is cached or
/// pinned under it. Says whether the version was there.
pub async fn evict_cached(
    conn: &mut Conn,
    environment: Id,
    file: &CachedFile,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let gone = sqlx::query(
        "DELETE FROM file_cache WHERE environment_id = $1 AND path = $2 AND size = $3 AND mtime_ns = $4",
    )
    .bind(environment)
    .bind(&file.path)
    .bind(file.size)
    .bind(file.mtime_ns)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;
    if gone {
        touch_blob(conn, &file.hash, at).await?;
    }
    Ok(gone)
}

// ---- what a thread carries ---------------------------------------------------------------------

/// The content parts of an input of the thread that holds the blob `hash`: an image or a file of it,
/// or of a message forwarded into it. None when no input of the thread carries the blob.
pub async fn input_carrying(
    conn: &mut Conn,
    thread: Id,
    hash: &Hash,
) -> Result<Option<Json>, DbError> {
    let blob = hash.to_string();
    let direct = serde_json::json!([{ "blob": blob }]);
    let images = serde_json::json!([{ "images": [{ "blob": blob }] }]);
    let attachments = serde_json::json!([{ "attachments": [{ "blob": blob }] }]);
    Ok(sqlx::query_scalar(
        "SELECT content FROM thread_inputs
          WHERE thread_id = $1 AND (content @> $2 OR content @> $3 OR content @> $4) LIMIT 1",
    )
    .bind(thread)
    .bind(&direct)
    .bind(&images)
    .bind(&attachments)
    .fetch_optional(&mut *conn)
    .await?)
}

// ---- snapshots --------------------------------------------------------------------------------

/// A snapshot, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotRow {
    /// Its identifier.
    pub id: Id,
    /// The thread it was taken through.
    pub thread_id: Option<Id>,
    /// The environment it was taken in.
    pub environment_id: Id,
    /// The generation of the environment's unit when it was taken.
    pub env_generation: i32,
    /// Why it was kept: manual, turn or restore.
    pub kind: String,
    /// The store that owns its immutable bytes.
    pub storage: String,
    /// The recorded state's owning directory.
    pub directory: Option<String>,
    /// Its immutable root in the environment-owned store.
    pub tree_ref: Option<String>,
    /// The locally known project HEAD when recorded.
    pub repository_head: Option<String>,
    /// Why a whole recorded state is unavailable.
    pub unavailable_reason: Option<String>,
    /// When the recorded root expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// What it is called.
    pub label: String,
    /// What it was asked to take: `{ include, exclude }`.
    pub selection: Json,
    /// `running`, `complete`, `partial` or `failed`.
    pub state: String,
    /// How many files it holds.
    pub file_count: i64,
    /// How many bytes it holds.
    pub total_bytes: i64,
    /// How many files are listed without their content.
    pub missing_count: i64,
    /// The list of what it took, once it has one.
    pub manifest_blob: Option<Hash>,
    /// Who took it.
    pub created_by: Option<Id>,
    /// When it was asked for.
    pub created_at: DateTime<Utc>,
    /// When it ended.
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct RawSnapshot {
    id: Id,
    thread_id: Option<Id>,
    environment_id: Id,
    env_generation: i32,
    kind: String,
    storage: String,
    directory: Option<String>,
    tree_ref: Option<String>,
    repository_head: Option<String>,
    unavailable_reason: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    label: String,
    selection: Json,
    state: String,
    file_count: i64,
    total_bytes: i64,
    missing_count: i64,
    manifest_blob: Option<Vec<u8>>,
    created_by: Option<Id>,
    created_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

impl TryFrom<RawSnapshot> for SnapshotRow {
    type Error = DbError;

    fn try_from(raw: RawSnapshot) -> Result<Self, DbError> {
        Ok(Self {
            id: raw.id,
            thread_id: raw.thread_id,
            environment_id: raw.environment_id,
            env_generation: raw.env_generation,
            kind: raw.kind,
            storage: raw.storage,
            directory: raw.directory,
            tree_ref: raw.tree_ref,
            repository_head: raw.repository_head,
            unavailable_reason: raw.unavailable_reason,
            expires_at: raw.expires_at,
            label: raw.label,
            selection: raw.selection,
            state: raw.state,
            file_count: raw.file_count,
            total_bytes: raw.total_bytes,
            missing_count: raw.missing_count,
            manifest_blob: raw.manifest_blob.as_deref().map(hash_of).transpose()?,
            created_by: raw.created_by,
            created_at: raw.created_at,
            completed_at: raw.completed_at,
        })
    }
}

const SNAPSHOT_COLUMNS: &str =
    "id, thread_id, environment_id, env_generation, kind, storage, directory, tree_ref,
    repository_head, unavailable_reason, expires_at, label, selection, state, file_count,
    total_bytes, missing_count, manifest_blob, created_by, created_at, completed_at";

/// The words of `snapshots.state`.
pub mod snapshot_state {
    /// Being taken.
    pub const RUNNING: &str = "running";
    /// All of it was taken.
    pub const COMPLETE: &str = "complete";
    /// Some files could not be taken.
    pub const PARTIAL: &str = "partial";
    /// Nothing usable came of it.
    pub const FAILED: &str = "failed";
    /// No complete file state could be recorded.
    pub const UNAVAILABLE: &str = "unavailable";
    /// The immutable root has expired.
    pub const EXPIRED: &str = "expired";
}

/// A snapshot to begin.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSnapshot<'a> {
    /// Its identifier.
    pub id: Id,
    /// The thread it is taken through.
    pub thread_id: Option<Id>,
    /// The environment it is taken in.
    pub environment_id: Id,
    /// The generation of the environment's unit.
    pub env_generation: i32,
    /// What it is called.
    pub label: &'a str,
    /// What it takes: `{ include, exclude }`.
    pub selection: &'a Json,
    /// Who asks.
    pub created_by: Option<Id>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Makes the row of a snapshot that is being taken.
pub async fn begin_snapshot(conn: &mut Conn, new: &NewSnapshot<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO snapshots (id, thread_id, environment_id, env_generation, label, selection, state, created_by, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, 'running', $7, $8)",
    )
    .bind(new.id)
    .bind(new.thread_id)
    .bind(new.environment_id)
    .bind(new.env_generation)
    .bind(new.label)
    .bind(new.selection)
    .bind(new.created_by)
    .bind(new.at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The snapshot `id`.
pub async fn snapshot(conn: &mut Conn, id: Id) -> Result<Option<SnapshotRow>, DbError> {
    let raw: Option<RawSnapshot> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SNAPSHOT_COLUMNS} FROM snapshots WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?;
    raw.map(SnapshotRow::try_from).transpose()
}

/// The snapshots taken through `thread`, newest first, `limit` of them that come after the place a
/// cursor names (`(created_at, id)` of the last one of the page before).
pub async fn snapshots_of_thread(
    conn: &mut Conn,
    thread: Id,
    after: Option<(DateTime<Utc>, Id)>,
    limit: i64,
) -> Result<Vec<SnapshotRow>, DbError> {
    let (at, id) = after.map_or((None, None), |(at, id)| (Some(at), Some(id)));
    let raw: Vec<RawSnapshot> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SNAPSHOT_COLUMNS} FROM snapshots
          WHERE thread_id = $1 AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
          ORDER BY created_at DESC, id DESC LIMIT $4"
    )))
    .bind(thread)
    .bind(at)
    .bind(id)
    .bind(limit)
    .fetch_all(conn)
    .await?;
    raw.into_iter().map(SnapshotRow::try_from).collect()
}

/// Lists only authorized entries before the cursor and limit, so hidden states affect no count.
pub async fn visible_snapshots_of_thread(
    conn: &mut Conn,
    thread: Id,
    after: Option<(DateTime<Utc>, Id)>,
    limit: i64,
    manual: bool,
    recorded_environments: &[Id],
) -> Result<Vec<SnapshotRow>, DbError> {
    let (at, id) = after.map_or((None, None), |(at, id)| (Some(at), Some(id)));
    let raw: Vec<RawSnapshot> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SNAPSHOT_COLUMNS} FROM snapshots WHERE thread_id = $1
            AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
            AND ((storage = 'blobs' AND $5) OR (storage = 'environment' AND environment_id = ANY($6)))
            ORDER BY created_at DESC, id DESC LIMIT $4"
    ))).bind(thread).bind(at).bind(id).bind(limit).bind(manual).bind(recorded_environments).fetch_all(conn).await?;
    raw.into_iter().map(SnapshotRow::try_from).collect()
}

/// What a snapshot that ended came to.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotEnd {
    /// `complete`, `partial` or `failed`.
    pub state: &'static str,
    /// How many files it holds.
    pub file_count: i64,
    /// How many bytes it holds.
    pub total_bytes: i64,
    /// How many files are listed without their content.
    pub missing_count: i64,
    /// The list of what it took.
    pub manifest_blob: Option<Hash>,
    /// When it ended.
    pub at: DateTime<Utc>,
}

/// Ends a snapshot that is being taken. Says whether it was.
pub async fn end_snapshot(conn: &mut Conn, id: Id, end: &SnapshotEnd) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE snapshots
            SET state = $2, file_count = $3, total_bytes = $4, missing_count = $5, manifest_blob = $6,
                completed_at = $7
          WHERE id = $1 AND state = 'running'",
    )
    .bind(id)
    .bind(end.state)
    .bind(end.file_count)
    .bind(end.total_bytes)
    .bind(end.missing_count)
    .bind(end.manifest_blob.as_ref().map(|hash| hash.as_bytes().to_vec()))
    .bind(end.at)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Gives a snapshot another label.
pub async fn relabel_snapshot(conn: &mut Conn, id: Id, label: &str) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE snapshots SET label = $2 WHERE id = $1")
        .bind(id)
        .bind(label)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}

/// Deletes a snapshot, and returns its list, whose blobs are the caller's to unpin.
pub async fn delete_snapshot(conn: &mut Conn, id: Id) -> Result<Option<Option<Hash>>, DbError> {
    let gone: Option<Option<Vec<u8>>> =
        sqlx::query_scalar("DELETE FROM snapshots WHERE id = $1 RETURNING manifest_blob")
            .bind(id)
            .fetch_optional(conn)
            .await?;
    gone.map(|manifest| manifest.as_deref().map(hash_of).transpose())
        .transpose()
}
