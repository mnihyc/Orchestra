//! A saved goal choice cannot bypass a place's unlocked policy limits.
use super::*;
use orch_settings_contract::registry::keys;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saved_goal_limits_remain_editable_but_only_tighten_a_places_unlocked_bounds() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("Bounded goal").await;
    let ada = Caller::session(world.ada);
    world
        .set(world.root, keys::THREAD_GOAL_MAX_TURNS, json!(2))
        .await;
    world
        .set(world.root, keys::THREAD_GOAL_MAX_MINUTES, json!(10))
        .await;
    world
        .set(world.root, keys::THREAD_GOAL_MAX_CREDITS, json!("2.5"))
        .await;
    let created = world
        .engine()
        .set_goal(
            &ada,
            thread,
            &ThreadGoalCreateBody {
                limits: Some(GoalLimits {
                    max_turns: Some(20),
                    max_minutes: Some(60),
                    max_credits: Some("9".into()),
                }),
                ..goal("A saved choice", "never done")
            },
        )
        .await
        .unwrap();
    assert_eq!(created.limits.max_turns, Some(20));
    assert_eq!(created.limits.max_minutes, Some(60));
    assert_eq!(created.limits.max_credits.as_deref(), Some("9"));
    assert_eq!(created.budgets.max_turns, 2);
    assert_eq!(created.budgets.max_minutes, 10);
    assert_eq!(created.budgets.max_credits, "2.5");
    assert!(
        !created.budgets.locked,
        "narrowing does not pretend to be a lock"
    );

    world
        .set(world.root, keys::THREAD_GOAL_MAX_TURNS, json!(50))
        .await;
    world
        .set(world.root, keys::THREAD_GOAL_MAX_MINUTES, json!(100))
        .await;
    world
        .set(world.root, keys::THREAD_GOAL_MAX_CREDITS, json!("20"))
        .await;
    let relaxed = world.engine().goal_of(thread).await.unwrap().unwrap();
    assert_eq!(relaxed.budgets.max_turns, 20);
    assert_eq!(relaxed.budgets.max_minutes, 60);
    assert_eq!(relaxed.budgets.max_credits, "9");
    assert_eq!(
        relaxed.limits, created.limits,
        "relaxing a parent does not rewrite the saved choice"
    );

    world
        .set(world.root, keys::THREAD_GOAL_MAX_TURNS, json!(2))
        .await;
    world
        .set(world.root, keys::THREAD_GOAL_MAX_MINUTES, json!(10))
        .await;
    world
        .set(world.root, keys::THREAD_GOAL_MAX_CREDITS, json!("2.5"))
        .await;
    let tighter = world
        .engine()
        .update_goal(
            &ada,
            thread,
            &ThreadGoalUpdateBody {
                limits: Some(GoalLimits {
                    max_turns: Some(1),
                    max_minutes: Some(5),
                    max_credits: Some("1.5".into()),
                }),
                ..update()
            },
        )
        .await
        .unwrap();
    assert_eq!(tighter.budgets.max_turns, 1);
    assert_eq!(tighter.budgets.max_minutes, 5);
    assert_eq!(tighter.budgets.max_credits, "1.5");
    let unbounded_choice = world
        .engine()
        .update_goal(
            &ada,
            thread,
            &ThreadGoalUpdateBody {
                limits: Some(GoalLimits {
                    max_turns: Some(0),
                    max_minutes: Some(0),
                    max_credits: Some("0".into()),
                }),
                ..update()
            },
        )
        .await
        .unwrap();
    assert_eq!(unbounded_choice.limits.max_turns, Some(0));
    assert_eq!(unbounded_choice.budgets.max_turns, 2);
    assert_eq!(unbounded_choice.budgets.max_minutes, 10);
    assert_eq!(unbounded_choice.budgets.max_credits, "2.5");
    world
        .set(world.root, keys::THREAD_GOAL_MAX_TURNS, json!(0))
        .await;
    world
        .set(world.root, keys::THREAD_GOAL_MAX_MINUTES, json!(0))
        .await;
    world
        .set(world.root, keys::THREAD_GOAL_MAX_CREDITS, json!("0"))
        .await;
    let unbounded = world.engine().goal_of(thread).await.unwrap().unwrap();
    assert_eq!(unbounded.budgets.max_turns, 0);
    assert_eq!(unbounded.budgets.max_minutes, 0);
    assert_eq!(unbounded.budgets.max_credits, "0");
}
