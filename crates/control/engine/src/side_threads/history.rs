//! One side's captured source and first native opening. A retry rejoins its
//! original stage/child; another parent selection is never substituted.

use super::*;
use crate::{
    commands::{Outcome, Outgoing},
    sessions::StartSnapshot,
};
use orch_authz::TokenGrant;
use orch_db::{
    Conn,
    queries::{link, source as credentials, thread_history_cuts as cuts},
};
use orch_link_contract::commands::{CommandKind, SessionOpenResult};
use orch_types::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
struct Authorization {
    actor: Id,
    token: Option<Id>,
    scope: Option<crate::continuations::recipe::AuthorizationScope>,
    continuation: Id,
    segment: Id,
}

pub(super) async fn capture(
    unit: &mut Unit<'_>,
    parent: &threads::ThreadRow,
    side: Id,
    through: i32,
    caller: &Caller,
) -> Result<(), EngineError> {
    let at = unit.now();
    let cut = cuts::capture(
        unit.conn,
        &cuts::Capture {
            id: side,
            thread: parent.node_id,
            view: parent.active_continuation_id,
            job: None,
            owner_thread: Some(side),
            segment: parent.current_segment_id,
            execution_revision: parent.execution_revision,
            range: (through > 0).then_some((1, through)),
            at,
        },
    )
    .await?;
    let child = threads::thread(unit.conn, side)
        .await?
        .ok_or_else(|| EngineError::unreadable("The side creation is unavailable."))?;
    let auth = Authorization {
        actor: caller.user(),
        token: caller.grant().and_then(|grant| grant.token()),
        continuation: child.active_continuation_id,
        segment: child
            .current_segment_id
            .ok_or_else(|| EngineError::unreadable("The side has no initial owned segment."))?,
        scope: caller
            .grant()
            .map(|grant| crate::continuations::recipe::AuthorizationScope {
                scopes: grant.scopes().to_strings(),
                subtree: grant.subtree(),
            }),
    };
    if !cuts::keep_artifact(
        unit.conn,
        cut.id,
        "side_authorization",
        &serde_json::to_value(auth)?,
        &[],
    )
    .await?
    {
        return Err(EngineError::unreadable(
            "This side already owns another source authorization.",
        ));
    }
    Ok(())
}

/// One check per initial native admission, not per history row. Original token
/// scope and its currently stored narrowing both apply; passive reading stays View-only.
pub(crate) async fn authorize(
    engine: &Engine,
    conn: &mut Conn,
    side: &threads::ThreadRow,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<cuts::Cut, EngineError> {
    let cut = cuts::cut(conn, side.node_id)
        .await?
        .filter(|cut| {
            cut.owner_thread_id == Some(side.node_id) && Some(cut.thread_id) == side.side_of
        })
        .ok_or_else(|| {
            EngineError::unreadable("This side has no confirmed original history source.")
        })?;
    let auth: Authorization = serde_json::from_value(cut.artifacts["side_authorization"].clone())?;
    if side.active_continuation_id != auth.continuation
        || side.current_segment_id != Some(auth.segment)
    {
        return Err(EngineError::unreadable(
            "This side's selected execution is not its confirmed initial history preparation; the original parent is not substituted.",
        ));
    }
    authorize_source_grant(engine, conn, side, &cut, &auth, now).await?;
    Ok(cut)
}

async fn authorize_source_grant(
    engine: &Engine,
    conn: &mut Conn,
    side: &threads::ThreadRow,
    cut: &cuts::Cut,
    auth: &Authorization,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let present: Option<Id> = sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
        .bind(auth.actor)
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    if present.is_none() {
        return Err(EngineError::unreadable(
            "The side's preparation author is unavailable.",
        ));
    }
    let caller = if let Some(token) = auth.token {
        let row = credentials::job_token(conn, token, auth.actor, now)
            .await?
            .ok_or_else(|| {
                EngineError::unreadable("The side's original credential is no longer valid.")
            })?;
        Caller::token(
            auth.actor,
            TokenGrant::from_stored(&row.scopes, row.node_id)
                .map_err(EngineError::unreadable)?
                .for_token(token),
        )
    } else {
        Caller::session(auth.actor)
    };
    let mut callers = vec![caller];
    if let Some(scope) = &auth.scope {
        callers.push(Caller::token(
            auth.actor,
            TokenGrant::from_stored(&scope.scopes, scope.subtree)
                .map_err(EngineError::unreadable)?,
        ));
    }
    for caller in callers {
        let principal = Principal::load(conn, caller.clone(), Time::new(now)).await?;
        for permission in [
            orch_access_contract::permissions::Permission::ThreadView,
            orch_access_contract::permissions::Permission::ThreadInteract,
        ] {
            if !matches!(
                nodes::check(
                    conn,
                    &principal,
                    cut.thread_id,
                    permission,
                    Act::Use,
                    orch_authz::Policy::default()
                )
                .await?,
                orch_access_contract::decisions::Decision::Allowed { .. }
            ) {
                return Err(EngineError::unreadable(
                    "Current and accepted rights no longer permit preparing this side's original source.",
                ));
            }
        }
        let mut unit = Unit::new(conn, now);
        engine
            .check_sender(&mut unit, &caller, side, true)
            .await
            .map_err(EngineError::unreadable)?;
    }
    Ok(())
}

/// Recovery reads this side's own frozen history under its original and current source grants.
pub(crate) async fn authorize_recovery(
    engine: &Engine,
    conn: &mut Conn,
    side: &threads::ThreadRow,
    recipient: &threads::SegmentRow,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let original = cuts::cut(conn, side.node_id)
        .await?
        .filter(|cut| {
            cut.owner_thread_id == Some(side.node_id) && Some(cut.thread_id) == side.side_of
        })
        .ok_or_else(|| {
            EngineError::unreadable("This side has no confirmed original history source.")
        })?;
    let auth: Authorization =
        serde_json::from_value(original.artifacts["side_authorization"].clone())?;
    if side.current_segment_id != Some(recipient.id)
        || side.active_continuation_id != auth.continuation
        || recipient.thread_id != side.node_id
        || recipient.continuation_id != auth.continuation
    {
        return Err(EngineError::unreadable(
            "The side's actual execution source changed before native admission.",
        ));
    }
    let created = original.artifacts.get("side_native_result")
        .ok_or_else(|| EngineError::unreadable("The side's original native creation has no confirmed result; an observed session alone does not authorize user input or another fork."))?;
    let initial = threads::segment(conn, auth.segment)
        .await?
        .filter(|segment| {
            segment.thread_id == side.node_id && segment.continuation_id == auth.continuation
        })
        .ok_or_else(|| EngineError::unreadable("The side segment is unavailable."))?;
    if created["segment_id"] != initial.id.to_string()
        || initial
            .native_session_ref
            .as_deref()
            .is_none_or(|native| created["native_session_ref"] != native)
        || original.artifacts.get("side_open_fork").is_some()
            && original.artifacts.get("side_native_verified").is_none()
            && original.artifacts.get("side_fork_unsupported").is_none()
    {
        return Err(EngineError::unreadable(
            "The native side child has not passed its complete recorded-prefix readback; no user input is delivered.",
        ));
    }
    let recipe: Option<Json> = sqlx::query_scalar(
        "SELECT agent_history_recovery FROM segments WHERE id=$1 AND thread_id=$2",
    )
    .bind(recipient.id)
    .bind(side.node_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let recipe = recipe.ok_or_else(|| {
        EngineError::unreadable("The retained recovery conversation is unavailable.")
    })?;
    let source: Id = serde_json::from_value(recipe["source_segment"].clone())?;
    let source = threads::segment(conn, source)
        .await?
        .filter(|segment| {
            segment.id != recipient.id
                && segment.thread_id == side.node_id
                && segment.continuation_id == auth.continuation
        })
        .ok_or_else(|| {
            EngineError::unreadable("The retained recovery conversation is unavailable.")
        })?;
    if recipe["history"] != "none" && recipe["done"] != true {
        let cut_id: Id = serde_json::from_value(recipe["cut"].clone())?;
        cuts::cut(conn, cut_id)
            .await?
            .filter(|cut| {
                cut.id == recipient.id
                    && cut.thread_id == side.node_id
                    && cut.owner_thread_id == Some(side.node_id)
                    && cut.continuation_id == auth.continuation
                    && cut.source_segment_id == Some(source.id)
            })
            .ok_or_else(|| {
                EngineError::unreadable("The retained recovery conversation is unavailable.")
            })?;
    }
    authorize_source_grant(engine, conn, side, &original, &auth, now).await
}

/// A durable successful creation can precede SessionOpened/engine publication.
/// Retain its real result and native identity before deciding whether to fork.
pub(crate) async fn recovered(
    engine: &Engine,
    snap: &StartSnapshot,
    cut: &cuts::Cut,
) -> Result<Option<String>, EngineError> {
    let retained = cut
        .artifacts
        .get("side_native_result")
        .map(|result| serde_json::from_value::<Id>(result["command_id"].clone()))
        .transpose()?;
    for slot in ["side_open_fork", "side_open_neutral", "side_open_empty"] {
        let Some(value) = cut.artifacts.get(slot) else {
            continue;
        };
        let original: Outgoing = serde_json::from_value(value.clone())?;
        {
            let mut conn = engine.db().acquire().await?;
            validate_creation(&mut conn, snap, &original).await?;
        }
        if retained.is_some_and(|id| id != original.id) {
            continue;
        }
        if retained.is_none()
            && slot == "side_open_fork"
            && cut.artifacts["side_fork_unsupported"]["command_id"] == original.id.to_string()
        {
            continue;
        }
        let outcome = engine
            .inner
            .commands
            .status(original.id)
            .await?
            .and_then(|info| info.outcome);
        if let Some(outcome) = outcome {
            let retry = finished(engine, snap, &original, &outcome).await?;
            if let Outcome::Succeeded(value) = outcome {
                let opened: SessionOpenResult = serde_json::from_value(value)?;
                let mut tx = engine.db().begin().await?;
                let mut unit = Unit::new(&mut tx, engine.inner.clock.now());
                let side = threads::lock(unit.conn, snap.thread.node_id)
                    .await?
                    .ok_or_else(|| EngineError::unreadable("The retained side is unavailable."))?;
                let segment = threads::segment(unit.conn, snap.segment.id)
                    .await?
                    .filter(|segment| {
                        segment.thread_id == side.node_id
                            && segment.continuation_id == snap.segment.continuation_id
                    })
                    .ok_or_else(|| {
                        EngineError::unreadable("The retained child belongs to another history.")
                    })?;
                if segment
                    .native_session_ref
                    .as_deref()
                    .is_some_and(|native| native != opened.native_session_ref)
                {
                    return Err(EngineError::unreadable(
                        "The side already retains another native child.",
                    ));
                }
                if segment.native_session_ref.is_none() {
                    let seq = unit.stamp(side.node_id).await?;
                    threads::set_native_session_ref(
                        unit.conn,
                        segment.id,
                        &opened.native_session_ref,
                        seq,
                    )
                    .await?;
                }
                let committed = unit.finish().await?;
                engine.commit(tx, committed).await?;
                return Ok(Some(opened.native_session_ref));
            }
            if retry {
                continue;
            }
        }
        return Err(EngineError::unreadable(
            "The original side creation has no confirmed successful outcome; an observed native reference cannot authorize a resume, another child or input delivery.",
        ));
    }
    if retained.is_some() {
        return Err(EngineError::unreadable(
            "The retained side result has no matching original creation command.",
        ));
    }
    Ok(None)
}

/// A session event identifies an observation, not the original command's result.
/// Join its actual retained admission before publishing/replaying that outcome.
async fn validate_creation(
    conn: &mut Conn,
    snap: &StartSnapshot,
    command: &Outgoing,
) -> Result<(), EngineError> {
    let row = link::command(conn, command.id).await?.ok_or_else(|| {
        EngineError::unreadable("The original side command receipt is unavailable.")
    })?;
    let body: orch_link_contract::continuations::SessionOpenFenced =
        serde_json::from_value(command.body.clone())?;
    if command.machine != snap.key.machine
        || command.kind != CommandKind::SessionOpenFenced
        || command.key != orch_link_contract::commands::SerialKey::Thread(snap.thread.node_id)
        || body.thread_id != snap.thread.node_id
        || body.session.thread_id != snap.thread.node_id
        || body.session.segment_id != snap.segment.id
        || row.machine_id != command.machine
        || row.kind != command.kind.as_str()
        || row.body != command.body
        || row.serial_key() != command.key.wire().as_deref()
        || command.deadline != Some(Time::new(row.deadline_at))
    {
        return Err(EngineError::unreadable(
            "The retained side command changed source, body, machine, key or original deadline.",
        ));
    }
    Ok(())
}

fn stage(mode: SessionMode) -> Option<&'static str> {
    match mode {
        SessionMode::Fork => Some("side_open_fork"),
        SessionMode::ImportNeutral => Some("side_open_neutral"),
        SessionMode::New => Some("side_open_empty"),
        _ => None,
    }
}

/// Initial receipt reconciliation includes neutral/empty as well as Fork.
/// SessionOpened alone completes neither that receipt nor native readback.
pub(crate) async fn needs_verification(
    engine: &Engine,
    snap: &StartSnapshot,
) -> Result<bool, EngineError> {
    if snap.thread.side_of.is_none() {
        return Ok(false);
    }
    let mut conn = engine.db().acquire().await?;
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_history_cuts WHERE id=$1
        AND (NOT (artifacts ? 'side_native_result') OR (artifacts ? 'side_open_fork'
          AND NOT (artifacts ? 'side_native_verified') AND NOT (artifacts ? 'side_fork_unsupported'))))")
        .bind(snap.thread.node_id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?)
}

pub(crate) fn opening_id(snap: &StartSnapshot, mode: SessionMode, ordinary: Id) -> Id {
    if snap.thread.side_of.is_some()
        && snap.segment.native_session_ref.is_none()
        && let Some(stage) = stage(mode)
    {
        crate::ids::derived(stage, &[snap.thread.node_id.as_bytes()])
    } else {
        ordinary
    }
}

pub(crate) async fn saved(
    engine: &Engine,
    snap: &StartSnapshot,
    mut command: Outgoing,
) -> Result<Outgoing, EngineError> {
    if snap.thread.side_of.is_none() {
        return Ok(command);
    }
    let mut tx = engine.db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let side = threads::lock(&mut tx, snap.thread.node_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The side was removed."))?;
    let current: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM turns t JOIN thread_inputs i ON i.id=t.input_id
      WHERE t.id=$1 AND t.thread_id=$2 AND t.segment_id=$3 AND t.state='starting' AND t.input_id=$4
        AND t.continuation_id=$5 AND i.continuation_id=$5 AND i.state='delivering')",
    )
    .bind(snap.turn.id)
    .bind(side.node_id)
    .bind(snap.segment.id)
    .bind(snap.input.id)
    .bind(side.active_continuation_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(orch_db::DbError::from)?;
    if !current
        || side.current_turn_id != Some(snap.turn.id)
        || side.current_segment_id != Some(snap.segment.id)
        || side.deleted_at.is_some()
        || side.archived_at.is_some()
        || side.execution_revision != snap.thread.execution_revision
    {
        return Err(EngineError::unreadable(
            "The side's actual execution source changed before native admission.",
        ));
    }
    let empty:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_native_guards WHERE thread_id=$1 AND empty_segment_id=$2 AND ready)")
        .bind(side.node_id).bind(snap.segment.id).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
    if empty {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(command);
    }
    let actual = threads::segment(&mut tx, snap.segment.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The side segment is unavailable."))?;
    if let Some(native) = actual.native_session_ref {
        let cut = cuts::cut(&mut tx, side.node_id).await?.ok_or_else(|| {
            EngineError::unreadable("The observed side has no retained original preparation.")
        })?;
        if cut.artifacts["side_native_result"]["native_session_ref"] != native
            || cut.artifacts["side_native_result"]["segment_id"] != snap.segment.id.to_string()
        {
            return Err(EngineError::unreadable(
                "The observed side reference has no matching original successful creation result.",
            ));
        }
        // A genuine previously successful stage was recovered before metadata publication.
        // Refresh only that child; never fork it again or claim the parent as the child.
        let session = match command.kind {
            CommandKind::SessionOpen => serde_json::from_value::<
                orch_link_contract::commands::SessionOpen,
            >(command.body.clone())?,
            CommandKind::SessionOpenFenced => {
                *serde_json::from_value::<orch_link_contract::continuations::SessionOpenFenced>(
                    command.body.clone(),
                )?
                .session
            }
            _ => {
                return Err(EngineError::unreadable(
                    "The side refresh is not an ordinary native open.",
                ));
            }
        };
        if session.mode != SessionMode::Resume
            || session.native_session_ref.as_deref() != Some(native.as_str())
        {
            return Err(EngineError::unreadable(
                "A retained child already exists; another native child is not created.",
            ));
        }
        command = Outgoing::new(
            command.id,
            command.machine,
            command.key,
            &orch_link_contract::continuations::SessionOpenFenced {
                thread_id: side.node_id,
                session: Box::new(session),
                predecessor: Some(orch_agent_contract::history::NativePrefixSource {
                    thread_id: side.node_id,
                    segment_id: snap.segment.id,
                    native_session_ref: native,
                }),
            },
        )?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(command);
    }
    let cut = authorize(engine, &mut tx, &side, engine.inner.clock.now()).await?;
    let body = match command.kind {
        CommandKind::SessionOpen => serde_json::from_value::<
            orch_link_contract::commands::SessionOpen,
        >(command.body.clone())?,
        CommandKind::SessionOpenFenced => {
            *serde_json::from_value::<orch_link_contract::continuations::SessionOpenFenced>(
                command.body.clone(),
            )?
            .session
        }
        _ => {
            return Err(EngineError::unreadable(
                "The initial side operation is not an ordinary native open.",
            ));
        }
    };
    let slot = stage(body.mode).ok_or_else(|| {
        EngineError::unreadable(
            "An unknown side history outcome cannot start another native conversation.",
        )
    })?;
    command = Outgoing::new(
        command.id,
        command.machine,
        command.key,
        &orch_link_contract::continuations::SessionOpenFenced {
            thread_id: side.node_id,
            session: Box::new(body),
            predecessor: None,
        },
    )?;
    if let Some(old) = cut.artifacts.get(slot) {
        let old: Outgoing = serde_json::from_value(old.clone())?;
        if old.machine != command.machine
            || old.key != command.key
            || old.kind != command.kind
            || old.body != command.body
        {
            return Err(EngineError::unreadable(
                "The original side opening has an unknown outcome under a different configuration; another child is not created.",
            ));
        }
        command = old;
    } else {
        command.deadline = Some(Time::new(
            engine.inner.clock.now()
                + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
        ));
    }
    let deadline = command.deadline.ok_or_else(|| {
        EngineError::unreadable("The retained side opening lost its original deadline.")
    })?;
    let key = command.key.wire();
    link::insert_command(
        &mut tx,
        &link::NewCommand {
            id: command.id,
            machine_id: command.machine,
            kind: command.kind.as_str(),
            body: &command.body,
            serial_key: key.as_deref(),
            deadline_at: deadline.get(),
        },
        engine.inner.clock.now(),
    )
    .await?;
    let row = link::command(&mut tx, command.id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The retained side command is unavailable."))?;
    if row.machine_id != command.machine
        || row.kind != command.kind.as_str()
        || row.body != command.body
        || row.serial_key() != key.as_deref()
        || Time::new(row.deadline_at) != deadline
    {
        return Err(EngineError::unreadable(
            "The retained side native command changed identity or budget.",
        ));
    }
    if !cuts::keep_artifact(&mut tx, cut.id, slot, &serde_json::to_value(&command)?, &[]).await? {
        return Err(EngineError::unreadable(
            "Another opening already owns this side history stage.",
        ));
    }
    cuts::hold_command(&mut tx, cut.id, command.id, command.machine).await?;
    sqlx::query("INSERT INTO thread_native_guards(thread_id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(side.node_id)
        .execute(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(command)
}

/// The first authored input may wait while its native history is prepared. Its
/// accepted token does not outlive a narrower current token at actual delivery.
pub(crate) async fn save_first_turn(
    engine: &Engine,
    snap: &StartSnapshot,
    mut command: Outgoing,
) -> Result<Outgoing, EngineError> {
    if snap.thread.side_of.is_none() {
        return Ok(command);
    }
    let mut tx = engine.db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let side = threads::lock(&mut tx, snap.thread.node_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("The side is unavailable."))?;
    let cut = cuts::cut(&mut tx, side.node_id).await?;
    let initial = cut
        .as_ref()
        .and_then(|cut| cut.artifacts.get("side_initial_input"))
        .and_then(|value| value["input_id"].as_str())
        .and_then(|id| id.parse::<Id>().ok());
    if cut
        .as_ref()
        .is_some_and(|cut| cut.artifacts.get("side_native_result").is_none())
    {
        return Err(EngineError::unreadable(
            "The side's original native creation has no confirmed result; an observed session alone does not authorize user input or another fork.",
        ));
    }
    if cut.as_ref().is_some_and(|cut| {
        cut.artifacts.get("side_open_fork").is_some()
            && cut.artifacts.get("side_native_verified").is_none()
            && cut.artifacts.get("side_fork_unsupported").is_none()
    }) {
        return Err(EngineError::unreadable(
            "The native side child has not passed its complete recorded-prefix readback; no user input is delivered.",
        ));
    }
    if initial != Some(snap.input.id) {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(command);
    }
    if side.current_turn_id != Some(snap.turn.id)
        || side.current_segment_id != Some(snap.segment.id)
        || side.active_continuation_id != snap.input.continuation_id
        || side.execution_revision != snap.thread.execution_revision
        || side.deleted_at.is_some()
        || side.archived_at.is_some()
    {
        return Err(EngineError::unreadable(
            "The side's actual first-input source changed before delivery.",
        ));
    }
    authorize(engine, &mut tx, &side, engine.inner.clock.now()).await?;
    let old = link::command(&mut tx, command.id).await?;
    let deadline = old.as_ref().map_or_else(
        || {
            Time::new(
                engine.inner.clock.now()
                    + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            )
        },
        |row| Time::new(row.deadline_at),
    );
    command.deadline = Some(deadline);
    let key = command.key.wire();
    link::insert_command(
        &mut tx,
        &link::NewCommand {
            id: command.id,
            machine_id: command.machine,
            kind: command.kind.as_str(),
            body: &command.body,
            serial_key: key.as_deref(),
            deadline_at: deadline.get(),
        },
        engine.inner.clock.now(),
    )
    .await?;
    let row = link::command(&mut tx, command.id).await?.ok_or_else(|| {
        EngineError::unreadable("The retained side input command is unavailable.")
    })?;
    if row.machine_id != command.machine
        || row.kind != command.kind.as_str()
        || row.serial_key() != key.as_deref()
        || row.body != command.body
    {
        return Err(EngineError::unreadable(
            "The side's first input command changed identity.",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(command)
}

/// Return true only for a known pre-effect Unsupported fork where the declared
/// neutral importer is available. No other failed/unknown creation may fall back.
pub(crate) async fn finished(
    engine: &Engine,
    snap: &StartSnapshot,
    command: &Outgoing,
    outcome: &Outcome,
) -> Result<bool, EngineError> {
    if snap.thread.side_of.is_none() {
        return Ok(false);
    }
    if let Outcome::Succeeded(value) = outcome {
        let opened: SessionOpenResult = serde_json::from_value(value.clone())?;
        let session = match command.kind {
            CommandKind::SessionOpen => Some(serde_json::from_value::<
                orch_link_contract::commands::SessionOpen,
            >(command.body.clone())?),
            CommandKind::SessionOpenFenced => Some(
                *serde_json::from_value::<orch_link_contract::continuations::SessionOpenFenced>(
                    command.body.clone(),
                )?
                .session,
            ),
            _ => None,
        };
        if let Some(session) = session
            && session.mode == SessionMode::Resume
            && session.native_session_ref.as_deref() != Some(opened.native_session_ref.as_str())
        {
            return Err(EngineError::unreadable(
                "The side resume returned another native conversation.",
            ));
        }
    }
    let mut tx = engine.db().begin().await?;
    let cut = cuts::cut(&mut tx, snap.thread.node_id).await?;
    let Some(cut) = cut else {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(false);
    };
    let admitted = cut
        .artifacts
        .as_object()
        .into_iter()
        .flat_map(|map| map.iter())
        .filter(|(name, _)| name.starts_with("side_open_"))
        .find_map(|(name, value)| {
            serde_json::from_value::<Outgoing>(value.clone())
                .ok()
                .filter(|old| old.id == command.id)
                .map(|old| (name.as_str(), old))
        });
    let Some((slot, original)) = admitted else {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(false);
    };
    if original != *command {
        return Err(EngineError::unreadable(
            "The side outcome does not belong to its retained original command body and deadline.",
        ));
    }
    validate_creation(&mut tx, snap, command).await?;
    let error;
    let ending = match outcome {
        Outcome::Succeeded(value) => link::Ending::Succeeded(value),
        Outcome::Failed(value) => {
            error = serde_json::to_value(value)?;
            link::Ending::Failed(&error)
        }
        Outcome::Cancelled => link::Ending::Cancelled,
        Outcome::Expired => link::Ending::Expired,
    };
    link::finish_command(&mut tx, command.id, &ending, engine.inner.clock.now()).await?;
    let retry = slot == "side_open_fork"
        && snap.descriptor.capabilities.import_neutral
        && matches!(outcome,Outcome::Failed(error) if error.code=="unsupported");
    if retry {
        cuts::settle_command(&mut tx, cut.id, command.id).await?;
        if !cuts::keep_artifact(
            &mut tx,
            cut.id,
            "side_fork_unsupported",
            &json!({"command_id":command.id,"code":"unsupported"}),
            &[],
        )
        .await?
        {
            return Err(EngineError::unreadable(
                "The side's retained fallback receipt changed.",
            ));
        }
    }
    if let Outcome::Succeeded(value) = outcome {
        let opened: SessionOpenResult = serde_json::from_value(value.clone())?;
        if opened.native_session_ref.is_empty()
            || cut.source_native_ref.as_deref() == Some(opened.native_session_ref.as_str())
        {
            return Err(EngineError::unreadable(
                "The side did not open a distinct owned native conversation.",
            ));
        }
        if !cuts::keep_artifact(
            &mut tx,
            cut.id,
            "side_native_result",
            &json!({"command_id":command.id,"segment_id":snap.segment.id,
            "native_session_ref":opened.native_session_ref}),
            &[],
        )
        .await?
        {
            return Err(EngineError::unreadable(
                "This side already retained a different native child.",
            ));
        }
        cuts::settle_command(&mut tx, cut.id, command.id).await?;
    }
    let observed = threads::segment(&mut tx, snap.segment.id)
        .await?
        .and_then(|segment| segment.native_session_ref);
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if let Outcome::Succeeded(value) = outcome {
        let opened: SessionOpenResult = serde_json::from_value(value.clone())?;
        if observed
            .as_deref()
            .is_some_and(|native| native != opened.native_session_ref)
        {
            return Err(EngineError::unreadable(
                "The observed side reference differs from its retained successful child; no replacement or input was admitted.",
            ));
        }
    }
    if slot == "side_open_fork"
        && let Outcome::Succeeded(value) = outcome
    {
        let opened: SessionOpenResult = serde_json::from_value(value.clone())?;
        verify_child(engine, snap, command, &opened.native_session_ref).await?;
    }
    Ok(retry)
}

async fn verify_child(
    engine: &Engine,
    snap: &StartSnapshot,
    creation: &Outgoing,
    native: &str,
) -> Result<(), EngineError> {
    let mut read = Outgoing::new(
        crate::ids::derived("side.verify-native-child", &[creation.id.as_bytes()]),
        creation.machine,
        orch_link_contract::commands::SerialKey::Harness {
            provider_id: snap.provider.id,
            agent_id: snap.agent.id,
        },
        &orch_link_contract::commands::AgentReadSession {
            provider_id: snap.provider.id,
            agent_id: snap.agent.id,
            native_session_ref: native.into(),
            write: true,
        },
    )?;
    read.deadline = creation.deadline;
    let boundary = {
        let mut tx = engine.db().begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let side = threads::lock(&mut tx, snap.thread.node_id)
            .await?
            .ok_or_else(|| {
                EngineError::unreadable("The side was removed before native readback.")
            })?;
        let cut = authorize(engine, &mut tx, &side, engine.inner.clock.now()).await?;
        if side.current_turn_id != Some(snap.turn.id) {
            return Err(EngineError::unreadable(
                "The side's native readback belongs to another starting input.",
            ));
        }
        if threads::segment(&mut tx, snap.segment.id)
            .await?
            .and_then(|segment| segment.native_session_ref)
            .is_some_and(|actual| actual != native)
        {
            return Err(EngineError::unreadable(
                "The side now retains a different native child.",
            ));
        }
        let boundary:crate::fork_history::cut::NativeBoundary=serde_json::from_value(cut.artifacts.get("native_boundary").cloned()
            .ok_or_else(||EngineError::unreadable("The original native prefix witness is unavailable."))?)
            .map_err(|_|EngineError::unreadable("The original side prefix has no complete typed record evidence; its child is retained without replacement."))?;
        if let Some(saved) = cut.artifacts.get("side_verify_child") {
            let old: Outgoing = serde_json::from_value(saved.clone())?;
            if old != read {
                return Err(EngineError::unreadable(
                    "The original side readback changed child, body or budget.",
                ));
            }
            read = old;
        }
        let deadline = read.deadline.ok_or_else(|| {
            EngineError::unreadable("The native child readback lost the original creation budget.")
        })?;
        let key = read.key.wire();
        link::insert_command(
            &mut tx,
            &link::NewCommand {
                id: read.id,
                machine_id: read.machine,
                kind: read.kind.as_str(),
                body: &read.body,
                serial_key: key.as_deref(),
                deadline_at: deadline.get(),
            },
            engine.inner.clock.now(),
        )
        .await?;
        if !cuts::keep_artifact(
            &mut tx,
            cut.id,
            "side_verify_child",
            &serde_json::to_value(&read)?,
            &[],
        )
        .await?
        {
            return Err(EngineError::unreadable(
                "The side retained a different native readback command.",
            ));
        }
        cuts::hold_command(&mut tx, cut.id, read.id, read.machine).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        boundary
    };
    let actual = crate::commands::run(engine.inner.commands.as_ref(), read.clone()).await?;
    {
        let mut tx = engine.db().begin().await?;
        let error;
        let ending = match &actual {
            Outcome::Succeeded(value) => link::Ending::Succeeded(value),
            Outcome::Failed(value) => {
                error = serde_json::to_value(value)?;
                link::Ending::Failed(&error)
            }
            Outcome::Cancelled => link::Ending::Cancelled,
            Outcome::Expired => link::Ending::Expired,
        };
        link::finish_command(&mut tx, read.id, &ending, engine.inner.clock.now()).await?;
        if matches!(&actual, Outcome::Succeeded(_)) {
            cuts::settle_command(&mut tx, snap.thread.node_id, read.id).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
    }
    let value = match &actual {
        Outcome::Succeeded(value) => value.clone(),
        _ => {
            return Err(EngineError::unreadable(
                "The successful side child has no confirmed native record; no replacement fork is created.",
            ));
        }
    };
    let child: orch_link_contract::commands::AgentReadSessionResult =
        serde_json::from_value(value)?;
    crate::fork_history::cut::verify_native_record(engine, &boundary, &child).await?;
    let mut tx = engine.db().begin().await?;
    if let Outcome::Succeeded(value) = &actual {
        link::finish_command(
            &mut tx,
            read.id,
            &link::Ending::Succeeded(value),
            engine.inner.clock.now(),
        )
        .await?;
    }
    cuts::settle_command(&mut tx, snap.thread.node_id, read.id).await?;
    if !cuts::keep_artifact(
        &mut tx,
        snap.thread.node_id,
        "side_native_verified",
        &json!({"creation_id":creation.id,
        "read_id":read.id,"native_session_ref":native}),
        &[],
    )
    .await?
    {
        return Err(EngineError::unreadable(
            "The side verified another native child.",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Closed sides have no starting turn left to reconcile an uncertain first
/// open. Reuse the existing sweep and its bounded batch; never issue that open
/// again. A same-key read or exact physical retirement may end its input hold.
pub(crate) async fn reap_closed(engine: &Engine) -> Result<(), EngineError> {
    let ids: Vec<(Id, Id, Id)> = {
        let mut conn = engine.db().acquire().await?;
        sqlx::query_as("SELECT h.cut_id,h.command_id,cut.owner_thread_id FROM thread_history_cut_commands h
          JOIN thread_history_cuts cut ON cut.id=h.cut_id JOIN nodes n ON n.id=cut.owner_thread_id
          JOIN commands c ON c.id=h.command_id WHERE NOT h.settled AND n.deleted_at IS NOT NULL
            AND c.state IN ('succeeded','failed','expired','cancelled') ORDER BY h.cut_id,h.command_id LIMIT $1")
            .bind(engine.config().sweep_batch).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
    };
    for (cut_id, id, side) in ids {
        let (cut, command) = {
            let mut conn = engine.db().acquire().await?;
            let Some(cut) = cuts::cut(&mut conn, cut_id).await? else {
                continue;
            };
            let command = cut
                .artifacts
                .as_object()
                .into_iter()
                .flat_map(|values| values.iter())
                .filter(|(name, _)| {
                    name.starts_with("side_open_") || name.as_str() == "side_verify_child"
                })
                .filter_map(|(_, value)| serde_json::from_value::<Outgoing>(value.clone()).ok())
                .find(|command| command.id == id)
                .ok_or_else(|| {
                    EngineError::unreadable(
                        "An unsettled side command lost its original creation identity.",
                    )
                })?;
            let row = link::command(&mut conn, id).await?.ok_or_else(|| {
                EngineError::unreadable("The original side native receipt is unavailable.")
            })?;
            let source_key = match command.kind {
                CommandKind::SessionOpenFenced => {
                    command.key == orch_link_contract::commands::SerialKey::Thread(side)
                }
                CommandKind::AgentReadSession => {
                    let body: orch_link_contract::commands::AgentReadSession =
                        serde_json::from_value(command.body.clone())?;
                    command.key
                        == orch_link_contract::commands::SerialKey::Harness {
                            provider_id: body.provider_id,
                            agent_id: body.agent_id,
                        }
                        && cut.artifacts["side_native_result"]["native_session_ref"]
                            == body.native_session_ref
                }
                _ => false,
            };
            if command.machine != row.machine_id
                || command.kind.as_str() != row.kind
                || command.body != row.body
                || !source_key
                || row.serial_key() != command.key.wire().as_deref()
                || command.deadline != Some(Time::new(row.deadline_at))
            {
                return Err(EngineError::unreadable(
                    "The closed side's retained command changed source or budget.",
                ));
            }
            (cut, command)
        };
        let actual = engine
            .inner
            .commands
            .status(id)
            .await?
            .and_then(|info| info.outcome);
        let native = matches!(&actual, Some(Outcome::Succeeded(_)))
            || matches!(&actual,Some(Outcome::Failed(error)) if error.code=="unsupported");
        let retired = {
            let mut conn = engine.db().acquire().await?;
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM thread_history_cut_commands h JOIN commands c
              ON c.kind='node.runtime.remove' AND c.state='succeeded' AND c.machine_id=h.retirement_machine_id
                AND c.body->>'runtime_id'=h.runtime_id::text WHERE h.command_id=$1 AND h.runtime_id IS NOT NULL)")
                .bind(id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?
        };
        if native || retired {
            let mut tx = engine.db().begin().await?;
            if command.kind == CommandKind::SessionOpenFenced
                && let Some(Outcome::Succeeded(value)) = actual
            {
                let opened: SessionOpenResult = serde_json::from_value(value)?;
                let body: orch_link_contract::continuations::SessionOpenFenced =
                    serde_json::from_value(command.body.clone())?;
                if opened.native_session_ref.is_empty()
                    || cut.source_native_ref.as_deref() == Some(opened.native_session_ref.as_str())
                {
                    return Err(EngineError::unreadable(
                        "The closed side's actual creation result names its parent or no child.",
                    ));
                }
                if !cuts::keep_artifact(&mut tx,cut_id,"side_native_result",&json!({"command_id":id,
                    "segment_id":body.session.segment_id,"native_session_ref":opened.native_session_ref}),&[]).await? {
                    return Err(EngineError::unreadable("The closed side retained another actual child."));
                }
            }
            cuts::settle_command(&mut tx, cut_id, id).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            continue;
        }
        let retained: Option<(Option<Id>, i64)> = {
            let mut conn = engine.db().acquire().await?;
            sqlx::query_as("SELECT fence_id,fence_attempt FROM thread_history_cut_commands WHERE command_id=$1 AND NOT settled")
                .bind(id).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?
        };
        let Some((previous, attempt)) = retained else {
            continue;
        };
        let progress = if let Some(fence) = previous {
            engine
                .inner
                .commands
                .status(fence)
                .await?
                .and_then(|info| info.outcome)
        } else {
            None
        };
        if matches!(progress, Some(Outcome::Succeeded(_))) {
            let mut conn = engine.db().acquire().await?;
            cuts::settle_command(&mut conn, cut_id, id).await?;
            continue;
        }
        let next = {
            let mut tx = engine.db().begin().await?;
            let owned: Option<(Option<Id>, i64)> = sqlx::query_as(
                "SELECT fence_id,fence_attempt FROM thread_history_cut_commands
                WHERE command_id=$1 AND NOT settled FOR UPDATE",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
            if owned != Some((previous, attempt)) {
                continue;
            }
            let replace = if let Some(fence) = previous {
                let row = link::command(&mut tx, fence).await?.ok_or_else(|| {
                    EngineError::unreadable("The original side queue fence is unavailable.")
                })?;
                if row.machine_id != command.machine
                    || row.kind != CommandKind::MachineProbe.as_str()
                    || row.body != json!({})
                    || row.serial_key() != command.key.wire().as_deref()
                {
                    return Err(EngineError::unreadable(
                        "The side queue fence names another source or read.",
                    ));
                }
                if progress.is_some() && engine.inner.clock.now() < row.deadline_at {
                    continue;
                }
                progress.is_some()
            } else {
                true
            };
            let counter = if replace && previous.is_some() {
                attempt.checked_add(1).ok_or_else(|| {
                    EngineError::unreadable("The side queue fence counter is exhausted.")
                })?
            } else {
                attempt
            };
            let fence = if replace {
                ids_for_probe(id, counter)
            } else {
                previous.ok_or_else(|| {
                    EngineError::unreadable("The retained fence identity is unavailable.")
                })?
            };
            let mut outgoing = Outgoing::new(
                fence,
                command.machine,
                command.key.clone(),
                &orch_link_contract::commands::MachineProbe {},
            )?;
            let old = link::command(&mut tx, fence).await?;
            let deadline = old.map_or_else(
                || {
                    engine.inner.clock.now()
                        + chrono::Duration::seconds(i64::from(
                            CommandKind::MachineProbe.deadline_secs(),
                        ))
                },
                |row| row.deadline_at,
            );
            let wire = command.key.wire();
            link::insert_command(
                &mut tx,
                &link::NewCommand {
                    id: fence,
                    machine_id: command.machine,
                    kind: CommandKind::MachineProbe.as_str(),
                    body: &outgoing.body,
                    serial_key: wire.as_deref(),
                    deadline_at: deadline,
                },
                engine.inner.clock.now(),
            )
            .await?;
            sqlx::query("UPDATE thread_history_cut_commands SET fence_id=$2,fence_attempt=$3 WHERE command_id=$1 AND NOT settled")
                .bind(id).bind(fence).bind(counter).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
            outgoing.deadline = Some(Time::new(deadline));
            tx.commit().await.map_err(orch_db::DbError::from)?;
            outgoing
        };
        // No metadata connection/transaction remains during native issue.
        engine.inner.commands.issue(next).await?;
    }
    Ok(())
}

fn ids_for_probe(original: Id, attempt: i64) -> Id {
    crate::ids::derived(
        "side.history-queue-fence",
        &[original.as_bytes(), &attempt.to_be_bytes()],
    )
}
