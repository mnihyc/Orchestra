//! Isolated native-corpus measurements of related lists for people and agents, with current access bounds checked.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_contract::search::{DocumentRelatedList, RelatedItems, RelatedMeaning};
use orch_api_core::{registry::Registry, testing::TestApp};
use orch_db::{
    Db,
    queries::access::{PageKey, SYSTEM_ID, testing::add_entry},
};
use orch_engine::tools::CallContext;
use orch_types::{Clock as _, Id, Json, new_id};
use serde_json::json;
use std::{path::PathBuf, time::Instant};

/// Match the search corpus method: setup reads, three warmups and thirty retained observations.
const WARMUPS: usize = 3;
const OBSERVATIONS: usize = 30;

fn isolated_url() -> String {
    let url =
        std::env::var("ORCH_RELATED_SCALE_URL").expect("name the isolated native search corpus");
    let parsed = url::Url::parse(&url).unwrap();
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!((41000..=41999).contains(&parsed.port().unwrap()));
    assert_ne!(parsed.port(), Some(41532));
    assert!(parsed.path().starts_with("/orchprobe_search_"));
    url
}

async fn identity(db: &Db, label: &str) -> Id {
    sqlx::query_scalar("SELECT search_probe_id($1)")
        .bind(label)
        .fetch_one(db.pool())
        .await
        .unwrap()
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

fn percentile(values: &mut [f64], percent: usize) -> f64 {
    values.sort_by(f64::total_cmp);
    // Nearest rank, identical to the public search runner; every retained sample remains in its report.
    values[(values.len() * percent).div_ceil(100).saturating_sub(1)]
}

fn save(value: &Json) {
    if let Some(path) = std::env::var_os("ORCH_RELATED_SCALE_REPORT") {
        std::fs::write(
            PathBuf::from(path),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    }
    println!("{value}");
}

#[tokio::test]
#[ignore = "extends an explicitly supplied isolated native search corpus"]
async fn prepare_related_vectors_and_reference_neighborhoods() {
    let db = Db::connect(&isolated_url(), 4).await.unwrap();
    let generation = identity(&db, "generation").await;
    let documents: i64 = sqlx::query_scalar("SELECT count(*) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(documents >= 1000 && documents % 1000 == 0);
    let mut conn = db.acquire().await.unwrap();
    sqlx::raw_sql("SET statement_timeout=0;SET maintenance_work_mem='1GB';SET max_parallel_maintenance_workers=0;").execute(&mut *conn).await.unwrap();
    let started = Instant::now();
    // Fill the set before constructing its graph, as the native chunk corpus does; per-row graph inserts would
    // measure fixture construction instead of the product lookup. This database is explicitly disposable.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP INDEX IF EXISTS search_item_vectors_{}",
        generation.simple()
    )))
    .execute(&mut *conn)
    .await
    .unwrap();
    // The native corpus already has checked distinct vectors for every chunk. Use exactly the product centroid
    // expression, set-wise, without calling a model or replaying a million row-at-a-time indexing operations.
    sqlx::query("INSERT INTO search_item_vectors (generation_id,kind,source_id,marker,embedding)
      SELECT s.generation_id,s.kind,s.source_id,s.marker,l2_normalize(avg(l2_normalize(v.embedding::vector)))::halfvec
      FROM search_sources s JOIN search_chunks c USING(generation_id,kind,source_id)
      JOIN search_vectors v ON v.generation_id=c.generation_id AND v.chunk_id=c.id AND v.text_hash=c.text_hash
      WHERE s.generation_id=$1 GROUP BY s.generation_id,s.kind,s.source_id,s.marker,s.chunks
      HAVING count(*)=s.chunks ON CONFLICT(generation_id,kind,source_id) DO NOTHING")
        .bind(generation).execute(&mut *conn).await.unwrap();
    // One in ten documents references a shared target, deliberately making a popular reverse neighborhood.
    // Updating real live blocks exercises the same transactional edge projection as ordinary writes.
    let target = identity(&db, "document/201").await;
    let batch: i64 = 10_000;
    for first in (1..=documents).step_by(usize::try_from(batch).unwrap()) {
        sqlx::query("UPDATE document_blocks b SET content=b.content||jsonb_build_array(jsonb_build_object('type','ref','kind','document','id',$3::uuid))
           FROM generate_series($1::bigint,$2::bigint) number
           WHERE number%10=1 AND b.id=search_probe_id('part/'||number)
             AND NOT EXISTS(SELECT 1 FROM related_references r WHERE r.part_id=b.id AND r.target_id=$3)")
            .bind(first).bind((first+batch-1).min(documents)).bind(target).execute(&mut *conn).await.unwrap();
    }
    orch_db::queries::semantic::make_index(&mut conn, generation, 256)
        .await
        .unwrap();
    sqlx::raw_sql("ANALYZE search_item_vectors;ANALYZE related_references;")
        .execute(&mut *conn)
        .await
        .unwrap();
    let vectors: i64 = sqlx::query_scalar("SELECT count(*) FROM search_item_vectors")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let references: i64 = sqlx::query_scalar("SELECT count(*) FROM related_references")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    save(
        &json!({"prepared":true,"profile":profile(),"documents":documents,"item_vectors":vectors,"references":references,"seconds":started.elapsed().as_secs_f64()}),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated disk-backed million-document search corpus"]
async fn a_million_document_corpus_reports_related_latency() {
    let db = Db::connect(&isolated_url(), 16).await.unwrap();
    let documents: i64 = sqlx::query_scalar("SELECT count(*) FROM documents")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(
        documents >= 1_000_000,
        "a setup smoke run is not a million-document measurement"
    );
    let corpus: Json = sqlx::query_scalar("SELECT jsonb_build_object(
        'documents',(SELECT count(*) FROM documents),'blocks',(SELECT count(*) FROM document_blocks),
        'messages',(SELECT count(*) FROM chat_messages),'items',(SELECT count(*) FROM items),
        'chunks',(SELECT count(*) FROM search_chunks),'chunk_vectors',(SELECT count(*) FROM search_vectors),
        'item_vectors',(SELECT count(*) FROM search_item_vectors),'reference_edges',(SELECT count(*) FROM related_references),
        'database_bytes',pg_database_size(current_database()),
        'item_vectors_bytes',pg_total_relation_size('search_item_vectors'),
        'reference_bytes',pg_total_relation_size('related_references'),
        'server_version',current_setting('server_version'),'shared_buffers',current_setting('shared_buffers'),
        'work_mem',current_setting('work_mem'))")
        .fetch_one(db.pool()).await.unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::new(db.clone(), orch_testkit::clock::manual(), registry);
    let now = app.clock.now();
    let root = identity(&db, "root").await;
    let source = identity(&db, "document/1").await;
    let denied = identity(&db, "document/3").await;
    let machine = identity(&db, "machine").await;
    let mut conn = db.acquire().await.unwrap();
    let defaults: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM permission_entries WHERE object_kind='system')",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    for shipped in orch_access_contract::pages::shipped()
        .into_iter()
        .filter(|_| !defaults)
    {
        add_entry(
            &mut conn,
            &PageKey::new("system", SYSTEM_ID, "defaults"),
            shipped.profile.as_str(),
            shipped.permission.as_str(),
            ("role", None, Some(shipped.role.as_str())),
            None,
            now,
        )
        .await
        .unwrap();
    }
    for (key, value) in [
        ("search.semantic.enabled", json!(true)),
        ("orch_tools.calls_per_minute", json!(100000)),
    ] {
        orch_settings::write::write(
            &mut conn,
            &orch_settings::write::Write {
                actor: orch_db::audit::Actor::System,
                target: orch_settings::Target::Node(root),
                key,
                value,
                locked: None,
                expected_revision: None,
                may_lock: false,
                origin: &Default::default(),
            },
            now,
        )
        .await
        .unwrap();
    }
    drop(conn);
    let run = new_id();
    let mut cases = Vec::new();
    for reader in ["all", "small"] {
        let user = identity(&db, reader).await;
        let thread = identity(&db, &format!("reader/{reader}")).await;
        let turn = identity(&db, &format!("reader-turn/{reader}")).await;
        let session = identity(&db, &format!("related-session/{reader}")).await;
        let token = format!("oat_related_probe_{reader}_{run}");
        let mut conn = db.acquire().await.unwrap();
        sqlx::query("DELETE FROM sessions WHERE id=$1")
            .bind(session)
            .execute(&mut *conn)
            .await
            .unwrap();
        orch_db::queries::identity::testing::insert_session(
            &mut conn,
            session,
            user,
            &orch_api_core::middleware::authentication::token_hash(&token),
            now + chrono::Duration::days(1),
        )
        .await
        .unwrap();
        drop(conn);
        for (document, allowed) in [(source, true), (denied, reader == "all")] {
            let answer = app
                .call(DocumentRelatedList, &[&document])
                .token(&token)
                .send_bytes()
                .await;
            assert_eq!(answer.status.as_u16(), if allowed { 200 } else { 404 });
            let mut conn = db.acquire().await.unwrap();
            let entry = orch_engine::document_tree::entry_in(
                &mut conn,
                orch_engine::document_tree::Actor::Agent { thread },
                orch_engine::document_tree::Item::Document(document),
                now.into(),
            )
            .await;
            assert_eq!(
                entry.is_ok(),
                allowed,
                "verify agent reach independently of human roles"
            );
        }
        for meaning in [true, false] {
            let mut conn = db.acquire().await.unwrap();
            orch_settings::write::write(
                &mut conn,
                &orch_settings::write::Write {
                    actor: orch_db::audit::Actor::System,
                    target: orch_settings::Target::Node(root),
                    key: "search.semantic.enabled",
                    value: json!(meaning),
                    locked: None,
                    expected_revision: None,
                    may_lock: false,
                    origin: &Default::default(),
                },
                now,
            )
            .await
            .unwrap();
            drop(conn);
            let mut samples = Vec::new();
            for sample in 0..WARMUPS + OBSERVATIONS {
                let load = std::fs::read_to_string("/proc/loadavg").unwrap();
                let started = Instant::now();
                let response = app
                    .call(DocumentRelatedList, &[&source])
                    .token(&token)
                    .send_bytes()
                    .await;
                let person_ms = started.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(
                    response.status,
                    200,
                    "{}",
                    String::from_utf8_lossy(&response.raw)
                );
                let result: RelatedItems = serde_json::from_slice(&response.raw).unwrap();
                assert!(!result.items.is_empty());
                assert_eq!(
                    result.meaning,
                    if meaning {
                        RelatedMeaning::Ready
                    } else {
                        RelatedMeaning::Off
                    }
                );
                if reader == "small" {
                    let mut conn = db.acquire().await.unwrap();
                    let expected = identity(&db, "topic/0").await;
                    let ids = result.items.iter().map(|item| item.id).collect::<Vec<_>>();
                    assert!(
                        orch_db::queries::related::items(&mut conn, &ids)
                            .await
                            .unwrap()
                            .iter()
                            .all(|item| item.path.contains(&expected))
                    );
                }
                let call_ref = format!("related-scale-{run}-{reader}-{meaning}-{sample}");
                let started = Instant::now();
                app.app
                    .engine
                    .answer_tool_call(&CallContext {
                        machine,
                        thread,
                        turn,
                        call_ref: call_ref.clone(),
                        attempt: None,
                        tool: orch_agent_contract::events::OrchTool::SearchShared,
                        arguments: json!({"related_to":format!("id:{source}")}),
                    })
                    .await
                    .unwrap();
                let agent_ms = started.elapsed().as_secs_f64() * 1000.0;
                let agent: Json = sqlx::query_scalar("SELECT body FROM commands WHERE id=$1")
                    .bind(orch_engine::ids::tool_result(thread, &call_ref))
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
                assert!(agent["error"].is_null(), "{agent}");
                assert_eq!(
                    agent["result"]["meaning"]["state"],
                    if meaning { "ready" } else { "off" },
                    "an unavailable agent lookup is not a successful meaning measurement: {agent}"
                );
                assert!(
                    agent["result"]["hits"]
                        .as_array()
                        .is_some_and(|hits| !hits.is_empty()),
                    "{agent}"
                );
                if reader == "small" {
                    assert!(
                        agent["result"]["hits"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|hit| hit["path"]
                                .as_str()
                                .is_some_and(|path| path.contains("/Scope 0/")))
                    );
                }
                if sample >= WARMUPS {
                    samples.push(json!({"person_ms":person_ms,"agent_ms":agent_ms,"load":load.trim(),"items":result.items.len(),"meaning":result.meaning,"agent_meaning":agent["result"]["meaning"]["state"]}));
                }
            }
            let mut people = samples
                .iter()
                .map(|sample| sample["person_ms"].as_f64().unwrap())
                .collect::<Vec<_>>();
            let mut agents = samples
                .iter()
                .map(|sample| sample["agent_ms"].as_f64().unwrap())
                .collect::<Vec<_>>();
            cases.push(json!({"reader":reader,"meaning":meaning,"observations":OBSERVATIONS,
                "person_p50_ms":percentile(&mut people,50),"person_p95_ms":percentile(&mut people,95),
                "agent_p50_ms":percentile(&mut agents,50),"agent_p95_ms":percentile(&mut agents,95),"samples":samples}));
            save(
                &json!({"profile":profile(),"corpus":corpus,"warmups":WARMUPS,"initial_cache":"unspecified; setup/access reads precede warmups","cases":cases}),
            );
        }
    }
}
