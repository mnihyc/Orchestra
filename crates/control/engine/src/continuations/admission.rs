//! One thread lock binds author, current authority, immutable membership, queue and native identities.

use super::{
    Error,
    recipe::{self, Commands, Expected, Kind, Lane, Recipe, Request, Running, RunningAction},
};
use crate::{
    engine::Engine,
    error::EngineError,
    inputs::{InputError, InputRequest},
    unit::Unit,
};
use orch_access_contract::{decisions::Decision, permissions::Permission};
use orch_api_contract::threads::{
    ContinuationActiveTurn, ContinuationRunningAction, Delivery, ThreadContinuationAccepted,
};
use orch_authz::{Act, Principal, nodes};
use orch_db::queries::{jobs, thread_continuations as rows, threads};
use orch_types::{Id, Time, new_id};
use serde_json::json;

pub(super) fn running(value: Option<ContinuationActiveTurn>) -> Option<Running> {
    value.map(|turn| Running {
        turn_id: turn.turn_id,
        action: match turn.action {
            ContinuationRunningAction::Wait => RunningAction::Wait,
            ContinuationRunningAction::Interrupt => RunningAction::Interrupt,
        },
    })
}

pub(crate) async fn require_view(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    thread: Id,
) -> Result<(), Error> {
    match nodes::check(
        conn,
        principal,
        thread,
        Permission::ThreadView,
        Act::Use,
        orch_authz::Policy::default(),
    )
    .await
    .map_err(EngineError::from)?
    {
        Decision::Allowed { .. } => Ok(()),
        Decision::Refused(why) => Err(InputError::Denied(Box::new(why)).into()),
    }
}

pub(crate) async fn lane(
    conn: &mut orch_db::Conn,
    thread: Id,
    continuation: Id,
    segment: Id,
) -> Result<(Lane, threads::SegmentRow), Error> {
    let segment = threads::segment(conn, segment)
        .await?
        .filter(|segment| segment.thread_id == thread && segment.continuation_id == continuation)
        .ok_or(Error::NotFound)?;
    let native = segment
        .native_session_ref
        .clone()
        .filter(|native| !native.is_empty())
        .ok_or(Error::Unsupported(
            "This continuation has no retained native conversation to resume.",
        ))?;
    let machine: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM segments WHERE id=$1")
        .bind(segment.id)
        .fetch_one(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    let lane = Lane {
        continuation_id: continuation,
        source: orch_agent_contract::history::NativePrefixSource {
            thread_id: thread,
            segment_id: segment.id,
            native_session_ref: native,
        },
        machine_id: machine.ok_or(Error::Unsupported(
            "The retained conversation has no reporting machine.",
        ))?,
        agent_id: segment.agent_id,
        provider_id: segment.provider_id,
        environment_id: segment.environment_id,
        environment_generation: segment.env_generation,
    };
    Ok((lane, segment))
}

/// A retained native lane is tied to the physical generation in which it was recorded.
pub(crate) async fn generation_current(
    conn: &mut orch_db::Conn,
    lane: &Lane,
) -> Result<bool, EngineError> {
    let generation: Option<i32> = sqlx::query_scalar(
        "SELECT generation FROM environments WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(lane.environment_id)
    .fetch_optional(conn)
    .await
    .map_err(orch_db::DbError::from)?;
    Ok(generation == Some(lane.environment_generation))
}

pub(super) async fn accept(
    engine: &Engine,
    unit: &mut Unit<'_>,
    principal: &Principal,
    thread: Id,
    client: &str,
    request: Request,
) -> Result<ThreadContinuationAccepted, Error> {
    if client.trim().is_empty() {
        return Err(Error::Invalid(
            "A continuation request needs its replay identity.",
        ));
    }
    request.expected.validate().map_err(Error::Invalid)?;
    let row = threads::lock(unit.conn, thread)
        .await?
        .ok_or(Error::NotFound)?;
    if row.deleted_at.is_some() {
        return Err(Error::NotFound);
    }
    require_view(unit.conn, principal, thread).await?;
    let hash = recipe::request_hash(&request)?;
    if let Some(found) = rows::replay(unit.conn, thread, principal.caller().user(), client).await? {
        if !found.same_body(&hash) {
            return Err(Error::Conflict(
                "This replay identity already names different words or another continuation.",
            ));
        }
        return Ok(ThreadContinuationAccepted {
            operation_id: found.id,
            continuation_id: found.target_continuation_id,
            input_id: found.input_id,
            job: serde_json::from_value(found.job_snapshot)?,
            seq: row.seq(),
        });
    }
    if row.archived_at.is_some() {
        return Err(InputError::Archived.into());
    }
    engine
        .check_sender(unit, principal.caller(), &row, true)
        .await?;
    // Runtime confirmations use both resource-first and thread-first lock paths. Try, rather
    // than wait on, a resource already held by a confirmation waiting for this thread.
    for (statement, id) in [
        (
            "SELECT id FROM environments WHERE id=$1 FOR UPDATE NOWAIT",
            row.environment_id,
        ),
        (
            "SELECT id FROM providers WHERE id=$1 FOR UPDATE NOWAIT",
            row.provider_id,
        ),
    ] {
        let id = id.ok_or(Error::Unsupported(
            "Bind this thread before preparing a native continuation.",
        ))?;
        match sqlx::query_scalar::<_, Id>(statement)
            .bind(id)
            .fetch_optional(&mut *unit.conn)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return Err(Error::NotFound),
            Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => {
                return Err(Error::Conflict(
                    "An environment or provider change is committing; retry this request.",
                ));
            }
            Err(error) => return Err(orch_db::DbError::from(error).into()),
        }
    }
    let environment_hold =
        jobs::changes::environment_hold(unit.conn, row.environment_id.ok_or(Error::NotFound)?)
            .await?;
    let provider_hold =
        jobs::changes::provider_hold(unit.conn, row.provider_id.ok_or(Error::NotFound)?).await?;
    if environment_hold
        .into_iter()
        .chain(provider_hold)
        .any(|job| job.kind != rows::JOB_KIND)
    {
        return Err(Error::Conflict(
            "The environment or provider is changing; its current native conversation is kept.",
        ));
    }
    if orch_db::queries::agent_history::removing(unit.conn,thread).await? || crate::locks::held_by(unit.conn,thread).await?.is_some() || sqlx::query_scalar::<_,bool>(
        "SELECT EXISTS(SELECT 1 FROM thread_continuation_operations WHERE thread_id=$1 AND status='preparing')")
        .bind(thread).fetch_one(&mut *unit.conn).await.map_err(orch_db::DbError::from)? {
        return Err(Error::Conflict("Another execution change holds this thread; its queued inputs are kept."));
    }
    let current = Expected {
        active_continuation_id: row.active_continuation_id,
        execution_revision: row.execution_revision,
        queue_revision: row.queue_revision,
    };
    request
        .expected
        .admits(current, row.current_turn_id, request.active_turn)
        .map_err(Error::Conflict)?;
    if request.kind == Kind::Activate
        && request.target_continuation_id == Some(row.active_continuation_id)
    {
        return Err(Error::Conflict(
            "This continuation is already selected for execution.",
        ));
    }
    let content_request = request.content.as_ref().map(|content| InputRequest {
        plan: false,
        content: content.clone(),
        delivery: Delivery::Queue,
        client_id: client.to_owned(),
        bypass: None,
    });
    if let Some(content) = &content_request {
        crate::inputs::validate_literal_input(content)?;
        engine
            .check_content(unit, principal.caller(), &content.content)
            .await?;
    }
    let chosen = rows::continuation(unit.conn, thread, request.source_continuation_id)
        .await?
        .ok_or(Error::NotFound)?;
    let retry = request.kind == Kind::Activate && chosen.status == "failed";
    if chosen.status != "ready" && !retry {
        return Err(Error::Conflict(
            "The requested continuation has not completed native preparation.",
        ));
    }
    let retained_prefix: Option<orch_agent_contract::history::NativeInputPrefix> = chosen
        .native_prefix
        .clone()
        .map(serde_json::from_value)
        .transpose()?;
    let retained_result: Option<recipe::PreparedNative> = chosen
        .open_result
        .clone()
        .map(serde_json::from_value)
        .transpose()?;
    let creation: Option<recipe::NativeCreation> = chosen
        .native_creation
        .clone()
        .map(serde_json::from_value)
        .transpose()?;
    let created: Option<orch_agent_contract::history::NativePrefixOpened> = chosen
        .native_creation_result
        .clone()
        .map(serde_json::from_value)
        .transpose()?;
    if retry && creation.is_some() && created.is_none() {
        return Err(Error::Conflict(
            "The original native creation outcome is unconfirmed; this version and its queued words are kept without creating another conversation.",
        ));
    }
    if let (Some(creation), Some(created)) = (&creation, &created) {
        if retained_prefix.as_ref() != Some(&created.prefix)
            || created.session.native_session_ref.is_empty()
            || created.session.native_session_ref == created.prefix.source.native_session_ref
        {
            return Err(Error::Unsupported(
                "The retained native creation receipt does not identify a verified exact prefix.",
            ));
        }
        let actual = threads::segment(unit.conn, creation.segment_id)
            .await?
            .filter(|segment| segment.thread_id == thread && segment.continuation_id == chosen.id)
            .ok_or(Error::NotFound)?;
        if actual.native_session_ref.as_deref() != Some(created.session.native_session_ref.as_str())
        {
            return Err(Error::Unsupported(
                "The verified native creation no longer matches its owned segment.",
            ));
        }
    }
    let known_child = if retry {
        match (chosen.current_segment_id, &retained_result) {
            (Some(segment), Some(prepared)) => threads::segment(unit.conn, segment)
                .await?
                .is_some_and(|segment| {
                    segment.thread_id == thread
                        && segment.continuation_id == chosen.id
                        && segment.native_session_ref.as_deref() == Some(prepared.native())
                }),
            _ => false,
        }
    } else {
        false
    };
    if retry && creation.is_some() && !known_child {
        return Err(Error::Conflict(
            "This version retains an original native creation but its resumable child is not confirmed; another child is not created.",
        ));
    }
    let needs_prefix = request.kind == Kind::Edit || retry && !known_child;
    let source = if retry && !known_child {
        rows::continuation(
            unit.conn,
            thread,
            chosen.parent_id.ok_or(Error::Unsupported(
                "The failed version has no retained native parent.",
            ))?,
        )
        .await?
        .ok_or(Error::NotFound)?
    } else {
        chosen.clone()
    };
    let source_segment = if retry && needs_prefix {
        retained_prefix
            .as_ref()
            .map(|prefix| prefix.source.segment_id)
            .or(source.current_segment_id)
    } else {
        source.current_segment_id
    }
    .ok_or(Error::Unsupported(
        "This continuation has no retained native session.",
    ))?;
    let (source_lane, segment) = lane(unit.conn, thread, source.id, source_segment).await?;
    if retry
        && needs_prefix
        && retained_prefix
            .as_ref()
            .is_some_and(|prefix| prefix.source != source_lane.source)
    {
        return Err(Error::Unsupported(
            "The failed version's exact retained native source is unavailable.",
        ));
    }
    if row.agent_id != Some(segment.agent_id)
        || row.provider_id != Some(segment.provider_id)
        || row.environment_id != Some(segment.environment_id)
        || row.arrangement.as_deref() != Some(segment.arrangement.as_str())
    {
        return Err(Error::Unsupported(
            "This continuation belongs to another binding; use the binding change operation to transfer its native history.",
        ));
    }
    if !generation_current(unit.conn, &source_lane).await? {
        return Err(Error::Unsupported(
            "The retained conversation belongs to an older environment generation; the selected native conversation is kept.",
        ));
    }
    let agent = orch_db::queries::resources::thread_engine::agent(unit.conn, segment.agent_id)
        .await?
        .ok_or(Error::NotFound)?;
    if crate::harnesses::descriptor_of(&agent)?
        .and_then(|descriptor| descriptor.capabilities.native_input_prefix)
        != Some(orch_agent_contract::descriptor::NativeInputPrefixSupport::TurnStart)
    {
        return Err(Error::Unsupported(
            "This agent program does not declare exact turn-start prompt continuation support.",
        ));
    }
    let (source_turn, boundary) = if needs_prefix {
        let input = threads::input(
            unit.conn,
            request
                .before_input_id
                .or(chosen.before_input_id)
                .ok_or(Error::Invalid("Choose the actual sent input to edit."))?,
        )
        .await?
        .filter(|input| input.thread_id == thread)
        .ok_or(Error::NotFound)?;
        if !rows::reading::contains_input(unit.conn, thread, source.id, input.id).await? {
            return Err(Error::NotFound);
        }
        if input.source != "user"
            || request.kind == Kind::Edit && input.author_id != Some(principal.caller().user())
        {
            return Err(Error::NotAuthor);
        }
        if input.state != threads::input_state::DELIVERED || input.native_action.is_some() {
            return Err(Error::Unsupported(
                "Edit applies to a delivered ordinary prompt; pending words use their queue edit action.",
            ));
        }
        let turn = threads::turn(
            unit.conn,
            input.turn_id.ok_or(Error::Unsupported(
                "This input has no admitted native turn.",
            ))?,
        )
        .await?
        .filter(|turn| turn.thread_id == thread && turn.input_id == input.id)
        .ok_or(Error::Unsupported(
            "This input is guidance inside a turn, not its first authored input.",
        ))?;
        let boundary:Option<i64>=sqlx::query_scalar("SELECT seq_created FROM items WHERE thread_id=$1 AND turn_id=$2
            AND kind='user_message' AND payload->>'input_id'=$3 AND native_ref IS NOT NULL AND NOT before_orch
            ORDER BY seq_created LIMIT 1").bind(thread).bind(turn.id).bind(input.id.to_string())
            .fetch_optional(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        (
            turn,
            Some(boundary.ok_or(Error::Unsupported(
                "The native program did not retain the exact authored input identity.",
            ))?),
        )
    } else {
        let turn:Option<Id>=sqlx::query_scalar("SELECT id FROM turns WHERE thread_id=$1 AND continuation_id=$2 ORDER BY ordinal DESC LIMIT 1")
            .bind(thread).bind(source.id).fetch_optional(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        let turn = match turn {
            Some(turn) => turn,
            None => threads::input(
                unit.conn,
                chosen.before_input_id.ok_or(Error::Unsupported(
                    "This continuation has no retained native input or parent checkpoint.",
                ))?,
            )
            .await?
            .and_then(|input| input.turn_id)
            .ok_or(Error::NotFound)?,
        };
        let turn = threads::turn(unit.conn, turn)
            .await?
            .ok_or(Error::NotFound)?;
        (turn, None)
    };
    let original_lane = match row.current_segment_id {
        Some(segment) => Some(
            lane(unit.conn, thread, row.active_continuation_id, segment)
                .await?
                .0,
        ),
        None => None,
    };
    let operation = new_id();
    let target = if request.kind == Kind::Edit {
        new_id()
    } else {
        chosen.id
    };
    let input_id = request.content.as_ref().map(|_| new_id());
    let segment_id = new_id();
    let options = crate::fork_plan::applied_options(unit.conn, thread, None)
        .await
        .map_err(|error| Error::Internal(EngineError::unreadable(error)))?;
    let now = unit.now();
    let seq = unit.stamp(thread).await?;
    if request.kind == Kind::Edit {
        rows::insert(
            unit.conn,
            &rows::NewContinuation {
                id: target,
                thread,
                parent: source.id,
                before_input: request
                    .before_input_id
                    .ok_or(Error::Invalid("Choose the actual sent input."))?,
                actor: principal.caller().user(),
                at: now,
                seq,
                queue: rows::QueueRow {
                    held: row.queue_held,
                    held_by: row.queue_held_by,
                    held_at: row.queue_held_at,
                    revision: 1,
                },
            },
        )
        .await?;
        rows::freeze_prefix(
            unit.conn,
            thread,
            source.id,
            target,
            source_turn.ordinal,
            boundary.ok_or(Error::Invalid("The actual input boundary is missing."))?,
            seq,
        )
        .await?;
    } else if retry {
        sqlx::query("UPDATE thread_continuations SET status='preparing',failure=NULL WHERE id=$1")
            .bind(target)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
    }
    let prepared = threads::insert_segment_in(
        unit.conn,
        &threads::NewSegment {
            id: segment_id,
            thread_id: thread,
            agent_id: segment.agent_id,
            provider_id: segment.provider_id,
            environment_id: segment.environment_id,
            env_generation: segment.env_generation,
            arrangement: &segment.arrangement,
            executor: segment.executor.as_deref(),
            tool_set: segment.tool_set.as_deref(),
            credential_path: &segment.credential_path,
            native_session_ref: None,
            started_by: Some(principal.caller().user()),
            started_at: now,
        },
        Some(target),
        seq,
    )
    .await?;
    sqlx::query("UPDATE segments SET machine_id=$2 WHERE id=$1")
        .bind(prepared.id)
        .bind(source_lane.machine_id)
        .execute(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
    if let (Some(input), Some(content)) = (input_id, &content_request) {
        let body = serde_json::to_value(&content.content)?;
        let replay = format!("continuation:{operation}");
        let provenance = if request.kind == Kind::Edit {
            let origin = threads::input(unit.conn, source_turn.input_id)
                .await?
                .ok_or(Error::NotFound)?;
            engine
                .check_input_author_driving(unit, &row, &origin)
                .await?;
            let mut provenance =
                crate::inputs::derived_input_provenance(&origin, Some(principal.caller()))?;
            provenance.submission_authority = crate::inputs::narrowed_input_authority(
                provenance.submission_authority.as_ref(),
                principal.caller(),
            )?;
            provenance
        } else {
            crate::inputs::InputProvenance {
                author_id: Some(principal.caller().user()),
                submission_authority: Some(crate::inputs::input_authority(principal.caller())?),
                run_as: None,
                run_as_authority: None,
            }
        };
        let inserted = threads::insert_input_in(
            unit.conn,
            &threads::NewInput {
                plan: false,
                id: input,
                thread_id: thread,
                author_id: provenance.author_id,
                submission_authority: provenance.submission_authority.as_ref(),
                run_as: provenance.run_as,
                run_as_authority: provenance.run_as_authority.as_ref(),
                source: "user",
                content: &body,
                delivery: "queue",
                state: "pending",
                turn_id: None,
                client_id: Some(&replay),
                limit_bypass: None,
                place: threads::Place::End,
                at: now,
            },
            Some(target),
            seq,
        )
        .await?
        .ok_or(Error::Conflict(
            "The revised input already has another identity.",
        ))?;
        orch_db::queries::storage::input_pins(
            unit.conn,
            inserted.id,
            &orch_chat::following::content_blobs(&content.content),
            now,
        )
        .await?;
    }
    let recipe = Recipe {
        operation_id: operation,
        thread_id: thread,
        actor_id: principal.caller().user(),
        authorization_token: principal.caller().grant().and_then(|grant| grant.token()),
        request: request.clone(),
        request_hash: hash,
        needs_prefix,
        authorization_scope: principal
            .caller()
            .grant()
            .map(|grant| recipe::AuthorizationScope {
                scopes: grant.scopes().to_strings(),
                subtree: grant.subtree(),
            }),
        source_continuation_id: source.id,
        target_continuation_id: target,
        input_id,
        segment_id,
        original_lane,
        source_lane,
        source_turn_id: source_turn.id,
        wait_minutes: options.wait_minutes,
        commands: Commands::of(operation),
        accepted_at: Time::new(now),
        deadline: Time::new(
            now + chrono::Duration::minutes(options.wait_minutes)
                + chrono::Duration::seconds(options.verify_timeout_secs),
        ),
        prefix: if retry && needs_prefix {
            retained_prefix
        } else {
            None
        },
        opened: None,
        activation_seq: None,
    };
    let plan = json!({"recipe":recipe,"origin":orch_db::audit::current_origin()});
    let progress = serde_json::to_value(crate::jobs::runner::initial_progress(
        crate::jobs::continuation::PHASES,
    ))?;
    let job = jobs::insert(
        unit.conn,
        &jobs::NewJob {
            id: operation,
            kind: rows::JOB_KIND,
            node_id: Some(thread),
            environment_id: Some(segment.environment_id),
            provider_id: Some(segment.provider_id),
            plan: &plan,
            progress: &progress,
            created_by: Some(principal.caller().user()),
        },
        now,
    )
    .await?;
    let reference = crate::locks::job_ref(&job);
    rows::create_operation(
        unit.conn,
        &rows::OperationRow {
            id: operation,
            job_id: Some(operation),
            job_snapshot: serde_json::to_value(&reference)?,
            thread_id: thread,
            actor_id: principal.caller().user(),
            client_id: client.to_owned(),
            body_hash: hash.as_bytes().to_vec(),
            kind: if request.kind == Kind::Edit {
                "edit".into()
            } else {
                "activate".into()
            },
            source_continuation_id: source.id,
            target_continuation_id: target,
            input_id,
            segment_id,
            expected_active_continuation_id: current.active_continuation_id,
            expected_execution_revision: current.execution_revision,
            expected_queue_revision: current.queue_revision,
            status: "preparing".into(),
            activation_seq: None,
            created_at: now,
        },
    )
    .await?;
    if request.kind == Kind::Edit {
        rows::add_version(
            unit.conn,
            thread,
            request
                .before_input_id
                .ok_or(Error::Invalid("Choose the actual sent input."))?,
            input_id.ok_or(Error::Invalid("A revised prompt needs its words."))?,
            target,
            operation,
            seq,
        )
        .await?;
    } else {
        sqlx::query(
            "UPDATE thread_prompt_versions SET operation_id=$2,job_id=$2 WHERE continuation_id=$1",
        )
        .bind(target)
        .bind(operation)
        .execute(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
    }
    unit.wake(thread);
    Ok(ThreadContinuationAccepted {
        operation_id: operation,
        continuation_id: target,
        input_id,
        job: reference,
        seq,
    })
}
