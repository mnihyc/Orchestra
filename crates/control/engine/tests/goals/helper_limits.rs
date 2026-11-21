//! The helper uses the chosen reading and explanation defaults for the real request.
use super::*;
use orch_link_contract::commands::{Command, CommandResult, ProviderCompleteResult};
use std::sync::{Arc, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_goal_helpers_chosen_item_and_reason_limits_apply_to_the_call_and_its_kept_report() {
    let world = World::on_a_key("synthetic-goal-defaults-key").await;
    native_tools::enable(&world).await;
    native_tools::existing_workspace(&world);
    world
        .set(
            world.topic,
            "auto.summarizer",
            json!({"kind":"model","backend":{"route":"summarizer-model.complete","offering":{"provider":world.provider,"offering":common::MODEL}}}),
        )
        .await;
    world
        .set(world.topic, "summary.goal_turn_items", json!(1))
        .await;
    world
        .set(world.topic, "summary.goal_reason_chars", json!(17))
        .await;
    sqlx::query("UPDATE settings SET updated_by=$2 WHERE node_id=$1 AND key='auto.summarizer'")
        .bind(world.topic)
        .bind(world.ada)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |command| {
            if let Command::ProviderComplete(request) = command {
                observed.lock().unwrap().push(request.clone());
            }
            Ok(CommandResult::ProviderComplete(ProviderCompleteResult {
                text: format!("not_yet\n{}", "More evidence is needed. ".repeat(40)),
                usage: Default::default(),
                request_ref: Some("synthetic-goal-defaults".into()),
                timing: Default::default(),
            }))
        });
    let thread = world.thread("A goal with chosen helper defaults").await;
    world
        .engine()
        .set_goal(
            &Caller::session(world.ada),
            thread,
            &goal("Finish the change", "the check passes"),
        )
        .await
        .unwrap();
    work(
        &world,
        thread,
        0,
        Some(json!({"command":"update","status":"met","evidence":"A check still needs evidence"})),
    )
    .await;
    eventually("the helper's bounded reason is kept", || async {
        world
            .engine()
            .goal_of(thread)
            .await
            .unwrap()
            .unwrap()
            .last_report
            .is_some_and(|report| {
                report.outcome == Some(orch_api_contract::goals::GoalOutcome::NotYet)
            })
    })
    .await;
    let report = world
        .engine()
        .goal_of(thread)
        .await
        .unwrap()
        .unwrap()
        .last_report
        .unwrap();
    assert_eq!(report.reason.as_ref().unwrap().chars().count(), 17);
    let captured = serde_json::to_value(&report).unwrap();
    assert_eq!(captured["helper"]["last_turn"], json!("items_exceeded"));
    assert!(
        captured["helper"]["input_allowance_tokens"]
            .as_i64()
            .is_some_and(|tokens| tokens > 0)
    );
    assert_eq!(captured["helper"]["usage_owner"]["id"], json!(world.ada));
    assert!(
        captured["helper"]["usage"].is_null(),
        "an absent native measurement does not become zero tokens"
    );
    {
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            captured["helper"]["output_allowance_tokens"],
            json!(sent[0].max_output_tokens)
        );
        assert!(
            sent[0].input.contains("more than 1 items"),
            "the last turn exceeds the chosen item allowance"
        );
    }
    world
        .set(world.topic, "summary.goal_reason_chars", json!(2400))
        .await;
    let later = world
        .engine()
        .goal_of(thread)
        .await
        .unwrap()
        .unwrap()
        .last_report
        .unwrap();
    assert_eq!(
        later.reason, report.reason,
        "a later default never rewrites the recorded outcome"
    );
    assert_eq!(
        serde_json::to_value(later).unwrap()["helper"],
        captured["helper"]
    );
}
