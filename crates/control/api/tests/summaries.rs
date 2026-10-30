//! Configured live summaries update automatic fields, preserve people's edits and charge real requests without delaying a turn.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::directory::{NodeTagCreate, NodeTagCreateBody};
use orch_api_contract::forks::{
    ForkHistory, ForkPlan, ForkRequest, ForkResult, HistoryMode, ThreadForkPreflight,
};
use orch_api_contract::goals::{GoalCheckedBy, GoalStatus, ThreadGoalCreateBody};
use orch_api_contract::nodes::{
    AutoAction, AutoField, Node, NodeAuto, NodeAutoBody, NodeUpdate, NodeUpdateBody,
};
use orch_api_contract::settings::{NodeSettingList, NodeSettingSet, NodeSettingSetBody};
use orch_api_contract::threads::Delivery;
use orch_authz::Caller;
use orch_engine::summaries::{handoff, indexed_transcript};
use orch_link_contract::commands::{
    Command, CommandKind, CommandResult, ProviderComplete, ProviderCompleteResult,
};
use orch_link_contract::events::TurnOutcome;
use orch_settings::Target;
use orch_types::{Id, Json, new_id};
use rig::{MODEL, Scene, eventually, turn_began, turn_ended};
use serde_json::json;
use std::sync::{Arc, Mutex};

async fn configure(s: &Scene, value: Json) {
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&s.tree.topic.base.id, &"auto.summarizer"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value,
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}

async fn resume_title(s: &Scene, thread: Id) {
    s.rig
        .world
        .app
        .call(NodeAuto, &[&thread])
        .token(&s.ada.token)
        .body(&NodeAutoBody {
            field: AutoField::Title,
            action: AutoAction::Resume,
        })
        .send()
        .await
        .ok();
}

async fn api_provider(s: &Scene) -> Id {
    let provider = new_id();
    // An API provider names at least one upstream (the schema's own check); the call itself reaches the machine.
    sqlx::query("INSERT INTO providers (id,name,owner_id,org_id,home_id,kind,key_machine_id,machine_id,protocols,arrangements,status,upstreams) VALUES ($1,'Summary API',$2,$3,$3,'api',$4,$4,ARRAY['openai-responses'],ARRAY['key_separated'],'ready',$5)")
        .bind(provider).bind(s.ada.id()).bind(s.tree.org.base.id).bind(s.rig.machine_id)
        .bind(json!([{"protocol":"openai-responses","base_url":"https://api.example.test","options":{}}]))
        .execute(s.rig.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO provider_models (id,provider_id,model_name,display_name,enabled,source,created_by) VALUES ($1,$2,$3,'Small',true,'manual',$4)")
        .bind(new_id()).bind(provider).bind(MODEL).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .lets_use("provider", provider, "access.own", &s.ada, json!({}))
        .await;
    provider
}

fn script(s: &Scene, malformed: bool) -> Arc<Mutex<Vec<ProviderComplete>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |command| {
            let Command::ProviderComplete(request) = command else {
                panic!("only a completion reaches this script");
            };
            recorded.lock().unwrap().push(request.clone());
            let text = if malformed {
                "   "
            } else if request.system.starts_with("You name") {
                "Repair login redirects"
            } else if request
                .system
                .starts_with("You keep a one-line status of a topic")
            {
                "Login fixed; verification remains"
            } else if request.system.starts_with("From <tags>") {
                "[\"backend\",\"invented\",\"backend\"]"
            } else if request.system.starts_with("You hand") {
                "The fix is in web/login.ts. The redirect now works; run its regression test next."
            } else {
                "The redirect test now passes"
            };
            Ok(CommandResult::ProviderComplete(ProviderCompleteResult {
                timing: Default::default(),
                text: text.to_owned(),
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    total_tokens: 120,
                    ..Default::default()
                },
                request_ref: Some(format!("summary-{}", recorded.lock().unwrap().len())),
            }))
        });
    seen
}

async fn attempts(s: &Scene, node: Id) -> (i64, Option<String>) {
    sqlx::query_as("SELECT failures,reason FROM node_summary_state WHERE node_id=$1")
        .bind(node)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

async fn wait_attempt(s: &Scene, node: Id) {
    eventually("the independent summary attempt finishes", || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM node_summary_state WHERE node_id=$1 AND last_at IS NOT NULL AND claimed_by IS NULL)")
            .bind(node).fetch_one(s.rig.db.pool()).await.unwrap()
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switching_off_each_source_or_deleting_its_provider_stops_an_accepted_summary_before_its_reply()
 {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_api_contract::environment::EnvironmentDisable;
    use orch_api_contract::resource_facts::FactTarget;
    use orch_api_contract::resources::{
        AgentUpdate, AgentUpdateBody, ProviderDelete, ProviderDisable, ProviderModelUpdate,
        ProviderModelUpdateBody,
    };
    use orch_link_contract::commands::Empty;
    for kind in ["provider", "model", "agent", "environment", "delete"] {
        let s = Scene::new().await;
        let provider = api_provider(&s).await;
        let replies = script(&s, false);
        s.rig.machine.handler.on(CommandKind::ProviderStop, |_| {
            Ok(CommandResult::ProviderStop(Empty {}))
        });
        s.rig.machine.handler.on(CommandKind::AgentStop, |_| {
            Ok(CommandResult::AgentStop(Empty {}))
        });
        configure(
            &s,
            json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
        )
        .await;
        s.rig
            .world
            .set_at(
                Target::Node(s.tree.topic.base.id),
                "auto.title",
                json!("summarizer"),
            )
            .await;
        let thread = s.thread().await.base.id;
        resume_title(&s, thread).await;
        s.rig.machine.handler.hold(CommandKind::ProviderComplete);
        s.exchange(thread, 0, "Keep the accepted summary", "The work is done")
            .await;
        eventually("the accepted summary is running on its machine", || async {
            s.rig
                .machine
                .handler
                .executions()
                .iter()
                .filter(|entry| entry.kind == CommandKind::ProviderComplete)
                .count()
                == 1
        })
        .await;
        let request = s
            .rig
            .machine
            .handler
            .executions()
            .into_iter()
            .find(|entry| entry.kind == CommandKind::ProviderComplete)
            .unwrap()
            .id;
        let source: (Option<Id>, Option<Id>) =
            sqlx::query_as("SELECT agent_id,environment_id FROM summary_calls WHERE id=$1")
                .bind(request)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(
            source,
            (Some(s.on.agent), Some(s.on.environment)),
            "the accepted request captures its actual source: {kind}"
        );
        let model: Id = sqlx::query_scalar(
            "SELECT id FROM provider_models WHERE provider_id=$1 AND model_name=$2",
        )
        .bind(provider)
        .bind(MODEL)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
        let target = FactTarget {
            object: ObjectRef {
                kind: match kind {
                    "agent" => ObjectKind::Agent,
                    "environment" => ObjectKind::Environment,
                    _ => ObjectKind::Provider,
                },
                id: match kind {
                    "agent" => s.on.agent,
                    "environment" => s.on.environment,
                    _ => provider,
                },
            },
            model_id: (kind == "model").then_some(model),
        };
        let found = orch_engine::resource_facts::activity::requests(
            &mut s.rig.db.acquire().await.unwrap(),
            &target,
        )
        .await
        .unwrap();
        assert_eq!(
            found,
            [request],
            "the shared facts source names the real accepted request: {kind}"
        );
        let Node::Thread(before) = places::read(&s.rig.world, &s.ada, thread).await else {
            panic!("a thread")
        };
        match kind {
            "provider" => {
                s.rig
                    .world
                    .app
                    .call(ProviderDisable, &[&provider])
                    .token(&s.ada.token)
                    .send()
                    .await
                    .ok();
            }
            "delete" => {
                s.rig
                    .world
                    .app
                    .call(ProviderDelete, &[&provider])
                    .token(&s.ada.token)
                    .send()
                    .await
                    .ok();
            }
            "model" => {
                let body: ProviderModelUpdateBody =
                    serde_json::from_value(json!({"enabled":false})).unwrap();
                s.rig
                    .world
                    .app
                    .call(ProviderModelUpdate, &[&provider, &model])
                    .token(&s.ada.token)
                    .body(&body)
                    .send()
                    .await
                    .ok();
            }
            "agent" => {
                let revision: i64 = sqlx::query_scalar("SELECT revision FROM agents WHERE id=$1")
                    .bind(s.on.agent)
                    .fetch_one(s.rig.db.pool())
                    .await
                    .unwrap();
                let body: AgentUpdateBody =
                    serde_json::from_value(json!({"expected_revision":revision,"disabled":true}))
                        .unwrap();
                s.rig
                    .world
                    .app
                    .call(AgentUpdate, &[&s.on.agent])
                    .token(&s.ada.token)
                    .body(&body)
                    .send()
                    .await
                    .ok();
            }
            _ => {
                s.rig
                    .world
                    .app
                    .call(EnvironmentDisable, &[&s.on.environment])
                    .token(&s.ada.token)
                    .send()
                    .await
                    .ok();
            }
        }
        wait_attempt(&s, thread).await;
        let reason = match kind {
            "model" => "Model switched off",
            "agent" => "Agent profile switched off",
            "environment" => "Environment switched off",
            "delete" => "Provider removed",
            _ => "Provider switched off",
        };
        assert_eq!(
            attempts(&s, thread).await,
            (0, Some(reason.into())),
            "a deliberate pause is not a producer failure: {kind}"
        );
        assert!(
            replies.lock().unwrap().is_empty(),
            "the held native call ends without releasing its reply: {kind}"
        );
        let Node::Thread(after) = places::read(&s.rig.world, &s.ada, thread).await else {
            panic!("a thread")
        };
        assert_eq!(
            after.base.name, before.base.name,
            "no stopped summary changes the field: {kind}"
        );
        let mut conn = s.rig.db.acquire().await.unwrap();
        assert!(
            !orch_engine::resource_facts::activity::accepts_context(
                &mut conn,
                provider,
                MODEL,
                Some(s.on.agent),
                Some(s.on.environment)
            )
            .await
            .unwrap(),
            "new auxiliary work is refused too: {kind}"
        );
        s.rig.machine.handler.release();
        assert!(replies.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn an_accepted_summary_id_cannot_revive_when_its_provider_is_available_again() {
    use orch_api_contract::resources::ProviderDisable;
    use orch_link_contract::commands::Empty;
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    s.rig.machine.handler.on(CommandKind::ProviderStop, |_| {
        Ok(CommandResult::ProviderStop(Empty {}))
    });
    let id = new_id();
    sqlx::query("INSERT INTO summary_calls(id,node_id,topic_id,provider_id,person_id,model_name,created_at) VALUES($1,$2,$2,$3,$4,$5,$6)")
        .bind(id).bind(s.tree.topic.base.id).bind(provider).bind(s.ada.id()).bind(MODEL).bind(s.rig.world.now()).execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .world
        .app
        .call(ProviderDisable, &[&provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let mut conn = s.rig.db.acquire().await.unwrap();
    assert_eq!(
        orch_engine::resource_facts::activity::request_stop_reason(&mut conn, id)
            .await
            .unwrap()
            .as_deref(),
        Some("Provider switched off")
    );
    // The scripted provider's ready state stands for its fake credential keeper, as in api_provider.
    sqlx::query("UPDATE providers SET status='ready' WHERE id=$1")
        .bind(provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        orch_engine::resource_facts::activity::accepts_request(&mut conn, provider, MODEL)
            .await
            .unwrap()
    );
    assert!(
        !orch_engine::resource_facts::activity::accepts_command(&mut conn, id, provider, MODEL)
            .await
            .unwrap()
    );
    assert!(
        orch_engine::resource_facts::activity::accepts_command(
            &mut conn,
            new_id(),
            provider,
            MODEL
        )
        .await
        .unwrap()
    );
    assert_eq!(
        s.rig.commands.issued_of("provider.complete"),
        0,
        "the accepted identity was stopped before any native dispatch"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_turn_updates_auto_fields_defined_tags_and_the_topic_and_counts_each_actual_request()
 {
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    let calls = script(&s, false);
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    for (key, value) in [
        ("auto.title", json!("summarizer")),
        ("auto.description", json!("summarizer")),
        ("auto.tags", json!("summarizer")),
        ("auto.topic_rollup", json!("rollup")),
    ] {
        s.rig
            .world
            .set_at(Target::Node(s.tree.topic.base.id), key, value)
            .await;
    }
    let tag = s
        .rig
        .world
        .app
        .call(NodeTagCreate, &[&s.tree.project.base.id])
        .token(&s.ada.token)
        .body(&NodeTagCreateBody {
            name: "backend".to_owned(),
            color: Some(2),
        })
        .send()
        .await
        .ok();
    let id = s.thread().await.base.id;
    resume_title(&s, id).await;
    s.exchange(id, 0, "Fix login", "The redirect is repaired")
        .await;
    wait_attempt(&s, id).await;
    wait_attempt(&s, s.tree.topic.base.id).await;
    let Node::Thread(thread) = places::read(&s.rig.world, &s.ada, id).await else {
        panic!("the conversation remains a thread");
    };
    assert_eq!(thread.base.name, "Repair login redirects");
    assert_eq!(thread.base.description, "The redirect test now passes");
    assert!(thread.base.auto_updated_at.is_some());
    let Node::Topic(topic) = places::read(&s.rig.world, &s.ada, s.tree.topic.base.id).await else {
        panic!("the topic remains one");
    };
    assert_eq!(topic.base.description, "Login fixed; verification remains");
    assert_eq!(
        topic.base.tags.iter().map(|tag| tag.id).collect::<Vec<_>>(),
        [tag.id]
    );
    // One request for each field automation keeps on each node: the thread's title, status and tags,
    // and the topic's status, rolled up from its threads, and its own tags.
    let made: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.system.clone())
        .collect();
    let of = |prefix: &str| {
        made.iter()
            .filter(|system| system.starts_with(prefix))
            .count()
    };
    assert_eq!(
        (
            of("You name"),
            of("You keep a one-line status for a team"),
            of("You keep a one-line status of a topic"),
            of("From <tags>"),
            made.len()
        ),
        (1, 1, 1, 2, 5),
        "{made:#?}"
    );
    let mut recorded = Vec::new();
    let mut per_node = Vec::new();
    for node in [id, s.tree.topic.base.id] {
        let page = s
            .rig
            .world
            .app
            .call(
                orch_api_contract::model_inputs::NodeModelInputList,
                &[&node],
            )
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        let before = recorded.len();
        for record in page.items.into_iter().filter(|record| {
            record.purpose == orch_api_contract::model_inputs::ModelInputPurpose::Summary
        }) {
            assert_eq!(
                record.state,
                orch_api_contract::model_inputs::ModelInputState::Sent
            );
            assert_eq!(
                record
                    .blocks
                    .iter()
                    .map(|block| block.key.as_str())
                    .collect::<Vec<_>>(),
                ["summary_system", "summary_input"]
            );
            recorded.push((
                record.blocks[0].text.inline.clone(),
                record.blocks[1].text.inline.clone(),
            ));
        }
        per_node.push(recorded.len() - before);
    }
    assert_eq!(
        per_node,
        [3, 2],
        "the thread's title, status and tags; the topic's status and tags"
    );
    let actual = calls
        .lock()
        .unwrap()
        .iter()
        .map(|request| (request.system.clone(), request.input.clone()))
        .collect::<Vec<_>>();
    assert_eq!(recorded.len(), actual.len());
    assert!(
        actual.iter().all(|request| recorded.contains(request)),
        "every auxiliary model receives exactly what its reader can inspect"
    );
    let use_rows: (i64, i64) = sqlx::query_as("SELECT count(*),sum(total_tokens)::bigint FROM usage_events WHERE origin='orch' AND purpose='summary' AND counted")
        .fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(use_rows, (5, 600));
    let rollup: i64 = sqlx::query_scalar(
        "SELECT sum(total_tokens)::bigint FROM usage_daily WHERE purpose='summary'",
    )
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(rollup, 600);
    assert_eq!(
        s.rig.commands.issued_of("turn.start"),
        1,
        "summaries create no model turn"
    );
    // The rebuild locks the rollups, so it is run in a transaction, as its callers do.
    let mut tx = s.rig.db.pool().begin().await.unwrap();
    orch_db::usage::rollup::rebuild(&mut tx, None)
        .await
        .unwrap();
    let rebuilt: i64 = sqlx::query_scalar(
        "SELECT sum(total_tokens)::bigint FROM usage_daily WHERE purpose='summary'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        rebuilt, 600,
        "rebuilt totals include the same actual summary requests"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_wins_a_race_with_a_real_summary_call_and_that_call_does_not_delay_the_turn() {
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    script(&s, false);
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "auto.title",
            json!("summarizer"),
        )
        .await;
    let id = s.thread().await.base.id;
    resume_title(&s, id).await;
    s.rig.machine.handler.hold(CommandKind::ProviderComplete);
    s.exchange(id, 0, "Fix login", "The fix is ready").await;
    eventually(
        "the provider request is in flight after the turn ended",
        || async { s.rig.commands.issued_of("provider.complete") == 1 },
    )
    .await;
    let Node::Thread(before) = places::read(&s.rig.world, &s.ada, id).await else {
        panic!("a thread");
    };
    let edited = s
        .rig
        .world
        .app
        .call(NodeUpdate, &[&id])
        .token(&s.ada.token)
        .body(&NodeUpdateBody {
            name: Some("The team's chosen title".to_owned()),
            description: None,
            color: None,
            custom_color: None,
            readme_document_id: None,
            expected_revision: before.base.revision,
        })
        .send()
        .await
        .ok();
    assert!(matches!(edited, Node::Thread(_)));
    s.say(
        &s.ada,
        id,
        "Check the next file",
        Delivery::Queue,
        "while-summary-waits",
    )
    .await
    .ok();
    let next = s.rig.turn_start(id, 1).await;
    s.rig.say(id, turn_began(next.turn_id)).await;
    s.rig.wait_turn(next.turn_id, "running").await;
    s.rig.machine.handler.release();
    wait_attempt(&s, id).await;
    let Node::Thread(after) = places::read(&s.rig.world, &s.ada, id).await else {
        panic!("a thread");
    };
    assert_eq!(after.base.name, "The team's chosen title");
    assert_eq!(
        after.base.title_source,
        orch_api_contract::nodes::FieldSource::User
    );
    let paid: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE origin='orch' AND purpose='summary'",
    )
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(paid, 1, "the discarded race still used the model");
    s.rig
        .say(
            id,
            turn_ended(
                next.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
}

#[tokio::test]
async fn no_chosen_producer_or_a_setting_writer_without_provider_use_makes_no_call_and_is_no_failure()
 {
    let s = Scene::new().await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "auto.title",
            json!("summarizer"),
        )
        .await;
    let id = s.thread().await.base.id;
    resume_title(&s, id).await;
    s.exchange(id, 0, "Work", "Done").await;
    wait_attempt(&s, id).await;
    assert_eq!(s.rig.commands.issued_of("provider.complete"), 0);
    assert_eq!(
        attempts(&s, id).await,
        (0, Some("no summarizer is configured".to_owned()))
    );
    // The summarizer's row is listed where the key is set, the topic and above (a thread lists the keys valid
    // at a thread), and shows the reason of that place's own attempt: here the topic's rollup, which has no
    // producer either.
    let topic = s.tree.topic.base.id;
    s.rig
        .world
        .set_at(Target::Node(topic), "auto.topic_rollup", json!("rollup"))
        .await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_engine::summaries::due(&mut conn, topic, "refresh", s.rig.world.now())
        .await
        .unwrap();
    drop(conn);
    s.rig.engine.summarize_due(topic).await.unwrap();
    wait_attempt(&s, topic).await;
    assert_eq!(
        attempts(&s, topic).await,
        (0, Some("no summarizer is configured".to_owned()))
    );
    assert_eq!(s.rig.commands.issued_of("provider.complete"), 0);
    let settings = s
        .rig
        .world
        .app
        .call(NodeSettingList, &[&topic])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        settings
            .iter()
            .find(|setting| setting.key == "auto.summarizer")
            .unwrap()
            .automation_reason
            .as_deref(),
        Some("no summarizer is configured")
    );
    s.rig.engine.stop();
    let provider = api_provider(&s).await;
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    // A historical writer who has since lost Use remains the writer; authority is read at issue.
    sqlx::query("UPDATE settings SET updated_by=$2 WHERE node_id=$1 AND key='auto.summarizer'")
        .bind(s.tree.topic.base.id)
        .bind(s.dan.id())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    s.rig.world.advance(chrono::Duration::seconds(121));
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_engine::summaries::due(&mut conn, id, "refresh", s.rig.world.now())
        .await
        .unwrap();
    drop(conn);
    s.rig.engine.summarize_due(id).await.unwrap();
    assert_eq!(s.rig.commands.issued_of("provider.complete"), 0);
    let (failures, reason) = attempts(&s, id).await;
    assert_eq!(failures, 0);
    assert!(reason.unwrap().contains("may not use"));
}

#[tokio::test]
async fn malformed_text_is_charged_and_backed_off_while_a_full_shared_cap_skips_future_calls() {
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    script(&s, true);
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "auto.title",
            json!("summarizer"),
        )
        .await;
    let id = s.thread().await.base.id;
    resume_title(&s, id).await;
    s.exchange(id, 0, "Work", "Done").await;
    wait_attempt(&s, id).await;
    assert_eq!(attempts(&s, id).await.0, 1);
    let delay: i64 = sqlx::query_scalar("SELECT EXTRACT(epoch FROM next_at-last_at)::bigint FROM node_summary_state WHERE node_id=$1")
        .bind(id).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(delay, 240);
    assert_eq!(s.rig.commands.issued_of("provider.complete"), 1);
    s.rig.engine.stop();
    sqlx::query("INSERT INTO usage_caps (id,node_id,meter,period,amount,created_by) VALUES ($1,$2,'tokens','day',120,$3)")
        .bind(new_id()).bind(s.tree.project.base.id).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    s.rig.world.advance(chrono::Duration::seconds(241));
    s.rig.engine.summarize_due(id).await.unwrap();
    assert_eq!(s.rig.commands.issued_of("provider.complete"), 1);
    assert_eq!(attempts(&s, id).await.0, 0, "lack of cap room is a skip");
}

#[tokio::test]
async fn a_handoff_uses_the_chosen_producer_and_keeps_an_indexed_full_file_and_the_source_on_failure()
 {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "Fix web/login.ts", "The redirect is repaired")
        .await;
    let request = new_id();
    assert!(
        handoff(&s.rig.engine, id, None, request).await.is_err(),
        "a transcript is never a substitute summary"
    );
    let provider = api_provider(&s).await;
    script(&s, false);
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    let text = handoff(&s.rig.engine, id, None, request).await.unwrap();
    assert!(text.contains("web/login.ts"));
    assert!(text.chars().count() <= 3_000 && text.split_whitespace().count() <= 400);
    let file = indexed_transcript(&s.rig.engine, id, None).await.unwrap();
    assert_eq!(file.turns, 1);
    assert!(file.text.contains("# Conversation index") && file.text.contains(" · line "));
    assert!(
        file.text.contains("Fix web/login.ts") && file.text.contains("The redirect is repaired")
    );
    script(&s, true);
    assert!(handoff(&s.rig.engine, id, None, new_id()).await.is_err());
    let Node::Thread(source) = places::read(&s.rig.world, &s.ada, id).await else {
        panic!("the source stays usable");
    };
    assert_eq!(source.base.id, id);
    assert_eq!(
        s.rig.commands.issued_of("turn.start"),
        1,
        "a failed handoff never starts another turn"
    );
}

#[tokio::test]
async fn material_sent_to_a_different_explicitly_chosen_provider_is_redacted_before_the_call() {
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    assert_ne!(provider, s.on.provider);
    let seen = script(&s, false);
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "auto.title",
            json!("summarizer"),
        )
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "redaction.patterns",
            json!([{"name":"a test secret","regex":"opaque-secret-123"}]),
        )
        .await;
    let id = s.thread_by(&s.ada, "opaque-secret-123").await.base.id;
    resume_title(&s, id).await;
    s.exchange(id, 0, "Fix opaque-secret-123", "opaque-secret-123 is fixed")
        .await;
    wait_attempt(&s, id).await;
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].input.contains("opaque-secret-123"));
    assert!(requests[0].input.contains("[masked]"));
    assert_eq!(requests[0].provider_id, provider);
}

#[tokio::test]
async fn an_installed_rule_producer_can_refresh_the_automatic_description_without_using_a_model() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "Fix login", "The tests pass").await;
    wait_attempt(&s, id).await;
    s.rig.engine.stop();
    let producer = Arc::new(orch_testkit::doubles::summarizer::Scripted::default());
    producer.push(Ok(orch_summarizer_contract::result::SummaryResult::text(
        "Tests pass; review next",
    )));
    s.rig.engine.attach_summarizer(producer.clone());
    configure(&s, json!({"kind":"scripted"})).await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "auto.description",
            json!("summarizer"),
        )
        .await;
    s.rig.world.advance(chrono::Duration::seconds(121));
    s.rig
        .world
        .app
        .call(NodeAuto, &[&id])
        .token(&s.ada.token)
        .body(&NodeAutoBody {
            field: AutoField::Description,
            action: AutoAction::Refresh,
        })
        .send()
        .await
        .ok();
    assert!(s.rig.engine.summarize_due(id).await.unwrap());
    let Node::Thread(thread) = places::read(&s.rig.world, &s.ada, id).await else {
        panic!("a thread");
    };
    assert_eq!(thread.base.description, "Tests pass; review next");
    assert_eq!(producer.requests().len(), 1);
    assert_eq!(
        producer.requests()[0].field,
        orch_summarizer_contract::descriptor::SummaryField::Description
    );
    assert_eq!(s.rig.commands.issued_of("provider.complete"), 0);
}

#[tokio::test]
async fn recovering_an_already_counted_completion_adds_neither_a_model_call_nor_usage_twice() {
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    script(&s, false);
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "auto.title",
            json!("summarizer"),
        )
        .await;
    let id = s.thread().await.base.id;
    resume_title(&s, id).await;
    s.exchange(id, 0, "Fix login", "Done").await;
    wait_attempt(&s, id).await;
    // A second reconciler may have fetched this pending attribution before the first committed.
    // The same factual response is safe to reconcile again under the same ledger identity.
    let mut recovery = s.rig.db.begin().await.unwrap();
    sqlx::query("INSERT INTO summary_calls (id,node_id,topic_id,thread_id,provider_id,person_id,model_name,created_at) SELECT id,thread_id,topic_id,thread_id,provider_id,user_id,model,at FROM usage_events WHERE origin='orch' AND purpose='summary'")
        .execute(&mut *recovery).await.unwrap();
    sqlx::query("UPDATE model_inputs SET state='prepared',sent_at=NULL WHERE node_id=$1 AND purpose='summary'")
        .bind(id).execute(&mut *recovery).await.unwrap();
    recovery.commit().await.unwrap();
    eventually(
        "the second reconciliation removes its pending attribution",
        || async {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM summary_calls")
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
                == 0
        },
    )
    .await;
    let use_rows: (i64,i64) = sqlx::query_as("SELECT count(*),sum(total_tokens)::bigint FROM usage_events WHERE origin='orch' AND purpose='summary'").fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(use_rows, (1, 120));
    assert_eq!(s.rig.commands.issued_of("provider.complete"), 1);
    let state: String =
        sqlx::query_scalar("SELECT state FROM model_inputs WHERE node_id=$1 AND purpose='summary'")
            .bind(id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        state, "sent",
        "recovery settles the same readable prompt, without another delivery"
    );
}

/// Sets one setting of the scene's topic.
async fn set_at_topic(s: &Scene, key: &str, value: Json) {
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&s.tree.topic.base.id, &key])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value,
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn a_long_threads_handoff_sends_the_newest_turns_in_brief_and_the_older_ones_as_an_index_within_the_share()
 {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    for nth in 0..30 {
        s.exchange(
            id,
            nth,
            &format!("Question {nth:02} {}", "q".repeat(300)),
            &format!("Answer {nth:02} {}", "a".repeat(300)),
        )
        .await;
    }
    let provider = api_provider(&s).await;
    let seen = script(&s, false);
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    // The model's entry names no window, so the one every such model has (32 000 tokens) is taken; three percent of
    // it, at three bytes to a token, is 2 880 bytes, and thirty turns of this size are far more.
    set_at_topic(&s, "summary.handoff_input_share", json!(3)).await;
    handoff(&s.rig.engine, id, None, new_id()).await.unwrap();
    let sent = seen
        .lock()
        .unwrap()
        .iter()
        .find(|request| request.system.starts_with("You hand"))
        .cloned()
        .expect("the producer was asked for the hand-off");
    assert!(
        sent.input.len() <= 32_000 * 3 / 100 * 3,
        "{} bytes were sent",
        sent.input.len()
    );
    assert!(
        sent.input.contains("Question 29") && sent.input.contains("Answer 29"),
        "the newest turn goes in brief: {}",
        sent.input
    );
    assert!(
        !sent.input.contains("Question 00"),
        "the oldest turn is left out"
    );
    assert!(sent.input.contains("# Older turns, one line each"));
    assert!(sent.input.contains("still older are not listed"));
}

/// The plan of a request for `thread`, as Ada asks for it.
async fn preflight(s: &Scene, thread: Id, request: &ForkRequest) -> ForkPlan {
    s.rig
        .world
        .app
        .call(ThreadForkPreflight, &[&thread])
        .token(&s.ada.token)
        .body(request)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn the_plan_of_a_summary_shows_what_it_will_send_and_what_that_costs_before_it_is_confirmed()
{
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    for nth in 0..4 {
        s.exchange(
            id,
            nth,
            &format!("Question {nth}"),
            &format!("Answer {nth}"),
        )
        .await;
    }
    let request = ForkRequest {
        result: ForkResult::New,
        topic_id: None,
        name: Some("Trial".to_owned()),
        access: None,
        history: Some(ForkHistory {
            mode: HistoryMode::Summary,
            from_turn: None,
            to_turn: None,
        }),
        to: None,
        environment: None,
        worktree: None,
        files_at: None,
        memory: None,
        directory: None,
        cwd: None,
        options: None,
        mounts: None,
        check_fingerprints: None,
        fingerprint_request_id: None,
    };
    let without = preflight(&s, id, &request).await;
    let none = without
        .history
        .summary
        .expect("a summary plan carries its estimate");
    assert!(!none.available, "no summary model is chosen yet");
    let provider = api_provider(&s).await;
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    let with = preflight(&s, id, &request).await;
    let estimate = with
        .history
        .summary
        .expect("a summary plan carries its estimate");
    assert!(estimate.available);
    assert_eq!(estimate.share, 50, "half of the window by default");
    assert!(
        estimate.input_tokens > 0 && estimate.input_tokens <= 32_000 / 2,
        "{estimate:?}"
    );
    assert!(estimate.output_tokens > 0);
    let other = ForkRequest {
        history: Some(ForkHistory {
            mode: HistoryMode::All,
            from_turn: None,
            to_turn: None,
        }),
        ..request.clone()
    };
    let carried = preflight(&s, id, &other).await;
    assert!(
        carried.history.summary.is_none(),
        "only a summary has something to estimate"
    );
}

/// What the agent's `goal` tool leaves when it reports in the running turn.
async fn report(s: &Scene, thread: Id, turn: Id, status: &str, evidence: &str) {
    sqlx::query(
        "INSERT INTO thread_goal_reports (id, goal_id, thread_id, turn_id, status, evidence, created_at)
         SELECT $1, g.id, $2, $3, $4, $5, now() FROM thread_goals g WHERE g.thread_id = $2 AND g.status = 'active'",
    )
    .bind(new_id())
    .bind(thread)
    .bind(turn)
    .bind(status)
    .bind(evidence)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_report_of_met_is_checked_by_the_helper_and_a_not_yet_starts_the_next_turn_with_its_reason()
 {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let provider = api_provider(&s).await;
    let asked = Arc::new(Mutex::new(Vec::<ProviderComplete>::new()));
    let recorded = asked.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |command| {
            let Command::ProviderComplete(request) = command else {
                panic!("only a completion reaches this script");
            };
            let mut seen = recorded.lock().unwrap();
            seen.push(request.clone());
            let text = if seen.len() == 1 {
                "not_yet\nNo test output is shown; run the tests and quote the result."
            } else {
                "met\nThe run shows 212 tests passing."
            };
            Ok(CommandResult::ProviderComplete(ProviderCompleteResult {
                timing: Default::default(),
                text: text.to_owned(),
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 100,
                    output_tokens: 10,
                    total_tokens: 110,
                    ..Default::default()
                },
                request_ref: Some(format!("goal-check-{}", seen.len())),
            }))
        });
    configure(
        &s,
        json!({"kind":"model","provider_id":provider,"model_name":MODEL}),
    )
    .await;
    let ada = Caller::session(s.ada.id());
    let set = s
        .rig
        .engine
        .set_goal(
            &ada,
            id,
            &ThreadGoalCreateBody {
                objective: "Make the tests pass".to_owned(),
                condition: "npm test exits 0".to_owned(),
                limits: None,
                notify_on_end: false,
                plan: false,
            },
        )
        .await
        .unwrap();
    assert!(set.helper, "a summary model is chosen, so a helper checks");
    // The agent says it is done without showing it; the helper says not yet, and the agent is told why.
    let first = s.rig.turn_start(id, 0).await;
    s.rig.answer(id, &first, "m0", "I think it is done").await;
    report(&s, id, first.turn_id, "met", "I fixed it").await;
    s.rig
        .say(id, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    let second = s.rig.turn_start(id, 1).await;
    assert!(
        second.text.contains(
            "is not enough yet: No test output is shown; run the tests and quote the result."
        ),
        "{}",
        second.text
    );
    {
        let seen = asked.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(
            seen[0]
                .system
                .starts_with("You judge whether a coding agent")
        );
        assert!(
            seen[0].input.contains("npm test exits 0") && seen[0].input.contains("I fixed it"),
            "the helper is sent the condition and the evidence: {}",
            seen[0].input
        );
    }
    // The next report shows it; the helper agrees, and the goal is met by the helper's word.
    s.rig.answer(id, &second, "m1", "Done").await;
    report(
        &s,
        id,
        second.turn_id,
        "met",
        "npm test: 212 passed, 0 failed",
    )
    .await;
    s.rig
        .say(id, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    eventually("the goal is met", || async {
        s.rig
            .engine
            .goal_of(id)
            .await
            .unwrap()
            .is_some_and(|goal| goal.status == GoalStatus::Met)
    })
    .await;
    let met = s.rig.engine.goal_of(id).await.unwrap().unwrap();
    assert_eq!(
        met.last_report.and_then(|report| report.checked_by),
        Some(GoalCheckedBy::Helper)
    );
    assert_eq!(met.used.turns, 2);
    assert_eq!(asked.lock().unwrap().len(), 2);
    assert_eq!(
        s.rig.commands.issued_of("turn.start"),
        2,
        "nothing goes on after it is met"
    );
}
