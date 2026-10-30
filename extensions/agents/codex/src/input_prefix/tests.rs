use super::*;
use checkpoint::{Position, Segment};
use std::io::Write;
use std::path::{Path, PathBuf};

type Proof = Result<(), Box<dyn std::error::Error>>;

fn input(turn: &str, id: &str) -> Vec<Json> {
    vec![
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":turn}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":id}]}}),
        json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":turn,
        "item":{"type":"UserMessage","id":id,"client_id":format!("client-{id}")}}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"retainedA"}]}}),
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":turn}}),
    ]
}

fn write(
    home: &Path,
    id: &str,
    base: Option<&Position>,
    rows: &[Json],
) -> Result<Segment, Box<dyn std::error::Error>> {
    let directory = home.join("sessions");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("rollout-fixture-{id}.jsonl"));
    let start = base.map_or(0, |base| base.end_ordinal_exclusive);
    let mut file = std::fs::File::create(&path)?;
    writeln!(
        file,
        "{}",
        json!({"type":"session_meta","ordinal":start,"payload":{
        "id":id,"cli_version":"0.165.0","history_mode":"paginated","history_base":base.map(|base|
            json!({"thread_id":base.thread_id,"end_ordinal_exclusive":base.end_ordinal_exclusive,"end_byte_offset":base.end_byte_offset}))}})
    )?;
    for (index, row) in rows.iter().enumerate() {
        let mut row = row.clone();
        row["ordinal"] = json!(start + index as u64 + 1);
        writeln!(file, "{row}")?;
    }
    drop(file);
    Ok(checkpoint::segment(home, &path, id, None)?)
}

fn source(id: &str) -> NativePrefixSource {
    NativePrefixSource {
        thread_id: orch_types::new_id(),
        segment_id: orch_types::new_id(),
        native_session_ref: id.into(),
    }
}
fn selection(turn: &str, id: &str) -> Selection {
    Selection {
        turn: turn.into(),
        item: id.into(),
        client: Some(format!("client-{id}")),
    }
}
fn cutoff(segment: &Segment, turn: &str) -> Result<Position, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(&segment.path)?;
    let mut offset = 0;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let row: Json = serde_json::from_slice(line)?;
        if row["type"] == "event_msg"
            && row["payload"]["type"] == "task_started"
            && row["payload"]["turn_id"] == turn
        {
            return Ok(Position {
                thread_id: segment.id.clone(),
                end_ordinal_exclusive: row["ordinal"].as_u64().ok_or("missing fixture ordinal")?,
                end_byte_offset: offset,
            });
        }
        offset += line.len() as u64;
    }
    Err("missing fixture turn".into())
}

#[test]
fn actual_first_input_membership_is_required_across_pages_and_duplicate_ids_refuse() -> Proof {
    let mut membership = Membership::default();
    membership.page(
        &[json!({"turnId":"turn-a","item":{"type":"userMessage","id":"a","clientId":"client-a"}})],
        "steer",
    )?;
    assert!(matches!(membership.page(&[json!({"turnId":"turn-a","item":{"type":"userMessage","id":"steer","clientId":"client-steer"}})],"steer"),Err(AdapterError::Unsupported(_))));
    let mut membership = Membership::default();
    membership.page(
        &[json!({"turnId":"turn-a","item":{"type":"userMessage","id":"a"}})],
        "client-b",
    )?;
    membership.page(&[json!({"turnId":"actual-turn-b","item":{"type":"userMessage","id":"b","clientId":"client-b"}})],"client-b")?;
    let selected = membership
        .selected
        .as_ref()
        .ok_or("missing selected input")?;
    assert_eq!(selected.turn, "actual-turn-b");
    assert_eq!(selected.item, "b");
    assert!(
        membership
            .page(
                &[json!({"turnId":"another-turn","item":{"type":"userMessage","id":"b"}})],
                "b"
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn external_goals_and_unknown_goal_state_are_refused_without_assuming_absence() -> Proof {
    goal_reply(&json!({"goal":null}))?;
    for goal in [
        json!({"text":"synthetic goal","status":"active"}),
        json!({"status":"paused"}),
        json!({"status":"completed"}),
    ] {
        assert!(matches!(
            goal_reply(&json!({"goal":goal})),
            Err(AdapterError::Unsupported(_))
        ));
    }
    assert!(goal_reply(&json!({})).is_err());
    Ok(())
}

#[test]
fn reused_client_identity_cannot_alias_two_retained_native_inputs() -> Proof {
    let home = tempfile::tempdir()?;
    let id = orch_types::new_id().to_string();
    let mut rows = input("turn-a", "a");
    rows.insert(
        3,
        json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"turn-a",
        "item":{"type":"UserMessage","id":"another-native-item","client_id":"client-a"}}}),
    );
    let segment = write(home.path(), &id, None, &rows)?;
    assert!(checkpoint::fingerprint(&[segment], &selection("turn-a", "a"), &source(&id)).is_err());
    Ok(())
}

#[test]
fn complete_inherited_prefix_changes_are_detected_and_appended_suffix_does_not_change_the_cut()
-> Proof {
    let home = tempfile::tempdir()?;
    let parent_id = orch_types::new_id().to_string();
    let child_id = orch_types::new_id().to_string();
    let mut rows = input("turn-a", "a");
    rows.extend(input("turn-b", "b"));
    let mut parent = write(home.path(), &parent_id, None, &rows)?;
    let end = cutoff(&parent, "turn-b")?;
    parent.end = Some(end.clone());
    let child_rows = input("turn-c", "c");
    let child = write(home.path(), &child_id, Some(&end), &child_rows)?;
    let source = source(&child_id);
    let selected = selection("turn-c", "c");
    let original = checkpoint::fingerprint(&[parent.clone(), child.clone()], &selected, &source)?;
    assert!(
        !original.1,
        "an inherited conversation cannot be treated as empty"
    );
    let mut later = child_rows;
    later.extend(input("turn-d", "d"));
    let appended = write(home.path(), &child_id, Some(&end), &later)?;
    assert_eq!(
        checkpoint::fingerprint(&[parent.clone(), appended], &selected, &source)?,
        original
    );
    let bytes = std::fs::read_to_string(&parent.path)?;
    std::fs::write(&parent.path, bytes.replacen("retainedA", "tamperedA", 1))?;
    assert_ne!(
        checkpoint::fingerprint(&[parent.clone(), child.clone()], &selected, &source)?.0,
        original.0
    );
    let mut truncated = std::fs::read(&parent.path)?;
    truncated.truncate(30);
    std::fs::write(&parent.path, truncated)?;
    assert!(checkpoint::fingerprint(&[parent, child], &selected, &source).is_err());
    assert!(
        !home.path().join(".orch-input-prefixes").exists(),
        "inspection allocates no auxiliary history"
    );
    Ok(())
}

#[test]
fn inherited_first_turn_and_ancestor_turn_edits_keep_all_earlier_context() -> Proof {
    let home = tempfile::tempdir()?;
    let parent_id = orch_types::new_id().to_string();
    let child_id = orch_types::new_id().to_string();
    let mut rows = input("turn-a", "a");
    rows.extend(input("turn-b", "b"));
    rows.extend(input("turn-z", "z"));
    let mut parent = write(home.path(), &parent_id, None, &rows)?;
    let end = cutoff(&parent, "turn-z")?;
    parent.end = Some(end.clone());
    let child = write(home.path(), &child_id, Some(&end), &input("turn-c", "c"))?;
    let source = source(&child_id);
    assert!(
        !checkpoint::fingerprint(
            &[parent.clone(), child.clone()],
            &selection("turn-b", "b"),
            &source
        )?
        .1
    );
    assert!(
        checkpoint::fingerprint(
            &[parent.clone(), child.clone()],
            &selection("turn-a", "a"),
            &source
        )?
        .1
    );
    assert!(
        checkpoint::fingerprint(
            std::slice::from_ref(&child),
            &selection("turn-c", "c"),
            &source
        )
        .is_err(),
        "missing ancestors cannot be ignored"
    );
    let mut cycle = parent.clone();
    cycle.base = Some(end);
    assert!(
        checkpoint::fingerprint(&[cycle, parent, child], &selection("turn-c", "c"), &source)
            .is_err()
    );
    Ok(())
}

#[test]
fn compaction_rollback_unfinished_prefix_and_in_turn_inputs_are_factual_refusals() -> Proof {
    let home = tempfile::tempdir()?;
    let id = orch_types::new_id().to_string();
    let source = source(&id);
    for fact in [
        json!({"type":"compacted","payload":{}}),
        json!({"type":"event_msg","payload":{"type":"thread_rolled_back"}}),
        json!({"type":"event_msg","payload":{"type":"context_compacted"}}),
    ] {
        let mut rows = input("turn-a", "a");
        rows.push(fact);
        let segment = write(home.path(), &id, None, &rows)?;
        assert!(matches!(
            checkpoint::fingerprint(&[segment], &selection("turn-a", "a"), &source),
            Err(AdapterError::Unsupported(_))
        ));
    }
    let mut rows = input("turn-a", "a");
    rows.pop();
    rows.extend(input("turn-b", "b"));
    let segment = write(home.path(), &id, None, &rows)?;
    assert!(matches!(
        checkpoint::fingerprint(&[segment], &selection("turn-b", "b"), &source),
        Err(AdapterError::Unsupported(_))
    ));
    let mut rows = input("turn-a", "a");
    rows.insert(3,json!({"type":"event_msg","payload":{"type":"item_completed","turn_id":"turn-a", "item":{"type":"UserMessage","id":"steer","client_id":"client-steer"}}}));
    let segment = write(home.path(), &id, None, &rows)?;
    assert!(matches!(
        checkpoint::fingerprint(&[segment], &selection("turn-a", "steer"), &source),
        Err(AdapterError::Unsupported(_))
    ));
    Ok(())
}

#[test]
fn compressed_native_ranges_have_the_same_identity_without_decoded_copies() -> Proof {
    let plain = tempfile::tempdir()?;
    let compressed = tempfile::tempdir()?;
    let id = orch_types::new_id().to_string();
    let mut rows = input("turn-a", "a");
    rows.extend(input("turn-b", "b"));
    let source = source(&id);
    let selected = selection("turn-b", "b");
    let segment = write(plain.path(), &id, None, &rows)?;
    let expected = checkpoint::fingerprint(std::slice::from_ref(&segment), &selected, &source)?;
    std::fs::create_dir_all(compressed.path().join("sessions"))?;
    let path = compressed
        .path()
        .join("sessions")
        .join(segment.path.file_name().ok_or("missing filename")?);
    let bytes = std::fs::read(&segment.path)?;
    std::fs::write(
        PathBuf::from(format!("{}.zst", path.display())),
        zstd::stream::encode_all(bytes.as_slice(), 0)?,
    )?;
    let segment = checkpoint::segment(compressed.path(), &path, &id, None)?;
    assert_eq!(
        checkpoint::fingerprint(&[segment], &selected, &source)?,
        expected
    );
    assert!(
        !path.exists(),
        "streaming verification must not publish a decoded copy"
    );
    Ok(())
}

fn disabled_goal_api() -> Result<Json, RpcError> {
    Err(RpcError::Remote {
        method: "thread/goal/get".into(),
        code: -32600,
        message: "goals feature is disabled".into(),
        data: None,
    })
}

#[test]
fn a_home_configured_with_goals_off_verifies_that_no_external_goal_can_run() -> Proof {
    let config = crate::process::effective_config(
        &crate::process::HomeOptions {
            managed: true,
            ..crate::process::HomeOptions::default()
        },
        &json!({"features":{"goals":true}}),
    );
    let home = HomeFacts::configured(true, &config);
    assert!(home.goals_disabled_by_orch);
    goal_state(disabled_goal_api(), home)?;
    assert!(goal_state(Ok(json!({"goal":{"status":"active"}})), home).is_err());
    Ok(())
}

#[test]
fn an_unmanaged_home_cannot_turn_a_disabled_goal_api_into_verified_absence() {
    let config = crate::process::effective_config(
        &crate::process::HomeOptions::default(),
        &json!({"features":{"goals":false}}),
    );
    let home = HomeFacts::configured(false, &config);
    assert!(!home.goals_disabled_by_orch);
    assert!(matches!(
        goal_state(disabled_goal_api(), home),
        Err(AdapterError::Unsupported(_))
    ));
}

#[test]
fn another_goal_state_error_stays_refused_even_in_an_orch_configured_home() {
    let config = crate::process::base_config(&crate::process::HomeOptions {
        managed: true,
        ..crate::process::HomeOptions::default()
    });
    let home = HomeFacts::configured(true, &config);
    assert!(home.goals_disabled_by_orch);
    let unrelated = Err(RpcError::Remote {
        method: "thread/goal/get".into(),
        code: -32600,
        message: "unrelated metadata failure".into(),
        data: None,
    });
    assert!(goal_state(unrelated, home).is_err());
    assert!(goal_state(Err(RpcError::Closed("connection ended".into())), home).is_err());
}
