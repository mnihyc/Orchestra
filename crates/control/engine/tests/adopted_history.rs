//! What a session held before Orch took it in is on its thread once, as its harness recorded it, from the session before Orch, and is never sent to the agent again.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, queued};
use orch_agent_contract::discovery::{RecordedEntry, RecordedItem, RecordedWords};
use orch_agent_contract::events::{AgentMessagePayload, ItemContent, ItemStatus, MAIN_STREAM};
use orch_authz::Caller;
use orch_link_contract::commands::Command;

/// The earlier words of the session: what a person said, and what the agent answered.
const ASKED: &str = "remember the word banana";
const ANSWERED: &str = "I will remember banana";

fn record() -> Vec<RecordedEntry> {
    vec![
        RecordedEntry::Said(RecordedWords {
            native_ref: "said:u1".to_owned(),
            turn: 1,
            text: ASKED.to_owned(),
            left_out: 0,
            at: None,
        }),
        RecordedEntry::Item(RecordedItem {
            native_ref: "msg-1".to_owned(),
            turn: 1,
            stream: MAIN_STREAM.to_owned(),
            parent_native_ref: None,
            status: ItemStatus::Completed,
            content: ItemContent::AgentMessage(AgentMessagePayload {
                text: ANSWERED.to_owned(),
                text_len: i64::try_from(ANSWERED.chars().count()).unwrap(),
                phase: None,
            }),
            at: None,
        }),
    ]
}

#[tokio::test]
async fn what_a_session_held_before_is_written_once_in_no_turn_and_marked_as_before_orch() {
    let world = World::new().await;
    let thread = world.thread("Adopted").await;
    let segment = world.segment_of(thread).await;
    let engine = world.engine();

    let first = engine
        .take_in_record(thread, segment, Some(world.ada), &record())
        .await
        .unwrap();
    assert_eq!((first.items, first.kept), (2, 0));
    let again = engine
        .take_in_record(thread, segment, Some(world.ada), &record())
        .await
        .unwrap();
    assert_eq!(
        (again.items, again.kept),
        (0, 2),
        "the same entries are not written again"
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND before_orch",
                thread
            )
            .await,
        2
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND before_orch AND turn_id IS NOT NULL",
                thread
            )
            .await,
        0,
        "they belong to no turn of the thread"
    );

    // A reader sees them in their order, the person's words as the harness's user's, each marked.
    let sync = world.sync(thread).await;
    let before: Vec<_> = sync.items.iter().filter(|item| item.before_orch).collect();
    assert_eq!(before.len(), 2);
    assert_eq!(
        before[0].author.as_ref().map(|author| author.id),
        Some(world.ada)
    );
    assert!(serde_json::to_string(&before[0]).unwrap().contains(ASKED));
    assert!(
        serde_json::to_string(&before[1])
            .unwrap()
            .contains(ANSWERED)
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let ids: Vec<_> = before.iter().map(|item| item.id).collect();
    let selected = orch_db::queries::shares::items_by_ids(&mut conn, thread, &ids)
        .await
        .unwrap();
    assert_eq!(selected.len(), 2);
    assert!(
        selected
            .iter()
            .all(|item| item.before_orch && item.turn_id.is_none()),
        "forwarding retains the original session boundary"
    );
    let band = orch_db::queries::shares::items_between(&mut conn, thread, 0, i64::MAX)
        .await
        .unwrap();
    assert_eq!(
        band.iter()
            .filter(|item| item.before_orch)
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        ids,
        "the exported history keeps both original items in order"
    );
    drop(conn);
    assert!(
        sync.items
            .iter()
            .filter(|item| !item.before_orch)
            .all(|item| !serde_json::to_string(item).unwrap().contains(ASKED)),
        "only the items from before say what was said before"
    );
}

#[tokio::test]
async fn what_a_session_held_before_is_never_sent_to_the_agent_again() {
    let world = World::new().await;
    let thread = world.thread("Adopted and driven").await;
    let segment = world.segment_of(thread).await;
    let engine = world.engine();
    engine
        .take_in_record(thread, segment, Some(world.ada), &record())
        .await
        .unwrap();

    engine
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("what was the word?"),
        )
        .await
        .unwrap();
    let start = world.run_turn(thread, 0, "banana").await;
    assert!(
        start.text.contains("what was the word?"),
        "the turn is given its own input: {}",
        start.text
    );
    let sent: Vec<Command> = world
        .ordered()
        .into_iter()
        .map(|(_, command)| command)
        .collect();
    for command in &sent {
        let written = format!("{command:?}");
        assert!(
            !written.contains(ASKED) && !written.contains(ANSWERED),
            "nothing the machine is asked holds what was said before: {written}"
        );
        if let Command::SessionOpen(open) = command {
            assert!(
                open.history.is_none() && open.archive.is_none(),
                "the session is not given a history to import"
            );
        }
    }
    // The turn's own words are in its turn; those from before stay where they were.
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND before_orch AND turn_id IS NULL",
                thread
            )
            .await,
        2
    );
}
