//! Bypass answers residual tool permissions while questions and plans remain with the person.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::Rig;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_agent_contract::events::{HarnessRequestKind, SessionEvent, TurnOutcome};
use orch_agent_contract::session::{ApprovalDecision, SessionMode};
use orch_types::Json;
use serde_json::json;

// This stand-in only exchanges protocol frames. It never executes the requested command.
const HARNESS: &str = r#"#!/usr/bin/env python3
import json, os, sys
if '--version' in sys.argv:
    print('2.1.296 (Claude Code)')
    sys.exit(0)
with open(sys.argv[0] + '.launches', 'a', encoding='utf-8') as out:
    out.write(os.environ.get('CLAUDE_CODE_EVEN_MORE_DANGEROUSLY_SKIP_PERMISSIONS', '-') + '\n')
def send(frame):
    print(json.dumps(frame, separators=(',', ':')), flush=True)
revise, revised, pending_finish = False, False, False
for line in sys.stdin:
    frame = json.loads(line)
    if frame.get('type') == 'control_request':
        with open(sys.argv[0] + '.controls', 'a', encoding='utf-8') as out:
            out.write(json.dumps(frame) + '\n')
        send({'type':'control_response','response':{'subtype':'success','request_id':frame['request_id'],'response':{}}})
        if pending_finish and frame['request'].get('subtype') == 'set_permission_mode':
            pending_finish = False
            send({'type':'result','subtype':'success','is_error':False,'terminal_reason':'completed','result':'done','usage':{'input_tokens':1,'output_tokens':1}})
    elif frame.get('type') == 'user':
        text = json.dumps(frame)
        revise = 'PLAN-REVISION' in text
        if 'STALE' in text:
            send({'type':'result','subtype':'success','is_error':False,'terminal_reason':'completed','result':'done','usage':{'input_tokens':1,'output_tokens':1}})
        if 'QUESTION' in text:
            tool, value = 'AskUserQuestion', {'questions':[{'question':'Which?','header':'Choice','options':[{'label':'A','description':'First'}]}]}
        elif 'PLAN' in text:
            tool, value = 'ExitPlanMode', {'plan':'Read, then change'}
        elif 'READ' in text:
            tool, value = 'Read', {'file_path':'/workspace/readme'}
        else:
            tool, value = 'Bash', {'command':'rm -rf "$danger"'}
        send({'type':'control_request','request_id':'ask-1','request':{'subtype':'can_use_tool','tool_name':tool,'input':value,'tool_use_id':'tool-1'}})
    elif frame.get('type') == 'control_response':
        with open(sys.argv[0] + '.answers', 'a', encoding='utf-8') as out:
            out.write(json.dumps(frame) + '\n')
        if revise and not revised and frame['response']['response'].get('behavior') == 'deny':
            revised = True
            send({'type':'control_request','request_id':'ask-2','request':{'subtype':'can_use_tool','tool_name':'ExitPlanMode','input':{'plan':'Read, explain rollback, then change'},'tool_use_id':'tool-2'}})
        else:
            if revise:
                pending_finish = True
                send({'type':'user','message':{'role':'user','content':[{'type':'tool_result','tool_use_id':'tool-2','content':'Plan approved'}]}})
            else:
                send({'type':'result','subtype':'success','is_error':False,'terminal_reason':'completed','result':'done','usage':{'input_tokens':1,'output_tokens':1}})
"#;

fn stand_in() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("harness");
    std::fs::write(&program, HARNESS).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    (directory, program)
}

fn records(program: &Path, suffix: &str) -> Vec<Json> {
    std::fs::read_to_string(format!("{}{suffix}", program.display()))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn rig(program: &Path) -> Rig {
    let mut rig = Rig::standing_in(program).await;
    rig.start_quick().await;
    let mut spec = rig.spec(SessionMode::New, None);
    spec.settings.approval_mode = ApprovalMode::Bypass;
    rig.open_with(spec).await;
    rig
}

#[tokio::test]
async fn bypass_allows_a_residual_dangerous_delete_without_a_card() {
    let (_directory, program) = stand_in();
    let mut rig = rig(&program).await;
    let mut turn = rig.turn("DELETE");
    turn.approval_mode = ApprovalMode::Bypass;
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_the_turn_ends().await;
    assert_eq!(common::outcome(&events), TurnOutcome::Completed);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
    );
    assert_eq!(
        records(&program, ".answers")[0]["response"]["response"],
        json!({
            "behavior":"allow", "updatedInput":{"command":"rm -rf \"$danger\""}
        })
    );
    assert_eq!(
        std::fs::read_to_string(format!("{}.launches", program.display())).unwrap(),
        "1\n"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn bypass_does_not_grant_an_action_during_a_plan() {
    let (_directory, program) = stand_in();
    let mut rig = rig(&program).await;
    let mut turn = rig.turn("DELETE");
    turn.approval_mode = ApprovalMode::Bypass;
    turn.plan = true;
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_the_turn_ends().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
    );
    assert_eq!(
        records(&program, ".answers")[0]["response"]["response"],
        json!({
            "behavior":"deny", "message":"Plan mode cannot approve tool actions."
        })
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn bypass_allows_reading_during_a_plan_without_a_card() {
    let (_directory, program) = stand_in();
    let mut rig = rig(&program).await;
    let mut turn = rig.turn("READ");
    turn.approval_mode = ApprovalMode::Bypass;
    turn.plan = true;
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_the_turn_ends().await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
    );
    assert_eq!(
        records(&program, ".answers")[0]["response"]["response"],
        json!({
            "behavior":"allow", "updatedInput":{"file_path":"/workspace/readme"}
        })
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn bypass_refuses_a_permission_request_after_the_turn_ends_without_a_card() {
    let (_directory, program) = stand_in();
    let mut rig = rig(&program).await;
    let mut turn = rig.turn("STALE");
    turn.approval_mode = ApprovalMode::Bypass;
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until_the_turn_ends().await;
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while records(&program, ".answers").is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the stale request receives an answer");
    let events = rig.drain(50).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
    );
    assert_eq!(
        records(&program, ".answers")[0]["response"]["response"],
        json!({
            "behavior":"deny", "message":"no turn is running to ask a person in"
        })
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn bypass_still_waits_for_a_question_and_a_proposed_plan() {
    for (input, kind) in [
        ("QUESTION", HarnessRequestKind::UserInput),
        ("PLAN", HarnessRequestKind::Plan),
    ] {
        let (_directory, program) = stand_in();
        let mut rig = rig(&program).await;
        let mut turn = rig.turn(input);
        turn.approval_mode = ApprovalMode::Bypass;
        turn.plan = input == "PLAN";
        rig.adapter.start_turn(turn).await.unwrap();
        let events = rig
            .until(
                |event| matches!(event, SessionEvent::ApprovalRequested(_)),
                20,
            )
            .await;
        let Some(SessionEvent::ApprovalRequested(request)) = events.last() else {
            panic!("a request for the person")
        };
        assert_eq!(request.request.request_kind, kind);
        assert!(records(&program, ".answers").is_empty());
        let decision = if input == "QUESTION" {
            ApprovalDecision::Answers {
                answers: BTreeMap::from([("q1".into(), vec!["A".into()])]),
                note: None,
            }
        } else {
            ApprovalDecision::Option {
                option_id: "keep_planning".into(),
                note: Some("Revise it".into()),
            }
        };
        rig.adapter
            .resolve_approval(rig.thread, &request.request_ref, decision)
            .await
            .unwrap();
        let ended = rig.until_the_turn_ends().await;
        assert_eq!(common::outcome(&ended), TurnOutcome::Completed);
        let answer = &records(&program, ".answers")[0]["response"]["response"];
        if input == "QUESTION" {
            assert_eq!(answer["updatedInput"]["answers"]["Which?"], "A");
        } else {
            assert_eq!(answer, &json!({"behavior":"deny", "message":"Revise it"}));
        }
        rig.adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn a_plan_can_be_revised_then_proceeded_with_under_the_composers_mode() {
    for (mode, restored) in [
        (ApprovalMode::Ask, "default"),
        (ApprovalMode::OnRequest, "acceptEdits"),
        (ApprovalMode::Bypass, "bypassPermissions"),
    ] {
        let (_directory, program) = stand_in();
        let mut rig = Rig::standing_in(&program).await;
        rig.start_quick().await;
        let mut spec = rig.spec(SessionMode::New, None);
        spec.settings.approval_mode = mode;
        rig.open_with(spec).await;
        let mut turn = rig.turn("PLAN-REVISION");
        turn.approval_mode = mode;
        turn.plan = true;
        rig.adapter.start_turn(turn).await.unwrap();
        let events = rig
            .until(
                |event| matches!(event, SessionEvent::ApprovalRequested(_)),
                20,
            )
            .await;
        let Some(SessionEvent::ApprovalRequested(first)) = events.last() else {
            panic!("the first plan")
        };
        assert_eq!(first.request.request_kind, HarnessRequestKind::Plan);
        rig.adapter
            .resolve_approval(
                rig.thread,
                &first.request_ref,
                ApprovalDecision::Option {
                    option_id: "keep_planning".into(),
                    note: Some("Explain rollback first. 请先读配置。".into()),
                },
            )
            .await
            .unwrap();
        let events = rig
            .until(
                |event| matches!(event, SessionEvent::ApprovalRequested(_)),
                20,
            )
            .await;
        let Some(SessionEvent::ApprovalRequested(revised)) = events.last() else {
            panic!("the revised plan")
        };
        assert_eq!(revised.request.request_kind, HarnessRequestKind::Plan);
        assert_ne!(revised.request_ref, first.request_ref);
        assert_eq!(
            serde_json::to_value(&revised.request).unwrap()["detail"]["plan"],
            "Read, explain rollback, then change"
        );
        let controls = records(&program, ".controls");
        assert_eq!(
            controls.last().unwrap()["request"]["mode"],
            "plan",
            "keeping a plan cannot restore action permissions"
        );
        rig.adapter
            .resolve_approval(
                rig.thread,
                &revised.request_ref,
                ApprovalDecision::Option {
                    option_id: "proceed".into(),
                    note: None,
                },
            )
            .await
            .unwrap();
        let ended = rig.until_the_turn_ends().await;
        assert_eq!(common::outcome(&ended), TurnOutcome::Completed);
        let answers = records(&program, ".answers");
        assert_eq!(
            answers[0]["response"]["response"],
            json!({"behavior":"deny", "message":"Explain rollback first. 请先读配置。"})
        );
        assert_eq!(answers[1]["response"]["response"]["behavior"], "allow");
        let controls = records(&program, ".controls");
        assert_eq!(
            controls.last().unwrap()["request"]["subtype"],
            "set_permission_mode"
        );
        assert_eq!(controls.last().unwrap()["request"]["mode"], restored);
        rig.adapter.stop().await.unwrap();
    }
}
