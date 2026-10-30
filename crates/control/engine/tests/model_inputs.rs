//! What the agent is given is kept before it is sent, block by block and exactly as it is sent: the text a session is
//! opened with, each turn's input with what goes before it, and each steer. Once the harness takes it, the record
//! says so.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, eventually, queued, steering};
use orch_authz::Caller;
use orch_db::queries::model_inputs::{self, ModelInputFilter, ModelInputRow};
use orch_link_contract::commands::{Command, CommandKind};
use orch_types::Id;

/// Every record of a thread, the newest first.
async fn records(world: &World, thread: Id) -> Vec<ModelInputRow> {
    let mut conn = world.pool.acquire().await.unwrap();
    model_inputs::list(
        &mut conn,
        thread,
        &ModelInputFilter {
            id: None,
            turn: None,
            before: None,
            after: None,
            limit: 50,
        },
    )
    .await
    .unwrap()
}

/// The text a record's blocks make, as it was sent.
fn joined(record: &ModelInputRow) -> String {
    record
        .blocks
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["text"].as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The keys of a record's blocks, in their order.
fn keys(record: &ModelInputRow) -> Vec<&str> {
    record
        .blocks
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["key"].as_str().unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_a_session_and_a_turn_are_given_is_kept_as_it_was_sent() {
    let world = World::new().await;
    let thread = world.thread("Kept").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Fix the login"))
        .await
        .unwrap();
    let start = world.run_turn(thread, 0, "Done").await;
    let Command::SessionOpen(open) = world.seen.of_kind(CommandKind::SessionOpen).remove(0) else {
        panic!("the session's opening")
    };
    eventually(
        "the opening, catalogue and input records settled",
        || async {
            let kept = records(&world, thread).await;
            kept.len() == 3 && kept.iter().all(|record| record.state == "sent")
        },
    )
    .await;
    let kept = records(&world, thread).await;
    // The newest first: the turn's input, then what the session was opened with.
    let input = kept
        .iter()
        .find(|record| record.purpose == "input")
        .unwrap();
    let session = kept
        .iter()
        .find(|record| record.purpose == "session")
        .unwrap();
    let tools = kept.iter().find(|record| record.purpose == "tool").unwrap();
    assert_eq!(session.purpose, "session");
    assert_eq!(keys(session)[0], "preamble");
    assert_eq!(joined(session), open.instructions);
    assert!(session.sent_at.is_some());
    assert_eq!(input.purpose, "input");
    assert_eq!(input.reference, start.turn_id.to_string());
    assert_eq!(input.turn_id, Some(start.turn_id));
    assert_eq!(keys(input).first(), Some(&"where_you_are"));
    assert_eq!(keys(input).last(), Some(&"input"));
    let words = input.blocks.as_array().unwrap().last().unwrap()["text"].clone();
    assert_eq!(words, "Fix the login");
    assert_eq!(joined(input), start.text);
    assert!(input.seq_created > session.seq_created);
    let catalogue: Vec<serde_json::Value> = tools
        .blocks
        .as_array()
        .unwrap()
        .iter()
        .map(|block| serde_json::from_str(block["text"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        catalogue
            .iter()
            .map(|definition| definition["name"].clone())
            .collect::<Vec<_>>(),
        open.orch_tools
            .iter()
            .map(|tool| serde_json::to_value(tool).unwrap())
            .collect::<Vec<_>>()
    );
    assert!(
        catalogue
            .iter()
            .all(|definition| definition["description"].is_string()
                && definition["inputSchema"].is_object())
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let linked: Option<Id> = sqlx::query_scalar("SELECT model_input_id FROM items WHERE thread_id=$1 AND kind='user_message' AND payload->>'input_id'=$2::text")
        .bind(thread).bind(input.input_id).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        linked,
        Some(input.id),
        "a person's message opens the exact delivered record"
    );
    for record in [input, session] {
        let marker: (Option<Id>, serde_json::Value) =
            sqlx::query_as("SELECT model_input_id,payload FROM items WHERE id=$1")
                .bind(record.item_id.unwrap())
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(marker.0, Some(record.id));
        assert_eq!(marker.1["marker"], "orch_context");
        assert_eq!(
            marker.1["data"]["model_input_id"],
            serde_json::json!(record.id)
        );
        let shared =
            orch_db::queries::shares::items_by_ids(&mut conn, thread, &[record.item_id.unwrap()])
                .await
                .unwrap();
        assert_eq!(shared.len(), 1);
        assert_eq!(
            shared[0].model_input_id,
            Some(record.id),
            "a shared or exported context marker retains its exact input record"
        );
    }
    let shared_turn = orch_db::queries::shares::items_of_turns(&mut conn, thread, &[start.turn_id])
        .await
        .unwrap();
    assert!(
        shared_turn
            .iter()
            .any(|item| item.model_input_id == Some(input.id)),
        "a shared conversation retains the delivered message's input record"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_is_kept_under_its_input_with_what_went_with_it() {
    let world = World::new().await;
    let thread = world.thread("Steered").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let submitted = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            steering("Also check the logout"),
        )
        .await
        .unwrap();
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    let Command::TurnSteer(delivery) = world.seen.of_kind(CommandKind::TurnSteer).remove(0) else {
        panic!("the steer")
    };
    let reference = submitted.input.id.to_string();
    eventually("the steer's record settled", || {
        let reference = reference.clone();
        let world = &world;
        async move {
            records(world, thread)
                .await
                .iter()
                .any(|record| record.reference == reference && record.state == "sent")
        }
    })
    .await;
    let kept = records(&world, thread).await;
    let steer = kept
        .iter()
        .find(|record| record.reference == reference)
        .unwrap();
    assert_eq!(steer.purpose, "input");
    assert_eq!(steer.turn_id, Some(start.turn_id));
    assert_eq!(steer.input_id, Some(submitted.input.id));
    assert_eq!(keys(steer).last(), Some(&"input"));
    assert_eq!(joined(steer), delivery.text);
    let linked: Option<Id> = sqlx::query_scalar("SELECT model_input_id FROM items WHERE thread_id=$1 AND kind='user_message' AND payload->>'input_id'=$2::text")
        .bind(thread).bind(submitted.input.id).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(linked, Some(steer.id));
}
