//! Existing whole-completed-turn Fork, not Edit or an in-turn checkpoint.

use super::*;

#[tokio::test]
async fn a_full_fork_keeps_the_identified_native_turn_prefix_and_its_original_live_source() -> Proof
{
    let mut rig = Rig::start("input-prefix")
        .await
        .ok_or("the real pinned native harness is required")?;
    assert_eq!(rig.boot().await.harness_version, "0.165.0");
    let source = new_id();
    let initial = rig.spec(source, SessionMode::New, None);
    let original = rig.adapter.open_session(initial).await?;
    let command = "printf x >> prefix-tool-runs; cat prefix-tool-result";
    std::fs::write(rig.workspace.path().join("prefix-tool-result"), RESULT)?;
    let first = format!(
        "#!mock\nsay {BEFORE}\ntool exec_command {}\nthen\nsay {A}\n",
        json!({"cmd":command,"yield_time_ms":30000})
    );
    let source_first = rig.run_turn(source, &first).await;
    assert!(source_first.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    assert_eq!(
        std::fs::read(rig.workspace.path().join("prefix-tool-runs"))?,
        b"x"
    );
    let selected = rig.run_turn(source, B).await;
    let witnesses = selected
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemStarted(start)
                if matches!(
                    &start.content,
                    orch_agent_contract::events::ItemContent::AgentMessage(_)
                ) =>
            {
                Some(start.native_ref.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        witnesses.len(),
        1,
        "select the actual emitted native assistant item, not prompt text or a controller ordinal"
    );
    let third = rig.run_turn(source, C).await;
    assert!(third.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let baseline = rig
        .model_requests()
        .last()
        .ok_or("the original third model input is absent")?
        .body
        .clone();
    let recorded = rig
        .adapter
        .read_session(&original.native_session_ref, true)
        .await?;
    let record = recorded
        .file
        .ok_or("the pinned native history API produced no complete record")?;
    rig.prefix_evidence_file(std::path::Path::new(&record.path));
    let entries = std::fs::read_to_string(&record.path)?
        .lines()
        .map(serde_json::from_str::<RecordedEntry>)
        .collect::<Result<Vec<_>, _>>()?;
    let matched = entries
        .iter()
        .filter_map(|entry| match entry {
            RecordedEntry::Item(item) if item.native_ref == witnesses[0] => Some(item.turn),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matched.len(),
        1,
        "the actual native item identity must have unambiguous native turn membership"
    );
    let through = u32::try_from(matched[0])?;
    let path = source_file(&rig, &original.native_session_ref)?;
    let before = std::fs::read(&path)?;
    std::fs::write(rig.home.path().join("prefix-source-before.jsonl"), &before)?;
    let sentinel = rig.workspace.path().join("prefix-current-file");
    std::fs::write(&sentinel, b"newer current workspace\0\xff")?;
    let requests = rig.model_requests().len();
    let target = new_id();
    let mut spec = rig.spec(
        target,
        SessionMode::Fork,
        Some(&original.native_session_ref),
    );
    spec.settings.upto_turn_ordinal = Some(through);
    let opened = rig.adapter.open_session(spec).await?;
    assert_ne!(opened.native_session_ref, original.native_session_ref);
    assert_eq!(
        rig.model_requests().len(),
        requests,
        "native preparation must submit no model input or implicit continuation"
    );
    assert!(
        std::fs::read(&path)? == before,
        "full Fork must preserve its original native source bytes"
    );
    assert_eq!(std::fs::read(&sentinel)?, b"newer current workspace\0\xff");
    let child = rig
        .adapter
        .read_session(&opened.native_session_ref, true)
        .await?
        .file
        .ok_or("the native child has no record readback")?;
    rig.prefix_evidence_file(std::path::Path::new(&child.path));
    let child = std::fs::read_to_string(child.path)?
        .lines()
        .map(serde_json::from_str::<RecordedEntry>)
        .collect::<Result<Vec<_>, _>>()?;
    let expected = entries
        .into_iter()
        .filter(|entry| entry.turn() <= i64::from(through))
        .collect::<Vec<_>>();
    assert!(
        child == expected,
        "the complete ordered native record differs from the captured completed-turn cut"
    );
    // No close/resume of the source: its own original runtime remains usable.
    let old = rig.run_turn(source, CONTINUE).await;
    assert!(old.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let parent = rig
        .model_requests()
        .last()
        .ok_or("the retained original source did not continue")?
        .body
        .clone();
    assert!(marker_tape(&parent, &[A, B, C, CONTINUE], None).contains(&("user".into(), C.into())));
    let revised = rig.run_turn(target, C).await;
    assert!(revised.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(end) if end.status==TurnOutcome::Completed)));
    let actual = rig
        .model_requests()
        .last()
        .ok_or("the native child produced no actual model input")?
        .body
        .clone();
    assert!(
        codex_model_input(&actual, C, false)? == codex_model_input(&baseline, C, false)?,
        "all ordered model conversation/tool/unknown blocks must equal the original pre-third-input history"
    );
    assert_eq!(
        rig.model_requests().len(),
        requests + 2,
        "only the two explicit source/child inputs ran after preparation"
    );
    assert_eq!(
        std::fs::read(rig.workspace.path().join("prefix-tool-runs"))?,
        b"x",
        "fork/readback/continuation cannot replay the original tool"
    );
    assert_eq!(std::fs::read(&sentinel)?, b"newer current workspace\0\xff");
    std::fs::write(
        rig.home.path().join("prefix-proof.json"),
        serde_json::to_vec(&json!({"case":"confirmed-full-fork",
        "source":original.native_session_ref,"child":opened.native_session_ref,"native_item":witnesses[0],"native_ordinal":through}))?,
    )?;
    rig.adapter.stop().await?;
    let _ = rig.keep_prefix_evidence()?;
    rig.prefix_proof_succeeded();
    Ok(())
}
