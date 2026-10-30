//! A harness asks, a person decides: the request waits, the first decision wins, and the harness is told once.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, command_asked, meanwhile, queued, turn_ended};
use orch_api_contract::approvals::{ApprovalDecision, HarnessOptionDecision};
use orch_api_contract::threads::{ItemKind, ItemStatus};
use orch_authz::Caller;
use orch_engine::approvals::ApprovalError;
use orch_link_contract::commands::{Command, CommandKind};
use orch_link_contract::events::{ApprovalClosed, Event, TurnOutcome};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use serde_json::json;
use std::time::Duration;

fn option(id: &str) -> ApprovalDecision {
    ApprovalDecision::Option(HarnessOptionDecision {
        option_id: id.to_owned(),
        note: None,
    })
}

fn plan_asked(turn: Id, reference: &str, plan: &str) -> Event {
    let Event::ApprovalRequested(mut asked) = command_asked(turn, reference) else {
        unreachable!("the helper creates an approval request")
    };
    asked.request = serde_json::from_value(json!({
        "request_kind": "plan", "detail": { "plan": plan },
        "options": [
            { "id": "proceed", "label": "Proceed", "style": "allow", "scope": "once" },
            { "id": "keep_planning", "label": "Keep planning", "style": "deny", "scope": null }
        ]
    }))
    .unwrap();
    Event::ApprovalRequested(asked)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_harness_plan_can_be_revised_then_proceeded_with_without_changing_the_approval_mode() {
    let world = World::new().await;
    let thread = world.thread("Review a proposed plan").await;
    let (start, _) = world.running_turn(thread, "Plan the change").await;
    let engine = world.engine();
    let caller = Caller::session(world.ada);
    for (index, reference, plan, choice, note, state) in [
        (
            1,
            "first-plan",
            "Read, then change",
            "keep_planning",
            Some("Explain rollback first. 请先读配置。"),
            "denied",
        ),
        (
            2,
            "revised-plan",
            "Read, explain rollback, then change",
            "proceed",
            None,
            "approved",
        ),
    ] {
        world
            .say(thread, plan_asked(start.turn_id, reference, plan))
            .await;
        world.wait_state(thread, "waiting_approval").await;
        let approval: Id = world
            .scalar(
                "SELECT id FROM approvals WHERE thread_id=$1 AND state='pending'",
                thread,
            )
            .await;
        let kind: String = world
            .scalar("SELECT kind FROM approvals WHERE id=$1", approval)
            .await;
        assert_eq!(kind, "harness", "this must use harness option settlement");
        let invalid = engine
            .decide_approval(&caller, approval, &option("unoffered"))
            .await;
        assert!(
            matches!(invalid, Err(ApprovalError::Invalid(ref reason)) if reason.contains("the request offers no option"))
        );
        assert_eq!(world.ran(CommandKind::ApprovalResolve), index - 1);
        let chosen = ApprovalDecision::Option(HarnessOptionDecision {
            option_id: choice.into(),
            note: note.map(str::to_owned),
        });
        let decided = engine
            .decide_approval(&caller, approval, &chosen)
            .await
            .unwrap();
        assert_eq!(decided.state, state);
        world.wait_state(thread, "running").await;
        world.wait_ran(CommandKind::ApprovalResolve, index).await;
        let Command::ApprovalResolve(told) =
            world.seen.of_kind(CommandKind::ApprovalResolve)[index - 1].clone()
        else {
            panic!("a plan answer is sent to the adapter")
        };
        assert_eq!(told.request_ref, reference);
        assert!(matches!(told.decision,
            orch_link_contract::foreign::ApprovalDecision::Option { ref option_id, note: ref forwarded_note }
            if option_id == choice && forwarded_note.as_deref() == note
        ));
        assert_eq!(world.sync(thread).await.thread.pending_approvals, 0);
        let mut conn = world.pool.acquire().await.unwrap();
        let settings = orch_engine::resolved::load(&mut conn, thread)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(settings.approval_mode).unwrap(),
            serde_json::to_value(start.approval_mode).unwrap()
        );
    }
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approval_waits_the_first_decision_wins_and_the_machine_is_told_once() {
    let world = World::new().await;
    let thread = world.thread("Careful").await;
    let (start, _) = world.running_turn(thread, "Clean the build").await;
    world
        .say(thread, command_asked(start.turn_id, "req-1"))
        .await;
    world.wait_state(thread, "waiting_approval").await;

    // The request is in the timeline, waiting, and the people who may decide it were told.
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id = $1", thread)
        .await;
    let sync = world.sync(thread).await;
    let item = sync
        .items
        .iter()
        .find(|item| item.kind() == ItemKind::Approval)
        .unwrap();
    assert_eq!(item.status, ItemStatus::Pending);
    assert_eq!(sync.thread.pending_approvals, 1);
    // The former LiveSink callback ran before commit. The real transport now queues its user
    // notice in the transaction; assert the person's committed notification here, and the
    // socket suite verifies that both API processes receive the committed decision/progress.
    assert!(
        world
            .scalar::<i64>(
                "SELECT count(*) FROM notifications WHERE thread_id=$1
        AND kind='approval.requested' AND user_id=(SELECT owner_id FROM nodes WHERE id=$1)",
                thread
            )
            .await
            > 0
    );
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(
        world.ran(CommandKind::ApprovalResolve),
        0,
        "nothing is sent while nobody has decided"
    );

    // Two decisions at the same moment: one wins and the other is told it lost.
    let ada = Caller::session(world.ada);
    let (allow, deny) = (option("allow_once"), option("deny"));
    let engine = world.engine();
    let (first, second) = tokio::join!(
        engine.decide_approval(&ada, approval, &allow),
        engine.decide_approval(&ada, approval, &deny)
    );
    let outcomes = [first, second];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "{outcomes:?}"
    );
    let won = outcomes
        .iter()
        .find_map(|outcome| outcome.as_ref().ok())
        .unwrap()
        .clone();
    assert!(
        outcomes
            .iter()
            .filter_map(|outcome| outcome.as_ref().err())
            .all(|error| matches!(error, ApprovalError::AlreadyDecided)),
        "{outcomes:?}"
    );
    assert!(matches!(
        engine.decide_approval(&ada, approval, &allow).await,
        Err(ApprovalError::AlreadyDecided)
    ));
    assert!(matches!(
        engine.decide_approval(&ada, approval, &deny).await,
        Err(ApprovalError::AlreadyDecided)
    ));

    // The thread goes on, and the machine is told what was decided, once.
    world.wait_state(thread, "running").await;
    world.wait_ran(CommandKind::ApprovalResolve, 1).await;
    meanwhile(Duration::from_millis(400)).await;
    assert_eq!(world.ran(CommandKind::ApprovalResolve), 1);
    let Command::ApprovalResolve(told) =
        world.seen.of_kind(CommandKind::ApprovalResolve)[0].clone()
    else {
        panic!("an approval.resolve")
    };
    assert_eq!(
        (told.thread_id, told.request_ref.as_str()),
        (thread, "req-1")
    );
    let (word, option_id) = if won.state == "approved" {
        ("approved", "allow_once")
    } else {
        ("denied", "deny")
    };
    let orch_link_contract::foreign::ApprovalDecision::Option {
        option_id: chosen, ..
    } = &told.decision
    else {
        panic!("the harness is told which option was chosen")
    };
    assert_eq!(chosen, option_id);
    assert_eq!(won.state, word);
    assert_eq!(won.decided_by, Some(world.ada));
    let sync = world.sync(thread).await;
    let item = sync
        .items
        .iter()
        .find(|item| item.kind() == ItemKind::Approval)
        .unwrap();
    assert_eq!(
        item.status,
        if word == "approved" {
            ItemStatus::Approved
        } else {
            ItemStatus::Denied
        }
    );
    assert_eq!(sync.thread.pending_approvals, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_the_harness_withdraws_is_cancelled_and_the_turn_goes_on() {
    let world = World::new().await;
    let thread = world.thread("Changed its mind").await;
    let (start, _) = world.running_turn(thread, "Do something").await;
    world
        .say(thread, command_asked(start.turn_id, "req-2"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id = $1", thread)
        .await;

    world
        .say(
            thread,
            Event::ApprovalClosed(ApprovalClosed {
                request_ref: "req-2".to_owned(),
            }),
        )
        .await;
    world.wait_state(thread, "running").await;
    let state: String = world
        .scalar("SELECT state FROM approvals WHERE id = $1", approval)
        .await;
    assert_eq!(state, "cancelled");
    let sync = world.sync(thread).await;
    let item = sync
        .items
        .iter()
        .find(|item| item.kind() == ItemKind::Approval)
        .unwrap();
    assert_eq!(item.status, ItemStatus::Cancelled);
    let late = world
        .engine()
        .decide_approval(&Caller::session(world.ada), approval, &option("allow_once"))
        .await;
    assert!(
        matches!(late, Err(ApprovalError::AlreadyDecided)),
        "{late:?}"
    );
    meanwhile(Duration::from_millis(200)).await;
    assert_eq!(
        world.ran(CommandKind::ApprovalResolve),
        0,
        "a request that is gone is not answered"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approval_still_waiting_when_its_turn_ends_is_cancelled() {
    let world = World::new().await;
    let thread = world.thread("Walked away").await;
    let (start, _) = world.running_turn(thread, "Do something").await;
    world
        .say(thread, command_asked(start.turn_id, "req-3"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id = $1", thread)
        .await;

    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, start.turn_id)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_state(thread, "idle").await;
    let state: String = world
        .scalar("SELECT state FROM approvals WHERE id = $1", approval)
        .await;
    assert_eq!(state, "cancelled");
    assert_eq!(world.sync(thread).await.thread.pending_approvals, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_withdraws_pending_harness_approvals_and_questions_before_the_turn_ends() {
    let world = World::new().await;
    let thread = world.thread("Stop before a decision").await;
    let (start, _) = world.running_turn(thread, "Do something").await;
    world
        .say(thread, command_asked(start.turn_id, "stop-command"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    world
        .say(
            thread,
            person_question(start.turn_id, "stop-question", None),
        )
        .await;
    common::eventually("both native waits are pending", || async {
        world
            .count(
                "SELECT count(*) FROM approvals WHERE thread_id=$1 AND state='pending'",
                thread,
            )
            .await
            == 2
    })
    .await;
    let command: Id = world
        .scalar(
            "SELECT id FROM approvals WHERE thread_id=$1 AND native_ref='stop-command'",
            thread,
        )
        .await;
    let question: Id = world
        .scalar(
            "SELECT id FROM approvals WHERE thread_id=$1 AND native_ref='stop-question'",
            thread,
        )
        .await;
    world.machine.handler.hold(CommandKind::TurnInterrupt);
    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, start.turn_id)
        .await
        .unwrap();
    assert_eq!(
        world.turn_state(start.turn_id).await,
        "running",
        "the native turn has not ended"
    );
    for (id, decision) in [(command, option("allow_once")), (question, person_answer())] {
        assert_eq!(
            world
                .scalar::<String>("SELECT state FROM approvals WHERE id=$1", id)
                .await,
            "cancelled"
        );
        assert!(matches!(
            world
                .engine()
                .decide_approval(&Caller::session(world.ada), id, &decision)
                .await,
            Err(ApprovalError::AlreadyDecided)
        ));
    }
    let sync = world.sync(thread).await;
    assert_eq!(sync.thread.pending_approvals, 0);
    let cards: Vec<_> = sync
        .items
        .iter()
        .filter(|item| item.kind() == ItemKind::Approval)
        .collect();
    assert_eq!(cards.len(), 2);
    assert!(
        cards
            .iter()
            .all(|item| item.status == ItemStatus::Cancelled)
    );
    assert_eq!(world.commands.issued_of("approval.resolve"), 0);
    world.machine.handler.release();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupting_with_a_new_or_queued_input_withdraws_native_waits_atomically() {
    use orch_api_contract::threads::{Delivery, ThreadInputUpdateBody};
    for change_existing in [false, true] {
        let world = World::with_clock(std::sync::Arc::new(orch_testkit::clock::manual())).await;
        let thread = world.thread("Interrupt a waiting question").await;
        let (start, _) = world.running_turn(thread, "Ask").await;
        world
            .say(thread, command_asked(start.turn_id, "interrupt-command"))
            .await;
        world
            .say(
                thread,
                person_question(start.turn_id, "interrupt-question", None),
            )
            .await;
        common::eventually("both native waits are pending", || async {
            world
                .count(
                    "SELECT count(*) FROM approvals WHERE thread_id=$1 AND state='pending'",
                    thread,
                )
                .await
                == 2
        })
        .await;
        let question: Id = world
            .scalar(
                "SELECT id FROM approvals WHERE thread_id=$1 AND native_ref='interrupt-question'",
                thread,
            )
            .await;
        let command: Id = world
            .scalar(
                "SELECT id FROM approvals WHERE thread_id=$1 AND native_ref='interrupt-command'",
                thread,
            )
            .await;
        world.machine.handler.hold(CommandKind::TurnInterrupt);
        let input = if change_existing {
            let waiting = world
                .engine()
                .submit_input(
                    &Caller::session(world.ada),
                    thread,
                    queued("Stop and change course"),
                )
                .await
                .unwrap();
            let mut tx = world.pool.begin().await.unwrap();
            let principal = orch_authz::Principal::load(
                &mut tx,
                Caller::session(world.ada),
                orch_types::Time::new(world.clock.now()),
            )
            .await
            .unwrap();
            world
                .engine()
                .update_input_guarded(
                    tx,
                    principal,
                    thread,
                    waiting.input.id,
                    ThreadInputUpdateBody {
                        delivery: Some(Delivery::Interrupt),
                        ..ThreadInputUpdateBody::default()
                    },
                )
                .await
                .unwrap()
                .input
        } else {
            world
                .engine()
                .submit_input(
                    &Caller::session(world.ada),
                    thread,
                    common::interrupting("Stop and change course"),
                )
                .await
                .unwrap()
                .input
        };
        assert_eq!(world.turn_state(start.turn_id).await, "running");
        assert_eq!(world.input_state(input.id).await.0, "pending");
        for (id, decision) in [(command, option("allow_once")), (question, person_answer())] {
            assert_eq!(
                world
                    .scalar::<String>("SELECT state FROM approvals WHERE id=$1", id)
                    .await,
                "cancelled"
            );
            assert!(matches!(
                world
                    .engine()
                    .decide_approval(&Caller::session(world.ada), id, &decision)
                    .await,
                Err(ApprovalError::AlreadyDecided)
            ));
        }
        assert_eq!(world.sync(thread).await.thread.pending_approvals, 0);
        assert_eq!(world.commands.issued_of("approval.resolve"), 0);
        world.machine.handler.release();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabling_a_provider_withdraws_a_waiting_question_in_the_interrupt_transaction() {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::resource_facts::FactTarget;
    use orch_engine::resource_pause::ResourceStop;
    let world = World::with_clock(std::sync::Arc::new(orch_testkit::clock::manual())).await;
    let thread = world.thread("Pause a waiting question").await;
    let (start, _) = world.running_turn(thread, "Ask").await;
    world
        .say(
            thread,
            person_question(start.turn_id, "resource-question", None),
        )
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    let target = FactTarget {
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: world.provider,
        },
        model_id: None,
    };
    let mut tx = world.pool.begin().await.unwrap();
    world
        .engine()
        .stop_resource_in(
            &mut tx,
            ResourceStop {
                target,
                name: "Provider",
                by: world.ada,
                note: None,
                deleted: false,
            },
            world.clock.now(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    assert_eq!(
        world
            .scalar::<String>("SELECT state FROM approvals WHERE id=$1", approval)
            .await,
        "cancelled"
    );
    assert!(matches!(
        world
            .engine()
            .decide_approval(&Caller::session(world.ada), approval, &person_answer())
            .await,
        Err(ApprovalError::AlreadyDecided)
    ));
    assert_eq!(world.sync(thread).await.thread.pending_approvals, 0);
    assert_eq!(world.commands.issued_of("approval.resolve"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_raised_again_is_one_approval_and_one_under_the_reference_of_an_earlier_turn_is_dropped()
 {
    let world = World::new().await;
    let thread = world.thread("Same references").await;
    let (first, _) = world.running_turn(thread, "First turn").await;
    // The same event applied again is the same request: it makes no second approval.
    world
        .say(thread, command_asked(first.turn_id, "req-same"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    world
        .say(thread, command_asked(first.turn_id, "req-same"))
        .await;
    world
        .say(thread, command_asked(first.turn_id, "req-after"))
        .await;
    common::eventually("the request that follows the repeated one", || async {
        world
            .count(
                "SELECT count(*) FROM approvals WHERE thread_id = $1",
                thread,
            )
            .await
            == 2
    })
    .await;

    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, first.turn_id)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_state(thread, "idle").await;

    // A request of a later turn that comes under a reference the thread was given before is dropped, as the same
    // request raised again is (the dedupe is by thread and reference, and stays so); a harness whose references
    // repeat is warned of in the log. The request that follows it asks the person as usual.
    let (second, _) = world.running_turn(thread, "Second turn").await;
    world
        .say(thread, command_asked(second.turn_id, "req-same"))
        .await;
    world
        .say(thread, command_asked(second.turn_id, "req-new"))
        .await;
    common::eventually("the request that follows the dropped one", || async {
        world
            .count(
                "SELECT count(*) FROM approvals WHERE thread_id = $1",
                thread,
            )
            .await
            == 3
    })
    .await;
    world.wait_state(thread, "waiting_approval").await;
    let kept: Id = world
        .scalar(
            "SELECT turn_id FROM approvals WHERE thread_id = $1 AND native_ref = 'req-same'",
            thread,
        )
        .await;
    assert_eq!(kept, first.turn_id, "the first request keeps its reference");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_who_does_not_manage_the_thread_cannot_decide() {
    let world = World::new().await;
    let thread = world.thread("Not yours").await;
    let (start, _) = world.running_turn(thread, "Do something").await;
    world
        .say(thread, command_asked(start.turn_id, "req-4"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id = $1", thread)
        .await;
    let stranger = world.person("eve").await;

    let refused = world
        .engine()
        .decide_approval(&Caller::session(stranger), approval, &option("allow_once"))
        .await;
    assert!(
        matches!(refused, Err(ApprovalError::Denied(_))),
        "{refused:?}"
    );
    let state: String = world
        .scalar("SELECT state FROM approvals WHERE id = $1", approval)
        .await;
    assert_eq!(state, "pending", "a refused decision decides nothing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manager_who_loses_access_while_a_question_is_open_cannot_answer_it() {
    let world = World::new().await;
    let manager = world.person("departing-manager").await;
    let role = world.give(manager, "developer").await;
    let thread = world.thread("A question while access changes").await;
    let (start, _) = world.running_turn(thread, "Ask").await;
    world
        .say(
            thread,
            person_question(start.turn_id, "revoked-manager", None),
        )
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    {
        let mut conn = world.pool.acquire().await.unwrap();
        let principal = orch_authz::Principal::load(
            &mut conn,
            Caller::session(manager),
            orch_types::Time::new(world.clock.now()),
        )
        .await
        .unwrap();
        assert!(
            orch_authz::nodes::check(
                &mut conn,
                &principal,
                thread,
                orch_access_contract::permissions::Permission::ThreadManage,
                orch_authz::Act::Use,
                orch_authz::Policy::default()
            )
            .await
            .unwrap()
            .is_allowed()
        );
    }
    world.take(role).await;
    assert!(matches!(
        world
            .engine()
            .decide_approval(&Caller::session(manager), approval, &person_answer())
            .await,
        Err(ApprovalError::Denied(_))
    ));
    assert_eq!(
        world
            .scalar::<String>("SELECT state FROM approvals WHERE id=$1", approval)
            .await,
        "pending"
    );
    assert_eq!(world.commands.issued_of("approval.resolve"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_bypass_a_question_still_waits_for_a_person() {
    let world = World::new().await;
    isolated_environment(&world).await;
    let thread = world.thread("Trusting, and asked").await;
    world
        .set(thread, keys::APPROVAL_MODE, json!("bypass"))
        .await;
    let (start, _) = world.running_turn(thread, "Pick one").await;
    world
        .say(
            thread,
            Event::ApprovalRequested(orch_link_contract::events::ApprovalRequested {
                turn_id: start.turn_id,
                request_ref: "ask-1".to_owned(),
                title: "Which branch?".to_owned(),
                request: serde_json::from_value(json!({
                    "request_kind": "user_input",
                    "detail": { "questions": [{
                        "id": "branch", "header": "Branch", "question": "Which branch should I use?",
                        "multiple": false, "allow_other": true,
                        "options": [{ "label": "main", "description": "the main line" }]
                    }] },
                    "options": [{ "id": "answer", "label": "Answer", "style": "allow", "scope": null }]
                })).unwrap(),
            }),
        )
        .await;
    world.wait_state(thread, "waiting_approval").await;
    meanwhile(Duration::from_millis(300)).await;
    let state: String = world
        .scalar("SELECT state FROM approvals WHERE thread_id = $1", thread)
        .await;
    assert_eq!(state, "pending", "no mode answers a question");
    assert_eq!(world.ran(CommandKind::ApprovalResolve), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn questions_in_admitted_modes_deliver_only_the_winning_answer_and_auto_keeps_its_admission_gate()
 {
    let world = World::new().await;
    isolated_environment(&world).await;
    let stranger = world.person("question-reader").await;
    for (index, mode) in ["ask", "on_request", "bypass", "auto"]
        .into_iter()
        .enumerate()
    {
        let thread = world.thread(&format!("Question in {mode}")).await;
        world.set(thread, keys::APPROVAL_MODE, json!(mode)).await;
        if mode == "auto" {
            let opened = world.ran(CommandKind::SessionOpen);
            world
                .engine()
                .submit_input(
                    &Caller::session(world.ada),
                    thread,
                    queued("Choose a branch"),
                )
                .await
                .unwrap();
            world.wait_state(thread, "blocked").await;
            let reason: String = world
                .scalar(
                    "SELECT blocked_reason FROM threads WHERE node_id=$1",
                    thread,
                )
                .await;
            assert_eq!(reason, "binding_invalid");
            assert_eq!(world.ran(CommandKind::SessionOpen), opened);
            assert_eq!(world.ran(CommandKind::ApprovalResolve), index);
            assert_eq!(world.sync(thread).await.thread.pending_approvals, 0);
            continue;
        }
        let (start, _) = world.running_turn(thread, "Choose a branch").await;
        let reference = format!("question-{index}");
        world.say(thread, Event::ApprovalRequested(orch_link_contract::events::ApprovalRequested {
            turn_id: start.turn_id,
            request_ref: reference.clone(),
            title: "Which branch?".into(),
            request: serde_json::from_value(json!({
                "request_kind": "user_input", "options": [],
                "detail": { "questions": [{ "id": "branch", "header": "Branch", "question": "Which branch?",
                    "multiple": false, "allow_other": false,
                    "options": [{ "label": "main", "description": "Main branch" }, { "label": "work", "description": "Work branch" }]
                }] }
            })).unwrap(),
        })).await;
        world.wait_state(thread, "waiting_approval").await;
        let approval: Id = world
            .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
            .await;
        let answer = |values| {
            serde_json::from_value::<ApprovalDecision>(json!({ "answers": values })).unwrap()
        };
        let main = answer(json!({ "branch": ["main"] }));
        assert!(matches!(
            world
                .engine()
                .decide_approval(&Caller::session(stranger), approval, &main)
                .await,
            Err(ApprovalError::Denied(_))
        ));
        for values in [
            json!({}),
            json!({ "unknown": ["main"] }),
            json!({ "branch": [] }),
            json!({ "branch": [" "] }),
            json!({ "branch": ["unoffered"] }),
            json!({ "branch": ["main", "work"] }),
        ] {
            assert!(matches!(
                world
                    .engine()
                    .decide_approval(&Caller::session(world.ada), approval, &answer(values))
                    .await,
                Err(ApprovalError::Invalid(_))
            ));
            let state: String = world
                .scalar("SELECT state FROM approvals WHERE id=$1", approval)
                .await;
            assert_eq!(state, "pending");
            assert_eq!(world.sync(thread).await.thread.pending_approvals, 1);
            assert_eq!(world.ran(CommandKind::ApprovalResolve), index);
        }
        let work = answer(json!({ "branch": ["work"] }));
        let caller = Caller::session(world.ada);
        let engine = world.engine();
        let decisions = tokio::join!(
            engine.decide_approval(&caller, approval, &main),
            engine.decide_approval(&caller, approval, &work)
        );
        let results = [decisions.0, decisions.1];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .all(|error| matches!(error, ApprovalError::AlreadyDecided))
        );
        let won = results
            .iter()
            .find_map(|result| result.as_ref().ok())
            .unwrap();
        assert_eq!(won.state, "answered");
        assert_eq!(won.decided_by, Some(world.ada));
        world
            .wait_ran(CommandKind::ApprovalResolve, index + 1)
            .await;
        let commands = world.seen.of_kind(CommandKind::ApprovalResolve);
        let Command::ApprovalResolve(delivered) = commands.last().unwrap() else {
            panic!("the native answer command")
        };
        assert_eq!(delivered.thread_id, thread);
        assert_eq!(delivered.request_ref, reference);
        assert_eq!(
            serde_json::to_value(&delivered.decision).unwrap(),
            won.decision.clone().unwrap()
        );
        let synced = world.sync(thread).await;
        assert_eq!(synced.thread.pending_approvals, 0);
        assert_eq!(
            synced
                .items
                .iter()
                .find(|item| item.kind() == ItemKind::Approval)
                .unwrap()
                .status,
            ItemStatus::Answered
        );
        assert!(matches!(
            engine.decide_approval(&caller, approval, &main).await,
            Err(ApprovalError::AlreadyDecided)
        ));
        world
            .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
            .await;
        world.wait_state(thread, "idle").await;
        let state: String = world
            .scalar("SELECT state FROM approvals WHERE id=$1", approval)
            .await;
        assert_eq!(state, "answered");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_bypass_a_permission_request_that_comes_anyway_is_a_card_and_orch_invents_no_answer()
{
    let world = World::new().await;
    isolated_environment(&world).await;
    let thread = world.thread("Trusting").await;
    world
        .set(thread, keys::APPROVAL_MODE, json!("bypass"))
        .await;
    let (start, _) = world.running_turn(thread, "Do something").await;
    world
        .say(thread, command_asked(start.turn_id, "req-5"))
        .await;

    world.wait_state(thread, "waiting_approval").await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(world.ran(CommandKind::ApprovalResolve), 0);
    let (state, by): (String, Option<Id>) =
        sqlx::query_as("SELECT state, decided_by FROM approvals WHERE thread_id = $1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), by),
        ("pending", None),
        "the harness's actual request still waits for a person"
    );
    assert_eq!(
        world.state(thread).await.0,
        "waiting_approval",
        "a request the harness still raises waits for a person in every mode"
    );
    let shown = world.sync(thread).await;
    assert_eq!(shown.thread.pending_approvals, 1);
    assert!(
        shown
            .items
            .iter()
            .any(|item| item.kind() == ItemKind::Approval && item.status == ItemStatus::Pending)
    );
}

async fn isolated_environment(world: &World) {
    let host = orch_types::new_id();
    sqlx::query(
        "INSERT INTO hosts (id,machine_id,name,drivers) VALUES ($1,$2,'Confined test host',$3)",
    )
    .bind(host)
    .bind(world.machine_id)
    .bind(json!({"test":{"confinement":"isolated"}}))
    .execute(world.pool.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO runtimes (id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state) SELECT $1,'environment',$2,$3,e.id,'test',e.template_id,$4,'running' FROM environments e WHERE e.id=$5")
        .bind(orch_types::new_id()).bind(host).bind(format!("test-{}",world.environment)).bind(vec![0_u8;32]).bind(world.environment)
        .execute(world.pool.pool()).await.unwrap();
}

fn person_question(turn: Id, reference: &str, source: Option<&str>) -> Event {
    Event::ApprovalRequested(orch_link_contract::events::ApprovalRequested {
        turn_id: turn, request_ref: reference.into(), title: "Choose a branch".into(),
        request: serde_json::from_value(json!({ "request_kind":"user_input", "options":[], "detail":{
            "native_item_ref":source, "questions":[{"id":"branch","header":"Branch","question":"Which branch?","multiple":false,"allow_other":false,
                "options":[{"label":"main","description":"Main"},{"label":"work","description":"Work"}]}]
        }})).unwrap(),
    })
}
fn person_answer() -> ApprovalDecision {
    serde_json::from_value(json!({"answers":{"branch":["main"]}})).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_question_answer_is_delivered_once_after_the_controller_restarts() {
    let world = World::new().await;
    let thread = world.thread("A durable answer").await;
    let (start, _) = world.running_turn(thread, "Ask").await;
    world
        .say(
            thread,
            person_question(start.turn_id, "durable-question", None),
        )
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    // Foreground commit can finish during scheduler shutdown; the next process must discover it.
    let stopped = world.engine();
    stopped.stop();
    let answered = stopped
        .decide_approval(&Caller::session(world.ada), approval, &person_answer())
        .await
        .unwrap();
    assert_eq!(answered.state, "answered");
    assert_eq!(world.ran(CommandKind::ApprovalResolve), 0);
    world.restart_api().await;
    world.wait_ran(CommandKind::ApprovalResolve, 1).await;
    world.restart_api().await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(world.ran(CommandKind::ApprovalResolve), 1);
    assert_eq!(
        world
            .scalar::<String>("SELECT state FROM approvals WHERE id=$1", approval)
            .await,
        "answered"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_not_yet_issued_is_not_sent_after_its_turn_ends() {
    let world = World::new().await;
    let thread = world.thread("An answer to a finished turn").await;
    let (start, _) = world.running_turn(thread, "Ask").await;
    world
        .say(thread, person_question(start.turn_id, "late-answer", None))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    // The person commits the answer, but no supervisor can issue it yet.
    let stopped = world.engine();
    stopped.stop();
    stopped
        .decide_approval(&Caller::session(world.ada), approval, &person_answer())
        .await
        .unwrap();
    assert_eq!(world.commands.issued_of("approval.resolve"), 0);
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.restart_api().await;
    common::eventually("the undeliverable answer is cancelled", || async {
        world
            .scalar::<String>("SELECT state FROM approvals WHERE id=$1", approval)
            .await
            == "cancelled"
    })
    .await;
    assert_eq!(world.commands.issued_of("approval.resolve"), 0);
    let saved: serde_json::Value = world
        .scalar("SELECT decision FROM approvals WHERE id=$1", approval)
        .await;
    assert_eq!(saved, json!({"answers":{"branch":["main"]}}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivered_answer_stays_answered_after_its_turn_ends_and_the_controller_restarts() {
    let world = World::new().await;
    let thread = world.thread("An answer already delivered").await;
    let (start, _) = world.running_turn(thread, "Ask").await;
    world
        .say(
            thread,
            person_question(start.turn_id, "answered-before-end", None),
        )
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    world
        .engine()
        .decide_approval(&Caller::session(world.ada), approval, &person_answer())
        .await
        .unwrap();
    world.wait_ran(CommandKind::ApprovalResolve, 1).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.restart_api().await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(world.commands.issued_of("approval.resolve"), 1);
    assert_eq!(
        world
            .scalar::<String>("SELECT state FROM approvals WHERE id=$1", approval)
            .await,
        "answered"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_the_native_harness_cannot_receive_is_cancelled_without_losing_its_content() {
    use orch_link_contract::commands::{CommandError, ErrorCode};
    let world = World::new().await;
    let thread = world.thread("Undeliverable answer").await;
    let (start, _) = world.running_turn(thread, "Ask").await;
    world
        .say(
            thread,
            person_question(start.turn_id, "lost-question", None),
        )
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    world.refuse(
        CommandKind::ApprovalResolve,
        vec![CommandError::new(
            ErrorCode::SessionNotFound,
            "The native question is no longer waiting",
        )],
    );
    world
        .engine()
        .decide_approval(&Caller::session(world.ada), approval, &person_answer())
        .await
        .unwrap();
    common::eventually(
        "the undelivered content answer to become cancelled",
        || async {
            world
                .scalar::<String>("SELECT state FROM approvals WHERE id=$1", approval)
                .await
                == "cancelled"
        },
    )
    .await;
    let decision: serde_json::Value = world
        .scalar("SELECT decision FROM approvals WHERE id=$1", approval)
        .await;
    assert_eq!(decision, json!({"answers":{"branch":["main"]}}));
    let synced = world.sync(thread).await;
    assert_eq!(
        synced
            .items
            .iter()
            .find(|item| item.kind() == ItemKind::Approval)
            .unwrap()
            .status,
        ItemStatus::Cancelled
    );
    assert!(synced.items.iter().any(|item| matches!(&item.payload, orch_api_contract::threads::ItemPayload::Error(error) if error.code == "approval_undelivered")));
    assert!(matches!(
        world
            .engine()
            .decide_approval(&Caller::session(world.ada), approval, &person_answer())
            .await,
        Err(ApprovalError::AlreadyDecided)
    ));
}

fn native_question_payload(
    world: &World,
    completed: bool,
) -> orch_link_contract::foreign::ItemPayload {
    use orch_agent_contract::events::{OutputRef, RawCall, ToolCallPayload, ToolPayload};
    orch_link_contract::foreign::ItemPayload::ToolCall(ToolCallPayload {
        title: "Native question".into(),
        started_at: world.clock.time(),
        duration_ms: completed.then_some(1),
        tool: ToolPayload::Other {
            name: "request_user_input".into(),
            arguments: Some(json!({"question":"Which branch?"})),
            result: completed.then(|| json!({"branch":"main"})),
        },
        raw: RawCall {
            name: "request_user_input".into(),
            input: OutputRef::of("{\"question\":\"Which branch?\"}"),
            output: completed.then(|| OutputRef::of("{\"branch\":\"main\"}")),
            is_error: false,
        },
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_links_its_exact_native_source_before_after_or_after_a_lost_start_and_turn_end()
{
    use orch_link_contract::events::{EmittedItemKind, ItemCompleted, ItemStarted};
    let world = World::new().await;
    for order in ["before", "after", "completion", "late-completion"] {
        let thread = world.thread(&format!("Question source {order}")).await;
        let (start, _) = world.running_turn(thread, "Ask").await;
        let source = format!("native-{order}");
        let began = Event::ItemStarted(ItemStarted {
            turn_id: start.turn_id,
            native_ref: source.clone(),
            stream: "main".into(),
            parent_native_ref: None,
            kind: EmittedItemKind::ToolCall,
            payload: native_question_payload(&world, false),
        });
        if order == "before" {
            world.say(thread, began.clone()).await;
        }
        world
            .say(
                thread,
                person_question(start.turn_id, &format!("request-{order}"), Some(&source)),
            )
            .await;
        world.wait_state(thread, "waiting_approval").await;
        let approval: Id = world
            .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
            .await;
        if order == "late-completion" {
            world
                .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
                .await;
            world.wait_state(thread, "idle").await;
        }
        if order == "after" {
            world.say(thread, began.clone()).await;
        }
        if order == "completion" || order == "late-completion" {
            world
                .say(
                    thread,
                    Event::ItemCompleted(ItemCompleted {
                        native_ref: source.clone(),
                        status: orch_link_contract::foreign::ItemStatus::Completed,
                        payload: native_question_payload(&world, true),
                        output_blob: None,
                    }),
                )
                .await;
        }
        common::eventually("the question's explicit native source edge", || async {
            world.scalar::<bool>("SELECT EXISTS(SELECT 1 FROM approvals a JOIN items card ON card.id=a.item_id JOIN items source ON source.id=card.parent_item_id WHERE a.id=$1 AND card.thread_id=source.thread_id AND card.turn_id=source.turn_id AND source.kind='tool_call')",approval).await
        }).await;
        let (native, turn): (String, Id) = sqlx::query_as("SELECT source.native_ref,source.turn_id FROM approvals a JOIN items card ON card.id=a.item_id JOIN items source ON source.id=card.parent_item_id WHERE a.id=$1").bind(approval).fetch_one(world.pool.pool()).await.unwrap();
        assert_eq!(native, source);
        assert_eq!(turn, start.turn_id);
        world
            .say(
                thread,
                person_question(start.turn_id, &format!("request-{order}"), Some(&source)),
            )
            .await;
        world.say(thread, began).await;
        assert_eq!(
            world
                .scalar::<i64>(
                    "SELECT count(*) FROM items WHERE thread_id=$1 AND kind='approval'",
                    thread
                )
                .await,
            1
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_never_borrows_a_native_source_from_another_turn() {
    use orch_link_contract::events::{EmittedItemKind, ItemStarted};
    let world = World::new().await;
    let thread = world.thread("Question source boundaries").await;
    let (first, _) = world.running_turn(thread, "First turn").await;
    world
        .say(
            thread,
            Event::ItemStarted(ItemStarted {
                turn_id: first.turn_id,
                native_ref: "older-native-source".into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: EmittedItemKind::ToolCall,
                payload: native_question_payload(&world, false),
            }),
        )
        .await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let (second, _) = world.running_turn(thread, "Second turn").await;
    world
        .say(
            thread,
            person_question(second.turn_id, "new-question", Some("older-native-source")),
        )
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let parent: Option<Id> = sqlx::query_scalar("SELECT item.parent_item_id FROM approvals a JOIN items item ON item.id=a.item_id WHERE a.thread_id=$1 AND a.turn_id=$2")
        .bind(thread).bind(second.turn_id).fetch_one(world.pool.pool()).await.unwrap();
    assert!(
        parent.is_none(),
        "content-source correlation cannot cross its owning turn"
    );
}
