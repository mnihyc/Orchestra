//! Admitted native tool calls for engine fixtures, with no authority or ingress bypass.

use crate::common::{PATIENCE, World, queued};
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_authz::Caller;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, OrchTool, OrchToolResult,
    TurnStart,
};
use orch_link_contract::events::{Event, OrchToolCall};
use orch_link_contract::source::{FsRecordResult, RecordState};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::future::Future;
use std::time::Duration;

/// Match common::eventually's sampling/deadline; the diagnostic budget cannot admit more work.
const SAMPLE: Duration = Duration::from_millis(20);
const DIAGNOSTIC_BUDGET: Duration = Duration::from_secs(2);

pub async fn wait<F, Fut>(world: &World, thread: Id, phase: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let result = tokio::time::timeout(PATIENCE, async {
        while !check().await {
            tokio::time::sleep(SAMPLE).await;
        }
    })
    .await;
    if result.is_err() {
        let state = tokio::time::timeout(DIAGNOSTIC_BUDGET, async {
            // Metadata identities/counts only: no input, arguments, credentials or document/file names.
            sqlx::query_scalar::<_, Json>("SELECT jsonb_build_object('state',t.state,'blocked',t.blocked_reason,
                'step',t.starting_step,'current',t.current_turn_id,'segment',t.current_segment_id,
                'arrangement',t.arrangement,
                'approval_mode',(SELECT value FROM settings WHERE node_id=t.node_id AND key='approval.mode'),
                'declared_approval_modes',(SELECT a.descriptor->'approval_modes' FROM agents a WHERE a.id=t.agent_id),
                'source_state',t.source#>'{info,state}','source_reason',t.source#>'{info,reason}',
                'turns',(SELECT count(*) FROM turns u WHERE u.thread_id=t.node_id),
                'current_state',(SELECT u.state FROM turns u WHERE u.id=t.current_turn_id),
                'last_error',(SELECT u.error->'code' FROM turns u WHERE u.thread_id=t.node_id ORDER BY ordinal DESC LIMIT 1),
                'session',s.native_session_ref,'session_machine',s.machine_id)
                FROM threads t LEFT JOIN segments s ON s.id=t.current_segment_id WHERE t.node_id=$1")
                .bind(thread).fetch_optional(world.pool.pool()).await
        }).await;
        panic!(
            "native tool fixture exceeded {PATIENCE:?} during {phase}; bounded state: {state:?}"
        );
    }
}

/// Report the same declaration both in stored discovery and at the real agent.start boundary.
pub async fn enable(world: &World) {
    let mut descriptor: Json = world
        .scalar("SELECT descriptor FROM agents WHERE id=$1", world.agent)
        .await;
    descriptor["capabilities"]["orch_tools"] = json!("mcp");
    descriptor["capabilities"]["mcp"] =
        json!({"stdio":true,"http":true,"reload":false,"strict":true});
    let typed: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(descriptor).expect("a declared fixture harness");
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(world.agent)
        .bind(json!(typed))
        .execute(world.pool.pool())
        .await
        .expect("the fixture declaration is stored");
    world.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(
            orch_link_contract::commands::AgentStartResult {
                reused: false,
                harness_version: "1.0.0".into(),
                descriptor: typed.clone(),
            },
        ))
    });
}

/// The shared fixture's existing workspace is read before admission; other paths remain absent.
/// Install once during setup so a test can replace a file handler without it being overwritten by a call.
pub fn existing_workspace(world: &World) {
    workspace_on(world, &world.machine, world.environment);
}

/// The native filesystem protocol names this directory relative to each
/// environment's own root, equally for a directory connector and an owned unit.
pub fn workspace_on(
    world: &World,
    machine: &orch_testkit::doubles::machine::ScriptedMachine,
    environment: Id,
) {
    let at = Time::new(world.clock.now());
    machine.handler.on(CommandKind::FsStat, move |command| {
        let Command::FsStat(body) = command else {
            unreachable!()
        };
        if body.environment_id != environment || body.mount.is_some() || body.path != "web" {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                "the fixture has no such directory",
            ));
        }
        Ok(CommandResult::FsStat(FileStat {
            entry: DirEntry {
                name: "web".into(),
                path: body.path.clone(),
                kind: DirEntryKind::Dir,
                size: 0,
                mtime: at,
                mode: 0o700,
                link_target: None,
                cached: false,
            },
            hash: None,
            media_type: "inode/directory".into(),
            is_text: false,
        }))
    });
    machine.handler.on(CommandKind::FsRecord, move |command| {
        let Command::FsRecord(body) = command else {
            unreachable!()
        };
        assert_eq!(body.environment_id, environment);
        // This follows the real Workspace::parse/RelPath contract. Absolute
        // process cwd values are not native filesystem command paths.
        if body.directory != "web" {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                "the fixture has no such directory",
            ));
        }
        Ok(CommandResult::FsRecord(FsRecordResult {
            state: RecordState::Unavailable,
            tree_ref: None,
            repository_head: None,
            manifest_blob: None,
            files: 0,
            bytes: 0,
            expires_at: None,
            reason: Some("not_ready".into()),
        }))
    });
}

pub async fn active(world: &World, thread: Id) -> Id {
    sqlx::query_scalar::<_, Id>("SELECT u.id FROM threads t JOIN turns u ON u.id=t.current_turn_id
        WHERE t.node_id=$1 AND u.thread_id=t.node_id AND u.segment_id=t.current_segment_id AND u.state='running'")
        .bind(thread).fetch_optional(world.pool.pool()).await.expect("the active turn is readable")
        .expect("a native tool call needs an actual admitted active turn")
}

/// An explicit fixture input owns this baseline. This never invokes a tool directly.
pub async fn start(world: &World, thread: Id, text: &str) -> TurnStart {
    let nth = world.turn_starts(thread).len();
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued(text))
        .await
        .expect("an authored fixture input");
    wait(world, thread, "the authored input's turn.start", || {
        let arrived = world.turn_starts(thread).len() > nth;
        async move { arrived }
    })
    .await;
    let start = world.turn_starts(thread)[nth].clone();
    let reference = format!("native-tool-baseline-{nth}");
    world
        .answer(
            thread,
            &start,
            &reference,
            "Working on the authored fixture input",
        )
        .await;
    wait(world,thread,"the exact admitted turn becoming active",|| async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM threads t JOIN turns u ON u.id=t.current_turn_id
            WHERE t.node_id=$1 AND u.id=$2 AND u.thread_id=t.node_id AND u.segment_id=t.current_segment_id AND u.state='running'
                AND EXISTS(SELECT 1 FROM items i WHERE i.turn_id=u.id AND i.segment_id=u.segment_id
                    AND i.native_ref=$3 AND i.status='completed'))")
            .bind(thread).bind(start.turn_id).bind(&reference).fetch_one(world.pool.pool()).await.expect("the admitted turn is readable")
    }).await;
    start
}

/// Reuse the admitted turn for all calls/replays; the first call explicitly authors
/// one fixture baseline if the test has not already started a turn of its own.
pub fn prepare(
    world: &World,
    thread: Id,
) -> std::pin::Pin<Box<dyn Future<Output = Id> + Send + '_>> {
    // Fixture wrappers must not add copies of the entire input-admission future
    // to the production preflight's stack. Keep the real admission and native
    // turn checks on the unchanged default worker stack.
    Box::pin(async move {
        let current: Option<Id> = world
            .scalar(
                "SELECT current_turn_id FROM threads WHERE node_id=$1",
                thread,
            )
            .await;
        if current.is_none() {
            start(
                world,
                thread,
                "Exercise the shared tools on this authored turn",
            )
            .await;
        }
        active(world, thread).await
    })
}

pub async fn send(
    world: &World,
    thread: Id,
    turn: Id,
    call_ref: &str,
    tool: OrchTool,
    arguments: Json,
) {
    assert_eq!(
        active(world, thread).await,
        turn,
        "tool effects belong to this exact current native turn"
    );
    let segment: Id = world
        .scalar("SELECT segment_id FROM turns WHERE id=$1", turn)
        .await;
    let stream = orch_link_contract::streams::Stream::Session {
        thread_id: thread,
        segment_id: segment,
    }
    .to_string();
    let before = sqlx::query_scalar::<_, i64>(
        "SELECT last_seq FROM link_cursors WHERE machine_id=$1 AND stream=$2",
    )
    .bind(world.machine_id)
    .bind(&stream)
    .fetch_optional(world.pool.pool())
    .await
    .expect("the fixture cursor is readable")
    .unwrap_or(0);
    world
        .say(
            thread,
            Event::OrchToolCall(OrchToolCall {
                turn_id: turn,
                call_ref: call_ref.into(),
                attempt: None,
                tool,
                arguments,
            }),
        )
        .await;
    // This scripted session emits only the test's ordered events. The completed
    // baseline was observed before sending; require ingestion even on a replay
    // whose canonical answer is already present, rather than reading it too early.
    wait(
        world,
        thread,
        "the native call report's committed session cursor",
        || async {
            sqlx::query_scalar::<_, i64>(
                "SELECT last_seq FROM link_cursors WHERE machine_id=$1 AND stream=$2",
            )
            .bind(world.machine_id)
            .bind(&stream)
            .fetch_optional(world.pool.pool())
            .await
            .expect("the native receipt cursor is readable")
            .is_some_and(|seen| seen > before)
        },
    )
    .await;
}

pub fn results(world: &World, thread: Id, call_ref: &str) -> Vec<OrchToolResult> {
    world
        .seen
        .of_kind(CommandKind::OrchToolResult)
        .into_iter()
        .filter_map(|command| match command {
            Command::OrchToolResult(answer)
                if answer.thread_id == thread && answer.call_ref == call_ref =>
            {
                Some(answer)
            }
            _ => None,
        })
        .collect()
}

pub async fn answer(world: &World, thread: Id, call_ref: &str) -> OrchToolResult {
    wait(
        world,
        thread,
        "the result for this thread and exact call reference",
        || {
            let found = !results(world, thread, call_ref).is_empty();
            async move { found }
        },
    )
    .await;
    results(world, thread, call_ref).remove(0)
}
