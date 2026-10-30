//! Fresh calls act only for their admitted selected turn. Completed answers bypass this gate.
//!
//! SQL effects recheck under the source thread's transaction lock. Native effects first put
//! their ordinary command in the durable queue under that same lock; continuation quiescence
//! includes those commands. External preparation never holds a metadata transaction.

use crate::{
    commands::{Outcome, Outgoing},
    engine::Engine,
    error::EngineError,
};
use orch_db::{
    Conn,
    queries::{link, threads},
};
use orch_link_contract::commands::{CommandKind, MachineProbe, SerialKey};
use orch_types::{Hash, Id, Json, Time};

#[cfg(test)]
pub(crate) mod tests;

pub(crate) mod download_admission;

/// The exact execution that admitted a fresh call, not a reader's current projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Source {
    thread: Id,
    turn: Id,
    segment: Id,
    continuation: Id,
    revision: i64,
    answer: Id,
}

tokio::task_local! {
    static SOURCE: Source;
}

impl Source {
    pub(super) async fn load(
        conn: &mut Conn,
        call: &super::CallContext,
    ) -> Result<(Self, threads::ThreadRow), EngineError> {
        let thread = threads::lock(conn, call.thread)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        let (segment, continuation, machine) = current_turn(conn, call.thread, call.turn)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        let source = Self {
            thread: call.thread,
            turn: call.turn,
            segment,
            continuation,
            revision: thread.execution_revision,
            answer: crate::ids::tool_result(call.thread, &call.call_ref),
        };
        if machine != call.machine || !source.matches(&thread) {
            return Err(EngineError::StaleToolSource);
        }
        Ok((source, thread))
    }

    fn matches(self, thread: &threads::ThreadRow) -> bool {
        thread.deleted_at.is_none()
            && thread.archived_at.is_none()
            && thread.current_turn_id == Some(self.turn)
            && thread.current_segment_id == Some(self.segment)
            && thread.active_continuation_id == self.continuation
            && thread.execution_revision == self.revision
    }

    pub(super) async fn scope<T>(self, work: impl std::future::Future<Output = T>) -> T {
        SOURCE.scope(self, work).await
    }
}

async fn current_turn(
    conn: &mut Conn,
    thread: Id,
    turn: Id,
) -> Result<Option<(Id, Id, Id)>, EngineError> {
    Ok(sqlx::query_as("SELECT s.id,u.continuation_id,s.machine_id FROM turns u
        JOIN segments s ON s.id=u.segment_id AND s.thread_id=u.thread_id AND s.continuation_id=u.continuation_id
        JOIN threads h ON h.node_id=u.thread_id AND h.current_turn_id=u.id AND h.current_segment_id=s.id
            AND h.active_continuation_id=u.continuation_id AND h.agent_id=s.agent_id AND h.provider_id=s.provider_id
            AND h.environment_id=s.environment_id AND h.arrangement=s.arrangement
        JOIN environments e ON e.id=s.environment_id AND e.generation=s.env_generation AND e.deleted_at IS NULL
        JOIN machines m ON m.id=s.machine_id AND m.status='active' AND m.deleted_at IS NULL
        WHERE u.thread_id=$1 AND u.id=$2 AND u.state IN ('starting','running') AND s.native_session_ref IS NOT NULL")
        .bind(thread).bind(turn).fetch_optional(conn).await.map_err(orch_db::DbError::from)?)
}

/// Rechecks the calling turn on the caller's connection. In a write transaction its
/// thread lock remains held through commit; outside a tool call this is a no-op.
/// API hooks for own tools use this same task-local source, never an owner-person grant.
pub async fn check(conn: &mut Conn) -> Result<(), EngineError> {
    let Some(source) = SOURCE.try_with(|source| *source).ok() else {
        return Ok(());
    };
    let current = threads::lock(conn, source.thread)
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    let active = current_turn(conn, source.thread, source.turn)
        .await?
        .is_some_and(|(segment, continuation, _)| {
            segment == source.segment && continuation == source.continuation
        });
    if !source.matches(&current) || !active {
        return Err(EngineError::StaleToolSource);
    }
    Ok(())
}

fn hash_json(value: Json) -> Result<Hash, EngineError> {
    fn canonical(value: Json) -> Json {
        match value {
            Json::Array(values) => Json::Array(values.into_iter().map(canonical).collect()),
            Json::Object(values) => Json::Object(
                values
                    .into_iter()
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect(),
            ),
            value => value,
        }
    }
    Ok(Hash::of(&serde_json::to_vec(&canonical(value))?))
}

/// A bounded logical receipt is admitted after the shared rate/schema checks, before reservations.
/// It retains no supplied text, endpoint or credential, only the canonical argument fingerprint.
pub(crate) async fn admit(engine: &Engine, call: &super::CallContext) -> Result<(), EngineError> {
    let source = SOURCE
        .try_with(|source| *source)
        .map_err(|_| EngineError::StaleToolSource)?;
    let hash = hash_json(serde_json::json!({"tool":call.tool,"arguments":call.arguments}))?;
    let mut tx = engine.db().begin().await?;
    check(&mut tx).await?;
    sqlx::query("INSERT INTO thread_tool_calls(answer_id,thread_id,turn_id,segment_id,continuation_id,execution_revision,tool,arguments_hash)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(answer_id) DO NOTHING")
        .bind(source.answer).bind(source.thread).bind(source.turn).bind(source.segment).bind(source.continuation)
        .bind(source.revision).bind(call.tool.name()).bind(hash.as_bytes().as_slice())
        .execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
    let exact:bool=sqlx::query_scalar("SELECT thread_id=$2 AND turn_id=$3 AND segment_id=$4 AND continuation_id=$5
        AND execution_revision=$6 AND tool=$7 AND arguments_hash=$8 AND NOT retired FROM thread_tool_calls WHERE answer_id=$1")
        .bind(source.answer).bind(source.thread).bind(source.turn).bind(source.segment).bind(source.continuation)
        .bind(source.revision).bind(call.tool.name()).bind(hash.as_bytes().as_slice()).fetch_one(&mut *tx)
        .await.map_err(orch_db::DbError::from)?;
    if !exact {
        return Err(EngineError::ToolCallIdentity);
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Only an actually retained canonical answer releases unfinished-call metadata protection.
pub(crate) async fn answered(engine: &Engine, answer: Id) -> Result<(), EngineError> {
    let mut conn = engine.db().acquire().await?;
    sqlx::query("UPDATE thread_tool_calls SET completed=true WHERE answer_id=$1 AND NOT completed")
        .bind(answer)
        .execute(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Rejoin a canonical queue body retained before a crash or a failed metadata checkpoint.
/// Calls still awaiting an answer cannot starve completed answers out of this bounded cut.
/// An absent/malformed body never releases a receipt; no native command is issued here.
async fn reconcile_answers(engine: &Engine, thread: Option<Id>) -> Result<(), EngineError> {
    let pending: Vec<(Id, Id, Json)> = {
        let mut conn = engine.db().acquire().await?;
        sqlx::query_as(
            "SELECT t.answer_id,t.thread_id,c.body FROM thread_tool_calls t
            JOIN commands c ON c.id=t.answer_id AND c.kind='orch.tool_result'
            WHERE NOT t.completed AND NOT t.retired AND ($1::uuid IS NULL OR t.thread_id=$1)
            ORDER BY t.answer_id LIMIT $2",
        )
        .bind(thread)
        .bind(engine.inner.config.sweep_batch)
        .fetch_all(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?
    };
    for (answer, owner, body) in pending {
        let body: orch_link_contract::commands::OrchToolResult = serde_json::from_value(body)?;
        if body.thread_id != owner || crate::ids::tool_result(owner, &body.call_ref) != answer {
            return Err(EngineError::ToolCallIdentity);
        }
        answered(engine, answer).await?;
    }
    Ok(())
}

/// A native resource belongs to one retained logical call even when that call is carried again.
pub(crate) fn resource_id(stage: &str, fallback: Id) -> Id {
    SOURCE
        .try_with(|source| {
            crate::ids::derived(
                "tool.source.resource",
                &[source.answer.as_bytes(), stage.as_bytes()],
            )
        })
        .unwrap_or(fallback)
}

fn native_stage(command: &Outgoing) -> Result<Option<Hash>, EngineError> {
    if !matches!(
        command.kind,
        CommandKind::FsWrite
            | CommandKind::DisplayOpen
            | CommandKind::DisplayAdopt
            | CommandKind::DisplayClose
            | CommandKind::BrowserOpen
            | CommandKind::BrowserAdopt
            | CommandKind::BrowserClose
            | CommandKind::BrowserProfile
    ) {
        return Ok(None);
    }
    let mut identity = serde_json::Map::new();
    identity.insert("kind".into(), serde_json::json!(command.kind.as_str()));
    for key in [
        "environment_id",
        "display_id",
        "browser_id",
        "path",
        "action",
        "do",
        "name",
        "to",
    ] {
        if let Some(value) = command.body.get(key) {
            identity.insert(key.into(), value.clone());
        }
    }
    hash_json(Json::Object(identity)).map(Some)
}

/// Small resolved defaults may be frozen before external preparation, independently of native results.
pub(crate) async fn value(engine: &Engine, key: &str, proposed: Json) -> Result<Json, EngineError> {
    let Some(source) = SOURCE.try_with(|source| *source).ok() else {
        return Ok(proposed);
    };
    let mut tx = engine.db().begin().await?;
    check(&mut tx).await?;
    let plan: Json =
        sqlx::query_scalar("SELECT plan FROM thread_tool_calls WHERE answer_id=$1 FOR UPDATE")
            .bind(source.answer)
            .fetch_one(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
    if let Some(saved) = plan.get(key) {
        let saved = saved.clone();
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(saved);
    }
    sqlx::query(
        "UPDATE thread_tool_calls SET plan=jsonb_set(plan,ARRAY[$2::text],$3) WHERE answer_id=$1",
    )
    .bind(source.answer)
    .bind(key)
    .bind(&proposed)
    .execute(&mut *tx)
    .await
    .map_err(orch_db::DbError::from)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(proposed)
}

/// A native timeout must leave an admitted effect in the durable queue until its
/// confirmed completion, rather than hiding it from quiescence by controller cancellation.
pub(crate) fn is_tool_call() -> bool {
    SOURCE.try_with(|_| ()).is_ok()
}

#[cfg(test)]
pub(crate) fn current_thread() -> Option<Id> {
    SOURCE.try_with(|source| source.thread).ok()
}

/// Already admitted screen resources may need reducing cleanup after a turn ends.
/// This neither opens a resource nor borrows the selected lane's rights: the exact
/// resource must belong to the original source's environment/generation/machine.
async fn owned_cleanup(
    conn: &mut Conn,
    source: Source,
    command: &Outgoing,
) -> Result<bool, EngineError> {
    let (field, statement) = match command.kind {
        CommandKind::BrowserClose => ("browser_id", "SELECT EXISTS(SELECT 1 FROM browser_sessions b JOIN segments s
            ON s.id=$2 AND s.thread_id=$3 AND s.continuation_id=$4 WHERE b.id=$1
            AND b.thread_id=s.thread_id AND b.opened_by IS NULL AND b.environment_id=s.environment_id
            AND b.env_generation=s.env_generation AND EXISTS(SELECT 1 FROM environments e WHERE e.id=s.environment_id
                AND e.generation=s.env_generation AND e.machine_id=$5))"),
        CommandKind::DisplayClose => ("display_id", "SELECT EXISTS(SELECT 1 FROM display_sessions b JOIN segments s
            ON s.id=$2 AND s.thread_id=$3 AND s.continuation_id=$4 WHERE b.id=$1
            AND b.thread_id=s.thread_id AND b.opened_by IS NULL AND b.environment_id=s.environment_id
            AND b.env_generation=s.env_generation AND EXISTS(SELECT 1 FROM environments e WHERE e.id=s.environment_id
                AND e.generation=s.env_generation AND e.machine_id=$5))"),
        _ => return Ok(false),
    };
    let Some(id) = command
        .body
        .get(field)
        .and_then(orch_types::Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    else {
        return Ok(false);
    };
    Ok(
        sqlx::query_scalar(sqlx::AssertSqlSafe(statement.to_owned()))
            .bind(id)
            .bind(source.segment)
            .bind(source.thread)
            .bind(source.continuation)
            .bind(command.machine)
            .fetch_one(conn)
            .await
            .map_err(orch_db::DbError::from)?,
    )
}

/// Durable native admission is the effect's linearization point. The ordinary queue
/// keeps its protected body, identity and original budget; this table holds no payload.
pub(crate) async fn issue(engine: &Engine, mut command: Outgoing) -> Result<Outgoing, EngineError> {
    if let Ok(source) = SOURCE.try_with(|source| *source) {
        #[cfg(test)]
        tests::prepared_native(source.thread, source.answer).await;
        if let Some(stage) = native_stage(&command)? {
            command.id = crate::ids::derived(
                "tool.source.native-stage",
                &[source.answer.as_bytes(), stage.as_bytes()],
            );
        }
        let mut tx = engine.db().begin().await?;
        match check(&mut tx).await {
            Err(EngineError::StaleToolSource)
                if owned_cleanup(&mut tx, source, &command).await? => {}
            result => result?,
        }
        let SerialKey::Environment(environment) = &command.key else {
            return Err(EngineError::unreadable(
                "An own-tool native effect must keep its environment's serial key.",
            ));
        };
        let actual:Id = sqlx::query_scalar("SELECT environment_id FROM segments WHERE id=$1 AND thread_id=$2 AND continuation_id=$3")
            .bind(source.segment).bind(source.thread).bind(source.continuation).fetch_one(&mut *tx)
            .await.map_err(orch_db::DbError::from)?;
        if actual != *environment {
            return Err(EngineError::StaleToolSource);
        }
        if let Some(saved) = link::command(&mut tx, command.id).await? {
            let exact:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_tool_commands WHERE command_id=$1
                AND thread_id=$2 AND turn_id=$3 AND segment_id=$4 AND continuation_id=$5 AND answer_id=$6)")
                .bind(command.id).bind(source.thread).bind(source.turn).bind(source.segment).bind(source.continuation).bind(source.answer)
                .fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
            if !exact
                || saved.machine_id != command.machine
                || saved.kind != command.kind.as_str()
                || saved.serial_key() != command.key.wire().as_deref()
            {
                return Err(EngineError::ToolCallIdentity);
            }
            let metadata: Option<Json> =
                sqlx::query_scalar("SELECT download FROM thread_tool_commands WHERE command_id=$1")
                    .bind(command.id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(orch_db::DbError::from)?;
            download_admission::retained(metadata)?;
            command.body = saved.body;
            command.deadline = Some(Time::new(saved.deadline_at));
            tx.commit().await.map_err(orch_db::DbError::from)?;
            engine.inner.commands.issue(command.clone()).await?;
            return Ok(command);
        }
        let completed: bool =
            sqlx::query_scalar("SELECT completed FROM thread_tool_calls WHERE answer_id=$1")
                .bind(source.answer)
                .fetch_one(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?;
        if completed && !owned_cleanup(&mut tx, source, &command).await? {
            return Err(EngineError::ToolCallIdentity);
        }
        let metadata = if command.kind == CommandKind::FsWrite {
            download_admission::admit(engine, &mut tx, &command.body).await?
        } else {
            None
        };
        let now = engine.inner.clock.now();
        let deadline = command.deadline.map_or_else(
            || now + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            Time::get,
        );
        let key = command.key.wire();
        link::insert_command(
            &mut tx,
            &link::NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: key.as_deref(),
                deadline_at: deadline,
            },
            now,
        )
        .await?;
        let saved = link::command(&mut tx, command.id)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        if saved.machine_id != command.machine
            || saved.kind != command.kind.as_str()
            || saved.body != command.body
            || saved.serial_key() != key.as_deref()
        {
            return Err(EngineError::unreadable(
                "The native tool command identity already names another command.",
            ));
        }
        let retirement:Option<(Id,Id)>=sqlx::query_as("SELECT r.id,h.machine_id FROM runtimes r
            JOIN hosts h ON h.id=r.host_id JOIN machines m ON m.id=r.machine_id AND m.runtime_id=r.id
            JOIN segments s ON s.id=$2 AND s.thread_id=$3 AND s.continuation_id=$4
            WHERE r.machine_id=$1 AND r.environment_id=s.environment_id")
            .bind(command.machine).bind(source.segment).bind(source.thread).bind(source.continuation)
            .fetch_optional(&mut *tx).await.map_err(orch_db::DbError::from)?;
        sqlx::query("INSERT INTO thread_tool_commands(command_id,thread_id,turn_id,segment_id,continuation_id,answer_id,
            runtime_id,retirement_machine_id,download,environment_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(command_id) DO NOTHING")
            .bind(command.id).bind(source.thread).bind(source.turn).bind(source.segment).bind(source.continuation)
            .bind(source.answer).bind(retirement.map(|value|value.0)).bind(retirement.map(|value|value.1))
            .bind(metadata).bind(environment)
            .execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        if command.kind == CommandKind::FsWrite {
            let hash: Hash = serde_json::from_value(command.body["blob"].clone())?;
            orch_db::queries::uploads::lock_blobs(&mut tx, &[hash]).await?;
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM blobs WHERE hash=$1)")
                    .bind(hash.as_bytes().as_slice())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(orch_db::DbError::from)?;
            if !exists {
                return Err(EngineError::unreadable(
                    "The prepared native input is no longer held by storage; no write was admitted.",
                ));
            }
            sqlx::query("WITH added AS (INSERT INTO thread_tool_command_blobs(command_id,blob) VALUES($1,$2)
                ON CONFLICT DO NOTHING RETURNING blob) UPDATE blobs b SET pin_count=pin_count+1
                FROM added a WHERE b.hash=a.blob")
                .bind(command.id).bind(hash.as_bytes().as_slice()).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        }
        command.deadline = Some(Time::new(saved.deadline_at));
        tx.commit().await.map_err(orch_db::DbError::from)?;
        #[cfg(test)]
        tests::native_admitted(source.thread, source.answer, command.kind).await;
    }
    engine.inner.commands.issue(command.clone()).await?;
    Ok(command)
}

pub(crate) async fn download_state(
    engine: &Engine,
) -> Result<Option<(Outgoing, Json)>, EngineError> {
    let Some(source) = SOURCE.try_with(|source| *source).ok() else {
        return Ok(None);
    };
    let mut conn = engine.db().acquire().await?;
    let row:Option<(Id,Json)>=sqlx::query_as("SELECT e.command_id,e.download FROM thread_tool_commands e JOIN commands c ON c.id=e.command_id
        WHERE e.answer_id=$1 AND e.thread_id=$2 AND e.turn_id=$3 AND e.segment_id=$4 AND e.continuation_id=$5
          AND c.kind='fs.write' AND e.download IS NOT NULL ORDER BY c.created_at,e.command_id LIMIT 1")
        .bind(source.answer).bind(source.thread).bind(source.turn).bind(source.segment).bind(source.continuation)
        .fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let Some((id, metadata)) = row else {
        return Ok(None);
    };
    let row = link::command(&mut conn, id)
        .await?
        .ok_or(EngineError::ToolCallIdentity)?;
    let environment: Id = serde_json::from_value(row.body["environment_id"].clone())?;
    Ok(Some((
        Outgoing {
            id,
            machine: row.machine_id,
            key: SerialKey::Environment(environment),
            kind: CommandKind::FsWrite,
            body: row.body,
            deadline: Some(Time::new(row.deadline_at)),
        },
        metadata,
    )))
}

#[derive(sqlx::FromRow)]
struct NativeHold {
    thread_id: Id,
    environment_id: Id,
    machine_id: Id,
    fence_id: Option<Id>,
    fence_retry_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn native_hold(conn: &mut Conn, id: Id) -> Result<Option<NativeHold>, EngineError> {
    Ok(sqlx::query_as("SELECT e.thread_id,e.environment_id,c.machine_id,e.fence_id,e.fence_retry_at
        FROM thread_tool_commands e JOIN commands c ON c.id=e.command_id
        WHERE e.command_id=$1 AND NOT e.settled AND COALESCE(c.serial_key,c.body->>'$key')=e.environment_id::text
        FOR UPDATE OF e")
        .bind(id).fetch_optional(conn).await.map_err(orch_db::DbError::from)?)
}

async fn settled(engine: &Engine, id: Id, fence: Option<Id>) -> Result<(), EngineError> {
    let mut tx = engine.db().begin().await?;
    let changed = sqlx::query(
        "UPDATE thread_tool_commands SET settled=true WHERE command_id=$1 AND NOT settled
        AND ($2::uuid IS NULL OR fence_id=$2)",
    )
    .bind(id)
    .bind(fence)
    .execute(&mut *tx)
    .await
    .map_err(orch_db::DbError::from)?
    .rows_affected();
    if changed == 1 {
        let raw: Vec<Vec<u8>> = sqlx::query_scalar(
            "DELETE FROM thread_tool_command_blobs WHERE command_id=$1 RETURNING blob",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
        let hashes = raw
            .iter()
            .map(|raw| Hash::from_slice(raw).map_err(EngineError::unreadable))
            .collect::<Result<Vec<_>, _>>()?;
        orch_db::queries::storage::unpin_blobs(&mut tx, &hashes, engine.inner.clock.now()).await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Controller cancellation, expiration and ACK failure do not prove a native handler ended.
/// A successful ordinary machine probe behind the original environment key proves all
/// earlier work in that queue has left it, even after that attachment was retired.
/// This read does not require an environment to exist and uses its own catalogue budget;
/// it cannot extend or reissue the original changing command.
async fn fence(engine: &Engine, id: Id) -> Result<(), EngineError> {
    let observed = {
        let mut conn = engine.db().acquire().await?;
        native_hold(&mut conn, id).await?
    };
    let Some(observed) = observed else {
        return Ok(());
    };
    let outcome = match observed.fence_id {
        Some(fence) => engine
            .inner
            .commands
            .status(fence)
            .await?
            .and_then(|info| info.outcome),
        None => None,
    };
    if matches!(&outcome, Some(Outcome::Succeeded(_))) {
        return settled(engine, id, observed.fence_id).await;
    }
    let now = engine.inner.clock.now();
    let replace = observed.fence_id.is_none()
        || outcome.is_some() && observed.fence_retry_at.is_some_and(|after| now >= after);
    if observed.fence_id.is_some() && outcome.is_some() && !replace {
        return Ok(());
    }
    let command = {
        let mut tx = engine.db().begin().await?;
        // Physical history collection is not proof that native work ended. An existing
        // source thread is locked; an absent one still has its immutable command-owned hold.
        let _ = threads::lock(&mut tx, observed.thread_id).await?;
        let Some(owned) = native_hold(&mut tx, id).await? else {
            return Ok(());
        };
        if owned.fence_id != observed.fence_id || owned.fence_retry_at != observed.fence_retry_at {
            return Ok(());
        }
        let fence_id = if replace {
            owned.fence_id.map_or_else(
                || crate::ids::derived("tool.source.queue-fence", &[id.as_bytes()]),
                crate::commands::busy_retry_id,
            )
        } else {
            owned.fence_id.ok_or(EngineError::StaleToolSource)?
        };
        let key = SerialKey::Environment(owned.environment_id);
        let mut command = Outgoing::new(fence_id, owned.machine_id, key, &MachineProbe {})?;
        if replace {
            let deadline = now
                + chrono::Duration::seconds(i64::from(CommandKind::MachineProbe.deadline_secs()));
            let wire = command.key.wire();
            link::insert_command(
                &mut tx,
                &link::NewCommand {
                    id: fence_id,
                    machine_id: owned.machine_id,
                    kind: CommandKind::MachineProbe.as_str(),
                    body: &command.body,
                    serial_key: wire.as_deref(),
                    deadline_at: deadline,
                },
                now,
            )
            .await?;
            sqlx::query("UPDATE thread_tool_commands SET fence_id=$2,fence_retry_at=$3 WHERE command_id=$1 AND NOT settled")
                .bind(id).bind(fence_id).bind(deadline).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        }
        let saved = link::command(&mut tx, fence_id)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        if saved.machine_id != command.machine
            || saved.kind != command.kind.as_str()
            || saved.body != command.body
            || saved.serial_key() != command.key.wire().as_deref()
        {
            return Err(EngineError::unreadable(
                "The retained native queue fence names another read.",
            ));
        }
        command.deadline = Some(Time::new(saved.deadline_at));
        tx.commit().await.map_err(orch_db::DbError::from)?;
        command
    };
    // No transaction or pooled connection remains while the machine is reached.
    engine.inner.commands.issue(command).await?;
    Ok(())
}

/// An exact successful physical unit removal is also completion proof when its old
/// connector cannot answer. Stop/desire/generation/machine changes are not such proof.
async fn retired(engine: &Engine, id: Id) -> Result<bool, EngineError> {
    let mut conn = engine.db().acquire().await?;
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM thread_tool_commands e JOIN commands c
        ON c.machine_id=e.retirement_machine_id AND c.body->>'runtime_id'=e.runtime_id::text
        AND c.kind='node.runtime.remove' AND c.state='succeeded'
        WHERE e.command_id=$1 AND NOT e.settled AND e.runtime_id IS NOT NULL)",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?)
}

/// Bound settlement by the existing sweep batch. An actual successful result ends its
/// hold directly. Other terminal statuses require a successful read behind its native key;
/// pending outcomes are never guessed from a controller timeout or a missing waiter.
pub(crate) async fn reap(engine: &Engine, thread: Option<Id>) -> Result<(), EngineError> {
    reconcile_answers(engine, thread).await?;
    let commands: Vec<Id> = {
        let mut conn = engine.db().acquire().await?;
        sqlx::query_scalar("SELECT e.command_id FROM thread_tool_commands e
            JOIN commands c ON c.id=e.command_id WHERE NOT e.settled AND ($1::uuid IS NULL OR e.thread_id=$1)
            ORDER BY c.created_at,e.command_id LIMIT $2")
            .bind(thread).bind(engine.inner.config.sweep_batch)
            .fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
    };
    for id in commands {
        if retired(engine, id).await? {
            settled(engine, id, None).await?;
            continue;
        }
        match engine
            .inner
            .commands
            .status(id)
            .await?
            .and_then(|info| info.outcome)
        {
            Some(Outcome::Succeeded(_)) => settled(engine, id, None).await?,
            Some(_) => fence(engine, id).await?,
            None => {}
        }
    }
    let mut conn = engine.db().acquire().await?;
    sqlx::query("UPDATE thread_tool_calls t SET retired=true WHERE t.answer_id IN(
        SELECT a.answer_id FROM thread_tool_calls a WHERE NOT a.retired
          AND NOT EXISTS(SELECT 1 FROM threads h WHERE h.node_id=a.thread_id)
          AND NOT EXISTS(SELECT 1 FROM thread_tool_commands e WHERE e.answer_id=a.answer_id AND NOT e.settled)
        ORDER BY a.answer_id LIMIT $1)")
        .bind(engine.inner.config.sweep_batch).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
    sqlx::query("DELETE FROM thread_tool_calls t WHERE t.answer_id IN(
        SELECT a.answer_id FROM thread_tool_calls a WHERE (a.completed OR a.retired)
          AND NOT EXISTS(SELECT 1 FROM commands c WHERE c.id=a.answer_id)
          AND NOT EXISTS(SELECT 1 FROM thread_tool_commands e WHERE e.answer_id=a.answer_id)
          AND NOT EXISTS(SELECT 1 FROM turns u WHERE u.id=a.turn_id AND u.state IN('starting','running'))
        ORDER BY a.answer_id LIMIT $1)")
        .bind(engine.inner.config.sweep_batch).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
    Ok(())
}
