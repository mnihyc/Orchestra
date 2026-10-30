//! Lifecycle Enable continues exact resource-stopped work as its original author before preserving queued work.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::environment::{EnvironmentDisable, EnvironmentEnable};
use orch_api_contract::resource_facts::{ResourcePauseQuery, ResourceResumeQuery};
use orch_api_contract::resources::{
    AgentUpdate, AgentUpdateBody, ProviderDisable, ProviderEnable, ProviderModelUpdate,
    ProviderModelUpdateBody,
};
use orch_api_contract::threads::Delivery;
use orch_link_contract::commands::{CommandKind, CommandResult, Empty};
use orch_link_contract::events::TurnOutcome;
use orch_types::{Id, Json};
use rig::{MODEL, Scene, eventually};
use serde_json::json;

type InputAuthority = (Option<Id>, Option<Json>, Option<Id>, Option<Json>);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_resource_enable_continues_the_stopped_task_as_its_author_then_runs_the_unchanged_queue()
 {
    for kind in ["provider", "model", "agent", "environment"] {
        let s = Scene::new().await;
        s.rig.machine.handler.on(CommandKind::AgentStop, |_| {
            Ok(CommandResult::AgentStop(Empty {}))
        });
        s.rig.machine.handler.on(CommandKind::ProviderStop, |_| {
            Ok(CommandResult::ProviderStop(Empty {}))
        });
        if kind != "environment" {
            let (resource, id, permission) = if kind == "agent" {
                ("agent", s.on.agent, "agent.manage")
            } else {
                ("provider", s.on.provider, "provider.manage")
            };
            s.rig
                .lets_use(resource, id, permission, &s.eve, json!({}))
                .await;
        }
        let thread = s.thread().await.base.id;
        let origin = s
            .say(
                &s.ada,
                thread,
                "Finish this task",
                Delivery::Queue,
                "original-task",
            )
            .await
            .ok()
            .input
            .id;
        let stopped = s.rig.turn_start(thread, 0).await;
        s.rig.say(thread, rig::turn_began(stopped.turn_id)).await;
        s.rig.wait_turn(stopped.turn_id, "running").await;
        let pending = s
            .say(
                &s.ada,
                thread,
                "Then do the next task",
                Delivery::Queue,
                "queued-task",
            )
            .await
            .ok()
            .input
            .id;
        let position: String =
            sqlx::query_scalar("SELECT position::text FROM thread_inputs WHERE id=$1")
                .bind(pending)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        let provenance:InputAuthority=sqlx::query_as("SELECT author_id,submission_authority,run_as,run_as_authority FROM thread_inputs WHERE id=$1")
            .bind(origin).fetch_one(s.rig.db.pool()).await.unwrap();
        let model: Id = sqlx::query_scalar(
            "SELECT id FROM provider_models WHERE provider_id=$1 AND model_name=$2",
        )
        .bind(s.on.provider)
        .bind(MODEL)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
        match kind {
            "provider" => {
                s.rig
                    .world
                    .app
                    .call(ProviderDisable, &[&s.on.provider])
                    .token(&s.eve.token)
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
                    .call(ProviderModelUpdate, &[&s.on.provider, &model])
                    .token(&s.eve.token)
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
                    .token(&s.eve.token)
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
                    .token(&s.eve.token)
                    .send()
                    .await
                    .ok();
            }
        }
        s.rig
            .say(
                thread,
                rig::turn_ended(stopped.turn_id, TurnOutcome::Interrupted),
            )
            .await;
        s.rig.wait_turn(stopped.turn_id, "interrupted").await;
        eventually("the accepted native stop has settled",||async {
            !sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM jobs WHERE kind='resource_quiesce' AND state IN ('queued','running'))").fetch_one(s.rig.db.pool()).await.unwrap()
        }).await;
        let states: Vec<String> =
            sqlx::query_scalar("SELECT state FROM jobs WHERE kind='resource_quiesce'")
                .fetch_all(s.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(
            states,
            ["succeeded"],
            "the stop is actually confirmed: {kind}"
        );
        let resume = ResourceResumeQuery {
            run_waiting: Some(true),
        };
        let patch = ResourcePauseQuery {
            note: None,
            run_waiting: Some(true),
        };
        match kind {
            "provider" => {
                s.rig
                    .world
                    .app
                    .call(ProviderEnable, &[&s.on.provider])
                    .token(&s.eve.token)
                    .query(&resume)
                    .send()
                    .await
                    .ok();
            }
            "model" => {
                let body: ProviderModelUpdateBody =
                    serde_json::from_value(json!({"enabled":true})).unwrap();
                s.rig
                    .world
                    .app
                    .call(ProviderModelUpdate, &[&s.on.provider, &model])
                    .token(&s.eve.token)
                    .query(&patch)
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
                    serde_json::from_value(json!({"expected_revision":revision,"disabled":false}))
                        .unwrap();
                s.rig
                    .world
                    .app
                    .call(AgentUpdate, &[&s.on.agent])
                    .token(&s.eve.token)
                    .query(&patch)
                    .body(&body)
                    .send()
                    .await
                    .ok();
            }
            _ => {
                s.rig
                    .world
                    .app
                    .call(EnvironmentEnable, &[&s.on.environment])
                    .token(&s.eve.token)
                    .query(&resume)
                    .send()
                    .await
                    .ok();
            }
        }
        let receipt: Json = sqlx::query_scalar("SELECT payload FROM items WHERE turn_id=$1 AND kind='marker' AND payload->>'marker'='resource_resumed'")
            .bind(stopped.turn_id)
            .fetch_optional(s.rig.db.pool())
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("no resumption receipt after enabling {kind}"));
        assert!(
            receipt["data"]["input_id"].is_string(),
            "the stopped task could not continue after enabling {kind}: {receipt}"
        );
        let mut continued = None;
        eventually(&format!("turn.start after enabling {kind}"), || {
            continued = s.rig.turn_starts(thread).get(1).cloned();
            let found = continued.is_some();
            async move { found }
        })
        .await;
        let continued = continued.expect("a continued turn was seen");
        let (input,content):(Id,Json)=sqlx::query_as("SELECT i.id,i.content FROM turns t JOIN thread_inputs i ON i.id=t.input_id WHERE t.id=$1").bind(continued.turn_id).fetch_one(s.rig.db.pool()).await.unwrap();
        assert_ne!(input, origin);
        assert_ne!(input, pending);
        assert_eq!(content, json!([{"type":"text","text":"Continue"}]));
        let actual:InputAuthority=sqlx::query_as("SELECT author_id,submission_authority,run_as,run_as_authority FROM thread_inputs WHERE id=$1").bind(input).fetch_one(s.rig.db.pool()).await.unwrap();
        assert_eq!(
            actual, provenance,
            "all original authority survives the manager's operation: {kind}"
        );
        assert_eq!(actual.0, Some(s.ada.id()));
        assert_ne!(actual.0, Some(s.eve.id()));
        assert_eq!(receipt["data"]["input_id"], input.to_string());
        assert_eq!(receipt["data"]["by"]["id"], s.eve.id().to_string());
        assert!(receipt["data"]["reason"].is_null());
        let recoveries: i64 =
            sqlx::query_scalar("SELECT count(*) FROM model_call_recovery WHERE node_id=$1")
                .bind(thread)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(
            recoveries, 0,
            "an administrative interruption invents no failed model call: {kind}"
        );
        s.rig.say(thread, rig::turn_began(continued.turn_id)).await;
        s.rig
            .say(
                thread,
                rig::turn_ended(continued.turn_id, TurnOutcome::Completed),
            )
            .await;
        s.rig.wait_turn(continued.turn_id, "completed").await;
        let next = s.rig.turn_start(thread, 2).await;
        let next_input: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
            .bind(next.turn_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(
            next_input, pending,
            "the queued input keeps its identity after Continue: {kind}"
        );
        let after: String =
            sqlx::query_scalar("SELECT position::text FROM thread_inputs WHERE id=$1")
                .bind(pending)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(after, position);
    }
}
