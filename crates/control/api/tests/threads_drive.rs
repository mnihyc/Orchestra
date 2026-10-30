//! A thread is driven over HTTP: an input queued, steered into the turn that runs or put first with that turn stopped, edited by its author, put in order and cancelled by anybody who drives, a turn stopped, a limit let past, a failed turn tried again; each needing the right to drive the thread and the sender's own use of its provider.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{
    ContentPart, Delivery, InputState, LimitBypassRequest, ThreadInput, ThreadInputBypass,
    ThreadInputCancel, ThreadInputUpdate, ThreadInputUpdateBody, ThreadInterrupt,
    ThreadInterruptBody, ThreadLimitsGet, ThreadLimitsGetQuery, ThreadQueueResume,
    ThreadQueueResumeBody, ThreadSync, ThreadSyncGet, ThreadSyncGetQuery, ThreadTurnRetry,
    ThreadTurnRetryBody, TurnState,
};
use orch_link_contract::commands::CommandKind;
use orch_link_contract::events::TurnOutcome;
use orch_types::{Id, new_id};
use places::{Who, missing_permissions};
use rig::{Scene, message_began, message_grew, turn_began, turn_ended};
use serde_json::json;

fn text(words: &str) -> Vec<ContentPart> {
    vec![ContentPart::Text {
        text: words.to_owned(),
    }]
}

#[tokio::test]
async fn an_unaccepted_steer_can_go_back_to_its_place_but_a_taken_steer_cannot() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    running(&s, thread, 0, "Work").await;
    s.rig.engine.stop();
    let first = s
        .say(&s.ada, thread, "Before", Delivery::Queue, "before-return")
        .await
        .ok()
        .input;
    let steer = s
        .say(
            &s.ada,
            thread,
            "Wait instead",
            Delivery::Steer,
            "return-steer",
        )
        .await
        .ok()
        .input;
    let last = s
        .say(&s.ada, thread, "After", Delivery::Queue, "after-return")
        .await
        .ok()
        .input;
    assert_eq!(steer.state, InputState::Delivering);
    let returned = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &steer.id])
        .token(&s.ada.token)
        .raw_body(&json!({"delivery":"queue"}))
        .send()
        .await
        .ok();
    assert_eq!(returned.input.state, InputState::Pending);
    assert_eq!(returned.input.position, steer.position);
    assert_eq!(
        open_inputs(&s, thread).await,
        ["Before", "Wait instead", "After"]
    );
    assert_ne!(first.id, last.id);
    assert!(s.rig.seen.of_kind(CommandKind::TurnSteer).is_empty());

    let taken = Scene::new().await;
    let thread = taken.thread().await.base.id;
    running(&taken, thread, 0, "Work").await;
    let steer = taken
        .say(
            &taken.ada,
            thread,
            "Take it",
            Delivery::Steer,
            "taken-steer",
        )
        .await
        .ok()
        .input;
    rig::eventually("the program takes the steer", || async {
        synced(&taken, thread, Some(0))
            .await
            .inputs
            .iter()
            .any(|input| input.id == steer.id && input.state == InputState::Delivered)
    })
    .await;
    let refused = taken
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &steer.id])
        .token(&taken.ada.token)
        .raw_body(&json!({"delivery":"queue"}))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::InputNotPending);
}

/// The inputs a thread holds that are open, in the order they will be delivered.
async fn open_inputs(s: &Scene, thread: Id) -> Vec<String> {
    let synced = s
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    let mut inputs: Vec<_> = synced
        .inputs
        .into_iter()
        .filter(|input| input.state == InputState::Pending)
        .collect();
    inputs.sort_by(|a, b| {
        a.position
            .len()
            .cmp(&b.position.len())
            .then(a.position.cmp(&b.position))
    });
    inputs
        .iter()
        .map(|input| match &input.content[0] {
            ContentPart::Text { text } => text.clone(),
            other => format!("{other:?}"),
        })
        .collect()
}

/// A thread whose turn cannot start, so that what is sent stays in the queue: its model is not one the
/// provider offers.
async fn stuck(s: &Scene) -> Id {
    let id = s.thread().await.base.id;
    sqlx::query("UPDATE provider_models SET enabled = false WHERE provider_id = $1")
        .bind(s.on.provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    id
}

/// A turn that runs: Ada sends `words`, the machine takes the turn up and the agent has begun to answer.
async fn running(
    s: &Scene,
    thread: Id,
    nth: usize,
    words: &str,
) -> orch_link_contract::commands::TurnStart {
    let sent = s
        .say(
            &s.ada,
            thread,
            words,
            Delivery::Queue,
            &format!("run-{nth}"),
        )
        .await;
    assert!(sent.status.is_success(), "{:?}", sent.body);
    let start = s.rig.turn_start(thread, nth).await;
    s.rig.say(thread, turn_began(start.turn_id)).await;
    s.rig
        .say(thread, message_began(start.turn_id, "working"))
        .await;
    s.rig
        .say(thread, message_grew("working", 0, "Looking"))
        .await;
    s.rig.wait_turn(start.turn_id, "running").await;
    start
}

#[tokio::test]
async fn an_input_is_queued_and_a_repeated_client_value_answers_the_input_that_stands() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let first = s
        .say(&s.ada, id, "Look into it", Delivery::Queue, "same")
        .await;
    assert_eq!(first.status, 201);
    let first = first.ok();
    let again = s
        .say(&s.ada, id, "Look into it", Delivery::Queue, "same")
        .await;
    assert_eq!(again.status, 200, "the same sender and value answer 200");
    assert_eq!(again.ok().input.id, first.input.id);
    // Another sender's value is their own.
    let theirs = s.say(&s.eve, id, "Me too", Delivery::Queue, "same").await;
    assert_eq!(theirs.status, 201);
    assert_ne!(theirs.ok().input.id, first.input.id);
    // The first input of an idle thread starts a turn at once; the other waits behind it.
    s.rig.turn_start(id, 0).await;
}

#[tokio::test]
async fn an_input_that_says_nothing_or_too_much_is_refused() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let empty = s.say(&s.ada, id, "", Delivery::Queue, "e").await.problem();
    assert_eq!(empty.code, ErrorCode::RequestInvalid);
    let long = "x".repeat(100_001);
    let huge = s
        .say(&s.ada, id, &long, Delivery::Queue, "l")
        .await
        .problem();
    assert_eq!(huge.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_steer_goes_into_the_turn_that_runs_and_an_interrupt_stops_it_and_goes_first() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let start = running(&s, id, 0, "Begin").await;
    let steer = s
        .say(
            &s.eve,
            id,
            "Also check the cookie",
            Delivery::Steer,
            "steer",
        )
        .await;
    assert_eq!(steer.status, 201);
    assert!(matches!(
        steer.ok().input.state,
        InputState::Delivering | InputState::Delivered
    ));
    eventually_seen(&s, CommandKind::TurnSteer).await;
    // An interrupt stops the turn and puts its input first.
    let interrupting = s
        .say(
            &s.ada,
            id,
            "Stop, do this instead",
            Delivery::Interrupt,
            "stop",
        )
        .await;
    assert_eq!(interrupting.status, 201);
    eventually_seen(&s, CommandKind::TurnInterrupt).await;
    assert_eq!(
        s.rig.turn_starts(id).first().map(|start| start.turn_id),
        Some(start.turn_id)
    );
}

async fn eventually_seen(s: &Scene, kind: CommandKind) {
    rig::eventually(&format!("the machine ran a {kind}"), || {
        let seen = !s.rig.seen.of_kind(kind).is_empty();
        async move { seen }
    })
    .await;
}

#[tokio::test]
async fn a_steer_with_no_turn_under_way_is_placed_first_in_the_queue() {
    let s = Scene::new().await;
    let id = stuck(&s).await;
    for (words, delivery, value) in [
        ("a", Delivery::Queue, "a"),
        ("b", Delivery::Queue, "b"),
        ("c", Delivery::Steer, "c"),
    ] {
        s.say(&s.ada, id, words, delivery, value).await.ok();
    }
    assert_eq!(open_inputs(&s, id).await, ["c", "a", "b"]);
}

async fn synced(s: &Scene, thread: Id, since: Option<i64>) -> ThreadSync {
    s.rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadSyncGetQuery {
            since,
            ..ThreadSyncGetQuery::default()
        })
        .send()
        .await
        .ok()
}

async fn change_delivery(
    s: &Scene,
    who: &Who,
    thread: Id,
    input: Id,
    delivery: Delivery,
) -> ThreadInput {
    s.rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &input])
        .token(&who.token)
        .body(&ThreadInputUpdateBody {
            delivery: Some(delivery),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .ok()
        .input
}

#[tokio::test]
async fn a_move_answers_with_every_renumbered_neighbor_as_the_live_delta_does() {
    let s = Scene::new().await;
    let thread = stuck(&s).await;
    let one = s
        .say(&s.ada, thread, "One", Delivery::Queue, "move-one")
        .await
        .ok()
        .input;
    let two = s
        .say(&s.ada, thread, "Two", Delivery::Queue, "move-two")
        .await
        .ok()
        .input;
    let three = s
        .say(&s.ada, thread, "Three", Delivery::Queue, "move-three")
        .await
        .ok()
        .input;
    let before = synced(&s, thread, None).await;
    let mut displayed: std::collections::BTreeMap<_, _> = before
        .inputs
        .into_iter()
        .map(|input| (input.id, input))
        .collect();
    let moved = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &two.id])
        .token(&s.dan.token)
        .body(&ThreadInputUpdateBody {
            after_input_id: Some(None),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(moved.input.id, two.id);
    assert_eq!(moved.input.position, "1");
    assert_eq!(moved.input.author, two.author);
    assert_eq!(moved.input.content, two.content);
    assert_eq!(moved.input.delivery, two.delivery);
    assert_eq!(moved.input.queue_held, two.queue_held);
    assert_eq!(
        moved.inputs.len(),
        2,
        "both rows that swapped positions, no unchanged third row"
    );
    assert!(
        moved
            .inputs
            .iter()
            .all(|input| input.seq_updated > before.seq)
    );
    assert_eq!(
        moved
            .inputs
            .iter()
            .find(|input| input.id == one.id)
            .unwrap()
            .position,
        "2"
    );
    // Use only the write's answer, with the very cache a person already has: no full reload.
    for input in &moved.inputs {
        displayed.insert(input.id, input.clone());
    }
    let mut order: Vec<_> = displayed.values().collect();
    order.sort_by_key(|input| input.position.parse::<u64>().unwrap());
    assert_eq!(
        order.iter().map(|input| input.id).collect::<Vec<_>>(),
        [two.id, one.id, three.id]
    );
    let frame = synced(&s, thread, Some(before.seq)).await;
    assert!(frame.complete);
    assert_eq!(
        frame.inputs, moved.inputs,
        "incremental sync and the socket's shared reader carry the same patch"
    );

    let back = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &two.id])
        .token(&s.dan.token)
        .body(&ThreadInputUpdateBody {
            after_input_id: Some(Some(three.id)),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        back.inputs.len(),
        3,
        "moving across two rows returns both changed neighbors"
    );
    assert!(
        back.inputs
            .iter()
            .all(|input| input.seq_updated > frame.seq)
    );
    for input in &back.inputs {
        displayed.insert(input.id, input.clone());
    }
    let mut order: Vec<_> = displayed.values().collect();
    order.sort_by_key(|input| input.position.parse::<u64>().unwrap());
    assert_eq!(
        order.iter().map(|input| input.id).collect::<Vec<_>>(),
        [one.id, three.id, two.id]
    );
    assert_eq!(
        synced(&s, thread, Some(frame.seq)).await.inputs,
        back.inputs
    );
}

#[tokio::test]
async fn changing_a_pending_input_to_steer_joins_the_running_turn_with_the_same_author_and_input() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let start = running(&s, thread, 0, "Start").await;
    let later = s
        .say(&s.ada, thread, "Later", Delivery::Queue, "later")
        .await
        .ok()
        .input;
    let waiting = s
        .say(
            &s.ada,
            thread,
            "Check the cookie now",
            Delivery::Queue,
            "now",
        )
        .await
        .ok()
        .input;
    let before = synced(&s, thread, None).await;

    let queued = change_delivery(&s, &s.dan, thread, later.id, Delivery::Queue).await;
    assert_eq!(queued.state, InputState::Pending);
    assert_eq!(queued.delivery, Delivery::Queue);
    assert_eq!(
        open_inputs(&s, thread).await,
        ["Check the cookie now", "Later"]
    );
    assert!(s.rig.seen.of_kind(CommandKind::TurnSteer).is_empty());
    assert!(s.rig.seen.of_kind(CommandKind::TurnInterrupt).is_empty());

    // Dan drives, but cannot use this provider himself. He changes delivery, not authorship.
    let changed = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &waiting.id])
        .token(&s.dan.token)
        .body(&ThreadInputUpdateBody {
            delivery: Some(Delivery::Steer),
            after_input_id: Some(None),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .ok()
        .input;
    assert_eq!(changed.id, waiting.id);
    assert_eq!(changed.author, waiting.author);
    assert_eq!(changed.delivery, Delivery::Steer);
    assert_eq!(changed.turn_id, Some(start.turn_id));
    assert!(matches!(
        changed.state,
        InputState::Delivering | InputState::Delivered
    ));
    eventually_seen(&s, CommandKind::TurnSteer).await;
    let commands = s.rig.seen.of_kind(CommandKind::TurnSteer);
    let orch_link_contract::commands::Command::TurnSteer(body) = &commands[0] else {
        panic!("the machine receives a steer")
    };
    assert_eq!(body.turn_id, start.turn_id);
    assert!(body.text.contains("Check the cookie now"));
    assert_eq!(s.rig.turn_starts(thread).len(), 1);

    let after = synced(&s, thread, Some(before.seq)).await;
    assert!(after.thread.queue.revision > before.thread.queue.revision);
    let row = after
        .inputs
        .iter()
        .find(|input| input.id == waiting.id)
        .unwrap();
    assert_eq!(row.delivery, Delivery::Steer);
    assert!(
        row.seq_updated > before.seq,
        "a reconnect receives the updated input"
    );
    assert_eq!(open_inputs(&s, thread).await, ["Later"]);
}

#[tokio::test]
async fn changing_a_pending_input_to_interrupt_stops_the_turn_and_puts_that_input_first() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let start = running(&s, thread, 0, "Start").await;
    let older = s
        .say(
            &s.ada,
            thread,
            "Older queued work",
            Delivery::Queue,
            "older",
        )
        .await
        .ok()
        .input;
    let waiting = s
        .say(
            &s.ada,
            thread,
            "Do this instead",
            Delivery::Queue,
            "instead",
        )
        .await
        .ok()
        .input;
    let before = synced(&s, thread, None).await;
    let changed = change_delivery(&s, &s.dan, thread, waiting.id, Delivery::Interrupt).await;
    assert_eq!(changed.id, waiting.id);
    assert_eq!(changed.author, waiting.author);
    assert_eq!(changed.delivery, Delivery::Interrupt);
    eventually_seen(&s, CommandKind::TurnInterrupt).await;
    let after = synced(&s, thread, None).await;
    let older = after
        .inputs
        .iter()
        .find(|input| input.id == older.id)
        .unwrap();
    assert!(
        changed.position.parse::<i64>().unwrap() < older.position.parse::<i64>().unwrap(),
        "the interrupting input stands first even if the forced stop already started its turn"
    );
    assert!(after.thread.queue.revision > before.thread.queue.revision);
    assert!(
        !after.thread.queue.held,
        "changing delivery is not a Stop of the waiting queue"
    );
    assert_eq!(
        after
            .turns
            .iter()
            .find(|turn| turn.id == start.turn_id)
            .unwrap()
            .interrupted_by
            .as_ref()
            .map(|person| person.id),
        Some(s.dan.id())
    );
    s.rig
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    let next = s.rig.turn_start(thread, 1).await;
    assert!(next.text.contains("Do this instead"));
    assert!(!next.text.contains("Older queued work"));
}

#[tokio::test]
async fn steering_a_queued_input_rechecks_its_authors_use_instead_of_lending_the_driver_its_own() {
    use orch_api_contract::access::{Holder, PageChange, PermissionPageGet, PermissionPageUpdate};
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    running(&s, thread, 0, "Start").await;
    let waiting = s
        .say(&s.eve, thread, "My work", Delivery::Queue, "eves-work")
        .await
        .ok()
        .input;
    let page = s
        .rig
        .world
        .app
        .call(PermissionPageGet, &[&"providers", &s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let use_row = page
        .rows
        .iter()
        .find(|row| row.permission == "provider.use")
        .unwrap();
    let removed: Vec<Id> = use_row
        .entries
        .iter()
        .filter(|entry| matches!(&entry.holder, Holder::User { user } if user.id == s.eve.id()))
        .map(|entry| entry.id)
        .collect();
    assert!(!removed.is_empty());
    let change: PageChange = serde_json::from_value(json!({
        "expected_revision": page.revision,
        "rows": [{ "permission": "provider.use", "remove": removed }]
    }))
    .unwrap();
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"providers", &s.on.provider])
        .token(&s.ada.token)
        .body(&change)
        .send()
        .await
        .ok();
    let before = synced(&s, thread, None).await;
    // Ada owns the provider; that does not let her turn Eve's revoked work into Ada's.
    let refused = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &waiting.id])
        .token(&s.ada.token)
        .body(&ThreadInputUpdateBody {
            delivery: Some(Delivery::Steer),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(missing_permissions(&refused).contains(&"provider.use".to_owned()));
    let after = synced(&s, thread, None).await;
    let input = after
        .inputs
        .iter()
        .find(|input| input.id == waiting.id)
        .unwrap();
    assert_eq!(input.delivery, Delivery::Queue);
    assert_eq!(input.author, waiting.author);
    assert_eq!(after.thread.queue.revision, before.thread.queue.revision);
    assert!(s.rig.seen.of_kind(CommandKind::TurnSteer).is_empty());
}

#[tokio::test]
async fn with_no_running_turn_steer_and_interrupt_go_first_and_queue_goes_last() {
    let s = Scene::new().await;
    let thread = stuck(&s).await;
    // Steering support is needed only to put words into a running turn.
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(descriptor, '{capabilities,steer}', 'false') WHERE id = $1")
        .bind(s.on.agent).execute(s.rig.db.pool()).await.unwrap();
    s.say(&s.ada, thread, "a", Delivery::Queue, "a").await.ok();
    let b = s
        .say(&s.ada, thread, "b", Delivery::Queue, "b")
        .await
        .ok()
        .input;
    let c = s
        .say(&s.ada, thread, "c", Delivery::Queue, "c")
        .await
        .ok()
        .input;
    let mut before = synced(&s, thread, None).await;
    for (input, delivery, expected) in [
        (c.id, Delivery::Steer, ["c", "a", "b"]),
        (b.id, Delivery::Interrupt, ["b", "c", "a"]),
        (b.id, Delivery::Queue, ["c", "a", "b"]),
    ] {
        let changed = change_delivery(&s, &s.dan, thread, input, delivery).await;
        assert_eq!(changed.id, input);
        assert_eq!(changed.delivery, delivery);
        assert_eq!(changed.state, InputState::Pending);
        assert_eq!(changed.turn_id, None);
        assert_eq!(open_inputs(&s, thread).await, expected);
        let after = synced(&s, thread, Some(before.seq)).await;
        assert!(after.thread.queue.revision > before.thread.queue.revision);
        assert!(
            after
                .inputs
                .iter()
                .any(|row| row.id == input && row.delivery == delivery)
        );
        before = after;
    }
    assert!(s.rig.seen.of_kind(CommandKind::TurnSteer).is_empty());
    assert!(s.rig.seen.of_kind(CommandKind::TurnInterrupt).is_empty());
    assert!(s.rig.turn_starts(thread).is_empty());
}

#[tokio::test]
async fn an_unsupported_steer_names_why_offers_interrupt_and_changes_nothing() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    running(&s, thread, 0, "Start").await;
    s.say(&s.ada, thread, "First waiting", Delivery::Queue, "first")
        .await
        .ok();
    let waiting = s
        .say(
            &s.ada,
            thread,
            "Keep these words",
            Delivery::Queue,
            "waiting",
        )
        .await
        .ok()
        .input;
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(descriptor, '{capabilities,steer}', 'false') WHERE id = $1")
        .bind(s.on.agent).execute(s.rig.db.pool()).await.unwrap();
    let before = synced(&s, thread, None).await;
    let refused = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &waiting.id])
        .token(&s.ada.token)
        .body(&ThreadInputUpdateBody {
            content: Some(text("These words must roll back too")),
            delivery: Some(Delivery::Steer),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::CapabilityUnsupported);
    assert_eq!(refused.extra["capability"], "steer");
    assert_eq!(refused.extra["fallback_delivery"], "interrupt");
    // The sentence is the catalogue's for the code; what is missing and the way on are the members above.
    assert_eq!(
        refused.detail,
        orch_api_contract::messages::for_code(ErrorCode::CapabilityUnsupported)
    );
    let after = synced(&s, thread, None).await;
    assert_eq!(after.thread.queue.revision, before.thread.queue.revision);
    let unchanged = after
        .inputs
        .iter()
        .find(|input| input.id == waiting.id)
        .unwrap();
    assert_eq!(unchanged.delivery, Delivery::Queue);
    assert_eq!(unchanged.content, waiting.content);
    assert_eq!(unchanged.seq_updated, waiting.seq_updated);
    assert_eq!(
        open_inputs(&s, thread).await,
        ["First waiting", "Keep these words"]
    );
    assert!(s.rig.seen.of_kind(CommandKind::TurnSteer).is_empty());
    change_delivery(&s, &s.dan, thread, waiting.id, Delivery::Interrupt).await;
    eventually_seen(&s, CommandKind::TurnInterrupt).await;
}

#[tokio::test]
async fn delivery_changes_need_drive_permission_and_an_input_that_is_still_pending() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    running(&s, thread, 0, "Start").await;
    let waiting = s
        .say(&s.ada, thread, "Wait", Delivery::Queue, "wait")
        .await
        .ok()
        .input;
    // A first read carries the open queue; what was delivered is among the changes since the start.
    let before = synced(&s, thread, Some(0)).await;
    let delivered = before
        .inputs
        .iter()
        .find(|input| input.state != InputState::Pending)
        .unwrap();
    for delivery in [Delivery::Queue, Delivery::Steer, Delivery::Interrupt] {
        for who in [&s.cai, &s.bea] {
            let refused = s
                .rig
                .world
                .app
                .call(ThreadInputUpdate, &[&thread, &waiting.id])
                .token(&who.token)
                .body(&ThreadInputUpdateBody {
                    delivery: Some(delivery),
                    ..ThreadInputUpdateBody::default()
                })
                .send()
                .await
                .problem();
            assert_eq!(refused.code, ErrorCode::AuthzDenied);
            assert!(missing_permissions(&refused).contains(&"thread.interact".to_owned()));
        }
        let late = s
            .rig
            .world
            .app
            .call(ThreadInputUpdate, &[&thread, &delivered.id])
            .token(&s.ada.token)
            .body(&ThreadInputUpdateBody {
                delivery: Some(delivery),
                ..ThreadInputUpdateBody::default()
            })
            .send()
            .await
            .problem();
        assert_eq!(late.code, ErrorCode::InputNotPending);
        assert_ne!(late.extra["state"], "pending");
    }
    let conflict = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &waiting.id])
        .token(&s.ada.token)
        .body(&ThreadInputUpdateBody {
            delivery: Some(Delivery::Interrupt),
            after_input_id: Some(Some(delivered.id)),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(conflict.code, ErrorCode::RequestInvalid);
    assert_eq!(
        synced(&s, thread, None).await.thread.queue.revision,
        before.thread.queue.revision
    );
    assert!(s.rig.seen.of_kind(CommandKind::TurnSteer).is_empty());
    assert!(s.rig.seen.of_kind(CommandKind::TurnInterrupt).is_empty());
}

#[tokio::test]
async fn a_blocked_input_or_one_held_for_its_authors_cap_can_be_edited_moved_and_removed() {
    for own_cap in [false, true] {
        let s = Scene::new().await;
        let thread = if own_cap {
            let thread = s.thread().await.base.id;
            sqlx::query("INSERT INTO usage_caps (id, node_id, subject_kind, subject_id, split, meter, amount, at_limit, created_by)
                VALUES ($1, $2, 'user', $3, 'shared', 'running', 0, 'stop', $3)")
                .bind(new_id()).bind(s.tree.topic.base.id).bind(s.ada.id())
                .execute(s.rig.db.pool()).await.unwrap();
            thread
        } else {
            stuck(&s).await
        };
        let first = s
            .say(&s.ada, thread, "First held", Delivery::Queue, "first")
            .await
            .ok()
            .input;
        let other = s
            .say(&s.ada, thread, "Other held", Delivery::Queue, "other")
            .await
            .ok()
            .input;
        assert_eq!(s.rig.state(thread).await.0, "blocked");
        if own_cap {
            assert_eq!(first.state_detail.as_deref(), Some("cap_reached"));
        }
        let before = synced(&s, thread, None).await;
        let edited = s
            .rig
            .world
            .app
            .call(ThreadInputUpdate, &[&thread, &first.id])
            .token(&s.ada.token)
            .body(&ThreadInputUpdateBody {
                content: Some(text("Edited while held")),
                ..ThreadInputUpdateBody::default()
            })
            .send()
            .await
            .ok()
            .input;
        assert_eq!(edited.content, text("Edited while held"));
        assert_eq!(edited.state, InputState::Pending);
        let changed = change_delivery(&s, &s.dan, thread, first.id, Delivery::Steer).await;
        assert_eq!(changed.state, InputState::Pending);
        if own_cap {
            assert_eq!(changed.state_detail.as_deref(), Some("cap_reached"));
        }
        let before_move = synced(&s, thread, None).await;
        assert!(before_move.thread.queue.revision > before.thread.queue.revision);
        s.rig
            .world
            .app
            .call(ThreadInputUpdate, &[&thread, &first.id])
            .token(&s.dan.token)
            .body(&ThreadInputUpdateBody {
                after_input_id: Some(Some(other.id)),
                ..ThreadInputUpdateBody::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(
            open_inputs(&s, thread).await,
            ["Other held", "Edited while held"]
        );
        let before_remove = synced(&s, thread, None).await;
        assert!(before_remove.thread.queue.revision > before_move.thread.queue.revision);
        let removed = s
            .rig
            .world
            .app
            .call(ThreadInputCancel, &[&thread, &first.id])
            .token(&s.dan.token)
            .send()
            .await
            .ok();
        assert_eq!(removed.state, InputState::Cancelled);
        assert_eq!(open_inputs(&s, thread).await, ["Other held"]);
        assert!(
            synced(&s, thread, None).await.thread.queue.revision
                > before_remove.thread.queue.revision
        );
        assert!(s.rig.turn_starts(thread).is_empty());
    }
}

#[tokio::test]
async fn stopped_inputs_keep_the_hold_through_edits_delivery_changes_and_reordering_and_can_be_removed()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let start = running(&s, thread, 0, "Start").await;
    let one = s
        .say(&s.ada, thread, "One", Delivery::Queue, "one")
        .await
        .ok()
        .input;
    let two = s
        .say(&s.ada, thread, "Two", Delivery::Queue, "two")
        .await
        .ok()
        .input;
    s.rig
        .world
        .app
        .call(ThreadInterrupt, &[&thread])
        .token(&s.dan.token)
        .body(&ThreadInterruptBody {
            turn_id: start.turn_id,
        })
        .send()
        .await
        .ok();
    let still_stopped = change_delivery(&s, &s.dan, thread, one.id, Delivery::Steer).await;
    assert!(
        still_stopped.queue_held,
        "a delivery toggle cannot release what Stop held"
    );
    assert_eq!(still_stopped.state, InputState::Pending);
    assert!(s.rig.seen.of_kind(CommandKind::TurnSteer).is_empty());
    s.rig
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    s.rig.wait_turn(start.turn_id, "interrupted").await;
    s.rig.wait_state(thread, "idle").await;
    let held = synced(&s, thread, None).await.thread.queue;
    assert!(held.held);
    assert_eq!(held.held_inputs, 2);
    let edited = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &one.id])
        .token(&s.ada.token)
        .body(&ThreadInputUpdateBody {
            content: Some(text("One edited")),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .ok()
        .input;
    assert!(edited.queue_held);
    s.rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &one.id])
        .token(&s.dan.token)
        .body(&ThreadInputUpdateBody {
            after_input_id: Some(Some(two.id)),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(open_inputs(&s, thread).await, ["Two", "One edited"]);
    let changed = change_delivery(&s, &s.dan, thread, one.id, Delivery::Steer).await;
    assert!(changed.queue_held);
    assert_eq!(changed.state, InputState::Pending);
    assert_eq!(open_inputs(&s, thread).await, ["One edited", "Two"]);
    let current = synced(&s, thread, None).await.thread.queue;
    assert!(current.held);
    assert!(current.revision > held.revision);
    let stale = s
        .rig
        .world
        .app
        .call(ThreadQueueResume, &[&thread])
        .token(&s.dan.token)
        .body(&ThreadQueueResumeBody {
            revision: held.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current_revision"], current.revision);
    for input in [one.id, two.id] {
        assert_eq!(
            s.rig
                .world
                .app
                .call(ThreadInputCancel, &[&thread, &input])
                .token(&s.dan.token)
                .send()
                .await
                .ok()
                .state,
            InputState::Cancelled
        );
    }
    let empty = synced(&s, thread, None).await;
    assert!(!empty.thread.queue.held);
    assert_eq!(empty.thread.queue.held_inputs, 0);
    assert!(empty.thread.queue.revision > current.revision);
    assert!(open_inputs(&s, thread).await.is_empty());
    assert_eq!(s.rig.turn_starts(thread).len(), 1);
    assert!(s.rig.seen.of_kind(CommandKind::TurnSteer).is_empty());
}

#[tokio::test]
async fn messages_sent_after_stop_join_the_held_queue_and_edit_reorder_remove_resume_keep_the_seen_order()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let first = running(&s, thread, 0, "Start").await;
    let gamma = s
        .say(&s.ada, thread, "Gamma", Delivery::Queue, "gamma")
        .await
        .ok()
        .input;
    s.rig
        .world
        .app
        .call(ThreadInterrupt, &[&thread])
        .token(&s.ada.token)
        .body(&ThreadInterruptBody {
            turn_id: first.turn_id,
        })
        .send()
        .await
        .ok();
    let stopped = synced(&s, thread, None).await;
    assert!(stopped.thread.queue.held);
    assert_eq!(stopped.thread.queue.held_inputs, 1);
    let epsilon = s
        .say(
            &s.ada,
            thread,
            "Epsilon",
            Delivery::Queue,
            "epsilon-after-stop",
        )
        .await
        .ok()
        .input;
    assert!(epsilon.queue_held);
    assert_eq!(epsilon.state, InputState::Pending);
    assert!(epsilon.turn_id.is_none());
    s.rig
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Interrupted))
        .await;
    s.rig.wait_turn(first.turn_id, "interrupted").await;
    s.rig.wait_state(thread, "idle").await;
    let zeta = s
        .say(&s.ada, thread, "Zeta", Delivery::Queue, "zeta-after-stop")
        .await
        .ok()
        .input;
    assert!(zeta.queue_held);
    assert_eq!(zeta.state, InputState::Pending);
    assert!(zeta.turn_id.is_none());
    let joined = synced(&s, thread, Some(stopped.seq)).await;
    assert!(joined.thread.queue.held);
    assert_eq!(joined.thread.queue.held_inputs, 3);
    assert_eq!(joined.thread.queue.by, stopped.thread.queue.by);
    assert_eq!(joined.thread.queue.at, stopped.thread.queue.at);
    assert!(joined.thread.queue.revision > stopped.thread.queue.revision);
    for id in [epsilon.id, zeta.id] {
        let input = joined.inputs.iter().find(|input| input.id == id).unwrap();
        assert!(input.queue_held);
        assert!(
            input.seq_updated > stopped.seq,
            "reconnecting/live readers receive the newly held row"
        );
    }
    assert_eq!(open_inputs(&s, thread).await, ["Gamma", "Epsilon", "Zeta"]);
    assert_eq!(s.rig.turn_starts(thread).len(), 1);
    let stale = s
        .rig
        .world
        .app
        .call(ThreadQueueResume, &[&thread])
        .token(&s.dan.token)
        .body(&ThreadQueueResumeBody {
            revision: stopped.thread.queue.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let edited = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &epsilon.id])
        .token(&s.ada.token)
        .body(&ThreadInputUpdateBody {
            content: Some(text("Epsilon edited")),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .ok()
        .input;
    assert!(edited.queue_held);
    s.rig
        .world
        .app
        .call(ThreadInputUpdate, &[&thread, &zeta.id])
        .token(&s.dan.token)
        .body(&ThreadInputUpdateBody {
            after_input_id: Some(Some(gamma.id)),
            ..ThreadInputUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        open_inputs(&s, thread).await,
        ["Gamma", "Zeta", "Epsilon edited"]
    );
    s.rig
        .world
        .app
        .call(ThreadInputCancel, &[&thread, &gamma.id])
        .token(&s.dan.token)
        .send()
        .await
        .ok();
    let remaining = synced(&s, thread, None).await;
    assert!(
        remaining.thread.queue.held,
        "removing the originally held input does not release later arrivals"
    );
    assert_eq!(remaining.thread.queue.held_inputs, 2);
    let repeated = s
        .say(
            &s.ada,
            thread,
            "Epsilon",
            Delivery::Queue,
            "epsilon-after-stop",
        )
        .await;
    assert_eq!(repeated.status, 200);
    let repeated = repeated.ok().input;
    assert_eq!(repeated.id, epsilon.id);
    assert_eq!(repeated.content, text("Epsilon edited"));
    assert!(repeated.queue_held);
    assert_eq!(
        synced(&s, thread, None).await.thread.queue.revision,
        remaining.thread.queue.revision
    );
    let eta = s
        .say(&s.ada, thread, "Eta", Delivery::Queue, "eta-after-stop")
        .await
        .ok()
        .input;
    assert!(eta.queue_held);
    assert_eq!(
        open_inputs(&s, thread).await,
        ["Zeta", "Epsilon edited", "Eta"]
    );
    let current = synced(&s, thread, None).await;
    let resumed = s
        .rig
        .world
        .app
        .call(ThreadQueueResume, &[&thread])
        .token(&s.dan.token)
        .body(&ThreadQueueResumeBody {
            revision: current.thread.queue.revision,
        })
        .send()
        .await
        .ok();
    assert!(!resumed.held);
    assert_eq!(resumed.held_inputs, 0);
    assert!(resumed.revision > current.thread.queue.revision);
    let mut turn_inputs = Vec::new();
    for (index, (words, input)) in [
        ("Zeta", zeta.id),
        ("Epsilon edited", epsilon.id),
        ("Eta", eta.id),
    ]
    .into_iter()
    .enumerate()
    {
        let next = s.rig.turn_start(thread, index + 1).await;
        assert!(
            next.text.ends_with(words),
            "the queued text is last after any ordinary context: {}",
            next.text
        );
        turn_inputs.push((next.turn_id, input));
        s.rig.say(thread, turn_began(next.turn_id)).await;
        s.rig
            .say(thread, turn_ended(next.turn_id, TurnOutcome::Completed))
            .await;
        s.rig.wait_turn(next.turn_id, "completed").await;
    }
    // Delivered inputs are among the changes since the start; a first read carries only the open queue.
    let finished = synced(&s, thread, Some(0)).await;
    for (turn, input) in turn_inputs {
        assert_eq!(
            finished
                .turns
                .iter()
                .find(|row| row.id == turn)
                .unwrap()
                .input_id,
            input
        );
        let input = finished.inputs.iter().find(|row| row.id == input).unwrap();
        assert_eq!(input.state, InputState::Delivered);
        assert!(!input.queue_held);
    }
    assert!(open_inputs(&s, thread).await.is_empty());
    assert_eq!(s.rig.turn_starts(thread).len(), 4);
}

#[tokio::test]
async fn a_held_authors_input_cannot_be_steered_past_their_cap_into_somebody_elses_running_turn() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    sqlx::query("INSERT INTO usage_caps (id, node_id, subject_kind, subject_id, split, meter, amount, at_limit, created_by)
        VALUES ($1, $2, 'user', $3, 'shared', 'running', 0, 'stop', $3)")
        .bind(new_id()).bind(s.tree.topic.base.id).bind(s.ada.id())
        .execute(s.rig.db.pool()).await.unwrap();
    let held = s
        .say(
            &s.ada,
            thread,
            "Ada waits for room",
            Delivery::Queue,
            "held",
        )
        .await
        .ok()
        .input;
    assert_eq!(held.state_detail.as_deref(), Some("cap_reached"));
    s.say(&s.eve, thread, "Eve may work", Delivery::Queue, "running")
        .await
        .ok();
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, turn_began(start.turn_id)).await;
    s.rig.wait_turn(start.turn_id, "running").await;
    let changed = change_delivery(&s, &s.dan, thread, held.id, Delivery::Steer).await;
    assert_eq!(changed.delivery, Delivery::Steer);
    assert_eq!(changed.state, InputState::Pending);
    assert_eq!(changed.state_detail.as_deref(), Some("cap_reached"));
    assert_eq!(changed.turn_id, None);
    assert!(s.rig.seen.of_kind(CommandKind::TurnSteer).is_empty());
    assert_eq!(s.rig.turn_starts(thread).len(), 1);
}

#[tokio::test]
async fn the_words_of_an_input_are_its_authors_alone_and_the_order_of_the_queue_is_anyones_who_drives()
 {
    let s = Scene::new().await;
    let id = stuck(&s).await;
    let a = s
        .say(&s.ada, id, "a", Delivery::Queue, "a")
        .await
        .ok()
        .input;
    s.say(&s.ada, id, "b", Delivery::Queue, "b").await.ok();
    let update = |who: &Who, body: ThreadInputUpdateBody| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadInputUpdate, &[&id, &a.id])
            .token(&who.token)
            .body(&body);
        async move { call.send().await }
    };
    // Dan cannot put words in Ada's mouth, but may put the input first, though he has no use of the provider.
    let words = ThreadInputUpdateBody {
        native_action: None,
        content: Some(text("changed")),
        delivery: None,
        after_input_id: None,
    };
    let refused = update(&s.dan, words.clone()).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["requestable"], false);
    let placed = update(
        &s.dan,
        ThreadInputUpdateBody {
            native_action: None,
            content: None,
            delivery: None,
            after_input_id: Some(None),
        },
    )
    .await
    .ok();
    assert_eq!(placed.input.id, a.id);
    update(&s.ada, words).await.ok();
    assert_eq!(open_inputs(&s, id).await, ["changed", "b"]);
    // Directly after another waiting input.
    let others = s
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&id])
        .token(&s.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .inputs;
    let b = others.iter().find(|input| input.id != a.id).unwrap();
    update(
        &s.ada,
        ThreadInputUpdateBody {
            native_action: None,
            content: None,
            delivery: None,
            after_input_id: Some(Some(b.id)),
        },
    )
    .await
    .ok();
    assert_eq!(open_inputs(&s, id).await, ["b", "changed"]);
    // Neither is possible for an input that is no longer waiting.
    let cancelled = s
        .rig
        .world
        .app
        .call(ThreadInputCancel, &[&id, &a.id])
        .token(&s.dan.token)
        .send()
        .await
        .ok();
    assert_eq!(
        cancelled.state,
        InputState::Cancelled,
        "anybody who drives cancels"
    );
    let late = update(
        &s.ada,
        ThreadInputUpdateBody {
            native_action: None,
            content: Some(text("too late")),
            delivery: None,
            after_input_id: None,
        },
    )
    .await
    .problem();
    // The refusal says that the input is not waiting, and what it is now: nobody's revision changed.
    assert_eq!(late.code, ErrorCode::InputNotPending);
    assert_eq!(late.extra["state"], "cancelled");
}

#[tokio::test]
async fn an_input_that_was_taken_up_is_refused_as_not_waiting_and_a_waiting_one_is_still_changed() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    // The first input starts the turn and is delivered; the second waits behind it.
    running(&s, id, 0, "First").await;
    // A first read carries the open queue; what was delivered is among the changes since the start.
    let first = synced(&s, id, Some(0))
        .await
        .inputs
        .into_iter()
        .find(|input| input.state != InputState::Pending)
        .unwrap();
    let waiting = s
        .say(&s.ada, id, "Second", Delivery::Queue, "second")
        .await
        .ok()
        .input;
    let cancel = |input: Id| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadInputCancel, &[&id, &input])
            .token(&s.ada.token);
        async move { call.send().await }
    };
    let refused = cancel(first.id).await.problem();
    assert_eq!(refused.code, ErrorCode::InputNotPending);
    assert_ne!(refused.extra["state"], "pending");
    let edited = s
        .rig
        .world
        .app
        .call(ThreadInputUpdate, &[&id, &first.id])
        .token(&s.ada.token)
        .body(&ThreadInputUpdateBody {
            native_action: None,
            content: Some(text("changed after the fact")),
            delivery: None,
            after_input_id: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(edited.code, ErrorCode::InputNotPending);
    // Nothing of the input that ran changed, and the one that waits is cancelled as before.
    assert_eq!(cancel(waiting.id).await.ok().state, InputState::Cancelled);
    assert_eq!(open_inputs(&s, id).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_turn_is_stopped_while_it_runs_and_cannot_be_once_it_is_over() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    // Dan drives the thread and has no use of its provider: stopping a turn asks nothing of him there.
    let start = running(&s, id, 0, "Begin").await;
    let stop = |turn: Id| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadInterrupt, &[&id])
            .token(&s.dan.token)
            .body(&ThreadInterruptBody { turn_id: turn });
        async move { call.send().await }
    };
    let stopped = stop(start.turn_id).await.ok();
    assert_eq!(stopped.id, start.turn_id);
    assert!(stopped.interrupted_by.is_some());
    s.rig
        .say(id, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    s.rig.wait_turn(start.turn_id, "interrupted").await;
    // A Stop that arrives as its turn ends, with nothing newer begun, answers the turn as it ended.
    let late = stop(start.turn_id).await.ok();
    assert_eq!(late.id, start.turn_id);
    assert_eq!(late.state, TurnState::Interrupted);
    // Once a newer turn runs, a Stop of the old one changes nothing and names the turn that stands.
    let next = running(&s, id, 1, "Again").await;
    let stale = stop(start.turn_id).await.problem();
    assert_eq!(stale.code, ErrorCode::ThreadNotRunning);
    assert_eq!(
        stale.extra.get("current_turn_id"),
        Some(&json!(next.turn_id))
    );
    assert_eq!(stop(new_id()).await.problem().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn an_input_needs_the_senders_own_right_to_drive_the_thread_and_to_use_its_provider() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    // Dan drives threads as a developer and has no use of the provider: as shipped, his message is
    // taken and waits for somebody who may use the model.
    let dan = s
        .say(&s.dan, id, "Hello", Delivery::Queue, "d")
        .await
        .ok()
        .input;
    assert_eq!(dan.state, InputState::Pending);
    assert_eq!(dan.state_detail.as_deref(), Some("model_use_required"));
    // Cai reads the thread and may not drive it, whatever the model.
    let cai = s
        .say(&s.cai, id, "Hello", Delivery::Queue, "c")
        .await
        .problem();
    assert_eq!(cai.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&cai), ["thread.interact"]);
    // A stranger is told no more than she may know.
    let bea = s
        .say(&s.bea, id, "Hello", Delivery::Queue, "b")
        .await
        .problem();
    assert_eq!(bea.code, ErrorCode::AuthzDenied);
    // Where the place refuses such messages, each refusal names everything missing.
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'thread.inputs_without_use','\"refused\"'::jsonb)")
        .bind(s.tree.topic.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let dan = s
        .say(&s.dan, id, "Hello again", Delivery::Queue, "d2")
        .await
        .problem();
    assert_eq!(dan.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&dan), ["provider.use"]);
    let cai = s
        .say(&s.cai, id, "Hello again", Delivery::Queue, "c2")
        .await
        .problem();
    let lacking = missing_permissions(&cai);
    assert!(
        lacking.contains(&"thread.interact".to_owned()),
        "{lacking:?}"
    );
    assert!(lacking.contains(&"provider.use".to_owned()), "{lacking:?}");
    assert_eq!(
        s.rig.turn_starts(id).len(),
        0,
        "nothing ran on anybody's access by itself"
    );
}

#[tokio::test]
async fn an_input_needs_the_senders_own_use_of_the_environment_its_agent_acts_in() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    // Both Use and Manage stop following the place: the developer otherwise inherits Manage,
    // which also grants Use. Eve is named in Manage on the page and still holds Use through it.
    {
        let mut conn = s.rig.db.acquire().await.unwrap();
        for permission in ["environment.use", "environment.manage"] {
            orch_db::queries::access::write_row(
                &mut conn,
                &orch_db::queries::access::RowWrite {
                    page: &orch_db::queries::access::PageKey::new(
                        "environment",
                        s.on.environment,
                        "live",
                    ),
                    profile: "any",
                    permission,
                    inherit: false,
                    locked: false,
                },
                Some(s.ada.id()),
                s.rig.world.now(),
            )
            .await
            .unwrap();
        }
    }
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'thread.inputs_without_use','\"refused\"'::jsonb)")
        .bind(s.tree.topic.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    // Dan drives threads as a developer: refused, the refusal names everything he lacks at once.
    let dan = s
        .say(&s.dan, id, "Hello", Delivery::Queue, "d1")
        .await
        .problem();
    assert_eq!(dan.code, ErrorCode::AuthzDenied);
    let lacking = missing_permissions(&dan);
    assert!(lacking.contains(&"provider.use".to_owned()), "{lacking:?}");
    assert!(
        lacking.contains(&"environment.use".to_owned()),
        "{lacking:?}"
    );
    // With the provider's use, the environment's is what is left.
    s.rig
        .lets_use("provider", s.on.provider, "provider.use", &s.dan, json!({}))
        .await;
    let dan = s
        .say(&s.dan, id, "Hello", Delivery::Queue, "d2")
        .await
        .problem();
    assert_eq!(missing_permissions(&dan), ["environment.use"]);
    // Named in the environment's Use row, he is heard.
    s.rig
        .lets_use(
            "environment",
            s.on.environment,
            "environment.use",
            &s.dan,
            json!({}),
        )
        .await;
    let dan = s
        .say(&s.dan, id, "Hello", Delivery::Queue, "d3")
        .await
        .ok()
        .input;
    assert_eq!(dan.state, InputState::Pending);
    assert_eq!(dan.state_detail, None);
    // Eve, who manages the environment, holds its Use with it.
    let eve = s
        .say(&s.eve, id, "Hello", Delivery::Queue, "e1")
        .await
        .ok()
        .input;
    assert_eq!(eve.state_detail, None);
}

#[tokio::test]
async fn an_archived_thread_takes_no_input() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.rig
        .world
        .app
        .call(orch_api_contract::nodes::NodeArchive, &[&id])
        .token(&s.ada.token)
        .body(&places::archive_body(&s.rig.world, &s.ada.token, id, true).await)
        .send()
        .await
        .ok();
    let refused = s
        .say(&s.ada, id, "Still there?", Delivery::Queue, "x")
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeArchived);
}

#[tokio::test]
async fn an_input_waiting_for_the_providers_limit_is_let_past_by_whoever_holds_its_bypass_row() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    sqlx::query("UPDATE providers SET status = 'exhausted' WHERE id = $1")
        .bind(s.on.provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let sent = s
        .say(&s.eve, id, "Go on anyway", Delivery::Queue, "limit")
        .await
        .ok();
    s.rig.wait_state(id, "blocked").await;
    assert_eq!(
        s.rig.state(id).await.1.as_deref(),
        Some("provider_exhausted")
    );
    let limits = |who: &Who| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadLimitsGet, &[&id])
            .token(&who.token)
            .query(&ThreadLimitsGetQuery {
                input_id: Some(sent.input.id),
            });
        async move { call.send().await.ok() }
    };
    // Ada owns the provider and holds the row; Eve does not and is told who does.
    let theirs = limits(&s.ada).await;
    assert_eq!(theirs.reached.len(), 1);
    assert!(theirs.reached[0].can_bypass && theirs.bypass_all);
    let mine = limits(&s.eve).await;
    assert!(!mine.reached[0].can_bypass && !mine.bypass_all);
    assert!(
        mine.reached[0]
            .ask
            .iter()
            .any(|person| person.username == "ada")
    );
    let stranger = s
        .rig
        .world
        .app
        .call(ThreadLimitsGet, &[&id])
        .token(&s.bea.token)
        .query(&ThreadLimitsGetQuery { input_id: None })
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&stranger), ["thread.view"]);
    let bypass = |who: &Who| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadInputBypass, &[&id, &sent.input.id])
            .token(&who.token)
            .body(&LimitBypassRequest {
                caps: None,
                provider_limit: Some(true),
            });
        async move { call.send().await }
    };
    let refused = bypass(&s.eve).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "provider.bypass");
    assert_eq!(s.rig.turn_starts(id).len(), 0);
    let passed = bypass(&s.ada).await.ok();
    assert_eq!(passed.limit_bypass.len(), 1);
    assert_eq!(passed.limit_bypass[0].by.username, "ada");
    assert!(passed.limit_bypass[0].provider_limit);
    let start = s.rig.turn_start(id, 0).await;
    assert_eq!(
        start.thread_id, id,
        "pre-flight ran again at once and the turn started"
    );
    let after = limits(&s.ada).await;
    assert_eq!(
        after.reached[0]
            .passed_by
            .as_ref()
            .map(|by| by.username.as_str()),
        Some("ada")
    );
    assert!(!after.bypass_all, "nothing is left that nobody has passed");
}

#[tokio::test]
async fn a_ready_provider_still_shows_reached_caps_and_who_can_pass_them() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let cap = new_id();
    sqlx::query("INSERT INTO usage_caps (id, provider_id, subject_kind, split, meter, amount, at_limit, created_by)
        VALUES ($1, $2, 'everyone', 'shared', 'running', 0, 'stop', $3)")
        .bind(cap).bind(s.on.provider).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    for who in [&s.ada, &s.eve] {
        let limits = s
            .rig
            .world
            .app
            .call(ThreadLimitsGet, &[&thread])
            .token(&who.token)
            .query(&ThreadLimitsGetQuery { input_id: None })
            .send()
            .await
            .ok();
        assert_eq!(limits.reached.len(), 1);
        let reached = &limits.reached[0];
        assert_eq!(reached.cap.as_ref().unwrap().cap.id, cap);
        assert_eq!(reached.provider_limit, None);
        assert_eq!(reached.can_bypass, who.id() == s.ada.id());
        assert_eq!(limits.bypass_all, who.id() == s.ada.id());
        if who.id() == s.eve.id() {
            assert!(reached.ask.iter().any(|who| who.id == s.ada.id()));
        }
    }
}

#[tokio::test]
async fn a_usage_record_names_the_exact_turn_number_in_a_readable_thread() {
    use orch_api_contract::usage::UsageEventList;
    use orch_db::usage::ledger::{self, NewEvent, Tokens};
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let first = running(&s, thread, 0, "First").await;
    s.rig
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    s.rig.wait_turn(first.turn_id, "completed").await;
    let second = running(&s, thread, 1, "Second").await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let attribution =
        orch_db::queries::threads::binding::attribution_of_turn(&mut conn, second.turn_id)
            .await
            .unwrap()
            .unwrap();
    ledger::record(
        &mut conn,
        &NewEvent {
            id: new_id(),
            at: s.rig.world.now(),
            origin: ledger::origin::HARNESS,
            scope: ledger::scope::TURN,
            counted: true,
            attribution: &attribution,
            model: rig::MODEL,
            tier: None,
            tokens: Tokens {
                input: 10,
                total: 10,
                ..Tokens::default()
            },
            context_tokens: None,
            request_ref: None,
            rate: None,
            rate_source: None,
            credits: None,
        },
    )
    .await
    .unwrap();
    drop(conn);
    let day = s.rig.world.now().format("%Y-%m-%d");
    let rows = s
        .rig
        .world
        .app
        .call(UsageEventList, &[])
        .token(&s.ada.token)
        .raw_query(&format!("scope=me&from={day}&to={day}"))
        .send()
        .await
        .ok();
    let record = rows
        .items
        .iter()
        .find(|row| row.turn_id == Some(second.turn_id))
        .unwrap();
    assert_eq!(record.thread.as_ref().unwrap().id, thread);
    assert_eq!(record.turn_ordinal, Some(2));
}

#[tokio::test]
async fn a_turn_that_failed_is_tried_again_as_the_callers_own_input_and_one_that_ended_well_is_not()
{
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "Works", "Fine").await;
    let start = running(&s, id, 1, "Breaks").await;
    s.rig
        .say(id, turn_ended(start.turn_id, TurnOutcome::Failed))
        .await;
    s.rig.wait_turn(start.turn_id, "failed").await;
    let retry = |who: &Who, turn: Id, value: &str| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadTurnRetry, &[&id, &turn])
            .token(&who.token)
            .body(&ThreadTurnRetryBody {
                client_id: value.to_owned(),
                bypass: None,
            });
        async move { call.send().await }
    };
    let tried = retry(&s.eve, start.turn_id, "again").await;
    assert_eq!(tried.status, 201);
    let tried = tried.ok();
    assert_eq!(tried.input.content, text("Breaks"));
    assert_eq!(
        tried
            .input
            .author
            .as_ref()
            .map(|author| author.username.as_str()),
        Some("eve")
    );
    assert_eq!(retry(&s.eve, start.turn_id, "again").await.status, 200);
    let synced = s
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&id])
        .token(&s.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    let done = synced
        .turns
        .iter()
        .find(|turn| turn.state == TurnState::Completed)
        .unwrap();
    assert_eq!(
        retry(&s.eve, done.id, "no").await.problem().code,
        ErrorCode::RequestInvalid
    );
    // Whoever retries needs what an input needs.
    let cai = retry(&s.cai, start.turn_id, "c").await.problem();
    assert!(missing_permissions(&cai).contains(&"thread.interact".to_owned()));
}

#[tokio::test]
async fn retry_does_not_copy_an_input_that_already_returned_to_the_queue() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.rig.seen.refuse(
        CommandKind::TurnStart,
        orch_link_contract::commands::CommandError::new(
            orch_link_contract::commands::ErrorCode::Timeout,
            "the start was not taken",
        )
        .retryable(),
    );
    let original = s
        .say(&s.ada, thread, "Keep this", Delivery::Queue, "first")
        .await
        .ok()
        .input;
    let turn = s.rig.turn_start(thread, 0).await;
    s.rig.wait_turn(turn.turn_id, "failed").await;
    let state: String = sqlx::query_scalar("SELECT state FROM thread_inputs WHERE id=$1")
        .bind(original.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(state, "pending");
    let retry = |who: &Who| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadTurnRetry, &[&thread, &turn.turn_id])
            .token(&who.token)
            .body(&ThreadTurnRetryBody {
                client_id: "retry-pending".to_owned(),
                bypass: None,
            });
        async move { call.send().await }
    };
    let refused = retry(&s.ada).await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(
        refused.detail,
        "The original input is waiting in the message queue or is being delivered"
    );
    assert_eq!(retry(&s.cai).await.problem().code, ErrorCode::AuthzDenied);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn every_driving_request_is_refused_to_a_viewer_and_to_a_stranger() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let sent = s
        .say(&s.ada, id, "Begin", Delivery::Queue, "first")
        .await
        .ok();
    let app = &s.rig.world.app;
    for who in [&s.cai, &s.bea] {
        let token = &who.token;
        let refused = [
            app.call(ThreadInputUpdate, &[&id, &sent.input.id])
                .token(token)
                .body(&ThreadInputUpdateBody {
                    native_action: None,
                    content: None,
                    delivery: None,
                    after_input_id: Some(None),
                })
                .send()
                .await
                .problem(),
            app.call(ThreadInputCancel, &[&id, &sent.input.id])
                .token(token)
                .send()
                .await
                .problem(),
            app.call(ThreadInterrupt, &[&id])
                .token(token)
                .body(&ThreadInterruptBody { turn_id: new_id() })
                .send()
                .await
                .problem(),
            app.call(ThreadInputBypass, &[&id, &sent.input.id])
                .token(token)
                .body(&LimitBypassRequest::default())
                .send()
                .await
                .problem(),
        ];
        for problem in refused {
            assert_eq!(problem.code, ErrorCode::AuthzDenied, "{problem:?}");
            assert!(
                missing_permissions(&problem).contains(&"thread.interact".to_owned()),
                "{problem:?}"
            );
        }
    }
    let _ = json!(null);
}
