//! An actual earlier command and CLI process survive the next ordinary input.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native retention assertions"
)]
mod common;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::background::BackgroundEndState;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{
    ItemContent, ItemStatus, SessionEvent, ToolCallPayload, ToolPayload,
};
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn identity(stat: &str) -> (&str, &str) {
    let (pid, _) = stat.split_once(' ').expect("a native PID");
    let (_, fields) = stat.rsplit_once(')').expect("a native comm field");
    let start = fields
        .split_whitespace()
        .nth(19)
        .expect("native start time");
    (pid, start)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_inputs_preserve_the_actual_earlier_command_and_one_native_process() {
    let mut rig = common::Rig::new()
        .await
        .expect("the pinned native image is required");
    rig.start_quick().await;
    rig.open().await;
    let pid = rig.work.path().join("retained.pid");
    let before = rig.work.path().join("retained.stat");
    let release = rig.work.path().join("release-retained");
    let command = format!(
        "pid=${{BASHPID:-$$}}; printf '%s\\n' \"$pid\" > {}; cat \"/proc/$pid/stat\" > {}; while [ ! -e {} ]; do sleep 0.1; done",
        quote(&pid),
        quote(&before),
        quote(&release)
    );
    let script = format!(
        "#!mock\ntool Bash {}\nthen\nsay Waiting for the retained command.\n",
        json!({"command":command,"description":"Retained command","run_in_background":true})
    );
    let mut first = rig.turn(&script);
    first.approval_mode = ApprovalMode::Bypass;
    let origin = first.turn_id;
    rig.adapter.start_turn(first).await.unwrap();
    rig.until_the_turn_ends().await;
    let old = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(value) = tokio::fs::read_to_string(&before).await {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the actual background command writes its positive receipt");
    let check = format!("pid=$(cat {}); cat \"/proc/$pid/stat\"", quote(&pid));
    let mut second = rig.turn(&format!(
        "#!mock\ntool Bash {}\nthen\nsay Checked the earlier command.\n",
        json!({"command":check})
    ));
    second.approval_mode = ApprovalMode::Bypass;
    rig.adapter.start_turn(second).await.unwrap();
    let events = rig.until_the_turn_ends().await;
    let started: HashSet<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(started)
                if matches!(&started.content, ItemContent::ToolCall(_)) =>
            {
                Some(started.native_ref.as_str())
            }
            _ => None,
        })
        .collect();
    let (completed, payload) = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(done) if started.contains(done.native_ref.as_str()) => {
                let payload: ToolCallPayload = serde_json::from_value(done.payload.clone())
                    .expect("a completed native tool has its typed payload");
                if matches!(&payload.tool, ToolPayload::Command { command, .. } if command == &check) {
                    Some((done, payload))
                } else {
                    None
                }
            }
            _ => None,
        })
        .expect("the real next-turn shell starts and completes the exact process check");
    assert_eq!(completed.status, ItemStatus::Completed);
    let ToolPayload::Command {
        command,
        exit_code,
        output,
        ..
    } = payload.tool
    else {
        panic!("the exact native process check is a command");
    };
    assert_eq!(command, check);
    assert_eq!(exit_code, Some(0), "the native process check succeeds");
    let actual = output.inline;
    assert_eq!(
        identity(actual.trim()),
        identity(old.trim()),
        "PID and start time identify the same surviving process"
    );
    let launches = rig
        .launcher
        .launched
        .lock()
        .unwrap()
        .iter()
        .filter(|launch| launch.args.iter().any(|arg| arg == "--input-format"))
        .count();
    assert_eq!(
        launches, 1,
        "live work pins the actual CLI process across inputs"
    );
    let roster = rig.adapter.background_commands(rig.thread).await.unwrap();
    let task = roster
        .items
        .iter()
        .find(|task| task.turn_id == Some(origin))
        .expect("the original task remains owned");
    let ended = rig
        .adapter
        .end_background_command(rig.thread, &task.id)
        .await
        .unwrap();
    assert_eq!(ended.state, BackgroundEndState::Ended);
    rig.adapter.stop().await.unwrap();
}
