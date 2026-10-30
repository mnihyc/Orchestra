//! A real scripted harness calls an Orch tool and a person follows its frozen responsibility into retained HTTP history.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#[path = "../../engine/tests/common/mod.rs"]
mod common;
#[path = "../../engine/tests/hosting/mod.rs"]
mod hosting;
#[path = "../../engine/tests/shared/mod.rs"]
mod shared;

use axum::{Json, Router, extract::State, routing::post};
use orch_api_contract::{
    common::ErrorCode,
    nodes::{NodeAuditList, NodeAuditListQuery},
    ownership::NodeDelete,
    threads::{
        ThreadItemList, ThreadItemListQuery, ThreadOutlineGet, ThreadSegmentList, ThreadSyncGet,
        ThreadSyncGetQuery, ThreadTurnList,
    },
};
use orch_api_core::{
    middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash},
    registry::Registry,
    testing::TestApp,
};
use orch_link_contract::{
    commands::CommandKind,
    events::{Event, OrchToolCall},
};
use orch_testkit::doubles::model::{MockConfig, MockLlm};
use orch_types::{Id, new_id};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;

struct Probe(String);
impl Drop for Probe {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "--force", &self.0])
            .output();
    }
}

async fn listener() -> tokio::net::TcpListener {
    for port in 41700..42000 {
        if let Ok(listener) =
            tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await
        {
            return listener;
        }
    }
    panic!("no free loopback port for the trace walk")
}
async fn models() -> MockLlm {
    for port in 41700..42000 {
        if let Ok(server) = MockLlm::start_on(
            SocketAddr::from(([127, 0, 0, 1], port)),
            MockConfig::default().with_credential("trace-scripted-model").with_default_script("tool mcp__orch.search_shared {\"query\":\"trace needle\"}\nthen\nsay The trace search was completed."),
        )
        .await
        {
            return server;
        }
    }
    panic!("no free loopback port for the scripted trace model")
}

async fn api(world: &common::World, clock: orch_testkit::clock::ManualClock) -> (TestApp, String) {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::with_config(
        orch_db::Db::from_pool(world.pool.pool().clone()),
        clock,
        registry,
        orch_config::ServerConfig::default(),
    );
    let token = new_token(ACCESS_TOKEN_PREFIX);
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::identity::testing::insert_session(
        &mut conn,
        new_id(),
        world.ada,
        &token_hash(&token),
        world.clock.now() + chrono::Duration::days(400),
    )
    .await
    .unwrap();
    (app, token)
}

#[derive(Clone)]
struct Mcp {
    world: Arc<common::World>,
    thread: Id,
    turn: Id,
    calls: Arc<AtomicUsize>,
}
async fn mcp(
    State(state): State<Mcp>,
    Json(message): Json<Value>,
) -> (http::StatusCode, Json<Value>) {
    let Some(id) = message.get("id").cloned() else {
        return (http::StatusCode::ACCEPTED, Json(Value::Null));
    };
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => {
            json!({"protocolVersion":message["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"Orch trace walk","version":"1"}})
        }
        "tools/list" => {
            json!({"tools":[{"name":"search_shared","description":"Search the places shared with this thread","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}}]})
        }
        "tools/call" => {
            state.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(message["params"]["name"], "search_shared");
            let call = new_id().to_string();
            state
                .world
                .say(
                    state.thread,
                    Event::OrchToolCall(OrchToolCall {
                        turn_id: state.turn,
                        call_ref: call.clone(),
                        attempt: None,
                        tool: orch_link_contract::commands::OrchTool::SearchShared,
                        arguments: message["params"]["arguments"].clone(),
                    }),
                )
                .await;
            let result = shared::answer_to(&state.world, state.thread, &call).await;
            let result = shared::ok(result);
            json!({"content":[{"type":"text","text":result.to_string()}],"isError":false})
        }
        "ping" => json!({}),
        other => panic!("the native harness sent unexpected MCP method {other}"),
    };
    (
        http::StatusCode::OK,
        Json(json!({"jsonrpc":"2.0","id":id,"result":result})),
    )
}

#[tokio::test]
async fn a_real_harness_tool_is_findable_for_its_original_person_and_deleted_history_is_read_without_waking_it()
 {
    let clock = orch_testkit::clock::manual();
    let world = Arc::new(common::World::with_clock(Arc::new(clock.clone())).await);
    shared::the_agent_takes_tools_again(&world).await;
    let thread = world.thread("Trace native walk").await;
    let (turn, _) = world.running_turn(thread, "Trace a real native tool").await;
    shared::the_agent_takes_tools_again(&world).await;
    let model = models().await;
    let socket = listener().await;
    let address = socket.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new().route("/mcp", post(mcp)).with_state(Mcp {
        world: world.clone(),
        thread,
        turn: turn.turn_id,
        calls: calls.clone(),
    });
    let server = tokio::spawn(async move {
        axum::serve(socket, router).await.unwrap();
    });
    let owned = new_id().simple().to_string();
    let name = format!("orchprobe-trace-agent-{owned}");
    assert!(
        !tokio::process::Command::new("docker")
            .args(["inspect", &name])
            .output()
            .await
            .unwrap()
            .status
            .success()
    );
    let probe = Probe(name.clone());
    let root = std::env::current_dir().unwrap().join(".tmp");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("trace-native-")
        .tempdir_in(root)
        .unwrap();
    let mut command = tokio::process::Command::new("docker");
    command.args([
        "run",
        "--rm",
        "--interactive",
        "--name",
        &name,
        "--network",
        "host",
        "--env",
        "ORCH_SCRIPTED_MODEL_TOKEN=trace-scripted-model",
        "-v",
    ]);
    command
        .arg(format!("{}:/workspace", directory.path().display()))
        .args([
            "orch-dev-harness",
            "codex",
            "exec",
            "--json",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--skip-git-repo-check",
            "-C",
            "/workspace",
            "--sandbox",
            "workspace-write",
            "--model",
            "mock-large",
        ]);
    for setting in [
        "model_provider=\"orch_trace\"".to_owned(),
        "model_providers.orch_trace.name=\"Orch scripted trace\"".to_owned(),
        format!(
            "model_providers.orch_trace.base_url=\"http://{}/v1\"",
            model.addr()
        ),
        "model_providers.orch_trace.wire_api=\"responses\"".to_owned(),
        "model_providers.orch_trace.env_key=\"ORCH_SCRIPTED_MODEL_TOKEN\"".to_owned(),
        "model_providers.orch_trace.requires_openai_auth=false".to_owned(),
        format!("mcp_servers.orch.url=\"http://{address}/mcp\""),
        "mcp_servers.orch.required=true".to_owned(),
        "mcp_servers.orch.default_tools_approval_mode=\"approve\"".to_owned(),
        "web_search=\"disabled\"".to_owned(),
        "analytics.enabled=false".to_owned(),
        "feedback.enabled=false".to_owned(),
    ] {
        command.arg("-c").arg(setting);
    }
    command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Search the shared places once.\n")
        .await
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(180), child.wait_with_output())
        .await
        .expect("the native scripted trace walk timed out")
        .unwrap();
    assert!(
        output.status.success(),
        "native harness failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let transcript = String::from_utf8(output.stdout).unwrap();
    assert!(
        transcript.contains("trace search was completed"),
        "{transcript}"
    );
    assert!(!model.requests().is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the real harness must call the production tool: {transcript}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (app, token) = api(&world, clock).await;
    let administrator = world.person("trace_administrator").await;
    let admin_token = new_token(ACCESS_TOKEN_PREFIX);
    {
        let mut conn = world.pool.acquire().await.unwrap();
        sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
            .bind(administrator)
            .execute(&mut *conn)
            .await
            .unwrap();
        orch_db::queries::identity::testing::insert_session(
            &mut conn,
            new_id(),
            administrator,
            &token_hash(&admin_token),
            world.clock.now() + chrono::Duration::days(400),
        )
        .await
        .unwrap();
    }
    let trail = app
        .call(NodeAuditList, &[&world.root])
        .token(&token)
        .query(&NodeAuditListQuery {
            actor: Some(world.ada),
            include_agents: Some(true),
            action: Some("share.searched_by_agent".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let acts:Vec<(String,Option<Id>,Vec<Id>)>=sqlx::query_as("SELECT action,for_user_id,scope_node_ids FROM audit_log WHERE actor_kind='agent' AND actor_id=$1").bind(thread).fetch_all(world.pool.pool()).await.unwrap();
    assert_eq!(
        trail.items.len(),
        1,
        "native transcript: {transcript}; committed agent acts: {acts:?}"
    );
    let ordinary_admin = app
        .call(orch_api_contract::admin::AdminAuditList, &[])
        .token(&admin_token)
        .query(&orch_api_contract::admin::AdminAuditListQuery {
            actor: Some(world.ada),
            include_agents: Some(true),
            action: Some("share.searched_by_agent".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(
        ordinary_admin.status, 403,
        "a system-admin person in ordinary mode has no administrative audit standing"
    );
    app.call(orch_api_contract::auth::MeAdminModeSet, &[])
        .token(&admin_token)
        .body(&orch_api_contract::auth::AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();
    let management = app
        .call(orch_api_contract::admin::AdminAuditList, &[])
        .token(&admin_token)
        .query(&orch_api_contract::admin::AdminAuditListQuery {
            actor: Some(world.ada),
            include_agents: Some(true),
            action: Some("share.searched_by_agent".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        management
            .items
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        trail.items.iter().map(|row| row.id).collect::<Vec<_>>()
    );
    match &trail.items[0].actor {
        orch_api_contract::nodes::AuditActor::Agent {
            thread: actor,
            for_user,
        } => {
            assert_eq!(actor.id, thread);
            assert_eq!(for_user.as_ref().unwrap().id, world.ada);
        }
        other => panic!("{other:?}"),
    };
    assert!(
        !trail.items[0]
            .details
            .to_string()
            .contains("Trace a real native tool")
    );
    world
        .say(
            thread,
            common::turn_ended(
                turn.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    world.wait_turn(turn.turn_id, "completed").await;
    // The native terminal row precedes its durable accounting/notification and
    // source-end follow-up. Finish that authored work before deleting the place
    // and taking the exact no-side-effect baseline for retained-history reads.
    common::eventually(
        "the native trace turn's terminal and source work settle",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT end_work IS NULL AND NOT files_end_due AND NOT files_work_pending
            FROM turns WHERE id=$1",
            )
            .bind(turn.turn_id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
        },
    )
    .await;
    app.call(NodeDelete, &[&world.topic])
        .token(&token)
        .raw_body(&json!({}))
        .send()
        .await
        .ok();
    let after_deletion = app
        .call(NodeAuditList, &[&world.root])
        .token(&token)
        .query(&NodeAuditListQuery {
            actor: Some(world.ada),
            include_agents: Some(true),
            action: Some("share.searched_by_agent".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(after_deletion.items.len(), 1);
    let retained_management = app
        .call(orch_api_contract::admin::AdminAuditList, &[])
        .token(&admin_token)
        .query(&orch_api_contract::admin::AdminAuditListQuery {
            actor: Some(world.ada),
            include_agents: Some(true),
            action: Some("share.searched_by_agent".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(retained_management.items[0].id, management.items[0].id);
    assert!(
        matches!(&after_deletion.items[0].actor,orch_api_contract::nodes::AuditActor::Agent{thread:actor,for_user:Some(person)} if actor.id==thread && person.id==world.ada)
    );
    let people_only = app
        .call(NodeAuditList, &[&world.root])
        .token(&token)
        .query(&NodeAuditListQuery {
            actor: Some(world.ada),
            include_agents: Some(false),
            action: Some("share.searched_by_agent".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(people_only.items.is_empty());
    // Deleting the place removes its environment by a job of its own, which tells its owner when it is
    // over: that is the deletion's work, settled before the baseline of reads that change nothing.
    common::eventually("the deleted place's removal jobs end", || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT NOT EXISTS(SELECT 1 FROM jobs WHERE state IN ('queued','running'))",
        )
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
    })
    .await;
    let before:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM thread_reads),(SELECT count(*) FROM subscriptions),(SELECT count(*) FROM notifications),(SELECT count(*) FROM turns)").fetch_one(world.pool.pool()).await.unwrap();
    let before_rows:Value=sqlx::query_scalar("SELECT jsonb_build_object('reads',(SELECT jsonb_agg(to_jsonb(r) ORDER BY to_jsonb(r)::text) FROM thread_reads r),'subscriptions',(SELECT jsonb_agg(to_jsonb(s) ORDER BY to_jsonb(s)::text) FROM subscriptions s),'notices',(SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM notifications n))").fetch_one(world.pool.pool()).await.unwrap();
    let starts = world.ran(CommandKind::TurnStart);
    let normal = app
        .call(ThreadSyncGet, &[&thread])
        .token(&token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .problem();
    assert_ne!(normal.status, 200);
    for _ in 0..2 {
        let sync = app
            .call(ThreadSyncGet, &[&thread])
            .token(&token)
            .query(&ThreadSyncGetQuery {
                inspect_deleted: Some(true),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(sync.thread.base.inspection.unwrap().readonly);
        assert!(
            !sync.thread.input_access.allowed
                && !sync.thread.can.fork
                && !sync.thread.can.archive
                && !sync.thread.can.export
                && !sync.thread.can.place
        );
        let items = app
            .call(ThreadItemList, &[&thread])
            .token(&token)
            .query(&ThreadItemListQuery {
                after: Some(0),
                inspect_deleted: Some(true),
                ..Default::default()
            })
            .send()
            .await;
        assert_eq!(items.status, 200, "{:?}", items.body);
        assert!(items.headers.contains_key("orch-inspection"));
        let outline = app
            .call(ThreadOutlineGet, &[&thread])
            .token(&token)
            .raw_query("inspect_deleted=true")
            .send()
            .await;
        assert_eq!(outline.status, 200, "{:?}", outline.body);
        let turns = app
            .call(ThreadTurnList, &[&thread])
            .token(&token)
            .raw_query("inspect_deleted=true")
            .send()
            .await;
        assert_eq!(turns.status, 200, "{:?}", turns.body);
        assert!(turns.headers.contains_key("orch-inspection"));
        let segments = app
            .call(ThreadSegmentList, &[&thread])
            .token(&token)
            .raw_query("inspect_deleted=true")
            .send()
            .await;
        assert_eq!(segments.status, 200, "{:?}", segments.body);
        assert!(segments.headers.contains_key("orch-inspection"));
    }
    let after:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM thread_reads),(SELECT count(*) FROM subscriptions),(SELECT count(*) FROM notifications),(SELECT count(*) FROM turns)").fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(
        before, after,
        "retained reads cannot change read/subscription/notification/turn counts"
    );
    let after_rows:Value=sqlx::query_scalar("SELECT jsonb_build_object('reads',(SELECT jsonb_agg(to_jsonb(r) ORDER BY to_jsonb(r)::text) FROM thread_reads r),'subscriptions',(SELECT jsonb_agg(to_jsonb(s) ORDER BY to_jsonb(s)::text) FROM subscriptions s),'notices',(SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM notifications n))").fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(before_rows, after_rows);
    assert_eq!(starts, world.ran(CommandKind::TurnStart));
    let (daily,history):(i64,bool)=sqlx::query_as("SELECT count(*),bool_or(history) FROM audit_log WHERE action='deleted.inspected' AND object_id=$1").bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!((daily, history), (1, false));
    let flagged = app
        .call(
            orch_api_contract::workspace::EnvironmentFileList,
            &[&world.environment],
        )
        .token(&token)
        .raw_query("inspect_deleted=true")
        .send()
        .await
        .problem();
    assert_eq!(flagged.code, ErrorCode::RequestInvalid);
    server.abort();
    let _ = server.await;
    drop(probe);
}

#[tokio::test]
async fn a_person_finds_a_removed_environment_inspects_its_frozen_day_and_restores_it_stopped_once_without_a_turn()
 {
    use orch_api_contract::environment::{
        EnvironmentDelete, EnvironmentDeleteQuery, EnvironmentGet, EnvironmentRestore,
        EnvironmentRestoreBody, NodeDeletedEnvironmentList, NodeDeletedEnvironmentListQuery,
    };
    let clock = orch_testkit::clock::manual();
    let world = common::World::with_clock(Arc::new(clock.clone())).await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "trace-http-kept", 32_768).await;
    let template = hosting::template(&world).await;
    let made = hosting::make(&world, template, "Trace HTTP kept files").await;
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let _unit = hosting::enroll(&world, host.started().unwrap()).await;
    assert_eq!(hosting::wait_over(&world, made.job.id).await.0, "succeeded");
    let (app, token) = api(&world, clock.clone()).await;
    let accepted = app
        .call(EnvironmentDelete, &[&made.id])
        .token(&token)
        .query(&EnvironmentDeleteQuery {
            confirm: "Trace HTTP kept files".to_owned(),
            at: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(accepted.removal.unwrap().keep_days, 7);
    clock.advance(chrono::Duration::seconds(31));
    world.engine().kick();
    hosting::await_event(
        "actual unit removal makes the kept row",
        Duration::from_secs(60),
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT removal_phase='kept' FROM environments WHERE id=$1",
            )
            .bind(made.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
        },
    )
    .await;
    let list = app
        .call(NodeDeletedEnvironmentList, &[&world.topic])
        .token(&token)
        .query(&NodeDeletedEnvironmentListQuery::default())
        .send()
        .await
        .ok();
    let kept = list
        .items
        .iter()
        .find(|row| row.environment.id == made.id)
        .unwrap();
    assert_eq!(kept.removal.keep_days, 7);
    assert!(kept.removal.bytes.is_none() && kept.removal.reason.is_some());
    let revision = kept.removal.revision;
    for _ in 0..2 {
        let detail = app
            .call(EnvironmentGet, &[&made.id])
            .token(&token)
            .raw_query("inspect_deleted=true")
            .send()
            .await
            .ok();
        assert!(detail.inspection.unwrap().readonly);
        assert_eq!(detail.removal.unwrap().purge_at, kept.removal.purge_at);
    }
    let stale = app
        .call(EnvironmentRestore, &[&made.id])
        .token(&token)
        .body(&EnvironmentRestoreBody {
            expected_revision: revision - 1,
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let job = app
        .call(EnvironmentRestore, &[&made.id])
        .token(&token)
        .body(&EnvironmentRestoreBody {
            expected_revision: revision,
        })
        .send()
        .await
        .ok();
    host.wait_for(CommandKind::NodeRuntimeStart, 2).await;
    let repeated = app
        .call(EnvironmentRestore, &[&made.id])
        .token(&token)
        .body(&EnvironmentRestoreBody {
            expected_revision: revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(repeated.id, job.id);
    let _restored = hosting::enroll(&world, host.started().unwrap()).await;
    let (state, _, error) = hosting::wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let restored = app
        .call(EnvironmentGet, &[&made.id])
        .token(&token)
        .send()
        .await
        .ok();
    assert_eq!(restored.generation, 2);
    assert_eq!(
        restored.base.status,
        orch_api_contract::environment::EnvironmentStatus::Stopped
    );
    assert!(restored.removal.is_none());
    assert_eq!(world.ran(CommandKind::TurnStart), 0);
    let (count,history):(i64,bool)=sqlx::query_as("SELECT count(*),bool_or(history) FROM audit_log WHERE action='deleted.inspected' AND object_id=$1").bind(made.id).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!((count, history), (1, false));
}
