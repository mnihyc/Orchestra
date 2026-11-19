//! Search by meaning: the sources an org's index cuts, their pieces, the generations of vectors and where indexing stands.
//!
//! A source is a document, a conversation or a chat of one root org. It is cut again when its own sequence
//! (the document's, the thread's, the chat's) passed the one it was cut at; its pieces keep their identifiers
//! by their place in it, and a piece whose text changed keeps its old vector only until a new one is made. The
//! vectors' table exists where the database has the `vector` extension ([`vectors_ready`]); every statement
//! that reads or writes vectors is used only then. A generation's index (HNSW, by cosine, over its
//! half-precision vectors) is a partial index made with the generation and named after it, because the length of
//! its vectors is its own.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id, Json};

/// The largest initial graph-search breadth pgvector permits; iterative scans can return larger windows.
pub const EF_SEARCH_MAX: i64 = 1_000;

/// Whether vectors can be kept: the vectors' table is there, made now if the extension became available. When
/// it is not, the word says why: `absent`, or `not_permitted` where the database may not make the extension.
pub async fn vectors_ready(conn: &mut Conn) -> Result<Result<(), String>, DbError> {
    let state: String = sqlx::query_scalar("SELECT orch_search_vectors_setup()")
        .fetch_one(conn)
        .await?;
    Ok(if state == "ready" { Ok(()) } else { Err(state) })
}

/// The live root orgs, by name: where an index can be.
pub async fn root_orgs(conn: &mut Conn) -> Result<Vec<(Id, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, name FROM nodes WHERE kind = 'org' AND parent_id IS NULL AND deleted_at IS NULL
          ORDER BY lower(name), id",
    )
    .fetch_all(conn)
    .await?)
}

/// The root org of a node, while the node is live.
pub async fn org_of(conn: &mut Conn, node: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM nodes WHERE id = $1 AND deleted_at IS NULL")
            .bind(node)
            .fetch_optional(conn)
            .await?,
    )
}

/// The live root orgs that enabled meaning or still have an answering generation.
pub async fn answering_orgs(conn: &mut Conn) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT n.id FROM nodes n
          LEFT JOIN settings own ON own.node_id=n.id AND own.key='search.semantic.enabled'
          LEFT JOIN settings system ON system.node_id IS NULL AND system.key='search.semantic.enabled'
         WHERE n.kind='org' AND n.parent_id IS NULL AND n.deleted_at IS NULL
           AND (COALESCE(CASE WHEN system.locked THEN system.value ELSE COALESCE(own.value,system.value) END,'false'::jsonb)='true'::jsonb
             OR EXISTS(SELECT 1 FROM search_generations g WHERE g.org_id=n.id AND g.state='active' AND g.dimensions IS NOT NULL))
         ORDER BY n.id",
    ).fetch_all(conn).await?)
}

/// The root orgs that have an index or a generation, whatever they chose now.
pub async fn indexed_orgs(conn: &mut Conn) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT org_id FROM search_indexes UNION SELECT org_id FROM search_generations ORDER BY 1",
    )
    .fetch_all(conn)
    .await?)
}

/// The endpoints and applied runtime recipe of a live provider, independent of model lists, rates, names or access changes.
pub async fn endpoint_fingerprint(
    conn: &mut Conn,
    provider: Id,
) -> Result<Option<Vec<u8>>, DbError> {
    let config: Option<Json> =
        sqlx::query_scalar("SELECT jsonb_build_object('upstreams',p.upstreams,'runtime',
             (SELECT encode(r.spec_hash,'hex') FROM runtimes r WHERE r.provider_id=p.id AND r.successor_of IS NULL
                AND r.state NOT IN ('retired','removed') LIMIT 1))
             FROM providers p WHERE p.id=$1 AND p.deleted_at IS NULL")
            .bind(provider)
            .fetch_optional(conn)
            .await?;
    Ok(config.map(|config| Hash::of(config.to_string().as_bytes()).as_bytes().to_vec()))
}

/// Why a provider's embeddings are intentionally paused, without treating its lifecycle as a model fault.
pub async fn provider_pause_reason(
    conn: &mut Conn,
    provider: Id,
) -> Result<Option<&'static str>, DbError> {
    let state: Option<(String, bool)> =
        sqlx::query_as("SELECT status,deleted_at IS NOT NULL FROM providers WHERE id=$1")
            .bind(provider)
            .fetch_optional(conn)
            .await?;
    Ok(match state {
        None => Some("provider_removed"),
        Some((status, removed)) if removed || status == "removed" => Some("provider_removed"),
        Some((status, _)) if status == "disabled" => Some("provider_disabled"),
        _ => None,
    })
}

/// A call's intentional stop survives a later Enable and also observes the provider's current lifecycle.
pub async fn embedding_pause_reason(
    conn: &mut Conn,
    call: Id,
    provider: Id,
) -> Result<Option<&'static str>, DbError> {
    let reason: Option<String> = sqlx::query_scalar(
        "SELECT COALESCE(c.cancel_reason,CASE
           WHEN p.id IS NULL OR p.deleted_at IS NOT NULL THEN 'provider_removed'
           WHEN p.status='disabled' THEN 'provider_disabled'
           WHEN m.id IS NOT NULL AND NOT m.enabled THEN 'model_disabled' END)
         FROM (VALUES($2::uuid)) asked(provider_id)
         LEFT JOIN providers p ON p.id=asked.provider_id
         LEFT JOIN summary_calls c ON c.id=$1 AND c.provider_id=asked.provider_id AND c.purpose='embeddings'
         LEFT JOIN provider_models m ON m.provider_id=p.id AND m.model_name=c.model_name",
    )
    .bind(call)
    .bind(provider)
    .fetch_one(conn)
    .await?;
    Ok(reason.map(|reason| match reason.as_str() {
        "provider_removed" | "Provider removed" => "provider_removed",
        "provider_disabled" | "Provider switched off" => "provider_disabled",
        "model_disabled" | "Model switched off" | "Model removed" => "model_disabled",
        _ => "provider",
    }))
}

/// Marks existing admissions in the same transaction that switches their provider off or removes it.
pub async fn mark_embedding_stop(conn: &mut Conn, provider: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE summary_calls c SET cancel_reason=COALESCE(c.cancel_reason,CASE WHEN p.deleted_at IS NOT NULL
           THEN 'provider_removed' ELSE 'provider_disabled' END)
         FROM providers p WHERE p.id=$1 AND c.provider_id=p.id AND c.purpose='embeddings'
           AND (p.status='disabled' OR p.deleted_at IS NOT NULL)",
    )
    .bind(provider)
    .execute(conn)
    .await?;
    Ok(())
}

/// Sets an org's indexing back to idle with no reason: what an admin's choice does before the next turn.
pub async fn reset_index(conn: &mut Conn, org: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE search_indexes SET state = 'idle', reason = NULL, detail = NULL, updated_at = $2 WHERE org_id = $1",
    )
    .bind(org)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Where indexing stands for one root org.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct IndexRow {
    /// The root org.
    pub org_id: Id,
    /// `idle`, `indexing`, `paused` or `waiting`.
    pub state: String,
    /// Why it pauses or waits, as a word.
    pub reason: Option<String>,
    /// That reason in a sentence.
    pub detail: Option<String>,
    /// The endpoint's estimated tokens a second, as last measured.
    pub rate: Option<f64>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
}

/// The state of an org's indexing, if it ever had any.
pub async fn index(conn: &mut Conn, org: Id) -> Result<Option<IndexRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT org_id, state, reason, detail, rate, updated_at FROM search_indexes WHERE org_id = $1",
    )
    .bind(org)
    .fetch_optional(conn)
    .await?)
}

/// Takes the org's indexing for `until`, unless another process holds it; `true` when it is this one's.
pub async fn claim(
    conn: &mut Conn,
    org: Id,
    claimant: Id,
    now: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<bool, DbError> {
    let taken = sqlx::query(
        "INSERT INTO search_indexes (org_id, claimed_by, claimed_until, updated_at) VALUES ($1, $2, $3, $4)
         ON CONFLICT (org_id) DO UPDATE SET claimed_by = $2, claimed_until = $3
          WHERE search_indexes.claimed_until IS NULL OR search_indexes.claimed_until <= $4
             OR search_indexes.claimed_by = $2",
    )
    .bind(org)
    .bind(claimant)
    .bind(until)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(taken.rows_affected() == 1)
}

/// Gives the org's indexing back, saying where it stands.
#[allow(
    clippy::too_many_arguments,
    reason = "one row's columns, written together by the process that held it"
)]
pub async fn release(
    conn: &mut Conn,
    org: Id,
    claimant: Id,
    state: &str,
    reason: Option<&str>,
    detail: Option<&str>,
    rate: Option<f64>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE search_indexes SET state = $3, reason = $4, detail = $5, rate = COALESCE($6, rate),
                updated_at = $7, claimed_by = NULL, claimed_until = NULL
          WHERE org_id = $1 AND claimed_by = $2",
    )
    .bind(org)
    .bind(claimant)
    .bind(state)
    .bind(reason)
    .bind(detail)
    .bind(rate)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// One generation of an org's vectors.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct GenerationRow {
    /// Its identifier.
    pub id: Id,
    /// The root org.
    pub org_id: Id,
    /// The provider whose endpoint makes its vectors now.
    pub provider_id: Id,
    /// The endpoint fingerprint whose model passed the stored probes.
    pub provider_fingerprint: Vec<u8>,
    /// Who named this generation's model; none after that person was removed.
    pub writer_id: Option<Id>,
    /// The model, by the name that is sent.
    pub model_name: String,
    /// The length asked of the model; `None` for its own.
    pub asked_dimensions: Option<i32>,
    /// The length its vectors have, once the first came back.
    pub dimensions: Option<i32>,
    /// What is put before a query.
    pub query_prefix: String,
    /// What is put before a document.
    pub document_prefix: String,
    /// How text is cut.
    pub chunking: String,
    /// The probe texts' vectors, as its model made them.
    pub probe: Json,
    /// `building`, `active` or `retired`.
    pub state: String,
    /// When it was begun.
    pub created_at: DateTime<Utc>,
    /// When it began to answer.
    pub activated_at: Option<DateTime<Utc>>,
}

const GENERATION_COLUMNS: &str =
    "id, org_id, provider_id, provider_fingerprint, writer_id, model_name, asked_dimensions, dimensions, query_prefix,
    document_prefix, chunking, probe, state, created_at, activated_at";

/// The org's generations that are not retired: the one that answers and the one being built, if any.
pub async fn generations(conn: &mut Conn, org: Id) -> Result<Vec<GenerationRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {GENERATION_COLUMNS} FROM search_generations WHERE org_id = $1 AND state <> 'retired'
          ORDER BY created_at, id"
    )))
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// What a new generation is made with.
#[derive(Debug, Clone)]
pub struct NewGeneration<'a> {
    /// Its identifier.
    pub id: Id,
    /// The root org.
    pub org_id: Id,
    /// The provider.
    pub provider_id: Id,
    /// The endpoint fingerprint that made its probe vectors.
    pub provider_fingerprint: &'a [u8],
    /// Who named its model and answers for its use.
    pub writer_id: Option<Id>,
    /// The model.
    pub model_name: &'a str,
    /// The length asked for.
    pub asked_dimensions: Option<i32>,
    /// What is put before a query.
    pub query_prefix: &'a str,
    /// What is put before a document.
    pub document_prefix: &'a str,
    /// How text is cut.
    pub chunking: &'a str,
    /// `building`, or `active` for the first of an org.
    pub state: &'a str,
    /// When.
    pub at: DateTime<Utc>,
}

/// Begins a generation.
pub async fn insert_generation(conn: &mut Conn, new: &NewGeneration<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO search_generations (id, org_id, provider_id, model_name, asked_dimensions, query_prefix,
                document_prefix, chunking, state, created_at, activated_at, provider_fingerprint, writer_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, CASE WHEN $9 = 'active' THEN $10 END, $11, $12)",
    )
    .bind(new.id)
    .bind(new.org_id)
    .bind(new.provider_id)
    .bind(new.model_name)
    .bind(new.asked_dimensions)
    .bind(new.query_prefix)
    .bind(new.document_prefix)
    .bind(new.chunking)
    .bind(new.state)
    .bind(new.at)
    .bind(new.provider_fingerprint)
    .bind(new.writer_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Records the length a generation's vectors came back with, and the probe texts' vectors.
pub async fn set_made(
    conn: &mut Conn,
    generation: Id,
    dimensions: i32,
    probe: &Json,
) -> Result<(), DbError> {
    sqlx::query("UPDATE search_generations SET dimensions = $2, probe = $3 WHERE id = $1")
        .bind(generation)
        .bind(dimensions)
        .bind(probe)
        .execute(conn)
        .await?;
    Ok(())
}

/// Hands an active generation to another provider and name of the same model, which the probe texts showed.
pub async fn move_generation(
    conn: &mut Conn,
    generation: Id,
    provider: Id,
    model_name: &str,
    fingerprint: &[u8],
    writer: Option<Id>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE search_generations SET provider_id = $2, model_name = $3,
        provider_fingerprint = $4, writer_id = $5 WHERE id = $1",
    )
    .bind(generation)
    .bind(provider)
    .bind(model_name)
    .bind(fingerprint)
    .bind(writer)
    .execute(conn)
    .await?;
    Ok(())
}

/// Records a new writer of the same active model configuration without rebuilding its vectors.
pub async fn generation_writer(
    conn: &mut Conn,
    generation: Id,
    writer: Option<Id>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE search_generations SET writer_id = $2 WHERE id = $1 AND writer_id IS DISTINCT FROM $2")
        .bind(generation).bind(writer).execute(conn).await?;
    Ok(())
}

/// Makes a built generation the one that answers and retires the one before it, in one step.
pub async fn activate(
    conn: &mut Conn,
    org: Id,
    generation: Id,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, DbError> {
    let retired: Vec<Id> = sqlx::query_scalar(
        "UPDATE search_generations SET state = 'retired', retired_at = $3
          WHERE org_id = $1 AND state = 'active' AND id <> $2 RETURNING id",
    )
    .bind(org)
    .bind(generation)
    .bind(at)
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE search_generations SET state = 'active', activated_at = $2 WHERE id = $1 AND state = 'building'",
    )
    .bind(generation)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    Ok(retired)
}

/// The name of a generation's index.
fn index_name(generation: Id) -> String {
    format!("search_vectors_{}", generation.simple())
}

/// Makes a generation's index of its vectors of `dimensions` numbers, compared by cosine. (An index of their binary
/// signatures with the candidates compared again exactly takes a third less, and found 85 % of the ten nearest of a
/// query on a corpus of Orch's own documents at 256 numbers; this one finds all of them.)
pub async fn make_index(conn: &mut Conn, generation: Id, dimensions: i32) -> Result<(), DbError> {
    // Both values are the system's own: an identifier and a positive length, never text a person wrote.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE INDEX IF NOT EXISTS {} ON search_vectors
           USING hnsw ((subvector(embedding,1,{dimensions})::halfvec({dimensions})) halfvec_cosine_ops) WHERE generation_id = '{generation}'",
        index_name(generation)
    )))
    .execute(&mut *conn)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE INDEX IF NOT EXISTS search_item_vectors_{} ON search_item_vectors
           USING hnsw ((subvector(embedding,1,{dimensions})::halfvec({dimensions})) halfvec_cosine_ops) WHERE generation_id = '{generation}'",
        generation.simple()
    )))
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes a generation with its vectors and its index.
pub async fn drop_generation(conn: &mut Conn, generation: Id) -> Result<(), DbError> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP INDEX IF EXISTS search_item_vectors_{}",
        generation.simple()
    )))
    .execute(&mut *conn)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP INDEX IF EXISTS {}",
        index_name(generation)
    )))
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM search_generations WHERE id = $1")
        .bind(generation)
        .execute(conn)
        .await?;
    Ok(())
}

/// Removes a generation's row where the vectors' table does not exist; its sources forget it.
pub async fn forget_generation(conn: &mut Conn, generation: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE search_sources SET embedded_for = NULL WHERE embedded_for = $1")
        .bind(generation)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM search_generations WHERE id = $1")
        .bind(generation)
        .execute(conn)
        .await?;
    Ok(())
}

/// Cuts a generation's vectors to their first `to` numbers, made unit length again, and makes its index anew.
pub async fn shorten(
    conn: &mut Conn,
    generation: Id,
    to: i32,
    probe: &Json,
) -> Result<(), DbError> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP INDEX IF EXISTS {}",
        index_name(generation)
    )))
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE search_vectors SET embedding = l2_normalize(subvector(embedding, 1, $2))
          WHERE generation_id = $1",
    )
    .bind(generation)
    .bind(to)
    .execute(&mut *conn)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP INDEX IF EXISTS search_item_vectors_{}",
        generation.simple()
    )))
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE search_item_vectors item SET embedding=centroid.embedding FROM (
       SELECT c.kind,c.source_id,l2_normalize(avg(l2_normalize(v.embedding::vector)))::halfvec embedding
       FROM search_chunks c JOIN search_vectors v ON v.generation_id=c.generation_id AND v.chunk_id=c.id AND v.text_hash=c.text_hash
       WHERE c.generation_id=$1 GROUP BY c.kind,c.source_id
       ) centroid WHERE item.generation_id=$1 AND item.kind=centroid.kind AND item.source_id=centroid.source_id")
        .bind(generation).execute(&mut *conn).await?;
    // Query caches live in the same generation's space; shorten them along with its stored vectors.
    sqlx::query("UPDATE search_query_vectors SET embedding = l2_normalize((embedding[1:$2])::vector)::real[] WHERE generation_id=$1")
        .bind(generation).bind(to).execute(&mut *conn).await?;
    sqlx::query(
        "UPDATE search_generations SET asked_dimensions = $2, dimensions = $2, probe = $3 WHERE id = $1",
    )
    .bind(generation)
    .bind(to)
    .bind(probe)
    .execute(&mut *conn)
    .await?;
    make_index(conn, generation, to).await
}

/// A source whose text is to be cut, with what its pieces' prefix calls its title.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SourceRow {
    /// `document`, `conversation` or `chat`.
    pub kind: String,
    /// The document, the thread or the chat.
    pub id: Id,
    /// Its own sequence now.
    pub marker: i64,
    /// When it last changed.
    pub changed_at: DateTime<Utc>,
    /// Its title, or the name of its thread or chat.
    pub title: String,
}

/// The sources of a root org changed since they were last cut, or never cut, newest first. A conversation
/// whose turn runs waits for its turn to end: what it says is still being written.
pub async fn changed_sources(
    conn: &mut Conn,
    org: Id,
    generation: Id,
    limit: i64,
) -> Result<Vec<SourceRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT * FROM (
           SELECT 'document'::text kind, d.id, d.doc_seq marker, d.updated_at changed_at, d.title
             FROM documents d JOIN nodes n ON n.id = d.node_id
             LEFT JOIN search_sources s ON s.kind = 'document' AND s.source_id = d.id AND s.generation_id = $3
            WHERE d.org_id = $1 AND d.deleted_at IS NULL AND (s.source_id IS NULL OR d.doc_seq <> s.marker OR d.title <> s.title)
              AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL)
           UNION ALL
           SELECT 'conversation', t.node_id, t.next_seq, COALESCE(t.last_activity_at, n.updated_at), n.name
             FROM threads t JOIN nodes n ON n.id = t.node_id
             LEFT JOIN search_sources s ON s.kind = 'conversation' AND s.source_id = t.node_id AND s.generation_id = $3
            WHERE n.org_id = $1 AND t.side_of IS NULL AND t.state NOT IN ('starting','running','waiting_approval')
              AND (s.source_id IS NULL OR t.next_seq <> s.marker OR n.name <> s.title)
              AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL)
           UNION ALL
           SELECT 'chat', c.id, c.next_seq, COALESCE(c.last_message_at, c.created_at), COALESCE(c.name, n.name, '')
             FROM chats c LEFT JOIN nodes n ON n.id = c.node_id
             LEFT JOIN search_sources s ON s.kind = 'chat' AND s.source_id = c.id AND s.generation_id = $3
            WHERE c.org_id = $1 AND (s.source_id IS NULL OR c.next_seq <> s.marker OR COALESCE(c.name, n.name, '') <> s.title)
              AND (n.id IS NULL OR NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL))
         ) found ORDER BY changed_at DESC, kind, id LIMIT $2",
    )
    .bind(org)
    .bind(limit)
    .bind(generation)
    .fetch_all(conn)
    .await?)
}

/// Forgets the sources of a root org that are gone: deleted, or under a deleted place. Their pieces and
/// vectors go with them.
pub async fn forget_gone(conn: &mut Conn, org: Id) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM search_sources s WHERE s.org_id = $1 AND CASE s.kind
           WHEN 'document' THEN NOT EXISTS (SELECT 1 FROM documents d JOIN nodes n ON n.id = d.node_id
             WHERE d.id = s.source_id AND d.org_id = s.org_id AND d.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL))
           WHEN 'conversation' THEN NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = s.source_id AND n.org_id = s.org_id
               AND NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL))
           ELSE NOT EXISTS (SELECT 1 FROM chats c LEFT JOIN nodes n ON n.id = c.node_id
             WHERE c.id = s.source_id AND c.org_id = s.org_id
               AND (n.id IS NULL OR NOT EXISTS (SELECT 1 FROM nodes gone WHERE gone.id = ANY(n.path) AND gone.deleted_at IS NOT NULL)))
         END",
    )
    .bind(org)
    .execute(conn)
    .await?
    .rows_affected())
}

/// One part of a source as it is cut: its identifier, its text, whether it is a heading, for a block the block it
/// lies in, and the bytes its row takes where it is stored.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PartRow {
    /// The block, item or message.
    pub id: Id,
    /// For a block: the block it lies in.
    pub parent: Option<Id>,
    /// Its text.
    pub text: String,
    /// Whether it is a heading.
    pub heading: bool,
    /// The bytes of its row as stored: what the disk share of search by meaning is a share of.
    pub stored: i64,
}

/// The live blocks of a document, siblings in their order; the caller walks them from the top.
pub async fn document_parts(conn: &mut Conn, document: Id) -> Result<Vec<PartRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT b.id, b.parent_id AS parent, b.text, (b.type = 'heading') AS heading,
                pg_column_size(b.*)::bigint AS stored
           FROM document_blocks b WHERE b.document_id = $1 AND b.removed_at IS NULL ORDER BY b.position, b.id",
    )
    .bind(document)
    .fetch_all(conn)
    .await?)
}

/// What a conversation says, in order: the messages of people and agents, plans and announcements of its main
/// stream, as search reads them, in the history the thread shows now (its present continuation with what it
/// keeps of those before it).
pub async fn conversation_parts(conn: &mut Conn, thread: Id) -> Result<Vec<PartRow>, DbError> {
    Ok(sqlx::query_as(
        "WITH owners AS (
           SELECT t.active_continuation_id AS owner, 1 AS first_turn, 2147483647 AS last_turn,
                  0::bigint AS first_seq, 9223372036854775807::bigint AS last_seq
             FROM threads t WHERE t.node_id = $1
           UNION ALL
           SELECT s.owner_continuation_id, s.first_turn, s.last_turn, s.first_seq, s.last_seq
             FROM threads t JOIN thread_continuation_spans s ON s.continuation_id = t.active_continuation_id
            WHERE t.node_id = $1)
         SELECT i.id, NULL::uuid AS parent, i.search_text AS text, false AS heading,
                pg_column_size(i.*)::bigint AS stored
           FROM items i JOIN owners o ON o.owner = i.continuation_id
          WHERE i.thread_id = $1 AND i.stream = 'main'
            AND i.kind IN ('user_message','agent_message','plan','announcement')
            AND i.search_text IS NOT NULL AND i.search_text NOT LIKE '%<orch>%'
            AND i.seq_created BETWEEN o.first_seq AND o.last_seq
            AND (i.turn_id IS NULL OR EXISTS (SELECT 1 FROM turns tu WHERE tu.id = i.turn_id
                                                 AND tu.ordinal BETWEEN o.first_turn AND o.last_turn))
          ORDER BY i.seq_created, i.id",
    )
    .bind(thread)
    .fetch_all(conn)
    .await?)
}

/// The live messages of a chat, in order.
pub async fn chat_parts(conn: &mut Conn, chat: Id) -> Result<Vec<PartRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.id, NULL::uuid AS parent, m.search_text AS text, false AS heading,
                pg_column_size(m.*)::bigint AS stored
           FROM chat_messages m WHERE m.chat_id = $1 AND m.deleted_at IS NULL AND m.search_text IS NOT NULL
          ORDER BY m.seq_created, m.id",
    )
    .bind(chat)
    .fetch_all(conn)
    .await?)
}

/// One piece of a cut, as it is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutPiece {
    /// The part it begins with.
    pub first_part: Id,
    /// BLAKE3 of its text.
    pub text_hash: Vec<u8>,
    /// Its estimated tokens.
    pub tokens: i32,
    /// The bytes of its text.
    pub bytes: i32,
}

/// A source's pieces as kept: identifier, place, text hash.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ChunkRow {
    /// Its identifier, kept while its place is.
    pub id: Id,
    /// Its place in the source.
    pub ordinal: i32,
    /// BLAKE3 of its text.
    pub text_hash: Vec<u8>,
}

/// Keeps a source's new cut: each piece in its place, pieces past the last removed, and the source's marker,
/// counts, the bytes its parts take where they are stored (`stored`) and change. A source with any new or changed
/// piece no longer counts as embedded. Returns the pieces.
pub async fn save_cut(
    conn: &mut Conn,
    source: &SourceRow,
    org: Id,
    generation: Id,
    pieces: &[CutPiece],
    stored: i64,
) -> Result<Vec<ChunkRow>, DbError> {
    let node: Option<Id>=sqlx::query_scalar("SELECT CASE $1::text WHEN 'document' THEN (SELECT node_id FROM documents WHERE id=$2)
       WHEN 'conversation' THEN $2::uuid ELSE (SELECT COALESCE(node_id,org_id) FROM chats WHERE id=$2) END")
       .bind(&source.kind).bind(source.id).fetch_one(&mut *conn).await?;
    let node = node.unwrap_or(org);
    let tokens: i64 = pieces.iter().map(|piece| i64::from(piece.tokens)).sum();
    let bytes = stored;
    let count = i32::try_from(pieces.len()).unwrap_or(i32::MAX);
    sqlx::query(
        "INSERT INTO search_sources (kind, source_id, org_id, marker, changed_at, chunks, tokens, bytes, generation_id, title)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (generation_id, kind, source_id) DO UPDATE SET org_id = $3, marker = $4, changed_at = $5, chunks = $6,
                tokens = $7, bytes = $8, title = $10",
    )
    .bind(&source.kind)
    .bind(source.id)
    .bind(org)
    .bind(source.marker)
    .bind(source.changed_at)
    .bind(count)
    .bind(tokens)
    .bind(bytes)
    .bind(generation)
    .bind(&source.title)
    .execute(&mut *conn)
    .await?;
    let ordinals: Vec<i32> = (0..count).collect();
    let ids: Vec<Id> = pieces.iter().map(|_| orch_types::new_id()).collect();
    let firsts: Vec<Id> = pieces.iter().map(|piece| piece.first_part).collect();
    let hashes: Vec<Vec<u8>> = pieces.iter().map(|piece| piece.text_hash.clone()).collect();
    let token_counts: Vec<i32> = pieces.iter().map(|piece| piece.tokens).collect();
    let byte_counts: Vec<i32> = pieces.iter().map(|piece| piece.bytes).collect();
    let changed: i64 = sqlx::query_scalar(
        "WITH old AS (SELECT ordinal, text_hash FROM search_chunks WHERE kind = $1 AND source_id = $2 AND generation_id = $10),
              put AS (
           INSERT INTO search_chunks (id, kind, source_id, org_id, ordinal, first_part, text_hash, tokens, bytes, generation_id, node_id)
           SELECT piece.id, $1, $2, $3, piece.ordinal, piece.first_part, piece.text_hash, piece.tokens, piece.bytes, $10, $11
             FROM unnest($4::uuid[], $5::int[], $6::uuid[], $7::bytea[], $8::int[], $9::int[])
                  AS piece(id, ordinal, first_part, text_hash, tokens, bytes)
           ON CONFLICT (generation_id, kind, source_id, ordinal) DO UPDATE SET org_id = EXCLUDED.org_id,node_id=EXCLUDED.node_id,
                  first_part = EXCLUDED.first_part, text_hash = EXCLUDED.text_hash, tokens = EXCLUDED.tokens,
                  bytes = EXCLUDED.bytes
           RETURNING ordinal, text_hash)
         SELECT count(*) FROM put LEFT JOIN old USING (ordinal) WHERE old.text_hash IS DISTINCT FROM put.text_hash",
    )
    .bind(&source.kind)
    .bind(source.id)
    .bind(org)
    .bind(&ids)
    .bind(&ordinals)
    .bind(&firsts)
    .bind(&hashes)
    .bind(&token_counts)
    .bind(&byte_counts)
    .bind(generation)
    .bind(node)
    .fetch_one(&mut *conn)
    .await?;
    let removed = sqlx::query(
        "DELETE FROM search_chunks WHERE kind = $1 AND source_id = $2 AND ordinal >= $3 AND generation_id = $4",
    )
    .bind(&source.kind)
    .bind(source.id)
    .bind(count)
    .bind(generation)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed > 0 || removed > 0 {
        sqlx::query(
            "UPDATE search_sources SET embedded_for = NULL WHERE kind = $1 AND source_id = $2 AND generation_id = $3",
        )
        .bind(&source.kind)
        .bind(source.id)
        .bind(generation)
        .execute(&mut *conn)
        .await?;
    }
    Ok(sqlx::query_as(
        "SELECT id, ordinal, text_hash FROM search_chunks WHERE kind = $1 AND source_id = $2 AND generation_id = $3 ORDER BY ordinal",
    )
    .bind(&source.kind)
    .bind(source.id)
    .bind(generation)
    .fetch_all(conn)
    .await?)
}

/// What a source's pieces put before their text as its title: a document's title, a thread's or a chat's name.
pub async fn source_title(conn: &mut Conn, kind: &str, source: Id) -> Result<String, DbError> {
    let title: Option<String> = match kind {
        "document" => sqlx::query_scalar("SELECT title FROM documents WHERE id = $1")
            .bind(source)
            .fetch_optional(conn)
            .await?,
        "conversation" => sqlx::query_scalar("SELECT name FROM nodes WHERE id = $1")
            .bind(source)
            .fetch_optional(conn)
            .await?,
        _ => sqlx::query_scalar(
            "SELECT COALESCE(c.name, n.name, '') FROM chats c LEFT JOIN nodes n ON n.id = c.node_id WHERE c.id = $1",
        )
        .bind(source)
        .fetch_optional(conn)
        .await?,
    };
    Ok(title.unwrap_or_default())
}

/// A source's pieces as kept, in their order.
pub async fn chunks_of(
    conn: &mut Conn,
    kind: &str,
    source: Id,
    generation: Id,
) -> Result<Vec<ChunkRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, ordinal, text_hash FROM search_chunks WHERE kind = $1 AND source_id = $2 AND generation_id = $3 ORDER BY ordinal",
    )
    .bind(kind)
    .bind(source)
    .bind(generation)
    .fetch_all(conn)
    .await?)
}

/// A source that was cut, with what it holds.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CutSource {
    /// `document`, `conversation` or `chat`.
    pub kind: String,
    /// The document, the thread or the chat.
    pub source_id: Id,
    /// Its sequence when it was cut.
    pub marker: i64,
    /// When it last changed then.
    pub changed_at: DateTime<Utc>,
    /// Its estimated tokens.
    pub tokens: i64,
}

/// Cut sources of a root org that lack a vector of `generation` for some piece: by the day they changed, newest
/// first; on one day, those with the most text first.
pub async fn sources_to_embed(
    conn: &mut Conn,
    org: Id,
    generation: Id,
    limit: i64,
) -> Result<Vec<CutSource>, DbError> {
    Ok(sqlx::query_as(
        "SELECT kind, source_id, marker, changed_at, tokens FROM search_sources
          WHERE org_id = $1 AND generation_id = $2 AND chunks > 0 AND (embedded_for IS DISTINCT FROM $2
             OR NOT EXISTS(SELECT 1 FROM search_item_vectors v
               WHERE v.generation_id=$2 AND v.kind=search_sources.kind AND v.source_id=search_sources.source_id))
          ORDER BY date_trunc('day', changed_at) DESC, tokens DESC, changed_at DESC, kind, source_id LIMIT $3",
    )
    .bind(org)
    .bind(generation)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Marks a source as having a vector of `generation` for each of its pieces.
pub async fn mark_embedded(
    conn: &mut Conn,
    kind: &str,
    source: Id,
    marker: i64,
    generation: Id,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE search_sources SET embedded_for = $4 WHERE kind = $1 AND source_id = $2 AND marker = $3 AND generation_id = $4",
    )
    .bind(kind)
    .bind(source)
    .bind(marker)
    .bind(generation)
    .execute(conn)
    .await?;
    Ok(())
}

/// Of these pieces, those without a vector of `generation` made of their present text.
pub async fn missing_vectors(
    conn: &mut Conn,
    generation: Id,
    chunks: &[Id],
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT c.id FROM search_chunks c WHERE c.id = ANY($2)
            AND NOT EXISTS (SELECT 1 FROM search_vectors v
                             WHERE v.generation_id = $1 AND v.chunk_id = c.id AND v.text_hash = c.text_hash)",
    )
    .bind(generation)
    .bind(chunks)
    .fetch_all(conn)
    .await?)
}

/// How many of these pieces already occupy a row in a generation, including rows awaiting replacement.
pub async fn stored_vectors(
    conn: &mut Conn,
    generation: Id,
    chunks: &[Id],
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM search_vectors WHERE generation_id = $1 AND chunk_id = ANY($2)",
    )
    .bind(generation)
    .bind(chunks)
    .fetch_one(conn)
    .await?)
}

/// Keeps vectors of `generation`, one for each piece, made of the text whose hash is given.
pub async fn put_vectors(
    conn: &mut Conn,
    generation: Id,
    rows: &[(Id, Vec<u8>, Vec<f32>)],
) -> Result<(), DbError> {
    let chunks: Vec<Id> = rows.iter().map(|(chunk, _, _)| *chunk).collect();
    let hashes: Vec<Vec<u8>> = rows.iter().map(|(_, hash, _)| hash.clone()).collect();
    // The extension reads a vector from its text form; this is the system's own numbers, never a person's text.
    let vectors: Vec<String> = rows
        .iter()
        .map(|(_, _, vector)| {
            let numbers: Vec<String> = vector.iter().map(f32::to_string).collect();
            format!("[{}]", numbers.join(","))
        })
        .collect();
    sqlx::query(
        "INSERT INTO search_vectors (generation_id, chunk_id, text_hash, embedding)
         SELECT $1, made.chunk, made.hash, made.vector::halfvec
           FROM unnest($2::uuid[], $3::bytea[], $4::text[]) AS made(chunk, hash, vector)
         ON CONFLICT (generation_id, chunk_id) DO UPDATE SET text_hash = EXCLUDED.text_hash,
                embedding = EXCLUDED.embedding",
    )
    .bind(generation)
    .bind(&chunks)
    .bind(&hashes)
    .bind(&vectors)
    .execute(conn)
    .await?;
    Ok(())
}

/// A piece found near a query, with where it lies.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct NearRow {
    /// The piece.
    pub chunk_id: Id,
    /// `document`, `conversation` or `chat`.
    pub kind: String,
    /// The document, the thread or the chat.
    pub source_id: Id,
    /// The part it begins with.
    pub first_part: Option<Id>,
    /// Its cosine distance to the query: 0 is the same direction.
    pub distance: f64,
}

/// The `keep` pieces of `generation` nearest a query, by the cosine of their half-precision vectors: the index looks
/// at `keep × oversample` candidates, and a candidate whose piece changed since its vector was made is left out.
/// Run in a transaction: the index is told how many candidates to give, which it would otherwise cut at its
/// default.
pub async fn nearest(
    conn: &mut Conn,
    generation: Id,
    dimensions: i32,
    query: &[f32],
    keep: i64,
    oversample: i64,
) -> Result<Vec<NearRow>, DbError> {
    nearest_filtered(conn, generation, dimensions, query, keep, oversample, None).await
}

/// Filters native sources before the nearest candidate cutoff, while pgvector iterates its graph.
pub async fn nearest_filtered(
    conn: &mut Conn,
    generation: Id,
    dimensions: i32,
    query: &[f32],
    keep: i64,
    oversample: i64,
    filter: Option<&Json>,
) -> Result<Vec<NearRow>, DbError> {
    let candidates = keep.saturating_mul(oversample).max(keep).max(1);
    let breadth = filter
        .and_then(|filter| filter.get("ef_search"))
        .and_then(Json::as_i64)
        .unwrap_or(candidates)
        .clamp(1, EF_SEARCH_MAX);
    sqlx::query("SELECT set_config('hnsw.ef_search', $1, true)")
        .bind(breadth.to_string())
        .execute(&mut *conn)
        .await?;
    let numbers: Vec<String> = query.iter().map(f32::to_string).collect();
    let vector = format!("[{}]", numbers.join(","));
    // The generation's identifier is written into the statement so that its partial index can be used; it and
    // the length are the system's own values. A vector of a piece whose text changed since is left out: it
    // says what the piece said before, and its new vector comes with the next turn of the indexer.
    let allowance = filter
        .and_then(|filter| filter.get("exact_filter_candidates"))
        .and_then(Json::as_i64)
        .unwrap_or(0);
    let within: Vec<Id> = filter
        .into_iter()
        .flat_map(|filter| {
            ["scope", "credential_scope"].into_iter().filter_map(|key| {
                filter
                    .get(key)
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse().ok())
            })
        })
        .collect();
    let focus = |key: &str| {
        filter
            .and_then(|filter| filter.get(key))
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
    };
    let document = focus("document_id");
    let chat = focus("chat_id");
    let scope = if document.is_some() || chat.is_some() {
        super::search::sparse::Scope::focused(conn, document, chat, allowance, Some(generation))
            .await?
    } else if let Some(readable) = filter.and_then(|filter| filter.get("readable")) {
        super::search::sparse::Scope::load(conn, readable, &within, allowance, Some(generation))
            .await?
    } else {
        None
    };
    let access = if scope.as_ref().is_some_and(|scope| scope.small) {
        "true".to_owned()
    } else if let Some(scope) = scope.as_ref() {
        format!(
            "((part.kind='document' AND {}) OR (part.kind='conversation' AND {}) OR (part.kind='chat' AND {}))",
            scope.predicate("document", "part.node_id", "part.source_id", 4),
            scope.predicate("conversation", "part.node_id", "part.source_id", 4),
            scope.predicate("chat", "part.node_id", "part.source_id", 4)
        )
    } else {
        super::search::CANDIDATE_ACCESS
            .replace("__ACCESS__", "($4->'readable')")
            .replace("__PATH__", "n.path")
            .replace("__OWNER__", "n.owner_id")
            .replace("__KIND__", "part.kind")
            .replace("__SOURCE__", "part.source_id")
    };
    let source_kind = "CASE part.kind WHEN 'conversation' THEN 'item' WHEN 'chat' THEN 'chat_message' ELSE part.kind END";
    let advanced = if super::search::advanced_source_needed(filter) {
        super::search::advanced_source_sql(
            "$4::jsonb",
            source_kind,
            "part.source_id",
            "part.first_part",
        )
    } else {
        "true".to_owned()
    };
    let files = if super::search::advanced_file_needed(filter) {
        super::search::advanced_file_sql(
            "$4::jsonb",
            source_kind,
            "part.source_id",
            "part.first_part",
            false,
        )
    } else {
        "true".to_owned()
    };
    let shared = if super::search::advanced_shared_needed(filter) {
        super::search::advanced_shared_sql("$4::jsonb", source_kind, "part.source_id")
    } else {
        "true".to_owned()
    };
    let predicate = format!(
        r#"{access} AND {advanced} AND {files} AND {shared} AND ($4::jsonb IS NULL OR (
               ($4->'source_kinds' IS NULL OR part.kind IN (SELECT jsonb_array_elements_text($4->'source_kinds')))
               AND ($4->'item_kinds' IS NULL OR part.kind<>'conversation' OR EXISTS(SELECT 1 FROM items i WHERE i.id=part.first_part AND i.kind IN(SELECT jsonb_array_elements_text($4->'item_kinds'))))
               AND ($4->>'scope' IS NULL OR part.node_id IN (SELECT id FROM nodes WHERE path @> ARRAY[($4->>'scope')::uuid]))
               AND ($4->>'credential_scope' IS NULL OR part.node_id IN (SELECT id FROM nodes WHERE path @> ARRAY[($4->>'credential_scope')::uuid]))
               AND ($4->>'document_id' IS NULL OR (part.kind='document' AND part.source_id=($4->>'document_id')::uuid))
               AND ($4->>'chat_id' IS NULL OR (part.kind='chat' AND part.source_id=($4->>'chat_id')::uuid))
               AND (($4->>'author_id' IS NULL AND $4->>'from' IS NULL AND $4->>'to' IS NULL AND NOT COALESCE(($4->>'with_file')::boolean,false) AND $4->>'mentions_id' IS NULL)
                 OR EXISTS(SELECT 1 FROM search_fields f WHERE f.part_id=part.first_part
                   AND f.kind=CASE part.kind WHEN 'conversation' THEN 'item' WHEN 'chat' THEN 'chat_message' ELSE 'document' END
                   AND ($4->>'author_id' IS NULL OR f.author_id=($4->>'author_id')::uuid)
                   AND ($4->>'date_field' IS NOT NULL OR $4->>'from' IS NULL OR f.at>=($4->>'from')::timestamptz)
                   AND ($4->>'date_field' IS NOT NULL OR $4->>'to' IS NULL OR f.at<=($4->>'to')::timestamptz)
                   AND (NOT COALESCE(($4->>'with_file')::boolean,false) OR f.has_file OR (f.kind='item' AND EXISTS(SELECT 1 FROM items adv_i WHERE adv_i.id=f.part_id AND adv_i.kind='tool_call' AND adv_i.payload->'tool'->>'kind'='image')))
                   AND ($4->>'mentions_id' IS NULL OR CASE f.kind WHEN 'chat_message' THEN f.mentions @> to_jsonb($4->>'mentions_id')
                     ELSE jsonb_path_exists(f.mentions,'$.** ? (@.type == "mention" && @.user_id == $person)',jsonb_build_object('person',$4->>'mentions_id')) END))
             )))"#
    );
    // Rebuilding after shortening can encounter the previous longer tuple versions before vacuum.
    // Project them before the typmod cast, matching the generation's index expression.
    let distance = format!(
        "subvector(v.embedding,1,{dimensions})::halfvec({dimensions}) <=> $1::halfvec({dimensions})"
    );
    let graph = format!("SELECT v.chunk_id,v.text_hash,({distance})::float8 distance
      FROM search_vectors v JOIN search_chunks part ON part.id=v.chunk_id JOIN nodes n ON n.id=part.node_id
      WHERE v.generation_id='{generation}' AND {predicate} ORDER BY {distance} LIMIT $2");
    let (inventory, retrieval) = if let Some(scope) = scope.as_ref().filter(|scope| scope.small) {
        // Planner estimates choose the entrance before ranking; no corpus inventory is counted per call.
        // Retain identifier ordering before top-N so exact distance ties keep their previous input order.
        // Only identifiers reach the vector scan. The chunk's foreign key already guarantees its node exists.
        let bounded = scope.chunks(generation, 4);
        (format!("WITH {},eligible AS MATERIALIZED (SELECT DISTINCT ON(part.id) part.id
          FROM ({bounded})part WHERE {predicate} ORDER BY part.id)",
            super::search::sparse::Scope::declarations(4)),
         format!("SELECT v.chunk_id,v.text_hash,({distance})::float8 distance
          FROM eligible part CROSS JOIN LATERAL (
            SELECT v.* FROM search_vectors v WHERE v.chunk_id=part.id AND v.generation_id='{generation}' OFFSET 0
          ) v ORDER BY ({distance})+0 LIMIT $2"))
    } else {
        (
            scope.as_ref().map_or_else(String::new, |_| {
                format!("WITH {}", super::search::sparse::Scope::declarations(4))
            }),
            graph,
        )
    };
    let mut statement=sqlx::query_as::<_,NearRow>(sqlx::AssertSqlSafe(format!(r#"
      {inventory}
      SELECT near.chunk_id,c.kind,c.source_id,c.first_part,near.distance FROM (
        {retrieval}
      ) near CROSS JOIN LATERAL (
        SELECT * FROM search_chunks c WHERE c.id=near.chunk_id AND c.text_hash=near.text_hash OFFSET 0
      ) c
         JOIN search_sources s ON s.generation_id = c.generation_id AND s.kind = c.kind AND s.source_id = c.source_id
         WHERE CASE c.kind
           WHEN 'document' THEN EXISTS (SELECT 1 FROM documents d WHERE d.id = c.source_id AND d.org_id = c.org_id AND d.deleted_at IS NULL
                                          AND d.doc_seq = s.marker AND d.title = s.title AND d.node_id=c.node_id)
           WHEN 'conversation' THEN EXISTS (SELECT 1 FROM threads t JOIN nodes n ON n.id = t.node_id
                                             WHERE t.node_id = c.source_id AND n.org_id = c.org_id AND t.next_seq = s.marker AND n.name = s.title)
           ELSE EXISTS (SELECT 1 FROM chats ch LEFT JOIN nodes n ON n.id = ch.node_id
                          WHERE ch.id = c.source_id AND ch.org_id = c.org_id AND ch.next_seq = s.marker AND COALESCE(ch.name, n.name, '') = s.title)
         END
          ORDER BY near.distance, near.chunk_id LIMIT $3"#)))
      .persistent(false).bind(vector).bind(keep).bind(keep).bind(filter);
    if let Some(scope) = scope.as_ref() {
        for ids in scope.parameters() {
            statement = statement.bind(ids);
        }
    }
    Ok(statement.fetch_all(conn).await?)
}

/// The bytes one row of `search_vectors` takes beyond the two bytes of each of its numbers: the row's header and
/// line pointer, the generation's and the piece's identifiers, the text hash and the vector's own header, with
/// the two indexes on its identifiers. Measured on pgvector 0.8.7 with 4,617 vectors: 764 bytes for 256 numbers.
pub const VECTOR_ROW_BYTES: i64 = 252;

/// The bytes one entry of a generation's index takes beyond the two bytes of each number of its vector: the
/// entry's header, where its row is, and its neighbours at pgvector's default sixteen connections a layer.
/// Measured on pgvector 0.8.7 with 4,617 vectors: 836 bytes for 256 numbers.
pub const INDEX_ENTRY_BYTES: i64 = 324;

/// The bytes one piece's row takes with its indexes: its identifiers, place, hash, counts and row overhead.
/// The generation-owned layout measured 317.6 bytes per row before its current node UUID (336 rounded up),
/// plus 67.9 bytes for the node index at 1.4 million rows (72 rounded up).
pub const CHUNK_ROW_BYTES: i64 = 408;

/// The number of additional vector rows needed to complete a generation; stale replacements occupy their row already.
pub async fn additional_vectors(conn: &mut Conn, generation: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM search_chunks c WHERE generation_id = $1
        AND NOT EXISTS (SELECT 1 FROM search_vectors v WHERE v.generation_id = $1 AND v.chunk_id = c.id)")
        .bind(generation).fetch_one(conn).await?)
}

/// The bytes one vector of `dimensions` numbers takes with its entry in the generation's index: its numbers in
/// half precision, two bytes each, are kept in both.
#[must_use]
pub fn bytes_per_vector(dimensions: i64) -> i64 {
    let numbers = 2 * dimensions.max(0);
    VECTOR_ROW_BYTES + numbers + INDEX_ENTRY_BYTES + numbers
}

/// Source-row indexes and page slack measured at 178.34 bytes per row over ten thousand ordinary cuts, rounded up.
pub const SOURCE_INDEX_BYTES: i64 = 184;
// A future embedded marker holds one UUID; reserve it before its nullable field is filled.
const SOURCE_MARKER_BYTES: i64 = 16;

/// How much of the database a root org's index takes, and how much text it indexes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sizes {
    /// The bytes the content it indexes takes where it is stored.
    pub content: i64,
    /// The bytes its pieces and vectors take with their indexes, as [`bytes_per_vector`] and
    /// [`CHUNK_ROW_BYTES`] estimate them from what there is.
    pub used: i64,
    /// Source rows and their indexes of the generation being filled; empty-source markers are bookkeeping.
    pub source_bytes: i64,
    /// Nonempty sources in the generation being filled, each reserving one whole-item vector.
    pub sources: i64,
    /// Its pieces.
    pub chunks: i64,
    /// Their estimated tokens.
    pub tokens: i64,
    /// Of those, the tokens of sources without a vector of each piece in the generation being filled.
    pub pending_tokens: i64,
    /// The bytes one more vector of the generation being filled takes.
    pub per_vector: i64,
}

/// The sizes of a root org's index. `generations` are its generations that are not retired, with the length of
/// their vectors; `filling` is the one vectors are being made for. Vectors are counted only where the vectors'
/// table exists (`vectors`).
pub async fn sizes(
    conn: &mut Conn,
    org: Id,
    generations: &[(Id, Option<i32>)],
    filling: Option<Id>,
    vectors: bool,
) -> Result<Sizes, DbError> {
    // Generations may cut a source differently; its stored content is counted once, its pieces once per cut.
    let (content,source_metadata):(i64,i64)=sqlx::query_as(
        "SELECT COALESCE(sum(bytes),0)::bigint,COALESCE(sum(metadata),0)::bigint FROM (
          SELECT max(bytes) bytes,COALESCE(sum(pg_column_size(s)+$2::bigint+CASE WHEN embedded_for IS NULL THEN $3::bigint ELSE 0 END) FILTER(WHERE chunks>0),0) metadata
          FROM search_sources s WHERE org_id=$1 GROUP BY kind,source_id) sources"
    ).bind(org).bind(SOURCE_INDEX_BYTES).bind(SOURCE_MARKER_BYTES).fetch_one(&mut *conn).await?;
    let (chunks, tokens, source_bytes): (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(sum(chunks),0)::bigint,COALESCE(sum(tokens),0)::bigint,
          COALESCE(sum(pg_column_size(s)+$3::bigint+CASE WHEN embedded_for IS NULL THEN $4::bigint ELSE 0 END) FILTER(WHERE chunks>0),0)::bigint
         FROM search_sources s WHERE org_id=$1 AND generation_id=$2",
    )
    .bind(org)
    .bind(filling)
    .bind(SOURCE_INDEX_BYTES)
    .bind(SOURCE_MARKER_BYTES)
    .fetch_one(&mut *conn)
    .await?;
    let all_chunks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM search_chunks WHERE org_id = $1")
            .bind(org)
            .fetch_one(&mut *conn)
            .await?;
    let pending_tokens: i64 = if vectors {
        sqlx::query_scalar("SELECT COALESCE(sum(c.tokens), 0)::bigint FROM search_chunks c
            WHERE c.org_id = $1 AND c.generation_id = $2 AND NOT EXISTS (
                SELECT 1 FROM search_vectors v WHERE v.generation_id = $2 AND v.chunk_id = c.id AND v.text_hash = c.text_hash)")
            .bind(org).bind(filling).fetch_one(&mut *conn).await?
    } else {
        sqlx::query_scalar(
        "SELECT COALESCE(sum(tokens), 0)::bigint FROM search_sources
          WHERE org_id = $1 AND generation_id = $2 AND chunks > 0 AND embedded_for IS DISTINCT FROM $2",
    )
    .bind(org)
    .bind(filling)
    .fetch_one(&mut *conn)
    .await?
    };
    let mut used = source_metadata.saturating_add(all_chunks.saturating_mul(CHUNK_ROW_BYTES));
    let mut per_vector = 0;
    let mut source_count = 0;
    for (generation, dimensions) in generations {
        let each = bytes_per_vector(i64::from(dimensions.unwrap_or(0)));
        // Reserve the whole-item vector with its source, before publishing it. It uses no more than the
        // chunk-vector estimate and must fit even when that source has not finished embedding yet.
        let sources: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM search_sources WHERE generation_id=$1 AND chunks>0",
        )
        .bind(generation)
        .fetch_one(&mut *conn)
        .await?;
        let item_bytes = sources.saturating_mul(each);
        used = used.saturating_add(item_bytes);
        if Some(*generation) == filling {
            per_vector = each;
            source_count = sources;
        }
        if !vectors {
            continue;
        }
        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM search_vectors WHERE generation_id = $1")
                .bind(generation)
                .fetch_one(&mut *conn)
                .await?;
        used = used.saturating_add(rows.saturating_mul(each));
    }
    Ok(Sizes {
        content,
        used,
        source_bytes,
        sources: source_count,
        chunks,
        tokens,
        pending_tokens,
        per_vector,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vector_of_256_numbers_takes_what_was_measured_with_its_entry() {
        assert_eq!(bytes_per_vector(256), 764 + 836);
        assert_eq!(bytes_per_vector(0), VECTOR_ROW_BYTES + INDEX_ENTRY_BYTES);
    }
}
