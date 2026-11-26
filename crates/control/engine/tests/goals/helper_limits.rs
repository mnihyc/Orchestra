//! The helper uses the chosen reading and explanation defaults for the real request.
use super::*;
use orch_link_contract::commands::{Command, CommandResult, ProviderCompleteResult};
use std::sync::{Arc, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_goal_helpers_chosen_item_and_reason_limits_apply_to_the_call_and_its_kept_report() {
    check_helper_limits(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_goal_checks_its_real_report_with_one_paid_model_pool_member() {
    check_helper_limits(true).await;
}

async fn check_helper_limits(use_pool: bool) {
    let world = World::on_a_key("synthetic-goal-defaults-key").await;
    native_tools::enable(&world).await;
    native_tools::existing_workspace(&world);
    let backend = json!({"route":"summarizer-model.complete","offering":{"provider":world.provider,"offering":common::MODEL}});
    let pool = if use_pool {
        Some(
            world
                .summary_model_pool("Goal helpers", &backend)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let producer = match pool {
        Some(id) => json!({"kind":"model","target":{"type":"pool","kind":"model","id":id}}),
        None => json!({"kind":"model","backend":backend}),
    };
    world.set(world.topic, "auto.summarizer", producer).await;
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
    if let Some(pool) = pool {
        let paid: Vec<Id> = sqlx::query_scalar("SELECT p.actor_id FROM pool_choices p JOIN usage_events c ON c.id=p.id AND c.purpose='summary' AND c.user_id IS NULL WHERE p.target->>'id'=$1 AND p.source->>'report'=$2 AND p.state='selected' AND c.thread_id=$3 AND c.provider_id=$4")
            .bind(pool.to_string()).bind(report.id.to_string()).bind(thread).bind(world.provider)
            .fetch_all(world.pool.pool()).await.unwrap();
        assert_eq!(
            paid,
            [world.ada],
            "the verdict belongs to the confirmed report and its payer"
        );
    }
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
