//! Invitations retain their native governance, giving and redemption under scoped credentials.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod chat_world;
mod places;
mod support;

#[path = "invitation_operation_scopes/continuations.rs"]
mod continuations;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::Role;
use orch_api_contract::users::{
    InvitationAccept, InvitationAcceptBody, InvitationCreate, InvitationCreateBody,
    InvitationDelete, InvitationKind, InvitationList, InvitationListQuery, InvitationLookup,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use places::Who;
use serde_json::json;
use support::World;

const CREATE: &str = "InvitationCreate";
const LIST: &str = "InvitationList";
const DELETE: &str = "InvitationDelete";
const ACCEPT: &str = "InvitationAccept";
const NOTE: &str = "Literal invitation note";

#[derive(Clone, Copy, Debug)]
enum Destination {
    Org,
    Node,
    Chat,
    System,
}
struct Scene {
    room: chat_world::Room,
    admin: Who,
    group: Id,
}
impl Scene {
    async fn new() -> Self {
        let room = chat_world::Room::new().await;
        let admin = places::admin(&room.world, "systemadmin").await;
        let group = room
            .group(&room.ada, &[&room.bea], Some("Invitation group"))
            .await
            .base
            .id;
        Self { room, admin, group }
    }
    fn world(&self) -> &World {
        &self.room.world
    }
    fn issuer(&self, destination: Destination) -> &Who {
        match destination {
            Destination::System => &self.admin,
            _ => &self.room.ada,
        }
    }
    fn target(&self, destination: Destination) -> (&'static str, Option<Id>) {
        match destination {
            Destination::Org => ("org", Some(self.room.root)),
            Destination::Node => ("project", Some(self.room.project)),
            Destination::Chat => ("chat", Some(self.group)),
            Destination::System => ("system", None),
        }
    }
    fn body(&self, destination: Destination) -> InvitationCreateBody {
        InvitationCreateBody {
            kind: match destination {
                Destination::Org => InvitationKind::Org,
                Destination::Node => InvitationKind::Node,
                Destination::Chat => InvitationKind::Chat,
                Destination::System => InvitationKind::System,
            },
            org_id: matches!(destination, Destination::Org).then_some(self.room.root),
            node_id: matches!(destination, Destination::Node).then_some(self.room.project),
            chat_id: matches!(destination, Destination::Chat).then_some(self.group),
            role: matches!(destination, Destination::Org | Destination::Node)
                .then_some(Role::Viewer),
            permissions: None,
            email: None,
            max_uses: Some(3),
            expires_in_minutes: Some(60),
            note: Some(NOTE.into()),
        }
    }
    fn rules(
        &self,
        destination: Destination,
        operation: &str,
        action: &str,
        full: bool,
    ) -> Vec<Json> {
        let (kind, id) = self.target(destination);
        let actions = if full && action != "read" {
            vec![action, "read"]
        } else {
            vec![action]
        };
        let mut rules = vec![rule(kind, id, "invitation", operation, &actions)];
        if full && !matches!(destination, Destination::System) {
            if let Some(id) = id {
                rules.push(rule(kind, Some(id), kind, operation, &["read"]));
            }
            if !matches!(destination, Destination::Org) {
                rules.push(rule(
                    "org",
                    Some(self.room.root),
                    "org",
                    operation,
                    &["read"],
                ));
            }
        }
        rules
    }
}
fn rule(kind: &str, id: Option<Id>, resource: &str, operation: &str, actions: &[&str]) -> Json {
    let mut target = json!({"kind":kind,"reach":"self"});
    if let Some(id) = id {
        target["id"] = json!(id);
    }
    json!({"target":target,"resources":[resource],"allow":[{"operation":operation,"actions":actions}]})
}
struct Key {
    id: Id,
    secret: String,
}
async fn key(world: &World, owner: Id, rules: Vec<Json>) -> Key {
    let policy: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(policy.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: owner,
            name: "Invitation operation control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(policy).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn legacy(world: &World, owner: Id, node: Id) -> Key {
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: owner,
            name: "Legacy invitation issuer",
            token_hash: &token_hash(&secret),
            scopes: &["*".into()],
            node_id: Some(node),
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn create(world: &World, token: &str, body: &InvitationCreateBody) -> RawReply {
    world
        .app
        .call(InvitationCreate, &[])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
async fn accept(world: &World, token: &str, offer: &str) -> RawReply {
    world
        .app
        .call(InvitationAccept, &[])
        .token(token)
        .body(&InvitationAcceptBody {
            accepted_terms_hash: None,
            token: offer.into(),
        })
        .send_bytes()
        .await
}
async fn list(world: &World, token: &str, org: Option<Id>) -> RawReply {
    world
        .app
        .call(InvitationList, &[])
        .token(token)
        .query(&InvitationListQuery { org_id: org })
        .send_bytes()
        .await
}
async fn delete(world: &World, token: &str, offer: Id) -> RawReply {
    world
        .app
        .call(InvitationDelete, &[&offer])
        .token(token)
        .send_bytes()
        .await
}
fn json_reply(reply: &RawReply) -> Json {
    serde_json::from_slice(&reply.raw).unwrap()
}
fn checked(reply: RawReply, expected: StatusCode) -> Json {
    let body = if reply.raw.is_empty() {
        Json::Null
    } else {
        json_reply(&reply)
    };
    // Invitation creation contains a generated bearer secret: never dump its
    // successful payload merely to diagnose an assertion failure.
    assert_eq!(
        reply.status, expected,
        "code={}, required={}",
        body["code"], body["required"]
    );
    body
}
struct Offer {
    id: Id,
    token: String,
}
fn full_created(body: &Json) -> Offer {
    assert!(
        body["invitation"].is_object(),
        "expected the ordinary Full creation projection"
    );
    Offer {
        id: serde_json::from_value(body["invitation"]["id"].clone()).unwrap(),
        token: body["token"].as_str().unwrap().into(),
    }
}
async fn browser_offer(s: &Scene, destination: Destination) -> Offer {
    let value = checked(
        create(
            s.world(),
            &s.issuer(destination).token,
            &s.body(destination),
        )
        .await,
        StatusCode::OK,
    );
    full_created(&value)
}
async fn uses(world: &World, invitation: Id) -> i32 {
    sqlx::query_scalar("SELECT uses FROM invitations WHERE id=$1")
        .bind(invitation)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn memberships(world: &World, root: Id, user: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM org_members WHERE org_id=$1 AND user_id=$2")
        .bind(root)
        .bind(user)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn granted(s: &Scene, destination: Destination, user: Id) {
    match destination {
        Destination::Org => assert_eq!(memberships(s.world(), s.room.root, user).await, 1),
        Destination::Node => {
            let roles: i64 = sqlx::query_scalar("SELECT count(*) FROM node_roles WHERE node_id=$1 AND subject_id=$2 AND removed_at IS NULL AND role='viewer'")
                .bind(s.room.project).bind(user).fetch_one(&s.world().db.pool).await.unwrap();
            assert_eq!(roles, 1);
            assert_eq!(memberships(s.world(), s.room.root, user).await, 0);
        }
        Destination::Chat => {
            let members: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_members WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL")
                .bind(s.group).bind(user).fetch_one(&s.world().db.pool).await.unwrap();
            assert_eq!(members, 1);
            assert_eq!(memberships(s.world(), s.room.root, user).await, 0);
        }
        Destination::System => {
            assert_eq!(memberships(s.world(), s.room.root, user).await, 0);
            let admin: bool = sqlx::query_scalar("SELECT is_system_admin FROM users WHERE id=$1")
                .bind(user)
                .fetch_one(&s.world().db.pool)
                .await
                .unwrap();
            assert!(!admin);
        }
    }
}

async fn create_full(destination: Destination) {
    let s = Scene::new().await;
    let native = browser_offer(&s, destination).await;
    assert_eq!(uses(s.world(), native.id).await, 0);
    let actor = s.issuer(destination);
    let caller = key(
        s.world(),
        actor.id(),
        s.rules(destination, CREATE, "manage", true),
    )
    .await;
    let value = checked(
        create(s.world(), &caller.secret, &s.body(destination)).await,
        StatusCode::OK,
    );
    let offer = full_created(&value);
    assert_eq!(value["invitation"]["note"], NOTE);
    assert!(!offer.token.is_empty());
    let mode: Option<bool> =
        sqlx::query_scalar("SELECT created_as_admin FROM invitations WHERE id=$1")
            .bind(offer.id)
            .fetch_one(&s.world().db.pool)
            .await
            .unwrap();
    assert_eq!(
        mode,
        Some(false),
        "keys never capture browser private administrator mode"
    );
    assert_eq!(uses(s.world(), offer.id).await, 0);
}
async fn list_full(destination: Destination) {
    let s = Scene::new().await;
    let offer = browser_offer(&s, destination).await;
    let actor = s.issuer(destination);
    let caller = key(
        s.world(),
        actor.id(),
        s.rules(destination, LIST, "read", true),
    )
    .await;
    let org = matches!(destination, Destination::Org).then_some(s.room.root);
    let ordinary = checked(list(s.world(), &actor.token, org).await, StatusCode::OK);
    assert!(
        ordinary
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == json!(offer.id))
    );
    let shown = checked(list(s.world(), &caller.secret, org).await, StatusCode::OK);
    assert_eq!(shown.as_array().unwrap().len(), 1);
    assert_eq!(shown[0]["id"], json!(offer.id));
    assert_eq!(shown[0]["note"], NOTE);
    assert!(
        !shown.to_string().contains(&offer.token),
        "a list never returns the bearer secret"
    );
}
async fn delete_offer(destination: Destination) {
    let s = Scene::new().await;
    let offer = browser_offer(&s, destination).await;
    let caller = key(
        s.world(),
        s.issuer(destination).id(),
        s.rules(destination, DELETE, "manage", false),
    )
    .await;
    checked(
        delete(s.world(), &caller.secret, offer.id).await,
        StatusCode::NO_CONTENT,
    );
    let revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM invitations WHERE id=$1")
            .bind(offer.id)
            .fetch_one(&s.world().db.pool)
            .await
            .unwrap();
    assert!(revoked);
    assert_eq!(uses(s.world(), offer.id).await, 0);
}
async fn accept_full(destination: Destination) {
    let s = Scene::new().await;
    let offer = browser_offer(&s, destination).await;
    let guest = &s.room.eve;
    let caller = key(
        s.world(),
        guest.id(),
        s.rules(destination, ACCEPT, "manage", true),
    )
    .await;
    let full = checked(
        accept(s.world(), &caller.secret, &offer.token).await,
        StatusCode::OK,
    );
    assert!(
        full.get("projection").is_none(),
        "the permitted ordinary projection remains Full"
    );
    match destination {
        Destination::Org => assert_eq!(full["org"]["id"], json!(s.room.root)),
        Destination::Node => assert_eq!(full["node"]["id"], json!(s.room.project)),
        Destination::Chat => assert_eq!(full["chat"]["id"], json!(s.group)),
        Destination::System => {
            assert!(full["org"].is_null() && full["node"].is_null() && full["chat"].is_null())
        }
    }
    assert_eq!(uses(s.world(), offer.id).await, 1);
    granted(&s, destination, guest.id()).await;
}

#[tokio::test]
async fn org_create_full_uses_the_actual_governing_org() {
    create_full(Destination::Org).await;
}
#[tokio::test]
async fn node_create_full_uses_the_actual_governing_node() {
    create_full(Destination::Node).await;
}
#[tokio::test]
async fn chat_create_full_uses_the_actual_governing_chat() {
    create_full(Destination::Chat).await;
}
#[tokio::test]
async fn system_create_full_is_business_admin_without_private_mode() {
    create_full(Destination::System).await;
}
#[tokio::test]
async fn org_list_full_reads_the_governed_offer() {
    list_full(Destination::Org).await;
}
#[tokio::test]
async fn node_list_full_reads_the_governed_offer() {
    list_full(Destination::Node).await;
}
#[tokio::test]
async fn chat_list_full_reads_the_governed_offer() {
    list_full(Destination::Chat).await;
}
#[tokio::test]
async fn system_list_full_reads_the_governed_offer() {
    list_full(Destination::System).await;
}

#[tokio::test]
async fn system_inventory_reads_other_issuers_with_business_standing_without_private_mode() {
    let s = Scene::new().await;
    let offer = browser_offer(&s, Destination::System).await;
    let other = places::admin(s.world(), "otheradmin").await;
    let ordinary = checked(list(s.world(), &other.token, None).await, StatusCode::OK);
    assert!(
        ordinary
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == json!(offer.id))
    );
    let caller = key(
        s.world(),
        other.id(),
        s.rules(Destination::System, LIST, "read", true),
    )
    .await;
    let shown = checked(list(s.world(), &caller.secret, None).await, StatusCode::OK);
    assert!(
        shown
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == json!(offer.id))
    );
    let unentitled = key(
        s.world(),
        s.room.eve.id(),
        s.rules(Destination::System, LIST, "read", true),
    )
    .await;
    let own_history = checked(
        list(s.world(), &unentitled.secret, None).await,
        StatusCode::OK,
    );
    assert!(
        own_history.as_array().unwrap().is_empty(),
        "scope supplies no system standing"
    );
}
#[tokio::test]
async fn org_delete_revokes_only_the_offer() {
    delete_offer(Destination::Org).await;
}
#[tokio::test]
async fn node_delete_revokes_only_the_offer() {
    delete_offer(Destination::Node).await;
}
#[tokio::test]
async fn chat_delete_revokes_only_the_offer() {
    delete_offer(Destination::Chat).await;
}
#[tokio::test]
async fn system_delete_revokes_only_the_offer() {
    delete_offer(Destination::System).await;
}
#[tokio::test]
async fn org_accept_uses_the_existing_recipient_and_membership_writer() {
    accept_full(Destination::Org).await;
}
#[tokio::test]
async fn node_accept_uses_the_existing_recipient_and_role_writer() {
    accept_full(Destination::Node).await;
}
#[tokio::test]
async fn chat_accept_uses_the_existing_recipient_and_member_writer() {
    accept_full(Destination::Chat).await;
}
#[tokio::test]
async fn system_accept_creates_no_account_role_or_administrator() {
    accept_full(Destination::System).await;
}

#[tokio::test]
async fn manage_only_creation_keeps_generated_capability_but_no_existing_destination_content() {
    let s = Scene::new().await;
    let caller = key(
        s.world(),
        s.room.ada.id(),
        s.rules(Destination::Org, CREATE, "manage", false),
    )
    .await;
    let receipt = checked(
        create(s.world(), &caller.secret, &s.body(Destination::Org)).await,
        StatusCode::OK,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert!(receipt.get("invitation").is_none());
    assert!(
        receipt["token"]
            .as_str()
            .is_some_and(|secret| !secret.is_empty())
    );
    let id: Id = serde_json::from_value(receipt["invitation_id"].clone()).unwrap();
    assert_eq!(uses(s.world(), id).await, 0);
    assert!(
        receipt.get("org").is_none()
            && receipt.get("node").is_none()
            && receipt.get("chat").is_none()
    );
}

#[tokio::test]
async fn manage_only_acceptance_commits_once_and_returns_a_truthful_receipt() {
    let s = Scene::new().await;
    let offer = browser_offer(&s, Destination::Org).await;
    let caller = key(
        s.world(),
        s.room.eve.id(),
        s.rules(Destination::Org, ACCEPT, "manage", false),
    )
    .await;
    let receipt = checked(
        accept(s.world(), &caller.secret, &offer.token).await,
        StatusCode::OK,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["invitation_id"], json!(offer.id));
    assert!(
        receipt.get("org").is_none()
            && receipt.get("node").is_none()
            && receipt.get("chat").is_none()
    );
    assert_eq!(uses(s.world(), offer.id).await, 1);
    granted(&s, Destination::Org, s.room.eve.id()).await;
}

#[tokio::test]
async fn governing_scope_is_not_creator_account_or_an_unimplemented_invitation_id_alias() {
    let s = Scene::new().await;
    let offer = browser_offer(&s, Destination::Org).await;
    for selection in [
        rule(
            "account",
            Some(s.room.ada.id()),
            "invitation",
            DELETE,
            &["manage"],
        ),
        rule(
            "invitation",
            Some(offer.id),
            "invitation",
            DELETE,
            &["manage"],
        ),
        rule(
            "org",
            Some(s.room.eve.person.personal_org),
            "invitation",
            DELETE,
            &["manage"],
        ),
    ] {
        let caller = key(s.world(), s.room.ada.id(), vec![selection]).await;
        checked(
            delete(s.world(), &caller.secret, offer.id).await,
            StatusCode::FORBIDDEN,
        );
    }
    let revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM invitations WHERE id=$1")
            .bind(offer.id)
            .fetch_one(&s.world().db.pool)
            .await
            .unwrap();
    assert!(!revoked);
    assert_eq!(uses(s.world(), offer.id).await, 0);
}

#[tokio::test]
async fn a_correct_scope_does_not_supply_the_native_inviter_right() {
    let s = Scene::new().await;
    let body = s.body(Destination::Org);
    let ordinary = checked(
        create(s.world(), &s.room.bea.token, &body).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(ordinary["required"], "members.manage");
    let caller = key(
        s.world(),
        s.room.bea.id(),
        s.rules(Destination::Org, CREATE, "manage", false),
    )
    .await;
    let denied = checked(
        create(s.world(), &caller.secret, &body).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(denied["code"], ordinary["code"]);
    assert_eq!(denied["required"], "members.manage");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invitations WHERE created_by=$1")
        .bind(s.room.bea.id())
        .fetch_one(&s.world().db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

async fn legacy_offer(s: &Scene) -> (Key, Offer) {
    let issuer = legacy(s.world(), s.room.ada.id(), s.room.project).await;
    let value = checked(
        create(s.world(), &issuer.secret, &s.body(Destination::Node)).await,
        StatusCode::OK,
    );
    (issuer, full_created(&value))
}
async fn revoke(s: &Scene, issuer: &Key) {
    assert!(
        identity::revoke_api_token(
            &mut *s.world().conn().await,
            issuer.id,
            s.room.ada.id(),
            s.world().now()
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn an_outstanding_legacy_key_offer_does_not_become_a_session_at_public_lookup() {
    let s = Scene::new().await;
    let (issuer, offer) = legacy_offer(&s).await;
    checked(
        s.world()
            .app
            .call(InvitationLookup, &[&offer.token])
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    revoke(&s, &issuer).await;
    let lost = checked(
        s.world()
            .app
            .call(InvitationLookup, &[&offer.token])
            .send_bytes()
            .await,
        StatusCode::GONE,
    );
    assert_eq!(lost["reason"], "invitation_issuer_authority_lost");
    assert_eq!(uses(s.world(), offer.id).await, 0);
}

#[tokio::test]
async fn issuer_key_revocation_stops_future_redemption_without_undoing_a_completed_grant() {
    let s = Scene::new().await;
    let (issuer, offer) = legacy_offer(&s).await;
    checked(
        accept(s.world(), &s.room.eve.token, &offer.token).await,
        StatusCode::OK,
    );
    assert_eq!(uses(s.world(), offer.id).await, 1);
    granted(&s, Destination::Node, s.room.eve.id()).await;
    revoke(&s, &issuer).await;
    let refused = checked(
        accept(s.world(), &s.room.fay.token, &offer.token).await,
        StatusCode::GONE,
    );
    assert_eq!(refused["reason"], "invitation_issuer_authority_lost");
    assert_eq!(uses(s.world(), offer.id).await, 1);
    granted(&s, Destination::Node, s.room.eve.id()).await;
    let other: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM node_roles WHERE node_id=$1 AND subject_id=$2 AND removed_at IS NULL",
    )
    .bind(s.room.project)
    .bind(s.room.fay.id())
    .fetch_one(&s.world().db.pool)
    .await
    .unwrap();
    assert_eq!(other, 0);
}

async fn creation_header(destination: Destination) {
    let s = Scene::new().await;
    let actor = s.issuer(destination);
    let input = s.body(destination);
    let writer = key(
        s.world(),
        actor.id(),
        s.rules(destination, CREATE, "manage", false),
    )
    .await;
    let act = new_id();
    let request = s
        .world()
        .app
        .call(InvitationCreate, &[])
        .token(&writer.secret)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let first = checked(request.send_bytes().await, StatusCode::OK);
    let id: Id = serde_json::from_value(first["invitation_id"].clone()).unwrap();
    let secret = first["token"]
        .as_str()
        .expect("the first native capability");
    let url = first["url"].as_str().expect("the first native link");
    assert!(!secret.is_empty());
    assert!(url.ends_with(secret));
    assert!(first.get("already_created").is_none());
    let before: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(actor.id())
    .bind(act.to_string())
    .fetch_one(&s.world().db.pool)
    .await
    .unwrap();
    assert_eq!(
        before.0["outcome"],
        json!({"kind":"invitation","id":id,"data":{"sent":false}})
    );
    assert!(!before.0.to_string().contains(secret));
    assert!(!before.0.to_string().contains(url));
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(writer.id)
        .bind(s.world().now())
        .execute(&s.world().db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE invitations SET note='Current native invitation note' WHERE id=$1")
        .bind(id)
        .execute(&s.world().db.pool)
        .await
        .unwrap();
    let reader = key(
        s.world(),
        actor.id(),
        s.rules(destination, CREATE, "read", true),
    )
    .await;
    let request = s
        .world()
        .app
        .call(InvitationCreate, &[])
        .token(&reader.secret)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let current = checked(request.send_bytes().await, StatusCode::OK);
    assert_eq!(current["invitation"]["id"], json!(id));
    assert_eq!(
        current["invitation"]["note"],
        "Current native invitation note"
    );
    assert_eq!(current["already_created"], true);
    assert!(current["token"].is_null());
    assert!(current["url"].is_null());
    let neighbour = key(
        s.world(),
        actor.id(),
        s.rules(destination, LIST, "read", true),
    )
    .await;
    let request = s
        .world()
        .app
        .call(InvitationCreate, &[])
        .token(&neighbour.secret)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let receipt = checked(request.send_bytes().await, StatusCode::OK);
    assert_eq!(
        receipt,
        json!({"projection":"receipt","invitation_id":id,"token":null,"url":null,"sent":false,"already_created":true})
    );
    let after: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(actor.id())
    .bind(act.to_string())
    .fetch_one(&s.world().db.pool)
    .await
    .unwrap();
    assert_eq!(after, before);
    let copies: i64 = sqlx::query_scalar("SELECT count(*) FROM invitations WHERE created_by=$1 AND note IN ($2,'Current native invitation note')")
        .bind(actor.id()).bind(NOTE).fetch_one(&s.world().db.pool).await.unwrap();
    assert_eq!(copies, 1);
}
#[tokio::test]
async fn an_org_invitation_header_keeps_one_capability_and_replays_current_content_without_its_secret()
 {
    creation_header(Destination::Org).await;
}
#[tokio::test]
async fn a_node_invitation_header_keeps_one_capability_and_replays_current_content_without_its_secret()
 {
    creation_header(Destination::Node).await;
}
#[tokio::test]
async fn a_chat_invitation_header_keeps_one_capability_and_replays_current_content_without_its_secret()
 {
    creation_header(Destination::Chat).await;
}
#[tokio::test]
async fn a_system_invitation_header_keeps_one_capability_and_replays_current_content_without_its_secret()
 {
    creation_header(Destination::System).await;
}
