//! Automatic document descriptions follow the selected producer, preserve writers and share pacing and usage.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use common::World;
use orch_db::queries::documents as rows;
use orch_documents::versions::Writer;
use orch_summarizer_contract::result::SummaryResult;
use orch_summarizer_contract::summarizer::{SummarizeError, Summarizer, SummaryRequest};
use orch_testkit::doubles::summarizer::Scripted;
use orch_types::{Clock, Id, ManualClock};
use serde_json::json;
use shared::Places;
use std::sync::Arc;

async fn setup() -> (World, Arc<ManualClock>, Arc<Scripted>) {
    let clock = Arc::new(ManualClock::new(chrono::Utc::now()));
    let world = World::with_clock(clock.clone()).await;
    world.engine().stop();
    let producer = Arc::new(Scripted::default());
    world.engine().attach_summarizer(producer.clone());
    world
        .set(world.topic, "auto.summarizer", json!({"kind":"scripted"}))
        .await;
    world
        .set(
            world.topic,
            "auto.triggers",
            json!({"on":["turn_completed","milestone"],"min_interval_secs":7}),
        )
        .await;
    (world, clock, producer)
}

async fn document(world: &World, clock: &ManualClock, title: &str) -> Id {
    let id = world
        .document_beside(
            world.topic,
            title,
            &["How redirects work.", "Keep a secret opaque-secret-123."],
        )
        .await;
    // The shared fixture authors blocks with wall time; bring the controlled clock to that writing.
    clock.set(chrono::Utc::now());
    id
}

async fn current(world: &World, id: Id) -> rows::DocumentRow {
    rows::get(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap()
}

async fn change_text(world: &World, document: Id, holds: &str, text: &str) {
    use orch_api_contract::documents::{BlockContent, DocumentOp, Inline};
    use orch_db::queries::documents::batch;
    let mut conn = world.pool.acquire().await.unwrap();
    let blocks = rows::live_blocks(&mut conn, document).await.unwrap();
    let block = blocks
        .iter()
        .find(|block| block.text.contains(holds))
        .unwrap();
    // Use the fixture's injected clock; the general versioning helper deliberately writes five seconds ahead.
    let outcome = batch::apply(
        &mut conn,
        document,
        &Writer::User(world.ada),
        Some(orch_types::new_id()),
        &[DocumentOp::Update {
            id: block.id,
            base_seq: block.seq_updated,
            block_type: None,
            props: None,
            content: Some(BlockContent::Inline(vec![Inline::Text {
                text: text.to_owned(),
                marks: None,
                href: None,
                color: None,
                background: None,
            }])),
        }],
        batch::Limits {
            max_bytes: 2_097_152,
            max_blocks: 5_000,
        },
        batch::Rules {
            window_secs: 600,
            kept: 200,
            history_max_bytes: 16_777_216,
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    assert!(outcome.is_kept(), "{outcome:?}");
}

#[tokio::test]
async fn a_changed_automatic_document_is_described_and_repeated_edits_coalesce_at_the_setting_interval()
 {
    let (world, clock, producer) = setup().await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"test secret","regex":"opaque-secret-123"}]),
        )
        .await;
    world
        .set(world.topic, "docs.description_chars", json!(36))
        .await;
    sqlx::query("UPDATE nodes SET name='opaque-secret-123 topic' WHERE id=$1")
        .bind(world.topic)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let doc = document(&world, &clock, "Redirects").await;
    let before = current(&world, doc).await;
    let versions: i64 = world
        .count(
            "SELECT count(*) FROM document_versions WHERE document_id=$1",
            doc,
        )
        .await;
    producer.push(Ok(SummaryResult::text("How the redirect flow works")));
    assert!(world.engine().summarize_document_due(doc).await.unwrap());
    let after = current(&world, doc).await;
    assert_eq!(after.description, "How the redirect flow works");
    assert_eq!(after.description_source, "auto");
    assert_eq!(after.doc_seq, before.doc_seq + 1);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM document_versions WHERE document_id=$1",
                doc
            )
            .await,
        versions
    );
    let requests = producer.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].limits.max_chars, 36);
    let material = serde_json::to_string(&requests[0]).unwrap();
    assert!(material.contains("Redirects"));
    assert!(material.contains("How redirects work"));
    assert!(!material.contains("opaque-secret-123"));
    assert!(material.contains("[masked]"));
    change_text(
        &world,
        doc,
        "How redirects work.",
        "New redirects use one endpoint.",
    )
    .await;
    assert!(!world.engine().summarize_document_due(doc).await.unwrap());
    assert_eq!(producer.requests().len(), 1);
    clock.advance(chrono::Duration::seconds(7));
    producer.push(Ok(SummaryResult::text("The single redirect endpoint")));
    assert!(world.engine().summarize_document_due(doc).await.unwrap());
    assert_eq!(
        current(&world, doc).await.description,
        "The single redirect endpoint"
    );
    assert_eq!(producer.requests().len(), 2);
}

#[tokio::test]
async fn user_descriptions_including_deliberately_empty_ones_are_never_replaced() {
    let (world, clock, producer) = setup().await;
    for (title, description) in [
        ("Keep my words", "A writer's own description"),
        ("Keep my empty description", ""),
    ] {
        let doc = document(&world, &clock, title).await;
        rows::describe(
            &mut world.pool.acquire().await.unwrap(),
            doc,
            description,
            &Writer::User(world.ada),
            clock.now(),
        )
        .await
        .unwrap();
        assert!(world.engine().summarize_document_due(doc).await.unwrap());
        let current = current(&world, doc).await;
        assert_eq!(current.description, description);
        assert_eq!(current.description_source, "user");
    }
    assert!(producer.requests().is_empty());
}

#[tokio::test]
async fn without_a_summarizer_the_document_keeps_its_paragraph_fallback() {
    let (world, clock, producer) = setup().await;
    world.unset(world.topic, "auto.summarizer").await;
    let doc = document(&world, &clock, "Undescribed").await;
    let before = current(&world, doc).await;
    assert!(world.engine().summarize_document_due(doc).await.unwrap());
    let after = current(&world, doc).await;
    assert_eq!(after.description, "");
    assert_eq!(after.description_source, "auto");
    assert_eq!(after.doc_seq, before.doc_seq);
    let first = rows::first_paragraphs(&mut world.pool.acquire().await.unwrap(), &[doc], 200)
        .await
        .unwrap();
    assert_eq!(
        first.get(&doc).map(String::as_str),
        Some("How redirects work.")
    );
    assert!(producer.requests().is_empty());
}

struct Held {
    inner: Scripted,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl Summarizer for Held {
    fn describe(&self) -> orch_summarizer_contract::descriptor::SummarizerDescriptor {
        self.inner.describe()
    }
    async fn summarize(&self, request: SummaryRequest) -> Result<SummaryResult, SummarizeError> {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.summarize(request).await
    }
}

#[tokio::test]
async fn a_description_written_while_the_producer_runs_wins_at_commit() {
    let (world, clock, _) = setup().await;
    let doc = document(&world, &clock, "Competing edit").await;
    let held = Arc::new(Held {
        inner: Scripted::default(),
        entered: Default::default(),
        release: Default::default(),
    });
    held.inner
        .push(Ok(SummaryResult::text("The delayed model's words")));
    world.engine().attach_summarizer(held.clone());
    let engine = world.engine();
    let task = tokio::spawn(async move { engine.summarize_document_due(doc).await });
    tokio::time::timeout(common::PATIENCE, held.entered.notified())
        .await
        .unwrap();
    rows::describe(
        &mut world.pool.acquire().await.unwrap(),
        doc,
        "The writer wins",
        &Writer::User(world.ada),
        clock.now(),
    )
    .await
    .unwrap();
    held.release.notify_one();
    assert!(task.await.unwrap().unwrap());
    let after = current(&world, doc).await;
    assert_eq!(after.description, "The writer wins");
    assert_eq!(after.description_source, "user");
}

#[tokio::test]
async fn a_project_document_uses_its_model_provider_records_private_input_and_is_charged_against_shared_caps()
 {
    use orch_link_contract::commands::{
        Command, CommandKind, CommandResult, ProviderCompleteResult,
    };
    let world = World::on_a_key("synthetic-summary-key").await;
    world.engine().stop();
    sqlx::query("UPDATE providers SET key_machine_id=$2 WHERE id=$1")
        .bind(world.provider)
        .bind(world.machine_id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = seen.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |command| {
            let Command::ProviderComplete(request) = command else {
                panic!("completion");
            };
            captured.lock().unwrap().push(request.clone());
            let request_ref = format!("document-summary-{}", captured.lock().unwrap().len());
            Ok(CommandResult::ProviderComplete(ProviderCompleteResult {
                timing: Default::default(),
                text: "How the API redirects requests".into(),
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    total_tokens: 120,
                    ..Default::default()
                },
                request_ref: Some(request_ref),
            }))
        });
    world
        .set(
            world.project,
            "auto.summarizer",
            json!({"kind":"model","provider_id":world.provider,"model_name":common::MODEL}),
        )
        .await;
    world
        .set(
            world.project,
            "auto.triggers",
            json!({"on":["turn_completed","milestone"],"min_interval_secs":0}),
        )
        .await;
    let doc = world
        .document_beside(
            world.project,
            "API redirects",
            &["The endpoint redirects a signed-in user."],
        )
        .await;
    assert!(world.engine().summarize_document_due(doc).await.unwrap());
    assert_eq!(
        current(&world, doc).await.description,
        "How the API redirects requests"
    );
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].system.starts_with("You describe a document"));
    assert!(requests[0].input.contains("The endpoint redirects"));
    let (project,scope,purpose,total):(Id,Id,String,i64)=sqlx::query_as("SELECT project_id,topic_id,purpose,total_tokens FROM usage_events WHERE origin='orch' AND request_ref='document-summary-1'")
        .fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(
        (project, scope, purpose.as_str(), total),
        (world.project, world.project, "summary", 120)
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM document_model_inputs WHERE document_id=$1",
                doc
            )
            .await,
        1
    );
    sqlx::query("INSERT INTO usage_caps(id,node_id,meter,period,amount,created_by) VALUES ($1,$2,'tokens','day',120,$3)")
        .bind(orch_types::new_id()).bind(world.project).bind(world.ada).execute(world.pool.pool()).await.unwrap();
    change_text(
        &world,
        doc,
        "The endpoint redirects a signed-in user.",
        "The endpoint now redirects all signed-in users.",
    )
    .await;
    let state: (
        Option<chrono::DateTime<chrono::Utc>>,
        chrono::DateTime<chrono::Utc>,
        Option<Id>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT due_at,next_at,claimed_by,reason FROM document_summary_state WHERE document_id=$1",
    )
    .bind(doc)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert!(
        world.engine().summarize_document_due(doc).await.unwrap(),
        "pending state: {state:?}"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "a full shared cap prevents the next model request"
    );
    let topics = world
        .count(
            "SELECT count(*) FROM nodes WHERE kind='topic' AND org_id=$1",
            world.root,
        )
        .await;
    world
        .set(
            world.root,
            "auto.summarizer",
            json!({"kind":"model","provider_id":world.provider,"model_name":common::MODEL}),
        )
        .await;
    let root_doc = world
        .document_beside(
            world.root,
            "Organization guide",
            &["How the organization arranges its projects."],
        )
        .await;
    assert!(
        world
            .engine()
            .summarize_document_due(root_doc)
            .await
            .unwrap()
    );
    let (org,project,scope,purpose):(Id,Id,Id,String)=sqlx::query_as("SELECT org_id,project_id,topic_id,purpose FROM usage_events WHERE origin='orch' AND request_ref='document-summary-2'")
        .fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(
        (org, project, scope, purpose.as_str()),
        (world.root, world.root, world.root, "summary")
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM nodes WHERE kind='topic' AND org_id=$1",
                world.root
            )
            .await,
        topics,
        "summary attribution uses actual scopes without creating a billing topic"
    );
}
