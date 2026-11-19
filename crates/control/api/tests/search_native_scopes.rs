//! Search keys select native identities before the existing ranked result windows.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod desk;
mod places;
mod rig;
mod search_world;
mod support;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::search::{DocumentRelatedList, NameSearchList, NameSearchListQuery};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;
async fn key_with_scope(world: &World, user: Id, value: Json) -> String {
    let scope: KeyScope = serde_json::from_value(value).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Usage native scope",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}

fn selected(kind: &str, id: Id, operation: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn value(reply: orch_api_core::testing::RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
#[tokio::test]
async fn name_search_filters_native_candidates_before_its_limit_and_binds_context_names_independently()
 {
    let world = World::new().await;
    let owner = places::person(&world, "searcher").await;
    let org = places::make_org(&world, &owner, "Search names", "search-names")
        .await
        .base
        .id;
    for i in 0..25 {
        places::make_project(&world, &owner, org, &format!("Needle row {i:02}")).await;
    }
    let target = places::make_project(&world, &owner, org, "Needle row selected")
        .await
        .base
        .id;
    let query = NameSearchListQuery {
        q: Some("needle row".into()),
        kinds: Some("project".into()),
        limit: Some(10),
        ..Default::default()
    };
    let ordinary = value(
        world
            .app
            .call(NameSearchList, &[])
            .token(&owner.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["items"].as_array().unwrap().len(), 10);
    let selected_key = key_with_scope(
        &world,
        owner.id(),
        json!({"rules":[selected("project",target,"NameSearchList")]}),
    )
    .await;
    let answer = value(
        world
            .app
            .call(NameSearchList, &[])
            .token(&selected_key)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(answer["items"].as_array().unwrap().len(), 1);
    assert_eq!(answer["items"][0]["id"], json!(target));
    assert_eq!(answer["items"][0]["name"], "Needle row selected");
    assert!(
        answer["items"][0]["path"]
            .as_array()
            .unwrap()
            .iter()
            .all(|step| step["id"] == json!(target)),
        "unselected ancestor labels do not ride on a child result's grant"
    );
    let unrelated = key_with_scope(
        &world,
        owner.id(),
        json!({"rules":[selected("project",target,"ProjectGet")]}),
    )
    .await;
    assert_eq!(
        world
            .app
            .call(NameSearchList, &[])
            .token(&unrelated)
            .query(&query)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}
#[tokio::test]
async fn name_search_tags_use_the_stored_defining_node_without_turning_tag_ids_into_nodes() {
    use orch_api_contract::directory::{NodeTagCreate, NodeTagCreateBody};
    let world = World::new().await;
    let owner = places::person(&world, "tagger").await;
    let org = places::make_org(&world, &owner, "Tag search", "tag-search")
        .await
        .base
        .id;
    let tag = world
        .app
        .call(NodeTagCreate, &[&org])
        .token(&owner.token)
        .body(&NodeTagCreateBody {
            name: "tag scope sentinel".into(),
            color: None,
        })
        .send()
        .await
        .ok();
    let query = NameSearchListQuery {
        q: Some("tag scope sentinel".into()),
        kinds: Some("tag".into()),
        ..Default::default()
    };
    let ordinary = value(
        world
            .app
            .call(NameSearchList, &[])
            .token(&owner.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["items"][0]["id"], json!(tag.id));
    let allowed = key_with_scope(
        &world,
        owner.id(),
        json!({"rules":[selected("org",org,"NameSearchList")]}),
    )
    .await;
    let answer = value(
        world
            .app
            .call(NameSearchList, &[])
            .token(&allowed)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(answer, ordinary);
    let wrong = key_with_scope(
        &world,
        owner.id(),
        json!({"rules":[selected("org",tag.id,"NameSearchList")]}),
    )
    .await;
    assert!(
        value(
            world
                .app
                .call(NameSearchList, &[])
                .token(&wrong)
                .query(&query)
                .send_bytes()
                .await
        )["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
async fn document(s: &rig::Scene, title: &str, text: &str) -> Id {
    use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
    let reply = s
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: title.into(),
            blocks: None,
            markdown: Some(text.into()),
            access: None,
        })
        .send_bytes()
        .await;
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let reply: Json = serde_json::from_slice(&reply.raw).unwrap();
    serde_json::from_value(reply["base"]["id"].clone()).unwrap()
}
async fn weights(s: &rig::Scene, place: &str, references: &str) {
    use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&s.tree.topic.base.id, &"related.weights"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!({"meaning":"0","place":place,"references":references}),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}
#[tokio::test]
async fn related_results_bind_source_and_selected_peers_and_keep_native_ordinary_ranking() {
    let s = rig::Scene::new().await;
    let world = &s.rig.world;
    let source = document(&s, "Scoped related source", "Source").await;
    let peer = document(&s, "Scoped related peer", "Peer").await;
    weights(&s, "1", "0").await;
    let ordinary = value(
        world
            .app
            .call(DocumentRelatedList, &[&source])
            .token(&s.ada.token)
            .send_bytes()
            .await,
    );
    assert!(
        ordinary["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == json!(peer))
    );
    let selected_key=key_with_scope(world,s.ada.id(),json!({"rules":[selected("document",source,"DocumentRelatedList"),selected("document",peer,"DocumentRelatedList")]})).await;
    let reply = value(
        world
            .app
            .call(DocumentRelatedList, &[&source])
            .token(&selected_key)
            .send_bytes()
            .await,
    );
    assert_eq!(reply["items"].as_array().unwrap().len(), 1);
    assert_eq!(reply["items"][0]["id"], json!(peer));
    assert_eq!(reply["items"][0]["name"], "Scoped related peer");
    assert!(reply["items"][0]["context"].as_array().unwrap().is_empty());
    let broad=key_with_scope(world,s.ada.id(),json!({"rules":[{"target":{"kind":"org","id":s.tree.org.base.id,"reach":"subtree"},"allow":[{"operation":"DocumentRelatedList","actions":["read"]}]}]})).await;
    assert_eq!(
        value(
            world
                .app
                .call(DocumentRelatedList, &[&source])
                .token(&broad)
                .send_bytes()
                .await
        ),
        ordinary
    );
    let source_only = key_with_scope(
        world,
        s.ada.id(),
        json!({"rules":[selected("document",source,"DocumentRelatedList")]}),
    )
    .await;
    assert!(
        value(
            world
                .app
                .call(DocumentRelatedList, &[&source])
                .token(&source_only)
                .send_bytes()
                .await
        )["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn related_chat_results_keep_native_participation_and_their_actual_chat_identity() {
    use orch_api_contract::chats::{OrgChatCreate, OrgChatCreateBody};
    let s = rig::Scene::new().await;
    let world = &s.rig.world;
    let chat = world
        .app
        .call(OrgChatCreate, &[&s.tree.org.base.id])
        .token(&s.ada.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![s.cai.id()],
            name: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the ordinary chat fixture is full");
    let source = document(
        &s,
        "Scoped chat reference",
        &format!("[Chat](/chats/{})", chat.base.id),
    )
    .await;
    weights(&s, "0", "1").await;
    let ordinary = value(
        world
            .app
            .call(DocumentRelatedList, &[&source])
            .token(&s.ada.token)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["items"].as_array().unwrap().len(), 1);
    assert_eq!(ordinary["items"][0]["id"], json!(chat.base.id));
    let selected_key=key_with_scope(world,s.ada.id(),json!({"rules":[selected("document",source,"DocumentRelatedList"),selected("chat",chat.base.id,"DocumentRelatedList")]})).await;
    let reply = value(
        world
            .app
            .call(DocumentRelatedList, &[&source])
            .token(&selected_key)
            .send_bytes()
            .await,
    );
    assert_eq!(reply["items"][0]["name"], ordinary["items"][0]["name"]);
    assert_eq!(reply["items"][0]["kind"], "chat");
    let wrong=key_with_scope(world,s.ada.id(),json!({"rules":[selected("document",source,"DocumentRelatedList"),selected("thread",chat.base.id,"DocumentRelatedList")]})).await;
    assert!(
        value(
            world
                .app
                .call(DocumentRelatedList, &[&source])
                .token(&wrong)
                .send_bytes()
                .await
        )["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn name_publication_cannot_replace_an_original_tag_with_another_under_the_same_node() {
    use orch_api_contract::directory::{NodeTagCreate, NodeTagCreateBody, TagDelete};
    for replace in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "tag_publisher").await;
        let org = places::make_org(&world, &owner, "Published tags", "published-tags")
            .await
            .base
            .id;
        let original = world
            .app
            .call(NodeTagCreate, &[&org])
            .token(&owner.token)
            .body(&NodeTagCreateBody {
                name: "Tag source original sentinel".into(),
                color: None,
            })
            .send()
            .await
            .ok();
        let secret = key_with_scope(
            &world,
            owner.id(),
            json!({"rules":[selected("org",org,"NameSearchList")]}),
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let action = tokio::spawn(async move {
            app.call(NameSearchList, &[])
                .token(&secret)
                .query(&NameSearchListQuery {
                    q: Some("tag source".into()),
                    kinds: Some("tag".into()),
                    ..Default::default()
                })
                .send_bytes()
                .await
        });
        // Same real-lock premise and watchdog as the other native publication controls.
        let wait = std::time::Duration::from_secs(30);
        let observed=tokio::time::timeout(wait,async {loop {
            if action.is_finished(){return false}
            let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if found {return true}tokio::task::yield_now().await;
        }}).await.unwrap();
        assert!(observed, "the actual final activity INSERT must be blocked");
        if replace {
            world
                .app
                .call(TagDelete, &[&original.id])
                .token(&owner.token)
                .send()
                .await
                .ok();
            let replacement = world
                .app
                .call(NodeTagCreate, &[&org])
                .token(&owner.token)
                .body(&NodeTagCreateBody {
                    name: "Tag source replacement sentinel".into(),
                    color: None,
                })
                .send()
                .await
                .ok();
            assert_ne!(replacement.id, original.id);
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, action).await.unwrap().unwrap();
        if replace {
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "same Node governor does not prove the old Tag record still belongs in this result"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Tag source original sentinel"));
        } else {
            let reply = value(reply);
            assert_eq!(reply["items"][0]["id"], json!(original.id));
            assert_eq!(reply["items"][0]["name"], "Tag source original sentinel");
        }
    }
}

#[tokio::test]
async fn content_search_selects_actual_document_governors_before_its_native_limit() {
    use orch_api_contract::search::{SearchList, SearchListQuery};
    let s = rig::Scene::new().await;
    let world = &s.rig.world;
    let wanted = document(
        &s,
        "Selected document",
        "Native scopedcontentneedle passage",
    )
    .await;
    for index in 0..8 {
        world.advance(chrono::Duration::seconds(1));
        document(
            &s,
            &format!("Unselected newer document {index}"),
            "Native scopedcontentneedle passage",
        )
        .await;
    }
    let query = SearchListQuery {
        q: "scopedcontentneedle".into(),
        kinds: Some("document".into()),
        limit: Some(1),
        ..Default::default()
    };
    let ordinary = value(
        world
            .app
            .call(SearchList, &[])
            .token(&s.ada.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["items"].as_array().unwrap().len(), 1);
    assert_ne!(ordinary["items"][0]["document"]["id"], json!(wanted));
    let selected_key = key_with_scope(
        world,
        s.ada.id(),
        json!({"rules":[selected("document",wanted,"SearchList")]}),
    )
    .await;
    let answer = value(
        world
            .app
            .call(SearchList, &[])
            .token(&selected_key)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(answer["items"].as_array().unwrap().len(), 1);
    assert_eq!(answer["items"][0]["document"]["id"], json!(wanted));
    assert!(answer["next_cursor"].is_null());
    assert!(
        answer["items"][0]["document"]["node"]["redacted"]
            .as_bool()
            .unwrap(),
        "a Document scope does not lend this search's protected parent labels"
    );
    let unrelated = key_with_scope(
        world,
        s.ada.id(),
        json!({"rules":[selected("document",wanted,"DocumentGet")]}),
    )
    .await;
    assert_eq!(
        world
            .app
            .call(SearchList, &[])
            .token(&unrelated)
            .query(&query)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}
#[tokio::test]
async fn content_search_scopes_own_metadata_but_never_grants_the_native_document_view() {
    use orch_api_contract::search::{SearchList, SearchListQuery};
    let s = rig::Scene::new().await;
    let world = &s.rig.world;
    let wanted = document(&s, "Native context document", "contextboundneedle passage").await;
    let query = SearchListQuery {
        q: "contextboundneedle".into(),
        document_id: Some(wanted),
        kinds: Some("document".into()),
        ..Default::default()
    };
    let ordinary = value(
        world
            .app
            .call(SearchList, &[])
            .token(&s.ada.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["items"].as_array().unwrap().len(), 1);
    let key=key_with_scope(world,s.ada.id(),json!({"rules":[{"target":{"kind":"org","id":s.tree.org.base.id,"reach":"subtree"},"allow":[{"operation":"SearchList","actions":["read"]}]}]})).await;
    assert_eq!(
        value(
            world
                .app
                .call(SearchList, &[])
                .token(&key)
                .query(&query)
                .send_bytes()
                .await
        ),
        ordinary
    );
    let stranger = places::person(world, "search_scope_stranger").await;
    let key = key_with_scope(
        world,
        stranger.id(),
        json!({"rules":[selected("document",wanted,"SearchList")]}),
    )
    .await;
    let answer = value(
        world
            .app
            .call(SearchList, &[])
            .token(&key)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert!(answer["items"].as_array().unwrap().is_empty());
}
#[tokio::test]
async fn content_search_item_ids_resolve_to_their_actual_thread_and_keep_native_continuations() {
    use orch_api_contract::search::{SearchList, SearchListQuery};
    let d = desk::Desk::new().await;
    let (thread, segment) =
        search_world::thread(&d, &d.ada, d.topic, "Scoped search history").await;
    let first = search_world::say(
        &d,
        &d.ada,
        thread,
        segment,
        "threadgovernorneedle first",
        d.world.now() - chrono::Duration::minutes(2),
    )
    .await;
    let second = search_world::say(
        &d,
        &d.ada,
        thread,
        segment,
        "threadgovernorneedle second",
        d.world.now() - chrono::Duration::minutes(1),
    )
    .await;
    let query = SearchListQuery {
        q: "threadgovernorneedle".into(),
        scope: Some(thread),
        kinds: Some("user_message".into()),
        limit: Some(1),
        ..Default::default()
    };
    let selected_key=key_with_scope(&d.world,d.ada.id(),json!({"rules":[{"target":{"kind":"org","id":d.org,"reach":"subtree"},"allow":[{"operation":"SearchList","actions":["read"]}]}]})).await;
    let ordinary = value(
        d.world
            .app
            .call(SearchList, &[])
            .token(&d.ada.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["items"][0]["item"]["id"], json!(second));
    assert!(ordinary["next_cursor"].is_string());
    let answer = value(
        d.world
            .app
            .call(SearchList, &[])
            .token(&selected_key)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(answer, ordinary);
    let mut next = query.clone();
    next.cursor = answer["next_cursor"].as_str().map(str::to_owned);
    let expected = value(
        d.world
            .app
            .call(SearchList, &[])
            .token(&d.ada.token)
            .query(&next)
            .send_bytes()
            .await,
    );
    assert_eq!(expected["items"][0]["item"]["id"], json!(first));
    assert_eq!(
        value(
            d.world
                .app
                .call(SearchList, &[])
                .token(&selected_key)
                .query(&next)
                .send_bytes()
                .await
        ),
        expected
    );
    let exact = key_with_scope(
        &d.world,
        d.ada.id(),
        json!({"rules":[selected("thread",thread,"SearchList")]}),
    )
    .await;
    let actual = value(
        d.world
            .app
            .call(SearchList, &[])
            .token(&exact)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(actual["items"][0]["item"]["id"], json!(second));
    assert_eq!(actual["items"][0]["thread"]["id"], json!(thread));
}

#[tokio::test]
async fn content_search_native_name_hits_keep_their_actual_org_family() {
    use orch_api_contract::search::{SearchList, SearchListQuery, SearchSort};
    let d = desk::Desk::new().await;
    let query = SearchListQuery {
        q: "Acme".into(),
        sort: Some(SearchSort::Relevance),
        title_only: Some(true),
        ..Default::default()
    };
    let ordinary = value(
        d.world
            .app
            .call(SearchList, &[])
            .token(&d.ada.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert!(
        ordinary["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hit| hit["kind"] == "name" && hit["node"]["id"] == json!(d.org))
    );
    let selected_key = key_with_scope(
        &d.world,
        d.ada.id(),
        json!({"rules":[selected("org",d.org,"SearchList")]}),
    )
    .await;
    let answer = value(
        d.world
            .app
            .call(SearchList, &[])
            .token(&selected_key)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(answer["items"].as_array().unwrap().len(), 1);
    assert_eq!(answer["items"][0]["kind"], "name");
    assert_eq!(answer["items"][0]["node"]["id"], json!(d.org));
}

fn stream_events(reply: orch_api_core::testing::RawReply) -> Vec<Json> {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    reply
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}
#[tokio::test]
async fn streamed_search_binds_its_native_parts_without_borrowing_the_page_operation() {
    use orch_api_contract::search::{SearchSort, SearchStreamList, SearchStreamListQuery};
    let s = rig::Scene::new().await;
    let world = &s.rig.world;
    let wanted = document(&s, "Stream target", "streamscopeneedle selected passage").await;
    let other = document(&s, "Stream other", "streamscopeneedle unselected passage").await;
    let query = SearchStreamListQuery {
        q: "streamscopeneedle".into(),
        sort: Some(SearchSort::Newest),
        kinds: Some("document".into()),
        immediate: Some(true),
        ..Default::default()
    };
    let ordinary = stream_events(
        world
            .app
            .call(SearchStreamList, &[])
            .token(&s.ada.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert!(
        ordinary
            .iter()
            .any(|event| event["type"] == "hit" && event["hit"]["document"]["id"] == json!(wanted))
    );
    let key = key_with_scope(
        world,
        s.ada.id(),
        json!({"rules":[selected("document",wanted,"SearchStreamList")]}),
    )
    .await;
    let actual = stream_events(
        world
            .app
            .call(SearchStreamList, &[])
            .token(&key)
            .query(&query)
            .send_bytes()
            .await,
    );
    let hits: Vec<_> = actual
        .iter()
        .filter(|event| event["type"] == "hit")
        .collect();
    assert!(!hits.is_empty());
    assert!(
        hits.iter()
            .all(|event| event["hit"]["document"]["id"] == json!(wanted))
    );
    assert!(
        !actual
            .iter()
            .any(|event| event["type"] == "hit" && event["hit"]["document"]["id"] == json!(other))
    );
    let wrong = key_with_scope(
        world,
        s.ada.id(),
        json!({"rules":[selected("document",wanted,"SearchList")]}),
    )
    .await;
    assert_eq!(
        world
            .app
            .call(SearchStreamList, &[])
            .token(&wrong)
            .query(&query)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        s.rig.commands.issued_of("provider.embed"),
        0,
        "native newest mode keeps meaning disabled"
    );
}
#[tokio::test]
async fn streamed_cursor_keeps_the_original_key_when_another_key_continues_it() {
    use orch_api_contract::search::{SearchSort, SearchStreamList, SearchStreamListQuery};
    let s = rig::Scene::new().await;
    let world = &s.rig.world;
    for index in 0..4 {
        document(&s, &format!("Cursororiginneedle {index}"), "Unmatched body").await;
    }
    world.set("search.page_size", json!(1)).await;
    world.set("search.word_candidates", json!(1)).await;
    world
        .set(
            "search.word_tuning",
            json!({"scope_candidates":1,"request_windows":1,"tiers":{}}),
        )
        .await;
    let query = SearchStreamListQuery {
        q: "cursororiginneedle".into(),
        scope: Some(s.tree.org.base.id),
        sort: Some(SearchSort::Relevance),
        title_only: Some(true),
        kinds: Some("document".into()),
        immediate: Some(true),
        ..Default::default()
    };
    let rule = json!({"rules":[{"target":{"kind":"org","id":s.tree.org.base.id,"reach":"subtree"},"allow":[{"operation":"SearchStreamList","actions":["read"]}]}]});
    let original = key_with_scope(world, s.ada.id(), rule.clone()).await;
    let controller = key_with_scope(world, s.ada.id(), rule).await;
    let native = stream_events(
        world
            .app
            .call(SearchStreamList, &[])
            .token(&s.ada.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert!(
        native
            .iter()
            .any(|event| event["type"] == "cursor" && event["cursor"].is_string())
    );
    let first = stream_events(
        world
            .app
            .call(SearchStreamList, &[])
            .token(&original)
            .query(&query)
            .send_bytes()
            .await,
    );
    let cursor = first
        .iter()
        .find(|event| event["type"] == "cursor")
        .unwrap()["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE token_hash=$1")
        .bind(token_hash(&original))
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut next = query;
    next.cursor = Some(cursor);
    let refused = world
        .app
        .call(SearchStreamList, &[])
        .token(&controller)
        .query(&next)
        .send_bytes()
        .await;
    assert_eq!(
        refused.status,
        StatusCode::UNAUTHORIZED,
        "a current key of the same account cannot replace the original source key"
    );
}
#[tokio::test]
async fn streamed_search_stops_delivery_after_the_actual_key_expires_between_frames() {
    use axum::body::Body;
    use http::Request;
    use http_body_util::BodyExt;
    use orch_api_contract::search::{SearchSort, SearchStreamListQuery};
    use tower::ServiceExt;
    let s = rig::Scene::new().await;
    let world = &s.rig.world;
    let doc = document(&s, "Frame source", "streamframeneedle protected passage").await;
    let key = key_with_scope(
        world,
        s.ada.id(),
        json!({"rules":[selected("document",doc,"SearchStreamList")]}),
    )
    .await;
    let query = SearchStreamListQuery {
        q: "streamframeneedle".into(),
        document_id: Some(doc),
        sort: Some(SearchSort::Newest),
        immediate: Some(true),
        ..Default::default()
    };
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(world.app.app.clone());
    let request = Request::builder()
        .uri(format!(
            "/search/stream?{}",
            serde_urlencoded::to_string(&query).unwrap()
        ))
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    let first: Json = serde_json::from_slice(&first).unwrap();
    assert_eq!(first["type"], "options");
    world.advance(chrono::Duration::hours(2));
    assert!(
        body.frame().await.is_none_or(|frame| frame.is_err()),
        "no queued old part is delivered after expiry"
    );
}

#[tokio::test]
async fn streamed_history_keeps_its_original_projection_even_when_the_same_parts_remain_readable() {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use orch_api_contract::search::{SearchSort, SearchStreamListQuery};
    use tower::ServiceExt;
    for changed in [false, true] {
        let d = desk::Desk::new().await;
        let (thread, segment) =
            search_world::thread(&d, &d.ada, d.topic, "Original stream history").await;
        let item = search_world::say(
            &d,
            &d.ada,
            thread,
            segment,
            "originalhistoryneedle passage",
            d.world.now(),
        )
        .await;
        let key = key_with_scope(
            &d.world,
            d.ada.id(),
            json!({"rules":[selected("thread",thread,"SearchStreamList")]}),
        )
        .await;
        let query = SearchStreamListQuery {
            q: "originalhistoryneedle".into(),
            scope: Some(thread),
            kinds: Some("user_message".into()),
            sort: Some(SearchSort::Newest),
            projection_revision: Some(1),
            immediate: Some(true),
            ..Default::default()
        };
        let mut registry = orch_api_core::registry::Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let router = registry
            .into_router_for(orch_api_contract::surface::HttpSurface::Key)
            .with_state(d.world.app.app.clone());
        let response = router
            .oneshot(
                http::Request::builder()
                    .uri(format!(
                        "/search/stream?{}",
                        serde_urlencoded::to_string(&query).unwrap()
                    ))
                    .header("authorization", format!("Bearer {key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        loop {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(30), body.frame())
                .await
                .unwrap()
                .expect("the original part is delivered while its projection is current")
                .unwrap()
                .into_data()
                .unwrap();
            let event: Json = serde_json::from_slice(&frame).unwrap();
            if event["type"] == "hit" {
                assert_eq!(event["id"], format!("item:{item}"));
                break;
            }
        }
        if changed {
            // Change the actual projection revision while retaining every original item/segment.
            sqlx::query(
                "UPDATE threads SET execution_revision=execution_revision+1 WHERE node_id=$1",
            )
            .bind(thread)
            .execute(&d.world.db.pool)
            .await
            .unwrap();
        }
        let remaining = tokio::time::timeout(std::time::Duration::from_secs(30), body.collect())
            .await
            .unwrap();
        if changed {
            assert!(
                remaining.is_err(),
                "the same readable item cannot validate an older continuation projection"
            );
        } else {
            assert!(
                remaining.is_ok(),
                "the unchanged native projection keeps its complete stream"
            );
        }
        let present: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM items WHERE id=$1 AND thread_id=$2)")
                .bind(item)
                .bind(thread)
                .fetch_one(&d.world.db.pool)
                .await
                .unwrap();
        assert!(
            present,
            "the deciding difference is the native projection, not replacement or removal of the part"
        );
    }
}
