//! Physical directory and host reports plus bounded fixture diagnostics, independent of placement labels.

use crate::common::{PATIENCE, World};
use orch_driver_contract::descriptor::{DriverDescriptor, EnvironmentDataStorage};
use orch_link_contract::policy::{LocalPolicy, PolicyKey, PolicyValues};
use orch_link_contract::session::{MachineFacts, Role};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

/// The same sample interval as common::eventually; neither the predicate nor the wait gets a larger bound.
const SAMPLE_EVERY: Duration = Duration::from_millis(20);
/// A failed observation gets at most two further seconds for diagnostics, never a further admission chance.
const DIAGNOSTIC_BUDGET: Duration = Duration::from_secs(2);
/// Four recent inputs/jobs/records retain the active attempt and its predecessor without dumping a world.
const DIAGNOSTIC_ROWS: i64 = 4;
/// Startup plus the current file-operation sequence fits in 32 kinds; older commands remain counted by kind.
const DIAGNOSTIC_COMMANDS: usize = 32;
/// The scripted host's normal capacity; reports do not claim a smaller machine than its fixture.
const MEMORY_MB: i64 = 16_384;

/// A Linux connector serving borrowed directories, with no declared machine isolation or owned data allocation.
pub fn directory_descriptor() -> DriverDescriptor {
    DriverDescriptor::registered_directories(
        "0.1.0",
        EnvironmentDataStorage {
            made_by_orch: None,
            existing_directory: true,
            reason: None,
        },
    )
}

/// Ingests the typed machine-report shape through the same database supplier as the API backplane.
/// The engine-only scripted applier deliberately does not ingest machine.facts events itself.
pub async fn report(
    world: &World,
    machine: Id,
    directory: bool,
    remote_write: Option<bool>,
    max_sandbox: &str,
) {
    let row = {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_db::queries::resources::machine(&mut conn, machine)
            .await
            .unwrap()
            .unwrap()
    };
    let roles: Vec<Role> = serde_json::from_value(json!(row.roles)).unwrap();
    let descriptor = (directory && roles.contains(&Role::Environment)).then(directory_descriptor);
    let policy = remote_write.map(|write| {
        let local = PolicyValues::from([
            ("environment.remote_write".into(), json!(write)),
            ("agent.max_sandbox".into(), json!(max_sandbox)),
            (
                "agent.harnesses".into(),
                json!([{"kind":"fake","path":"/test/harness"}]),
            ),
            ("environment.roots".into(), json!(["/home/ada"])),
        ]);
        let mut policy =
            LocalPolicy::resolve(&["*".into()], &local, &PolicyValues::default()).unwrap();
        policy.roles = Some(roles.clone());
        policy
    });
    let facts: MachineFacts = serde_json::from_value(json!({
        "hostname":format!("fixture-{machine}"),"os_version":"scripted Linux","cpus":8,
        "memory_bytes":MEMORY_MB*1024*1024,"harnesses":row.facts.get("harnesses").cloned().unwrap_or_else(||json!([])),
        "adapters":row.facts.get("adapters").cloned().unwrap_or_else(||json!([])),"plugins":[],
        "environment_descriptor":descriptor,"policy":policy,
        "rates":{"up_bps":0,"down_bps":0,"round_trip_ms":0},"clock_offset_ms":0
    })).unwrap();
    let written = serde_json::to_value(&facts).unwrap();
    if let Some(write) = remote_write {
        if PolicyKey::EnvironmentRemoteWrite.applies_to_roles(&roles) {
            assert_eq!(
                written.pointer("/policy/environment.remote_write"),
                Some(&json!(write))
            );
        } else {
            assert!(
                written
                    .pointer("/policy/environment.remote_write")
                    .is_none()
            );
        }
        assert_eq!(
            written.pointer("/policy/agent.max_sandbox"),
            Some(&json!(max_sandbox))
        );
    } else {
        assert!(written["policy"].is_null());
    }
    let mut tx = world.pool.begin().await.unwrap();
    // Zero rates are not measurements; the weight has no effect on this report's averages.
    orch_db::queries::resources::set_machine_facts(
        &mut tx,
        machine,
        &written,
        (0, 0),
        1.0,
        world.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let stored: Json = world
        .scalar("SELECT facts FROM machines WHERE id=$1", machine)
        .await;
    assert_eq!(stored["policy"], written["policy"]);
    assert_eq!(
        stored.get("environment_descriptor"),
        written.get("environment_descriptor")
    );
}

/// An explicitly existing thread directory, with native stat and an unprepared recording baseline.
/// Unknown paths still answer NotFound; no write, sandbox or provider permission is inferred here.
pub fn existing_thread_directory(
    world: &World,
    machine: &super::Unit,
    environment: Id,
    directory: &str,
) {
    use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
    use orch_link_contract::commands::{
        Command, CommandError, CommandKind, CommandResult, ErrorCode,
    };
    use orch_link_contract::source::{FsRecordResult, RecordState};
    let expected = directory.to_owned();
    let at = orch_types::Time::new(world.clock.now());
    machine
        .added
        .machine
        .handler
        .on(CommandKind::FsStat, move |command| {
            let Command::FsStat(body) = command else {
                unreachable!()
            };
            assert_eq!(body.environment_id, environment);
            if body.mount.is_some() || body.path != expected {
                return Err(CommandError::new(
                    ErrorCode::NotFound,
                    "the fixture has no such directory",
                ));
            }
            Ok(CommandResult::FsStat(FileStat {
                entry: DirEntry {
                    name: expected.rsplit('/').next().unwrap_or_default().to_owned(),
                    path: expected.clone(),
                    kind: DirEntryKind::Dir,
                    size: 0,
                    mtime: at,
                    mode: 0o755,
                    link_target: None,
                    cached: false,
                },
                hash: None,
                media_type: "inode/directory".into(),
                is_text: false,
            }))
        });
    let expected = directory.to_owned();
    machine
        .added
        .machine
        .handler
        .on(CommandKind::FsRecord, move |command| {
            let Command::FsRecord(body) = command else {
                unreachable!()
            };
            assert_eq!(body.environment_id, environment);
            assert_eq!(body.directory, expected);
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

/// Waits for a real native attach/current machine session, not only an arbitrary Running row.
pub async fn attached(world: &World, environment: Id) {
    crate::common::eventually(
        "the directory attaches in its current machine session",
        || is_attached(world, environment),
    )
    .await;
}

/// The attach fence uses the actual presence timestamp used by connected_out_of_step.
pub async fn is_attached(world: &World, environment: Id) -> bool {
    sqlx::query_scalar::<_,bool>("SELECT COALESCE(e.status='running' AND e.attached_session=COALESCE(m.session_started_at,m.last_seen_at)
            AND m.online AND COALESCE(m.session_started_at,m.last_seen_at) IS NOT NULL,false)
            FROM environments e JOIN machines m ON m.id=e.machine_id WHERE e.id=$1")
        .bind(environment).fetch_one(world.pool.pool()).await.unwrap()
}

/// Prints the same bounded snapshot before a disputed readiness assertion, without waiting for another state.
pub async fn describe(world: &World, environment: Id, thread: Id, label: &str) {
    let observed =
        tokio::time::timeout(DIAGNOSTIC_BUDGET, snapshot(world, environment, thread)).await;
    eprintln!("{label}: {observed:?}");
}

/// A timeout retains the tested deadline and prints only bounded state/identity data, never credentials or input text.
pub async fn wait<F, Fut>(world: &World, environment: Id, thread: Id, what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let result = tokio::time::timeout(PATIENCE, async {
        loop {
            if check().await {
                return;
            }
            tokio::time::sleep(SAMPLE_EVERY).await;
        }
    })
    .await;
    if result.is_err() {
        let observed =
            tokio::time::timeout(DIAGNOSTIC_BUDGET, snapshot(world, environment, thread)).await;
        panic!(
            "gave up waiting for {what} within {PATIENCE:?}; state/binding/source/input diagnostics: {observed:?}"
        );
    }
}

/// Stored identity and selected readiness facts, plus command kinds across every physical and unit connector.
pub async fn snapshot(world: &World, environment: Id, thread: Id) -> Json {
    let mut conn = world.pool.acquire().await.unwrap();
    let state:Json=sqlx::query_scalar("SELECT jsonb_build_object(
        'state',to_jsonb(t)->'state','blocked_reason',to_jsonb(t)->'blocked_reason',
        'blocked_detail',to_jsonb(t)->'blocked_detail','starting_step',to_jsonb(t)->'starting_step',
        'environment',to_jsonb(t)->'environment_id','arrangement',to_jsonb(t)->'arrangement',
        'executor',to_jsonb(t)->'executor','tool_set',to_jsonb(t)->'tool_set',
        'current_turn',to_jsonb(t)->'current_turn_id','current_segment',to_jsonb(t)->'current_segment_id',
        'turn',(SELECT jsonb_build_object('state',to_jsonb(turn)->'state','error_code',to_jsonb(turn)#>'{error,code}')
            FROM turns turn WHERE turn.id=t.current_turn_id),
        'source_state',t.source#>'{info,state}','source_reason',t.source#>'{info,reason}',
        'source_job',t.source#>'{info,job}','preparation',to_jsonb(t)->'preparation',
        'segment',(SELECT jsonb_build_object('environment',to_jsonb(s)->'environment_id','arrangement',to_jsonb(s)->'arrangement',
            'executor',to_jsonb(s)->'executor','tool_set',to_jsonb(s)->'tool_set','generation',to_jsonb(s)->'env_generation')
            FROM segments s WHERE s.id=t.current_segment_id),
        'inputs',(SELECT jsonb_agg(jsonb_build_object('id',i.id,'state',i.state,'detail',i.state_detail,'turn',i.turn_id))
            FROM (SELECT * FROM thread_inputs WHERE thread_id=t.node_id ORDER BY created_at DESC LIMIT $2) i))
        FROM threads t WHERE t.node_id=$1")
        .bind(thread).bind(DIAGNOSTIC_ROWS).fetch_one(&mut *conn).await.unwrap();
    let environment_state:Json=sqlx::query_scalar("SELECT jsonb_build_object('status',status,'detail',status_detail,
        'machine',machine_id,'template',template_id,'generation',generation,'attached_session',attached_session,
        'executors',facts->'executors','after_up',facts->'after_up') FROM environments WHERE id=$1")
        .bind(environment).fetch_one(&mut *conn).await.unwrap();
    let physical = orch_db::queries::environment_facts::of(&mut conn, environment)
        .await
        .unwrap();
    let jobs: Json = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(jsonb_build_object('id',j.id,'kind',j.kind,'state',j.state,
        'phase',j.progress->'phase','error_code',j.error->'code')),'[]'::jsonb)
        FROM (SELECT * FROM jobs WHERE environment_id=$1 ORDER BY created_at DESC LIMIT $2) j",
    )
    .bind(environment)
    .bind(DIAGNOSTIC_ROWS)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let recorded:Json=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(jsonb_build_object('id',s.id,
        'state',to_jsonb(s)->'state','reason',to_jsonb(s)->'reason','generation',to_jsonb(s)->'env_generation')),'[]'::jsonb)
        FROM (SELECT * FROM snapshots WHERE environment_id=$1 ORDER BY created_at DESC LIMIT $2) s")
        .bind(environment).bind(DIAGNOSTIC_ROWS).fetch_one(&mut *conn).await.unwrap();
    drop(conn);
    let ordered = world.ordered();
    let mut counts = BTreeMap::new();
    for (_, command) in &ordered {
        *counts.entry(command.kind().as_str()).or_insert(0usize) += 1;
    }
    let from = ordered.len().saturating_sub(DIAGNOSTIC_COMMANDS);
    json!({"thread":state,"environment":environment_state,"jobs":jobs,"recordings":recorded,"physical":physical.map(|facts|json!({
        "machine":facts.machine_id,"descriptor":facts.descriptor,"sandbox_max":facts.policy.as_ref().and_then(|policy|policy.get("agent.max_sandbox")),
        "remote_write":facts.policy.as_ref().and_then(|policy|policy.get("environment.remote_write"))})),
        "command_counts":counts,"commands":ordered.into_iter().skip(from).map(|(machine,command)|json!({"machine":machine,"kind":command.kind().as_str()})).collect::<Vec<_>>()})
}
