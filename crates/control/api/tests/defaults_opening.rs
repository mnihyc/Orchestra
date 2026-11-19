//! The opt-in Defaults opening reuses the native readers and keeps their independent authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod chat_world;
mod places;
mod support;
use support::FullTokenReply as _;

use chat_world::Room;
use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate, MeTokenDelete, SensitiveProof};
use orch_api_contract::chats::NodeChatOpeningGet;
use orch_api_contract::nodes::NodeGet;
use orch_api_contract::resources::{
    CreateProvider, Provider, ProviderCreate, ProviderGet, Upstream,
};
use orch_api_contract::settings::{
    NodeSecretList, NodeSecretSet, NodeSecretSetBody, NodeSettingList, NodeSettingSet,
    NodeSettingSetBody, SettingRegistryGet,
};
use orch_api_core::testing::RawReply;
use orch_db::secrets::MasterKey;
use orch_types::Id;
use serde_json::{Value, json};

const SECRET_VALUE: &str = "defaults-opening-private-value-never-returned";
const OPEN: &str =
    "defaults=true&secret_names=true&default_provider_name=true&scope=topic&limit=100";

fn data(reply: &RawReply) -> Value {
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn opening(room: &Room, token: &str, node: Id, query: &str) -> RawReply {
    room.world
        .app
        .call(NodeChatOpeningGet, &[&node])
        .token(token)
        .raw_query(query)
        .send_bytes()
        .await
}

async fn configure(room: &Room) -> Provider {
    orch_api_core::routes::install_master_key(&MasterKey::new([9; 32]));
    let home = places::make_org(
        &room.world,
        &room.ada,
        "Private provider home",
        "private-models",
    )
    .await;
    let provider = room
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&room.ada.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Selected private provider".into(),
            home_id: home.base.id,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".into(),
                base_url: "https://api.example.test/v1".into(),
                options: json!({}),
            }],
            api_key: Some("sk-defaults-fixture".into()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    room.world
        .app
        .call(NodeSettingSet, &[&room.root, &"agent.default_provider_id"])
        .token(&room.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(provider.base.id),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    room.world
        .app
        .call(NodeSecretSet, &[&room.root, &"PIPELINE_TOKEN"])
        .token(&room.ada.token)
        .body(&NodeSecretSetBody {
            value: SECRET_VALUE.into(),
        })
        .send()
        .await
        .ok();
    provider
}

#[tokio::test]
async fn ordinary_chat_opening_keeps_its_native_wire_without_defaults() {
    let room = Room::new().await;
    let expected = opening(&room, &room.ada.token, room.topic, "scope=topic&limit=100").await;
    assert_eq!(expected.status, 200);
    let expected = data(&expected);
    let mut keys = expected
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, vec!["opening", "scopes"]);
    assert!(expected["opening"].is_object());
    assert!(!expected["scopes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_defaults_opening_matches_each_native_reader_and_preserves_names_without_secret_values() {
    let room = Room::new().await;
    let provider = configure(&room).await;
    room.say(
        &room.ada,
        room.chat,
        "The selected visible chat remains real",
    )
    .await;
    let registry = room
        .world
        .app
        .call(SettingRegistryGet, &[])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    for node in [room.root, room.project, room.topic] {
        let native_node = room
            .world
            .app
            .call(NodeGet, &[&node])
            .token(&room.ada.token)
            .send()
            .await
            .ok();
        let settings = room
            .world
            .app
            .call(NodeSettingList, &[&node])
            .token(&room.ada.token)
            .send()
            .await
            .ok();
        let secrets = room
            .world
            .app
            .call(NodeSecretList, &[&node])
            .token(&room.ada.token)
            .send()
            .await
            .ok();
        let native_provider = room
            .world
            .app
            .call(ProviderGet, &[&provider.base.id])
            .token(&room.ada.token)
            .send()
            .await
            .ok();
        let chat = opening(&room, &room.ada.token, node, "scope=topic&limit=100").await;
        assert_eq!(
            chat.status, 200,
            "the original native opening must work before opt-in"
        );
        let opened = opening(&room, &room.ada.token, node, OPEN).await;
        assert_eq!(opened.status, 200, "the opt-in projection must be admitted");
        let body = data(&opened);
        let projection = &body["defaults"];
        assert_eq!(body["scopes"], data(&chat)["scopes"]);
        assert_eq!(body["opening"], data(&chat)["opening"]);
        assert_eq!(
            projection["node"],
            serde_json::to_value(native_node).unwrap()
        );
        assert_eq!(
            projection["registry"],
            serde_json::to_value(&registry).unwrap()
        );
        assert_eq!(
            projection["settings"],
            serde_json::to_value(settings).unwrap()
        );
        assert_eq!(
            projection["secrets"],
            serde_json::to_value(secrets).unwrap()
        );
        for member in ["registry_problem", "settings_problem", "secrets_problem"] {
            assert!(projection[member].is_null());
        }
        let selected = projection["providers"].as_array().unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0]["setting"], "agent.default_provider_id");
        assert_eq!(selected[0]["id"], json!(provider.base.id));
        assert_eq!(
            selected[0]["provider"],
            serde_json::to_value(native_provider).unwrap()
        );
        assert!(selected[0]["problem"].is_null());
        assert!(!String::from_utf8_lossy(&opened.raw).contains(SECRET_VALUE));
    }
}

#[tokio::test]
async fn defaults_do_not_borrow_node_view_for_a_private_selected_provider() {
    let room = Room::new().await;
    let provider = configure(&room).await;
    let native = room
        .world
        .app
        .call(ProviderGet, &[&provider.base.id])
        .token(&room.bea.token)
        .send()
        .await;
    assert_eq!(
        native.status, 403,
        "the private provider is not in this viewer's reach"
    );
    let native = native.problem();
    let chat = opening(&room, &room.bea.token, room.topic, "scope=topic&limit=100").await;
    assert_eq!(chat.status, 200);
    let opened = opening(&room, &room.bea.token, room.topic, OPEN).await;
    assert_eq!(
        opened.status, 200,
        "a denied Defaults member must not fail its visible chat"
    );
    let body = data(&opened);
    assert_eq!(body["opening"], data(&chat)["opening"]);
    let selected = body["defaults"]["providers"].as_array().unwrap();
    assert_eq!(selected.len(), 1);
    assert!(selected[0]["provider"].is_null());
    let problem = &selected[0]["problem"];
    let mut expected_problem = serde_json::to_value(native).unwrap();
    expected_problem
        .as_object_mut()
        .unwrap()
        .remove("request_id");
    let mut actual_problem = problem.clone();
    assert!(!actual_problem["request_id"].as_str().unwrap().is_empty());
    actual_problem.as_object_mut().unwrap().remove("request_id");
    assert_eq!(
        actual_problem, expected_problem,
        "all native refusal members remain intact"
    );
    let native_settings = room
        .world
        .app
        .call(NodeSettingList, &[&room.topic])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(
        body["defaults"]["settings"],
        serde_json::to_value(native_settings).unwrap()
    );
    assert!(!String::from_utf8_lossy(&opened.raw).contains("Selected private provider"));
}

#[tokio::test]
async fn name_readers_are_only_included_when_their_existing_controls_demand_them() {
    let room = Room::new().await;
    let provider = configure(&room).await;
    let opened = opening(
        &room,
        &room.ada.token,
        room.topic,
        "defaults=true&scope=topic&limit=100",
    )
    .await;
    assert_eq!(opened.status, 200);
    let body = data(&opened);
    let projection = &body["defaults"];
    assert!(
        projection["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["key"] == "agent.default_provider_id"
                && row["value"] == json!(provider.base.id))
    );
    assert!(projection["secrets"].is_null() && projection["secrets_problem"].is_null());
    assert!(projection["providers"].as_array().unwrap().is_empty());
    let plain = opening(
        &room,
        &room.ada.token,
        room.topic,
        "defaults=false&secret_names=true&default_provider_name=true&scope=topic&limit=100",
    )
    .await;
    assert_eq!(plain.status, 200);
    assert!(
        data(&plain).get("defaults").is_none(),
        "flags alone do not enable another projection"
    );
}

#[tokio::test]
async fn a_scoped_token_keeps_its_native_resource_refusal_inside_the_defaults_projection() {
    let room = Room::new().await;
    let provider = configure(&room).await;
    let made = room
        .world
        .app
        .call(MeTokenCreate, &[])
        .token(&room.ada.token)
        .body(&ApiTokenCreateBody {
            rules: None,
            name: "scoped-defaults".into(),
            scopes: Some(vec![
                "org.view".into(),
                "project.view".into(),
                "topic.view".into(),
                "chat.view".into(),
                "provider.view".into(),
            ]),
            node_id: Some(room.root),
            expires_at: None,
            proof: Some(SensitiveProof {
                password: Some(support::PASSWORD.into()),
                ..Default::default()
            }),
        })
        .send()
        .await
        .full_token()
        .ok();
    let token = made.token.unwrap();
    let node = room
        .world
        .app
        .call(NodeGet, &[&room.topic])
        .token(&token)
        .send()
        .await;
    assert_eq!(
        node.status, 200,
        "ordinary underlying View and token narrowing are legitimate"
    );
    let denied = room
        .world
        .app
        .call(ProviderGet, &[&provider.base.id])
        .token(&token)
        .send()
        .await;
    assert_eq!(
        denied.status, 403,
        "a node-bound token cannot read a standalone provider"
    );
    let opened = opening(&room, &token, room.topic, OPEN).await;
    assert_eq!(opened.status, 200);
    let body = data(&opened);
    let selected = body["defaults"]["providers"].as_array().unwrap();
    assert_eq!(selected.len(), 1);
    assert!(selected[0]["provider"].is_null());
    assert_eq!(selected[0]["problem"]["status"], 403);
    let outside =
        places::make_org(&room.world, &room.ada, "Outside scope", "outside-defaults").await;
    let refused = opening(&room, &token, outside.base.id, OPEN).await;
    assert_eq!(refused.status, 403);
    assert!(data(&refused).get("defaults").is_none());
    room.world
        .app
        .call(MeTokenDelete, &[&made.info.id])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let revoked = opening(&room, &token, room.topic, OPEN).await;
    assert_eq!(revoked.status, 401);
    assert!(
        data(&revoked).get("defaults").is_none(),
        "a prior admitted opening donates no revoked authority"
    );
}
