//! Link grants retain native authority and record a validated context with the real server actor.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

#[path = "link_grants/permission_rules.rs"]
mod permission_rules;
mod places;
mod rig;
mod support;

use orch_api_contract::{
    access::{PermissionPageUpdate, SharePreview, SharePreviewCreate},
    chats::{OrgChatCreate, OrgChatCreateBody},
    common::IDEMPOTENCY_HEADER,
    documents::NodeDocumentCreate,
    notifications::{ChatPersonGrant, ChatPersonGrantBody},
    share::{AgentReadBeforeReason, ShareAgentCreate},
};
use orch_types::{Id, Json, new_id};
use rig::Scene;
use serde_json::json;

async fn document(s: &Scene) -> Id {
    let topic = places::make_topic(&s.rig.world, &s.ada, s.tree.project.base.id, "Source topic")
        .await
        .base
        .id;
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&topic])
        .token(&s.ada.token)
        .raw_body(&json!({"title":"Linked private material","access":"private"}))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id
}

fn gift(person: Id, origin: Json) -> Json {
    let mut body = json!({"name":[{"choice":"full_view","holders":[{"kind":"user","id":person}],"export":false}]});
    if !origin.is_null() {
        body["link_origin"] = origin;
    }
    body
}

fn share(document: Id, recipient: Id) -> Json {
    json!({"what":{"document_id":document},"parts":{"documents":"read"},"download":false,
        "mode":"live","to":[{"thread_id":recipient}],"client_id":"explicit-link-view"})
}

async fn preview(s: &Scene, document: Id, body: &Json) -> SharePreview {
    s.rig
        .world
        .app
        .call(SharePreviewCreate, &[&"documents", &document])
        .token(&s.ada.token)
        .raw_body(&json!({"kind":"agents","share":body}))
        .send()
        .await
        .ok()
}

async fn counts(s: &Scene) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM permission_entries),(SELECT count(*) FROM audit_log),(SELECT count(*) FROM thread_inputs)")
        .fetch_one(s.rig.db.pool()).await.unwrap()
}

async fn attribution(s: &Scene, object: Id) -> Vec<Json> {
    // Permission audits name the changed entry; its native target is retained in details.object.
    sqlx::query_scalar("SELECT details->'link_origin' FROM audit_log WHERE (object_id=$1 OR details->'object'->>'id'=$1::text) AND details ? 'link_origin' ORDER BY at,id")
        .bind(object).fetch_all(s.rig.db.pool()).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn effective_read_is_captured_before_the_trial_and_includes_other_current_givers() {
    let s = Scene::new().await;
    s.rig.tools_of_agents(json!("mcp")).await;
    let receiver = s.thread_by(&s.ada, "Recipient").await.base.id;
    let doc = document(&s).await;
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc])
        .token(&s.ada.token)
        .raw_body(&gift(s.cai.id(), Json::Null))
        .send()
        .await
        .ok();
    let mut request = share(doc, receiver);
    let before = counts(&s).await;
    let fresh = preview(&s, doc, &request).await;
    assert!(fresh.allowed, "{fresh:?}");
    assert_eq!(
        fresh.agents[0].effective_before.as_ref().unwrap().readable,
        Some(false)
    );
    assert!(fresh.agents[0].current.as_ref().is_some_and(
        |current| current.state == orch_api_contract::share::AgentShareCurrentState::Active
    ));
    assert_eq!(
        counts(&s).await,
        before,
        "proposed access cannot become the baseline"
    );

    s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.cai.token)
        .raw_body(&request)
        .send()
        .await
        .ok();
    let from_another_giver = preview(&s, doc, &request).await;
    assert_eq!(
        from_another_giver.agents[0].already_shared,
        Default::default()
    );
    assert_eq!(
        from_another_giver.agents[0]
            .effective_before
            .as_ref()
            .unwrap()
            .readable,
        Some(true)
    );
    let entry: Id = sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='user' AND holder_id=$2 AND permission='doc.view' LIMIT 1")
        .bind(doc).bind(s.cai.id()).fetch_one(s.rig.db.pool()).await.unwrap();
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc])
        .token(&s.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","remove":[entry]}]}))
        .send()
        .await
        .ok();
    let revoked = preview(&s, doc, &request).await;
    assert_eq!(
        revoked.agents[0]
            .effective_before
            .as_ref()
            .unwrap()
            .readable,
        Some(false)
    );

    request["link_origin"] = json!({"kind":"thread_input","thread_id":receiver});
    s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.ada.token)
        .raw_body(&request)
        .send()
        .await
        .ok();
    let origins = attribution(&s, doc).await;
    assert!(!origins.is_empty());
    assert!(origins.iter().all(
        |origin| origin == &json!({"kind":"thread_input","thread_id":receiver,"by":s.ada.id()})
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_place_recipient_is_unassessed_instead_of_becoming_a_fabricated_agent() {
    let s = Scene::new().await;
    let doc = document(&s).await;
    let mut request = share(doc, new_id());
    request["to"] = json!([{"node_id":s.tree.topic.base.id}]);
    let before = counts(&s).await;
    let result = preview(&s, doc, &request).await;
    assert!(result.allowed, "{result:?}");
    let fact = result.agents[0].effective_before.as_ref().unwrap();
    assert_eq!(fact.readable, None);
    assert_eq!(
        fact.reason,
        Some(AgentReadBeforeReason::RecipientContextRequired)
    );
    assert_eq!(counts(&s).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn human_link_grants_validate_send_context_and_record_only_the_authenticated_actor() {
    let s = Scene::new().await;
    let doc = document(&s).await;
    let chat = s.tree.topic.base.chat_id;
    let before = counts(&s).await;
    let forged = gift(
        s.cai.id(),
        json!({"kind":"chat","chat_id":chat,"by":s.eve.id()}),
    );
    let reply = s
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc])
        .token(&s.ada.token)
        .raw_body(&forged)
        .send_bytes()
        .await;
    assert_eq!(reply.status.as_u16(), 400);
    assert_eq!(counts(&s).await, before);
    let private_chat = s
        .rig
        .world
        .app
        .call(OrgChatCreate, &[&s.tree.org.base.id])
        .token(&s.cai.token)
        .body(&OrgChatCreateBody {
            home_id: None,
            visibility: None,
            user_ids: vec![s.dan.id(), s.eve.id()],
            name: Some("Other people's context".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    let denied = gift(s.cai.id(), json!({"kind":"chat","chat_id":private_chat}));
    let before = counts(&s).await;
    let refused = s
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc])
        .token(&s.ada.token)
        .raw_body(&denied)
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    assert_eq!(
        counts(&s).await,
        before,
        "inspection does not establish a sending context"
    );
    let valid = gift(s.cai.id(), json!({"kind":"chat","chat_id":chat}));
    for _ in 0..2 {
        s.rig
            .world
            .app
            .call(PermissionPageUpdate, &[&"documents", &doc])
            .token(&s.ada.token)
            .raw_body(&valid)
            .send()
            .await
            .ok();
    }
    let origins = attribution(&s, doc).await;
    assert_eq!(
        origins.len(),
        1,
        "a repeated coalesced viewing gift adds no second grant audit"
    );
    assert_eq!(
        origins[0],
        json!({"kind":"chat","chat_id":chat,"by":s.ada.id()})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_person_link_grants_keep_native_replay_without_a_second_grant_or_notice() {
    let s = Scene::new().await;
    let source = s.tree.topic.base.chat_id;
    let target = s
        .rig
        .world
        .app
        .call(OrgChatCreate, &[&s.tree.org.base.id])
        .token(&s.ada.token)
        .body(&OrgChatCreateBody {
            home_id: None,
            visibility: None,
            user_ids: vec![s.cai.id(), s.dan.id()],
            name: Some("Target group".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    let body = ChatPersonGrantBody {
        link_origin: Some(orch_access_contract::audit::LinkOrigin::Chat { chat_id: source }),
        choice: orch_api_contract::access::AccessChoice::FullInteract,
        notify: false,
        anchor: None,
    };
    let request_key = new_id().to_string();
    s.rig
        .world
        .app
        .call(ChatPersonGrant, &[&target, &s.eve.id()])
        .token(&s.ada.token)
        .header(IDEMPOTENCY_HEADER, request_key.as_str())
        .body(&body)
        .send()
        .await
        .ok();
    let first = attribution(&s, target).await;
    assert!(!first.is_empty());
    assert!(
        first
            .iter()
            .all(|origin| origin == &json!({"kind":"chat","chat_id":source,"by":s.ada.id()}))
    );
    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(s.tree.topic.base.id),
            "chat.enabled",
            json!(false),
        )
        .await;
    let before = counts(&s).await;
    s.rig
        .world
        .app
        .call(ChatPersonGrant, &[&target, &s.eve.id()])
        .token(&s.ada.token)
        .header(IDEMPOTENCY_HEADER, request_key.as_str())
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(
        counts(&s).await,
        before,
        "an accepted replay is independent of new-effect context"
    );
    assert_eq!(attribution(&s, target).await, first);
}
