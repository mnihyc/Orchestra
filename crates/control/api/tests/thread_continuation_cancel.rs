//! Real continuation-job cancellation by a current interactor is reducing housekeeping.

mod continuation_diagnostics;
mod continuation_execution_support;
mod places;
mod rig;
mod support;

use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{Permission, ResourceKind},
};
use orch_agent_contract::history::NativePrefixSource;
use orch_api_contract::{
    common::ErrorCode,
    nodes::JobState,
    threads::{
        ContentPart, ThreadContinuationAccepted, ThreadContinuationStatus,
        ThreadPromptVersionCreate, ThreadPromptVersionCreateBody, ThreadSyncGet,
        ThreadSyncGetQuery,
    },
    workspace::{JobCancel, JobGet},
};
use orch_authz::{Caller, Policy, Principal};
use orch_link::{
    commands::{CommandContext, CommandHandler},
    journal::Step,
};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode as NativeError,
};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Semaphore;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct HeldCreation {
    inner: Arc<orch_testkit::doubles::machine::ScriptedHandler>,
    entered: Semaphore,
    release: Semaphore,
}

#[async_trait::async_trait]
impl CommandHandler for HeldCreation {
    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        if command.kind() == CommandKind::SessionPrefixOpen {
            self.entered.add_permits(1);
            // Real in-progress native work need not immediately stop its present I/O.
            // The actual command server keeps the original thread key until this joins.
            self.release
                .acquire()
                .await
                .map_err(|_| {
                    CommandError::new(
                        NativeError::Internal,
                        "the controlled native creation ended",
                    )
                })?
                .forget();
        }
        self.inner.handle(context, command).await
    }
}

async fn cancel_when(native_started: bool) -> Proof {
    continuation_diagnostics::install()?;
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(thread, 0, "Actual first input", "Actual first answer")
        .await;
    scene
        .exchange(thread, 1, "Actual second input", "Actual second answer")
        .await;
    rig::eventually(
        "actual original terminal housekeeping settles before cancellation setup",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1
            AND (end_work IS NOT NULL OR files_end_due OR files_work_pending))",
            )
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,native_input_prefix}','\"turn_start\"') WHERE id=$1")
        .bind(scene.on.agent).execute(scene.rig.db.pool()).await?;
    let descriptor: Json = sqlx::query_scalar("SELECT descriptor FROM agents WHERE id=$1")
        .bind(scene.on.agent)
        .fetch_one(scene.rig.db.pool())
        .await?;
    let (input, turn, segment, native): (Id, Id, Id, String) = sqlx::query_as(
        "SELECT u.input_id,u.id,s.id,s.native_session_ref
        FROM turns u JOIN segments s ON s.id=u.segment_id WHERE u.thread_id=$1 AND u.ordinal=2",
    )
    .bind(thread)
    .fetch_one(scene.rig.db.pool())
    .await?;
    let source = NativePrefixSource {
        thread_id: thread,
        segment_id: segment,
        native_session_ref: native,
    };
    continuation_execution_support::install(
        &scene.rig.machine.handler,
        scene.on.agent,
        scene.on.provider,
        source.clone(),
        turn,
        serde_json::from_value(descriptor)?,
    );
    let baseline = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    let projection = baseline
        .projection
        .as_ref()
        .ok_or("the actual original projection is absent")?;
    let queue: (bool, Option<Id>, Option<chrono::DateTime<chrono::Utc>>, i64) = sqlx::query_as(
        "SELECT queue_held,queue_held_by,queue_held_at,queue_revision
        FROM thread_continuations WHERE id=$1",
    )
    .bind(projection.active_continuation_id)
    .fetch_one(scene.rig.db.pool())
    .await?;
    let ledger: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id")
            .bind(thread)
            .fetch_all(scene.rig.db.pool())
            .await?;
    let held = Arc::new(HeldCreation {
        inner: scene.rig.machine.handler.clone(),
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    if native_started {
        scene.rig.machine.link.serve(held.clone());
    } else {
        scene
            .rig
            .machine
            .handler
            .hold(CommandKind::SessionPrefixInspect);
    }
    let accepted: ThreadContinuationAccepted = scene
        .rig
        .world
        .app
        .call(ThreadPromptVersionCreate, &[&thread, &input])
        .token(&scene.ada.token)
        .body(&ThreadPromptVersionCreateBody {
            client_id: if native_started {
                "cancel-started"
            } else {
                "cancel-before-effect"
            }
            .into(),
            source_continuation_id: projection.viewed_continuation_id,
            content: vec![ContentPart::Text {
                text: "Retain the revised pending words".into(),
            }],
            expected_active_continuation_id: projection.active_continuation_id,
            expected_execution_revision: projection.execution_revision,
            expected_queue_revision: baseline.thread.queue.revision,
            active_turn: None,
        })
        .send()
        .await
        .ok();
    let commands = orch_engine::continuations::recipe::Commands::of(accepted.operation_id);
    if native_started {
        tokio::time::timeout(rig::PATIENCE, held.entered.acquire())
            .await??
            .forget();
        let entry: orch_link::journal::Entry = serde_json::from_slice(
            &tokio::fs::read(
                scene
                    .rig
                    .machine
                    .data_dir()
                    .join("journal")
                    .join(format!("{}.json", commands.open_prefix)),
            )
            .await?,
        )?;
        assert!(
            matches!(entry.step, Step::Started),
            "the held native work really entered its durable journal"
        );
    } else {
        tokio::time::timeout(
            rig::PATIENCE,
            scene.rig.machine.handler.started(commands.preinspect),
        )
        .await?;
    }
    let app = &scene.rig.world.app;
    let visible = app
        .call(JobGet, &[&accepted.job.id])
        .token(&scene.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(
        visible.id, accepted.job.id,
        "a current thread reader can reopen its actual continuation job without environment management"
    );
    let denied = app
        .call(JobCancel, &[&accepted.job.id])
        .token(&scene.cai.token)
        .send()
        .await
        .problem();
    assert_eq!(
        denied.code,
        ErrorCode::AuthzDenied,
        "View alone cannot cancel work"
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT plan ? 'cancel' FROM jobs WHERE id=$1")
            .bind(accepted.job.id)
            .fetch_one(scene.rig.db.pool())
            .await?
    );
    scene
        .rig
        .lets_use("node", thread, "thread.interact", &scene.cai, json!({}))
        .await;
    orch_db::queries::access::bump_epoch(&mut *scene.rig.world.conn().await).await?;
    let mut conn = scene.rig.world.conn().await;
    let principal = Principal::load(
        &mut conn,
        Caller::session(scene.cai.id()),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    for (kind, id, permission) in [
        (
            ResourceKind::Environment,
            scene.on.environment,
            Permission::EnvironmentManage,
        ),
        (
            ResourceKind::Provider,
            scene.on.provider,
            Permission::ProviderUse,
        ),
    ] {
        assert!(
            !orch_authz::resources::check(
                &mut conn,
                &principal,
                ObjectRef {
                    kind: kind.into(),
                    id
                },
                permission,
                Policy::default()
            )
            .await?
            .is_allowed(),
            "the cancellation control must not accidentally have environment-management or provider-use authority"
        );
    }
    drop(conn);
    let cancelled = app
        .call(JobCancel, &[&accepted.job.id])
        .token(&scene.cai.token)
        .send()
        .await;
    assert!(cancelled.status.is_success());
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT plan#>>'{cancel,by}' FROM jobs WHERE id=$1")
            .bind(accepted.job.id)
            .fetch_one(scene.rig.db.pool())
            .await?,
        scene.cai.id().to_string()
    );
    if native_started {
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT state IN('queued','running') FROM jobs WHERE id=$1"
            )
            .bind(accepted.job.id)
            .fetch_one(scene.rig.db.pool())
            .await?,
            "cancellation must not abandon a still-owned native thread key"
        );
        assert_eq!(scene.rig.turn_starts(thread).len(), 2);
        held.release.add_permits(1);
    } else {
        scene.rig.machine.handler.release();
    }
    rig::eventually(
        "cancellation reconciles the admitted outcome and restores the exact original native lane",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT state='cancelled' FROM jobs WHERE id=$1")
                .bind(accepted.job.id)
                .fetch_one(scene.rig.db.pool())
                .await
                .unwrap_or(false)
        },
    )
    .await;
    let ended = app
        .call(JobGet, &[&accepted.job.id])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(ended.state, JobState::Cancelled);
    assert!(ended.error.as_ref().is_some_and(|error| error.undone));
    let after = app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        after.thread.active_continuation_id,
        baseline.thread.active_continuation_id
    );
    assert_eq!(
        after.thread.execution_revision,
        baseline.thread.execution_revision
    );
    assert_eq!(after.thread.current_turn_id, None);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT native_session_ref FROM segments WHERE id=$1")
            .bind(source.segment_id)
            .fetch_one(scene.rig.db.pool())
            .await?,
        source.native_session_ref
    );
    assert_eq!(
        sqlx::query_as::<_, (bool, Option<Id>, Option<chrono::DateTime<chrono::Utc>>, i64)>(
            "SELECT queue_held,queue_held_by,queue_held_at,queue_revision
        FROM thread_continuations WHERE id=$1"
        )
        .bind(projection.active_continuation_id)
        .fetch_one(scene.rig.db.pool())
        .await?,
        queue
    );
    assert_eq!(
        sqlx::query_scalar::<_, Json>(
            "SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id"
        )
        .bind(thread)
        .fetch_all(scene.rig.db.pool())
        .await?,
        ledger
    );
    let revised = accepted
        .input_id
        .ok_or("the retained revised input is absent")?;
    let pending: (Id, String, Json) =
        sqlx::query_as("SELECT continuation_id,state,content FROM thread_inputs WHERE id=$1")
            .bind(revised)
            .fetch_one(scene.rig.db.pool())
            .await?;
    assert_eq!(
        pending,
        (
            accepted.continuation_id,
            "pending".into(),
            json!([{"type":"text","text":"Retain the revised pending words"}])
        )
    );
    let viewed = app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.cai.token)
        .query(&ThreadSyncGetQuery {
            continuation_id: Some(accepted.continuation_id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let prompt = viewed
        .viewed_prompt
        .ok_or("the cancelled viewed version lost its durable status")?;
    assert_eq!(prompt.status, ThreadContinuationStatus::Failed);
    assert_eq!(
        prompt.job.as_ref().map(|job| job.state),
        Some(JobState::Cancelled)
    );
    assert_eq!(
        scene
            .rig
            .commands
            .issued_of(CommandKind::SessionPrefixOpen.as_str()),
        usize::from(native_started)
    );
    assert_eq!(
        scene.rig.turn_starts(thread).len(),
        2,
        "cancelling or reading the failed version cannot author another native turn"
    );
    Ok(())
}

#[tokio::test]
async fn a_current_thread_interactor_can_cancel_preparation_without_environment_manage_or_new_provider_use()
-> Proof {
    cancel_when(false).await
}

#[tokio::test]
async fn cancellation_of_started_prefix_work_joins_its_native_lane_and_recovers_without_consuming_inactive_words()
-> Proof {
    cancel_when(true).await
}
