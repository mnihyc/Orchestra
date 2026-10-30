//! Best matches joined with results by meaning: a text in another language found by what it means, a result both orders rank first, nothing the reader may not read, and plain results by words where the org does not search by meaning.
//!
//! The vectors need the database's `vector` extension; where the test database has none, the search answers by
//! words alone, which is checked, and the rest of a case is left out.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::documents::{NewDocumentAccess, NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::search::{
    SearchFound, SearchHit, SearchList, SearchListQuery, SearchSort, SearchStreamEvent,
    SearchStreamList, SearchStreamListQuery,
};
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_link_contract::commands::{
    Command, CommandKind, CommandResult, ProviderEmbedResult, encode_vector,
};
use orch_types::{Id, Json, new_id};
use rig::Scene;
use serde_json::json;

/// What the scripted model understands: texts about undoing a release mean one thing in any language, texts
/// about lunch another, and every other text something of its own.
fn meaning_of(text: &str) -> Vec<f32> {
    let lower = text.to_lowercase();
    if ["undo", "revert", "回滚"]
        .iter()
        .any(|word| lower.contains(word))
    {
        return vec![1.0, 0.0, 0.0, 0.0];
    }
    if ["lunch", "午饭"].iter().any(|word| lower.contains(word)) {
        return vec![0.0, 1.0, 0.0, 0.0];
    }
    let hash = orch_types::Hash::of(lower.as_bytes());
    hash.as_bytes()[..4]
        .iter()
        .map(|byte| f32::from(*byte) / 255.0 - 0.5)
        .collect()
}

fn script(s: &Scene) {
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderEmbed, |command| {
            let Command::ProviderEmbed(request) = command else {
                panic!("only a batch of texts reaches this script");
            };
            Ok(CommandResult::ProviderEmbed(ProviderEmbedResult {
                timing: Default::default(),
                vectors: request
                    .inputs
                    .iter()
                    .map(|text| encode_vector(&meaning_of(text)))
                    .collect(),
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 3,
                    total_tokens: 3,
                    ..Default::default()
                },
                request_ref: None,
            }))
        });
}

async fn set(s: &Scene, key: &str, value: Json) {
    set_at(s, s.tree.org.base.id, key, value).await;
}

async fn set_at(s: &Scene, org: Id, key: &str, value: Json) {
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&org, &key])
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

async fn turn_on(s: &Scene) {
    turn_on_at(s, s.tree.org.base.id).await;
}

async fn turn_on_at(s: &Scene, org: Id) {
    let provider = new_id();
    sqlx::query("INSERT INTO providers (id,name,owner_id,org_id,home_id,kind,key_machine_id,machine_id,upstreams,protocols,arrangements,status) VALUES ($1,'Meaning',$2,$3,$3,'api',$4,$4,'[{\"protocol\":\"openai-embeddings\",\"base_url\":\"https://embeddings.invalid/v1\",\"options\":{\"auth\":\"none\"}}]',ARRAY['openai-embeddings'],ARRAY['key_separated'],'ready')")
        .bind(provider).bind(s.ada.id()).bind(org).bind(s.rig.machine_id).execute(s.rig.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO provider_models (id,provider_id,model_name,display_name,enabled,source,created_by) VALUES ($1,$2,'meaning-1','Meaning',true,'manual',$3)")
        .bind(new_id()).bind(provider).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .lets_use("provider", provider, "access.own", &s.ada, json!({}))
        .await;
    set_at(s, org, "search.semantic.enabled", json!(true)).await;
    set_at(s, org, "search.semantic.max_disk_share", json!(null)).await;
    set_at(
        s,
        org,
        "search.semantic.embeddings",
        json!({"provider_id": provider, "model_name": "meaning-1"}),
    )
    .await;
}

async fn document(s: &Scene, title: &str, markdown: &str, private: bool) -> Id {
    document_at(s, s.tree.topic.base.id, title, markdown, private).await
}

async fn document_at(s: &Scene, node: Id, title: &str, markdown: &str, private: bool) -> Id {
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&node])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: title.into(),
            blocks: None,
            markdown: Some(markdown.into()),
            access: private.then_some(NewDocumentAccess::Private),
        })
        .send()
        .await
        .ok()
        .base
        .id
}

async fn streamed(
    s: &Scene,
    token: &str,
    query: Json,
) -> orch_api_contract::common::Page<SearchHit> {
    let mut selection: SearchStreamListQuery = serde_json::from_value(query).unwrap();
    selection.immediate = Some(true);
    let response = s
        .rig
        .world
        .app
        .call(SearchStreamList, &[])
        .token(token)
        .query(&selection)
        .send_bytes()
        .await;
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.raw)
    );
    let mut hits = std::collections::HashMap::new();
    let mut order = Vec::new();
    let mut next_cursor = None;
    for line in response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        match serde_json::from_slice::<SearchStreamEvent>(line).unwrap() {
            SearchStreamEvent::Hit { id, hit, .. } => {
                assert!(hits.insert(id, *hit).is_none(), "one payload per part");
            }
            SearchStreamEvent::Order { items, .. } => {
                order = items.into_iter().map(|item| item.id).collect()
            }
            SearchStreamEvent::Cursor { cursor } => next_cursor = cursor,
            _ => {}
        }
    }
    orch_api_contract::common::Page {
        items: order
            .into_iter()
            .map(|id| hits.remove(&id).unwrap())
            .collect(),
        next_cursor,
    }
}

async fn search_in(s: &Scene, token: &str, query: Json) -> Vec<SearchHit> {
    streamed(s, token, query).await.items
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_selected_small_document_keeps_exact_retrieval_when_the_reader_owns_larger_sources() {
    let s = Scene::new().await;
    let selected = document(&s, "Small cookie guide", "Cookie recipe", false).await;
    let paragraphs = (0..20)
        .map(|index| format!("Cookie notes {index}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let larger = document(&s, "Large cookie guide", &paragraphs, false).await;
    sqlx::raw_sql(
        "ANALYZE documents; ANALYZE document_blocks; ANALYZE items; ANALYZE chat_messages",
    )
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let mut conn = s.rig.db.pool().acquire().await.unwrap();
    let scope =
        orch_db::queries::search::sparse::Scope::focused(&mut conn, Some(selected), None, 10, None)
            .await
            .unwrap()
            .unwrap();
    assert!(
        scope.small,
        "only the selected source determines the exact-scan allowance"
    );
    assert_eq!(scope.documents, vec![selected]);
    assert!(scope.document_nodes.is_empty() && scope.threads.is_empty() && scope.chats.is_empty());
    let generation = new_id();
    sqlx::query(
        "INSERT INTO search_generations(id,org_id,provider_id,model_name,chunking,state,created_at)
      VALUES($1,$2,$3,'focused-test','test','building',now())",
    )
    .bind(generation)
    .bind(s.tree.org.base.id)
    .bind(s.on.provider)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("INSERT INTO search_sources(generation_id,kind,source_id,title,org_id,marker,changed_at,chunks,tokens,bytes)
      VALUES($1,'document',$2,'Selected',$4,1,now(),1,0,0),
            ($1,'document',$3,'Larger',$4,1,now(),1000000,0,0)")
        .bind(generation).bind(selected).bind(larger).bind(s.tree.org.base.id).execute(&mut *conn).await.unwrap();
    let selected_scope = orch_db::queries::search::sparse::Scope::focused(
        &mut conn,
        Some(selected),
        None,
        10,
        Some(generation),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        selected_scope.small,
        "a kept one-chunk source uses exact vectors despite larger sources beside it"
    );
    let large_scope = orch_db::queries::search::sparse::Scope::focused(
        &mut conn,
        Some(larger),
        None,
        10,
        Some(generation),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        !large_scope.small,
        "a source over the allowance still uses the graph"
    );
}

async fn search(s: &Scene, token: &str, q: &str) -> Vec<SearchHit> {
    search_in(
        s,
        token,
        json!({"q": q, "sort": SearchSort::Relevance, "limit": 20}),
    )
    .await
}

fn documents_of(hits: &[SearchHit]) -> Vec<Id> {
    hits.iter()
        .filter_map(|hit| match &hit.found {
            SearchFound::Document { document, .. } => Some(document.id),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sparse_visibility_keeps_empty_windows_bounded_and_continues_to_the_same_authorized_order()
{
    let s = Scene::new().await;
    for index in 0..6 {
        document(
            &s,
            &format!("Cookie public {index}"),
            "Visible recipe",
            false,
        )
        .await;
        s.rig.world.advance(chrono::Duration::seconds(1));
        document(
            &s,
            &format!("Cookie secret {index}"),
            "Confidential recipe",
            true,
        )
        .await;
        s.rig.world.advance(chrono::Duration::seconds(1));
    }
    s.rig.world.set("search.word_candidates", json!(1)).await;
    s.rig
        .world
        .set(
            "search.word_tuning",
            json!({"scope_candidates":1,"request_windows":100,"tiers":{}}),
        )
        .await;
    let query = json!({"q":"cookie","scope":s.tree.org.base.id,"sort":"relevance","kinds":"document","title_only":true,"limit":20});
    let reference = streamed(&s, &s.cai.token, query.clone()).await;
    assert_eq!(reference.items.len(), 6);
    s.rig
        .world
        .set(
            "search.word_tuning",
            json!({"scope_candidates":1,"request_windows":1,"tiers":{}}),
        )
        .await;
    let mut query = query;
    let mut found = Vec::new();
    let mut finished = false;
    // Twelve titles, four content tiers and their exhaustion probes fit in sixty-four windows.
    for _ in 0..64 {
        let page = streamed(&s, &s.cai.token, query.clone()).await;
        assert!(page.items.len() <= 1, "one title candidate per request");
        assert!(
            !serde_json::to_string(&page.items)
                .unwrap()
                .contains("secret")
        );
        found.extend(page.items);
        if let Some(cursor) = page.next_cursor {
            query["cursor"] = json!(cursor);
        } else {
            finished = true;
            break;
        }
    }
    assert!(finished, "the continuation exhausts instead of looping");
    assert_eq!(
        found, reference.items,
        "bounded responses retain every authorized hit, payload and order"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_word_retrieval_preserves_public_order_and_never_returns_private_text() {
    let s = Scene::new().await;
    let visible = document(
        &s,
        "Cookie catalog",
        "# Cookie heading\n\nCookie cookie policy 回滚",
        false,
    )
    .await;
    let hidden = document(&s, "Cookie secret", "Cookie confidential 回滚", true).await;
    for query in ["cookie", "回滚", "回", "\"cookie policy\""] {
        s.rig
            .world
            .set(
                "search.word_tuning",
                json!({"scope_candidates":1,"tiers":{}}),
            )
            .await;
        let reference = search(&s, &s.cai.token, query).await;
        s.rig
            .world
            .set(
                "search.word_tuning",
                json!({"scope_candidates":30000,"tiers":{}}),
            )
            .await;
        let scoped = search(&s, &s.cai.token, query).await;
        assert!(!scoped.is_empty(), "the public source matches {query}");
        assert_eq!(
            scoped, reference,
            "the same reader keeps exact hits, snippets and order for {query}"
        );
        let documents = documents_of(&scoped);
        assert!(documents.contains(&visible));
        assert!(!documents.contains(&hidden));
        assert!(
            !serde_json::to_string(&scoped)
                .unwrap()
                .contains("confidential")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_word_cursor_keeps_all_unseen_words_when_semantic_search_is_switched_off_between_pages() {
    let s = Scene::new().await;
    script(&s);
    for title in ["Undo A", "Undo B", "Undo C"] {
        document(
            &s,
            title,
            "We undo a deployment by restoring the previous release.",
            false,
        )
        .await;
    }
    turn_on(&s).await;
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        return;
    }
    for _ in 0..40 {
        s.rig.engine.keep_indexing().await.unwrap();
        if s.rig
            .engine
            .semantic_state(s.tree.org.base.id)
            .await
            .unwrap()
            .state
            == "idle"
        {
            break;
        }
    }
    let query: SearchListQuery =
        serde_json::from_value(json!({"q":"undo","sort":"relevance","kinds":"document","limit":1}))
            .unwrap();
    let first = s
        .rig
        .world
        .app
        .call(SearchList, &[])
        .token(&s.ada.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(first.items.len(), 1);
    let mut cursor = first.next_cursor;
    assert!(cursor.is_some());
    set(&s, "search.semantic.enabled", json!(false)).await;
    let words = search_in(
        &s,
        &s.ada.token,
        json!({"q":"undo","sort":"relevance","kinds":"document","limit":20}),
    )
    .await;
    let requests = s.rig.commands.issued_of("provider.embed");
    let mut paged = first.items;
    for _ in 0..=words.len() {
        let Some(next) = cursor.take() else {
            break;
        };
        let query: SearchListQuery = serde_json::from_value(
            json!({"q":"undo","sort":"relevance","kinds":"document","limit":1,"cursor":next}),
        )
        .unwrap();
        let page = s
            .rig
            .world
            .app
            .call(SearchList, &[])
            .token(&s.ada.token)
            .query(&query)
            .send()
            .await
            .ok();
        paged.extend(page.items);
        cursor = page.next_cursor;
        if paged.len() == 2 {
            set(&s, "search.semantic.enabled", json!(true)).await;
        }
    }
    assert!(cursor.is_none(), "paging the small fixture ends");
    let parts = |hits: &[SearchHit]| {
        hits.iter()
            .map(|hit| match &hit.found {
                SearchFound::Document {
                    document, block_id, ..
                } => (document.id, *block_id),
                other => panic!("only documents were requested: {other:?}"),
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(words.len(), 6, "each title and its block match the words");
    assert_eq!(
        paged.len(),
        words.len(),
        "a fallback neither repeats nor drops lexical hits"
    );
    assert_eq!(parts(&paged), parts(&words));
    assert_eq!(
        s.rig.commands.issued_of("provider.embed"),
        requests,
        "a cursor that fell back to words does not restart paid semantic queries midway"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_stream_cursor_continues_both_sources_and_never_restarts_meaning_after_falling_back() {
    let s = Scene::new().await;
    script(&s);
    for title in ["Undo A", "Undo B", "Undo C"] {
        document(&s, title, "We undo the deployment.", false).await;
    }
    turn_on(&s).await;
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        return;
    }
    s.rig.world.set("search.page_size", json!(1)).await;
    s.rig.world.set("search.word_candidates", json!(1)).await;
    set(&s,"search.semantic.tuning",json!({"chunk_tokens":400,"batch_tokens":4096,"candidates":1,"oversample":4,"fusion_k":60,"same_model_cosine_milli":990,"interval_secs":10,"query_wait_secs":3})).await;
    for _ in 0..40 {
        s.rig.engine.keep_indexing().await.unwrap();
        if s.rig
            .engine
            .semantic_state(s.tree.org.base.id)
            .await
            .unwrap()
            .state
            == "idle"
        {
            break;
        }
    }
    let select = json!({"q":"undo","sort":"relevance","kinds":"document"});
    let first = streamed(&s, &s.ada.token, select.clone()).await;
    assert_eq!(
        first.items.len(),
        2,
        "one words header and one distinct semantic block arrive independently"
    );
    let mut cursor = first.next_cursor;
    assert!(cursor.is_some());
    set(&s, "search.semantic.enabled", json!(false)).await;
    let requests = s.rig.commands.issued_of("provider.embed");
    let mut paged = first.items;
    for page in 0..8 {
        let Some(next) = cursor.take() else {
            break;
        };
        let mut query = select.clone();
        query["cursor"] = json!(next);
        let answer = streamed(&s, &s.ada.token, query).await;
        paged.extend(answer.items);
        cursor = answer.next_cursor;
        if page == 0 {
            set(&s, "search.semantic.enabled", json!(true)).await;
        }
    }
    assert!(
        cursor.is_none(),
        "the small fixture completes both source positions"
    );
    let parts: std::collections::BTreeSet<_> = paged
        .iter()
        .map(|hit| match &hit.found {
            SearchFound::Document {
                document, block_id, ..
            } => (document.id, *block_id),
            _ => panic!("only documents"),
        })
        .collect();
    assert_eq!(parts.len(), 6, "every header and block survives fallback");
    assert_eq!(
        paged.len(),
        6,
        "each represented part arrives once across continued segments"
    );
    assert_eq!(
        s.rig.commands.issued_of("provider.embed"),
        requests,
        "falling back ends this cursor's semantic source"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn best_matches_join_what_is_found_by_meaning_in_any_language_and_only_what_the_reader_may_read()
 {
    let s = Scene::new().await;
    script(&s);
    let rollout = document(
        &s,
        "Rollout plan",
        "Build first, then ship; to undo, revert the tag.",
        false,
    )
    .await;
    let chinese = document(&s, "回滚流程", "如何回滚一次发布：先停止流量。", false).await;
    let lunch = document(&s, "Cafeteria", "Lunch menu for Friday.", false).await;
    let hidden = document(&s, "Private notes", "We revert on Fridays.", true).await;
    // Off: words alone.
    let words = documents_of(&search(&s, &s.ada.token, "undo").await);
    assert_eq!(words, vec![rollout]);
    turn_on(&s).await;
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        let still = documents_of(&search(&s, &s.ada.token, "undo").await);
        assert_eq!(still, vec![rollout], "without vectors, words alone");
        eprintln!("this test database has no vector extension; the rest of this case needs one");
        return;
    }
    let org = s.tree.org.base.id;
    for _ in 0..40 {
        s.rig.engine.keep_indexing().await.unwrap();
        if s.rig.engine.semantic_state(org).await.unwrap().state == "idle" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let joined_hits = search(&s, &s.ada.token, "undo").await;
    let joined = documents_of(&joined_hits);
    assert_eq!(
        joined.first(),
        Some(&rollout),
        "first in both orders: {joined:?}"
    );
    assert!(
        joined.contains(&chinese),
        "found by meaning in another language: {joined:?}"
    );
    let place = |id: &Id| joined.iter().position(|found| found == id);
    assert!(
        place(&lunch).is_none_or(|lunch| place(&chinese).is_some_and(|chinese| chinese < lunch)),
        "what means something else comes after: {joined:?}"
    );
    assert!(
        joined.contains(&hidden),
        "the owner reads her private notes"
    );
    let generation = s
        .rig
        .engine
        .semantic_state(org)
        .await
        .unwrap()
        .active
        .unwrap();
    let expected:Vec<(String,Id,Option<Id>)>=sqlx::query_as("SELECT c.kind,c.source_id,c.first_part
      FROM search_vectors v JOIN search_chunks c ON c.id=v.chunk_id
      WHERE v.generation_id=$1
      ORDER BY (subvector(v.embedding,1,4)::halfvec(4) <=> '[1,0,0,0]'::halfvec(4))+0,c.id LIMIT 50")
        .bind(generation.id).fetch_all(s.rig.db.pool()).await.unwrap();
    let exact = s
        .rig
        .engine
        .meaning_filtered(
            org,
            "undo",
            0,
            None,
            Some(json!({"readable":{"scopes":[org]}})),
        )
        .await
        .unwrap();
    let orch_engine::semantic::query::Meaning::Found { near, .. } = exact else {
        panic!("the small owner scope is available")
    };
    assert_eq!(
        near.into_iter()
            .map(|part| (part.kind, part.source_id, part.first_part))
            .collect::<Vec<_>>(),
        expected,
        "a small scope returns the exact half-vector order"
    );
    // Paging the fused order must return exactly the same parts, each once, even when words found none of them.
    let mut paged = Vec::new();
    let mut cursor = None;
    for _ in 0..=joined_hits.len() {
        let query: SearchListQuery = serde_json::from_value(json!({
            "q":"undo", "sort":"relevance", "limit":1, "cursor":cursor,
        }))
        .unwrap();
        let page = streamed(&s, &s.ada.token, serde_json::to_value(query).unwrap()).await;
        paged.extend(page.items);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(cursor.is_none(), "the small fixture's fused order must end");
    assert_eq!(
        paged, joined_hits,
        "a cursor neither repeats nor loses a fused hit"
    );
    let future = s.rig.world.now() + chrono::Duration::days(1);
    let dated = search_in(
        &s,
        &s.ada.token,
        json!({"q":"undo","sort":"relevance","from":future}),
    )
    .await;
    assert!(
        dated.is_empty(),
        "date filters apply to semantic hits as well as word matches"
    );
    let later = s.rig.world.now() + chrono::Duration::hours(1);
    sqlx::query(
        "UPDATE documents SET owner_id=$2,created_at=$3,updated_at=$4,updated_by=$5 WHERE id=$1",
    )
    .bind(chinese)
    .bind(s.cai.id())
    .bind(s.rig.world.now())
    .bind(later)
    .bind(s.ada.id())
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let advanced = json!({"q":"undo","sort":"relevance","kinds":"document","limit":1,
        "owner_id":s.cai.id(),"edited_by_id":s.ada.id(),"date_field":"edited","from":later,"to":later});
    let filtered = documents_of(&search_in(&s, &s.ada.token, advanced.clone()).await);
    assert_eq!(
        filtered,
        vec![chinese],
        "the meaning-only document survives source filters before the candidate limit"
    );
    let mut created = advanced;
    created["date_field"] = json!("created");
    assert!(search_in(&s, &s.ada.token, created).await.is_empty());
    let reader = s.cai.token.clone();
    let theirs = documents_of(&search(&s, &reader, "undo").await);
    assert!(theirs.contains(&chinese));
    assert!(
        !theirs.contains(&hidden),
        "nothing found by meaning that the reader may not read"
    );
    // Newest first stays words alone.
    let newest = search_in(&s, &s.ada.token, json!({"q": "undo", "limit": 20})).await;
    assert_eq!(documents_of(&newest), vec![rollout]);
    set(
        &s,
        "search.semantic.tuning",
        json!({
            "chunk_tokens":400,"batch_tokens":4096,"candidates":50,"oversample":4,"fusion_k":60,
            "same_model_cosine_milli":990,"interval_secs":10,"query_wait_secs":1
        }),
    )
    .await;
    let mut rebuilding = s.rig.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE search_vectors IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *rebuilding)
        .await
        .unwrap();
    let during_rebuild = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        search(&s, &s.ada.token, "undo"),
    )
    .await
    .expect("a locked optional index must not block lexical search");
    assert_eq!(
        documents_of(&during_rebuild),
        vec![rollout],
        "words still answer while a semantic index is rebuilt"
    );
    rebuilding.rollback().await.unwrap();
    sqlx::query("UPDATE nodes SET deleted_at = $1 WHERE id = $2")
        .bind(s.rig.world.now())
        .bind(org)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        !s.rig.engine.semantic_orgs().await.unwrap().contains(&org),
        "a deleted org cannot answer a query or spend on its embeddings"
    );
    assert!(
        search(&s, &s.ada.token, "undo").await.is_empty(),
        "a deleted org's old index cannot break lexical search or reveal its content"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn words_arrive_while_the_query_vector_waits_and_dropping_the_stream_cancels_its_provider_command()
 {
    use http_body_util::BodyExt as _;
    use orch_engine::commands::{Commands as _, Outcome};
    use std::time::Duration;
    let s = Scene::new().await;
    script(&s);
    document(
        &s,
        "Undo release",
        "Undo the deployment with the previous version.",
        false,
    )
    .await;
    turn_on(&s).await;
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        return;
    }
    for _ in 0..40 {
        s.rig.engine.keep_indexing().await.unwrap();
        if s.rig
            .engine
            .semantic_state(s.tree.org.base.id)
            .await
            .unwrap()
            .state
            == "idle"
        {
            break;
        }
    }
    s.rig.machine.handler.hold(CommandKind::ProviderEmbed);
    let query: SearchStreamListQuery =
        serde_json::from_value(json!({"q":"undo","sort":"relevance","immediate":true})).unwrap();
    let response = s
        .rig
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&s.ada.token)
        .query(&query)
        .send_response()
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/x-ndjson");
    let mut body = response.into_body();
    let (first, words) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        let mut first = Some(first);
        while let Some(frame) = body.frame().await {
            let frame = frame.unwrap();
            let Some(data) = frame.data_ref() else {
                continue;
            };
            for line in data
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
            {
                if let SearchStreamEvent::Order { items, .. } =
                    serde_json::from_slice::<SearchStreamEvent>(line).unwrap()
                    && !items.is_empty()
                    && let Some(first) = first.take()
                {
                    let _ = first.send(items);
                }
            }
        }
    });
    let arrived = tokio::time::timeout(Duration::from_secs(1), words)
        .await
        .expect("words must not wait for an embedding")
        .unwrap();
    assert!(!arrived.is_empty());
    let call = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let call: Option<Id> = sqlx::query_scalar("SELECT id FROM summary_calls WHERE purpose='embeddings' ORDER BY created_at DESC LIMIT 1").fetch_optional(s.rig.db.pool()).await.unwrap();
            if let Some(call) = call { break call; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the other retrieval starts concurrently");
    tokio::time::timeout(Duration::from_secs(1), s.rig.machine.handler.started(call))
        .await
        .unwrap();
    reader.abort();
    let _ = reader.await;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if s.rig
                .commands
                .status(call)
                .await
                .unwrap()
                .is_some_and(|info| matches!(info.outcome, Some(Outcome::Cancelled)))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("closing the response cancels the actual provider command");
    s.rig.machine.handler.release();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_queries_reuse_the_vector_only_within_its_org_and_generation() {
    let s = Scene::new().await;
    script(&s);
    document(&s, "Undo release", "Undo the deployment.", false).await;
    turn_on(&s).await;
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        return;
    }
    for _ in 0..40 {
        s.rig.engine.keep_indexing().await.unwrap();
        if s.rig
            .engine
            .semantic_state(s.tree.org.base.id)
            .await
            .unwrap()
            .state
            == "idle"
        {
            break;
        }
    }
    let before = s.rig.commands.issued_of("provider.embed");
    let first = search(&s, &s.ada.token, "undo").await;
    assert!(!first.is_empty());
    assert_eq!(s.rig.commands.issued_of("provider.embed"), before + 1);
    assert_eq!(search(&s, &s.ada.token, "undo").await, first);
    assert_eq!(
        s.rig.commands.issued_of("provider.embed"),
        before + 1,
        "a repeated search does not spend another query embedding"
    );
    assert!(!search(&s, &s.ada.token, "revert").await.is_empty());
    assert_eq!(
        s.rig.commands.issued_of("provider.embed"),
        before + 2,
        "another query has its own vector"
    );
    let generation: Id = sqlx::query_scalar(
        "SELECT generation_id FROM search_query_vectors WHERE org_id=$1 LIMIT 1",
    )
    .bind(s.tree.org.base.id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM search_query_vectors WHERE org_id=$1 AND generation_id=$2",
    )
    .bind(s.tree.org.base.id)
    .bind(generation)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(stored, 2);
    let outside = places::person(&s.rig.world, "outside").await;
    let paid = s.rig.commands.issued_of("provider.embed");
    assert!(
        search(&s, &outside.token, "unseen provider probe")
            .await
            .is_empty()
    );
    assert_eq!(
        s.rig.commands.issued_of("provider.embed"),
        paid,
        "a root the reader cannot read never spends its embeddings"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_a_search_cancels_its_postgres_read_before_a_blocking_lock_is_released() {
    use http_body_util::BodyExt as _;
    use std::time::Duration;
    let s = Scene::new().await;
    document(&s, "Undo release", "Undo the deployment.", false).await;
    let mut blocked = s.rig.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE document_blocks IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocked)
        .await
        .unwrap();
    let query: SearchStreamListQuery =
        serde_json::from_value(json!({"q":"undo","sort":"relevance","immediate":true})).unwrap();
    let response = s
        .rig
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&s.ada.token)
        .query(&query)
        .send_response()
        .await;
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();
    let reader = tokio::spawn(async move {
        while let Some(frame) = body.frame().await {
            frame.unwrap();
        }
    });
    let pid: i32 = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let pid = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND application_name LIKE 'orch-search:%' AND wait_event_type='Lock' LIMIT 1")
                .fetch_optional(s.rig.db.pool()).await.unwrap();
            if let Some(pid) = pid {break pid;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the search reaches the deliberately blocked table");
    reader.abort();
    let _ = reader.await;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND state='active' AND wait_event_type='Lock')")
                .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
            if !waiting {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("giving up the response cancels the database query, not merely its Rust waiter");
    blocked.rollback().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expression_uses_the_shared_matcher_and_highlights_words_without_spending_on_meaning() {
    let s = Scene::new().await;
    script(&s);
    let doc = document(&s, "Undo release", "We undo the deployment.", false).await;
    turn_on(&s).await;
    if s.rig.engine.semantic_available().await.unwrap().is_ok() {
        for _ in 0..40 {
            s.rig.engine.keep_indexing().await.unwrap();
            if s.rig
                .engine
                .semantic_state(s.tree.org.base.id)
                .await
                .unwrap()
                .state
                == "idle"
            {
                break;
            }
        }
    }
    let before = s.rig.commands.issued_of("provider.embed");
    let query: SearchStreamListQuery = serde_json::from_value(
        json!({"q":"u.do","regex":true,"sort":"relevance","immediate":true}),
    )
    .unwrap();
    let legacy = s
        .rig
        .world
        .app
        .call(SearchList, &[])
        .token(&s.ada.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(
        legacy.status, 400,
        "the legacy word reader must not silently interpret an expression as literal text"
    );
    let problem: orch_api_contract::common::Problem = serde_json::from_slice(&legacy.raw).unwrap();
    assert_eq!(
        problem.code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
    assert!(
        problem.extra["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fault| fault["path"] == "regex")
    );
    let response = s
        .rig
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&s.ada.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.raw)
    );
    let events: Vec<SearchStreamEvent> = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert!(events.iter().any(|event|matches!(event,SearchStreamEvent::Source{source,state,reason:Some(reason),..} if source=="meaning" && state=="done" && reason=="regex")));
    assert!(events.iter().any(|event|matches!(event,SearchStreamEvent::Hit{hit,..} if matches!(&hit.found,SearchFound::Document{document,..} if document.id==doc) && !hit.matches.is_empty())));
    assert_eq!(
        s.rig.commands.issued_of("provider.embed"),
        before,
        "expressions never spend embeddings"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_org_continues_its_own_semantic_window_when_candidate_sizes_differ() {
    let s = Scene::new().await;
    script(&s);
    let other = places::make_org(&s.rig.world, &s.ada, "Second", "second").await;
    let project = places::make_project(&s.rig.world, &s.ada, other.base.id, "Web").await;
    let topic = places::make_topic(&s.rig.world, &s.ada, project.base.id, "Review").await;
    let mut expected = std::collections::HashSet::new();
    for node in [s.tree.topic.base.id, topic.base.id] {
        for number in 0..3 {
            expected.insert(
                document_at(
                    &s,
                    node,
                    &format!("Checklist {number}"),
                    "回滚流程：先停止流量。",
                    false,
                )
                .await,
            );
        }
    }
    for (org, candidates) in [(s.tree.org.base.id, 3), (other.base.id, 1)] {
        turn_on_at(&s, org).await;
        set_at(&s, org, "search.semantic.tuning", json!({
            "chunk_tokens":400,"batch_tokens":4096,"candidates":candidates,"oversample":4,"fusion_k":60,
            "same_model_cosine_milli":990,"interval_secs":10,"query_wait_secs":3
        })).await;
    }
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        return;
    }
    for _ in 0..40 {
        s.rig.engine.keep_indexing().await.unwrap();
        if s.rig
            .engine
            .semantic_state(s.tree.org.base.id)
            .await
            .unwrap()
            .state
            == "idle"
            && s.rig
                .engine
                .semantic_state(other.base.id)
                .await
                .unwrap()
                .state
                == "idle"
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let before = s.rig.commands.issued_of("provider.embed");
    let mut cursor = None;
    let mut received = Vec::new();
    for _ in 0..10 {
        let page = streamed(
            &s,
            &s.ada.token,
            json!({"q":"undo","sort":"relevance","immediate":true,"cursor":cursor}),
        )
        .await;
        received.extend(documents_of(&page.items));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(
        cursor.is_none(),
        "both small sources finish their own windows"
    );
    assert_eq!(
        received.len(),
        expected.len(),
        "each represented document arrives once"
    );
    assert_eq!(
        received
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
        expected,
        "the org with the smaller candidate setting skips no part of its next window"
    );
    assert_eq!(
        s.rig.commands.issued_of("provider.embed") - before,
        2,
        "each org makes one query vector and reuses it through all of its continuations"
    );
    let first = streamed(&s, &s.ada.token, json!({"q":"undo","sort":"relevance"})).await;
    let old_cursor = first.next_cursor.expect("both sources have a next window");
    let active = s
        .rig
        .engine
        .semantic_state(other.base.id)
        .await
        .unwrap()
        .active
        .unwrap();
    set_at(
        &s,
        other.base.id,
        "search.semantic.embeddings",
        json!({
            "provider_id":active.provider_id,"model_name":active.model_name,"dimensions":2
        }),
    )
    .await;
    for _ in 0..40 {
        s.rig.engine.keep_indexing().await.unwrap();
        if s.rig
            .engine
            .semantic_state(other.base.id)
            .await
            .unwrap()
            .active
            .is_some_and(|generation| generation.dimensions == Some(2))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let shortened = s
        .rig
        .engine
        .semantic_state(other.base.id)
        .await
        .unwrap()
        .active
        .unwrap();
    assert_eq!(
        shortened.id, active.id,
        "shortening keeps the generation identity"
    );
    assert_eq!(shortened.dimensions, Some(2));
    assert_eq!(
        s.rig
            .engine
            .semantic_state(s.tree.org.base.id)
            .await
            .unwrap()
            .active
            .unwrap()
            .dimensions,
        Some(4)
    );
    assert!(
        matches!(s.rig.engine.meaning(s.tree.org.base.id,"undo").await.unwrap(),
        orch_engine::semantic::query::Meaning::Found {near,..} if near.len()==3),
        "another org keeps answering in its original embedding space"
    );
    let before = s.rig.commands.issued_of("provider.embed");
    let selection: SearchStreamListQuery = serde_json::from_value(json!({
        "q":"undo","sort":"relevance","immediate":true,"cursor":old_cursor
    }))
    .unwrap();
    let response = s
        .rig
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&s.ada.token)
        .query(&selection)
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let events: Vec<SearchStreamEvent> = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert!(
        events.iter().any(
            |event| matches!(event,SearchStreamEvent::Source {source,state,reason:Some(reason),..}
        if source=="meaning" && state=="unavailable" && reason=="model_changed")
        ),
        "a continuation cannot change embedding lengths midway through its nearest order"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SearchStreamEvent::Hit { .. }))
    );
    assert_eq!(s.rig.commands.issued_of("provider.embed"), before);
    let repeated = s
        .rig
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&s.ada.token)
        .query(&selection)
        .send_bytes()
        .await;
    assert_eq!(
        repeated.status, 400,
        "the final continuation is consumed too"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_gets_one_merged_answer_and_keeps_words_when_meaning_times_out() {
    use orch_api_contract::threads::Delivery;
    use orch_db::queries::access::{PageKey, testing::add_entry};
    use orch_engine::{commands::Commands as _, tools::CallContext};
    let s = Scene::new().await;
    script(&s);
    let rollout = document(
        &s,
        "Rollout plan",
        "To undo a release, revert the tag.",
        false,
    )
    .await;
    let chinese = document(&s, "回滚流程", "如何回滚一次发布：先停止流量。", false).await;
    let hidden = document(&s, "Private notes", "We revert on Fridays.", true).await;
    turn_on(&s).await;
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        return;
    }
    for _ in 0..40 {
        s.rig.engine.keep_indexing().await.unwrap();
        if s.rig
            .engine
            .semantic_state(s.tree.org.base.id)
            .await
            .unwrap()
            .state
            == "idle"
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let mut descriptor = rig::descriptor();
    descriptor["capabilities"]["orch_tools"] = json!("mcp");
    descriptor["capabilities"]["mcp"] =
        json!({"stdio":true,"http":true,"reload":false,"strict":true});
    let descriptor: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(descriptor).unwrap();
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(s.on.agent)
        .bind(json!(descriptor))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    s.rig.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(
            orch_link_contract::commands::AgentStartResult {
                reused: false,
                harness_version: "1.0.0".into(),
                descriptor: descriptor.clone(),
            },
        ))
    });
    let thread = s.thread().await.base.id;
    for document in [rollout, chinese] {
        let mut conn = s.rig.db.acquire().await.unwrap();
        add_entry(
            &mut conn,
            &PageKey::new("document", document, "live"),
            "any",
            "doc.view",
            ("agents", Some(thread), None),
            None,
            s.rig.world.now(),
        )
        .await
        .unwrap();
    }
    s.say(
        &s.ada,
        thread,
        "Review the rollout.",
        Delivery::Queue,
        "search-merged-answer",
    )
    .await
    .ok();
    let started = s.rig.turn_start(thread, 0).await;
    s.rig
        .answer(thread, &started, "search-baseline", "Reviewing the rollout")
        .await;
    s.rig.wait_turn(started.turn_id, "running").await;
    let turn = started.turn_id;
    let ask = |cursor: Option<String>, limit| {
        let s = &s;
        async move {
            let call_ref = new_id().to_string();
            s.rig.engine.answer_tool_call(&CallContext {
                machine:s.rig.machine_id,thread,turn,call_ref:call_ref.clone(),attempt:None,
                tool:orch_agent_contract::events::OrchTool::SearchShared,
                arguments:json!({"query":"undo","path":"/","kinds":["documents"],"max_results":limit,"cursor":cursor}),
            }).await.unwrap();
            let answer = s
                .rig
                .commands
                .retained_body(orch_engine::ids::tool_result(thread, &call_ref))
                .await
                .unwrap()
                .unwrap();
            assert!(
                answer["error"].is_null(),
                "the admitted tool answers: {answer}"
            );
            answer["result"].clone()
        }
    };
    let first = ask(None, 1).await;
    assert_eq!(first["meaning"]["state"], "done");
    assert_eq!(first["hits"][0]["id"], json!(rollout));
    assert_eq!(first["hits"][0]["found_by"], json!(["words", "meaning"]));
    let mut hits = first["hits"].as_array().unwrap().clone();
    let mut cursor = first["next_cursor"].as_str().map(str::to_owned);
    for _ in 0..10 {
        let Some(next) = cursor.take() else { break };
        let page = ask(Some(next), 1).await;
        hits.extend(page["hits"].as_array().unwrap().iter().cloned());
        cursor = page["next_cursor"].as_str().map(str::to_owned);
    }
    assert!(
        cursor.is_none(),
        "a small merged search finishes its continuations"
    );
    let by_meaning = hits.iter().find(|hit| hit["id"] == json!(chinese)).unwrap();
    assert_eq!(by_meaning["found_by"], json!(["meaning"]));
    assert_eq!(by_meaning["match_count"], 0);
    assert!(
        !hits.iter().any(|hit| hit["id"] == json!(hidden)),
        "private unshared content never enters the answer"
    );
    assert_eq!(
        hits.iter()
            .filter(|hit| hit["id"] == json!(rollout))
            .count(),
        1
    );
    set(
        &s,
        "search.semantic.tuning",
        json!({
            "chunk_tokens":400,"batch_tokens":4096,"candidates":50,"oversample":4,"fusion_k":60,
            "same_model_cosine_milli":990,"interval_secs":10,"query_wait_secs":1
        }),
    )
    .await;
    let mut locked = s.rig.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE search_vectors IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *locked)
        .await
        .unwrap();
    let fallback = tokio::time::timeout(std::time::Duration::from_secs(5), ask(None, 20))
        .await
        .unwrap();
    assert_eq!(
        fallback["meaning"],
        json!({"state":"unavailable","reason":"query_timeout"})
    );
    assert!(
        fallback["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hit| hit["id"] == json!(rollout))
    );
    locked.rollback().await.unwrap();
}

/// Coordinates an opt-in cold sample before its request clock starts.
async fn reset_scale_database(
    control: Option<&std::path::Path>,
    reader: &str,
    query: &str,
    sample: usize,
    phase: &str,
) {
    let Some(control) = control else { return };
    let stem = format!("{reader}-{query}-{sample}-{phase}");
    std::fs::write(control.join(format!("{stem}.request")), b"reset\n").unwrap();
    let ready = control.join(format!("{stem}.ready"));
    // This coordinates an owned PostgreSQL restart outside the measured request; product deadlines stay fixed.
    const CONTROL_WAIT: std::time::Duration = std::time::Duration::from_secs(90);
    const CONTROL_POLL: std::time::Duration = std::time::Duration::from_millis(25);
    tokio::time::timeout(CONTROL_WAIT, async {
        loop {
            if let Ok(bytes) = std::fs::read(&ready)
                && let Ok(answer) = serde_json::from_slice::<Json>(&bytes)
            {
                assert_eq!(answer["ok"], true, "cold coordinator failed: {answer}");
                break;
            }
            tokio::time::sleep(CONTROL_POLL).await;
        }
    })
    .await
    .expect("the owned database must restart before a sample is sent");
}

/// Labels actual statement plans without logging search text or changing the measured request.
async fn scale_plan_marker(
    pool: &sqlx::PgPool,
    enabled: bool,
    reader: &str,
    query: &str,
    sample: usize,
    phase: &str,
) {
    if enabled {
        // Reader/query labels and phases are the fixture's own fixed words; the query itself is never inserted.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DO $probe$ BEGIN RAISE LOG 'search_scale {reader} {query} {sample} {phase}'; END $probe$;"
        )))
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Builds a reproducible corpus only in an explicitly named empty, isolated database.
#[tokio::test]
#[ignore = "creates an isolated disk-backed search corpus"]
async fn prepare_an_isolated_search_measurement_corpus() {
    use std::path::PathBuf;
    let url = std::env::var("ORCH_SEARCH_SCALE_URL").expect("name an empty isolated database");
    let parsed = url::Url::parse(&url).unwrap();
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!((41000..=41999).contains(&parsed.port().unwrap()));
    assert_ne!(parsed.port(), Some(41532));
    assert!(
        parsed.path().starts_with("/orchprobe_search_"),
        "corpus creation requires an explicitly named scratch database"
    );
    // A thousand documents divides evenly over one hundred scopes and both languages, including messages.
    let documents: i64 = std::env::var("ORCH_SEARCH_SCALE_DOCUMENTS")
        .unwrap_or_else(|_| "1000000".to_owned())
        .parse()
        .unwrap();
    assert!(documents >= 1000 && documents % 1000 == 0);
    let db = orch_db::Db::connect(&url, 4).await.unwrap();
    let populated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','f'))")
        .fetch_one(db.pool()).await.unwrap();
    assert!(
        !populated,
        "never migrate or replace an existing database for a benchmark"
    );
    orch_db::migrations::apply(
        &db,
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../migrations"),
    )
    .await
    .unwrap();
    let mut conn = db.acquire().await.unwrap();
    // The measured graph needs about 1.6 GiB; two GiB avoids a disk build within the four-GiB fixture.
    sqlx::raw_sql("SET statement_timeout=0;SET maintenance_work_mem='2GB';SET max_parallel_maintenance_workers=0;")
        .execute(&mut *conn).await.unwrap();
    sqlx::query("SELECT set_config('orch_probe.documents',$1,false)")
        .bind(documents.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    let (before, rest) = include_str!("../../../../scripts/search-scale/corpus.sql")
        .split_once("-- @documents\n")
        .unwrap();
    let (document_sql, after) = rest.split_once("-- @content\n").unwrap();
    sqlx::raw_sql(before).execute(&mut *conn).await.unwrap();
    // Separate commits bound the row-trigger event memory of the million-document fixture.
    const DOCUMENT_BATCH: i64 = 10_000;
    for first in (1..=documents).step_by(usize::try_from(DOCUMENT_BATCH).unwrap()) {
        sqlx::query(document_sql)
            .bind(first)
            .bind((first + DOCUMENT_BATCH - 1).min(documents))
            .execute(&mut *conn)
            .await
            .unwrap();
        eprintln!(
            "Prepared {} documents",
            (first + DOCUMENT_BATCH - 1).min(documents)
        );
    }
    for (phase, sql) in after.split("-- @phase\n").enumerate() {
        sqlx::raw_sql(sql).execute(&mut *conn).await.unwrap();
        eprintln!("Prepared content phase {phase}");
    }
    let generation: Id = sqlx::query_scalar("SELECT search_probe_id('generation')")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    orch_db::queries::semantic::make_index(&mut conn, generation, 256)
        .await
        .unwrap();
    sqlx::query("ANALYZE search_vectors")
        .execute(&mut *conn)
        .await
        .unwrap();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let report = json!([{"prepared":true,"profile":profile,"documents":documents,
        "chat_messages":documents/5,"thread_items":documents/5,"vectors":documents*7/5,
        "storage":"native_tables_and_generation_hnsw"}]);
    if let Some(path) = std::env::var_os("ORCH_SEARCH_SCALE_REPORT") {
        std::fs::write(
            PathBuf::from(path),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
    println!("{report}");
}

/// Runs only against an explicitly supplied, isolated corpus; ordinary suites never load a large fixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated disk-backed search corpus"]
async fn a_million_document_corpus_reports_words_fused_and_agent_latency() {
    use http_body_util::BodyExt as _;
    use orch_api_core::{registry::Registry, testing::TestApp};
    use orch_db::{
        Db,
        audit::Actor,
        queries::access::{PageKey, SYSTEM_ID, testing::add_entry},
    };
    use orch_engine::tools::CallContext;
    use orch_settings::{
        Place, Target, get,
        write::{Write, write},
    };
    use orch_settings_contract::registry::keys;
    use orch_types::Clock as _;
    use std::{path::PathBuf, time::Instant};
    if std::env::var_os("ORCH_SEARCH_SCALE_TRACE").is_some() {
        tracing_subscriber::fmt()
            .with_env_filter("orch::search::timing=debug")
            .with_ansi(false)
            .without_time()
            .try_init()
            .expect("the isolated benchmark owns its timing subscriber");
    }
    let url =
        std::env::var("ORCH_SEARCH_SCALE_URL").expect("a private scale database must be named");
    let parsed = url::Url::parse(&url).unwrap();
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!((41000..=41999).contains(&parsed.port().unwrap()));
    assert_ne!(
        parsed.port(),
        Some(41532),
        "large fixtures never use the shared test servers"
    );
    // Use the service's connection policy, including its disabled JIT and request limits.
    let db = Db::connect(&url, 16).await.unwrap();
    let pool = db.pool().clone();
    let id = |label: &str| {
        let label = label.to_owned();
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Id>("SELECT search_probe_id($1)")
                .bind(label)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let root = id("root").await;
    let owner = id("owner").await;
    let generation = id("generation").await;
    let provider = id("provider").await;
    let machine = id("machine").await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::new(db.clone(), orch_testkit::clock::manual(), registry);
    let now = app.clock.now();
    let mut conn = db.acquire().await.unwrap();
    let system = PageKey::new("system", SYSTEM_ID, "defaults");
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
            &system,
            shipped.profile.as_str(),
            shipped.permission.as_str(),
            ("role", None, Some(shipped.role.as_str())),
            None,
            now,
        )
        .await
        .unwrap();
    }
    for (target, key, value) in [
        (Target::Node(root), "search.semantic.enabled", json!(true)),
        (
            Target::Node(root),
            "search.semantic.embeddings",
            json!({"provider_id":provider,"model_name":"probe-256","dimensions":256}),
        ),
        (
            Target::Node(root),
            "orch_tools.calls_per_minute",
            json!(100000),
        ),
    ] {
        write(
            &mut conn,
            &Write {
                actor: Actor::System,
                target,
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
    let page_size = get::<i64>(&mut conn, Place::Node(root), keys::SEARCH_PAGE_SIZE)
        .await
        .unwrap()
        .unwrap();
    let word_candidates = get::<i64>(&mut conn, Place::Node(root), keys::SEARCH_WORD_CANDIDATES)
        .await
        .unwrap()
        .unwrap();
    let word_tuning = get::<Json>(&mut conn, Place::Node(root), keys::SEARCH_WORD_TUNING)
        .await
        .unwrap()
        .unwrap();
    // Two title, two description, two heading/file and four body streams form one native window.
    let word_window: i64 = [
        ("title", 2),
        ("description", 2),
        ("heading", 2),
        ("body", 4),
    ]
    .into_iter()
    .map(|(tier, streams)| {
        streams
            * word_tuning["tiers"][tier]
                .as_i64()
                .unwrap_or(word_candidates)
    })
    .sum();
    // A final word window can overshoot the page target; it cannot retain a second full window.
    let word_bound = usize::try_from(page_size.saturating_add(word_window)).unwrap();
    let semantic_tuning = get::<Json>(&mut conn, Place::Node(root), keys::SEARCH_SEMANTIC_TUNING)
        .await
        .unwrap()
        .unwrap();
    let active_orgs: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT org_id) FROM search_generations WHERE state='active'",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let meaning_bound = usize::try_from(
        semantic_tuning["candidates"]
            .as_i64()
            .unwrap()
            .saturating_mul(active_orgs),
    )
    .unwrap();
    let hit_bound = word_bound.saturating_add(meaning_bound);
    let fingerprint = orch_db::queries::semantic::endpoint_fingerprint(&mut conn, provider)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE search_generations SET provider_fingerprint=$2,writer_id=$3 WHERE id=$1")
        .bind(generation)
        .bind(fingerprint)
        .bind(owner)
        .execute(&mut *conn)
        .await
        .unwrap();
    let terms = [
        ("english-common", "release"),
        ("english-rare", "zephyrfax"),
        ("chinese-two", "回滚"),
        ("chinese-one", "回"),
        ("phrase", "\"previous release\""),
    ];
    // Exact selectors let a failed large-corpus case be diagnosed without repeating completed cases.
    let selected_reader = std::env::var("ORCH_SEARCH_SCALE_READER").ok();
    let selected_query = std::env::var("ORCH_SEARCH_SCALE_QUERY").ok();
    // The fixture spans about twelve days. A seven-day middle window exercises a useful nonempty date cut.
    let owner_edited = std::env::var_os("ORCH_SEARCH_SCALE_OWNER_EDITED").is_some();
    assert!(
        selected_reader
            .as_deref()
            .is_none_or(|reader| ["all", "small"].contains(&reader)),
        "the scale reader is all or small"
    );
    assert!(
        selected_query
            .as_deref()
            .is_none_or(|query| terms.iter().any(|(label, _)| *label == query)),
        "the scale query names one of the measured cases"
    );
    for (_, term) in terms {
        let hash = orch_types::Hash::of(term.as_bytes());
        let prototype = i64::from(hash.as_bytes()[0]) * 32 + i64::from(hash.as_bytes()[1] % 32);
        let vector: Vec<f32> = sqlx::query_scalar(
            "SELECT embedding::vector::real[] FROM corpus_vector_prototypes WHERE i=$1",
        )
        .bind(prototype)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO search_query_vectors(org_id,generation_id,query_hash,embedding,expires_at) VALUES($1,$2,$3,$4,$5)
           ON CONFLICT(org_id,generation_id,query_hash) DO UPDATE SET embedding=EXCLUDED.embedding,expires_at=EXCLUDED.expires_at")
           .bind(root).bind(generation).bind(orch_types::Hash::of(term.as_bytes()).to_string()).bind(&vector).bind(now+chrono::Duration::days(1)).execute(&mut *conn).await.unwrap();
    }
    drop(conn);
    // Setup reads and three warmups are disclosed; no sample is described as a cold-machine claim.
    let corpus: Json = sqlx::query_scalar("SELECT jsonb_build_object(
      'documents',(SELECT count(*) FROM documents),
      'document_blocks',(SELECT count(*) FROM document_blocks),
      'chat_messages',(SELECT count(*) FROM chat_messages),
      'thread_items',(SELECT count(*) FROM items),
      'vectors',(SELECT count(*) FROM search_vectors),
      'vector_relation_kind',(SELECT relkind::text FROM pg_class WHERE oid='search_vectors'::regclass),
      'logical_document_bytes',(SELECT sum(bytes)::bigint FROM documents),
      'database_bytes',pg_database_size(current_database()),
      'relations',(SELECT jsonb_object_agg(c.relname,jsonb_build_object('heap_bytes',pg_relation_size(c.oid),
        'index_bytes',pg_indexes_size(c.oid),'total_bytes',pg_total_relation_size(c.oid))) FROM pg_class c
        WHERE c.relnamespace='public'::regnamespace AND c.relname IN
        ('documents','document_blocks','chat_messages','items','search_chunks','search_sources',
         'search_query_vectors','search_sessions','search_vectors','corpus_vectors_unique')),
      'shared_buffers',current_setting('shared_buffers'),
      'work_mem',current_setting('work_mem'),
      'server_version',current_setting('server_version'))")
        .fetch_one(&pool).await.unwrap();
    let load = || {
        std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|text| {
                let values: Vec<f64> = text
                    .split_whitespace()
                    .take(3)
                    .filter_map(|word| word.parse().ok())
                    .collect();
                (values.len() == 3).then_some(values)
            })
    };
    const WARMUPS: usize = 3;
    const OBSERVATIONS: usize = 30;
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let initial_cache =
        std::env::var("ORCH_SEARCH_SCALE_CACHE_STATE").unwrap_or_else(|_| "unspecified".to_owned());
    let cold_control = std::env::var_os("ORCH_SEARCH_SCALE_CONTROL_DIR").map(PathBuf::from);
    let plan_markers = std::env::var_os("ORCH_SEARCH_SCALE_PLAN_MARKERS").is_some();
    let run = new_id();
    let mut report = Vec::new();
    for reader in ["all", "small"] {
        if selected_reader
            .as_deref()
            .is_some_and(|selected| selected != reader)
        {
            continue;
        }
        let user = id(reader).await;
        let token = format!("oat_search_probe_{reader}");
        let mut conn = db.acquire().await.unwrap();
        let session = id(&format!("session/{reader}")).await;
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
        let thread = id(&format!("reader/{reader}")).await;
        for (number, allowed) in [(1, true), (3, reader == "all")] {
            let document = id(&format!("document/{number}")).await;
            let response = app
                .call(orch_api_contract::documents::DocumentGet, &[&document])
                .token(&token)
                .send_bytes()
                .await;
            if allowed {
                assert_eq!(
                    response.status, 200,
                    "the fixture grants its intended readable scope"
                );
            } else {
                assert!(
                    [403, 404].contains(&response.status.as_u16()),
                    "the one-percent reader must not read another scope"
                );
            }
            let mut conn = db.acquire().await.unwrap();
            let agent_read = orch_engine::document_tree::entry_in(
                &mut conn,
                orch_engine::document_tree::Actor::Agent { thread },
                orch_engine::document_tree::Item::Document(document),
                now.into(),
            )
            .await;
            if allowed {
                assert!(
                    agent_read.is_ok(),
                    "the fixture grants the agent its intended readable scope: {agent_read:?}"
                );
            } else {
                assert!(
                    matches!(agent_read, Err(orch_engine::document_tree::Error::NotFound)),
                    "the one-percent agent must not read another scope: {agent_read:?}"
                );
            }
        }
        let turn = id(&format!("reader-turn/{reader}")).await;
        for (label, term) in terms {
            if selected_query
                .as_deref()
                .is_some_and(|selected| selected != label)
            {
                continue;
            }
            let mut samples = Vec::new();
            // Availability is an observation here; the ordinary functional tests assert search semantics.
            for sample in 0..WARMUPS + OBSERVATIONS {
                reset_scale_database(cold_control.as_deref(), reader, label, sample, "api").await;
                scale_plan_marker(&pool, plan_markers, reader, label, sample, "api_begin").await;
                let mut selection = json!({"q":term,"sort":"relevance","immediate":true});
                if owner_edited {
                    selection["kinds"] = json!("document");
                    selection["owner_id"] = json!(owner);
                    selection["date_field"] = json!("edited");
                    selection["from"] = json!("2026-01-03T00:00:00.000Z");
                    selection["to"] = json!("2026-01-10T00:00:00.000Z");
                }
                let query: SearchStreamListQuery = serde_json::from_value(selection).unwrap();
                let load_started = load();
                let started = Instant::now();
                let response = app
                    .call(SearchStreamList, &[])
                    .token(&token)
                    .query(&query)
                    .send_response()
                    .await;
                let status = response.status().as_u16();
                let mut body = response.into_body();
                let mut words = None;
                let mut hits = 0;
                let mut payloads = std::collections::BTreeMap::new();
                let mut order = Vec::new();
                let mut cursor = false;
                let mut words_source = json!({"state":"not_returned","reason":null,"received":0});
                let mut meaning_source = words_source.clone();
                let mut body_error = None;
                while let Some(frame) = body.frame().await {
                    let frame = match frame {
                        Ok(frame) => frame,
                        Err(error) => {
                            body_error = Some(error.to_string());
                            break;
                        }
                    };
                    if let Some(data) = frame.data_ref() {
                        for line in data
                            .split(|byte| *byte == b'\n')
                            .filter(|line| !line.is_empty())
                        {
                            let event = match serde_json::from_slice::<SearchStreamEvent>(line) {
                                Ok(event) => event,
                                Err(error) => {
                                    body_error = Some(error.to_string());
                                    continue;
                                }
                            };
                            match event {
                                SearchStreamEvent::Hit { id, hit, .. } => {
                                    hits += 1;
                                    assert!(
                                        hits <= hit_bound,
                                        "one stream cannot retain more than its configured source windows"
                                    );
                                    payloads.insert(id, hit);
                                }
                                SearchStreamEvent::Order { items, .. } => {
                                    assert!(
                                        items.len() <= hit_bound,
                                        "an order contains only the bounded source windows"
                                    );
                                    order = items;
                                }
                                SearchStreamEvent::Source {
                                    source,
                                    state,
                                    reason,
                                    received,
                                } => {
                                    if source == "words" {
                                        assert!(
                                            received <= i64::try_from(word_bound).unwrap(),
                                            "words retain at most a page and its last native window"
                                        );
                                        if state == "done" {
                                            words = Some(started.elapsed().as_secs_f64() * 1000.);
                                        }
                                        words_source = json!({"state":state,"reason":reason,"received":received});
                                    } else if source == "meaning" {
                                        assert!(
                                            received <= i64::try_from(meaning_bound).unwrap(),
                                            "meaning retains only configured nearest candidates"
                                        );
                                        meaning_source = json!({"state":state,"reason":reason,"received":received});
                                    }
                                }
                                SearchStreamEvent::Cursor { .. } => cursor = true,
                                _ => {}
                            }
                        }
                    }
                }
                let segment = started.elapsed().as_secs_f64() * 1000.;
                scale_plan_marker(&pool, plan_markers, reader, label, sample, "api_end").await;
                let fused_available = status == 200
                    && body_error.is_none()
                    && cursor
                    && hits > 0
                    && words_source["state"] == "done"
                    && meaning_source["state"] == "done"
                    && (label == "phrase"
                        || meaning_source["received"]
                            .as_i64()
                            .is_some_and(|count| count > 0));
                let fused = fused_available.then_some(segment);
                reset_scale_database(cold_control.as_deref(), reader, label, sample, "agent").await;
                scale_plan_marker(&pool, plan_markers, reader, label, sample, "agent_begin").await;
                let started = Instant::now();
                let call_ref = format!("scale-{run}-{reader}-{label}-{sample}");
                let answer_error = app
                    .app
                    .engine
                    .answer_tool_call(&CallContext {
                        machine,
                        thread,
                        turn,
                        call_ref: call_ref.clone(),
                        attempt: None,
                        tool: orch_agent_contract::events::OrchTool::SearchShared,
                        arguments: json!({"query":term,"path":"/","max_results":20}),
                    })
                    .await
                    .err()
                    .map(|error| error.to_string());
                let agent_elapsed = started.elapsed().as_secs_f64() * 1000.;
                scale_plan_marker(&pool, plan_markers, reader, label, sample, "agent_end").await;
                // The canonical result has an indexed identity; never scan retained command payloads.
                let answer: Option<Json> =
                    sqlx::query_scalar("SELECT body FROM commands WHERE id=$1")
                        .bind(orch_engine::ids::tool_result(thread, &call_ref))
                        .fetch_optional(&pool)
                        .await
                        .unwrap();
                let agent_valid = answer_error.is_none()
                    && answer.as_ref().is_some_and(|answer| {
                        answer["error"].is_null()
                            && answer["result"]["hits"]
                                .as_array()
                                .is_some_and(|hits| !hits.is_empty())
                    });
                let agent_meaning = answer
                    .as_ref()
                    .map(|answer| answer["result"]["meaning"].clone());
                let agent_fused = agent_valid
                    && agent_meaning.as_ref().is_some_and(|meaning| {
                        meaning["state"] == if label == "phrase" { "none" } else { "done" }
                    });
                assert!(payloads.len() <= hit_bound);
                samples.push(json!({"sample":sample,"warmup":sample<WARMUPS,"words_ms":words,
                    "segment_ms":segment,"fused_ms":fused,"search_shared_ms":agent_valid.then_some(agent_elapsed),
                    "search_shared_fused_ms":agent_fused.then_some(agent_elapsed),"agent_elapsed_ms":agent_elapsed,
                    "response_status":status,"cursor":cursor,"hits":hits,"body_error":body_error,
                    "ordered_hits":order,"retained_payloads":payloads.len(),"payloads":payloads,
                    "words":words_source,"meaning":meaning_source,"agent_meaning":agent_meaning,
                    "agent_error":answer_error,"agent_result_error":answer.as_ref().map(|answer| &answer["error"]),
                    "agent_hits":answer.as_ref().and_then(|answer| answer["result"]["hits"].as_array().map(Vec::len)),
                    "agent_payloads":answer.as_ref().map(|answer| &answer["result"]["hits"]),
                    "load_started":load_started,"load_finished":load()}));
                eprintln!(
                    "{reader}/{label} sample {sample}: words={words:?}ms fused={fused:?}ms segment={segment}ms search_shared={agent_elapsed}ms"
                );
            }
            let statistics = |key: &str| {
                let mut values: Vec<_> = samples
                    .iter()
                    .filter(|sample| sample["warmup"] == false)
                    .filter_map(|sample| sample[key].as_f64())
                    .collect();
                values.sort_by(f64::total_cmp);
                let percentile = |fraction: f64| {
                    values
                        .get(((values.len() as f64 * fraction).ceil() as usize).saturating_sub(1))
                        .copied()
                };
                json!({"count":values.len(),"p50":percentile(0.5),"p95":percentile(0.95)})
            };
            let unavailable = samples
                .iter()
                .filter(|sample| {
                    sample["warmup"] == false && sample["meaning"]["state"] == "unavailable"
                })
                .count();
            let summaries: Vec<_> = [
                "words_ms",
                "fused_ms",
                "segment_ms",
                "search_shared_ms",
                "search_shared_fused_ms",
            ]
            .into_iter()
            .map(|key| (key, statistics(key)))
            .collect();
            let mut row = json!({"reader":reader,"query":label,"profile":profile,"corpus":corpus,
                "selection":if owner_edited {"owner_and_edited_dates"} else {"all_kinds"},
                "bounds_checked":true,"word_hit_bound":word_bound,"meaning_hit_bound":meaning_bound,"retained_hit_bound":hit_bound,
                "initial_cache_state":initial_cache,"sampling_state":"after_setup_reads_and_three_warmups",
                "fixture_readers_verified":["people","agents"],
                "query_vectors":"precomputed_for_org_and_generation","warmups":WARMUPS,"observations":OBSERVATIONS,
                "postgres_restart_between_samples":cold_control.is_some(),"plan_markers":plan_markers,
                "meaning_unavailable":unavailable,"samples":samples});
            for (key, summary) in summaries {
                row[key] = summary;
            }
            println!("{row}");
            report.push(row);
            if let Some(path) = std::env::var_os("ORCH_SEARCH_SCALE_REPORT") {
                std::fs::write(
                    PathBuf::from(path),
                    serde_json::to_vec_pretty(&report).unwrap(),
                )
                .unwrap();
            }
        }
    }
}
