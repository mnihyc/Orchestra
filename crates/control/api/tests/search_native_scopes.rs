//! Search keys select native identities before the existing ranked result windows.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod rig;
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
        .ok();
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
