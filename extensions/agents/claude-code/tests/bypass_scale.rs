//! A large idle world and a million residual permissions must not accumulate approval cards or history.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use common::Rig;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_agent_contract::session::SessionMode;

// The stand-in executes no tool. Each permission must be answered before it sends the next one.
// Only the final counters are written, so the fixture cannot hide a leak by retaining a million frames.
const HARNESS: &str = r#"#!/usr/bin/env python3
import json, sys
if '--version' in sys.argv:
    print('VERSION (Claude Code)')
    sys.exit(0)
answered = 0
pending = 0
peak_pending = 0
def send(frame):
    print(json.dumps(frame, separators=(',', ':')), flush=True)
def ask():
    global pending, peak_pending
    pending += 1
    peak_pending = max(peak_pending, pending)
    send({'type':'control_request','request_id':str(answered),'request':{'subtype':'can_use_tool','tool_name':'Bash','input':{'command':'rm -rf "$danger"'},'tool_use_id':'tool-'+str(answered)}})
for line in sys.stdin:
    frame = json.loads(line)
    if frame.get('type') == 'control_request':
        send({'type':'control_response','response':{'subtype':'success','request_id':frame['request_id'],'response':{}}})
    elif frame.get('type') == 'user':
        ask()
    elif frame.get('type') == 'control_response':
        response = frame['response']
        assert response['request_id'] == str(answered), response
        assert response['response'] == {'behavior':'allow','updatedInput':{'command':'rm -rf "$danger"'}}, response
        pending -= 1
        answered += 1
        if answered < 1000000:
            ask()
        else:
            with open(sys.argv[0] + '.counts', 'w', encoding='utf-8') as out:
                json.dump({'answered':answered,'pending':pending,'peak_pending':peak_pending}, out)
            send({'type':'result','subtype':'success','is_error':False,'terminal_reason':'completed','result':'done','usage':{'input_tokens':10000000000,'output_tokens':1}})
"#;

fn resident_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:").map(str::trim))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scale_a_thousand_idle_threads_and_a_million_bypass_requests_keep_one_pending_frame() {
    let temporary = tempfile::tempdir().unwrap();
    let program = temporary.path().join("harness");
    let source = HARNESS.replace(
        "VERSION",
        orch_adapter_claude_code::config::SUPPORTED_RANGE.trim_start_matches('='),
    );
    std::fs::write(&program, source).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut rig = Rig::standing_in(&program).await;
    rig.start_quick().await;
    let before_world = resident_kib();
    for index in 0..1_000 {
        let mut spec = rig.spec(SessionMode::New, None);
        spec.settings.approval_mode = ApprovalMode::Bypass;
        if index > 0 {
            spec.settings.thread_id = orch_types::new_id();
            spec.settings.segment_id = orch_types::new_id();
        }
        rig.adapter.open_session(spec).await.unwrap();
    }
    let baseline = resident_kib();
    // At most 128 KiB per idle session, including the actor and its mapper/configuration.
    assert!(baseline.saturating_sub(before_world) < 128 * 1024);
    let peak = Arc::new(AtomicU64::new(baseline));
    let done = Arc::new(AtomicBool::new(false));
    let sampler = {
        let peak = Arc::clone(&peak);
        let done = Arc::clone(&done);
        tokio::spawn(async move {
            while !done.load(Ordering::Relaxed) {
                peak.fetch_max(resident_kib(), Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
    };
    let mut turn = rig.turn("SCALE");
    turn.approval_mode = ApprovalMode::Bypass;
    rig.adapter.start_turn(turn).await.unwrap();
    // This dedicated scale case has its own ten-minute deadline; existing functional deadlines stand.
    let events = rig
        .until(|event| matches!(event, SessionEvent::TurnCompleted(_)), 600)
        .await;
    done.store(true, Ordering::Relaxed);
    sampler.await.unwrap();
    let peak_growth = peak.load(Ordering::Relaxed).saturating_sub(baseline);
    assert!(
        peak_growth < 32 * 1024,
        "residual asks grew resident memory by {peak_growth} KiB"
    );
    assert_eq!(common::outcome(&events), TurnOutcome::Completed);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
    );
    assert!(
        events.len() < 32,
        "a permission must not become retained history"
    );
    let counts: serde_json::Value =
        serde_json::from_slice(&std::fs::read(program.with_extension("counts")).unwrap()).unwrap();
    assert_eq!(
        counts,
        serde_json::json!({"answered":1_000_000,"pending":0,"peak_pending":1})
    );
    let usage = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::UsageUpdated(usage) => Some(usage),
            _ => None,
        })
        .unwrap();
    assert_eq!(usage.turn.input_tokens, 10_000_000_000);
    assert_eq!(usage.turn.total_tokens, 10_000_000_001);
    rig.adapter.stop().await.unwrap();
}
