//! Provider model operations through real scoped-key HTTP admission. Discovery uses
//! the existing durable-command-row responder cut, not a native journal or vendor.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::resources::{
    ProviderCreate, ProviderModelCreate, ProviderModelDelete, ProviderModelList,
    ProviderModelSuggestionAccept, ProviderModelSuggestionList, ProviderModelUpdate,
};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::{access, identity, link};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::time::Duration;
use support::World;

// Match resources::answer_discovery: the ordinary fixture waits at most ten
// seconds, polling every 25 ms for the actual existing discovery command.
const DISCOVERY_WAIT: Duration = Duration::from_secs(10);
const DISCOVERY_POLL: Duration = Duration::from_millis(25);

struct Key {
    id: Id,
    secret: String,
}

struct Scene {
    world: World,
    people: resources::Company,
    machine: Id,
    provider: Id,
    sibling: Id,
    manage_entry: Id,
}

fn body(reply: &RawReply, status: StatusCode) -> Json {
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

fn id(value: &Json) -> Id {
    value.as_str().expect("a UUID string").parse().unwrap()
}

fn scope(provider: Id, allow: Json) -> Json {
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"provider","id":provider,"reach":"self"}, "allow":allow
    }]}))
    .unwrap();
    assert!(scope.validate().is_empty());
    serde_json::to_value(scope).unwrap()
}

impl Scene {
    async fn new() -> Self {
        orch_api_core::routes::resources::sealing::install_master_key(
            orch_db::secrets::MasterKey::new([7; 32]),
        );
        let world = World::new().await;
        let people = resources::company(&world).await;
        let machine = resources::machine_forwarding_to(
            &world,
            people.boss.id(),
            &["https://api.example.com/v1"],
        )
        .await;
        let mut providers = Vec::new();
        for name in ["Scoped provider", "Sibling provider"] {
            let reply = world
                .app
                .call(ProviderCreate, &[])
                .token(&people.boss.token)
                .raw_body(&json!({"kind":"api", "name":name, "home_id":people.acme,
                    "keeper_machine_id":machine,
                    "upstreams":[{"protocol":"openai-responses",
                        "base_url":"https://api.example.com/v1","options":{}}]}))
                .send_bytes()
                .await;
            providers.push(id(&body(&reply, StatusCode::OK)["id"]));
        }
        let (provider, sibling) = (providers[0], providers[1]);
        let manage_entry = resources::name_person(
            &world,
            "provider",
            provider,
            "provider.manage",
            people.viewer.id(),
        )
        .await;
        // Give the same ordinary manager access to both resources. Cross-provider
        // refusals below must come from the key boundary, not a missing ACL.
        resources::name_person(
            &world,
            "provider",
            sibling,
            "provider.manage",
            people.viewer.id(),
        )
        .await;
        resources::online(&world, machine).await;
        Self {
            world,
            people,
            machine,
            provider,
            sibling,
            manage_entry,
        }
    }

    async fn key_for(&self, user: Id, provider: Id, allow: Json) -> Key {
        let secret = new_token(API_TOKEN_PREFIX);
        let hash = token_hash(&secret);
        let id = new_id();
        identity::create_api_token_scoped(
            &mut *self.world.conn().await,
            &identity::NewApiToken {
                id,
                user_id: user,
                name: "Provider model operation control",
                token_hash: &hash,
                scopes: &[],
                node_id: None,
                at: self.world.now(),
                // Use the existing operation-key fixture's short ordinary lifetime.
                expires_at: Some(self.world.now() + chrono::Duration::hours(1)),
            },
            Some(&scope(provider, allow)),
        )
        .await
        .unwrap();
        Key { id, secret }
    }

    async fn key(&self, operation: &str, actions: &[&str]) -> Key {
        self.key_for(
            self.people.viewer.id(),
            self.provider,
            json!([{"operation":operation,"actions":actions}]),
        )
        .await
    }

    async fn add(&self, provider: Id, name: &str, display: &str) -> Id {
        let reply = self
            .world
            .app
            .call(ProviderModelCreate, &[&provider])
            .token(&self.people.boss.token)
            .raw_body(&json!({"model_name":name,"display_name":display}))
            .send_bytes()
            .await;
        id(&body(&reply, StatusCode::OK)["id"])
    }

    async fn models(&self, provider: Id) -> Vec<Json> {
        // Exact persisted allowlist, independent of whether the tested key can read it.
        sqlx::query_scalar(
            "SELECT to_jsonb(m) FROM provider_models m WHERE provider_id=$1 ORDER BY id",
        )
        .bind(provider)
        .fetch_all(&self.world.db.pool)
        .await
        .unwrap()
    }
}

#[derive(Debug, Clone, Copy)]
enum DiscoveryRequest {
    List,
    Accept,
}

impl DiscoveryRequest {
    fn operation(self) -> &'static str {
        match self {
            Self::List => "ProviderModelSuggestionList",
            Self::Accept => "ProviderModelSuggestionAccept",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Withdrawal {
    None,
    Scope,
    Credential,
    Manage,
}

// Abort an in-flight HTTP request on assertion failure; no request task can keep
// this fixture's database alive while its outer owner unwinds.
struct PendingHttp(tokio::task::JoinHandle<RawReply>);
impl Drop for PendingHttp {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn waiting_discovery(scene: &Scene) -> Id {
    tokio::time::timeout(DISCOVERY_WAIT, async {
        loop {
            sqlx::query(
                "UPDATE commands SET state='succeeded',result='{}'
                WHERE machine_id=$1 AND kind='provider.start' AND body->>'provider_id'=$2
                    AND state IN ('queued','sent','accepted')",
            )
            .bind(scene.machine)
            .bind(scene.provider.to_string())
            .execute(&scene.world.db.pool)
            .await
            .unwrap();
            let command: Option<Id> = sqlx::query_scalar(
                "SELECT id FROM commands
                WHERE machine_id=$1 AND kind='provider.discover_models'
                    AND body->>'provider_id'=$2 AND state IN ('queued','sent','accepted')
                ORDER BY created_at,id LIMIT 1",
            )
            .bind(scene.machine)
            .bind(scene.provider.to_string())
            .fetch_optional(&scene.world.db.pool)
            .await
            .unwrap();
            if let Some(command) = command {
                return command;
            }
            tokio::time::sleep(DISCOVERY_POLL).await;
        }
    })
    .await
    .expect("the ordinary model source was not asked within its existing fixture bound")
}

async fn discover(
    scene: &Scene,
    key: &Key,
    request: DiscoveryRequest,
    names: &[&str],
    report: Json,
    withdrawal: Withdrawal,
) -> RawReply {
    let app = scene.world.app.clone();
    let provider = scene.provider;
    let secret = key.secret.clone();
    let asked = json!({"model_names":names});
    let mut pending = PendingHttp(tokio::spawn(async move {
        match request {
            DiscoveryRequest::List => {
                let request = app.call(ProviderModelSuggestionList, &[&provider])
                    .token(&secret);
                request.send_bytes().await
            }
            DiscoveryRequest::Accept => {
                let request = app.call(ProviderModelSuggestionAccept, &[&provider])
                    .token(&secret)
                    .raw_body(&asked);
                request.send_bytes().await
            }
        }
    }));
    let command = tokio::select! {
        result = &mut pending.0 => {
            // In the baseline a Manage-only acceptance is refused before discovery.
            // Return that actual 403 immediately to its receipt assertion.
            assert!(matches!(withdrawal, Withdrawal::None),
                "a withdrawal control must reach the real discovery wait first");
            return result.unwrap();
        }
        command = waiting_discovery(scene) => command,
    };
    assert!(!pending.0.is_finished());
    match withdrawal {
        Withdrawal::None => {}
        Withdrawal::Scope => {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key.id)
                .bind(scope(
                    scene.sibling,
                    json!([{"operation":request.operation(),"actions":["manage","read"]}]),
                ))
                .execute(&scene.world.db.pool)
                .await
                .unwrap();
        }
        Withdrawal::Credential => {
            assert!(
                identity::revoke_api_token(
                    &mut *scene.world.conn().await,
                    key.id,
                    scene.people.viewer.id(),
                    scene.world.now()
                )
                .await
                .unwrap()
            );
        }
        Withdrawal::Manage => {
            let mut tx = scene.world.db.pool.begin().await.unwrap();
            assert!(
                access::remove_entry(
                    &mut tx,
                    scene.manage_entry,
                    scene.people.boss.id(),
                    scene.world.now()
                )
                .await
                .unwrap()
            );
            access::bump_epoch(&mut tx).await.unwrap();
            tx.commit().await.unwrap();
        }
    }
    assert!(
        link::finish_command(
            &mut *scene.world.conn().await,
            command,
            &link::Ending::Succeeded(&report),
            scene.world.now()
        )
        .await
        .unwrap()
    );
    let saved = link::command(&mut *scene.world.conn().await, command)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.kind, "provider.discover_models");
    assert_eq!(saved.body["provider_id"], json!(scene.provider));
    assert_eq!(saved.state, "succeeded");
    assert_eq!(saved.result, Some(report));
    tokio::time::timeout(DISCOVERY_WAIT, &mut pending.0)
        .await
        .expect("the synchronous discovery reply did not finish")
        .unwrap()
}

fn suggestions() -> Json {
    json!({"suggestions":[{
        "model_name":"scope-private-offer","display_name":"Scoped offer","listed":true,
        "parameters":{"efforts":["low","high"],"context_window":4096,"context_max":8192,
            "default_context":8192,"service_tiers":[{"id":"fast","label":"Fast"}],
            "default_service_tier":"fast","access_programs":["preview"],
            "default_access_program":"preview"}
    }]})
}

#[tokio::test]
async fn a_manage_only_model_create_returns_only_its_actual_receipt() {
    let scene = Scene::new().await;
    // Read on a different exact operation does not authorize this response.
    let key = scene
        .key_for(
            scene.people.viewer.id(),
            scene.provider,
            json!([
                {"operation":"ProviderModelCreate","actions":["manage"]},
                {"operation":"ProviderModelList","actions":["read"]}
            ]),
        )
        .await;
    let reply = scene
        .world
        .app
        .call(ProviderModelCreate, &[&scene.provider])
        .token(&key.secret)
        .raw_body(&json!({"model_name":"created","display_name":"Created"}))
        .send_bytes()
        .await;
    let receipt = body(&reply, StatusCode::OK);
    let model = id(&receipt["model_id"]);
    assert_eq!(
        receipt,
        json!({"projection":"receipt","provider_id":scene.provider,"model_id":model})
    );
    let models = scene.models(scene.provider).await;
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], json!(model));
    assert_eq!(models[0]["model_name"], "created");
    assert_eq!(models[0]["created_by"], json!(scene.people.viewer.id()));
}

#[tokio::test]
async fn a_manage_only_model_update_returns_only_its_actual_receipt() {
    let scene = Scene::new().await;
    let model = scene.add(scene.provider, "changed", "Before").await;
    let key = scene.key("ProviderModelUpdate", &["manage"]).await;
    let reply = scene
        .world
        .app
        .call(ProviderModelUpdate, &[&scene.provider, &model])
        .token(&key.secret)
        .raw_body(&json!({"display_name":"After","groups":["research"]}))
        .send_bytes()
        .await;
    assert_eq!(
        body(&reply, StatusCode::OK),
        json!({"projection":"receipt","provider_id":scene.provider,"model_id":model})
    );
    let models = scene.models(scene.provider).await;
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], json!(model));
    assert_eq!(models[0]["display_name"], "After");
    assert_eq!(models[0]["groups"], json!(["research"]));
}

#[tokio::test]
async fn a_manage_only_suggestion_accept_returns_only_the_actual_added_ids() {
    let scene = Scene::new().await;
    let key = scene
        .key("ProviderModelSuggestionAccept", &["manage"])
        .await;
    let reply = discover(
        &scene,
        &key,
        DiscoveryRequest::Accept,
        &["scope-private-offer", "scope-private-offer"],
        suggestions(),
        Withdrawal::None,
    )
    .await;
    let receipt = body(&reply, StatusCode::OK);
    let models = scene.models(scene.provider).await;
    assert_eq!(models.len(), 1, "duplicate selections add one model");
    assert_eq!(
        receipt,
        json!({"projection":"receipt","provider_id":scene.provider,
        "model_ids":[models[0]["id"].clone()]})
    );
    assert_eq!(models[0]["source"], "suggested");
    assert_eq!(models[0]["created_by"], json!(scene.people.viewer.id()));
    let again = discover(
        &scene,
        &key,
        DiscoveryRequest::Accept,
        &["scope-private-offer"],
        suggestions(),
        Withdrawal::None,
    )
    .await;
    assert_eq!(
        body(&again, StatusCode::OK),
        json!({"projection":"receipt",
        "provider_id":scene.provider,"model_ids":[]})
    );
    assert_eq!(scene.models(scene.provider).await, models);
}

#[tokio::test]
async fn the_same_model_operations_with_read_return_their_existing_full_shapes() {
    let scene = Scene::new().await;
    let create = scene.key("ProviderModelCreate", &["manage", "read"]).await;
    let reply = scene
        .world
        .app
        .call(ProviderModelCreate, &[&scene.provider])
        .token(&create.secret)
        .raw_body(&json!({"model_name":"manual","display_name":"Before"}))
        .send_bytes()
        .await;
    let created = body(&reply, StatusCode::OK);
    let model = id(&created["id"]);
    assert!(created.get("projection").is_none());
    assert_eq!(created["model_name"], "manual");
    let update = scene.key("ProviderModelUpdate", &["manage", "read"]).await;
    let reply = scene
        .world
        .app
        .call(ProviderModelUpdate, &[&scene.provider, &model])
        .token(&update.secret)
        .raw_body(&json!({"display_name":"After"}))
        .send_bytes()
        .await;
    let changed = body(&reply, StatusCode::OK);
    assert!(changed.get("projection").is_none());
    assert_eq!(changed["id"], json!(model));
    assert_eq!(changed["display_name"], "After");
    let accept = scene
        .key("ProviderModelSuggestionAccept", &["manage", "read"])
        .await;
    let reply = discover(
        &scene,
        &accept,
        DiscoveryRequest::Accept,
        &["scope-private-offer"],
        suggestions(),
        Withdrawal::None,
    )
    .await;
    let accepted = body(&reply, StatusCode::OK);
    assert!(accepted.get("projection").is_none());
    assert_eq!(accepted["added"].as_array().unwrap().len(), 1);
    assert_eq!(accepted["provider"]["id"], json!(scene.provider));
    assert_eq!(accepted["provider"]["models"].as_array().unwrap().len(), 2);
    let added = &accepted["added"][0];
    assert_eq!(added["enabled"], true);
    assert_eq!(added["parameters"]["offered"]["context_max"], 8192);
    assert_eq!(
        added["parameters"]["offered"]["access_programs"],
        json!(["preview"])
    );
    assert_eq!(
        added["parameters"]["offered"]["service_tiers"],
        json!([{"id":"fast","label":"Fast"}])
    );
    for member in [
        "service_tiers",
        "default_service_tier",
        "access_programs",
        "default_access_program",
        "context_max",
        "default_context",
    ] {
        assert!(
            added["parameters"].get(member).is_none_or(Json::is_null),
            "discovery cannot enable the offered {member}"
        );
    }
}

#[tokio::test]
async fn model_list_and_empty_delete_keep_their_existing_key_boundary() {
    let scene = Scene::new().await;
    let local = scene.add(scene.provider, "local", "Local").await;
    let foreign = scene.add(scene.sibling, "foreign", "Foreign").await;
    let sibling = scene.models(scene.sibling).await;
    let read = scene.key("ProviderModelList", &["read"]).await;
    let reply = scene
        .world
        .app
        .call(ProviderModelList, &[&scene.provider])
        .token(&read.secret)
        .send_bytes()
        .await;
    let listed = body(&reply, StatusCode::OK);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], json!(local));
    let outside = scene
        .world
        .app
        .call(ProviderModelList, &[&scene.sibling])
        .token(&read.secret)
        .send_bytes()
        .await;
    assert_eq!(outside.status, StatusCode::FORBIDDEN);
    let delete = scene.key("ProviderModelDelete", &["manage"]).await;
    let mismatch = scene
        .world
        .app
        .call(ProviderModelDelete, &[&scene.provider, &foreign])
        .token(&delete.secret)
        .send_bytes()
        .await;
    assert_eq!(mismatch.status, StatusCode::NOT_FOUND);
    let outside = scene
        .world
        .app
        .call(ProviderModelDelete, &[&scene.sibling, &foreign])
        .token(&delete.secret)
        .send_bytes()
        .await;
    assert_eq!(outside.status, StatusCode::FORBIDDEN);
    let update = scene.key("ProviderModelUpdate", &["manage", "read"]).await;
    let mismatch = scene
        .world
        .app
        .call(ProviderModelUpdate, &[&scene.provider, &foreign])
        .token(&update.secret)
        .raw_body(&json!({"display_name":"Must not change"}))
        .send_bytes()
        .await;
    assert_eq!(mismatch.status, StatusCode::NOT_FOUND);
    assert_eq!(scene.models(scene.sibling).await, sibling);
    let deleted = scene
        .world
        .app
        .call(ProviderModelDelete, &[&scene.provider, &local])
        .token(&delete.secret)
        .send_bytes()
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(deleted.raw.is_empty());
    assert!(scene.models(scene.provider).await.is_empty());
    assert_eq!(scene.models(scene.sibling).await, sibling);
}

#[tokio::test]
async fn model_keys_do_not_add_ordinary_rights_or_skip_validation_and_collision_rules() {
    let scene = Scene::new().await;
    let first = scene.add(scene.provider, "alpha", "One").await;
    let second = scene.add(scene.provider, "beta", "Two").await;
    let before = scene.models(scene.provider).await;
    let powerless = scene
        .key_for(
            scene.people.stranger.id(),
            scene.provider,
            json!([
                {"operation":"ProviderModelCreate","actions":["manage","read"]},
                {"operation":"ProviderModelList","actions":["read"]}
            ]),
        )
        .await;
    let refused = scene
        .world
        .app
        .call(ProviderModelCreate, &[&scene.provider])
        .token(&powerless.secret)
        .raw_body(&json!({"model_name":"unowned"}))
        .send_bytes()
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let refused = scene
        .world
        .app
        .call(ProviderModelList, &[&scene.provider])
        .token(&powerless.secret)
        .send_bytes()
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let create = scene.key("ProviderModelCreate", &["manage", "read"]).await;
    let invalid = scene
        .world
        .app
        .call(ProviderModelCreate, &[&scene.provider])
        .token(&create.secret)
        .raw_body(&json!({"model_name":"invalid",
            "parameters":{"efforts":["low"],"default_effort":"high"}}))
        .send_bytes()
        .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    let duplicate = scene
        .world
        .app
        .call(ProviderModelCreate, &[&scene.provider])
        .token(&create.secret)
        .raw_body(&json!({"model_name":"alpha"}))
        .send_bytes()
        .await;
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
    let update = scene.key("ProviderModelUpdate", &["manage", "read"]).await;
    let clash = scene
        .world
        .app
        .call(ProviderModelUpdate, &[&scene.provider, &second])
        .token(&update.secret)
        .raw_body(&json!({"display_name":"One"}))
        .send_bytes()
        .await;
    assert_eq!(clash.status, StatusCode::CONFLICT);
    let wrong_operation = scene
        .world
        .app
        .call(ProviderModelCreate, &[&scene.provider])
        .token(&update.secret)
        .raw_body(&json!({"model_name":"wrong-operation"}))
        .send_bytes()
        .await;
    assert_eq!(wrong_operation.status, StatusCode::FORBIDDEN);
    let outside = scene
        .world
        .app
        .call(ProviderModelCreate, &[&scene.sibling])
        .token(&create.secret)
        .raw_body(&json!({"model_name":"outside"}))
        .send_bytes()
        .await;
    assert_eq!(outside.status, StatusCode::FORBIDDEN);
    assert_eq!(scene.models(scene.provider).await, before);
    assert!(scene.models(scene.sibling).await.is_empty());
    assert_ne!(first, second);
}

#[tokio::test]
async fn suggestion_accept_keeps_collision_rollback_for_a_fully_admitted_key() {
    let scene = Scene::new().await;
    let key = scene
        .key("ProviderModelSuggestionAccept", &["manage", "read"])
        .await;
    let report = json!({"suggestions":[
        {"model_name":"one","display_name":"Same name","listed":true,"parameters":{}},
        {"model_name":"two","display_name":"Same name","listed":true,"parameters":{}}
    ]});
    let reply = discover(
        &scene,
        &key,
        DiscoveryRequest::Accept,
        &["one", "two"],
        report,
        Withdrawal::None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(scene.models(scene.provider).await.is_empty());
}

#[tokio::test]
async fn suggestion_list_keeps_its_same_path_manage_only_positive() {
    let scene = Scene::new().await;
    let key = scene.key("ProviderModelSuggestionList", &["manage"]).await;
    let reply = discover(
        &scene,
        &key,
        DiscoveryRequest::List,
        &[],
        suggestions(),
        Withdrawal::None,
    )
    .await;
    let listed = body(&reply, StatusCode::OK);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["model_name"], "scope-private-offer");
    assert!(scene.models(scene.provider).await.is_empty());
}

async fn withdrawn(request: DiscoveryRequest, withdrawal: Withdrawal) {
    let scene = Scene::new().await;
    let key = scene.key(request.operation(), &["manage", "read"]).await;
    let before = scene.models(scene.provider).await;
    let reply = discover(
        &scene,
        &key,
        request,
        &["scope-private-offer"],
        suggestions(),
        withdrawal,
    )
    .await;
    assert_eq!(
        reply.status,
        match withdrawal {
            Withdrawal::Credential => StatusCode::UNAUTHORIZED,
            Withdrawal::Scope | Withdrawal::Manage => StatusCode::FORBIDDEN,
            Withdrawal::None => unreachable!("this control withdraws actual authority"),
        },
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(!String::from_utf8_lossy(&reply.raw).contains("scope-private-offer"));
    assert_eq!(
        scene.models(scene.provider).await,
        before,
        "the guarded acceptance must not edit the allowlist after its authority is withdrawn"
    );
}

#[tokio::test]
async fn suggestion_list_withholds_content_after_scope_changes_during_discovery() {
    withdrawn(DiscoveryRequest::List, Withdrawal::Scope).await;
}

#[tokio::test]
async fn suggestion_list_withholds_content_after_key_revocation_during_discovery() {
    withdrawn(DiscoveryRequest::List, Withdrawal::Credential).await;
}

#[tokio::test]
async fn suggestion_list_withholds_content_after_manage_is_removed_during_discovery() {
    withdrawn(DiscoveryRequest::List, Withdrawal::Manage).await;
}

#[tokio::test]
async fn suggestion_accept_rechecks_existing_authority_after_discovery_before_writing() {
    for withdrawal in [
        Withdrawal::Scope,
        Withdrawal::Credential,
        Withdrawal::Manage,
    ] {
        withdrawn(DiscoveryRequest::Accept, withdrawal).await;
    }
}
