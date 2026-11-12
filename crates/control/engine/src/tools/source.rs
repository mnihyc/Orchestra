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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
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

tokio::task_local! {
    static REPLY_DEADLINE: Option<Time>;
}

pub(super) async fn within_reply_deadline<T>(
    deadline: Option<Time>,
    work: impl std::future::Future<Output = T>,
) -> T {
    REPLY_DEADLINE.scope(deadline, work).await
}

impl Source {
    pub(crate) fn answer(self) -> Id {
        self.answer
    }
    pub(crate) fn same_execution(self, other: Self) -> bool {
        self.thread == other.thread
            && self.turn == other.turn
            && self.segment == other.segment
            && self.continuation == other.continuation
            && self.revision == other.revision
    }
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

    pub(super) fn captured() -> Result<Self, EngineError> {
        SOURCE
            .try_with(|source| *source)
            .map_err(|_| EngineError::StaleToolSource)
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

/// An API-owned managed tool must originate in this exact admitted dispatch,
/// rather than a constructed HTTP user or an unscoped internal callback.
pub async fn check_call(conn: &mut Conn, call: &super::CallContext) -> Result<(), EngineError> {
    let source = SOURCE
        .try_with(|source| *source)
        .map_err(|_| EngineError::StaleToolSource)?;
    if source.thread != call.thread
        || source.turn != call.turn
        || source.answer != crate::ids::tool_result(call.thread, &call.call_ref)
    {
        return Err(EngineError::StaleToolSource);
    }
    check(conn).await?;
    let hash = hash_json(serde_json::json!({"tool":call.tool,"arguments":call.arguments}))?;
    let exact:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_tool_calls WHERE answer_id=$1 AND thread_id=$2 AND turn_id=$3 AND tool=$4 AND arguments_hash=$5 AND NOT retired)")
        .bind(source.answer).bind(call.thread).bind(call.turn).bind(call.tool.name()).bind(hash.as_bytes().as_slice())
        .fetch_one(conn).await.map_err(orch_db::DbError::from)?;
    if !exact {
        return Err(EngineError::ToolCallIdentity);
    }
    Ok(())
}

/// Original accepted callers for a current running tool, plus independent ledger facts.
/// Consumers still check their actual target's current Use and first effect.
#[derive(Debug, Clone)]
pub struct CallOrigins {
    callers: Vec<orch_authz::Caller>,
    actor: Id,
    attribution: orch_db::usage::ledger::Attribution,
}

impl CallOrigins {
    /// Every original author/actor origin remains conjunctive; none replaces another.
    #[must_use]
    pub fn callers(&self) -> &[orch_authz::Caller] {
        &self.callers
    }
    /// The execution actor actually retained by the original input producer.
    #[must_use]
    pub fn actor(&self) -> Id {
        self.actor
    }
    /// The real turn's immutable accounting associations, never a permission.
    #[must_use]
    pub fn attribution(&self) -> &orch_db::usage::ledger::Attribution {
        &self.attribution
    }
}

/// Restore the existing input-authority producer for this exact tool receipt.
/// No caller is constructed from the tool's user id, its arguments or a current browser session.
pub async fn original_callers(
    conn: &mut Conn,
    call: &super::CallContext,
    clock: &dyn orch_types::Clock,
    access: &dyn crate::access::ResourceAccess,
) -> Result<CallOrigins, EngineError> {
    Box::pin(original_callers_with_limits(conn, call, clock, access, &[])).await
}

/// A native consumer retains observed restrictions without replacing the original input producer.
pub(crate) async fn original_callers_with_limits(
    conn: &mut Conn,
    call: &super::CallContext,
    clock: &dyn orch_types::Clock,
    access: &dyn crate::access::ResourceAccess,
    limits: &[orch_authz::CapturedCaller],
) -> Result<CallOrigins, EngineError> {
    check_call(conn, call).await?;
    let thread = threads::thread(conn, call.thread)
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    let turn = threads::turn(conn, call.turn)
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    let input = threads::input(conn, turn.input_id)
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    if turn.state != threads::turn_state::RUNNING
        || turn.interrupted_by.is_some()
        || input.thread_id != thread.node_id
        || input.turn_id != Some(turn.id)
        || !matches!(
            input.state.as_str(),
            threads::input_state::DELIVERING | threads::input_state::DELIVERED
        )
    {
        #[cfg(test)]
        if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
            eprintln!("service_original_source refusal 1");
        }
        return Err(EngineError::StaleToolSource);
    }
    let associated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM model_inputs WHERE node_id=$1 AND segment_id=$2 AND turn_id=$3 AND input_id=$4 AND purpose='input' AND reference=$3::text)")
        .bind(thread.node_id).bind(turn.segment_id).bind(turn.id).bind(input.id)
        .fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if !associated {
        #[cfg(test)]
        if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
            eprintln!("service_original_source refusal 2");
        }
        return Err(EngineError::StaleToolSource);
    }
    // Paid follow-on work cannot turn absent imported provenance into a new owner session.
    if input.submission_authority.is_none() || input.author_id.is_none() {
        #[cfg(test)]
        if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
            eprintln!("service_original_source refusal 3");
        }
        return Err(EngineError::StaleToolSource);
    }
    let attribution = orch_db::usage::ledger::attribution_of_turn(conn, turn.id)
        .await?
        .ok_or_else(|| {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_original_source missing attribution");
            }
            EngineError::StaleToolSource
        })?;
    let mut unit = crate::unit::Unit::new(conn, clock.now());
    let author = crate::inputs::input_author_caller(&mut unit, thread.node_id, &input, clock)
        .await?
        .ok_or_else(|| {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_original_source missing author");
            }
            EngineError::StaleToolSource
        })?;
    let actor = crate::inputs::input_caller(&mut unit, &thread, &input, clock)
        .await?
        .ok_or_else(|| {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_original_source missing actor");
            }
            EngineError::StaleToolSource
        })?;
    let ordinary = |result: Result<(), crate::inputs::InputError>| -> Result<(), EngineError> {
        match result {
            Ok(()) => Ok(()),
            Err(crate::inputs::InputError::Internal(EngineError::Authz(
                orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::NotFound(_)
                | orch_authz::Error::Suspended { .. }
                | orch_authz::Error::Archived { .. },
            ))) => Err(EngineError::StaleToolSource),
            Err(crate::inputs::InputError::Internal(error)) => Err(error),
            Err(_) => Err(EngineError::StaleToolSource),
        }
    };
    let mut callers = Vec::new();
    for (origins, need_provider) in [(&author, false), (&actor, true)] {
        for caller in origins.callers() {
            let caller = if let Some(limit) = limits.get(callers.len()) {
                let restored = limit
                    .restore_current(unit.conn, clock)
                    .await?
                    .ok_or(EngineError::StaleToolSource)?;
                // Input restoration just admitted this origin for its actual current work.
                // Keep that fresh proof under the retained ceiling; a plain credential
                // intersection deliberately discards admission and cannot execute a scoped key.
                if caller
                    .grant()
                    .is_some_and(|grant| grant.operation_scope().is_some())
                {
                    restored
                        .narrow_current_admission(caller)
                        .map_err(|_| EngineError::StaleToolSource)?
                } else {
                    caller
                        .intersect_restrictions(&restored)
                        .map_err(EngineError::unreadable)?
                }
            } else {
                if !limits.is_empty() {
                    return Err(EngineError::StaleToolSource);
                }
                caller.clone()
            };
            ordinary(
                crate::inputs::check_sender_for_turn(
                    &mut unit,
                    &caller,
                    &thread,
                    need_provider,
                    Some(&turn),
                    access,
                )
                .await,
            )
            .inspect_err(|_| {
                #[cfg(test)]
                if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                    eprintln!("service_original_source sender");
                }
            })?;
            if crate::preflight::second_step_missing(unit.conn, caller.user(), thread.node_id)
                .await?
            {
                #[cfg(test)]
                if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                    eprintln!("service_original_source refusal 4");
                }
                return Err(EngineError::StaleToolSource);
            }
            callers.push(caller);
        }
    }
    if !limits.is_empty() && limits.len() != callers.len() {
        return Err(EngineError::StaleToolSource);
    }
    Ok(CallOrigins {
        callers,
        actor: actor.user(),
        attribution,
    })
}

pub(super) fn hash_json(value: Json) -> Result<Hash, EngineError> {
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
    if super::capability_service::managed_tool(call.tool) {
        let latest = Time::new(
            engine.inner.clock.now()
                + chrono::Duration::seconds(
                    orch_agent_contract::loopback::ORCH_TOOL_RESULT_WAIT_SECONDS as i64,
                ),
        );
        let deadline = REPLY_DEADLINE
            .try_with(|deadline| *deadline)
            .ok()
            .flatten()
            .map_or(latest, |deadline| deadline.min(latest));
        sqlx::query("UPDATE thread_tool_calls SET plan=jsonb_set(plan,'{capability_deadline}',$2) WHERE answer_id=$1 AND NOT (plan ? 'capability_deadline')")
            .bind(source.answer).bind(serde_json::to_value(deadline)?).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// The same admitted receipt keeps one original reply budget across every retry and replay.
pub(crate) async fn capability_deadline(
    conn: &mut Conn,
    call: &super::CallContext,
) -> Result<Time, EngineError> {
    check_call(conn, call).await?;
    let value:Option<Json>=sqlx::query_scalar("SELECT plan->'capability_deadline' FROM thread_tool_calls WHERE answer_id=$1 AND NOT retired")
        .bind(crate::ids::tool_result(call.thread,&call.call_ref)).fetch_optional(conn).await.map_err(orch_db::DbError::from)?.flatten();
    value
        .map(serde_json::from_value)
        .transpose()?
        .ok_or(EngineError::StaleToolSource)
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
        .try_with(|source| resource_from_answer(stage, source.answer))
        .unwrap_or(fallback)
}

/// Canonical factual resource identity; callers still need their actual admitted source.
pub(crate) fn resource_from_answer(stage: &str, answer: Id) -> Id {
    crate::ids::derived(
        "tool.source.resource",
        &[answer.as_bytes(), stage.as_bytes()],
    )
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
pub(crate) fn issue<'a>(
    engine: &'a Engine,
    mut command: Outgoing,
) -> futures::future::BoxFuture<'a, Result<Outgoing, EngineError>> {
    Box::pin(async move {
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
            let image_delivery =
                super::image_generation::delivery::seal(engine, &mut tx, &mut command).await?
                    || super::image_generation::inputs::seal(engine, &mut tx, &mut command).await?;
            let actual:Id = sqlx::query_scalar("SELECT environment_id FROM segments WHERE id=$1 AND thread_id=$2 AND continuation_id=$3")
            .bind(source.segment).bind(source.thread).bind(source.continuation).fetch_one(&mut *tx)
            .await.map_err(orch_db::DbError::from)?;
            let provider = match &command.key {
                SerialKey::Environment(environment) if *environment == actual => None,
                SerialKey::Provider(provider)
                    if matches!(
                        command.kind,
                        CommandKind::ProviderService | CommandKind::ProviderGenerate
                    ) && matches!(
                        &command.authority,
                        Some(crate::commands::CommandAuthority::CapabilityService(_))
                    ) =>
                {
                    Some(*provider)
                }
                _ => {
                    return Err(EngineError::unreadable(
                        "An own-tool native effect must keep its actual environment or admitted provider serial key.",
                    ));
                }
            };
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
                if (provider.is_some() || image_delivery)
                    && link::command_authority(&mut tx, command.id).await?
                        != command
                            .authority
                            .as_ref()
                            .map(serde_json::to_value)
                            .transpose()?
                {
                    return Err(EngineError::ToolCallIdentity);
                }
                let metadata: Option<Json> = sqlx::query_scalar(
                    "SELECT download FROM thread_tool_commands WHERE command_id=$1",
                )
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
            let metadata = if command.kind == CommandKind::FsWrite && !image_delivery {
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
            let private = command
                .authority
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?;
            let proposed = link::NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: key.as_deref(),
                deadline_at: deadline,
            };
            let current = if provider.is_some() || image_delivery {
                command
                    .authority
                    .as_ref()
                    .ok_or(EngineError::ToolCallIdentity)?
                    .current_issue_callers(
                        &mut tx,
                        &proposed,
                        engine.inner.clock.as_ref(),
                        engine.inner.access.as_ref(),
                    )
                    .await?
                    .ok_or(EngineError::StaleToolSource)?
            } else {
                Vec::new()
            };
            link::insert_command_with_authority(&mut tx, &proposed, now, private.as_ref()).await?;
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
            WHERE r.machine_id=$1 AND (r.environment_id=s.environment_id OR ($5::uuid IS NOT NULL AND r.provider_id=$5))")
            .bind(command.machine).bind(source.segment).bind(source.thread).bind(source.continuation)
            .bind(provider)
            .fetch_optional(&mut *tx).await.map_err(orch_db::DbError::from)?;
            sqlx::query("INSERT INTO thread_tool_commands(command_id,thread_id,turn_id,segment_id,continuation_id,answer_id,
            runtime_id,retirement_machine_id,download,environment_id,queue_provider_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT(command_id) DO NOTHING")
            .bind(command.id).bind(source.thread).bind(source.turn).bind(source.segment).bind(source.continuation)
            .bind(source.answer).bind(retirement.map(|value|value.0)).bind(retirement.map(|value|value.1))
            .bind(metadata).bind(actual).bind(provider)
            .execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
            let input_blobs: Vec<Hash> = match command.kind {
                CommandKind::FsWrite => vec![serde_json::from_value(command.body["blob"].clone())?],
                CommandKind::ProviderGenerate => {
                    let request: orch_link_contract::commands::ProviderGenerate =
                        serde_json::from_value(command.body.clone())?;
                    request
                        .input
                        .messages
                        .into_iter()
                        .flat_map(|message| message.content)
                        .filter_map(|part| match part {
                            orch_model_contract::generation::GenerationPart::ImageRef {
                                blob,
                                ..
                            } => Some(blob),
                            _ => None,
                        })
                        .collect()
                }
                CommandKind::ProviderService => {
                    let request: orch_link_contract::commands::ProviderService =
                        serde_json::from_value(command.body.clone())?;
                    request
                        .input_images
                        .into_iter()
                        .filter_map(|part| match part {
                            orch_model_contract::generation::GenerationPart::ImageRef {
                                blob,
                                ..
                            } => Some(blob),
                            _ => None,
                        })
                        .collect()
                }
                _ => Vec::new(),
            };
            for hash in input_blobs {
                orch_db::queries::uploads::lock_blobs(&mut tx, &[hash]).await?;
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM blobs WHERE hash=$1)")
                        .bind(hash.as_bytes().as_slice())
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(orch_db::DbError::from)?;
                if !exists {
                    return Err(EngineError::unreadable(
                        "the prepared native input is no longer held by storage; no write was admitted",
                    ));
                }
                sqlx::query("WITH added AS (INSERT INTO thread_tool_command_blobs(command_id,blob) VALUES($1,$2) ON CONFLICT DO NOTHING RETURNING blob) UPDATE blobs b SET pin_count=pin_count+1 FROM added a WHERE b.hash=a.blob")
                    .bind(command.id).bind(hash.as_bytes().as_slice()).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
            }
            if current.iter().any(|(caller, deadline)| {
                caller
                    .as_ref()
                    .is_some_and(|caller| !caller.key_deadline_allows(engine.inner.clock.as_ref()))
                    || deadline.is_some_and(|deadline| engine.inner.clock.time() >= deadline)
            }) {
                return Err(EngineError::StaleToolSource);
            }
            command.deadline = Some(Time::new(saved.deadline_at));
            tx.commit().await.map_err(orch_db::DbError::from)?;
            #[cfg(test)]
            tests::native_admitted(source.thread, source.answer, command.kind).await;
        }
        engine.inner.commands.issue(command.clone()).await?;
        Ok(command)
    })
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
            authority: None,
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
    queue_provider_id: Option<Id>,
    machine_id: Id,
    fence_id: Option<Id>,
    fence_retry_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn native_hold(conn: &mut Conn, id: Id) -> Result<Option<NativeHold>, EngineError> {
    Ok(sqlx::query_as("SELECT e.thread_id,e.environment_id,e.queue_provider_id,c.machine_id,e.fence_id,e.fence_retry_at
        FROM thread_tool_commands e JOIN commands c ON c.id=e.command_id
        WHERE e.command_id=$1 AND NOT e.settled AND COALESCE(c.serial_key,c.body->>'$key')=COALESCE(e.queue_provider_id,e.environment_id)::text
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
/// A successful ordinary machine probe behind the original native serial key proves all
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
        let key = owned.queue_provider_id.map_or_else(
            || SerialKey::Environment(owned.environment_id),
            SerialKey::Provider,
        );
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
pub(crate) fn reap(
    engine: &Engine,
    thread: Option<Id>,
) -> futures::future::BoxFuture<'_, Result<(), EngineError>> {
    Box::pin(async move {
        reconcile_answers(engine, thread).await?;
        let commands: Vec<Id> = {
            let mut conn = engine.db().acquire().await?;
            // Rotate even still-pending receipts. A disconnected older queue cannot consume
            // every bounded sweep while a later completed paid request waits for accounting.
            sqlx::query_scalar(
                "WITH selected AS (
                SELECT e.command_id FROM thread_tool_commands e
                WHERE NOT e.settled AND ($1::uuid IS NULL OR e.thread_id=$1)
                ORDER BY e.check_order,e.command_id LIMIT $2 FOR UPDATE SKIP LOCKED
            ) UPDATE thread_tool_commands e SET check_order=nextval('thread_tool_command_checks')
              FROM selected s WHERE e.command_id=s.command_id RETURNING e.command_id",
            )
            .bind(thread)
            .bind(engine.inner.config.sweep_batch)
            .fetch_all(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?
        };
        for id in commands {
            super::capability_service::reconcile(engine, id).await?;
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
        drop(conn);
        super::capability_service::reap_accepted_results(engine, thread).await?;
        let mut conn = engine.db().acquire().await?;
        sqlx::query("DELETE FROM thread_tool_calls t WHERE t.answer_id IN(
        SELECT a.answer_id FROM thread_tool_calls a WHERE (a.completed OR a.retired)
          AND (a.plan#>>'{owner_read,completed}' IS DISTINCT FROM 'true'
            OR NOT EXISTS(SELECT 1 FROM threads h WHERE h.node_id=a.thread_id))
          AND NOT EXISTS(SELECT 1 FROM commands c WHERE c.id=a.answer_id)
          AND NOT EXISTS(SELECT 1 FROM thread_tool_commands e WHERE e.answer_id=a.answer_id)
          AND NOT EXISTS(SELECT 1 FROM items i WHERE i.thread_id=a.thread_id AND i.kind='tool_call'
              AND i.payload->'tool'->>'kind'='image' AND i.native_ref LIKE 'orch.image_generation:' || a.answer_id::text || ':%')
          AND NOT EXISTS(SELECT 1 FROM thread_tool_service_results r WHERE r.answer_id=a.answer_id)
          AND NOT EXISTS(SELECT 1 FROM turns u WHERE u.id=a.turn_id AND u.state IN('starting','running'))
        ORDER BY a.answer_id LIMIT $1)")
        .bind(engine.inner.config.sweep_batch).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
        Ok(())
    })
}
