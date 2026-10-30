//! Proposed plans and held inputs share one decision ledger, one winner, and the same live audience rule.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
use common::{World, command_asked, queued, turn_began, turn_ended};
use orch_api_contract::approvals::RequestDecisionAction;
use orch_api_contract::threads::ThreadPlanDecideBody;
use orch_authz::{Caller, Principal};
use orch_engine::inputs::{InputError, InputRequest};
use orch_types::{Id, Time};
use serde_json::json;

async fn proposal(world: &World, thread: Id) -> Id {
    world
        .exec(r#"UPDATE agents SET descriptor_overrides='{"capabilities":{"plan_mode":true}}'"#)
        .await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            InputRequest {
                plan: true,
                ..queued("Plan the change")
            },
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(start.turn_id)).await;
    let payload=serde_json::from_value(json!({"explanation":"The proposed work","steps":[{"text":"Read the code","status":"pending"}]})).unwrap();
    world
        .say(
            thread,
            orch_link_contract::events::Event::ItemStarted(
                orch_link_contract::events::ItemStarted {
                    turn_id: start.turn_id,
                    native_ref: "plan-1".into(),
                    stream: "main".into(),
                    parent_native_ref: None,
                    kind: orch_link_contract::events::EmittedItemKind::Plan,
                    payload: orch_link_contract::foreign::ItemPayload::Plan(payload),
                },
            ),
        )
        .await;
    world.say(thread,orch_link_contract::events::Event::ItemCompleted(orch_link_contract::events::ItemCompleted {native_ref:"plan-1".into(),status:orch_agent_contract::events::ItemStatus::Completed,payload:orch_link_contract::foreign::ItemPayload::Plan(serde_json::from_value(json!({"explanation":"The proposed work","steps":[{"text":"Read the code","status":"pending"}]})).unwrap()),output_blob:None})).await;
    world
        .say(
            thread,
            turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world
        .scalar(
            "SELECT id FROM items WHERE thread_id=$1 AND kind='plan'",
            thread,
        )
        .await
}
async fn plan_decision(
    world: &World,
    who: Id,
    thread: Id,
    item: Id,
    proceed: bool,
) -> Result<orch_api_contract::threads::ThreadPlanDecideReply, InputError> {
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, Caller::session(who), Time::new(world.clock.now()))
        .await
        .unwrap();
    world
        .engine()
        .decide_plan_guarded(
            tx,
            principal,
            thread,
            item,
            &ThreadPlanDecideBody {
                proceed,
                text: (!proceed).then(|| "Cover the rollback too".to_owned()),
            },
        )
        .await
}
async fn told(world: &World, approval: Id) -> Vec<Id> {
    sqlx::query_scalar("SELECT user_id FROM notifications WHERE approval_id=$1 AND kind IN ('approval.requested','access.requested') ORDER BY user_id").bind(approval).fetch_all(world.pool.pool()).await.unwrap()
}
async fn settled(world: &World, approval: Id, by: Id, state: &str) {
    let row =
        orch_db::queries::approvals::approval(&mut world.pool.acquire().await.unwrap(), approval)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(row.state, state);
    assert_eq!(row.decided_by, Some(by));
    let unread: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE approval_id=$1 AND read_at IS NULL",
    )
    .bind(approval)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(unread, 0);
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM approvals WHERE id=$1 AND state='pending'")
            .bind(approval)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_plan_deciders_share_one_outcome_and_only_one_follow_up_is_queued() {
    let world = World::new().await;
    let bo = world.person("bob").await;
    let cy = world.person("cyrus").await;
    world.give(bo, "interactor").await;
    world.give(cy, "interactor").await;
    let thread = world.thread("Plan answer").await;
    let item = proposal(&world, thread).await;
    let approval: Id = world
        .scalar(
            "SELECT (payload->>'approval_id')::uuid FROM items WHERE id=$1",
            item,
        )
        .await;
    assert_eq!(told(&world, approval).await, [world.ada]);
    let (a, b) = tokio::join!(
        plan_decision(&world, bo, thread, item, true),
        plan_decision(&world, cy, thread, item, false)
    );
    let outcomes = [a, b];
    assert_eq!(
        outcomes.iter().filter(|result| result.is_ok()).count(),
        1,
        "{outcomes:?}"
    );
    let won = outcomes
        .iter()
        .find_map(|result| result.as_ref().ok())
        .unwrap();
    let winner = won.decision.decided_by.as_ref().unwrap().id;
    assert!(outcomes.iter().filter_map(|result|result.as_ref().err()).all(|error|matches!(error,InputError::AlreadyDecided(status) if status.action==won.decision.action&&status.decided_by.as_ref().unwrap().id==winner)));
    settled(
        &world,
        approval,
        winner,
        if won.decision.action == Some(RequestDecisionAction::KeepPlanning) {
            "answered"
        } else {
            "approved"
        },
    )
    .await;
    let followups: i64 = world
        .scalar(
            "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
            thread,
        )
        .await;
    assert_eq!(followups, 2, "one original and one decision input");
    let raw: serde_json::Value = world
        .scalar("SELECT payload FROM items WHERE id=$1", item)
        .await;
    assert!(
        raw.get("decision").is_none(),
        "only the approval row stores the decision"
    );
    let sync = world.sync(thread).await;
    let view = serde_json::to_value(sync.items.iter().find(|row| row.id == item).unwrap()).unwrap();
    assert_eq!(
        view["payload"]["decision"]["decided_by"]["id"],
        json!(winner)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plan_proceeds_as_the_person_who_approved_it() {
    let world = World::new().await;
    let writer = world.person("plan-writer").await;
    world.give(writer, "interactor").await;
    let thread = world.thread("Plan authority").await;
    let item = proposal(&world, thread).await;
    let chosen = plan_decision(&world, writer, thread, item, true)
        .await
        .unwrap();
    assert_eq!(chosen.input.author.as_ref().unwrap().id, writer);
    assert_eq!(chosen.input.run_as, None);
    assert_eq!(chosen.decision.decided_by.as_ref().unwrap().id, writer);
    let start = world.turn_start(thread, 1).await;
    assert!(!start.plan);
    let responsible: Option<Id> = world
        .scalar(
            "SELECT audit_for_user_id FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    assert_eq!(responsible, Some(writer));
    world.say(thread, turn_began(start.turn_id)).await;
    world
        .say(
            thread,
            turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plan_proceed_without_the_writers_model_use_waits_for_an_authorized_person() {
    let world = World::new().await;
    let writer = world.person("plan-writer-without-model").await;
    world.give(writer, "interactor").await;
    let thread = world.thread("Plan without model use").await;
    let item = proposal(&world, thread).await;
    world.access.deny_model(writer, common::MODEL);
    let chosen = plan_decision(&world, writer, thread, item, true)
        .await
        .unwrap();
    assert_eq!(chosen.input.author.as_ref().unwrap().id, writer);
    assert_eq!(chosen.input.run_as, None);
    assert_eq!(
        world.input_state(chosen.input.id).await,
        ("pending".into(), Some("model_use_required".into()))
    );
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "only the earlier plan turn ran"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_model_users_may_run_one_held_input_once_and_its_author_stays_named() {
    let world = World::new().await;
    let bo = world.person("bob").await;
    let cy = world.person("cyrus").await;
    let author = world.person("author").await;
    for person in [bo, cy, author] {
        world.give(person, "interactor").await;
    }
    let thread = world.thread("Held input").await;
    world.access.deny(author);
    let made = world
        .engine()
        .submit_input(
            &Caller::session(author),
            thread,
            queued("Please implement this"),
        )
        .await
        .unwrap();
    let approval = made.input.approval_id.unwrap();
    assert_eq!(told(&world, approval).await, [world.ada]);
    let engine = world.engine();
    let a = Caller::session(bo);
    let b = Caller::session(cy);
    let (first, second) = tokio::join!(
        engine.run_input(&a, thread, made.input.id),
        engine.run_input(&b, thread, made.input.id)
    );
    let outcomes = [first, second];
    assert_eq!(
        outcomes.iter().filter(|result| result.is_ok()).count(),
        1,
        "{outcomes:?}"
    );
    let won = outcomes
        .iter()
        .find_map(|result| result.as_ref().ok())
        .unwrap();
    assert_eq!(won.author.as_ref().unwrap().id, author);
    let winner = won.run_as.as_ref().unwrap().id;
    settled(&world, approval, winner, "approved").await;
    assert!(outcomes.iter().filter_map(|result|result.as_ref().err()).all(|error|matches!(error,InputError::AlreadyDecided(status) if status.action==Some(RequestDecisionAction::Run)&&status.decided_by.as_ref().unwrap().id==winner)));
    let starts = world.turn_start(thread, 0).await;
    let responsible: Option<Id> = world
        .scalar(
            "SELECT audit_for_user_id FROM turns WHERE id=$1",
            starts.turn_id,
        )
        .await;
    assert_eq!(responsible, Some(winner));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_input_tells_the_only_model_user_when_no_owner_or_admin_can_run_it() {
    let world = World::new().await;
    let bo = world.person("bob").await;
    world.give(bo, "interactor").await;
    let thread = world.thread("Private model").await;
    world.access.deny(world.ada);
    let made = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Words from Ada"),
        )
        .await
        .unwrap();
    let approval = made.input.approval_id.unwrap();
    assert_eq!(told(&world, approval).await, [bo]);
    let ran = world
        .engine()
        .run_input(&Caller::session(bo), thread, made.input.id)
        .await
        .unwrap();
    assert_eq!(ran.author.as_ref().unwrap().id, world.ada);
    assert_eq!(ran.run_as.as_ref().unwrap().id, bo);
    let start = world.turn_start(thread, 0).await;
    let responsible: Option<Id> = world
        .scalar(
            "SELECT audit_for_user_id FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    assert_eq!(responsible, Some(bo));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_tells_its_eligible_starter_and_otherwise_its_owner() {
    let world = World::new().await;
    let bo = world.person("bob").await;
    let cy = world.person("cyrus").await;
    world.give(bo, "developer").await;
    world.give(cy, "interactor").await;
    for (index, starter, expected) in [(0, bo, bo), (1, cy, world.ada), (2, bo, world.ada)] {
        let thread = world.thread(&format!("Audience {index}")).await;
        let input = world
            .engine()
            .submit_input(&Caller::session(starter), thread, queued("Start work"))
            .await
            .unwrap()
            .input
            .id;
        let start = world.turn_start(thread, 0).await;
        world.say(thread, turn_began(start.turn_id)).await;
        if index == 2 {
            // A scheduled occurrence keeps Bo as author, but Bo did not deliberately start this turn.
            let schedule = orch_types::new_id();
            let occurrence = orch_types::new_id();
            let now = world.clock.now();
            sqlx::query("INSERT INTO input_schedules(id,topic_id,author_id,name,target,content,timing,state,topic_seq,created_at,updated_at) VALUES($1,$2,$3,'Scheduled work',$4,$5,$6,'finished',0,$7,$7)")
                .bind(schedule).bind(world.topic).bind(bo).bind(json!({"kind":"thread","thread_id":thread})).bind(json!([{"type":"text","text":"Start work"}])).bind(json!({"kind":"once","at":Time::new(now),"zone":"UTC"})).bind(now).execute(world.pool.pool()).await.unwrap();
            sqlx::query("INSERT INTO input_schedule_runs(id,schedule_id,occurrence,scheduled_for,state,thread_id,input_id,turn_id,topic_seq,created_at,started_at) VALUES($1,$2,'once',$3,'started',$4,$5,$6,0,$3,$3)")
                .bind(occurrence).bind(schedule).bind(now).bind(thread).bind(input).bind(start.turn_id).execute(world.pool.pool()).await.unwrap();
        }
        world
            .say(thread, command_asked(start.turn_id, "review"))
            .await;
        world.wait_state(thread, "waiting_approval").await;
        let approval: Id = world
            .scalar(
                "SELECT id FROM approvals WHERE thread_id=$1 AND kind='harness'",
                thread,
            )
            .await;
        assert_eq!(told(&world, approval).await, [expected]);
    }
}
