//! Preserve the existing truthful bounded-Fork refusal; no empty side fallback.

use super::*;

#[tokio::test]
async fn a_bounded_full_fork_refuses_without_touching_the_original_native_session() -> Proof {
    let mut rig = Rig::new()
        .await
        .ok_or("the real pinned native harness is required")?;
    assert_eq!(rig.start_quick().await.harness_version, "2.1.296");
    let original = rig.open().await;
    for text in [A, B, C] {
        let (_, events) = rig.run(text).await;
        assert_eq!(common::outcome(&events), TurnOutcome::Completed);
    }
    let path = orch_adapter_claude_code::history::find_transcript(
        rig.home.path(),
        &original.native_session_ref,
    )
    .ok_or("the original native transcript is unavailable")?;
    let before = std::fs::read(&path)?;
    std::fs::write(rig.home.path().join("prefix-source-before.jsonl"), &before)?;
    let sentinel = rig.work.path().join("prefix-current-file");
    std::fs::write(&sentinel, b"current full-fork workspace\0\xff")?;
    let count = model_requests(&rig).len();
    let mut spec = rig.spec(SessionMode::Fork, Some(&original.native_session_ref));
    spec.settings.thread_id = orch_types::new_id();
    spec.settings.segment_id = orch_types::new_id();
    spec.settings.upto_turn_ordinal = Some(2);
    assert!(
        matches!(
            rig.adapter.open_session(spec).await,
            Err(orch_agent_contract::adapter::AdapterError::Unsupported(_))
        ),
        "the unverified bounded native path stays unsupported; this is not a successful fork claim"
    );
    assert_eq!(
        model_requests(&rig).len(),
        count,
        "refusal authors no native model input"
    );
    claude_source_preserved(
        &before,
        &std::fs::read(&path)?,
        &original.native_session_ref,
    )?;
    assert_eq!(
        std::fs::read(&sentinel)?,
        b"current full-fork workspace\0\xff"
    );
    let (_, events) = rig.run(CONTINUE).await;
    assert_eq!(common::outcome(&events), TurnOutcome::Completed);
    let continued = model_requests(&rig)
        .last()
        .ok_or("the original native continuation is absent")?
        .body
        .clone();
    let tape = marker_tape(&continued, &[A, B, C, CONTINUE], None);
    for text in [A, B, C, CONTINUE] {
        assert!(tape.contains(&("user".into(), text.into())));
    }
    assert_eq!(model_requests(&rig).len(), count + 1);
    rig.adapter.stop().await?;
    retain_prefix_evidence(&rig)?;
    rig.prefix_proof_succeeded();
    Ok(())
}
