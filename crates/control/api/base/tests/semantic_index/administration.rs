//! Drive the actual native indexer/model/publication stages with a retained administrative key.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::admin::{
    AdminSemanticSearchKeep, AdminSemanticSearchRebuild, SemanticSearchKeepBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::semantic as rows;

async fn administrative_key(s: &Scene, operation: &str) -> String {
    s.rig.world.make_admin(&s.ada.person).await;
    let policy: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"org","id":s.tree.org.base.id,"reach":"self"},
        "resources":["org"],"allow":[{"operation":operation,"actions":["manage"]}]
    }]}))
    .unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut s.rig.db.acquire().await.unwrap(),
        &NewApiToken {
            id: new_id(),
            user_id: s.ada.id(),
            name: "Native index work",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::seconds(10)),
        },
        Some(&serde_json::to_value(policy).unwrap()),
    )
    .await
    .unwrap();
    secret
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn administrative_index_work_rechecks_after_the_model_before_generation_or_vector_publication()
 {
    for content in [false, true] {
        for expire in [false, true] {
            let s = Scene::without_supervisor().await;
            let seen = script(&s, &[]);
            let org = s.tree.org.base.id;
            let provider = embeddings_provider(&s, &["embed-a"]).await;
            set(&s, "search.semantic.enabled", json!(true)).await;
            set(&s, "search.semantic.max_disk_share", json!(null)).await;
            name(&s, provider, "embed-a", 4).await;
            let availability = s.rig.engine.semantic_available().await.unwrap();
            assert!(
                availability.is_ok(),
                "this deciding native publication control requires the vector-enabled test database: {availability:?}; content={content} expire={expire}"
            );
            let original = if content {
                document(
                    &s,
                    "Existing source",
                    "A completed original vector remains available.",
                )
                .await;
                Some(index_until(&s, "idle").await.active.unwrap().id)
            } else {
                document(
                    &s,
                    "Source for rebuild",
                    "A rebuilt native index retains its ordinary content capability.",
                )
                .await;
                None
            };
            let operation = if content {
                "AdminSemanticSearchKeep"
            } else {
                "AdminSemanticSearchRebuild"
            };
            let key = administrative_key(&s, operation).await;
            let receipt = if let Some(generation_id) = original {
                s.rig
                    .world
                    .app
                    .call(AdminSemanticSearchKeep, &[&org])
                    .token(&key)
                    .body(&SemanticSearchKeepBody { generation_id })
                    .send_bytes()
                    .await
            } else {
                s.rig
                    .world
                    .app
                    .call(AdminSemanticSearchRebuild, &[&org])
                    .token(&key)
                    .send_bytes()
                    .await
            };
            assert_eq!(
                receipt.status,
                200,
                "{}",
                String::from_utf8_lossy(&receipt.raw)
            );
            let receipt: Json = serde_json::from_slice(&receipt.raw).unwrap();
            assert_eq!(receipt["projection"], "receipt");
            if content {
                document(
                    &s,
                    "New source",
                    "This new model result needs a fresh publication frame.",
                )
                .await;
            }
            let before: i64 = sqlx::query_scalar("SELECT count(*) FROM search_vectors v JOIN search_generations g ON g.id=v.generation_id WHERE g.org_id=$1")
                .bind(org).fetch_one(s.rig.db.pool()).await.unwrap();
            let issued_before = seen.lock().unwrap().len();
            s.rig.machine.handler.hold(CommandKind::ProviderEmbed);
            let engine = s.rig.engine.clone();
            let indexing = tokio::spawn(async move { engine.keep_indexing().await });
            let call = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                loop {
                    if indexing.is_finished() {
                        let state = s.rig.engine.semantic_state(org).await.unwrap();
                        panic!("the native embedding request must be reached; content={content} expire={expire} {state:?}");
                    }
                    if let Some(call) = sqlx::query_scalar::<_, Id>("SELECT id FROM summary_calls WHERE node_id=$1 AND purpose='embeddings' ORDER BY created_at,id LIMIT 1")
                        .bind(org).fetch_optional(s.rig.db.pool()).await.unwrap() { break call; }
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                s.rig.machine.handler.started(call),
            )
            .await
            .unwrap();
            let authority = s
                .rig
                .commands
                .issued_authority(call)
                .expect("the real emitted command retains its exact administrative origin");
            assert!(matches!(
                authority,
                orch_engine::commands::CommandAuthority::SemanticEmbedding(_)
            ));
            let envelope = serde_json::to_value(&authority).unwrap();
            assert_eq!(
                envelope["authority"]["work"]["request"]["origin"]["operation"],
                operation
            );
            if expire {
                s.rig.world.advance(chrono::Duration::seconds(11));
            }
            s.rig.machine.handler.release();
            tokio::time::timeout(std::time::Duration::from_secs(30), indexing)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(
                seen.lock().unwrap().len() > issued_before,
                "the scripted native endpoint really answered"
            );
            let charged: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM usage_events WHERE id=$1 AND purpose='embeddings'",
            )
            .bind(call)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
            assert_eq!(
                charged, 1,
                "accepted model usage is still reconciled exactly once"
            );
            let after: i64 = sqlx::query_scalar("SELECT count(*) FROM search_vectors v JOIN search_generations g ON g.id=v.generation_id WHERE g.org_id=$1")
                .bind(org).fetch_one(s.rig.db.pool()).await.unwrap();
            let generations = rows::generations(&mut s.rig.db.acquire().await.unwrap(), org)
                .await
                .unwrap();
            if expire {
                assert_eq!(
                    after, before,
                    "an expired request cannot publish the old model result"
                );
                if content {
                    assert_eq!(generations[0].id, original.unwrap());
                } else {
                    assert!(
                        generations.is_empty(),
                        "the probe cannot create a generation after expiry"
                    );
                }
            } else {
                assert!(
                    after > before,
                    "a live request retains the native generation/vector capability"
                );
                assert_eq!(
                    json!(generations[0].administration_id),
                    receipt["request_id"]
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn administrative_embedding_queue_insert_rechecks_after_its_actual_native_wait() {
    for expire in [false, true] {
        let s = Scene::without_supervisor().await;
        script(&s, &[]);
        let org = s.tree.org.base.id;
        let provider = embeddings_provider(&s, &["embed-a"]).await;
        set(&s, "search.semantic.enabled", json!(true)).await;
        set(&s, "search.semantic.max_disk_share", json!(null)).await;
        name(&s, provider, "embed-a", 4).await;
        let availability = s.rig.engine.semantic_available().await.unwrap();
        assert!(
            availability.is_ok(),
            "vector prerequisite: {availability:?}; expire={expire}"
        );
        let key = administrative_key(&s, "AdminSemanticSearchRebuild").await;
        let receipt = s
            .rig
            .world
            .app
            .call(AdminSemanticSearchRebuild, &[&org])
            .token(&key)
            .send_bytes()
            .await;
        assert_eq!(
            receipt.status,
            200,
            "{}",
            String::from_utf8_lossy(&receipt.raw)
        );
        // Stop at the actual prepared envelope before the scripted journal or native send.
        let prepared = s.rig.commands.stop_next_issue(CommandKind::ProviderEmbed);
        let engine = s.rig.engine.clone();
        let indexing = tokio::spawn(async move { engine.keep_indexing().await });
        let frame = tokio::time::timeout(std::time::Duration::from_secs(30), prepared)
            .await
            .expect("the administrative embedding must reach its native issuer")
            .unwrap();
        let call = frame.id;
        assert!(matches!(
            frame.authority.as_ref(),
            Some(orch_engine::commands::CommandAuthority::SemanticEmbedding(
                _
            ))
        ));
        let mut blocker = s.rig.db.begin().await.unwrap();
        sqlx::query("SELECT org_id FROM search_indexes WHERE org_id=$1 FOR UPDATE")
            .bind(org)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let dispatcher = s.rig.world.app.app.link.dispatcher();
        let command = orch_api_core::dispatch::StoredCommand {
            id: Some(call),
            machine: frame.machine,
            kind: frame.kind.as_str().to_owned(),
            key: frame.key.wire(),
            deadline: frame.deadline,
            body: frame.body,
            authority: frame.authority,
        };
        let issue = tokio::spawn(async move { dispatcher.issue_stored(command).await });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!issue.is_finished(), "new queue insertion reaches the actual native source wait");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE 'SELECT org_id FROM search_indexes%FOR UPDATE%')")
                    .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE id=$1")
            .bind(call)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(before, 0, "this is the first durable command insertion");
        if expire {
            s.rig.world.advance(chrono::Duration::seconds(11));
        }
        blocker.commit().await.unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), issue)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.is_ok(), !expire);
        let saved: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE id=$1")
            .bind(call)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(
            saved,
            i64::from(!expire),
            "a closed native source writes no queue row"
        );
        indexing.abort();
        assert!(indexing.await.unwrap_err().is_cancelled());
    }
}
