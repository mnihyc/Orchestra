//! A real separated turn loses only its selected local executor, and its actual receipt reaches Engine.
//! Commands/session admission use the existing scripted Engine fixture; this is not full connector orchestration.
//! The removal/watch trigger has separate controls; this case observes the native outcome of an explicit stop.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
#[path = "../../../../extensions/agents/codex/tests/common/mod.rs"]
mod native;

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt as _;
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};
use orch_agent_contract::events::{
    ItemContent, SessionEvent, ToolCallPayload, ToolPayload, TurnCompleted, TurnOutcome,
};
use orch_agent_contract::session::SessionMode;
use orch_authz::Caller;
use orch_envhost::executors::Executors;
use orch_executor_codex::server::{CodexConfig, CodexExecServer};
use orch_executor_contract::context::EnvironmentContext;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, SessionOpenResult,
};
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json, new_id};
use serde_json::json;

/// Only containers of this one fixture are removed, even during assertion unwinding.
struct Containers(String);
impl Drop for Containers {
    fn drop(&mut self) {
        let listed = std::process::Command::new("docker")
            .args(["ps", "-aq", "--filter", &format!("name=^{}-", self.0)])
            .output();
        if let Ok(listed) = listed {
            for id in String::from_utf8_lossy(&listed.stdout).split_whitespace() {
                let _ = std::process::Command::new("docker")
                    .args(["rm", "-f", id])
                    .output();
            }
        }
    }
}

fn owned_containers(prefix: &str) -> Vec<String> {
    let listed = std::process::Command::new("docker")
        .args(["ps", "-q", "--filter", &format!("name=^{prefix}-")])
        .output()
        .unwrap();
    assert!(
        listed.status.success(),
        "owned native process inspection succeeded"
    );
    String::from_utf8_lossy(&listed.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn expected_build() -> String {
    let builds: Vec<Json> =
        serde_json::from_str(include_str!("../../../../deploy/harness-builds.json")).unwrap();
    builds
        .iter()
        .find(|build| {
            build["kind"] == "codex"
                && build["os"] == "linux"
                && build["arch"] == std::env::consts::ARCH
                && build["is_default"] == true
        })
        .unwrap()["version"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The consumer fixture declares a connected directory and explicit owner policy, not isolation.
/// Its ordinary scripted Orch executor report is independent of the real native peer exercised below.
async fn connected_consumer(world: &common::World, context: &EnvironmentContext) {
    let root = context.workspace_root.clone();
    let generation = context.generation;
    world
        .machine
        .handler
        .on(CommandKind::EnvAttach, move |command| {
            let Command::EnvAttach(attach) = command else {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    "unexpected connected proof command",
                ));
            };
            // The existing connected confinement-matrix fixture's typed attachment producer.
            serde_json::from_value(json!({
                "os":"linux","arch":std::env::consts::ARCH,
                "shell":{"name":"sh","path":"/bin/sh"},"user":"agent","home":"/home/agent",
                "workspace_root":attach.workspace_root.as_deref().unwrap_or(&root),
                "path_style":"posix","case_sensitive":true,
                "executors":[{"kind":"orch-exec","version":"0.1.0","available":true,"reason":null,
                    "sandbox_modes":["read_only","workspace","open"],"tool_sets":["plain"]}],
                "mcp_found":[],"index":"full","mounts":[],"private_views":false,
                "suspend":false,"git":true,"generation":attach.data_generation.unwrap_or(generation)
            }))
            .map(CommandResult::EnvAttach)
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))
        });
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{policy,agent.max_sandbox}',$2) WHERE id=$1",
    )
    .bind(world.machine_id)
    .bind(json!("open"))
    .execute(world.pool.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET placement='connected',template_id=NULL,workspace_root=$2 WHERE id=$1")
        .bind(world.environment).bind(&context.workspace_root).execute(world.pool.pool()).await.unwrap();
    common::eventually("the proof directory is attached by its machine",||async {
        sqlx::query_scalar::<_,bool>("SELECT COALESCE(e.status='running' AND e.attached_session=COALESCE(m.session_started_at,m.last_seen_at),false) FROM environments e JOIN machines m ON m.id=e.machine_id WHERE e.id=$1")
            .bind(world.environment).fetch_one(world.pool.pool()).await.unwrap()
    }).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let machine = orch_db::queries::environment_facts::of(&mut conn, world.environment)
        .await
        .unwrap()
        .unwrap();
    let template =
        orch_db::queries::environments::thread_engine::template_spec(&mut conn, world.environment)
            .await
            .unwrap();
    assert!(template.is_none());
    assert_eq!(machine.machine_id, Some(world.machine_id));
    assert_eq!(
        machine.confinement(),
        "none",
        "this fixture makes no isolation declaration"
    );
    assert_eq!(
        orch_engine::sessions::widest_sandbox(&machine, None),
        SandboxMode::Open
    );
}

/// The same self-contained native program boundary as the existing executor Docker control.
/// The version probe needs only connector PATH; neither tools_dir nor a held build is supplied.
fn environment(
    directory: &Path,
    workspace: &Path,
    image: &str,
) -> (EnvironmentContext, Containers) {
    let directory = directory.canonicalize().unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let prefix = format!("orch-test-native-loss-{}", new_id());
    let program = directory.join("confirmed-program");
    let source = format!(
        r#"#!/bin/sh
set -e
if [ "$1" = "--version" ]; then
  exec docker run --rm --name {prefix}-version-$$ --network none {image} codex "$@"
fi
exec docker run --rm -i --init --name {prefix}-$$ --network host \
  -v {data} -v {workspace} -e CODEX_HOME {image} codex "$@"
"#,
        prefix = quote(&prefix),
        image = quote(image),
        data = quote(&format!("{}:{}", directory.display(), directory.display())),
        workspace = quote(&format!("{}:{}", workspace.display(), workspace.display()))
    );
    std::fs::write(&program, source).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let context = serde_json::from_value(json!({
        "environment_id":new_id(),"generation":1,"data_dir":directory.join("data"),
        "workspace_root":workspace,"home":null,"tools_dir":null,"tools":[],
        "local_programs":{"codex":program},"mounts":[],"user":"agent",
        "shell":{"name":"sh","path":"/bin/sh"},"os":"linux","arch":std::env::consts::ARCH,
        "path_style":"posix","limits":{},"variables":{},"mcp_servers_enabled":false
    }))
    .unwrap();
    (context, Containers(prefix))
}

fn source_container(rig: &native::Rig) -> String {
    let requests = rig.launcher.launched.lock().unwrap();
    let containers = rig.launcher.containers.lock().unwrap();
    let sources: Vec<_> = requests
        .iter()
        .zip(containers.iter())
        .filter(|(request, _)| request.args.first().is_some_and(|arg| arg == "app-server"))
        .map(|(_, name)| name.clone())
        .collect();
    assert_eq!(
        sources.len(),
        1,
        "one source app-server was actually launched"
    );
    sources[0].clone()
}

fn running(name: &str) -> bool {
    std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.Running}}", name])
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout == b"true\n")
}

/// The normal connector conversion, with the exact original stream identity retained.
async fn forward(
    world: &common::World,
    thread: Id,
    segment: Id,
    event: &AdapterEvent,
) -> Result<(), String> {
    let linked = match event {
        AdapterEvent::Session {
            thread_id,
            segment_id,
            event,
        } => {
            if *thread_id != thread || *segment_id != segment {
                return Err("the real native event has a different thread/segment".into());
            }
            let value = serde_json::to_value(event).map_err(|error| error.to_string())?;
            Event::decode(
                event.kind(),
                value.get("body").cloned().unwrap_or(Json::Null),
            )
            .map_err(|error| error.to_string())?
        }
        AdapterEvent::ModelRequest(report)
            if report.thread_id == thread && report.segment_id == segment =>
        {
            Event::ModelRequest(orch_link_contract::events::ModelRequest {
                turn_id: report.turn_id,
                request: report.request.clone(),
            })
        }
        _ => return Ok(()),
    };
    world
        .machine
        .publish(
            &Stream::Session {
                thread_id: thread,
                segment_id: segment,
            },
            &linked,
        )
        .await;
    Ok(())
}

#[derive(Debug)]
enum Terminal {
    Turn(Box<TurnCompleted>),
    SessionLost(String),
}

async fn prove(
    world: &common::World,
    native: &mut native::Rig,
    executors: &Executors,
    context: &EnvironmentContext,
    ids: (Id, Id),
    source: &str,
    destination: &str,
) -> Result<(), String> {
    let (thread, segment) = ids;
    let mut spec = native.spec(thread, SessionMode::New, None);
    spec.settings.segment_id = segment;
    spec.settings.arrangement = ArrangementType::Separated;
    spec.settings.model = common::MODEL.into();
    spec.settings.available_models = vec![common::MODEL.into()];
    spec.settings.raw_events = true;
    spec.settings.approval_mode = ApprovalMode::OnRequest;
    spec.settings.sandbox_mode = SandboxMode::Open;
    spec.exec.executor = Some("codex-exec-server".into());
    spec.exec.generation = context.generation;
    let opened = native
        .adapter
        .open_session(spec)
        .await
        .map_err(|error| error.to_string())?;
    let actual: SessionOpenResult = serde_json::from_value(serde_json::to_value(&opened).unwrap())
        .map_err(|error| error.to_string())?;
    // Scripted admission returns the real native session's receipt, not a fabricated native id.
    for kind in [CommandKind::SessionOpen, CommandKind::SessionOpenFenced] {
        let actual = actual.clone();
        world
            .machine
            .handler
            .on(kind, move |command| match command {
                Command::SessionOpen(open)
                    if open.thread_id == thread && open.segment_id == segment =>
                {
                    Ok(CommandResult::SessionOpen(actual.clone()))
                }
                Command::SessionOpenFenced(open)
                    if open.session.thread_id == thread && open.session.segment_id == segment =>
                {
                    Ok(CommandResult::SessionOpenFenced(actual.clone()))
                }
                _ => Err(CommandError::new(
                    ErrorCode::Internal,
                    "unexpected native proof session",
                )),
            });
    }
    world.open_session_for_reports(thread, segment).await;
    let stored: String = world
        .scalar(
            "SELECT native_session_ref FROM segments WHERE id=$1",
            segment,
        )
        .await;
    if stored != opened.native_session_ref {
        return Err("the Engine admission did not retain the actual native reference".into());
    }
    for event in native.drain() {
        forward(world, thread, segment, &event).await?;
    }
    eprintln!("native loss proof stage=opened mapping=exact");

    let marker = native.workspace_path().join("command-started");
    let release = native.workspace_path().join("release-command");
    let after = native.workspace_path().join("command-completed");
    // This command cannot finish naturally before the fixture deliberately releases it.
    let command = format!(
        "printf started > {}; while [ ! -e {} ]; do sleep 1; done; printf completed > {}",
        quote(&marker.display().to_string()),
        quote(&release.display().to_string()),
        quote(&after.display().to_string())
    );
    let text = native::script(
        "hold a separated command",
        &[
            &format!(
                "tool exec_command {}",
                json!({"cmd":command,"yield_time_ms":30000})
            ),
            "then",
            "say The command failed after its executor was removed: {{result}}",
        ],
    );
    // A command error may continue the same native turn. The second scripted response
    // acknowledges its actual tool result; it never emits another tool or a native terminal frame.
    let input = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, common::queued(&text))
        .await
        .map_err(|error| error.to_string())?;
    let start = world.turn_start(thread, 0).await;
    let turn: orch_agent_contract::session::TurnStart =
        serde_json::from_value(serde_json::to_value(&start).unwrap())
            .map_err(|error| error.to_string())?;
    if turn.approval_mode != ApprovalMode::OnRequest || turn.sandbox_mode != SandboxMode::Open {
        return Err("the consumer fixture did not lawfully admit On request/Open".into());
    }
    native
        .adapter
        .start_turn(turn)
        .await
        .map_err(|error| error.to_string())?;
    let mut native_turn = None;
    let mut tool_started = false;
    let mut command_ref = None;
    let mut reported_usage = None;
    let owned_command = |call: &ToolCallPayload| {
        matches!(&call.tool,
        ToolPayload::Command {command, ..} if command.contains(&marker.display().to_string())
            && command.contains(&release.display().to_string())
            && command.contains(&after.display().to_string()))
    };
    // The existing Engine fixture samples positive asynchronous predicates every20ms.
    let mut poll = tokio::time::interval(Duration::from_millis(20));
    tokio::time::timeout(native::WAIT,async {
        loop {
            tokio::select! {
                event=native.events.next()=>{
                    let event=event.ok_or("the source native event stream closed before the command")?;
                    if let AdapterEvent::Session {event,..}=&event {match event {
                        SessionEvent::TurnStarted(started) if started.turn_id==start.turn_id=>native_turn=Some(started.native_turn_ref.clone()),
                        SessionEvent::ItemStarted(item) if matches!(&item.content,ItemContent::ToolCall(_))=>{
                            let ItemContent::ToolCall(call)=&item.content else { return Err("the command projection is absent".to_owned()); };
                            if !owned_command(call) || command_ref.as_ref().is_some_and(|id| id!=&item.native_ref) {
                                return Err("the native turn started a tool other than the owned held command".to_owned());
                            }
                            command_ref=Some(item.native_ref.clone());tool_started=true;
                        },
                        SessionEvent::UsageUpdated(usage) if usage.turn_id==start.turn_id=>reported_usage=Some(usage.turn),
                        SessionEvent::TurnCompleted(_) | SessionEvent::SessionLost(_)=>return Err("native work ended before destination retirement".to_owned()),
                        _=>{}
                    }}
                    let approval = matches!(&event, AdapterEvent::Session {event:SessionEvent::ApprovalRequested(_),..});
                    forward(world,thread,segment,&event).await?;
                    if approval { return Err("a real native approval was raised before the loss oracle; it remains unanswered".to_owned()); }
                }
                _=poll.tick()=>{
                    if marker.is_file() && native_turn.is_some() && tool_started { break; }
                }
            }
        }
        Ok::<(),String>(())
    }).await.map_err(|_|"the actual separated command did not reach its owned marker within90s".to_owned())??;
    world.wait_state(thread, "running").await;
    // The original input has materialized the session and started its held command. The reader
    // also pages turn metadata, which the native server does not expose before the first input.
    native
        .adapter
        .read_session(&opened.native_session_ref, false)
        .await
        .map_err(|error| {
            format!("the actual source could not read its started native session: {error}")
        })?;
    let requests = native.model_requests().len();
    if requests != 1 || !running(source) {
        return Err(
            "one actual model request and a live source are required before retirement".into(),
        );
    }
    let native_turn = native_turn.ok_or("the real native turn identity is missing")?;
    let command_ref = command_ref.ok_or("the real held command identity is missing")?;
    eprintln!(
        "native loss proof stage=running remote_marker=true model_requests=1 input=delivered source_rpc=thread/read+thread/turns/list"
    );

    // The policy/watch removal trigger has separate lifecycle controls. This oracle uses
    // the existing stop API to observe genuine native executor loss on a live turn.
    executors
        .stop("codex-exec-server")
        .await
        .map_err(|error| error.to_string())?;
    if executors.endpoint("codex-exec-server").await.is_some() {
        return Err("the removed program still has its registered native executor".into());
    }
    if !owned_containers(destination).is_empty() {
        return Err("the destination native executor container survived its stop".into());
    }
    native
        .adapter
        .read_session(&opened.native_session_ref, false)
        .await
        .map_err(|error| {
            format!("the source could not read its exact native session after executor retirement: {error}")
        })?;
    if !running(source) {
        return Err("destination retirement also lost the source app-server".into());
    }
    eprintln!(
        "native loss proof stage=retired stop=existing_executor_api endpoint_absent=true destination_process=gone source_rpc=thread/read+thread/turns/list source_process=running"
    );
    let mut raw_terminal = false;
    let mut pending_terminal = None;
    let mut failed_command = None;
    let mut followup_result = false;
    let terminal=tokio::time::timeout(native::WAIT,async {
        loop {
            let observed = native.model_requests();
            if observed.len()>requests+1 {
                return Err("the controlled tool-result follow-up made another model request".to_owned());
            }
            if observed.len()==requests+1 {
                let response=&observed[requests];
                let results=response.body["input"].as_array().into_iter().flatten()
                    .filter(|item|item["type"]=="function_call_output").collect::<Vec<_>>();
                if response.round!=1 || results.len()!=1
                    || results[0]["call_id"].as_str()!=Some(command_ref.as_str())
                    || results[0]["output"].as_str().is_none_or(str::is_empty) {
                    return Err("the extra inference was not the single actual tool-result round".to_owned());
                }
                followup_result=true;
            }
            tokio::select! {
                event=native.events.next()=>{
                    let event=event.ok_or("the native event stream ended without a receipt")?;
                    if let AdapterEvent::Session {event,..}=&event {match event {
                        SessionEvent::RawFrame(frame) if frame.direction=="in" && frame.payload["method"]=="turn/completed"
                            && frame.payload["params"]["threadId"].as_str()==Some(opened.native_session_ref.as_str())
                            && frame.payload["params"]["turn"]["id"].as_str()==Some(native_turn.as_str())=>raw_terminal=true,
                        SessionEvent::ItemStarted(item) if matches!(&item.content,ItemContent::ToolCall(_)) && item.native_ref!=command_ref=>
                            return Err("the loss follow-up started an unexpected native tool".to_owned()),
                        SessionEvent::ItemCompleted(item) if item.native_ref==command_ref=>{
                            let call:ToolCallPayload=serde_json::from_value(item.payload.clone()).map_err(|error|error.to_string())?;
                            if !owned_command(&call) || !call.raw.is_error {
                                return Err("the held native command did not report its actual failed result".to_owned());
                            }
                            failed_command=Some(item.clone());
                        },
                        SessionEvent::UsageUpdated(usage) if usage.turn_id==start.turn_id=>reported_usage=Some(usage.turn),
                        SessionEvent::TurnCompleted(ended) if ended.turn_id==start.turn_id=>pending_terminal=Some(Terminal::Turn(Box::new(ended.clone()))),
                        SessionEvent::SessionLost(lost)=>pending_terminal=Some(Terminal::SessionLost(lost.reason.clone())),
                        _=>{}
                    }}
                    forward(world,thread,segment,&event).await?;
                }
                _=poll.tick()=>{}
            }
            if failed_command.is_some() && followup_result
                && (matches!(&pending_terminal,Some(Terminal::SessionLost(_))) || raw_terminal)
                && let Some(terminal)=pending_terminal.take() { return Ok(terminal); }
        }
    }).await.map_err(|_|format!("no real terminal receipt within90s after executor retirement; model_requests={} raw_terminal={raw_terminal}",native.model_requests().len()))??;
    let actual_requests = native.model_requests();
    if actual_requests.len() != requests + 1
        || actual_requests[0].round != 0
        || actual_requests[1].round != 1
        || actual_requests
            .iter()
            .any(|request| request.status != Some(200))
        || after.exists()
        || release.exists()
        || !running(source)
    {
        return Err(
            "the held command or model/source boundaries changed during observation".into(),
        );
    }
    // This is an actual RPC, unlike CodexAdapter::probe, which reads its cached descriptor.
    native
        .adapter
        .read_session(&opened.native_session_ref, false)
        .await
        .map_err(|error| error.to_string())?;
    for event in native.drain() {
        if let AdapterEvent::Session {
            event: SessionEvent::UsageUpdated(usage),
            ..
        } = &event
            && usage.turn_id == start.turn_id
        {
            reported_usage = Some(usage.turn);
        }
        forward(world, thread, segment, &event).await?;
    }
    // Match the existing Engine normalization, not byte equality of its stored error JSON.
    // Recovery metadata may be added independently; only the three normalized fields are projected.
    let projected = |error: &orch_agent_contract::events::TurnError| json!({"code":error.code,"message":error.message,"retryable":error.retryable});
    let (expected, error) = match &terminal {
        Terminal::Turn(ended) => {
            if !raw_terminal {
                return Err(
                    "neutral completion has no matching native turn/completed frame".into(),
                );
            }
            match ended.status {
                TurnOutcome::Completed => ("completed", None),
                TurnOutcome::Interrupted => ("interrupted", ended.error.as_ref().map(projected)),
                TurnOutcome::Failed => (
                    "failed",
                    Some(ended.error.as_ref().map(projected).unwrap_or_else(|| {
                        json!({"code":"harness_error","message":"the turn failed","retryable":false})
                    })),
                ),
            }
        }
        Terminal::SessionLost(reason) => (
            "failed",
            Some(json!({"code":"session_lost",
                "message":format!("The harness lost the session: {reason}"),"retryable":true})),
        ),
    };
    world.wait_turn(start.turn_id, expected).await;
    let failed_command = failed_command.ok_or("no correlated failed command was retained")?;
    let usage = reported_usage.ok_or("the real native usage report is missing")?;
    if usage.total_tokens <= 0 {
        return Err("the actual follow-up use was not reported".into());
    }
    let command_status =
        serde_json::to_value(failed_command.status).map_err(|error| error.to_string())?;
    let command_status = command_status
        .as_str()
        .ok_or("the command status is absent")?
        .to_owned();
    // The stream accepts publication before asynchronous application. The ordinary terminal wait
    // precedes this positive convergence check, which also covers the usage drained after the terminal.
    common::eventually("the native failed command and measured use are durably applied",||async {
        let Ok(mut conn)=world.pool.acquire().await else {return false};
        let Ok(Some((status,payload)))=sqlx::query_as::<_,(String,Json)>(
            "SELECT status,payload FROM items WHERE thread_id=$1 AND turn_id=$2 AND native_ref=$3")
            .bind(thread).bind(start.turn_id).bind(&command_ref).fetch_optional(&mut *conn).await else {return false};
        let Ok(call)=serde_json::from_value::<ToolCallPayload>(payload) else {return false};
        if status!=command_status || !owned_command(&call) || !call.raw.is_error {return false;}
        let Ok((stored,billed))=sqlx::query_as::<_,(Option<Json>,Option<Id>)>(
            "SELECT usage,audit_for_user_id FROM turns WHERE id=$1")
            .bind(start.turn_id).fetch_one(&mut *conn).await else {return false};
        let Ok(counted)=orch_db::usage::ledger::counted_request_totals(&mut conn,start.turn_id).await else {return false};
        drop(conn);
        let reported=orch_engine::providers::tokens_of(&serde_json::to_value(usage).unwrap());
        let expected=if counted.total>reported.total {orch_engine::providers::larger(&reported,&counted)} else {reported};
        let Ok(expected)=serde_json::from_value::<orch_agent_contract::events::TokenUsage>(orch_engine::providers::usage_json(&expected)) else {return false};
        let Some(stored)=stored else {return false};
        let Ok(retained)=serde_json::from_value::<orch_agent_contract::events::TokenUsage>(stored) else {return false};
        billed==Some(world.ada) && retained==expected
            && world.ledger_of(start.turn_id).await.iter().all(|row|!row.counted||row.user_id==Some(world.ada))
    }).await;
    let (state, retained_error, confirmed): (String, Option<Json>, bool) =
        sqlx::query_as("SELECT state,error,native_end_confirmed FROM turns WHERE id=$1")
            .bind(start.turn_id)
            .fetch_one(world.pool.pool())
            .await
            .map_err(|error| error.to_string())?;
    let (input_state, author, turn): (String, Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT state,author_id,turn_id FROM thread_inputs WHERE id=$1")
            .bind(input.input.id)
            .fetch_one(world.pool.pool())
            .await
            .map_err(|error| error.to_string())?;
    if state != expected
        || input_state != "delivered"
        || author != Some(world.ada)
        || turn != Some(start.turn_id)
        || world.turn_starts(thread).len() != 1
        || world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                thread,
            )
            .await
            != 1
    {
        return Err(
            "Engine did not retain the original delivered input and exactly one real turn".into(),
        );
    }
    if matches!(terminal, Terminal::Turn(_)) && !confirmed {
        return Err("the Engine did not keep the native completion proof".into());
    }
    match error {
        None if retained_error.is_some() => {
            return Err(
                "Engine retained an error where this terminal outcome normalizes to none".into(),
            );
        }
        Some(expected)
            if !retained_error.as_ref().is_some_and(|error| {
                ["code", "message", "retryable"]
                    .iter()
                    .all(|key| error.get(*key) == expected.get(*key))
            }) =>
        {
            return Err(
                "Engine changed the normalized native code, message or retryability".into(),
            );
        }
        _ => {}
    }
    eprintln!(
        "native loss proof stage=terminal receipt={terminal:?} engine_state={state} original_input=delivered starts=1 model_requests={} failed_command=true usage_tokens={}",
        actual_requests.len(),
        usage.total_tokens
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retiring_a_confirmed_program_ends_its_actual_separated_turn_without_losing_the_input() {
    let mut native = native::Rig::start_with(
        "executor-turn-loss",
        native::DockerLauncher::without_workspace,
    )
    .await
    .expect("the pinned native image is required; a skipped setup is not executor proof");
    let outer = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let world = common::World::observed().await;
    let (mut context, containers) =
        environment(outer.path(), native.workspace_path(), &native::image());
    context.environment_id = world.environment;
    let generation: i32 = world
        .scalar(
            "SELECT generation FROM environments WHERE id=$1",
            world.environment,
        )
        .await;
    context.generation = i64::from(generation);
    assert!(context.tools_dir.is_none() && context.tools.is_empty());
    connected_consumer(&world, &context).await;
    let executors = Executors::new();
    executors
        .register(Box::new(CodexExecServer::with_config(CodexConfig {
            ports: Some(41000..=41999),
            ..CodexConfig::default()
        })))
        .await
        .unwrap();
    executors
        .start("codex-exec-server", None, &context)
        .await
        .unwrap();
    let endpoint = executors.endpoint("codex-exec-server").await.unwrap();
    assert_eq!(
        endpoint.version,
        expected_build(),
        "the actual endpoint is the deployment-pinned build"
    );
    assert_eq!(owned_containers(&containers.0).len(), 1);
    eprintln!(
        "native loss proof stage=program-started tool_cache=false native_build={}",
        endpoint.version
    );
    native.token = endpoint.token.as_ref().unwrap().expose().to_owned();
    native.mock.set_credential(Some(&native.token));
    let address: SocketAddr = endpoint
        .address
        .strip_prefix("ws://")
        .unwrap()
        .trim_end_matches('/')
        .parse()
        .unwrap();
    let proxy = native::remote::SplitProxy::start(native.mock.addr(), address).await;
    native.loopback_port = Some(proxy.port());
    native.agent_id = world.agent;
    native.provider_id = world.provider;
    native.environment_id = world.environment;
    let started = native.boot().await;
    assert_eq!(
        started.harness_version, endpoint.version,
        "both native processes use the same pinned build"
    );
    let source = source_container(&native);
    assert_eq!(proxy.exec_connections(), 0);
    let thread = world.thread("Executor removed during a native tool").await;
    world
        .set(thread, "approval.mode", json!("on_request"))
        .await;
    world.set(thread, "sandbox.mode", json!("open")).await;
    let segment = world.segment_of(thread).await;
    let result = prove(
        &world,
        &mut native,
        &executors,
        &context,
        (thread, segment),
        &source,
        &containers.0,
    )
    .await;
    // Even the expected failing predecessor closes owned processes and DB before the final assertion.
    executors.stop_all().await;
    native.adapter.stop().await.unwrap();
    world.shutdown().await;
    result.unwrap();
}
