//! Exact native checkpoint forks, verified in the pinned harness's actual model input.

mod common;
#[path = "input_prefix/confirmed_fork.rs"]
mod confirmed_fork;

use common::Rig;
use futures::StreamExt;
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_agent_contract::history::NativePrefixSource;
use orch_agent_contract::session::{SessionMode, TurnSteer};
use orch_testkit::doubles::model::RecordedRequest;
use orch_testkit::prefix_history::{
    claude_first_model_input, claude_source_preserved, full_model_input, marker_tape,
};
use orch_types::{Json, SystemClock};
use serde_json::json;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const WAIT: Duration = Duration::from_secs(90); // The existing native Rig turn budget.
const A: &str = "PREFIX-USER-A";
const B: &str = "PREFIX-USER-B";
const C: &str = "PREFIX-USER-C";
const REVISED: &str = "PREFIX-REVISED-INPUT";
const BEFORE: &str = "PREFIX-BEFORE-STEER-ASSISTANT";
const AFTER: &str = "PREFIX-AFTER-STEER-ASSISTANT";
const HELD: &str = "PREFIX-HELD-TOOL";
const RESULT: &str = "PREFIX-TOOL-RESULT";
const STEER: &str = "PREFIX-STEER-USER";
const CONTINUE: &str = "PREFIX-CONTINUE-ORIGINAL";

#[derive(Debug)]
struct WireGate {
    delegate: Arc<common::DockerLauncher>,
    program: std::path::PathBuf,
    sent: std::path::PathBuf,
    echoed: std::path::PathBuf,
}
impl orch_adapter_claude_code::process::Launcher for WireGate {
    fn command(
        &self,
        spec: &orch_adapter_claude_code::process::LaunchSpec,
    ) -> tokio::process::Command {
        let mut spec = spec.clone();
        let native = std::mem::replace(&mut spec.program, "node".into());
        spec.args.insert(0, native);
        spec.args
            .insert(0, self.program.to_string_lossy().into_owned());
        spec.env.insert(
            "ORCH_PREFIX_WIRE_SENT".into(),
            self.sent.to_string_lossy().into_owned(),
        );
        spec.env.insert(
            "ORCH_PREFIX_WIRE_ACK".into(),
            self.echoed.to_string_lossy().into_owned(),
        );
        orch_adapter_claude_code::process::Launcher::command(self.delegate.as_ref(), &spec)
    }
}

fn model_requests(rig: &Rig) -> Vec<RecordedRequest> {
    rig.model
        .requests()
        .into_iter()
        .filter(|request| request.format.is_some())
        .collect()
}

fn retain_prefix_evidence(rig: &Rig) -> Proof {
    let bodies: Vec<_> = model_requests(rig)
        .into_iter()
        .map(|request| request.body)
        .collect();
    let _ = orch_testkit::prefix_history::preserve_prefix_failure(
        "claude",
        rig.home.path(),
        rig.work.path(),
        &[],
        &bodies,
        &[common::TOKEN],
        orch_adapter_claude_code::config::FRAME_MAX_BYTES,
    )?;
    Ok(())
}

fn native_input(
    path: &Path,
    marker: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut found = Vec::new();
    for line in std::fs::read_to_string(path)?.lines() {
        let row: Json = serde_json::from_str(line)?;
        if row["type"] == "user"
            && row["isSidechain"] != true
            && row["isMeta"] != true
            && row["message"]["content"].to_string().contains(marker)
        {
            found.push(
                row["uuid"]
                    .as_str()
                    .ok_or("a native user entry has no UUID")?
                    .to_owned(),
            );
        }
    }
    assert_eq!(
        found.len(),
        1,
        "the fixture must observe exactly one actual stored native user identity"
    );
    Ok(found.remove(0))
}

fn first_native_user_text(
    path: &Path,
    session: &str,
    identity: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let (bytes, _) = queue_rows(path, session)?;
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let row: Json = serde_json::from_slice(line)?;
        if row["type"] != "user" || row["isMeta"] == true {
            continue;
        }
        if row["uuid"] != identity || row["sessionId"] != session || row["isSidechain"] == true {
            return Err(
                "the empty-prefix input is not the actual first native main-user identity".into(),
            );
        }
        let content = &row["message"]["content"];
        return content
            .as_str()
            .or_else(|| {
                content
                    .as_array()
                    .and_then(|blocks| blocks.iter().find(|block| block["type"] == "text"))
                    .and_then(|block| block["text"].as_str())
            })
            .map(str::to_owned)
            .ok_or_else(|| {
                "the first native input has no retained text for its billing fingerprint".into()
            });
    }
    Err("the native source has no first non-meta user input".into())
}

#[derive(serde::Serialize)]
struct QueueBaseline {
    session: String,
    bytes: usize,
    fingerprint: orch_types::Hash,
    enqueues: usize,
}

fn queue_rows(
    path: &Path,
    session: &str,
) -> Result<(Vec<u8>, usize), Box<dyn std::error::Error + Send + Sync>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(orch_adapter_claude_code::config::FRAME_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > orch_adapter_claude_code::config::FRAME_MAX_BYTES {
        return Err("the synthetic queue witness exceeds the native frame budget".into());
    }
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    bytes.truncate(complete);
    let mut enqueues = 0;
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let row: Json = serde_json::from_slice(line)?;
        if row["type"] == "queue-operation" {
            if row["sessionId"] != session {
                return Err("a queue witness belongs to another native source session".into());
            }
            if row["operation"] == "enqueue" {
                enqueues += 1;
            }
        }
    }
    Ok((bytes, enqueues))
}

fn queue_baseline(
    path: &Path,
    session: &str,
) -> Result<QueueBaseline, Box<dyn std::error::Error + Send + Sync>> {
    let (bytes, enqueues) = queue_rows(path, session)?;
    Ok(QueueBaseline {
        session: session.into(),
        bytes: bytes.len(),
        fingerprint: orch_types::Hash::of(&bytes),
        enqueues,
    })
}

fn queue_advanced(
    path: &Path,
    baseline: &QueueBaseline,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let (bytes, enqueues) = queue_rows(path, &baseline.session)?;
    if bytes.len() < baseline.bytes
        || orch_types::Hash::of(&bytes[..baseline.bytes]) != baseline.fingerprint
    {
        return Err("the exact native source queue baseline was replaced or truncated".into());
    }
    if enqueues > baseline.enqueues + 1 {
        return Err("more than the sole controlled steer was queued during the held tool".into());
    }
    Ok(enqueues == baseline.enqueues + 1)
}

fn queued_input_witness(
    path: &Path,
    session: &str,
    input: &str,
) -> Result<Json, Box<dyn std::error::Error + Send + Sync>> {
    input.parse::<orch_types::Id>()?;
    let (bytes, _) = queue_rows(path, session)?;
    let rows: Vec<Json> = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice)
        .collect::<Result<_, _>>()?;
    let found: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row["type"] == "attachment"
                && row["attachment"]["type"] == "queued_command"
                && row["attachment"]["source_uuid"] == input
        })
        .collect();
    if found.len() != 1 {
        return Err(
            "the echoed native input must name exactly one queued-command source identity".into(),
        );
    }
    let (index, entry) = found[0];
    if entry["sessionId"] != session
        || entry["isSidechain"] == true
        || entry["renderedRole"] != "system"
    {
        return Err("the native queued input is not main-session system guidance".into());
    }
    let attachment = entry["uuid"]
        .as_str()
        .ok_or("the native queued attachment has no UUID")?;
    attachment.parse::<orch_types::Id>()?;
    if attachment == input {
        return Err(
            "the queued source identity must remain distinct from its transcript attachment".into(),
        );
    }
    let parent = entry["parentUuid"]
        .as_str()
        .ok_or("the native queued input has no retained parent")?;
    let parents: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row["uuid"] == parent)
        .collect();
    if parents.len() != 1 {
        return Err("the native queued input has a missing or ambiguous retained parent".into());
    }
    let (parent_index, parent) = parents[0];
    if parent_index >= index
        || parent["sessionId"] != session
        || parent["type"] != "user"
        || !parent["message"]["content"]
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
    {
        return Err("the queued input did not follow its actual main-session tool result".into());
    }
    let delivery = entry["attachment"]["delivery_id"]
        .as_str()
        .ok_or("the queued input has no native delivery identity")?;
    delivery.parse::<orch_types::Id>()?;
    let removals: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row["type"] == "queue-operation"
                && row["operation"] == "remove"
                && row["commandUuid"] == input
                && row["deliveryId"] == delivery
        })
        .collect();
    if removals.len() != 1
        || removals[0].0 <= index
        || removals[0].1["sessionId"] != session
        || removals[0].1["reason"] != "absorbed_mid_turn"
    {
        return Err(
            "the native queued input lacks its matching same-session mid-turn consumption record"
                .into(),
        );
    }
    Ok(json!({"source_uuid":input,"attachment":entry,"parent":parent,"consumption":removals[0].1}))
}

#[test]
fn an_anonymous_queue_phase_is_source_bound_and_never_claims_user_identity() -> Proof {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(
        file.path(),
        b"{\"type\":\"queue-operation\",\"sessionId\":\"source\",\"operation\":\"enqueue\"}\n",
    )?;
    let baseline = queue_baseline(file.path(), "source")?;
    assert!(!queue_advanced(file.path(), &baseline)?);
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new().append(true).open(file.path())?,
        "{}",
        json!({"type":"queue-operation","sessionId":"source","operation":"enqueue"})
    )?;
    assert!(
        queue_advanced(file.path(), &baseline)?,
        "a new queue phase does not itself name or certify a delivered user input"
    );
    assert!(queue_baseline(file.path(), "another-source").is_err());
    std::fs::write(file.path(), b"{}\n")?;
    assert!(queue_advanced(file.path(), &baseline).is_err());
    Ok(())
}

#[test]
fn queued_guidance_requires_distinct_source_and_entry_uuids_and_the_actual_consumed_parent() -> Proof
{
    const SESSION: &str = "11111111-1111-4111-8111-111111111111";
    const INPUT: &str = "22222222-2222-4222-8222-222222222222";
    const ENTRY: &str = "33333333-3333-4333-8333-333333333333";
    const PARENT: &str = "44444444-4444-4444-8444-444444444444";
    const DELIVERY: &str = "55555555-5555-4555-8555-555555555555";
    let file = tempfile::NamedTempFile::new()?;
    let rows = vec![
        json!({"type":"user","uuid":PARENT,"sessionId":SESSION,"message":{"role":"user",
            "content":[{"type":"tool_result","tool_use_id":"held-call","content":RESULT,"is_error":false}]}}),
        json!({"type":"attachment","uuid":ENTRY,"parentUuid":PARENT,"sessionId":SESSION,"renderedRole":"system",
            "attachment":{"type":"queued_command","source_uuid":INPUT,"delivery_id":DELIVERY,
                "prompt":[{"type":"text","text":STEER}]}}),
        json!({"type":"queue-operation","sessionId":SESSION,"operation":"remove","reason":"absorbed_mid_turn",
            "commandUuid":INPUT,"deliveryId":DELIVERY}),
    ];
    let write = |rows: &[Json]| {
        std::fs::write(
            file.path(),
            rows.iter()
                .map(Json::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
    };
    write(&rows)?;
    let witness = queued_input_witness(file.path(), SESSION, INPUT)?;
    assert_eq!(witness["source_uuid"], INPUT);
    assert_eq!(witness["attachment"]["uuid"], ENTRY);
    assert_eq!(
        witness["parent"]["message"]["content"],
        rows[0]["message"]["content"]
    );
    assert!(
        queued_input_witness(file.path(), SESSION, ENTRY).is_err(),
        "an entry UUID cannot impersonate the echoed input UUID"
    );
    for (index, field, value) in [
        (1, "sessionId", json!("another-session")),
        (1, "parentUuid", json!(INPUT)),
        (2, "deliveryId", json!(ENTRY)),
        (2, "reason", json!("not_absorbed")),
    ] {
        let mut changed = rows.clone();
        changed[index][field] = value;
        write(&changed)?;
        assert!(queued_input_witness(file.path(), SESSION, INPUT).is_err());
    }
    let mut changed = rows.clone();
    changed[1]["attachment"]["source_uuid"] = json!(ENTRY);
    write(&changed)?;
    assert!(
        queued_input_witness(file.path(), SESSION, INPUT).is_err(),
        "identical authored text is not native checkpoint identity"
    );
    let mut duplicate = rows.clone();
    duplicate.push(rows[1].clone());
    write(&duplicate)?;
    assert!(queued_input_witness(file.path(), SESSION, INPUT).is_err());
    Ok(())
}

#[test]
fn the_first_billing_input_is_bound_to_its_native_identity_not_repeated_text() -> Proof {
    const FIRST: &str = "11111111-1111-4111-8111-111111111111";
    const SECOND: &str = "22222222-2222-4222-8222-222222222222";
    const SOURCE: &str = "33333333-3333-4333-8333-333333333333";
    let file = tempfile::NamedTempFile::new()?;
    let rows = [
        json!({"type":"user","uuid":FIRST,"sessionId":SOURCE,"message":{"content":A}}),
        json!({"type":"user","uuid":SECOND,"sessionId":SOURCE,"message":{"content":A}}),
    ];
    std::fs::write(
        file.path(),
        rows.iter()
            .map(Json::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )?;
    assert_eq!(first_native_user_text(file.path(), SOURCE, FIRST)?, A);
    assert!(
        first_native_user_text(file.path(), SOURCE, SECOND).is_err(),
        "identical text does not make the second native input the first"
    );
    assert!(first_native_user_text(file.path(), SECOND, FIRST).is_err());
    let mut side = rows[0].clone();
    side["isSidechain"] = json!(true);
    std::fs::write(file.path(), format!("{side}\n"))?;
    assert!(first_native_user_text(file.path(), SOURCE, FIRST).is_err());
    std::fs::write(
        file.path(),
        format!(
            "{}\n",
            json!({"type":"user","uuid":FIRST,"isMeta":true,"sessionId":SOURCE,"message":{"content":A}})
        ),
    )?;
    assert!(first_native_user_text(file.path(), SOURCE, FIRST).is_err());
    Ok(())
}

async fn fresh_native_prefix_case(first: bool) -> Proof {
    let mut rig = Rig::new()
        .await
        .ok_or("the pinned Docker harness is required for prefix acceptance")?;
    assert_eq!(rig.start_quick().await.harness_version, "2.1.296");
    let original = rig.open().await;
    let source = NativePrefixSource {
        thread_id: rig.thread,
        segment_id: rig.segment,
        native_session_ref: original.native_session_ref.clone(),
    };
    let selected = if first { A } else { B };
    let mut selected_model = None;
    for text in [A, B, C] {
        let (_, events) = rig.run(text).await;
        assert_eq!(common::outcome(&events), TurnOutcome::Completed);
        if text == selected {
            selected_model = Some(
                model_requests(&rig)
                    .last()
                    .ok_or("the selected source model input is absent")?
                    .body
                    .clone(),
            );
        }
    }
    let path = orch_adapter_claude_code::history::find_transcript(
        rig.home.path(),
        &original.native_session_ref,
    )
    .ok_or("the pinned source transcript is absent")?;
    let id = native_input(&path, if first { A } else { B })?;
    if first {
        assert_eq!(
            first_native_user_text(&path, &original.native_session_ref, &id)?,
            selected
        );
    }
    std::fs::write(
        rig.home.path().join("prefix-proof.json"),
        serde_json::to_vec(&json!({
            "case":if first {"first"} else {"middle"},"source":source.clone(),"requested_native_input":id,
        }))?,
    )?;
    let token = rig
        .adapter
        .inspect_input_prefix(source.clone(), &id)
        .await?;
    std::fs::write(
        rig.home.path().join("prefix-proof.json"),
        serde_json::to_vec(
            &json!({"case":if first {"first"} else {"middle"},"prefix":token.clone()}),
        )?,
    )?;
    assert_eq!(token.empty, first);
    let mut occupied = rig.spec(SessionMode::Fork, Some(&original.native_session_ref));
    occupied.settings.segment_id = orch_types::new_id();
    let count = model_requests(&rig).len();
    assert_eq!(count, 3, "only the three authored source inputs ran");
    assert!(
        rig.adapter
            .open_input_prefix(occupied, token.clone())
            .await
            .is_err()
    );
    assert!(
        rig.adapter
            .inspect_input_prefix(source.clone(), "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
            .await
            .is_err()
    );
    let mut wrong = source.clone();
    wrong.segment_id = orch_types::new_id();
    assert!(rig.adapter.inspect_input_prefix(wrong, &id).await.is_err());
    assert_eq!(
        model_requests(&rig).len(),
        count,
        "refused preparation submits no input or hidden turn"
    );
    rig.adapter.close_session(rig.thread).await?;
    let old = std::fs::read(&path)?;
    std::fs::write(rig.home.path().join("prefix-source-before.jsonl"), &old)?;
    let sentinel = rig.work.path().join("prefix-current-file");
    std::fs::write(&sentinel, b"current workspace\0\xff")?;
    rig.segment = orch_types::new_id();
    let mut stale = token.clone();
    stale.fingerprint = orch_types::Hash::of(b"different inspected prefix");
    let refused = rig.spec(SessionMode::Fork, Some(&original.native_session_ref));
    assert!(rig.adapter.open_input_prefix(refused, stale).await.is_err());
    let spec = rig.spec(SessionMode::Fork, Some(&original.native_session_ref));
    let opened = rig.adapter.open_input_prefix(spec, token.clone()).await?;
    assert_ne!(
        opened.session.native_session_ref,
        original.native_session_ref
    );
    assert_eq!(opened.prefix, token);
    assert_eq!(
        model_requests(&rig).len(),
        count,
        "native preparation must not submit a model input or continue a goal"
    );
    claude_source_preserved(&old, &std::fs::read(&path)?, &original.native_session_ref)?;
    assert_eq!(std::fs::read(&sentinel)?, b"current workspace\0\xff");
    let (revised_id, events) = rig.run(REVISED).await;
    assert_eq!(common::outcome(&events), TurnOutcome::Completed);
    let materialized = orch_adapter_claude_code::history::find_transcript(
        rig.home.path(),
        &opened.session.native_session_ref,
    )
    .ok_or("the distinct native prefix identity was not materialized by the real harness")?;
    if first {
        assert_eq!(
            first_native_user_text(
                &materialized,
                &opened.session.native_session_ref,
                &revised_id.to_string()
            )?,
            REVISED
        );
    }
    let rows: Vec<Json> = std::fs::read_to_string(materialized)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert!(
        rows.iter().any(|row| row["type"] == "user"
            && row["uuid"] == revised_id.to_string()
            && row["sessionId"] == opened.session.native_session_ref),
        "the actual native input must belong to the reported fresh session"
    );
    let request = model_requests(&rig)
        .pop()
        .ok_or("the revised model input is absent")?;
    let expected: Vec<(String, String)> = if first {
        vec![("user".into(), REVISED.into())]
    } else {
        vec![
            ("user".into(), A.into()),
            ("assistant".into(), A.into()),
            ("user".into(), REVISED.into()),
        ]
    };
    assert_eq!(
        marker_tape(&request.body, &[A, B, C, REVISED], None),
        expected,
        "the real model must receive the exact chosen user/assistant prefix, not hidden old suffixes"
    );
    assert_eq!(std::fs::read(&sentinel)?, b"current workspace\0\xff");
    claude_source_preserved(&old, &std::fs::read(&path)?, &original.native_session_ref)?;
    rig.adapter.close_session(rig.thread).await?;
    rig.segment = orch_types::new_id();
    let resume = rig.spec(SessionMode::Resume, Some(&original.native_session_ref));
    rig.open_with(resume).await;
    let (_, resumed) = rig.run(CONTINUE).await;
    assert_eq!(common::outcome(&resumed), TurnOutcome::Completed);
    let parent = model_requests(&rig)
        .pop()
        .ok_or("the original session did not resume")?;
    assert_eq!(
        marker_tape(&parent.body, &[A, B, C, REVISED, CONTINUE], None),
        vec![
            ("user".into(), A.into()),
            ("assistant".into(), A.into()),
            ("user".into(), B.into()),
            ("assistant".into(), B.into()),
            ("user".into(), C.into()),
            ("assistant".into(), C.into()),
            ("user".into(), CONTINUE.into()),
        ],
        "resuming the original retains every complete original exchange without the revised branch"
    );
    let selected_model =
        selected_model.ok_or("the selected source model input was not recorded")?;
    let (expected_full, actual_full) = if first {
        // Only the actual first input of a proved empty prefix can change
        // this native input-derived billing checksum. Middle stays strict.
        (
            claude_first_model_input(&selected_model, selected)?,
            claude_first_model_input(&request.body, REVISED)?,
        )
    } else {
        (
            full_model_input(&selected_model, selected)?,
            full_model_input(&request.body, REVISED)?,
        )
    };
    assert!(
        actual_full == expected_full,
        "full native model input differs: unknown user/developer/tool content cannot be hidden by marker selection; see retained model-body evidence"
    );
    rig.adapter.stop().await?;
    retain_prefix_evidence(&rig)?;
    rig.prefix_proof_succeeded();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_prefix_session_excludes_the_actual_input_and_suffix_and_preserves_the_original()
-> Proof {
    fresh_native_prefix_case(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_native_prefix_preserves_the_complete_first_model_input() -> Proof {
    fresh_native_prefix_case(true).await
}

async fn entered(rig: &mut Rig) -> Proof {
    let marker = rig.work.path().join("prefix-tool-entered");
    tokio::time::timeout(WAIT,async {
        while !marker.exists() {
            tokio::select! {
                event=rig.events.next()=>match event.ok_or("the native event stream ended before the held tool")? {
                    AdapterEvent::Session {event:SessionEvent::ApprovalRequested(ask),..}=>
                        rig.adapter.resolve_approval(rig.thread,&ask.request_ref,common::allow(&ask)).await?,
                    AdapterEvent::Session {event:SessionEvent::TurnCompleted(_),..}=>return Err("the native turn ended before its deterministic tool latch".into()),
                    _=>{},
                },
                ()=tokio::time::sleep(Duration::from_millis(20))=>{},
            }
        }
        Ok::<(),Box<dyn std::error::Error+Send+Sync>>(())
    }).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_actual_queued_steer_keeps_its_native_guidance_and_refuses_a_user_message_checkpoint()
-> Proof {
    let mut rig = Rig::new()
        .await
        .ok_or("the pinned Docker harness is required for prefix acceptance")?;
    let gate = rig.home.path().join("prefix-wire-gate.cjs");
    std::fs::write(&gate, orch_testkit::prefix_history::STEER_WIRE_GATE)?;
    let sent = rig.work.path().join("prefix-steer-sent");
    let echoed = rig.work.path().join("prefix-steer-echoed");
    rig.adapter = orch_adapter_claude_code::ClaudeCodeAdapter::with_launcher(
        Arc::new(SystemClock),
        Arc::new(WireGate {
            delegate: rig.launcher.clone(),
            program: gate,
            sent: sent.clone(),
            echoed: echoed.clone(),
        }),
    );
    assert_eq!(rig.start_quick().await.harness_version, "2.1.296");
    let original = rig.open().await;
    let source = NativePrefixSource {
        thread_id: rig.thread,
        segment_id: rig.segment,
        native_session_ref: original.native_session_ref.clone(),
    };
    std::fs::write(
        rig.home.path().join("prefix-proof.json"),
        serde_json::to_vec(&json!({"case":"steer","source":source.clone()}))?,
    )?;
    let (_, base) = rig.run(A).await;
    assert_eq!(common::outcome(&base), TurnOutcome::Completed);
    let path = orch_adapter_claude_code::history::find_transcript(
        rig.home.path(),
        &original.native_session_ref,
    )
    .ok_or("the source transcript is absent")?;
    let first = native_input(&path, A)?;
    std::fs::write(rig.work.path().join("prefix-tool-result"), RESULT)?;
    let command = "# PREFIX-HELD-TOOL\nprintf x >> prefix-tool-runs; printf x > prefix-tool-entered; while [ ! -f prefix-tool-release ]; do sleep 0.02; done; cat prefix-tool-result";
    let prompt = format!(
        "#!mock\nsay {BEFORE}\ntool Bash {}\nthen\nsay {AFTER}\n",
        json!({"command":command,"description":"held native prefix proof"})
    );
    let turn = rig.turn(&prompt);
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await?;
    entered(&mut rig).await?;
    assert!(
        rig.adapter
            .inspect_input_prefix(source.clone(), &first)
            .await
            .is_err(),
        "the current exact source lane must be quiescent"
    );
    let baseline = queue_baseline(&path, &original.native_session_ref)?;
    std::fs::write(
        rig.home.path().join("prefix-proof.json"),
        serde_json::to_vec(
            &json!({"case":"steer","source":source.clone(),"queue_baseline":&baseline}),
        )?,
    )?;
    let release = rig.work.path().join("prefix-tool-release");
    let deadline = tokio::time::Instant::now() + WAIT;
    let steering = rig.adapter.steer(TurnSteer {
        thread_id: rig.thread,
        turn_id,
        text: STEER.into(),
        images: Vec::new(),
    });
    let releasing = async {
        // Anonymous native queue persistence is only a phase latch. The exact
        // UUID echo, retained input and same-turn model delivery are proved below.
        tokio::time::timeout_at(deadline, async {
            while !sent.exists() || !queue_advanced(&path, &baseline)? {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        })
        .await
        .map_err(|_| "the held native tool did not observe the new source-session queue phase")??;
        std::fs::write(release, b"release")?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (steered, released) = tokio::join!(steering, releasing);
    steered?;
    released?;
    tokio::time::timeout_at(deadline, async {
        while !echoed.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| "the released native tool was not followed by the matching steer UUID echo")?;
    let events = rig.until_the_turn_ends().await;
    assert_eq!(common::outcome(&events), TurnOutcome::Completed);
    assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.turn_id==turn_id && end.status==TurnOutcome::Completed)),
        "the steer must be consumed by the actual admitted original turn");
    let markers = [A, BEFORE, HELD, AFTER, RESULT, STEER, REVISED, CONTINUE];
    let requests = model_requests(&rig);
    let delivered = requests
        .iter()
        .find(|request| {
            marker_tape(&request.body, &[STEER], None).contains(&("system".into(), STEER.into()))
        })
        .ok_or("the native queued steer never reached its actual system-role model guidance")?;
    let delivered_tape = marker_tape(&delivered.body, &markers, None);
    assert!(delivered_tape.contains(&("assistant".into(), BEFORE.into())));
    assert!(delivered_tape.contains(&("tool_call".into(), HELD.into())));
    assert!(delivered_tape.contains(&("tool_result".into(), RESULT.into())));
    assert_eq!(
        delivered_tape
            .iter()
            .filter(|(_, marker)| marker == STEER)
            .count(),
        1,
        "the sole controlled queued input must be delivered exactly once, in its native role"
    );
    let final_text = events
        .iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(done) => done.payload["text"].as_str(),
            _ => None,
        })
        .ok_or("the original native final response is absent")?;
    let mut original_tape = marker_tape(
        &requests
            .last()
            .ok_or("the original native model input is absent")?
            .body,
        &markers,
        None,
    );
    original_tape.extend(marker_tape(
        &json!({"messages":[{"role":"assistant","content":final_text}]}),
        &markers,
        None,
    ));
    original_tape.push(("user".into(), CONTINUE.into()));
    let native = std::fs::read_to_string(&sent)?;
    assert_eq!(
        std::fs::read_to_string(&echoed)?,
        native,
        "an anonymous enqueue cannot replace actual matching native echo"
    );
    let witness = queued_input_witness(&path, &original.native_session_ref, &native)?;
    assert_eq!(
        witness["attachment"]["attachment"]["prompt"],
        json!([{"type":"text","text":STEER}]),
        "identity is the echoed source UUID; the sole authored bytes remain independently exact"
    );
    let messages = delivered.body["messages"]
        .as_array()
        .ok_or("the native model request has no ordered messages")?;
    let guidance: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row["role"] == "system"
                && row["content"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["text"]
                            .as_str()
                            .is_some_and(|text| text.contains(STEER))
                    })
                })
        })
        .collect();
    assert_eq!(guidance.len(), 1);
    let index = guidance[0].0;
    assert!(index > 0);
    assert_eq!(messages[index - 1]["role"], "user");
    assert_eq!(
        messages[index - 1]["content"],
        witness["parent"]["message"]["content"],
        "every actual tool-result block before native guidance must reach the model unchanged"
    );
    assert!(
        full_model_input(&delivered.body, STEER).is_err(),
        "system-role guidance is not one authored user block and cannot receive user-text substitution"
    );
    let preparation_count = model_requests(&rig).len();
    let attachment = witness["attachment"]["uuid"]
        .as_str()
        .ok_or("the native attachment identity disappeared")?;
    for identity in [native.as_str(), attachment] {
        assert!(
            matches!(
                rig.adapter
                    .inspect_input_prefix(source.clone(), identity)
                    .await,
                Err(orch_agent_contract::adapter::AdapterError::Unsupported(_))
            ),
            "a queued source or attachment UUID must not be rounded to an earlier user turn"
        );
    }
    let mut fabricated = rig.adapter.inspect_input_prefix(source, &first).await?;
    fabricated.before_input_ref = native.clone();
    assert_eq!(
        model_requests(&rig).len(),
        preparation_count,
        "inspection cannot author an input while refusing a checkpoint"
    );
    std::fs::write(
        rig.home.path().join("prefix-proof.json"),
        serde_json::to_vec(&json!({
            "case":"accepted-native-guidance-with-explicit-checkpoint-refusal","native_input":&witness,
            "full_model_input":delivered.body,"queue_baseline":&baseline,"echoed_uuid":native,"admitted_turn":turn_id,
        }))?,
    )?;
    rig.adapter.close_session(rig.thread).await?;
    let original_bytes = std::fs::read(&path)?;
    let sentinel = rig.work.path().join("prefix-current-file");
    std::fs::write(&sentinel, b"current workspace\0\xff")?;
    std::fs::write(
        rig.home.path().join("prefix-source-before.jsonl"),
        &original_bytes,
    )?;
    rig.segment = orch_types::new_id();
    let target = rig.spec(SessionMode::Fork, Some(&original.native_session_ref));
    let count = model_requests(&rig).len();
    assert_eq!(
        count, preparation_count,
        "closing a refused source cannot author a preparation input"
    );
    assert!(
        matches!(
            rig.adapter.open_input_prefix(target, fabricated).await,
            Err(orch_agent_contract::adapter::AdapterError::Unsupported(_))
        ),
        "a fabricated prepared token cannot bypass the per-input support boundary"
    );
    assert_eq!(
        model_requests(&rig).len(),
        count,
        "unsupported preparation authors no model turn"
    );
    claude_source_preserved(
        &original_bytes,
        &std::fs::read(&path)?,
        &original.native_session_ref,
    )?;
    assert_eq!(
        std::fs::read(rig.work.path().join("prefix-tool-runs"))?,
        b"x"
    );
    assert_eq!(std::fs::read(&sentinel)?, b"current workspace\0\xff");
    rig.segment = orch_types::new_id();
    let source = rig.spec(SessionMode::Resume, Some(&original.native_session_ref));
    rig.open_with(source).await;
    let (_, resumed) = rig.run(CONTINUE).await;
    assert_eq!(common::outcome(&resumed), TurnOutcome::Completed);
    let continued = model_requests(&rig)
        .pop()
        .ok_or("the original steered native session did not resume")?;
    assert_eq!(marker_tape(&continued.body, &markers, None), original_tape);
    assert_eq!(
        std::fs::read(rig.work.path().join("prefix-tool-runs"))?,
        b"x",
        "the retained source continuation must not replay its old tool"
    );
    assert_eq!(std::fs::read(&sentinel)?, b"current workspace\0\xff");
    rig.adapter.stop().await?;
    retain_prefix_evidence(&rig)?;
    rig.prefix_proof_succeeded();
    Ok(())
}
