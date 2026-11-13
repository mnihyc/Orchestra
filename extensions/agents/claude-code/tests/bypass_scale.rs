//! A large idle world and a million residual permissions must not accumulate approval cards or history.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

mod common;

use std::ffi::OsStr;
use std::future::Future;
use std::num::NonZeroU64;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use common::Rig;
use futures::StreamExt;
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_agent_contract::session::SessionMode;

// The default leaves CI headroom; slower runners can raise it without reducing the load.
const DEFAULT_SCALE_TIMEOUT_SECS: u64 = 15 * 60;
const SCALE_TIMEOUT_ENV: &str = "ORCH_CLAUDE_BYPASS_SCALE_TIMEOUT_SECS";

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

fn scale_timeout(configured: Option<&OsStr>) -> Duration {
    let seconds = configured.map_or(DEFAULT_SCALE_TIMEOUT_SECS, |value| {
        value
            .to_str()
            .and_then(|value| value.parse::<NonZeroU64>().ok())
            .unwrap_or_else(|| {
                panic!("{SCALE_TIMEOUT_ENV} must be a positive integer number of seconds")
            })
            .get()
    });
    Duration::from_secs(seconds)
}

#[test]
fn the_scale_timeout_is_a_default_not_a_ceiling() {
    assert_eq!(
        scale_timeout(None),
        Duration::from_secs(DEFAULT_SCALE_TIMEOUT_SECS)
    );
    for seconds in [
        DEFAULT_SCALE_TIMEOUT_SECS / 2,
        DEFAULT_SCALE_TIMEOUT_SECS * 2,
    ] {
        assert_eq!(
            scale_timeout(Some(OsStr::new(&seconds.to_string()))),
            Duration::from_secs(seconds)
        );
    }
}

async fn within_deadline<F: Future>(
    deadline: tokio::time::Instant,
    phase: &str,
    work: F,
) -> F::Output {
    tokio::time::timeout_at(deadline, work)
        .await
        .unwrap_or_else(|_| panic!("the scale deadline elapsed while {phase}; budget is configurable with {SCALE_TIMEOUT_ENV}"))
}

async fn turn_events(rig: &mut Rig) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    while let Some(event) = rig.events.next().await {
        let AdapterEvent::Session { event, .. } = event else {
            continue;
        };
        assert!(
            !matches!(event, SessionEvent::ApprovalRequested(_)),
            "a residual bypass permission must not wait for an approval card"
        );
        let completed = matches!(event, SessionEvent::TurnCompleted(_));
        events.push(event);
        // Preserve the existing history bound while failing before the fixture can retain a leak.
        assert!(
            events.len() < 32,
            "a permission must not become retained history; received {} session events",
            events.len()
        );
        if completed {
            return events;
        }
    }
    panic!("the adapter event stream closed before the scale turn completed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scale_a_thousand_idle_threads_and_a_million_bypass_requests_keep_one_pending_frame() {
    let began = tokio::time::Instant::now();
    let configured_timeout = std::env::var_os(SCALE_TIMEOUT_ENV);
    let timeout = scale_timeout(configured_timeout.as_deref());
    // One absolute deadline covers fixture startup, every opening, the turn and teardown.
    let deadline = began
        .checked_add(timeout)
        .unwrap_or_else(|| panic!("{SCALE_TIMEOUT_ENV} exceeds the platform clock range"));
    eprintln!("scale: time budget is {timeout:?}; override with {SCALE_TIMEOUT_ENV}");
    let temporary = tempfile::tempdir().unwrap();
    let program = temporary.path().join("harness");
    let source = HARNESS.replace("VERSION", orch_adapter_claude_code::config::SHIPPED_BUILD);
    std::fs::write(&program, source).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut rig =
        within_deadline(deadline, "creating the fixture", Rig::standing_in(&program)).await;
    within_deadline(deadline, "starting the adapter", rig.start_quick()).await;
    let before_world = resident_kib();
    for index in 0..1_000 {
        let mut spec = rig.spec(SessionMode::New, None);
        spec.settings.approval_mode = ApprovalMode::Bypass;
        if index > 0 {
            spec.settings.thread_id = orch_types::new_id();
            spec.settings.segment_id = orch_types::new_id();
        }
        // Opening reports SessionOpened and status through the bounded host queue.
        // Consume them per opening; awaiting 1,000 producers before their reader deadlocks.
        within_deadline(
            deadline,
            &format!("opening idle session {}", index + 1),
            rig.open_with(spec),
        )
        .await;
    }
    eprintln!("scale: 1,000 idle sessions opened in {:?}", began.elapsed());
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
    within_deadline(
        deadline,
        "starting the scale turn",
        rig.adapter.start_turn(turn),
    )
    .await
    .unwrap();
    let events = within_deadline(
        deadline,
        "answering the million bypass requests",
        turn_events(&mut rig),
    )
    .await;
    eprintln!(
        "scale: 1,000,000 bypass requests completed in {:?}",
        began.elapsed()
    );
    done.store(true, Ordering::Relaxed);
    within_deadline(deadline, "joining the memory sampler", sampler)
        .await
        .unwrap();
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
    within_deadline(deadline, "stopping the adapter", rig.adapter.stop())
        .await
        .unwrap();
}
