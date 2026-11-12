//! Durable resource-summary caches and the original accepted refresh journal.
//!
//! Nothing here authorizes a resource read, a model call, or publication. The native producer
//! must materialize the actual resource, restore original/current authority, check current
//! provider Use and caps, and record the governing model inputs. It repeats the source and
//! publication guards in the transaction that calls [`finish`]. No transaction spans model IO.

use crate::{Conn, DbError, Tx};
use chrono::{DateTime, Utc};
use orch_api_contract::summaries::SummaryTarget;
use orch_types::{Clock, Id, Json, new_id};

macro_rules! cache_columns {
    () => {
        "id,kind,node_id,document_id,chat_id,viewer_id,own_options,options_revision,options_updated_by,\
         text,producer,source_snapshot,source_refs,completed_request_id,completed_at,revision,\
         dirty_revision,completed_dirty_revision,created_at,updated_at"
    };
}

macro_rules! request_columns {
    () => {
        "id,summary_id,origin,requested_by,paying_user,client_id,accepted_operation,arguments,accepted_source,prepared,accepted_revision,\
         options_revision,dirty_revision,state,lease_token,lease_until,problem,created_at,due_at,finished_at"
    };
}

/// Private persistence, deliberately not a serializable API response. Source/producer metadata
/// can contain private references and needs the ordinary current-resource projection.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Cache {
    /// Cache identity.
    pub id: Id,
    /// One generated target kind.
    pub kind: String,
    /// Actual node for a thread/topic, never a document's containing-node guess.
    pub node_id: Option<Id>,
    /// Actual document, independently governed.
    pub document_id: Option<Id>,
    /// Actual chat.
    pub chat_id: Option<Id>,
    /// Original requesting reader; None is the shared automatic cache.
    pub viewer_id: Option<Id>,
    /// Explicit own options on the shared resource row, or inheritance.
    pub own_options: Option<Json>,
    /// Compare-and-set version for options.
    pub options_revision: i64,
    /// Writer of the own options, never an invented requester.
    pub options_updated_by: Option<Id>,
    /// Conformed paragraph, independent of every description.
    pub text: Option<String>,
    /// Private producer metadata, projected through current visibility later.
    pub producer: Option<Json>,
    /// Actual material/cursor/settings fingerprint at completion.
    pub source_snapshot: Option<Json>,
    /// Every governing source that must still be readable before exposing the paragraph.
    pub source_refs: Json,
    /// Stable completed native request identity.
    pub completed_request_id: Option<Id>,
    /// Completion time.
    pub completed_at: Option<DateTime<Utc>>,
    /// Observable cache version.
    pub revision: i64,
    /// Local invalidation sequence, not an authority or complete source fingerprint.
    pub dirty_revision: i64,
    /// Sequence represented by the stored paragraph.
    pub completed_dirty_revision: Option<i64>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last observable change.
    pub updated_at: DateTime<Utc>,
}

/// Private durable work; only trusted captured-operation restoration may consume its origin.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Request {
    /// Stable identity retained across worker leases/restarts.
    pub id: Id,
    /// Its cache.
    pub summary_id: Id,
    /// Automatic or explicitly person-issued.
    pub origin: String,
    /// The actual admitted issuer, separate from the automatic setting's writer.
    pub requested_by: Option<Id>,
    /// Paying/Use identity; deleted identities do not acquire a replacement payer.
    pub paying_user: Option<Id>,
    /// A person's original idempotency identity.
    pub client_id: Option<Id>,
    /// Exact captured operation, serialized by the trusted handler, never from request JSON.
    pub accepted_operation: Option<Json>,
    /// Original normalized submitted arguments; a range appears here only if explicitly
    /// submitted. The server's original capture lives separately in accepted_source.
    pub arguments: Json,
    /// Server-captured native boundary, notably the reader's original chat range. This is
    /// separate from caller arguments so replay cannot repin an omitted range after a read mark.
    pub accepted_source: Json,
    /// Trusted producer's prepared source and route state; no credentials.
    pub prepared: Option<Json>,
    /// Monotonic acceptance order in this cache, independent of equal/wall-clock timestamps.
    pub accepted_revision: i64,
    /// Own-options version admitted for this work.
    pub options_revision: i64,
    /// Invalidation sequence admitted for this work.
    pub dirty_revision: i64,
    /// Durable state.
    pub state: String,
    /// Fresh fencing identity for this worker claim; never reused on reclaim.
    pub lease_token: Option<Id>,
    /// Deadline of that claim.
    pub lease_until: Option<DateTime<Utc>>,
    /// Safe, projected problem only; no raw provider error or source content.
    pub problem: Option<Json>,
    /// Acceptance time.
    pub created_at: DateTime<Utc>,
    /// Original event's paced due time, retained on replay/reclaim rather than a retry timer.
    pub due_at: DateTime<Utc>,
    /// Final disposition time.
    pub finished_at: Option<DateTime<Utc>>,
}

fn selector(target: SummaryTarget) -> (&'static str, Option<Id>, Option<Id>, Option<Id>) {
    match target {
        SummaryTarget::Thread { id } => ("thread", Some(id), None, None),
        SummaryTarget::Topic { id } => ("topic", Some(id), None, None),
        SummaryTarget::Document { id } => ("document", None, Some(id), None),
        SummaryTarget::Chat { id } => ("chat", None, None, Some(id)),
        SummaryTarget::ChatTitle { id } => ("chat_title", None, None, Some(id)),
    }
}

/// Reads only an existing cache. Merely opening a page creates no cache, job or model request.
pub async fn get(
    conn: &mut Conn,
    target: SummaryTarget,
    viewer: Option<Id>,
) -> Result<Option<Cache>, DbError> {
    let (kind, node, document, chat) = selector(target);
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        cache_columns!(),
        " FROM resource_summaries WHERE kind=$1 ",
        "AND node_id IS NOT DISTINCT FROM $2 AND document_id IS NOT DISTINCT FROM $3 ",
        "AND chat_id IS NOT DISTINCT FROM $4 AND viewer_id IS NOT DISTINCT FROM $5"
    ))
    .bind(kind)
    .bind(node)
    .bind(document)
    .bind(chat)
    .bind(viewer)
    .fetch_optional(conn)
    .await?)
}

/// Factual stored rows for one native bounded batch; neither this query nor ids grant any Read.
pub async fn get_many(
    conn: &mut Conn,
    targets: &[SummaryTarget],
    viewer: Id,
) -> Result<Vec<Cache>, DbError> {
    let wanted: Vec<_> = targets
        .iter()
        .map(|target| serde_json::json!({"kind":selector(*target).0,"id":target.id()}))
        .collect();
    Ok(sqlx::query_as(concat!("SELECT ",cache_columns!()," FROM resource_summaries r
        WHERE (viewer_id IS NULL OR viewer_id=$2) AND EXISTS(SELECT 1 FROM jsonb_to_recordset($1) w(kind text,id uuid)
          WHERE w.kind=r.kind AND w.id=COALESCE(r.node_id,r.document_id,r.chat_id))"))
        .bind(serde_json::Value::Array(wanted)).bind(viewer).fetch_all(conn).await?)
}

/// Match this row's actual typed columns, without guessing a resource family from an id.
pub fn is_target(cache: &Cache, target: SummaryTarget) -> bool {
    let (kind, node, document, chat) = selector(target);
    cache.kind == kind
        && cache.node_id == node
        && cache.document_id == document
        && cache.chat_id == chat
}

/// Creates a cache only for a correctly typed live resource, as part of an admitted write/work
/// transaction. A document id can never masquerade as a node, nor a project as a topic.
pub async fn ensure(
    conn: &mut Conn,
    target: SummaryTarget,
    viewer: Option<Id>,
    at: DateTime<Utc>,
) -> Result<Option<Cache>, DbError> {
    let (kind, node, document, chat) = selector(target);
    Ok(sqlx::query_as(concat!(
        "INSERT INTO resource_summaries(id,kind,node_id,document_id,chat_id,viewer_id,created_at,updated_at) ",
        "SELECT $1,$2,$3,$4,$5,$6,$7,$7 WHERE ",
        "($2 IN ('thread','topic') AND EXISTS(SELECT 1 FROM nodes WHERE id=$3 AND kind=$2 AND deleted_at IS NULL)) ",
        "OR ($2='document' AND EXISTS(SELECT 1 FROM documents WHERE id=$4 AND deleted_at IS NULL)) ",
        "OR ($2 IN ('chat','chat_title') AND EXISTS(SELECT 1 FROM chats WHERE id=$5)) ",
        "ON CONFLICT (kind,node_id,document_id,chat_id,viewer_id) DO UPDATE SET id=resource_summaries.id ",
        "RETURNING ", cache_columns!()
    ))
    .bind(new_id()).bind(kind).bind(node).bind(document).bind(chat).bind(viewer).bind(at)
    .fetch_optional(conn).await?)
}

/// Individual options follow the existing document-tree scope prefix, then the ordinary
/// thread/node/settings serialization footprint, before the document/cache writer. Call on
/// the native writer's short action transaction; no lock is retained across model IO.
pub async fn lock_document_options(
    conn: &mut Conn,
    document: Id,
) -> Result<Option<super::documents::DocumentRow>, DbError> {
    use super::{documents, hierarchy, threads};
    documents::containers::lock_document_scope(conn, document).await?;
    let Some(original) = documents::get(conn, document).await? else {
        return Ok(None);
    };
    if hierarchy::node(conn, original.node_id)
        .await?
        .is_some_and(|node| node.kind == "thread")
    {
        threads::tools::lock_threads(conn, &[original.node_id]).await?;
    }
    let mut nodes = hierarchy::with_ancestors(conn, &[original.node_id])
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect::<Vec<_>>();
    nodes.sort_unstable();
    nodes.dedup();
    for node in &nodes {
        hierarchy::lock(conn, *node).await?;
    }
    sqlx::query("SELECT node_id,key FROM settings WHERE key=$1 AND (node_id=ANY($2) OR node_id IS NULL) ORDER BY node_id NULLS FIRST FOR UPDATE")
        .bind(orch_settings_contract::registry::keys::SUMMARY_TARGETS).bind(&nodes).fetch_all(&mut *conn).await?;
    let current = documents::lock(conn, document).await?;
    if current
        .as_ref()
        .is_some_and(|row| row.node_id != original.node_id)
    {
        return Err(DbError::Retry);
    }
    Ok(current)
}

/// Updates a shared target's own options, with a separate options CAS. Authority, inherited
/// locks and schema validation belong to its ordinary settings producer, before this query.
pub async fn configure(
    conn: &mut Conn,
    id: Id,
    expected_revision: i64,
    options: Option<&Json>,
    writer: Id,
    at: DateTime<Utc>,
) -> Result<Option<Cache>, DbError> {
    Ok(sqlx::query_as(concat!(
        "WITH changed AS (UPDATE resource_summaries SET own_options=$3,options_updated_by=$4,options_revision=options_revision+1,",
        "dirty_revision=dirty_revision+1,revision=revision+1,updated_at=$5 ",
        "WHERE id=$1 AND options_revision=$2 AND viewer_id IS NULL AND kind='document' RETURNING ", cache_columns!(),"), ",
        "invalidated AS (UPDATE resource_summaries p SET dirty_revision=p.dirty_revision+1,revision=p.revision+1,updated_at=$5 ",
        "FROM changed c WHERE p.viewer_id IS NOT NULL AND p.kind=c.kind ",
        "AND p.node_id IS NOT DISTINCT FROM c.node_id AND p.document_id IS NOT DISTINCT FROM c.document_id ",
        "AND p.chat_id IS NOT DISTINCT FROM c.chat_id) SELECT ",cache_columns!()," FROM changed"
    ))
    .bind(id).bind(expected_revision).bind(options).bind(writer).bind(at)
    .fetch_optional(conn).await?)
}

/// Trusted origin supplied after ordinary admission. DB cannot validate the opaque captured
/// operation; the engine must use CapturedOperation rather than accepting this JSON from a body.
pub enum Origin<'a> {
    /// A standing setting writer, with no person-issued attribution.
    Automatic {
        /// Actual current setting writer.
        writer: Id,
    },
    /// One admitted original person act, retained unchanged on replay.
    Person {
        /// Actual admitted original requester.
        issuer: Id,
        /// Original client nonce for this act.
        client_id: Id,
        /// Trusted serialized CapturedOperation.
        accepted_operation: &'a Json,
    },
}

/// Result of accepting one request. A reused nonce with different arguments is a conflict;
/// an active request is never silently replaced or given a newer, broader caller.
#[derive(Debug, Clone, PartialEq)]
pub enum Enqueued {
    /// New work accepted.
    New(Request),
    /// This same act already exists (including finished acts).
    Replay(Request),
    /// Another act owns this target's current work.
    Busy,
    /// A nonce was reused for another target or different normalized arguments.
    Conflict,
    /// Cache/viewer does not match the actual origin.
    WrongOrigin,
}

/// Accepts inside the caller's transaction. The advisory key serializes the same person's
/// nonce across different resources; a hash collision only serializes unrelated requests.
/// Cache locking plus the partial unique index keeps at most one active job per cache.
/// `accepted_source` is server materialization, never request-body authority; replay retains
/// its original value. `due_at` is the original event pacing (now for a person's Refresh).
pub async fn enqueue(
    tx: &mut Tx,
    summary: Id,
    origin: Origin<'_>,
    arguments: &Json,
    accepted_source: &Json,
    at: DateTime<Utc>,
    due_at: DateTime<Utc>,
) -> Result<Enqueued, DbError> {
    let (kind, issuer, client, payer, accepted) = match origin {
        Origin::Automatic { writer } => ("automatic", None, None, writer, None),
        Origin::Person {
            issuer,
            client_id,
            accepted_operation,
        } => {
            let key = format!("summary.request:{issuer}:{client_id}");
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(key)
                .execute(&mut **tx)
                .await?;
            let previous: Option<Request> = sqlx::query_as(concat!(
                "SELECT ",
                request_columns!(),
                " FROM resource_summary_requests WHERE requested_by=$1 AND client_id=$2"
            ))
            .bind(issuer)
            .bind(client_id)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some(previous) = previous {
                return Ok(
                    if previous.summary_id == summary && previous.arguments == *arguments {
                        Enqueued::Replay(previous)
                    } else {
                        Enqueued::Conflict
                    },
                );
            }
            (
                "person",
                Some(issuer),
                Some(client_id),
                issuer,
                Some(accepted_operation),
            )
        }
    };
    let cache: Option<Cache> = sqlx::query_as(concat!(
        "SELECT ",
        cache_columns!(),
        " FROM resource_summaries WHERE id=$1 FOR UPDATE"
    ))
    .bind(summary)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(cache) = cache.filter(|cache| cache.viewer_id == issuer) else {
        return Ok(Enqueued::WrongOrigin);
    };
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM resource_summary_requests WHERE summary_id=$1 AND state IN ('queued','running'))"
    ).bind(summary).fetch_one(&mut **tx).await?;
    if active {
        return Ok(Enqueued::Busy);
    }
    let request = sqlx::query_as(concat!(
        "INSERT INTO resource_summary_requests(id,summary_id,origin,requested_by,paying_user,client_id,",
        "accepted_operation,arguments,options_revision,dirty_revision,created_at,accepted_revision,accepted_source,due_at) ",
        "VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING ",request_columns!()
    ))
    .bind(new_id()).bind(summary).bind(kind).bind(issuer).bind(payer).bind(client)
    .bind(accepted).bind(arguments).bind(cache.options_revision).bind(cache.dirty_revision).bind(at)
    .bind(cache.revision+1)
    .bind(accepted_source)
    .bind(due_at)
    .fetch_one(&mut **tx).await?;
    sqlx::query("UPDATE resource_summaries SET revision=revision+1,updated_at=$2 WHERE id=$1")
        .bind(summary)
        .bind(at)
        .execute(&mut **tx)
        .await?;
    Ok(Enqueued::New(request))
}

/// Reads durable work, without treating its original authority as current admission.
pub async fn request(conn: &mut Conn, id: Id) -> Result<Option<Request>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        request_columns!(),
        " FROM resource_summary_requests WHERE id=$1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Active or most recent completed work for a cache, for a properly admitted read projection.
pub async fn latest(conn: &mut Conn, summary: Id) -> Result<Option<Request>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",request_columns!()," FROM resource_summary_requests WHERE summary_id=$1 ORDER BY accepted_revision DESC LIMIT 1"
    )).bind(summary).fetch_optional(conn).await?)
}

macro_rules! recoverable_sources {
    () => {
        " FROM model_call_recovery c \
         JOIN resource_summary_requests r ON r.id=c.call_key \
         JOIN resource_summaries s ON s.id=r.summary_id \
         WHERE c.purpose='summary' AND c.context->>'task'='resource_summary' \
         AND c.state IN ('scheduled','running') AND (c.due_at IS NULL OR c.due_at<=$1) \
         AND r.state='failed' \
         AND NOT EXISTS(SELECT 1 FROM resource_summary_requests newer \
             WHERE newer.summary_id=r.summary_id AND newer.accepted_revision>r.accepted_revision) \
         AND NOT EXISTS(SELECT 1 FROM resource_summary_requests active \
             WHERE active.summary_id=r.summary_id AND active.state IN ('queued','running'))"
    };
}

/// Promote one bounded due page, retaining every original request and recovery identity.
/// The due recovery index supplies a stable drain cursor; promoted requests leave this supplier.
/// Lock caches before requests, matching enqueue/finish, and recheck after that lock boundary.
pub async fn revive(tx: &mut Tx, at: DateTime<Utc>, limit: i64) -> Result<Vec<Id>, DbError> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let selected: Vec<Id> = sqlx::query_scalar(concat!(
        "SELECT r.id",
        recoverable_sources!(),
        " ORDER BY c.due_at NULLS FIRST,c.updated_at,c.call_key,c.node_id \
          LIMIT $2 FOR UPDATE OF s SKIP LOCKED"
    ))
    .bind(at)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    if selected.is_empty() {
        return Ok(selected);
    }
    Ok(sqlx::query_scalar(concat!(
        "WITH current AS (SELECT r.id",
        recoverable_sources!(),
        " AND r.id=ANY($2)) \
         UPDATE resource_summary_requests r SET state='queued',finished_at=NULL,problem=NULL \
         FROM current WHERE r.id=current.id RETURNING r.id"
    ))
    .bind(at)
    .bind(&selected)
    .fetch_all(&mut **tx)
    .await?)
}

/// Claims one queued or expired request. The caller derives its lease deadline from the
/// existing model-command deadline/reconciliation grace, not an independent paid retry timer.
/// A reclaim keeps the original request/call identity and original arguments/authority.
pub async fn claim(
    conn: &mut Conn,
    at: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Option<Request>, DbError> {
    Ok(sqlx::query_as(concat!(
        "WITH next AS (SELECT id FROM resource_summary_requests WHERE $2>$1 AND due_at<=$1 AND ",
        "(state='queued' OR (state='running' AND lease_until<=$1)) ORDER BY due_at,created_at,id ",
        "FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE resource_summary_requests r SET state='running',",
        "lease_token=$3,lease_until=$2 FROM next WHERE r.id=next.id RETURNING ",
        "r.id,r.summary_id,r.origin,r.requested_by,r.paying_user,r.client_id,r.accepted_operation,",
        "r.arguments,r.accepted_source,r.prepared,r.accepted_revision,r.options_revision,r.dirty_revision,r.state,r.lease_token,r.lease_until,",
        "r.problem,r.created_at,r.due_at,r.finished_at"
    )).bind(at).bind(until).bind(new_id()).fetch_optional(conn).await?)
}

/// Persists one preparation before IO. A reclaimed command uses the same preparation rather
/// than silently switching its input/range/model after a possibly completed external effect.
pub async fn prepare(
    conn: &mut Conn,
    id: Id,
    token: Id,
    at: DateTime<Utc>,
    prepared: &Json,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE resource_summary_requests SET prepared=COALESCE(prepared,$4) WHERE id=$1 \
         AND state='running' AND lease_token=$2 AND lease_until>$3 \
         AND (prepared IS NULL OR prepared=$4)",
    )
    .bind(id)
    .bind(token)
    .bind(at)
    .bind(prepared)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// A successful, already conformed producer output. Caller must retain every actual source
/// reference, including restricted documents/chats, for current-read checks on later views.
pub struct Paragraph<'a> {
    /// Nonblank conformed text.
    pub text: &'a str,
    /// Private producer snapshot.
    pub producer: &'a Json,
    /// Source/range/settings fingerprint.
    pub source_snapshot: &'a Json,
    /// Array of actual governing source references.
    pub source_refs: &'a Json,
}

/// Durable final disposition. Stale work cannot overwrite a newer paragraph.
pub enum Outcome<'a> {
    /// Publish after the ordinary current source/authority/provider guard, in this transaction.
    Paragraph(Paragraph<'a>),
    /// Safe failure; retain the previous paragraph.
    Failed(&'a Json),
    /// Source/configuration/authority changed; retain previous text and do not start a new paid act.
    Discarded,
}

/// Finishes one live claim. Lock order is cache then request, matching enqueue. Time is sampled
/// after both waits. This lease/version CAS supplements, never replaces, the native source and
/// original/current authority checks that the engine performs in the same short transaction.
pub async fn finish(
    tx: &mut Tx,
    id: Id,
    token: Id,
    clock: &dyn Clock,
    outcome: Outcome<'_>,
) -> Result<bool, DbError> {
    let summary: Option<Id> =
        sqlx::query_scalar("SELECT summary_id FROM resource_summary_requests WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(summary) = summary else {
        return Ok(false);
    };
    let cache: Cache = sqlx::query_as(concat!(
        "SELECT ",
        cache_columns!(),
        " FROM resource_summaries WHERE id=$1 FOR UPDATE"
    ))
    .bind(summary)
    .fetch_one(&mut **tx)
    .await?;
    let work: Request = sqlx::query_as(concat!(
        "SELECT ",
        request_columns!(),
        " FROM resource_summary_requests WHERE id=$1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    let at = clock.now();
    if work.state != "running"
        || work.lease_token != Some(token)
        || work.lease_until.is_none_or(|until| until <= at)
    {
        return Ok(false);
    }
    let current = cache.options_revision == work.options_revision
        && cache.dirty_revision == work.dirty_revision;
    let (state, problem) = match outcome {
        Outcome::Paragraph(paragraph) if current && work.prepared.is_some() => {
            sqlx::query("UPDATE resource_summaries SET text=$2,producer=$3,source_snapshot=$4,source_refs=$5,\
                completed_request_id=$6,completed_at=$7,completed_dirty_revision=dirty_revision WHERE id=$1")
                .bind(summary).bind(paragraph.text).bind(paragraph.producer).bind(paragraph.source_snapshot)
                .bind(paragraph.source_refs).bind(id).bind(at).execute(&mut **tx).await?;
            ("done", None)
        }
        Outcome::Failed(problem) => ("failed", Some(problem)),
        Outcome::Paragraph(_) | Outcome::Discarded => ("discarded", None),
    };
    sqlx::query("UPDATE resource_summary_requests SET state=$2,problem=$3,finished_at=$4,lease_token=NULL,lease_until=NULL WHERE id=$1")
        .bind(id).bind(state).bind(problem).bind(at).execute(&mut **tx).await?;
    sqlx::query("UPDATE resource_summaries SET revision=revision+1,updated_at=$2 WHERE id=$1")
        .bind(summary)
        .bind(at)
        .execute(&mut **tx)
        .await?;
    Ok(true)
}

/// Invalidates existing caches of this exact native target, without creating or queueing work.
/// The source producer calls this alongside its committed change. Topic dependency collection
/// still uses its ordinary audience-safe materializer, not blind private-child propagation.
pub async fn invalidate(
    conn: &mut Conn,
    target: SummaryTarget,
    at: DateTime<Utc>,
) -> Result<u64, DbError> {
    let (kind, node, document, chat) = selector(target);
    Ok(sqlx::query("UPDATE resource_summaries SET dirty_revision=dirty_revision+1,revision=revision+1,updated_at=$5 \
        WHERE kind=$1 AND node_id IS NOT DISTINCT FROM $2 AND document_id IS NOT DISTINCT FROM $3 AND chat_id IS NOT DISTINCT FROM $4")
        .bind(kind).bind(node).bind(document).bind(chat).bind(at).execute(conn).await?.rows_affected())
}

/// Actual descendant main-thread identities in stable pages. Visibility/restriction checks
/// happen in the native producer before its input budget is spent on a candidate.
pub async fn topic_threads(
    conn: &mut Conn,
    topic: Id,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id \
         WHERE $1=ANY(n.path) AND t.side_of IS NULL AND ($2::uuid IS NULL OR t.node_id>$2) \
         AND NOT EXISTS(SELECT 1 FROM nodes a WHERE a.id=ANY(n.path) AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL)) \
         ORDER BY t.node_id LIMIT $3"
    ).bind(topic).bind(after).bind(limit).fetch_all(conn).await?)
}

/// One current source text with its native identity/version; no resolved reference labels.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct TextSource {
    /// Native newest-first page position, never a guessed timestamp.
    pub ordinal: i64,
    /// Actual block/message identity.
    pub id: Id,
    /// Its last content change.
    pub seq_updated: i64,
    /// Authored source text, still requiring the ordinary redaction producer before a model.
    pub text: String,
    /// Its native time.
    pub at: DateTime<Utc>,
}

// The same native live document tree supplies bounded text and its exact eligible count.
const DOCUMENT_TEXT: &str = "WITH RECURSIVE tree AS (
 SELECT id FROM document_blocks WHERE document_id=$1 AND parent_id IS NULL AND removed_at IS NULL
 UNION ALL SELECT b.id FROM document_blocks b JOIN tree t ON b.parent_id=t.id
 WHERE b.document_id=$1 AND b.removed_at IS NULL)";

/// Recent nonempty live blocks in bounded pages; the retained content sequence is its cursor.
pub async fn document_text(
    conn: &mut Conn,
    document: Id,
    before: Option<(i64, Id)>,
    limit: i64,
) -> Result<Vec<TextSource>, DbError> {
    // Natural inclusive type maxima include every row on the first page. A later
    // page excludes only its exact cursor; the composite index still supplies the seek.
    let cursor = before.unwrap_or((i64::MAX, Id::from_u128(u128::MAX)));
    Ok(sqlx::query_as(include_str!("summaries/document_text.sql"))
        .bind(document)
        .bind(cursor.0)
        .bind(cursor.1)
        .bind(limit)
        .bind(before.is_none())
        .fetch_all(conn)
        .await?)
}

/// Exact eligible block count from the same live document tree; does not load its body text.
pub async fn document_text_total(conn: &mut Conn, document: Id) -> Result<i64, DbError> {
    let sql = format!(
        "{DOCUMENT_TEXT} SELECT count(*) FROM tree t JOIN document_blocks b ON b.id=t.id WHERE btrim(b.text)<>''"
    );
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(document)
        .fetch_one(conn)
        .await?)
}

// No reply/forward dereference, roster, live label or attachment-only message contributes words.
const CHAT_TEXT: &str = "WITH words AS (SELECT m.id,m.seq_updated,m.seq_created,m.created_at AS at,
  (SELECT string_agg(p->>'text', E'\\n' ORDER BY part_index)
   FROM jsonb_array_elements(CASE WHEN jsonb_typeof(m.content)='array' THEN m.content
        ELSE COALESCE(m.content->'parts','[]'::jsonb) END) WITH ORDINALITY AS part(p,part_index)
   WHERE p->>'type'='text') AS text FROM chat_messages m
  WHERE m.chat_id=$1 AND m.kind='message' AND m.deleted_at IS NULL AND m.seq_created>$2 AND m.seq_created<=$3)";

/// Current text candidates in the already validated pinned range, newest first in bounded pages.
pub async fn chat_text(
    conn: &mut Conn,
    chat: Id,
    after: i64,
    through: i64,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<TextSource>, DbError> {
    let sql = format!(
        "{CHAT_TEXT} SELECT id,seq_updated,text,at,seq_created AS ordinal FROM words
      WHERE btrim(COALESCE(text,''))<>'' AND ($4::bigint IS NULL OR seq_created<$4)
      ORDER BY seq_created DESC,id DESC LIMIT $5"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(chat)
        .bind(after)
        .bind(through)
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// Exact eligible message count, with the same source and empty/system/deleted filters.
pub async fn chat_text_total(
    conn: &mut Conn,
    chat: Id,
    after: i64,
    through: i64,
) -> Result<i64, DbError> {
    let sql = format!("{CHAT_TEXT} SELECT count(*) FROM words WHERE btrim(COALESCE(text,''))<>''");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(chat)
        .bind(after)
        .bind(through)
        .fetch_one(conn)
        .await?)
}

/// Metadata only, before native ordinary/scope filtering or an input budget is consumed.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct TopicSource {
    /// Actual latest native thread/chat activity, for newest-first filling.
    pub at: DateTime<Utc>,
    /// Actual native family, from this query's own branches.
    pub kind: String,
    /// Actual thread/chat identity.
    pub id: Id,
    /// Actual governing node, solely for the ordinary restricted-child collector.
    pub place: Id,
}
/// Live ordinary threads and node/public group chats share one native metadata cursor.
/// Private group chats and removed/archived ancestry never enter a topic rollup.
pub async fn topic_sources(
    conn: &mut Conn,
    topic: Id,
    after: Option<(DateTime<Utc>, Id, &str)>,
    limit: i64,
) -> Result<Vec<TopicSource>, DbError> {
    Ok(sqlx::query_as(include_str!("summaries/topic_sources.sql"))
        .bind(topic)
        .bind(after.map(|cursor| cursor.0))
        .bind(after.map(|cursor| cursor.1))
        .bind(after.map(|cursor| cursor.2))
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// One coalesced native source change; no stored permission or model input.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Event {
    /// Native generated family.
    pub kind: String,
    /// Actual resource identity.
    pub target_id: Id,
    /// Selected native event.
    pub event: String,
    /// CAS fence for a later source change.
    pub revision: i64,
    /// Original native change time.
    pub changed_at: DateTime<Utc>,
}
/// Exact native event and existing-cache invalidation in the writer's own transaction.
pub async fn changed(
    conn: &mut Conn,
    target: SummaryTarget,
    event: &str,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let (kind, _, _, _) = selector(target);
    sqlx::query("SELECT orch_resource_summary_event($1,$2,$3,$4)")
        .bind(kind)
        .bind(target.id())
        .bind(event)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}
/// Fair native metadata scan; active targets retain their newer coalesced event for after work ends.
pub async fn events(conn: &mut Conn, limit: i64) -> Result<Vec<Event>, DbError> {
    Ok(sqlx::query_as("SELECT e.kind,e.target_id,e.event,e.revision,e.changed_at FROM resource_summary_events e WHERE NOT EXISTS(SELECT 1 FROM resource_summaries s JOIN resource_summary_requests r ON r.summary_id=s.id WHERE s.viewer_id IS NULL AND s.kind=e.kind AND COALESCE(s.node_id,s.document_id,s.chat_id)=e.target_id AND r.state IN ('queued','running')) ORDER BY e.checked_order,e.changed_at,e.kind,e.target_id,e.event LIMIT $1")
        .bind(limit).fetch_all(conn).await?)
}
/// A successful metadata check consumes only its observed event version. A pending cap/Use
/// check rotates fairly without creating or retrying paid work, even on a stationary clock.
pub async fn checked(conn: &mut Conn, event: &Event, consumed: bool) -> Result<(), DbError> {
    let sql = if consumed {
        "DELETE FROM resource_summary_events WHERE kind=$1 AND target_id=$2 AND event=$3 AND revision=$4"
    } else {
        "UPDATE resource_summary_events SET checked_order=nextval('resource_summary_event_checks') WHERE kind=$1 AND target_id=$2 AND event=$3 AND revision=$4"
    };
    sqlx::query(sql)
        .bind(&event.kind)
        .bind(event.target_id)
        .bind(&event.event)
        .bind(event.revision)
        .execute(conn)
        .await?;
    Ok(())
}

/// Exact immutable native cache association for continuing work, without loading paragraph text.
pub async fn belongs_to(
    conn: &mut Conn,
    summary: Id,
    target: SummaryTarget,
    viewer: Option<Id>,
) -> Result<bool, DbError> {
    let (kind, node, document, chat) = selector(target);
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_summaries WHERE id=$1 AND kind=$2 AND node_id IS NOT DISTINCT FROM $3 AND document_id IS NOT DISTINCT FROM $4 AND chat_id IS NOT DISTINCT FROM $5 AND viewer_id IS NOT DISTINCT FROM $6)")
        .bind(summary).bind(kind).bind(node).bind(document).bind(chat).bind(viewer).fetch_one(conn).await?)
}
