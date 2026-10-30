//! A held summary cannot publish or consume a newer selected history; the next bounded request reads its frozen prefix.

mod continuation_diagnostics;
mod continuation_read_support;
mod places;
mod rig;
mod support;

use async_trait::async_trait;
use continuation_read_support::{Proof, retained, select};
use orch_api_contract::{
    nodes::{AutoAction, AutoField, NodeAuto, NodeAutoBody},
    settings::{NodeSettingSet, NodeSettingSetBody},
};
use orch_summarizer_contract::{
    descriptor::{SummarizerDescriptor, SummaryField},
    result::SummaryResult,
    summarizer::{SummarizeError, Summarizer, SummaryRequest},
};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Semaphore;

struct Held {
    requests: Mutex<Vec<SummaryRequest>>,
    entered: Semaphore,
    release: Semaphore,
}

#[async_trait]
impl Summarizer for Held {
    fn describe(&self) -> SummarizerDescriptor {
        SummarizerDescriptor {
            name: "continuation-summary".into(),
            title: "Controlled history summary".into(),
            config_schema: json!({"type":"object"}),
            fields: vec![SummaryField::Description],
            extensions: Default::default(),
        }
    }
    async fn summarize(&self, request: SummaryRequest) -> Result<SummaryResult, SummarizeError> {
        let first = {
            let mut requests = self.requests.lock().unwrap_or_else(PoisonError::into_inner);
            requests.push(request);
            requests.len() == 1
        };
        if first {
            self.entered.add_permits(1);
            self.release
                .acquire()
                .await
                .map_err(|_| SummarizeError::Failed {
                    message: "The controlled summary phase was closed.".into(),
                    retryable: false,
                })?
                .forget();
        }
        Ok(SummaryResult::text(if first {
            "Excluded old summary"
        } else {
            "Retained current summary"
        }))
    }
}

fn setup_failure(
    phase: &str,
    status: http::StatusCode,
    problem: orch_api_contract::common::Problem,
) -> Box<dyn std::error::Error + Send + Sync> {
    let fields = problem
        .extra
        .keys()
        .filter(|key| matches!(key.as_str(), "errors" | "required" | "scope" | "field"))
        .cloned()
        .collect::<Vec<_>>();
    format!(
        "continuation summary setup {phase}: HTTP {status}, code {}, field labels {fields:?}",
        problem.code
    )
    .into()
}

async fn setting(scene: &rig::Scene, node: Id, key: &str, value: Json) -> Proof {
    let reply = scene
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&node, &key])
        .token(&scene.ada.token)
        .body(&NodeSettingSetBody {
            value,
            locked: None,
            expected_revision: None,
        })
        .send()
        .await;
    let status = reply.status;
    reply
        .body
        .map_err(|problem| setup_failure(key, status, problem))?;
    Ok(())
}

async fn refresh(scene: &rig::Scene, thread: Id) -> Proof {
    let reply = scene
        .rig
        .world
        .app
        .call(NodeAuto, &[&thread])
        .token(&scene.ada.token)
        .body(&NodeAutoBody {
            field: AutoField::Description,
            action: AutoAction::Refresh,
        })
        .send()
        .await;
    let status = reply.status;
    reply
        .body
        .map_err(|problem| setup_failure("description refresh", status, problem))?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inflight_summary_keeps_its_old_usage_and_releases_only_its_claim_without_publishing_over_a_new_history()
-> Proof {
    continuation_diagnostics::install()?;
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(thread, 0, "Retained first input", "Retained first answer")
        .await;
    scene
        .exchange(thread, 1, "Excluded second input", "Excluded second answer")
        .await;
    rig::eventually("native inputs and immutable terminal obligations finish before the read projection fixture",||async {
        sqlx::query_scalar::<_,bool>("SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1 AND
          (end_work IS NOT NULL OR files_end_due OR files_work_pending))")
            .bind(thread).fetch_one(scene.rig.db.pool()).await.unwrap_or(false)
    }).await;
    scene.rig.engine.stop();
    let view = retained(
        &scene.rig.world.app.app.db,
        thread,
        scene.ada.id(),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let producer = Arc::new(Held {
        requests: Mutex::new(Vec::new()),
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    scene.rig.engine.attach_summarizer(producer.clone());
    setting(
        &scene,
        scene.tree.topic.base.id,
        "auto.summarizer",
        json!({"kind":"continuation-summary"}),
    )
    .await?;
    setting(
        &scene,
        scene.tree.topic.base.id,
        "auto.description",
        json!("summarizer"),
    )
    .await?;
    scene.rig.world.advance(chrono::Duration::seconds(121));
    refresh(&scene, thread).await?;
    let ledger: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id")
            .bind(thread)
            .fetch_all(scene.rig.db.pool())
            .await?;
    let commands = scene.rig.machine.handler.executions().len();
    let engine = scene.rig.engine.clone();
    let running = tokio::spawn(async move { engine.summarize_due(thread).await });
    rig::eventually(
        "the original summary reaches its controlled producer",
        || async { producer.entered.available_permits() == 1 },
    )
    .await;
    let first = producer
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)[0]
        .clone();
    assert!(
        first
            .activity
            .iter()
            .any(|item| item.text.contains("Excluded second answer"))
    );
    let description: String = sqlx::query_scalar("SELECT description FROM nodes WHERE id=$1")
        .bind(thread)
        .fetch_one(scene.rig.db.pool())
        .await?;
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.child,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    producer.release.add_permits(1);
    assert!(tokio::time::timeout(rig::PATIENCE, running).await???);
    let after:(String,i64,Option<Id>,Option<Json>)=sqlx::query_as("SELECT n.description,s.last_seq,s.claimed_by,t.usage
      FROM nodes n JOIN threads t ON t.node_id=n.id JOIN node_summary_state s ON s.node_id=n.id WHERE n.id=$1")
        .bind(thread).fetch_one(scene.rig.db.pool()).await?;
    assert_eq!(
        after,
        (description, 0, None, None),
        "a completed old answer cannot replace the selection's description, consume its reset cursor, or move old context"
    );
    refresh(&scene, thread).await?;
    assert!(scene.rig.engine.summarize_due(thread).await?);
    let requests = producer
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(requests.len(), 2);
    let texts = requests[1]
        .activity
        .iter()
        .map(|item| item.text.as_str())
        .collect::<Vec<_>>();
    assert!(texts.contains(&"Retained first input") && texts.contains(&"Retained first answer"));
    assert!(!texts.iter().any(|text| text.contains("Excluded second")));
    let description: String = sqlx::query_scalar("SELECT description FROM nodes WHERE id=$1")
        .bind(thread)
        .fetch_one(scene.rig.db.pool())
        .await?;
    assert_eq!(description, "Retained current summary");
    let same: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id")
            .bind(thread)
            .fetch_all(scene.rig.db.pool())
            .await?;
    assert_eq!(
        same, ledger,
        "selection never deletes, rewrites or duplicates actual turn usage"
    );
    assert_eq!(
        scene.rig.machine.handler.executions().len(),
        commands,
        "a controlled rule summary starts no native turn or provider request"
    );
    Ok(())
}
