//! The screens' new-environment -> thread -> first-message flow retains work and starts it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{MachineEnvironmentCreate, MachineEnvironmentCreateBody};
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, FirstInput, ThreadCreate};
use orch_api_contract::source::SourceState;
use orch_api_contract::threads::{
    ContentPart, Delivery, StartingStep, ThreadState, ThreadSyncGet, ThreadSyncGetQuery,
};
use orch_link_contract::commands::{Command, CommandKind, CommandResult};
use orch_types::{Id, Time};
use rig::{MODEL, Scene};
use serde_json::json;

/// Register a new environment through the same ordinary route used by the creation UI. Only the
/// machine's facts/owner and its native replies are fixtures; the resource/page/thread paths are real.
async fn new_environment(s: &Scene) -> Id {
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: None,
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET owner_id=$2,home_id=n.id,org_id=n.org_id,facts=jsonb_set(facts || $3,'{policy}',COALESCE(facts->'policy','{}') || $4) FROM users u, nodes n WHERE machines.id=$1 AND u.id=$2 AND n.id=u.personal_org_id")
        .bind(s.rig.machine_id).bind(s.ada.id()).bind(json!({"environment_descriptor":descriptor}))
        .bind(json!({"environment.roots":["/workspace"],"environment.remote_write":true,"agent.max_sandbox":"workspace"}))
        .execute(s.rig.db.pool()).await.unwrap();
    // Owning a machine is its Owners row, as enrolling it writes.
    sqlx::query(
        "INSERT INTO permission_entries (id, object_kind, object_id, page, permission, holder_kind, holder_id,
                                         created_by, source)
         VALUES ($1, 'machine', $2, 'live', 'access.own', 'user', $3, $3, 'creation')
         ON CONFLICT DO NOTHING",
    )
    .bind(orch_types::new_id())
    .bind(s.rig.machine_id)
    .bind(s.ada.id())
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    s.rig.machine.handler.on(CommandKind::EnvAttach,|command|{
        let Command::EnvAttach(body)=command else {unreachable!()};
        Ok(CommandResult::EnvAttach(serde_json::from_value(json!({
            "os":"linux","arch":"x86_64","shell":{"name":"sh","path":"/bin/sh"},
            "user":"agent","home":"/home/agent","workspace_root":body.workspace_root.as_deref().unwrap_or("/workspace"),
            "path_style":"posix","case_sensitive":true,"executors":[{"kind":"orch-exec","version":"1","available":true,
                "reason":null,"sandbox_modes":["read_only","workspace","open"],"tool_sets":["plain"]}],
            "mcp_found":[],"index":"full","mounts":[],"private_views":false,"suspend":false,"git":true,"generation":1
        })).unwrap()))
    });
    let now = Time::new(s.rig.world.now());
    s.rig.machine.handler.on(CommandKind::FsStat,move |command|{
        let Command::FsStat(body)=command else {unreachable!()};
        Ok(CommandResult::FsStat(serde_json::from_value(json!({
            "name":"workspace","path":body.path,"type":"dir","size":0,"mtime":now,"mode":493,"link_target":null,"cached":false,
            "hash":null,"media_type":"inode/directory","is_text":false
        })).unwrap()))
    });
    s.rig
        .world
        .app
        .call(MachineEnvironmentCreate, &[&s.rig.machine_id])
        .token(&s.ada.token)
        .body(&MachineEnvironmentCreateBody {
            data_sources: None,
            home_id: Some(s.tree.org.base.id),
            name: "New workspace".into(),
            root: "/workspace/new".into(),
            home_node_id: Some(s.tree.topic.base.id),
            remote_write: Some(true),
            terminal: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("ordinary directory registration retains full native content")
        .base
        .id
}

fn request(s: &Scene, environment: Id, first: bool) -> CreateThread {
    CreateThread {
        binding: Some(CreateThreadBinding {
            agent_id: s.on.agent,
            provider_id: s.on.provider,
            arrangement: None,
        }),
        model: Some(MODEL.into()),
        environment: Some(EnvironmentChoice::Use(environment)),
        prepare_source: Some(true),
        first_input: first.then(|| FirstInput {
            content: vec![ContentPart::Text {
                text: "Keep this first message".into(),
            }],
            client_id: "first".into(),
        }),
        ..CreateThread::default()
    }
}

fn staged_request(s: &Scene, environment: Id, text: &str) -> CreateThread {
    let mut body = serde_json::to_value(request(s, environment, true)).unwrap();
    body["workspace_edits"] = json!([{"path":"AGENTS.md","text":text,"expected_hash":null}]);
    serde_json::from_value(body)
        .expect("creation accepts a conditional workspace file edit in its start plan")
}

#[tokio::test]
async fn the_first_input_waits_until_its_staged_shared_instructions_have_been_written() {
    use orch_link_contract::commands::ExpectedHash;
    let s = Scene::new().await;
    let environment = new_environment(&s).await;
    let text = "Use the checked-out workspace and keep these instructions shared.\n";
    let wanted = orch_types::Hash::of(text.as_bytes());
    let now = Time::new(s.rig.world.now());
    s.rig.machine.handler.on(CommandKind::FsWrite,move|command|{
        let Command::FsWrite(body)=command else{unreachable!()};
        assert_eq!(body.environment_id,environment); assert_eq!(body.path,"AGENTS.md");
        assert_eq!(body.blob,wanted); assert_eq!(body.expected_hash,Some(ExpectedHash::Absent));
        Ok(CommandResult::FsWrite(serde_json::from_value(json!({"name":"AGENTS.md","path":body.path,"type":"file","size":text.len(),"mtime":now,"mode":420,"link_target":null,"cached":false,"hash":wanted,"media_type":"text/markdown","is_text":true})).unwrap()))
    });
    s.rig.machine.handler.hold(CommandKind::FsWrite);
    let made = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&staged_request(&s, environment, text))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    rig::eventually("the start's actual file-write command", || async {
        !s.rig.seen.of_kind(CommandKind::FsWrite).is_empty()
    })
    .await;
    assert!(
        s.rig.turn_starts(made.base.id).is_empty(),
        "the accepted first input stays held while instructions are being written"
    );
    s.rig.machine.handler.release();
    let turn = s.rig.turn_start(made.base.id, 0).await;
    assert_eq!(turn.thread_id, made.base.id);
    let source: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT source FROM threads WHERE node_id=$1")
            .bind(made.base.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert!(
        source.unwrap().get("workspace_edits").is_none(),
        "an applied file edit is consumed instead of becoming a permanent thread instruction override"
    );
}

#[tokio::test]
async fn a_conflict_with_instructions_brought_by_the_source_keeps_both_texts_and_never_starts_the_agent()
 {
    use orch_link_contract::commands::{CommandError, ErrorCode as NativeError};
    let s = Scene::new().await;
    let environment = new_environment(&s).await;
    let proposed = "The staged instructions the person reviewed.\n";
    let source_text = std::sync::Arc::new(std::sync::Mutex::new(
        "The instructions brought by the source.\n".to_owned(),
    ));
    let current = source_text.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::FsWrite, move |command| {
            let Command::FsWrite(body) = command else {
                unreachable!()
            };
            assert!(
                body.expected_hash.is_some(),
                "an unknown source is never an unconditional overwrite"
            );
            let hash = orch_types::Hash::of(current.lock().unwrap().as_bytes());
            Err(CommandError {
                details: Some(json!({"current":{"hash":hash}})),
                ..CommandError::new(
                    NativeError::Conflict,
                    "The source brought a different AGENTS.md",
                )
            })
        });
    let made = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&staged_request(&s, environment, proposed))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    rig::eventually("the conflict in the start's own source state", || async {
        let value: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT source FROM threads WHERE node_id=$1")
                .bind(made.base.id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        value.is_some_and(|source| {
            source["info"]["state"] == "failed"
                && source["info"]["reason"] == "workspace.source_conflict"
        })
    })
    .await;
    assert!(s.rig.turn_starts(made.base.id).is_empty());
    assert_eq!(
        source_text.lock().unwrap().as_str(),
        "The instructions brought by the source.\n"
    );
    let kept: Option<String> = sqlx::query_scalar(
        "SELECT source->'workspace_edits'->0->>'text' FROM threads WHERE node_id=$1",
    )
    .bind(made.base.id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(kept.as_deref(), Some(proposed));
    let edit_query = orch_api_contract::source::EnvironmentSourceEditsQuery {
        thread_id: made.base.id,
    };
    let edits = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::source::EnvironmentSourceEditsGet,
            &[&environment],
        )
        .token(&s.ada.token)
        .query(&edit_query)
        .send()
        .await
        .ok();
    assert_eq!(edits.edits[0].text, proposed);
    s.rig
        .lets_use("node", made.base.id, "thread.view", &s.eve, json!({}))
        .await;
    let denied = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::source::EnvironmentSourceEditsGet,
            &[&environment],
        )
        .token(&s.eve.token)
        .query(&edit_query)
        .send_bytes()
        .await;
    assert_eq!(
        denied.status, 403,
        "viewing the conversation does not disclose its pending workspace file"
    );
    assert!(!String::from_utf8_lossy(&denied.raw).contains(proposed.trim_end()));
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_inputs WHERE thread_id=$1 AND state='pending'",
    )
    .bind(made.base.id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(pending, 1);
    let shown = s
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&made.base.id])
        .token(&s.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert!(
        !serde_json::to_string(&shown)
            .unwrap()
            .contains(proposed.trim_end()),
        "the staged file text is read through its file authority, not leaked by conversation metadata"
    );
    rig::eventually("the refused source attempt has finished before its replacement", || async {
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id=$1 AND state IN ('queued','running','undoing')")
            .bind(environment).fetch_one(s.rig.db.pool()).await.unwrap();
        count==0
    }).await;
    use orch_api_contract::source::{
        EnvironmentSourcePrepare, EnvironmentSourcePrepareBody, WorkspaceFileEdit,
    };
    let reviewed = orch_types::Hash::of(source_text.lock().unwrap().as_bytes());
    let replacement = WorkspaceFileEdit {
        path: "AGENTS.md".into(),
        text: proposed.into(),
        expected_hash: Some(reviewed),
    };
    let stale = s
        .rig
        .world
        .app
        .call(EnvironmentSourcePrepare, &[&environment])
        .token(&s.ada.token)
        .body(&EnvironmentSourcePrepareBody {
            thread_id: made.base.id,
            source: None,
            workspace_edits: Some(vec![replacement.clone()]),
            workspace_edit_revision: Some(orch_types::Hash::of(b"an older saved plan")),
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        source_text.lock().unwrap().as_str(),
        "The instructions brought by the source.\n"
    );
    assert!(s.rig.turn_starts(made.base.id).is_empty());
    let current = source_text.clone();
    let now = Time::new(s.rig.world.now());
    s.rig.machine.handler.on(CommandKind::FsWrite,move|command|{
        let Command::FsWrite(body)=command else {unreachable!()};
        assert_eq!(body.expected_hash,Some(orch_link_contract::commands::ExpectedHash::Is(reviewed)));
        assert_eq!(body.blob,orch_types::Hash::of(proposed.as_bytes()));
        *current.lock().unwrap()=proposed.into();
        Ok(CommandResult::FsWrite(serde_json::from_value(json!({"name":"AGENTS.md","path":body.path,"type":"file","size":proposed.len(),"mtime":now,"mode":420,"link_target":null,"cached":false,"hash":body.blob,"media_type":"text/markdown","is_text":true})).unwrap()))
    });
    s.rig
        .world
        .app
        .call(EnvironmentSourcePrepare, &[&environment])
        .token(&s.ada.token)
        .body(&EnvironmentSourcePrepareBody {
            thread_id: made.base.id,
            source: None,
            workspace_edits: Some(vec![replacement]),
            workspace_edit_revision: Some(edits.revision),
        })
        .send()
        .await
        .ok();
    let turn = s.rig.turn_start(made.base.id, 0).await;
    assert_eq!(turn.thread_id, made.base.id);
    assert_eq!(source_text.lock().unwrap().as_str(), proposed);
    let consumed = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::source::EnvironmentSourceEditsGet,
            &[&environment],
        )
        .token(&s.ada.token)
        .query(&edit_query)
        .send()
        .await
        .ok();
    assert!(
        consumed.edits.is_empty(),
        "the reviewed edit is consumed before the retained first input runs"
    );
}

#[tokio::test]
async fn a_new_environment_and_its_source_prepare_before_the_first_message_runs_in_either_creation_flow()
 {
    for send_with_create in [true, false] {
        let s = Scene::new().await;
        let environment = new_environment(&s).await;
        s.rig.machine.handler.hold(CommandKind::FsStat);
        let made = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&s.tree.topic.base.id])
            .token(&s.ada.token)
            .body(&request(&s, environment, send_with_create))
            .send()
            .await
            .ok()
            .into_full()
            .unwrap();
        if !send_with_create {
            s.say(
                &s.ada,
                made.base.id,
                "Keep this first message",
                Delivery::Queue,
                "first",
            )
            .await
            .ok();
        }
        rig::eventually("the source step, without a turn", || async {
            let mut conn = s.rig.world.conn().await;
            let row = orch_db::queries::threads::thread(&mut conn, made.base.id)
                .await
                .unwrap()
                .unwrap();
            row.state == "starting" && row.starting_step.as_deref() == Some("source")
        })
        .await;
        let waiting = s
            .rig
            .world
            .app
            .call(ThreadSyncGet, &[&made.base.id])
            .token(&s.ada.token)
            .query(&ThreadSyncGetQuery::default())
            .send()
            .await
            .ok()
            .thread;
        assert_eq!(waiting.state, ThreadState::Starting);
        assert_eq!(waiting.starting_step, Some(StartingStep::Source));
        assert_eq!(waiting.current_turn_id, None);
        assert!(s.rig.turn_starts(made.base.id).is_empty());
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM thread_inputs WHERE thread_id=$1 AND state='pending'",
        )
        .bind(made.base.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
        assert_eq!(
            pending, 1,
            "the original first message is accepted while source work waits"
        );
        s.rig.machine.handler.release();
        let turn = s.rig.turn_start(made.base.id, 0).await;
        assert_eq!(turn.thread_id, made.base.id);
        let ready = s
            .rig
            .world
            .app
            .call(ThreadSyncGet, &[&made.base.id])
            .token(&s.ada.token)
            .query(&ThreadSyncGetQuery::default())
            .send()
            .await
            .ok()
            .thread;
        assert_eq!(
            ready.source.as_ref().map(|source| source.state),
            Some(SourceState::Ready)
        );
        s.rig.say(made.base.id, rig::turn_began(turn.turn_id)).await;
        s.rig
            .say(
                made.base.id,
                rig::turn_ended(
                    turn.turn_id,
                    orch_link_contract::events::TurnOutcome::Completed,
                ),
            )
            .await;
        s.rig.wait_turn(turn.turn_id, "completed").await;
    }
}

#[tokio::test]
async fn a_refused_first_message_rolls_back_the_thread_source_segment_page_and_settings() {
    let s = Scene::new().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id=$1")
        .bind(s.tree.topic.base.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    for empty_content in [true, false] {
        let mut body = request(&s, s.on.environment, true);
        let first = body.first_input.as_mut().unwrap();
        if empty_content {
            first.content.clear();
        } else {
            first.client_id.clear();
        }
        let refused = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&s.tree.topic.base.id])
            .token(&s.ada.token)
            .body(&body)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id=$1")
            .bind(s.tree.topic.base.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(
            after, before,
            "a failed start leaves no unusable New thread"
        );
    }
    assert!(s.rig.seen.of_kind(CommandKind::TurnStart).is_empty());
}
