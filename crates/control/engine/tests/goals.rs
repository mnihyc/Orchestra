//! Goal mode: Orch keeps a thread working, turn after turn, in the name of the person who set the goal, until the
//! agent's report that it is met is taken, a limit that is set is reached, or a person stops it; and it waits for
//! whatever asks a person.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(
    dead_code,
    reason = "the helpers shared with the native-tool tests are not all used here"
)]

mod common;
mod native_tools;

use common::{World, eventually, meanwhile, turn_ended};
use orch_api_contract::goals::{
    GoalAction, GoalCheckedBy, GoalLimits, GoalReason, GoalStatus, GoalWaiting,
    ThreadGoalCreateBody, ThreadGoalUpdateBody,
};
use orch_authz::Caller;
use orch_engine::goals::GoalError;
use orch_link_contract::commands::{CommandKind, OrchTool, TurnStart};
use orch_link_contract::events::TurnOutcome;
use orch_types::{Id, new_id};
use serde_json::json;
use std::time::Duration;

async fn a_world_whose_agent_takes_tools() -> World {
    let world = World::new().await;
    native_tools::enable(&world).await;
    native_tools::existing_workspace(&world);
    world
}

async fn a_world_with_a_clock_whose_agent_takes_tools(
    clock: std::sync::Arc<orch_types::ManualClock>,
) -> World {
    let world = World::with_clock(clock).await;
    native_tools::enable(&world).await;
    native_tools::existing_workspace(&world);
    world
}

fn goal(objective: &str, condition: &str) -> ThreadGoalCreateBody {
    ThreadGoalCreateBody {
        objective: objective.to_owned(),
        condition: condition.to_owned(),
        limits: None,
        notify_on_end: false,
        plan: false,
    }
}

fn limited(objective: &str, condition: &str, turns: i64) -> ThreadGoalCreateBody {
    ThreadGoalCreateBody {
        limits: Some(GoalLimits {
            max_turns: Some(turns),
            ..GoalLimits::default()
        }),
        ..goal(objective, condition)
    }
}

fn update() -> ThreadGoalUpdateBody {
    ThreadGoalUpdateBody {
        objective: None,
        condition: None,
        limits: None,
        notify_on_end: None,
        action: None,
    }
}

/// One turn of the agent: it takes the input up, reports on the goal if it says so, and the turn ends.
async fn work(
    world: &World,
    thread: Id,
    nth: usize,
    report: Option<serde_json::Value>,
) -> TurnStart {
    let start = world.turn_start(thread, nth).await;
    world
        .answer(thread, &start, &format!("m{nth}"), "Working")
        .await;
    world.wait_turn(start.turn_id, "running").await;
    if let Some(arguments) = report {
        let call = format!("report-{nth}");
        native_tools::send(
            world,
            thread,
            start.turn_id,
            &call,
            OrchTool::Goal,
            arguments,
        )
        .await;
        let answer = native_tools::answer(world, thread, &call).await;
        assert!(answer.error.is_none(), "{answer:?}");
    }
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    start
}

async fn status(world: &World, thread: Id) -> GoalStatus {
    world
        .engine()
        .goal_of(thread)
        .await
        .unwrap()
        .expect("the thread has a goal")
        .status
}

async fn quiet(world: &World) {
    meanwhile(Duration::from_millis(600)).await;
    let _ = world;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_goal_runs_turn_after_turn_until_the_agents_report_that_it_is_met_is_taken() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("A goal").await;
    let ada = Caller::session(world.ada);
    let set = world
        .engine()
        .set_goal(
            &ada,
            thread,
            &goal("Make the tests pass", "npm test exits 0"),
        )
        .await
        .unwrap();
    assert_eq!(set.status, GoalStatus::Active);
    assert_eq!(set.set_by.as_ref().map(|person| person.id), Some(world.ada));
    // The first turn begins at once, in the name of the person who set the goal, framed as Orch's.
    let first = world.turn_start(thread, 0).await;
    assert!(
        first
            .text
            .contains("Goal: keep working until npm test exits 0"),
        "{}",
        first.text
    );
    assert!(first.text.contains("Turn 1"), "{}", first.text);
    assert!(first.text.contains("Objective: Make the tests pass"));
    work(
        &world,
        thread,
        0,
        Some(json!({"command":"update","status":"working","next":"Run the tests"})),
    )
    .await;
    // The next turn follows the end of the last, with its number.
    let second = world.turn_start(thread, 1).await;
    assert!(second.text.contains("Turn 2"), "{}", second.text);
    world.answer(thread, &second, "m1", "Working").await;
    world.wait_turn(second.turn_id, "running").await;
    native_tools::send(
        &world,
        thread,
        second.turn_id,
        "met",
        OrchTool::Goal,
        json!({"command":"update","status":"met","evidence":"npm test: 212 passed, 0 failed"}),
    )
    .await;
    assert!(
        native_tools::answer(&world, thread, "met")
            .await
            .error
            .is_none()
    );
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    eventually("the goal is met", || async {
        status(&world, thread).await == GoalStatus::Met
    })
    .await;
    quiet(&world).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        2,
        "nothing goes on after it is met"
    );
    let shown = world.engine().goal_of(thread).await.unwrap().unwrap();
    assert_eq!(shown.used.turns, 2);
    let report = shown.last_report.expect("the last report stands");
    assert_eq!(report.evidence, "npm test: 212 passed, 0 failed");
    assert_eq!(report.checked_by, Some(GoalCheckedBy::Agent));
    let lines: i64 = world
        .scalar(
            "SELECT count(*) FROM items WHERE thread_id=$1 AND kind='marker' AND payload->>'marker'='goal_met'",
            thread,
        )
        .await;
    assert_eq!(lines, 1, "one line of history says it was met");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_goal_tool_is_offered_only_while_the_thread_has_a_goal_under_way() {
    let world = a_world_whose_agent_takes_tools().await;
    let ada = Caller::session(world.ada);
    let without = world.thread("No goal").await;
    world
        .engine()
        .submit_input(&ada, without, common::queued("Start"))
        .await
        .unwrap();
    world.run_turn(without, 0, "Fine").await;
    let opened = |thread: Id| {
        world
            .seen
            .of_kind(CommandKind::SessionOpen)
            .into_iter()
            .filter_map(|command| match command {
                orch_link_contract::commands::Command::SessionOpen(open)
                    if open.thread_id == thread =>
                {
                    Some(open)
                }
                _ => None,
            })
            .next_back()
            .expect("a session was opened")
    };
    assert!(!opened(without).orch_tools.contains(&OrchTool::Goal));
    let with = world.thread("A goal").await;
    world
        .engine()
        .set_goal(&ada, with, &goal("Fix the build", "the build is green"))
        .await
        .unwrap();
    world.turn_start(with, 0).await;
    eventually("the session of the goal's thread is opened", || async {
        world
            .seen
            .of_kind(CommandKind::SessionOpen)
            .iter()
            .any(|command| matches!(command, orch_link_contract::commands::Command::SessionOpen(open) if open.thread_id == with))
    })
    .await;
    assert!(opened(with).orch_tools.contains(&OrchTool::Goal));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_limit_that_is_set_stops_the_goal_and_raising_it_lets_a_person_resume() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("Limited").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .set_goal(&ada, thread, &limited("Fix it", "it is fixed", 2))
        .await
        .unwrap();
    work(&world, thread, 0, None).await;
    work(&world, thread, 1, None).await;
    eventually("the goal stops at its turn limit", || async {
        status(&world, thread).await == GoalStatus::Stopped
    })
    .await;
    let stopped = world.engine().goal_of(thread).await.unwrap().unwrap();
    assert_eq!(stopped.reason, Some(GoalReason::Turns));
    assert_eq!(stopped.used.turns, 2);
    quiet(&world).await;
    assert_eq!(world.turn_starts(thread).len(), 2);
    let resumed = world
        .engine()
        .update_goal(
            &ada,
            thread,
            &ThreadGoalUpdateBody {
                action: Some(GoalAction::Resume),
                ..update()
            },
        )
        .await;
    assert!(
        matches!(resumed, Err(GoalError::LimitReached)),
        "a limit that is still reached is not resumed past: {resumed:?}"
    );
    world
        .engine()
        .update_goal(
            &ada,
            thread,
            &ThreadGoalUpdateBody {
                limits: Some(GoalLimits {
                    max_turns: Some(3),
                    ..GoalLimits::default()
                }),
                action: Some(GoalAction::Resume),
                ..update()
            },
        )
        .await
        .unwrap();
    let third = world.turn_start(thread, 2).await;
    assert!(third.text.contains("Turn 3 of up to 3"), "{}", third.text);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_pauses_resumes_and_clears_the_goal() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("Held").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .set_goal(&ada, thread, &goal("Keep going", "never done"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.answer(thread, &first, "m0", "Working").await;
    world.wait_turn(first.turn_id, "running").await;
    let paused = world
        .engine()
        .update_goal(
            &ada,
            thread,
            &ThreadGoalUpdateBody {
                action: Some(GoalAction::Pause),
                ..update()
            },
        )
        .await
        .unwrap();
    assert_eq!(paused.status, GoalStatus::Paused);
    assert_eq!(paused.reason, Some(GoalReason::Person));
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(first.turn_id, "completed").await;
    quiet(&world).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "a paused goal queues nothing"
    );
    world
        .engine()
        .update_goal(
            &ada,
            thread,
            &ThreadGoalUpdateBody {
                action: Some(GoalAction::Resume),
                ..update()
            },
        )
        .await
        .unwrap();
    let second = world.turn_start(thread, 1).await;
    assert!(second.text.contains("Turn 2"), "{}", second.text);
    world.answer(thread, &second, "m1", "Working").await;
    world.wait_turn(second.turn_id, "running").await;
    let cleared = world.engine().clear_goal(&ada, thread).await.unwrap();
    assert_eq!(cleared.status, GoalStatus::Cleared);
    assert!(world.engine().goal_of(thread).await.unwrap().is_none());
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(second.turn_id, "completed").await;
    quiet(&world).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        2,
        "a cleared goal queues nothing"
    );
    // And the thread takes a new goal.
    world
        .engine()
        .set_goal(&ada, thread, &goal("Another", "another is done"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_goal_is_refused_while_one_is_under_way_and_somebody_who_may_not_drive_cannot_set_one()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("One at a time").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .set_goal(&ada, thread, &goal("First", "first is done"))
        .await
        .unwrap();
    let again = world
        .engine()
        .set_goal(&ada, thread, &goal("Second", "second is done"))
        .await;
    assert!(matches!(again, Err(GoalError::Exists)), "{again:?}");
    let stranger = world.person("stranger").await;
    let denied = world
        .engine()
        .update_goal(
            &Caller::session(stranger),
            thread,
            &ThreadGoalUpdateBody {
                action: Some(GoalAction::Pause),
                ..update()
            },
        )
        .await;
    assert!(matches!(denied, Err(GoalError::Denied(_))), "{denied:?}");
    let empty = world
        .engine()
        .set_goal(&ada, world.thread("Empty").await, &goal("  ", "done"))
        .await;
    assert!(matches!(empty, Err(GoalError::Invalid(_))), "{empty:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_goal_waits_while_something_asks_a_person_and_goes_on_when_it_is_answered() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("Waiting").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .set_goal(&ada, thread, &goal("Deploy it", "it is deployed"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.answer(thread, &first, "m0", "Working").await;
    world.wait_turn(first.turn_id, "running").await;
    // A question that no turn owns waits for a person when the turn ends.
    let approval = new_id();
    sqlx::query(
        "INSERT INTO approvals (id, kind, state, node_id, thread_id, title, request, required_permission)
         VALUES ($1, 'capability', 'pending', $2, $2, 'A question', '{}', 'thread.interact')",
    )
    .bind(approval)
    .bind(thread)
    .execute(world.pool.pool())
    .await
    .unwrap();
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(first.turn_id, "completed").await;
    quiet(&world).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "the goal does not go on over a question"
    );
    let shown = world.engine().goal_of(thread).await.unwrap().unwrap();
    assert_eq!(shown.status, GoalStatus::Active);
    assert_eq!(shown.waiting, Some(GoalWaiting::Approval));
    sqlx::query("UPDATE approvals SET state = 'approved' WHERE id = $1")
        .bind(approval)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().wake(thread);
    let second = world.turn_start(thread, 1).await;
    assert!(second.text.contains("Turn 2"), "{}", second.text);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_stop_pauses_the_goal_and_an_agent_that_is_blocked_pauses_it_with_its_words() {
    let world = a_world_whose_agent_takes_tools().await;
    let ada = Caller::session(world.ada);
    let stopped = world.thread("Stopped").await;
    world
        .engine()
        .set_goal(&ada, stopped, &goal("Stop me", "never"))
        .await
        .unwrap();
    let first = world.turn_start(stopped, 0).await;
    world.answer(stopped, &first, "m0", "Working").await;
    world.wait_turn(first.turn_id, "running").await;
    world
        .engine()
        .interrupt(&ada, stopped, first.turn_id)
        .await
        .unwrap();
    world
        .say(stopped, turn_ended(first.turn_id, TurnOutcome::Interrupted))
        .await;
    eventually("the goal is paused by the stop", || async {
        status(&world, stopped).await == GoalStatus::Paused
    })
    .await;
    assert_eq!(
        world
            .engine()
            .goal_of(stopped)
            .await
            .unwrap()
            .unwrap()
            .reason,
        Some(GoalReason::Interrupted)
    );
    let blocked = world.thread("Blocked").await;
    world
        .engine()
        .set_goal(&ada, blocked, &goal("Needs a key", "it works"))
        .await
        .unwrap();
    work(
        &world,
        blocked,
        0,
        Some(json!({"command":"update","status":"blocked","evidence":"The API key is missing","next":"Give me a key"})),
    )
    .await;
    eventually("the goal is paused for the agent's report", || async {
        status(&world, blocked).await == GoalStatus::Paused
    })
    .await;
    let shown = world.engine().goal_of(blocked).await.unwrap().unwrap();
    assert_eq!(shown.reason, Some(GoalReason::Blocked));
    assert!(shown.detail.unwrap().contains("The API key is missing"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_limit_of_minutes_stops_the_goal_when_the_time_it_was_active_reaches_it() {
    let clock = std::sync::Arc::new(orch_testkit::clock::manual());
    let world = a_world_with_a_clock_whose_agent_takes_tools(clock.clone()).await;
    let thread = world.thread("Timed").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .set_goal(
            &ada,
            thread,
            &ThreadGoalCreateBody {
                limits: Some(GoalLimits {
                    max_minutes: Some(10),
                    ..GoalLimits::default()
                }),
                ..goal("Take your time", "never done")
            },
        )
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.answer(thread, &first, "m0", "Working").await;
    world.wait_turn(first.turn_id, "running").await;
    clock.advance(chrono::Duration::minutes(11));
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    eventually("the goal stops at its minutes", || async {
        status(&world, thread).await == GoalStatus::Stopped
    })
    .await;
    let stopped = world.engine().goal_of(thread).await.unwrap().unwrap();
    assert_eq!(stopped.reason, Some(GoalReason::Minutes));
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_limit_of_credits_stops_the_goal_when_what_it_has_used_reaches_it() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("Priced").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .set_goal(
            &ada,
            thread,
            &ThreadGoalCreateBody {
                limits: Some(GoalLimits {
                    max_credits: Some("1.5".to_owned()),
                    ..GoalLimits::default()
                }),
                ..goal("Spend little", "never done")
            },
        )
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.answer(thread, &first, "m0", "Working").await;
    world.wait_turn(first.turn_id, "running").await;
    // What the ledger counted for the turn: two credits, past the one and a half.
    sqlx::query(
        "UPDATE thread_goals SET credits_used = 2 WHERE thread_id = $1 AND status = 'active'",
    )
    .bind(thread)
    .execute(world.pool.pool())
    .await
    .unwrap();
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    eventually("the goal stops at its credits", || async {
        status(&world, thread).await == GoalStatus::Stopped
    })
    .await;
    let stopped = world.engine().goal_of(thread).await.unwrap().unwrap();
    assert_eq!(stopped.reason, Some(GoalReason::Credits));
    assert_eq!(stopped.budgets.max_credits, "1.5");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_goal_that_has_ended_leaves_the_line_at_the_next_persons_input() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("Leaves").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .set_goal(&ada, thread, &limited("One turn", "never done", 1))
        .await
        .unwrap();
    work(&world, thread, 0, None).await;
    eventually("the goal stops at its turn limit", || async {
        status(&world, thread).await == GoalStatus::Stopped
    })
    .await;
    world
        .engine()
        .submit_input(&ada, thread, common::queued("Now something else"))
        .await
        .unwrap();
    assert!(
        world.engine().goal_of(thread).await.unwrap().is_none(),
        "a person's input takes a goal that ended away"
    );
}

/// What the planner says it will do for `sql` with one id bound, as lines of text.
async fn plan_of(world: &World, sql: &str, id: Id) -> String {
    sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!("EXPLAIN {sql}")))
        .bind(id)
        .fetch_all(world.pool.pool())
        .await
        .unwrap()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_goal_with_two_hundred_thousand_reports_and_inputs_is_read_by_a_page_and_by_index() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("A goal of days").await;
    let ada = Caller::session(world.ada);
    let set = world
        .engine()
        .set_goal(&ada, thread, &goal("Keep at it", "never done"))
        .await
        .unwrap();
    // Let the harness begin before the bulk fixture work: an unanswered start is
    // still subject to the ordinary start deadline while SQL is seeding history.
    let first = world.turn_start(thread, 0).await;
    world.answer(thread, &first, "m0", "Working").await;
    world.wait_turn(first.turn_id, "running").await;
    let pool = world.pool.pool();
    // A goal that has run for days: reports and inputs by the hundred thousand, made by SQL in one statement each.
    sqlx::query(
        "INSERT INTO thread_goal_reports (id, goal_id, thread_id, status, evidence, outcome, decided_at, created_at)
         SELECT gen_random_uuid(), $1, $2, 'working', 'x', 'noted', now(), now() - make_interval(secs => n)
           FROM generate_series(1, 200000) n",
    )
    .bind(set.id)
    .bind(thread)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO thread_inputs (id, thread_id, continuation_id, source, content, delivery, state, position, seq_updated, goal_id)
         SELECT gen_random_uuid(), t.node_id, t.active_continuation_id, 'goal', '[]', 'queue', 'delivered', n + 1000, 0, $1
           FROM threads t, generate_series(1, 200000) n WHERE t.node_id = $2",
    )
    .bind(set.id)
    .bind(thread)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE thread_goal_reports")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("ANALYZE thread_inputs")
        .execute(pool)
        .await
        .unwrap();
    // What the agent's tool and the screen read is a page of reports, and the newest one.
    let mut conn = world.pool.acquire().await.unwrap();
    let page =
        orch_db::queries::goals::reports(&mut conn, set.id, orch_engine::goals::REPORTS_SHOWN)
            .await
            .unwrap();
    assert_eq!(page.len(), 10, "the agent is given a page, never all");
    drop(conn);
    // Neither read, nor the count of lost inputs the loop makes before each turn, reads a table in full.
    for sql in [
        "SELECT id FROM thread_goal_reports WHERE goal_id = $1 ORDER BY created_at DESC, id DESC LIMIT 10",
        "SELECT count(*) FROM thread_inputs WHERE goal_id = $1 AND state IN ('cancelled','failed')",
    ] {
        let plan = plan_of(&world, sql, set.id).await;
        assert!(!plan.contains("Seq Scan"), "{sql}\n{plan}");
    }
    // And the loop goes on at that size: the first turn ends, the next is queued with its number.
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(first.turn_id, "completed").await;
    let next = world.turn_start(thread, 1).await;
    assert!(next.text.contains("Turn 2"), "{}", next.text);
    let shown = world.engine().goal_of(thread).await.unwrap().unwrap();
    assert!(shown.last_report.is_some());
}
