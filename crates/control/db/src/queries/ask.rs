//! Ask metadata, delivery events and generation journal attached to ordinary threads.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};

/// Metadata of a hidden or kept built-in conversation.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Conversation {
    pub temporary: bool,
    pub id: Id,
    pub user_id: Id,
    pub root_org_id: Id,
    pub home_org_id: Id,
    pub title: String,
    pub context: Option<String>,
    pub model: Option<Json>,
    pub payer_id: Option<Id>,
    pub retention_days: i32,
    pub last_message_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub state: String,
    pub shown: bool,
    pub seq: Seq,
    pub segment: Option<Id>,
    pub turn: Option<Id>,
    pub runner: Option<Id>,
    pub heartbeat: Option<DateTime<Utc>>,
}

const SELECT: &str = "SELECT (t.ask_root_org_id IS NOT NULL) temporary,t.node_id id,n.owner_id user_id,COALESCE(t.ask_root_org_id,t.program_root_org_id) root_org_id,n.org_id home_org_id,n.name title,t.ask_context context,t.ask_model model,t.ask_payer_id payer_id,t.ask_retention_days retention_days,COALESCE(t.ask_last_message_at,n.created_at) last_message_at,n.created_at,COALESCE(t.ask_state,'ready') state,NOT n.unlisted shown,t.next_seq-1 seq,t.current_segment_id segment,t.current_turn_id turn,t.ask_runner_id runner,t.ask_heartbeat_at heartbeat FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.program='orch' AND n.deleted_at IS NULL";

/// Governance facts for native thread, chat, document and private-blob readers.
/// Kept ordinary threads have no temporary Ask boundary.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Governance {
    pub thread_id: Id,
    pub root_org_id: Id,
    pub person_id: Id,
    pub retention_days: i32,
    pub last_message_at: DateTime<Utc>,
    pub root_live: bool,
}

pub async fn governance(conn: &mut Conn, threads: &[Id]) -> Result<Vec<Governance>, DbError> {
    Ok(sqlx::query_as("SELECT t.node_id thread_id,t.ask_root_org_id root_org_id,n.owner_id person_id,t.ask_retention_days retention_days,t.ask_last_message_at last_message_at,(r.deleted_at IS NULL AND r.kind='org') root_live FROM threads t JOIN nodes n ON n.id=t.node_id JOIN nodes r ON r.id=t.ask_root_org_id WHERE t.node_id=ANY($1) AND t.ask_root_org_id IS NOT NULL")
        .bind(threads).fetch_all(conn).await?)
}

/// The declared control-plane program's real governing root, without an agent surrogate.
pub async fn program_root(conn: &mut Conn, thread: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT COALESCE(ask_root_org_id,program_root_org_id) FROM threads WHERE node_id=$1 AND program='orch'")
        .bind(thread).fetch_optional(conn).await?)
}

/// Current built-in binding facts, also available to an authorized retained-node reader.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProgramBinding {
    pub root_org_id: Id,
    pub model: Option<Json>,
}

pub async fn program_binding(
    conn: &mut Conn,
    thread: Id,
) -> Result<Option<ProgramBinding>, DbError> {
    Ok(sqlx::query_as("SELECT COALESCE(ask_root_org_id,program_root_org_id) root_org_id,ask_model model FROM threads WHERE node_id=$1 AND program='orch'")
        .bind(thread).fetch_optional(conn).await?)
}

/// Historical placement of an actual built-in segment, never inferred from missing native bindings.
pub async fn segment_root(conn: &mut Conn, segment: Id) -> Result<Option<Id>, DbError> {
    // A kept thread stays in the same root when its current program changes.
    // The segment's stored declaration remains historical, independently of that binding.
    Ok(sqlx::query_scalar("SELECT COALESCE(t.ask_root_org_id,t.program_root_org_id,n.org_id) FROM segments s JOIN threads t ON t.node_id=s.thread_id JOIN nodes n ON n.id=t.node_id WHERE s.id=$1 AND s.program='orch'")
        .bind(segment).fetch_optional(conn).await?)
}

/// Actual dialogue placement recorded by one built-in history segment.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SegmentBinding {
    pub provider_id: Id,
    pub protocol: String,
    pub model: String,
}

/// Historical provider and interface, independent of the thread's current choice.
pub async fn segment_binding(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
) -> Result<Option<SegmentBinding>, DbError> {
    Ok(sqlx::query_as(
        "SELECT provider_id,program_protocol protocol,program_model model FROM segments WHERE id=$1 AND thread_id=$2 AND program='orch'",
    )
    .bind(segment)
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// Knowledge domains of actual source nodes, independent of their storage homes.
pub async fn knowledge_roots(conn: &mut Conn, nodes: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT COALESCE(ask_root_org_id,program_root_org_id) FROM threads WHERE node_id=ANY($1) AND program='orch' AND COALESCE(ask_root_org_id,program_root_org_id) IS NOT NULL")
        .bind(nodes).fetch_all(conn).await?)
}

/// One conversation, optionally locked for a state transition.
pub async fn get(conn: &mut Conn, id: Id, lock: bool) -> Result<Option<Conversation>, DbError> {
    let suffix = if lock {
        " AND t.node_id=$1 FOR UPDATE OF t"
    } else {
        " AND t.node_id=$1"
    };
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT}{suffix}")))
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Personal history headers; visibility in each root is checked by the API before returning a row.
pub async fn list(
    conn: &mut Conn,
    person: Option<Id>,
    root: Option<Id>,
    before: Option<Id>,
    limit: i64,
) -> Result<Vec<Conversation>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} AND t.ask_root_org_id IS NOT NULL AND ($1::uuid IS NULL OR n.owner_id=$1) AND ($2::uuid IS NULL OR t.ask_root_org_id=$2) AND ($3::uuid IS NULL OR t.node_id<$3) ORDER BY t.node_id DESC LIMIT $4")))
        .bind(person).bind(root).bind(before).bind(limit).fetch_all(conn).await?)
}

/// An unlisted project/topic container, found by a stable caller-supplied identity.
pub async fn system_container(
    conn: &mut Conn,
    id: Id,
    parent: Id,
    kind: &str,
    owner: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    // The stable container can be the first one for two concurrent requests.
    // Serialize its check and create without relying on a unique-conflict retry.
    super::chats::lock_named(conn, &format!("ask.container.{id}")).await?;
    if super::hierarchy::node(conn, id).await?.is_none() {
        super::hierarchy::create(
            conn,
            &super::hierarchy::NewNode {
                id,
                kind,
                parent: Some(parent),
                name: "Ask",
                name_by_person: true,
                description: "",
                owner,
                org: None,
                at,
            },
        )
        .await?;
        sqlx::query("UPDATE nodes SET unlisted=true WHERE id=$1")
            .bind(id)
            .execute(conn)
            .await?;
    }
    Ok(())
}

/// Bind a newly created thread to the declared built-in program without inventing an environment.
pub async fn bind(
    conn: &mut Conn,
    thread: Id,
    root: Id,
    context: Option<&str>,
    model: Option<&Json>,
    retention: i32,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET program='orch',program_root_org_id=$2,ask_root_org_id=$2,ask_context=$3,ask_model=$4,ask_retention_days=$5,ask_last_message_at=$6,ask_state='ready',state='idle',blocked_reason=NULL,blocked_detail=NULL,starting_step=NULL,arrangement='control_plane' WHERE node_id=$1 AND agent_id IS NULL AND environment_id IS NULL")
        .bind(thread).bind(root).bind(context).bind(model).bind(retention).bind(at).execute(&mut *conn).await?;
    sqlx::query("UPDATE nodes SET unlisted=true WHERE id=$1")
        .bind(thread)
        .execute(conn)
        .await?;
    Ok(())
}

/// Begin a built-in segment with the actual API provider and no harness or environment.
pub async fn segment(
    conn: &mut Conn,
    id: Id,
    thread: Id,
    binding: &SegmentBinding,
    person: Id,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query("UPDATE segments SET ended_at=$2,end_reason='switched' WHERE thread_id=$1 AND program='orch' AND ended_at IS NULL").bind(thread).bind(at).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO segments (id,thread_id,ordinal,program,provider_id,program_protocol,program_model,env_generation,arrangement,credential_path,started_by,started_at,seq_updated,continuation_id) SELECT $1,t.node_id,(SELECT COALESCE(max(ordinal),0)+1 FROM segments WHERE thread_id=t.node_id),'orch',$3,$7,$8,0,'control_plane','forwarded',$4,$5,$6,t.active_continuation_id FROM threads t WHERE t.node_id=$2")
        .bind(id).bind(thread).bind(binding.provider_id).bind(person).bind(at).bind(seq).bind(&binding.protocol).bind(&binding.model).execute(&mut *conn).await?;
    sqlx::query("UPDATE threads SET provider_id=$3,current_segment_id=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(id)
        .bind(binding.provider_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Set the selected turn; input time alone extends retention.
pub async fn running(
    conn: &mut Conn,
    thread: Id,
    turn: Id,
    model: Option<&Json>,
    payer: Id,
    context: Option<&str>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET ask_state='running',state='running',seq_state=next_seq-1,current_turn_id=$2,ask_model=$3,ask_payer_id=$4,ask_context=COALESCE($5,ask_context),ask_last_message_at=$6,ask_heartbeat_at=$6 WHERE node_id=$1")
        .bind(thread).bind(turn).bind(model).bind(payer).bind(context).bind(at).execute(conn).await?;
    Ok(())
}

/// One actual control-plane runner owns this turn's heartbeat, not a credential.
pub async fn runner(
    conn: &mut Conn,
    thread: Id,
    turn: Id,
    owner: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE threads SET ask_runner_id=$3,ask_heartbeat_at=$4 WHERE node_id=$1 AND current_turn_id=$2 AND ask_state='running'")
        .bind(thread).bind(turn).bind(owner).bind(at).execute(conn).await?.rows_affected()==1)
}

pub async fn heartbeat(
    conn: &mut Conn,
    thread: Id,
    turn: Id,
    owner: Id,
    at: DateTime<Utc>,
    after: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE threads SET ask_heartbeat_at=$4 WHERE node_id=$1 AND current_turn_id=$2 AND ask_runner_id=$3 AND ask_state='running' AND ask_heartbeat_at>$5")
        .bind(thread).bind(turn).bind(owner).bind(at).bind(after).execute(conn).await?.rows_affected()==1)
}

/// Exact selected turns abandoned by their controller, locked for cleanup in bounded pages.
pub async fn orphaned(
    conn: &mut Conn,
    before: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as("SELECT node_id,current_turn_id FROM threads WHERE program='orch' AND ask_state='running' AND current_turn_id IS NOT NULL AND COALESCE(ask_heartbeat_at,ask_last_message_at)<=$1 ORDER BY COALESCE(ask_heartbeat_at,ask_last_message_at),node_id LIMIT $2 FOR UPDATE SKIP LOCKED")
        .bind(before).bind(limit).fetch_all(conn).await?)
}

/// Finalize only the turn that was actually selected; a stopped/replaced turn cannot reopen itself.
pub async fn finish(conn: &mut Conn, thread: Id, turn: Id) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE threads SET ask_state='ready',state='idle',seq_state=next_seq-1,current_turn_id=NULL,ask_runner_id=NULL,ask_heartbeat_at=NULL WHERE node_id=$1 AND current_turn_id=$2")
        .bind(thread).bind(turn).execute(conn).await?.rows_affected()==1)
}

/// One sequenced event, notified only with its transaction's commit.
pub async fn emit(conn: &mut Conn, thread: Id, body: &Json) -> Result<Seq, DbError> {
    let seq = crate::sequences::next(conn, thread)
        .await?
        .ok_or_else(|| DbError::Reference {
            constraint: "ask_thread".into(),
        })?;
    sqlx::query("INSERT INTO ask_events (thread_id,seq,body) VALUES ($1,$2,$3)")
        .bind(thread)
        .bind(seq)
        .bind(body)
        .execute(&mut *conn)
        .await?;
    crate::sequences::notify(conn, thread, seq).await?;
    Ok(seq)
}

/// A bounded stream replay page.
pub async fn events(
    conn: &mut Conn,
    thread: Id,
    after: Seq,
    limit: i64,
) -> Result<Vec<(Seq, Json)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT seq,body FROM ask_events WHERE thread_id=$1 AND seq>$2 ORDER BY seq LIMIT $3",
    )
    .bind(thread)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Retain an authorized model step before its command may emit streaming text.
#[allow(
    clippy::too_many_arguments,
    reason = "the journal retains the exact native command, actor, source and immutable request together"
)]
pub async fn retain_call(
    conn: &mut Conn,
    id: Id,
    thread: Id,
    turn: Id,
    item: Id,
    root: Id,
    person: Id,
    provider: Id,
    machine: Id,
    request: &Json,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO ask_calls (id,thread_id,turn_id,item_id,root_org_id,person_id,provider_id,machine_id,request,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (id) DO NOTHING")
        .bind(id).bind(thread).bind(turn).bind(item).bind(root).bind(person).bind(provider).bind(machine).bind(request).bind(at).execute(conn).await?;
    Ok(())
}

/// The retained issuer's facts needed to count an actual provider reply,
/// including a reply received after the controller already recorded Stop.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Call {
    pub id: Id,
    pub root_org_id: Id,
    pub thread_id: Id,
    pub turn_id: Id,
    pub segment_id: Id,
    pub person_id: Id,
    pub provider_id: Id,
    pub machine_id: Id,
    pub request: Json,
}

pub async fn call(conn: &mut Conn, id: Id, machine: Id) -> Result<Option<Call>, DbError> {
    Ok(sqlx::query_as("SELECT c.id,c.root_org_id,c.thread_id,c.turn_id,t.segment_id,c.person_id,c.provider_id,c.machine_id,c.request FROM ask_calls c JOIN turns t ON t.id=c.turn_id WHERE c.id=$1 AND c.machine_id=$2")
        .bind(id).bind(machine).fetch_optional(conn).await?)
}

/// A current provider-stream command association. Stored request bytes are not returned to readers.
pub async fn delta(
    conn: &mut Conn,
    provider: Id,
    command: Id,
    offset: i64,
    text: &str,
) -> Result<Option<(Id, Id)>, DbError> {
    let len = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
    let found:Option<(Id,Id)>=sqlx::query_as("UPDATE ask_calls c SET text_offset=text_offset+$4 FROM threads t WHERE c.id=$2 AND c.provider_id=$1 AND c.text_offset=$3 AND NOT c.settled AND t.node_id=c.thread_id AND t.current_turn_id=c.turn_id AND t.ask_state='running' RETURNING c.thread_id,c.item_id")
        .bind(provider).bind(command).bind(offset).bind(len).fetch_optional(&mut *conn).await?;
    if let Some((_, item)) = found {
        sqlx::query("UPDATE items SET payload=jsonb_set(jsonb_set(payload,'{text}',to_jsonb(COALESCE(payload->>'text','')||$2::text)),'{text_len}',to_jsonb($3::bigint+$4::bigint)) WHERE id=$1")
            .bind(item).bind(text).bind(offset).bind(len).execute(conn).await?;
    }
    Ok(found)
}

/// Bind an accepted input to its selected native turn.
pub async fn input_turn(conn: &mut Conn, input: Id, turn: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE thread_inputs SET turn_id=$2 WHERE id=$1")
        .bind(input)
        .bind(turn)
        .execute(conn)
        .await?;
    Ok(())
}

/// Permanent deletion has no second retention window.
pub async fn delete(conn: &mut Conn, thread: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM nodes WHERE id=$1")
        .bind(thread)
        .execute(conn)
        .await?;
    Ok(())
}

/// A kept thread uses the same program and fixed root without temporary retention.
pub async fn bind_kept(
    conn: &mut Conn,
    thread: Id,
    root: Id,
    model: Option<&Json>,
    payer: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET program='orch',program_root_org_id=$2,ask_model=$3,ask_payer_id=$4,ask_state='ready',ask_last_message_at=$5,state='idle',blocked_reason=NULL,blocked_detail=NULL,starting_step=NULL,arrangement='control_plane' WHERE node_id=$1")
        .bind(thread).bind(root).bind(model).bind(payer).bind(at).execute(conn).await?;
    Ok(())
}

/// The original Ask remains a kept history link, with no further inputs admitted.
pub async fn kept(conn: &mut Conn, source: Id, target: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET ask_state='kept',ask_kept_thread_id=$2 WHERE node_id=$1")
        .bind(source)
        .bind(target)
        .execute(conn)
        .await?;
    Ok(())
}

/// Stop cancels only this turn's exact retained commands.
pub async fn calls(conn: &mut Conn, turn: Id) -> Result<Vec<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM ask_calls WHERE turn_id=$1 AND NOT settled ORDER BY id")
            .bind(turn)
            .fetch_all(conn)
            .await?,
    )
}

/// Mark the existing command's accounting and result applied.
pub async fn settle(conn: &mut Conn, call: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE ask_calls SET settled=true WHERE id=$1")
        .bind(call)
        .execute(conn)
        .await?;
    Ok(())
}

/// Read conversation items in one bounded page, also used by the built-in host on resume.
pub async fn items(
    conn: &mut Conn,
    thread: Id,
    before: Option<Seq>,
    limit: i64,
) -> Result<Vec<super::threads::ItemRow>, DbError> {
    let Some(row) = super::threads::thread(conn, thread).await? else {
        return Ok(Vec::new());
    };
    items_in_view(
        conn,
        thread,
        row.active_continuation_id,
        before,
        false,
        limit,
    )
    .await
}

/// Bounded native selected/frozen history, with a pinned view for multi-page readers.
pub async fn items_in_view(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    boundary: Option<Seq>,
    forward: bool,
    limit: i64,
) -> Result<Vec<super::threads::ItemRow>, DbError> {
    super::thread_continuations::reading::items_bounded(
        conn,
        thread,
        view,
        boundary,
        forward,
        Some("main"),
        limit,
    )
    .await
}

/// Message-only pages through the same selected/frozen decoder as native thread readers.
pub async fn messages_in_view(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    before: Option<Seq>,
    limit: i64,
) -> Result<Vec<super::threads::ItemRow>, DbError> {
    super::thread_continuations::reading::messages_bounded(conn, thread, view, before, limit).await
}

/// A logical, immutable tool journal witness; it conveys no authority.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ToolWitness {
    pub witness: String,
    pub call_id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<Json>,
    pub trace: Option<Json>,
}

/// Immutable tool provenance, selected by logical journal identity in the current visible history.
/// Copies preserve this identity in their payload; a provider's reusable call id is not an identity.
pub async fn tool_witnesses(
    conn: &mut Conn,
    thread: Id,
    ids: &[Id],
) -> Result<Vec<ToolWitness>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let Some(row) = super::threads::thread(conn, thread).await? else {
        return Ok(Vec::new());
    };
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let owners = super::thread_continuations::reading::OWNERS;
    let sql = format!("{owners} SELECT i.payload->>'ask_witness' witness,i.payload->>'ask_call_id' call_id,
        i.payload#>>'{{tool,tool}}' name,i.payload#>'{{tool,arguments}}' arguments,i.payload->'ask_trace' trace FROM items i JOIN segments s ON s.id=i.segment_id
        WHERE i.thread_id=$1 AND i.kind='tool_call' AND s.program='orch'
        AND char_length(i.payload->>'ask_witness')=36 AND i.payload->>'ask_witness'=ANY($3::text[])
        AND EXISTS(SELECT 1 FROM owners o WHERE o.owner=i.continuation_id
            AND i.seq_created BETWEEN o.first_seq AND o.last_seq
            AND (i.turn_id IS NULL OR EXISTS(SELECT 1 FROM turns t WHERE t.id=i.turn_id AND t.ordinal BETWEEN o.first_turn AND o.last_turn)))");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(row.active_continuation_id)
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

/// Delete expired Ask nodes directly; cascades erase their thread, items and events in the same act.
pub async fn purge(conn: &mut Conn, at: DateTime<Utc>, limit: i64) -> Result<u64, DbError> {
    Ok(sqlx::query("DELETE FROM nodes WHERE id IN (SELECT t.node_id FROM threads t WHERE t.ask_root_org_id IS NOT NULL AND COALESCE(t.ask_last_message_at,$1)<=$1-make_interval(days=>t.ask_retention_days) ORDER BY t.ask_last_message_at LIMIT $2 FOR UPDATE OF t SKIP LOCKED)")
        .bind(at).bind(limit).execute(conn).await?.rows_affected())
}

/// Each fixed root's current retention policy is resolved once by the worker.
pub async fn roots(conn: &mut Conn) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT ask_root_org_id FROM threads WHERE ask_root_org_id IS NOT NULL ORDER BY ask_root_org_id").fetch_all(conn).await?)
}

/// Clamp stored choices to current root policy at deletion, including after policy edits.
pub async fn purge_root(
    conn: &mut Conn,
    root: Id,
    minimum: i32,
    maximum: i32,
    at: DateTime<Utc>,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query("DELETE FROM nodes WHERE id IN (SELECT t.node_id FROM threads t WHERE t.ask_root_org_id=$1 AND t.ask_last_message_at<=$4-make_interval(days=>GREATEST($2,LEAST($3,t.ask_retention_days))) ORDER BY t.ask_last_message_at,t.node_id LIMIT $5 FOR UPDATE OF t SKIP LOCKED)")
        .bind(root).bind(minimum).bind(maximum).bind(at).bind(limit).execute(conn).await?.rows_affected())
}

/// Preferences alter neither the scope nor the last-message retention clock.
pub async fn preferences(
    conn: &mut Conn,
    thread: Id,
    shown: Option<bool>,
    days: Option<i32>,
) -> Result<(), DbError> {
    if let Some(shown) = shown {
        sqlx::query("UPDATE nodes SET unlisted=NOT $2 WHERE id=$1")
            .bind(thread)
            .bind(shown)
            .execute(&mut *conn)
            .await?;
    }
    if let Some(days) = days {
        sqlx::query("UPDATE threads SET ask_retention_days=$2 WHERE node_id=$1")
            .bind(thread)
            .bind(days)
            .execute(conn)
            .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct File {
    pub name: String,
    pub media_type: String,
    pub size: i64,
}

pub async fn file(
    conn: &mut Conn,
    thread: Id,
    blob: &orch_types::Hash,
) -> Result<Option<File>, DbError> {
    Ok(
        sqlx::query_as("SELECT name,media_type,size FROM ask_files WHERE thread_id=$1 AND blob=$2")
            .bind(thread)
            .bind(blob.as_bytes().as_slice())
            .fetch_optional(conn)
            .await?,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "the ordinary attachment custody writer retains one immutable file and its conversation association"
)]
pub async fn attach(
    conn: &mut Conn,
    thread: Id,
    blob: &orch_types::Hash,
    name: &str,
    media_type: &str,
    size: i64,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let inserted=sqlx::query("INSERT INTO ask_files (thread_id,blob,name,media_type,size,created_at) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
        .bind(thread).bind(blob.as_bytes().as_slice()).bind(name).bind(media_type).bind(size).bind(at).execute(&mut *conn).await?.rows_affected()==1;
    if inserted {
        crate::queries::storage::pin_blobs(conn, &[*blob]).await?;
    }
    Ok(())
}

/// Carry only files actually present in the copied conversation, including forwarded files.
pub async fn copy_files(
    conn: &mut Conn,
    source: Id,
    target: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let hashes:Vec<Vec<u8>>=sqlx::query_scalar("INSERT INTO ask_files(thread_id,blob,name,media_type,size,created_at)
        SELECT $2,f.blob,f.name,f.media_type,f.size,$3 FROM ask_files f WHERE f.thread_id=$1
        AND EXISTS(SELECT 1 FROM items i WHERE i.thread_id=$2 AND i.kind='user_message' AND (
            i.payload->'parts' @> jsonb_build_array(jsonb_build_object('blob',encode(f.blob,'hex')))
            OR i.payload->'parts' @> jsonb_build_array(jsonb_build_object('images',jsonb_build_array(jsonb_build_object('blob',encode(f.blob,'hex')))))
            OR i.payload->'parts' @> jsonb_build_array(jsonb_build_object('attachments',jsonb_build_array(jsonb_build_object('blob',encode(f.blob,'hex')))))))
        ON CONFLICT DO NOTHING RETURNING blob")
        .bind(source).bind(target).bind(at).fetch_all(&mut *conn).await?;
    for hash in hashes {
        sqlx::query("UPDATE blobs SET pin_count=pin_count+1 WHERE hash=$1")
            .bind(hash)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Ends a built-in binding only as part of installing its confirmed native replacement.
/// Historical segments and their fixed knowledge roots remain independently readable.
pub async fn end_program(conn: &mut Conn, thread: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE threads SET program=NULL,ask_root_org_id=NULL,ask_model=NULL,ask_payer_id=NULL,ask_context=NULL,ask_state=NULL,ask_runner_id=NULL,ask_heartbeat_at=NULL WHERE node_id=$1 AND program='orch'")
        .bind(thread).execute(conn).await?;
    Ok(())
}
