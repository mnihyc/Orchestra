//! The embeddings an org names for search by meaning: a setting whose writer answers for its use, texts made into vectors in the provider's runtime at the named length, counted under the purpose `embeddings`, and refused with the reason when the writer may not use the provider.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_engine::commands::{Commands, Outgoing};
use orch_engine::semantic::{EmbedError, choice};
use orch_link_contract::commands::{
    Command, CommandKind, CommandResult, ProviderEmbed, ProviderEmbedResult, encode_vector,
};
use orch_types::{Id, Json, new_id};
use rig::Scene;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// The model the endpoint offers.
const MODEL: &str = "embed-1";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_embedding_call_survives_a_lost_counting_process_and_is_reconciled_once_even_while_search_is_off()
 {
    let s = Scene::new().await;
    let provider = embeddings_provider(&s).await;
    let seen = script(&s);
    let org = s.tree.org.base.id;
    name(&s, provider, 2).await;
    let call = new_id();
    sqlx::query("INSERT INTO summary_calls (id,node_id,topic_id,provider_id,person_id,model_name,created_at,purpose)
        VALUES ($1,$2,$2,$3,$4,$5,$6,'embeddings')")
        .bind(call).bind(org).bind(provider).bind(s.ada.id()).bind(MODEL).bind(s.rig.world.now())
        .execute(s.rig.db.pool()).await.unwrap();
    let command = Outgoing::new(
        call,
        s.rig.machine_id,
        orch_link_contract::commands::SerialKey::None,
        &ProviderEmbed {
            provider_id: provider,
            protocol: "openai-embeddings".into(),
            model_name: MODEL.into(),
            inputs: vec!["query".into()],
            dimensions: Some(2),
        },
    )
    .unwrap();
    s.rig.commands.issue(command).await.unwrap();
    assert!(matches!(
        s.rig.commands.wait(call).await.unwrap(),
        orch_engine::commands::Outcome::Succeeded(_)
    ));
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE id=$1")
        .bind(call)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        before, 0,
        "the counting process did not consume this retained response"
    );
    s.rig.engine.keep_indexing().await.unwrap();
    let counted: (String, Id, i64) =
        sqlx::query_as("SELECT purpose,user_id,total_tokens FROM usage_events WHERE id=$1")
            .bind(call)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(counted, ("embeddings".into(), s.ada.id(), 7));
    sqlx::query("INSERT INTO summary_calls (id,node_id,topic_id,provider_id,person_id,model_name,created_at,purpose)
        SELECT id,topic_id,topic_id,provider_id,user_id,model,at,'embeddings' FROM usage_events WHERE id=$1")
        .bind(call).execute(s.rig.db.pool()).await.unwrap();
    s.rig.engine.keep_indexing().await.unwrap();
    let total: (i64, i64) =
        sqlx::query_as("SELECT count(*),sum(total_tokens)::bigint FROM usage_events WHERE id=$1")
            .bind(call)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        total,
        (1, 7),
        "a second reconciliation never adds another charge"
    );
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM summary_calls WHERE id=$1")
        .bind(call)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(pending, 0);
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "recovery makes no provider request"
    );
    assert_eq!(s.rig.commands.issued_of("provider.embed"), 1);
}

/// An API provider of the Embeddings format, ready, keeping its key on the scene's machine, owned by Ada.
async fn embeddings_provider(s: &Scene) -> Id {
    let provider = new_id();
    sqlx::query("INSERT INTO providers (id,name,owner_id,org_id,home_id,kind,key_machine_id,machine_id,upstreams,protocols,arrangements,status) VALUES ($1,'Embeddings API',$2,$3,$3,'api',$4,$4,'[{\"protocol\":\"openai-embeddings\",\"base_url\":\"https://embeddings.invalid/v1\",\"options\":{\"auth\":\"none\"}}]',ARRAY['openai-embeddings'],ARRAY['key_separated'],'ready')")
        .bind(provider).bind(s.ada.id()).bind(s.tree.org.base.id).bind(s.rig.machine_id).execute(s.rig.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO provider_models (id,provider_id,model_name,display_name,enabled,source,created_by) VALUES ($1,$2,$3,'Embed',true,'manual',$4)")
        .bind(new_id()).bind(provider).bind(MODEL).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .lets_use("provider", provider, "access.own", &s.ada, json!({}))
        .await;
    provider
}

/// Ada names the provider for the org, asking for vectors of `dimensions`.
async fn name(s: &Scene, provider: Id, dimensions: u32) {
    s.rig
        .world
        .app
        .call(
            NodeSettingSet,
            &[&s.tree.org.base.id, &"search.semantic.embeddings"],
        )
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!({
                "provider_id": provider, "model_name": MODEL, "dimensions": dimensions,
                "query_prefix": "task: search result | query: ", "document_prefix": "title: {title} | text: "
            }),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}

/// The endpoint answers every text with the vector (3, 4, 12) and says it used seven tokens.
fn script(s: &Scene) -> Arc<Mutex<Vec<ProviderEmbed>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderEmbed, move |command| {
            let Command::ProviderEmbed(request) = command else {
                panic!("only a batch of texts reaches this script");
            };
            recorded.lock().unwrap().push(request.clone());
            Ok(CommandResult::ProviderEmbed(ProviderEmbedResult {
                timing: Default::default(),
                vectors: request
                    .inputs
                    .iter()
                    .map(|_| encode_vector(&[3.0, 4.0, 12.0]))
                    .collect(),
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 7,
                    total_tokens: 7,
                    ..Default::default()
                },
                request_ref: None,
            }))
        });
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn texts_become_vectors_of_the_named_length_counted_as_embeddings_for_whoever_named_the_provider()
 {
    let s = Scene::new().await;
    let provider = embeddings_provider(&s).await;
    let seen = script(&s);
    name(&s, provider, 2).await;
    let org = s.tree.org.base.id;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let chosen = choice(&mut conn, org).await.unwrap();
    drop(conn);
    assert!(
        !chosen.enabled,
        "search by meaning is off until it is turned on"
    );
    assert_eq!(chosen.max_disk_share, Some(100));
    assert_eq!(chosen.tuning.fusion_k, 60);
    let named = chosen.embeddings.clone().unwrap();
    assert_eq!(
        named.writer,
        Some(s.ada.id()),
        "whoever named it answers for it"
    );
    let texts = vec![
        named.document("发布流程", "先构建。"),
        named.document("", "rollout"),
    ];
    let made = s
        .rig
        .engine
        .embed_texts(org, &named, texts.clone())
        .await
        .unwrap();
    assert_eq!(made.len(), 2);
    for vector in &made {
        assert_eq!(vector.len(), 2, "cut to the named length");
        assert!(
            (vector[0] - 0.6).abs() < 1e-6 && (vector[1] - 0.8).abs() < 1e-6,
            "and made unit length again: {vector:?}"
        );
    }
    let asked = seen.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].provider_id, provider);
    assert_eq!(asked[0].protocol, "openai-embeddings");
    assert_eq!(asked[0].dimensions, Some(2));
    assert_eq!(asked[0].inputs, texts);
    assert_eq!(asked[0].inputs[0], "title: 发布流程 | text: 先构建。");
    let row: (String, Id, Id, Id, Option<Id>, i64) = sqlx::query_as(
        "SELECT purpose, org_id, project_id, topic_id, user_id, input_tokens FROM usage_events WHERE provider_id = $1",
    )
    .bind(provider)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        row,
        ("embeddings".to_owned(), org, org, org, Some(s.ada.id()), 7),
        "counted apart from turns and summaries, as the org's own, for whoever named it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_who_may_not_use_the_provider_makes_no_request_and_is_told_why() {
    let s = Scene::new().await;
    let provider = embeddings_provider(&s).await;
    let seen = script(&s);
    name(&s, provider, 2).await;
    let org = s.tree.org.base.id;
    // The value stays; whoever wrote it last is now somebody who may not use the provider.
    sqlx::query("UPDATE settings SET updated_by = $1 WHERE node_id = $2 AND key = 'search.semantic.embeddings'")
        .bind(s.cai.id())
        .bind(org)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let mut conn = s.rig.db.acquire().await.unwrap();
    let named = choice(&mut conn, org).await.unwrap().embeddings.unwrap();
    drop(conn);
    assert_eq!(named.writer, Some(s.cai.id()));
    let refused = s
        .rig
        .engine
        .embed_texts(org, &named, vec!["rollout".to_owned()])
        .await;
    assert!(
        matches!(
            refused,
            Err(EmbedError::Unavailable {
                reason: "access",
                ..
            })
        ),
        "{refused:?}"
    );
    assert!(seen.lock().unwrap().is_empty(), "nothing was sent");
    let counted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE provider_id = $1")
            .bind(provider)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(counted, 0);
    let value: Json = sqlx::query_scalar(
        "SELECT value FROM settings WHERE node_id = $1 AND key = 'search.semantic.embeddings'",
    )
    .bind(org)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(value["model_name"], MODEL);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_use_retains_the_writers_model_limits_without_requiring_thread_creation() {
    let s = Scene::new().await;
    let provider = embeddings_provider(&s).await;
    let seen = script(&s);
    name(&s, provider, 2).await;
    let org = s.tree.org.base.id;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let mut named = choice(&mut conn, org).await.unwrap().embeddings.unwrap();
    drop(conn);
    named.writer = Some(s.cai.id());
    // A former settings writer can retain private provider use without an interactive role at its org.
    s.rig
        .lets_use(
            "provider",
            provider,
            "provider.use",
            &s.cai,
            json!({"models":["another-model"]}),
        )
        .await;
    let refused = s
        .rig
        .engine
        .embed_texts(org, &named, vec!["rollout".into()])
        .await;
    assert!(
        matches!(
            refused,
            Err(EmbedError::Unavailable {
                reason: "access",
                ..
            })
        ),
        "{refused:?}"
    );
    assert!(seen.lock().unwrap().is_empty());
    sqlx::query("UPDATE permission_entries SET options=$3 WHERE object_kind='provider' AND object_id=$1 AND holder_kind='user' AND holder_id=$2 AND permission='provider.use' AND removed_at IS NULL")
        .bind(provider).bind(s.cai.id()).bind(json!({"models":[MODEL]})).execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .engine
        .embed_texts(org, &named, vec!["rollout".into()])
        .await
        .unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    let writer: Id = sqlx::query_scalar("SELECT user_id FROM usage_events WHERE provider_id=$1")
        .bind(provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(writer, s.cai.id());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_spending_switch_and_provider_deletion_cancel_a_real_embedding_without_releasing_its_reply()
 {
    use orch_api_contract::environment::EnvironmentDisable;
    use orch_api_contract::resources::{
        AgentUpdate, AgentUpdateBody, ProviderDelete, ProviderDisable, ProviderModelUpdate,
        ProviderModelUpdateBody,
    };
    use orch_link_contract::commands::Empty;
    for kind in ["provider", "model", "agent", "environment", "delete"] {
        let s = Scene::new().await;
        let provider = embeddings_provider(&s).await;
        sqlx::query("UPDATE providers SET agent_id=$2,held_by_environment_id=$3 WHERE id=$1")
            .bind(provider)
            .bind(s.on.agent)
            .bind(s.on.environment)
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        let seen = script(&s);
        s.rig.machine.handler.hold(CommandKind::ProviderEmbed);
        s.rig.machine.handler.on(CommandKind::ProviderStop, |_| {
            Ok(CommandResult::ProviderStop(Empty {}))
        });
        s.rig.machine.handler.on(CommandKind::AgentStop, |_| {
            Ok(CommandResult::AgentStop(Empty {}))
        });
        let org = s.tree.org.base.id;
        name(&s, provider, 2).await;
        let named = choice(&mut s.rig.db.acquire().await.unwrap(), org)
            .await
            .unwrap()
            .embeddings
            .unwrap();
        let engine = s.rig.engine.clone();
        let work = tokio::spawn(async move {
            engine
                .embed_texts(org, &named, vec!["Keep this exact model and length".into()])
                .await
        });
        rig::eventually(
            "the real embedding is accepted but its reply is held",
            || async {
                s.rig
                    .machine
                    .handler
                    .executions()
                    .iter()
                    .any(|entry| entry.kind == CommandKind::ProviderEmbed)
            },
        )
        .await;
        let id = s
            .rig
            .machine
            .handler
            .executions()
            .into_iter()
            .find(|entry| entry.kind == CommandKind::ProviderEmbed)
            .unwrap()
            .id;
        let model: Id = sqlx::query_scalar(
            "SELECT id FROM provider_models WHERE provider_id=$1 AND model_name=$2",
        )
        .bind(provider)
        .bind(MODEL)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
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
        rig::eventually(
            "administrative cancellation settles the accepted embedding",
            || async { work.is_finished() },
        )
        .await;
        let error = work.await.unwrap().unwrap_err();
        let expected = match kind {
            "provider" => "provider_disabled",
            "delete" => "provider_removed",
            "model" => "model_disabled",
            _ => "provider",
        };
        assert!(
            matches!(error,EmbedError::Unavailable{reason,..} if reason==expected),
            "the pause is stable rather than a vendor failure: {kind}: {error:?}"
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "the native reply was never released: {kind}"
        );
        assert!(
            matches!(
                s.rig.commands.wait(id).await.unwrap(),
                orch_engine::commands::Outcome::Cancelled
            ),
            "the real native command is cancelled: {kind}"
        );
        let counted: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE id=$1")
            .bind(id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(
            counted, 0,
            "an unreported cancelled request invents no usage: {kind}"
        );
        let failures: i64 =
            sqlx::query_scalar("SELECT count(*) FROM model_call_recovery WHERE node_id=$1")
                .bind(org)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(
            failures, 0,
            "no failed-call or guard attempt is reserved: {kind}"
        );
    }
}
