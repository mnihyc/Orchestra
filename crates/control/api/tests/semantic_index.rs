//! The index of meaning of an org: its text cut and made into vectors in the background, newest first; a pause with its reason before the disk share is passed; a change of model that builds a new generation while the old one answers, the same model served elsewhere and a shorter length that need none; and without the vector extension, everything else working and the reason said.
//!
//! The vectors need the database's `vector` extension. Where the test database has none, each case checks that
//! search by meaning says so and makes no request, and stops there; with it, the whole case runs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::resources::{ProviderDelete, ProviderDisable, ProviderEnable};
use orch_api_contract::search::{SearchStreamEvent, SearchStreamList, SearchStreamListQuery};
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_link_contract::commands::{
    Command, CommandKind, CommandResult, ProviderEmbed, ProviderEmbedResult, encode_vector,
};
use orch_types::{Hash, Id, Json, new_id};
use rig::Scene;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// The length the scripted endpoint gives every vector.
const LENGTH: usize = 8;

/// A vector of a text as the scripted model `model` makes it: numbers from the hash of the model and the text,
/// so that one model gives one text one vector and another model another.
fn vector_of(model: &str, text: &str) -> Vec<f32> {
    let hash = Hash::of(format!("{model}\n{text}").as_bytes());
    hash.as_bytes()[..LENGTH]
        .iter()
        .map(|byte| f32::from(*byte) / 255.0 - 0.5)
        .collect()
}

/// The endpoint: every text gets the vector its model makes, whatever length was asked (as llama.cpp's server
/// does); `same_as` makes a model answer as another does, for the same model served elsewhere.
fn script(
    s: &Scene,
    same_as: &'static [(&'static str, &'static str)],
) -> Arc<Mutex<Vec<ProviderEmbed>>> {
    script_with_pause(s, same_as, None)
}

fn script_with_pause(
    s: &Scene,
    same_as: &'static [(&'static str, &'static str)],
    paused: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Arc<Mutex<Vec<ProviderEmbed>>> {
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
            if paused
                .as_ref()
                .is_some_and(|paused| paused.load(std::sync::atomic::Ordering::Relaxed))
                && request.inputs.iter().any(|text| {
                    text.starts_with("title: ") && !text.starts_with("title: none | text: ")
                })
            {
                return Err(orch_link_contract::commands::CommandError::new(
                    orch_link_contract::commands::ErrorCode::Internal,
                    "the scripted endpoint pauses document requests",
                ));
            }
            let model = same_as
                .iter()
                .find(|(name, _)| *name == request.model_name)
                .map_or(request.model_name.as_str(), |(_, real)| *real);
            Ok(CommandResult::ProviderEmbed(ProviderEmbedResult {
                timing: Default::default(),
                vectors: request
                    .inputs
                    .iter()
                    .map(|text| encode_vector(&vector_of(model, text)))
                    .collect(),
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 5,
                    total_tokens: 5,
                    ..Default::default()
                },
                request_ref: None,
            }))
        });
    seen
}

/// An API provider of the Embeddings format with these models, ready, owned by Ada.
async fn embeddings_provider(s: &Scene, models: &[&str]) -> Id {
    let provider = new_id();
    sqlx::query("INSERT INTO providers (id,name,owner_id,org_id,home_id,kind,key_machine_id,machine_id,upstreams,protocols,arrangements,status) VALUES ($1,$2,$3,$4,$4,'api',$5,$5,'[{\"protocol\":\"openai-embeddings\",\"base_url\":\"https://embeddings.invalid/v1\",\"options\":{\"auth\":\"none\"}}]',ARRAY['openai-embeddings'],ARRAY['key_separated'],'ready')")
        .bind(provider).bind(format!("Embeddings {provider}")).bind(s.ada.id()).bind(s.tree.org.base.id).bind(s.rig.machine_id)
        .execute(s.rig.db.pool()).await.unwrap();
    for model in models {
        sqlx::query("INSERT INTO provider_models (id,provider_id,model_name,display_name,enabled,source,created_by) VALUES ($1,$2,$3,$3,true,'manual',$4)")
            .bind(new_id()).bind(provider).bind(*model).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    }
    s.rig
        .lets_use("provider", provider, "access.own", &s.ada, json!({}))
        .await;
    provider
}

async fn set(s: &Scene, key: &str, value: Json) {
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&s.tree.org.base.id, &key])
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

async fn name(s: &Scene, provider: Id, model: &str, dimensions: u32) {
    set(
        s,
        "search.semantic.embeddings",
        json!({"provider_id": provider, "model_name": model, "dimensions": dimensions,
               "query_prefix": "task: search result | query: ", "document_prefix": "title: {title} | text: "}),
    )
    .await;
}

async fn document(s: &Scene, title: &str, markdown: &str) -> Id {
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: title.into(),
            blocks: None,
            markdown: Some(markdown.into()),
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id
}

/// Turns of the indexer until the org's index stands as `state`, or the patience runs out.
async fn index_until(s: &Scene, state: &str) -> orch_engine::semantic::state::OrgState {
    let org = s.tree.org.base.id;
    for _ in 0..40 {
        s.rig.engine.keep_indexing().await.unwrap();
        let now = s.rig.engine.semantic_state(org).await.unwrap();
        if now.state == state {
            return now;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let now = s.rig.engine.semantic_state(org).await.unwrap();
    panic!("the index never stood as {state}: {now:?}");
}

/// Whether the test database can keep vectors; where it cannot, checks that nothing is asked and the reason is said.
async fn vectors_or_reason(s: &Scene, seen: &Arc<Mutex<Vec<ProviderEmbed>>>) -> bool {
    if s.rig.engine.semantic_available().await.unwrap().is_ok() {
        return true;
    }
    s.rig.engine.keep_indexing().await.unwrap();
    let now = s
        .rig
        .engine
        .semantic_state(s.tree.org.base.id)
        .await
        .unwrap();
    assert_eq!(now.state, "unavailable");
    assert_eq!(now.reason.as_deref(), Some("extension"));
    assert!(now.detail.unwrap().contains("pgvector"));
    assert!(
        seen.lock().unwrap().is_empty(),
        "nothing is sent while vectors cannot be kept"
    );
    eprintln!("this test database has no vector extension; the rest of this case needs one");
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn counting_all_sources_precedes_progress_and_paid_content_embeddings() {
    let s = Scene::new().await;
    let seen = script(&s, &[]);
    let provider = embeddings_provider(&s, &["embed-a"]).await;
    for title in ["First", "Second", "Third"] {
        document(&s, title, "A release can be rolled back.").await;
    }
    set(&s, "search.semantic.enabled", json!(true)).await;
    set(&s, "search.semantic.max_disk_share", json!(null)).await;
    set(
        &s,
        "search.semantic.tuning",
        json!({
            "chunk_tokens":400,"batch_tokens":4096,"candidates":50,"oversample":4,"fusion_k":60,
            "same_model_cosine_milli":990,"interval_secs":10,"query_wait_secs":3,"source_batch":1
        }),
    )
    .await;
    name(&s, provider, "embed-a", 4).await;
    if !vectors_or_reason(&s, &seen).await {
        return;
    }
    s.rig.engine.keep_indexing().await.unwrap();
    let state = s
        .rig
        .engine
        .semantic_state(s.tree.org.base.id)
        .await
        .unwrap();
    assert_eq!(state.state, "indexing");
    assert_eq!(state.reason.as_deref(), Some("counting"));
    assert!(
        state.seconds_left.is_none(),
        "an incomplete inventory has no whole-index duration"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "only model probes, no content, before the inventory is complete"
    );
    assert!(matches!(
        s.rig
            .engine
            .meaning(s.tree.org.base.id, "rollback")
            .await
            .unwrap(),
        orch_engine::semantic::query::Meaning::Unavailable(_)
    ));
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "a query does not spend while the index has no vector"
    );
    let done = index_until(&s, "idle").await;
    assert_eq!(done.pending_tokens, 0);
    assert!(seen.lock().unwrap().len() > 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_by_meaning_is_off_until_turned_on_and_named_and_says_where_it_stands() {
    let s = Scene::new().await;
    let seen = script(&s, &[]);
    let org = s.tree.org.base.id;
    document(&s, "Rollout plan", "Build first, then ship.").await;
    s.rig.engine.keep_indexing().await.unwrap();
    assert_eq!(s.rig.engine.semantic_state(org).await.unwrap().state, "off");
    set(&s, "search.semantic.enabled", json!(true)).await;
    s.rig.engine.keep_indexing().await.unwrap();
    assert_eq!(
        s.rig.engine.semantic_state(org).await.unwrap().state,
        "not_named"
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "nothing named, nothing sent"
    );
    let provider = embeddings_provider(&s, &["embed-a"]).await;
    name(&s, provider, "embed-a", 4).await;
    if !vectors_or_reason(&s, &seen).await {
        return;
    }
    // A vector takes more than a short sentence does; without a share, nothing holds it back.
    set(&s, "search.semantic.max_disk_share", json!(null)).await;
    let done = index_until(&s, "idle").await;
    assert!(done.allowed_bytes.is_none());
    assert_eq!(done.pending_tokens, 0);
    assert!(done.active.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_indexer_makes_vectors_newest_first_and_pauses_before_the_disk_share_is_passed() {
    let s = Scene::new().await;
    let seen = script(&s, &[]);
    let provider = embeddings_provider(&s, &["embed-a"]).await;
    set(&s, "search.semantic.enabled", json!(true)).await;
    name(&s, provider, "embed-a", 4).await;
    document(&s, "发布流程", "先构建，再部署到预发环境，最后上线。").await;
    s.rig.world.advance(chrono::Duration::days(2));
    document(
        &s,
        "Rollout plan",
        "Build first, deploy to staging, then ship.",
    )
    .await;
    if !vectors_or_reason(&s, &seen).await {
        return;
    }
    // A share of nothing: the generation is begun (its probe texts are made) and nothing more is sent.
    set(&s, "search.semantic.max_disk_share", json!(0)).await;
    let paused = index_until(&s, "paused").await;
    assert_eq!(paused.reason.as_deref(), Some("disk_share"));
    assert!(
        paused.used_bytes <= paused.allowed_bytes.unwrap(),
        "metadata also stays within the disk share"
    );
    assert!(paused.detail.unwrap().contains("bytes"));
    let probes = seen.lock().unwrap().len();
    assert_eq!(probes, 1, "only the probe texts were made");
    // No limit: everything is indexed, the newest document's text first.
    set(&s, "search.semantic.max_disk_share", json!(null)).await;
    let done = index_until(&s, "idle").await;
    assert_eq!(done.pending_tokens, 0);
    assert!(done.used_bytes > 0);
    assert!(done.allowed_bytes.is_none());
    let asked = seen.lock().unwrap().clone();
    let texts: Vec<String> = asked[probes..]
        .iter()
        .flat_map(|request| request.inputs.clone())
        .collect();
    let rollout = texts
        .iter()
        .position(|text| text.contains("Rollout plan"))
        .unwrap();
    let chinese = texts
        .iter()
        .position(|text| text.contains("发布流程"))
        .unwrap();
    assert!(rollout < chinese, "the newer document first: {texts:?}");
    assert!(
        texts.iter().all(|text| text.starts_with("title: ")),
        "the document prefix with its title"
    );
    assert!(asked.iter().all(|request| request.dimensions == Some(4)));
    let stored: Vec<i32> =
        sqlx::query_scalar("SELECT vector_dims(embedding::vector) FROM search_vectors")
            .fetch_all(s.rig.db.pool())
            .await
            .unwrap();
    assert!(
        !stored.is_empty() && stored.iter().all(|length| *length == 4),
        "cut to the length named: {stored:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_model_builds_a_new_generation_and_the_same_model_elsewhere_or_shorter_needs_none() {
    let s = Scene::new().await;
    // "served-a" is the model "embed-a" served under another name.
    let seen = script(&s, &[("served-a", "embed-a")]);
    let org = s.tree.org.base.id;
    let provider = embeddings_provider(&s, &["embed-a", "embed-b"]).await;
    let elsewhere = embeddings_provider(&s, &["served-a"]).await;
    set(&s, "search.semantic.enabled", json!(true)).await;
    set(&s, "search.semantic.max_disk_share", json!(null)).await;
    name(&s, provider, "embed-a", 4).await;
    document(
        &s,
        "Rollout plan",
        "Build first, deploy to staging, then ship.",
    )
    .await;
    document(&s, "发布流程", "先构建，再部署到预发环境，最后上线。").await;
    if !vectors_or_reason(&s, &seen).await {
        return;
    }
    let first = index_until(&s, "idle").await.active.unwrap();
    // Updating names, model offers or access must not suspend a still-valid endpoint's old index.
    sqlx::query("UPDATE providers SET revision = revision + 1 WHERE id = $1")
        .bind(provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let answer = s.rig.engine.meaning(org, "rollback").await.unwrap();
    assert!(
        matches!(answer, orch_engine::semantic::query::Meaning::Found { .. }),
        "unrelated provider edits leave the active model answering: {answer:?}"
    );
    let pieces: i64 = sqlx::query_scalar("SELECT count(*) FROM search_chunks WHERE org_id = $1")
        .bind(org)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    // The same model elsewhere: the probe texts agree, the generation moves, nothing is made again.
    let before = seen.lock().unwrap().len();
    name(&s, elsewhere, "served-a", 4).await;
    let moved = index_until(&s, "idle").await.active.unwrap();
    assert_eq!(moved.id, first.id);
    assert_eq!(moved.provider_id, elsewhere);
    assert_eq!(
        seen.lock().unwrap().len(),
        before + 1,
        "only the probe texts were made again"
    );
    // Editing an endpoint under the same provider and routing name still requires the stored probes.
    sqlx::query("UPDATE providers SET upstreams = jsonb_set(upstreams, '{0,base_url}', to_jsonb('https://moved.invalid/v1'::text)), revision = revision + 1 WHERE id = $1")
        .bind(elsewhere)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        matches!(
            s.rig.engine.meaning(org, "rollback").await.unwrap(),
            orch_engine::semantic::query::Meaning::Unavailable(_)
        ),
        "an unchecked endpoint is never compared with stored vectors"
    );
    let before = seen.lock().unwrap().len();
    let checked = index_until(&s, "idle").await.active.unwrap();
    assert_eq!(
        checked.id, first.id,
        "the same model at the edited endpoint reuses its generation"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        before + 1,
        "the endpoint is checked by its probes"
    );
    let cached_before: Vec<f32> =
        sqlx::query_scalar("SELECT embedding FROM search_query_vectors WHERE generation_id=$1")
            .bind(first.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    // Shorter vectors of the same model: cut in place, the index made anew, nothing made again.
    let before = seen.lock().unwrap().len();
    name(&s, elsewhere, "served-a", 2).await;
    let shorter = index_until(&s, "idle").await.active.unwrap();
    assert_eq!(shorter.id, first.id);
    assert_eq!(shorter.dimensions, Some(2));
    assert_eq!(seen.lock().unwrap().len(), before + 1);
    let before_query = seen.lock().unwrap().len();
    assert!(matches!(
        s.rig.engine.meaning(org, "rollback").await.unwrap(),
        orch_engine::semantic::query::Meaning::Found { .. }
    ));
    assert_eq!(
        seen.lock().unwrap().len(),
        before_query,
        "a shortened generation reuses its shortened query cache without another embedding"
    );
    // A query admitted before shortening can finish after it and write its former length.
    sqlx::query("UPDATE search_query_vectors SET embedding=$2 WHERE generation_id=$1")
        .bind(first.id)
        .bind(&cached_before)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        s.rig.engine.meaning(org, "rollback").await.unwrap(),
        orch_engine::semantic::query::Meaning::Found { .. }
    ));
    assert_eq!(
        seen.lock().unwrap().len(),
        before_query,
        "an in-flight old-length cache fill is shortened without another paid query"
    );
    // Another model, with room for both: a new generation is built and then answers; the old one goes.
    let before = seen.lock().unwrap().len();
    name(&s, provider, "embed-b", 2).await;
    let rebuilt = index_until(&s, "idle").await.active.unwrap();
    assert_ne!(rebuilt.id, first.id);
    assert_eq!(rebuilt.model_name, "embed-b");
    let made: usize = seen.lock().unwrap()[before..]
        .iter()
        .map(|request| request.inputs.len())
        .sum();
    assert!(
        made >= usize::try_from(pieces).unwrap(),
        "every piece made again: {made} of {pieces}"
    );
    let generations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM search_generations WHERE org_id = $1")
            .bind(org)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        generations, 1,
        "the old generation is removed once the new one answers"
    );
    // A model whose generation the share cannot hold beside the present one waits for an admin's choice.
    // A share that holds the present vectors and a quarter more, not a second set of them.
    let used = s.rig.engine.semantic_state(org).await.unwrap();
    let share = (used.used_bytes + used.used_bytes / 4) * 100 / used.content_bytes.max(1) + 1;
    set(&s, "search.semantic.max_disk_share", json!(share)).await;
    name(&s, provider, "embed-a", 2).await;
    let waiting = index_until(&s, "waiting").await;
    assert_eq!(waiting.reason.as_deref(), Some("choice"));
    assert!(
        waiting.rebuild_bytes.unwrap() > 0,
        "the size is shown before the choice"
    );
    assert_eq!(
        waiting.active.unwrap().id,
        rebuilt.id,
        "the present model still answers"
    );
    // Rebuilding in place removes the present generation; a new one is built where it was.
    s.rig.engine.semantic_rebuild_in_place(org).await.unwrap();
    let rebuilt_again = index_until(&s, "idle").await.active.unwrap();
    assert_eq!(rebuilt_again.model_name, "embed-a");
    assert_ne!(rebuilt_again.id, rebuilt.id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_chunk_size_keeps_the_old_layout_answering_until_the_replacement_is_complete_and_a_rename_invalidates_its_prompt()
 {
    let s = Scene::new().await;
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = script_with_pause(&s, &[], Some(Arc::clone(&paused)));
    let org = s.tree.org.base.id;
    let provider = embeddings_provider(&s, &["embed-a"]).await;
    set(&s, "search.semantic.enabled", json!(true)).await;
    set(&s, "search.semantic.max_disk_share", json!(null)).await;
    name(&s, provider, "embed-a", 4).await;
    let doc = document(
        &s,
        "Rollout plan",
        &"Build, verify the release and keep a rollback available. ".repeat(80),
    )
    .await;
    if !vectors_or_reason(&s, &seen).await {
        return;
    }
    let first = index_until(&s, "idle").await.active.unwrap();
    let count = |generation| {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM search_chunks WHERE generation_id = $1")
            .bind(generation)
            .fetch_one(s.rig.db.pool())
    };
    let original_chunks = count(first.id).await.unwrap();
    paused.store(true, std::sync::atomic::Ordering::Relaxed);
    set(
        &s,
        "search.semantic.tuning",
        json!({
            "chunk_tokens":64,"batch_tokens":4096,"candidates":50,"oversample":4,"fusion_k":60,
            "same_model_cosine_milli":990,"interval_secs":10,"query_wait_secs":3
        }),
    )
    .await;
    let during = index_until(&s, "paused").await;
    assert_eq!(during.active.as_ref().unwrap().id, first.id);
    let building = during.building.unwrap();
    assert_ne!(
        building.id, first.id,
        "a chunk-size change builds a generation"
    );
    assert_eq!(
        count(first.id).await.unwrap(),
        original_chunks,
        "the answering generation keeps its cuts"
    );
    assert!(
        count(building.id).await.unwrap() > original_chunks,
        "shorter chunks have their own layout"
    );
    let orch_engine::semantic::query::Meaning::Found { near, .. } =
        s.rig.engine.meaning(org, "rollback").await.unwrap()
    else {
        panic!("the old generation must keep answering while the new one is paused");
    };
    assert!(near.iter().any(|part| part.source_id == doc));
    paused.store(false, std::sync::atomic::Ordering::Relaxed);
    let complete = index_until(&s, "idle").await;
    assert_eq!(complete.active.unwrap().id, building.id);
    assert_eq!(
        count(first.id).await.unwrap(),
        0,
        "retired cuts go with their generation"
    );
    sqlx::query("UPDATE documents SET title = 'Revised rollout' WHERE id = $1")
        .bind(doc)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let orch_engine::semantic::query::Meaning::Found { near, .. } =
        s.rig.engine.meaning(org, "rollback").await.unwrap()
    else {
        panic!("an active generation still answers");
    };
    assert!(
        near.iter().all(|part| part.source_id != doc),
        "vectors with the former prompt title are stale immediately"
    );
    let before = seen.lock().unwrap().len();
    index_until(&s, "idle").await;
    assert!(
        seen.lock().unwrap()[before..]
            .iter()
            .flat_map(|call| &call.inputs)
            .any(|text| text.starts_with("title: Revised rollout | text: ")),
        "a rename re-embeds the title in its prompt"
    );
    // A long source reports completed batches rather than waiting until its last piece to show progress.
    set(
        &s,
        "search.semantic.tuning",
        json!({
            "chunk_tokens":64,"batch_tokens":64,"candidates":50,"oversample":4,"fusion_k":60,
            "same_model_cosine_milli":990,"interval_secs":1,"query_wait_secs":3
        }),
    )
    .await;
    s.rig.engine.semantic_rebuild_in_place(org).await.unwrap();
    s.rig
        .machine
        .handler
        .take_time(std::time::Duration::from_millis(500));
    let mut partial = None;
    for _ in 0..8 {
        s.rig.engine.keep_indexing().await.unwrap();
        let state = s.rig.engine.semantic_state(org).await.unwrap();
        if state.pending_tokens > 0 && state.pending_tokens < state.tokens {
            partial = Some(state);
            break;
        }
    }
    assert!(
        partial.is_some(),
        "a completed batch must appear in progress before the whole source finishes"
    );
    s.rig.machine.handler.take_time(std::time::Duration::ZERO);
    index_until(&s, "idle").await;
    // A replacement's writer may use that provider alone; the old generation retains its own writer.
    let mut conn = s.rig.db.acquire().await.unwrap();
    let mut old = orch_engine::semantic::choice(&mut conn, org)
        .await
        .unwrap()
        .embeddings
        .unwrap();
    drop(conn);
    old.writer = Some(s.cai.id());
    assert!(
        matches!(
            s.rig
                .engine
                .embed_texts(org, &old, vec!["query".into()])
                .await,
            Err(orch_engine::semantic::EmbedError::Unavailable {
                reason: "access",
                ..
            })
        ),
        "the replacement's writer has no use of the old provider"
    );
    let replacement = embeddings_provider(&s, &["embed-b"]).await;
    s.rig
        .lets_use("provider", replacement, "provider.use", &s.cai, json!({}))
        .await;
    name(&s, replacement, "embed-b", 4).await;
    sqlx::query("UPDATE settings SET updated_by = $1 WHERE node_id = $2 AND key = 'search.semantic.embeddings'")
        .bind(s.cai.id()).bind(org).execute(s.rig.db.pool()).await.unwrap();
    paused.store(true, std::sync::atomic::Ordering::Relaxed);
    let waiting = index_until(&s, "paused").await;
    assert_eq!(waiting.active.as_ref().unwrap().model_name, "embed-a");
    assert_eq!(
        waiting
            .building
            .as_ref()
            .unwrap_or_else(|| panic!("the replacement must be building: {waiting:?}"))
            .model_name,
        "embed-b"
    );
    assert!(
        matches!(
            s.rig.engine.meaning(org, "rollback").await.unwrap(),
            orch_engine::semantic::query::Meaning::Found { .. }
        ),
        "the old generation still answers under its original writer's current authority"
    );
    paused.store(false, std::sync::atomic::Ordering::Relaxed);
    let finished = index_until(&s, "idle").await;
    assert_eq!(finished.active.unwrap().model_name, "embed-b");
    let writers: Vec<Id> =
        sqlx::query_scalar("SELECT DISTINCT user_id FROM usage_events WHERE provider_id = $1")
            .bind(replacement)
            .fetch_all(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        writers,
        vec![s.cai.id()],
        "replacement embeddings are counted to their own writer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_off_an_embeddings_provider_stops_its_batch_and_keeps_stored_search_until_it_resumes()
 {
    use orch_engine::commands::{Commands as _, Outcome};
    use orch_engine::semantic::query::Meaning;
    use std::time::Duration;
    // Readiness includes inventory and admission; an accepted stop must release its batch within a second.
    const START_WAIT: Duration = Duration::from_secs(5);
    const STOP_WAIT: Duration = Duration::from_secs(1);
    let s = Scene::new().await;
    let seen = script(&s, &[]);
    let org = s.tree.org.base.id;
    let provider = embeddings_provider(&s, &["embed-a"]).await;
    set(&s, "search.semantic.enabled", json!(true)).await;
    set(&s, "search.semantic.max_disk_share", json!(null)).await;
    name(&s, provider, "embed-a", 4).await;
    document(
        &s,
        "Rollback checklist",
        "Keep the previous release for rollback.",
    )
    .await;
    if !vectors_or_reason(&s, &seen).await {
        return;
    }
    let original = index_until(&s, "idle").await.active.unwrap();
    assert!(
        matches!(s.rig.engine.meaning(org, "rollback").await.unwrap(), Meaning::Found { ref near, paused: None, .. } if !near.is_empty())
    );
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM search_vectors WHERE generation_id=$1")
            .bind(original.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    document(
        &s,
        "New checklist",
        &"New text to index after a rollback. ".repeat(80),
    )
    .await;
    s.rig.machine.handler.hold(CommandKind::ProviderEmbed);
    let engine = s.rig.engine.clone();
    let indexing = tokio::spawn(async move { engine.keep_indexing().await });
    let call = tokio::time::timeout(START_WAIT, async {
        loop {
            if let Some(call) = sqlx::query_scalar::<_, Id>("SELECT id FROM summary_calls WHERE provider_id=$1 AND purpose='embeddings' ORDER BY created_at,id LIMIT 1")
                .bind(provider).fetch_optional(s.rig.db.pool()).await.unwrap() { break call; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the added content starts an embedding batch");
    tokio::time::timeout(START_WAIT, s.rig.machine.handler.started(call))
        .await
        .unwrap();
    let unissued = new_id();
    sqlx::query("INSERT INTO summary_calls(id,node_id,topic_id,provider_id,person_id,model_name,created_at,purpose) VALUES($1,$2,$2,$3,$4,'embed-a',$5,'embeddings')")
        .bind(unissued).bind(org).bind(provider).bind(s.ada.id()).bind(s.rig.world.now())
        .execute(s.rig.db.pool()).await.unwrap();
    let before = s.rig.commands.issued_of("provider.embed");
    s.rig
        .world
        .app
        .call(ProviderDisable, &[&provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(
        s.rig
            .commands
            .status(call)
            .await
            .unwrap()
            .is_some_and(|info| matches!(info.outcome, Some(Outcome::Cancelled))),
        "Disable cancels the actual command before it answers"
    );
    tokio::time::timeout(STOP_WAIT, indexing)
        .await
        .expect("the stopped batch releases its index claim")
        .unwrap()
        .unwrap();
    let paused = s.rig.engine.semantic_state(org).await.unwrap();
    assert_eq!(
        (paused.state.as_str(), paused.reason.as_deref()),
        ("paused", Some("provider_disabled"))
    );
    assert_eq!(
        paused.active.unwrap().id,
        original.id,
        "stored vectors are kept"
    );
    for _ in 0..3 {
        s.rig.engine.keep_indexing().await.unwrap();
    }
    assert_eq!(
        s.rig.commands.issued_of("provider.embed"),
        before,
        "a switched-off provider is never retried"
    );
    assert!(
        matches!(s.rig.engine.meaning(org, "rollback").await.unwrap(), Meaning::Found { ref near, paused: Some(ref reason), .. } if !near.is_empty() && reason=="provider_disabled")
    );
    assert!(
        matches!(s.rig.engine.meaning(org, "a fresh uncached query").await.unwrap(), Meaning::Unavailable(ref reason) if reason=="provider_disabled")
    );
    assert_eq!(
        s.rig.commands.issued_of("provider.embed"),
        before,
        "cached or fresh searches spend nothing while off"
    );
    let query: SearchStreamListQuery =
        serde_json::from_value(json!({"q":"rollback","sort":"relevance","immediate":true}))
            .unwrap();
    let response = s
        .rig
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&s.ada.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let events: Vec<SearchStreamEvent> = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert!(events.iter().any(|event| matches!(event, SearchStreamEvent::Source {source,state,reason:Some(reason),received} if source=="meaning" && state=="done" && reason=="provider_disabled" && *received>0)), "a cached-vector search reports the index pause without failing its source");
    assert!(events.iter().any(|event| matches!(event, SearchStreamEvent::Source {source,state,received,..} if source=="words" && state=="done" && *received>0)), "words continue throughout the pause");
    let cancelled_usage: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE id=$1")
        .bind(call)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        cancelled_usage, 0,
        "an uncompleted cancellation records no invented use or failure"
    );
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM search_vectors WHERE generation_id=$1")
            .bind(original.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(retained, stored);
    s.rig.machine.handler.release();
    s.rig
        .world
        .app
        .call(ProviderEnable, &[&provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    {
        let mut conn = s.rig.db.acquire().await.unwrap();
        assert_eq!(
            orch_db::queries::semantic::provider_pause_reason(&mut conn, provider)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            orch_db::queries::semantic::embedding_pause_reason(&mut conn, unissued, provider)
                .await
                .unwrap(),
            Some("provider_disabled"),
            "Enable cannot revive admission stopped before command issuance"
        );
    }
    let resumed = index_until(&s, "idle").await;
    assert_eq!(
        resumed.active.unwrap().id,
        original.id,
        "Enable resumes the existing model and index"
    );
    assert_eq!(resumed.reason, None);
    assert!(
        s.rig.commands.issued_of("provider.embed") > before,
        "the unfinished content resumes without another setup act"
    );
    s.rig
        .world
        .app
        .call(ProviderDelete, &[&provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    s.rig.engine.keep_indexing().await.unwrap();
    let removed = s.rig.engine.semantic_state(org).await.unwrap();
    assert_eq!(
        (removed.state.as_str(), removed.reason.as_deref()),
        ("paused", Some("provider_removed"))
    );
    let before = s.rig.commands.issued_of("provider.embed");
    assert!(
        matches!(s.rig.engine.meaning(org, "rollback").await.unwrap(), Meaning::Found { ref near, paused: Some(ref reason), .. } if !near.is_empty() && reason=="provider_removed")
    );
    assert_eq!(s.rig.commands.issued_of("provider.embed"), before);
}
