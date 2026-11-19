//! Native control acknowledgements and actual task outcomes have distinct receipts.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native protocol fixture assertions"
)]
mod common;
use futures::{StreamExt as _, poll};
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::background::BackgroundEndState;
use orch_agent_contract::events::SessionEvent;
use serde_json::json;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

// The complete program is written before it starts. Only its separate release receipt changes later.
const PROGRAM: &str = r#"#!/usr/bin/env python3
import json, sys, time
from pathlib import Path
if '--version' in sys.argv:
    print('2.1.312 (Claude Code)')
    raise SystemExit(0)
session = next((sys.argv[i+1] for i,arg in enumerate(sys.argv[:-1]) if arg in ('--session-id','--resume')), 'session')
with open(__file__ + '.starts', 'a') as receipt:
    receipt.write('started\n')
def emit(value):
    print(json.dumps(value), flush=True)
def task(subtype, **fields):
    emit(dict(type='system',subtype=subtype,session_id=session,task_id='task',run_id='run-1',tool_use_id='bg',**fields))
for line in sys.stdin:
    frame=json.loads(line)
    if frame.get('type')=='control_request':
        request=frame['request']
        emit(dict(type='control_response',response=dict(subtype='success',request_id=frame['request_id'],response={})))
        if request.get('subtype')=='stop_task':
            task('task_progress',summary='ack-only')
            while not Path(__RELEASE__).exists():
                time.sleep(0.01)
            task('task_notification',status=__OUTCOME__,summary='fixture terminal')
    elif frame.get('type')=='user':
        if 'foreground-only' not in json.dumps(frame):
            emit(dict(type='assistant',message=dict(id='message',content=[dict(type='tool_use',id='bg',name='Bash',input=dict(command='fixture command',run_in_background=True))])))
            emit(dict(type='user',message=dict(role='user',content=[dict(type='tool_result',tool_use_id='bg',content='background task launched')]),tool_use_result=dict(backgroundTaskId='task')))
            task('task_started',task_type='local_bash',is_backgrounded=True,description='Fixture task')
        emit(dict(type='result',subtype='success',is_error=False,num_turns=1,result='Waiting.',usage=dict(input_tokens=1,output_tokens=1)))
with open(__file__ + '.stops', 'a') as receipt:
    receipt.write('stopped\n')
"#;

fn stand_in(directory: &Path, name: &str, release: &Path, outcome: &str) -> PathBuf {
    let program = directory.join(name);
    let body = PROGRAM
        .replace(
            "__RELEASE__",
            &serde_json::to_string(&release.display().to_string()).unwrap(),
        )
        .replace("__OUTCOME__", &serde_json::to_string(outcome).unwrap());
    std::fs::write(&program, body).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

async fn acknowledged_then(outcome: &str, expected: BackgroundEndState) {
    let files = tempfile::tempdir().unwrap();
    let release = files.path().join("release");
    let program = stand_in(files.path(), "native-control", &release, outcome);
    let mut rig = common::Rig::standing_in(&program).await;
    rig.start(json!({}), Default::default()).await;
    rig.open().await;
    rig.run("Start the fixture task").await;
    let thread = rig.thread;
    let roster = rig.adapter.background_commands(thread).await.unwrap();
    let id = roster
        .items
        .first()
        .expect("the observed task is listed")
        .id
        .clone();
    let result = {
        let pending = rig.adapter.end_background_command(thread, &id);
        tokio::pin!(pending);
        tokio::time::timeout(Duration::from_secs(20),async {
            loop {
                tokio::select! {
                    result=&mut pending => panic!("a control acknowledgement is not a terminal receipt: {result:?}"),
                    event=rig.events.next()=>{
                        let event=event.expect("the native event stream remains open");
                        if matches!(event,AdapterEvent::Session {event:SessionEvent::ItemUpdated(ref update),..}
                            if update.payload["title"]=="ack-only") {break;}
                    }
                }
            }
        }).await.expect("native progress after the acknowledged request");
        assert!(
            poll!(&mut pending).is_pending(),
            "an acknowledgement cannot synthesize stopped"
        );
        tokio::fs::write(&release, b"finish").await.unwrap();
        tokio::time::timeout(Duration::from_secs(20), pending)
            .await
            .expect("the actual native terminal event")
            .unwrap()
    };
    assert_eq!(result.state, expected);
    assert!(
        rig.model.requests().is_empty(),
        "native End sends no model input"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_control_ack_waits_for_the_exact_native_stopped_event() {
    acknowledged_then("stopped", BackgroundEndState::Ended).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn natural_completion_after_a_control_ack_remains_already_ended() {
    acknowledged_then("completed", BackgroundEndState::AlreadyEnded).await;
}

async fn unloadable(program: &Path) -> common::Rig {
    let mut rig = common::Rig::standing_in(program).await;
    rig.start(json!({"idle_exit_secs":1}), Default::default())
        .await;
    let mut spec = rig.spec(orch_agent_contract::session::SessionMode::New, None);
    spec.exec.executor = Some("test-executor".into());
    spec.exec.tool_set = Some("custom".into());
    rig.open_with(spec).await;
    rig
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_task_prevents_executor_mode_idle_unload_until_its_actual_stop() {
    let files = tempfile::tempdir().unwrap();
    let release = files.path().join("release");
    let busy_program = stand_in(files.path(), "busy", &release, "stopped");
    let idle_program = stand_in(files.path(), "idle", &release, "stopped");
    let mut busy = unloadable(&busy_program).await;
    busy.run("Start the fixture task").await;
    let roster = busy.adapter.background_commands(busy.thread).await.unwrap();
    let id = roster
        .items
        .first()
        .expect("the owned running task is observed")
        .id
        .clone();
    let mut idle = unloadable(&idle_program).await;
    idle.run("foreground-only").await;
    let idle_stopped = files.path().join("idle.stops");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !idle_stopped.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the identical idle control actually unloads");
    busy.run("foreground-only").await;
    let starts = std::fs::read_to_string(files.path().join("busy.starts")).unwrap();
    assert_eq!(
        starts.lines().count(),
        1,
        "the task pins the same process after the unload deadline"
    );
    assert!(!files.path().join("busy.stops").exists());
    tokio::fs::write(&release, b"finish").await.unwrap();
    let ended = busy
        .adapter
        .end_background_command(busy.thread, &id)
        .await
        .unwrap();
    assert_eq!(ended.state, BackgroundEndState::Ended);
    let busy_stopped = files.path().join("busy.stops");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !busy_stopped.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the exact stopped fact releases idle unload");
    busy.adapter.stop().await.unwrap();
    idle.adapter.stop().await.unwrap();
}
