use super::*;
use orch_link_contract::commands::{SerialKey, SessionClose};
use orch_types::new_id;
use serde_json::json;
use std::sync::Arc;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn failed_terminal_writes_keep_started_visible_and_retry_the_first_staged_outcome() -> TestResult {
    let directory = tempfile::tempdir()?;
    let now: Time = "2026-10-16T00:00:00Z".parse()?;
    let id = new_id();
    let cmd = Cmd::new(
        id,
        &SerialKey::Thread(new_id()),
        now,
        &SessionClose {
            thread_id: new_id(),
        },
    )?;
    let mut journal = Journal::open(directory.path(), Duration::from_secs(86_400), now)?;
    journal.accept(&cmd, now)?;
    journal.started(id)?;
    journal.terminal_writes(Arc::new(TestWrites::new(id, false, 1)));
    let original = json!({"captured":"original"});
    let (first, staged) = journal.stage_finish(id, Ok(original.clone()))?;
    let staged = staged.ok_or("no staged terminal write")?;
    assert!(staged.write().is_err());
    assert!(matches!(
        journal.get(id).map(|entry| &entry.step),
        Some(Step::Started)
    ));
    let disk = Journal::open(directory.path(), Duration::from_secs(86_400), now)?;
    assert!(matches!(
        disk.get(id).map(|entry| &entry.step),
        Some(Step::Started)
    ));
    let (second, retry) = journal.stage_finish(
        id,
        Err(CommandError::new(
            orch_link_contract::commands::ErrorCode::Internal,
            "a later caller chose another outcome",
        )),
    )?;
    assert_eq!(first, second);
    let retry = retry.ok_or("a failed write must still be staged")?;
    retry.write()?;
    journal.keep_finished(retry.entry);
    assert_eq!(
        journal
            .get(id)
            .and_then(Entry::result)
            .map(|answer| &answer.outcome),
        Some(&Ok(original.clone()))
    );
    let recovered = Journal::open(directory.path(), Duration::from_secs(86_400), now)?;
    assert_eq!(recovered.get(id).and_then(Entry::result), Some(&first));
    assert_eq!(
        journal.finish(id, Ok(json!({"captured":"replacement"})))?,
        first
    );
    Ok(())
}

#[test]
fn active_and_staged_entries_survive_age_collection_until_their_terminal_write_is_durable()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let now: Time = "2026-10-16T00:00:00Z".parse()?;
    let mut journal = Journal::open(directory.path(), Duration::from_secs(86_400), now)?;
    let first = new_id();
    let second = new_id();
    for id in [first, second] {
        journal.accept(
            &Cmd::new(
                id,
                &SerialKey::None,
                now,
                &SessionClose {
                    thread_id: new_id(),
                },
            )?,
            now,
        )?;
    }
    let (_, staged) = journal.stage_finish(first, Ok(json!({})))?;
    let later = Time::new(now.get() + chrono::Duration::days(2));
    assert_eq!(journal.purge_except(later, &HashSet::from([second]))?, 0);
    let staged = staged.ok_or("missing staged result")?;
    staged.write()?;
    journal.keep_finished(staged.entry);
    assert_eq!(journal.purge_except(later, &HashSet::from([second]))?, 1);
    assert!(journal.get(first).is_none());
    assert!(journal.get(second).is_some());
    Ok(())
}
