//! Destination actions inherit the stored node's existing Settings Manage authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;

use async_trait::async_trait;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::notifications::{
    DestinationDelete, DestinationTest, DestinationUpdate, DestinationUpdateBody,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::notifications::channels::{self as rows, NewDestination, owner};
use orch_db::queries::settings;
use orch_db::secrets::MasterKey;
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery, LinkPoll};
use orch_notifier_contract::descriptor::NotifierDescriptor;
use orch_notifier_contract::notifier::Notifier;
use orch_types::{Id, Json, new_id};
use orch_worker_core::channels::{Notifiers, seal_secret, secret_binding};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::Duration;
use support::World;

// Same watchdog as the existing identity-guard rendezvous. Only an observed fake-notifier
// entry establishes a phase boundary; elapsed time never grants authority.
const PHASE_WAIT: Duration = Duration::from_secs(30);
const ORIGINAL_SECRET: &str = "fixture-original-private-secret";
const NEXT_SECRET: &str = "fixture-rotated-private-secret";
const CHANNEL: &str = "scope-destination";
const MASTER: [u8; 32] = [7; 32];
#[derive(Default)]
struct Gate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[derive(Default, Clone)]
struct Probe {
    checks: usize,
    sends: usize,
    accepted: usize,
    secret_seen: bool,
    check_gate: Option<Arc<Gate>>,
    send_gate: Option<Arc<Gate>>,
}
fn probes() -> &'static Mutex<HashMap<String, Probe>> {
    static PROBES: OnceLock<Mutex<HashMap<String, Probe>>> = OnceLock::new();
    PROBES.get_or_init(|| Mutex::new(HashMap::new()))
}
fn observed(url: &str) -> Probe {
    probes()
        .lock()
        .unwrap()
        .get(url)
        .cloned()
        .unwrap_or_default()
}
async fn held(gate: Option<Arc<Gate>>) {
    if let Some(gate) = gate {
        gate.entered.notify_one();
        gate.release.notified().await;
    }
}
struct ScopedNotifier;
#[async_trait]
impl Notifier for ScopedNotifier {
    fn describe(&self) -> NotifierDescriptor {
        let mut descriptor = orch_notifier_webhook::WebhookNotifier::new().describe();
        descriptor.name = CHANNEL.into();
        descriptor
    }
    async fn check(&self, config: &Config) -> Result<Json, DeliverError> {
        let key = config.text("url").unwrap_or_default().to_owned();
        let gate = {
            let mut all = probes().lock().unwrap();
            let state = all.entry(key).or_default();
            state.checks += 1;
            state.secret_seen |= config.secret("secret").is_some();
            state.check_gate.clone()
        };
        held(gate).await;
        Ok(Json::Null)
    }
    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        let key = delivery.config.text("url").unwrap_or_default().to_owned();
        let gate = {
            let mut all = probes().lock().unwrap();
            let state = all.entry(key.clone()).or_default();
            state.sends += 1;
            state.secret_seen |= delivery.config.secret("secret").is_some();
            state.send_gate.clone()
        };
        held(gate).await;
        probes().lock().unwrap().entry(key).or_default().accepted += 1;
        Ok(())
    }
    fn link_url(&self, _: &Config, _: &str) -> Option<String> {
        None
    }
    async fn poll_links(
        &self,
        _: &Config,
        _: Option<String>,
        _: Duration,
    ) -> Result<LinkPoll, DeliverError> {
        Ok(LinkPoll::default())
    }
}
fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let mut notifiers = Notifiers::new();
        notifiers.register(Arc::new(ScopedNotifier)).unwrap();
        assert!(orch_api_core::routes::notifications::install_notifiers(
            notifiers
        ));
        orch_api_core::routes::install_master_key(&MasterKey::new(MASTER));
    });
}
struct Key {
    id: Id,
    secret: String,
}
struct Scene {
    world: World,
    company: resources::Company,
    project: Id,
    topic: Id,
}
impl Scene {
    async fn new() -> Self {
        install();
        let world = World::new().await;
        let company = resources::company(&world).await;
        let project =
            places::make_project(&world, &company.boss, company.acme, "Notification project")
                .await
                .base
                .id;
        let topic = places::make_topic(&world, &company.boss, project, "Notification topic")
            .await
            .base
            .id;
        Self {
            world,
            company,
            project,
            topic,
        }
    }
    async fn destination(&self, node: Id) -> (Id, String) {
        let id = new_id();
        let url = format!("https://scope-{id}.invalid/hook");
        let mut conn = self.world.conn().await;
        rows::insert_destination(
            &mut conn,
            &NewDestination {
                id,
                node_id: node,
                channel: CHANNEL,
                name: "Stored private destination",
                config: &json!({"url":url}),
                templates: &json!({}),
                kinds: &[],
                level: "important",
                enabled: true,
                created_by: self.company.boss.id(),
                at: self.world.now(),
            },
        )
        .await
        .unwrap();
        let sealed = seal_secret(
            &MasterKey::new(MASTER),
            owner::DESTINATION,
            id,
            "secret",
            ORIGINAL_SECRET,
        )
        .unwrap();
        settings::put_secret(
            &mut conn,
            owner::DESTINATION,
            id,
            "secret",
            &sealed,
            Some(self.company.boss.id()),
            self.world.now(),
        )
        .await
        .unwrap();
        probes()
            .lock()
            .unwrap()
            .insert(url.clone(), Probe::default());
        (id, url)
    }
}
fn rule(operation: &str, kind: &str, node: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":node,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}
fn inside(operation: &str, node: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":"org","id":node,"reach":"subtree"},"allow":[{"operation":operation,"actions":actions}]})
}
async fn issue(s: &Scene, user: Id, rules: Option<Vec<Json>>) -> Key {
    let policy = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        json!(scope)
    });
    let scopes = if policy.is_none() {
        vec!["*".to_owned()]
    } else {
        Vec::new()
    };
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Destination scope control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: s.world.now(),
            expires_at: Some(s.world.now() + chrono::Duration::hours(1)),
        },
        policy.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn key(s: &Scene, user: Id, rules: Vec<Json>) -> Key {
    issue(s, user, Some(rules)).await
}
async fn update(s: &Scene, token: &str, id: Id, body: &DestinationUpdateBody) -> RawReply {
    s.world
        .app
        .call(DestinationUpdate, &[&id])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
async fn delete(s: &Scene, token: &str, id: Id) -> RawReply {
    s.world
        .app
        .call(DestinationDelete, &[&id])
        .token(token)
        .send_bytes()
        .await
}
async fn deliver(s: &Scene, token: &str, id: Id) -> RawReply {
    s.world
        .app
        .call(DestinationTest, &[&id])
        .token(token)
        .send_bytes()
        .await
}
fn shown(stage: &str, reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{stage}: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn denied(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
}
async fn plain_secret(s: &Scene, id: Id) -> Vec<u8> {
    let sealed: Vec<u8> = sqlx::query_scalar(
        "SELECT sealed FROM secrets WHERE owner_kind=$1 AND owner_id=$2 AND name='secret'",
    )
    .bind(owner::DESTINATION)
    .bind(id)
    .fetch_one(&s.world.db.pool)
    .await
    .unwrap();
    MasterKey::new(MASTER)
        .open(&sealed, &secret_binding(owner::DESTINATION, id, "secret"))
        .unwrap()
}
fn patch_body(url: &str) -> DestinationUpdateBody {
    DestinationUpdateBody {
        name: Some("Changed under this node".into()),
        config: Some(json!({"url":url})),
        secrets: Some(BTreeMap::from([("secret".into(), NEXT_SECRET.into())])),
        ..Default::default()
    }
}

#[tokio::test]
async fn update_uses_the_stored_node_and_preserves_full_config_with_exact_secret_recheck() {
    let s = Scene::new().await;
    let (ordinary_id, ordinary_url) = s.destination(s.topic).await;
    let ordinary = shown(
        "ordinary browser full Update",
        update(
            &s,
            &s.company.boss.token,
            ordinary_id,
            &patch_body(&ordinary_url),
        )
        .await,
    );
    assert_eq!(ordinary["node"]["id"], json!(s.topic));
    assert_eq!(ordinary["config"]["url"], ordinary_url);
    assert_eq!(ordinary["secrets_set"], json!(["secret"]));
    assert!(!ordinary.to_string().contains(NEXT_SECRET));
    assert_eq!(observed(&ordinary_url).checks, 1);
    assert!(observed(&ordinary_url).secret_seen);
    assert_eq!(plain_secret(&s, ordinary_id).await, NEXT_SECRET.as_bytes());
    let (legacy_id, legacy_url) = s.destination(s.topic).await;
    let legacy = issue(&s, s.company.boss.id(), None).await;
    let legacy_full = shown(
        "legacy full Update",
        update(&s, &legacy.secret, legacy_id, &patch_body(&legacy_url)).await,
    );
    assert_eq!(legacy_full["name"], ordinary["name"]);
    assert_eq!(observed(&legacy_url).checks, 1);
    let (id, url) = s.destination(s.topic).await;
    let token = key(
        &s,
        s.company.boss.id(),
        vec![rule(
            "DestinationUpdate",
            "topic",
            s.topic,
            &["manage", "read"],
        )],
    )
    .await;
    let full = shown(
        "modern full Update",
        update(&s, &token.secret, id, &patch_body(&url)).await,
    );
    assert_eq!(full["id"], json!(id));
    assert_eq!(full["node"]["id"], json!(s.topic));
    assert_eq!(full["config"]["url"], url);
    assert_eq!(full["secrets_set"], json!(["secret"]));
    assert!(!full.to_string().contains(NEXT_SECRET));
    assert_eq!(plain_secret(&s, id).await, NEXT_SECRET.as_bytes());
    assert_eq!(
        observed(&url).checks,
        1,
        "external notifier validation is not replayed"
    );
    assert_eq!(observed(&url).sends, 0);
}

#[tokio::test]
async fn manage_only_update_returns_an_accepted_receipt_without_existing_configuration() {
    let s = Scene::new().await;
    let (id, url) = s.destination(s.topic).await;
    let ordinary = shown(
        "ordinary Update baseline before receipt",
        update(
            &s,
            &s.company.boss.token,
            id,
            &DestinationUpdateBody::default(),
        )
        .await,
    );
    assert_eq!(ordinary["config"]["url"], url);
    assert_eq!(ordinary["secrets_set"], json!(["secret"]));
    let token = key(
        &s,
        s.company.boss.id(),
        vec![rule("DestinationUpdate", "topic", s.topic, &["manage"])],
    )
    .await;
    let receipt = shown(
        "modern Manage-only Update receipt",
        update(&s, &token.secret, id, &patch_body(&url)).await,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["destination_id"], json!(id));
    assert_eq!(receipt["node_id"], json!(s.topic));
    for field in ["config", "secrets_set", "templates", "node", "name"] {
        assert!(
            receipt.get(field).is_none(),
            "no Full field {field} is fabricated"
        );
    }
    assert!(!receipt.to_string().contains(&url) && !receipt.to_string().contains(NEXT_SECRET));
    assert_eq!(
        rows::destination(&mut *s.world.conn().await, id)
            .await
            .unwrap()
            .unwrap()
            .name,
        "Changed under this node"
    );
    assert_eq!(plain_secret(&s, id).await, NEXT_SECRET.as_bytes());
    assert_eq!(observed(&url).checks, 1);
    assert_eq!(observed(&url).sends, 0);
}

#[tokio::test]
async fn delete_uses_actual_node_management_and_removes_only_that_destination_and_its_secrets() {
    let s = Scene::new().await;
    for legacy in [false, true] {
        let (id, _) = s.destination(s.topic).await;
        let issued = if legacy {
            Some(issue(&s, s.company.boss.id(), None).await)
        } else {
            None
        };
        let response = delete(
            &s,
            issued
                .as_ref()
                .map_or(s.company.boss.token.as_str(), |key| key.secret.as_str()),
            id,
        )
        .await;
        assert_eq!(
            response.status,
            StatusCode::NO_CONTENT,
            "{} delete",
            if legacy {
                "legacy baseline"
            } else {
                "ordinary browser"
            }
        );
    }
    let (id, url) = s.destination(s.topic).await;
    let (kept, _) = s.destination(s.project).await;
    let token = key(
        &s,
        s.company.boss.id(),
        vec![rule("DestinationDelete", "topic", s.topic, &["manage"])],
    )
    .await;
    assert_eq!(
        delete(&s, &token.secret, id).await.status,
        StatusCode::NO_CONTENT,
        "modern exact-node Delete"
    );
    assert!(
        rows::destination(&mut *s.world.conn().await, id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        rows::destination(&mut *s.world.conn().await, kept)
            .await
            .unwrap()
            .is_some()
    );
    let secrets: i64 =
        sqlx::query_scalar("SELECT count(*) FROM secrets WHERE owner_kind=$1 AND owner_id=$2")
            .bind(owner::DESTINATION)
            .bind(id)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    assert_eq!(secrets, 0);
    assert_eq!(observed(&url).checks, 0);
    assert_eq!(observed(&url).sends, 0);
}

#[tokio::test]
async fn test_delivers_one_factual_sample_under_manage_without_reading_the_configuration() {
    let s = Scene::new().await;
    let (id, url) = s.destination(s.topic).await;
    let ordinary = shown(
        "ordinary browser Test",
        deliver(&s, &s.company.boss.token, id).await,
    );
    assert_eq!(ordinary, json!({"ok":true,"error":null}));
    let legacy = issue(&s, s.company.boss.id(), None).await;
    assert_eq!(
        shown("legacy Test", deliver(&s, &legacy.secret, id).await),
        ordinary
    );
    assert_eq!(observed(&url).accepted, 2);
    let token = key(
        &s,
        s.company.boss.id(),
        vec![inside("DestinationTest", s.company.acme, &["manage"])],
    )
    .await;
    let result = shown(
        "modern Manage-only Test",
        deliver(&s, &token.secret, id).await,
    );
    assert_eq!(result, ordinary);
    assert_eq!(observed(&url).sends, 3);
    assert_eq!(observed(&url).accepted, 3);
    assert!(observed(&url).secret_seen);
    assert_eq!(
        observed(&url).checks,
        0,
        "a delivery is not a duplicate pre-save validation"
    );
    assert!(!result.to_string().contains(&url) && !result.to_string().contains(ORIGINAL_SECRET));
    assert!(
        rows::destination(&mut *s.world.conn().await, id)
            .await
            .unwrap()
            .unwrap()
            .last_ok_at
            .is_some()
    );
}

#[derive(Clone, Copy)]
enum Act {
    Update,
    Delete,
    Test,
}
impl Act {
    fn name(self) -> &'static str {
        match self {
            Self::Update => "DestinationUpdate",
            Self::Delete => "DestinationDelete",
            Self::Test => "DestinationTest",
        }
    }
    async fn send(self, s: &Scene, token: &str, id: Id, url: &str) -> RawReply {
        match self {
            Self::Update => update(s, token, id, &patch_body(url)).await,
            Self::Delete => delete(s, token, id).await,
            Self::Test => deliver(s, token, id).await,
        }
    }
}
async fn refused_bounds(act: Act) {
    let s = Scene::new().await;
    let (id, url) = s.destination(s.topic).await;
    let ordinary = act.send(&s, &s.company.viewer.token, id, &url).await;
    denied(ordinary);
    let mut wrong_resource = rule(act.name(), "topic", s.topic, &["manage"]);
    wrong_resource["resources"] = json!(["provider"]);
    for rules in [
        Vec::new(),
        vec![rule(act.name(), "topic", s.topic, &["read"])],
        vec![rule(act.name(), "topic", id, &["manage"])],
        vec![rule("NodeGet", "topic", s.topic, &["read"])],
        vec![rule(act.name(), "project", s.project, &["manage"])],
        vec![wrong_resource],
    ] {
        let token = key(&s, s.company.boss.id(), rules).await;
        denied(act.send(&s, &token.secret, id, &url).await);
    }
    let outsider = key(
        &s,
        s.company.viewer.id(),
        vec![inside(act.name(), s.company.acme, &["manage", "read"])],
    )
    .await;
    denied(act.send(&s, &outsider.secret, id, &url).await);
    let row = rows::destination(&mut *s.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.name, "Stored private destination");
    assert!(row.last_ok_at.is_none());
    assert_eq!(plain_secret(&s, id).await, ORIGINAL_SECRET.as_bytes());
    assert_eq!(observed(&url).checks, 0);
    assert_eq!(observed(&url).sends, 0);
}
#[tokio::test]
async fn update_cannot_borrow_another_action_endpoint_resource_or_destination_url_id() {
    refused_bounds(Act::Update).await;
}
#[tokio::test]
async fn delete_cannot_borrow_another_action_endpoint_resource_or_destination_url_id() {
    refused_bounds(Act::Delete).await;
}
#[tokio::test]
async fn test_cannot_borrow_another_action_endpoint_resource_or_destination_url_id() {
    refused_bounds(Act::Test).await;
}

async fn validation_change(move_node: bool) {
    let s = Scene::new().await;
    let (id, url) = s.destination(s.topic).await;
    let token = key(
        &s,
        s.company.boss.id(),
        vec![rule(
            "DestinationUpdate",
            "topic",
            s.topic,
            &["manage", "read"],
        )],
    )
    .await;
    let gate = Arc::new(Gate::default());
    probes().lock().unwrap().get_mut(&url).unwrap().check_gate = Some(gate.clone());
    let body = patch_body(&url);
    let request = update(&s, &token.secret, id, &body);
    tokio::pin!(request);
    let reached = tokio::select! { _=gate.entered.notified()=>true, _=&mut request=>false, _=tokio::time::sleep(PHASE_WAIT)=>false };
    assert!(
        reached,
        "the existing validation phase must actually be entered; an early unbound refusal is not validation evidence"
    );
    if move_node {
        sqlx::query("UPDATE destinations SET node_id=$2 WHERE id=$1")
            .bind(id)
            .bind(s.project)
            .execute(&s.world.db.pool)
            .await
            .unwrap();
    } else {
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(token.id)
            .bind(json!(KeyScope::default()))
            .execute(&s.world.db.pool)
            .await
            .unwrap();
    }
    gate.release.notify_one();
    let result = tokio::time::timeout(PHASE_WAIT, request)
        .await
        .expect("released validation finishes");
    denied(result);
    let row = rows::destination(&mut *s.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.name, "Stored private destination");
    assert_eq!(plain_secret(&s, id).await, ORIGINAL_SECRET.as_bytes());
    assert_eq!(observed(&url).checks, 1);
    assert_eq!(observed(&url).sends, 0);
}
#[tokio::test]
async fn update_rechecks_current_key_after_one_external_validation() {
    validation_change(false).await;
}
#[tokio::test]
async fn update_rechecks_actual_stored_node_after_one_external_validation() {
    validation_change(true).await;
}

#[tokio::test]
async fn an_authorized_delivery_is_factual_but_scope_loss_prevents_status_writes_and_new_dispatch()
{
    let s = Scene::new().await;
    let (id, url) = s.destination(s.topic).await;
    let token = key(
        &s,
        s.company.boss.id(),
        vec![rule("DestinationTest", "topic", s.topic, &["manage"])],
    )
    .await;
    let gate = Arc::new(Gate::default());
    probes().lock().unwrap().get_mut(&url).unwrap().send_gate = Some(gate.clone());
    let request = deliver(&s, &token.secret, id);
    tokio::pin!(request);
    let reached = tokio::select! {_=gate.entered.notified()=>true,_=&mut request=>false,_=tokio::time::sleep(PHASE_WAIT)=>false};
    assert!(
        reached,
        "the currently authorized delivery must reach its actual receiver before scope changes"
    );
    assert_eq!(observed(&url).sends, 1);
    assert_eq!(observed(&url).accepted, 0);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token.id)
        .bind(json!(KeyScope::default()))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    gate.release.notify_one();
    denied(
        tokio::time::timeout(PHASE_WAIT, request)
            .await
            .expect("accepted delivery finishes"),
    );
    assert_eq!(
        observed(&url).accepted,
        1,
        "scope loss cannot unsend a previously authorized sample"
    );
    assert!(
        rows::destination(&mut *s.world.conn().await, id)
            .await
            .unwrap()
            .unwrap()
            .last_ok_at
            .is_none(),
        "fresh guard denies status persistence"
    );
    denied(deliver(&s, &token.secret, id).await);
    assert_eq!(
        observed(&url).sends,
        1,
        "a currently denied scope causes no new dispatch"
    );
}
