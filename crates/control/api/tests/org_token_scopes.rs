//! Org inventory reads whole keys under their real owners; refusal changes only one org policy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod resources;
mod support;

use crate::support::FullPermissionPageReply;
use http::StatusCode;
use orch_access_contract::key_scope::{KeyPlaceExclusion, KeyScope};
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::access::{PageChange, PageChangeWhole, PermissionPageUpdate};
use orch_api_contract::auth::{MeGet, OrgTokenList, OrgTokenListQuery, OrgTokenRefuse};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{OrgServiceAccountCreate, OrgServiceAccountCreateBody};
use orch_api_contract::nodes::{DeletedInspectionQuery, NodeGet};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::{Caller, Principal};
use orch_db::queries::{access, identity};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const LIST: &str = "OrgTokenList";
const REFUSE: &str = "OrgTokenRefuse";

struct Key {
    id: Id,
    secret: String,
}
fn rule(
    kind: &str,
    id: Id,
    reach: &str,
    resource: &str,
    operation: &str,
    actions: &[&str],
) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":[resource],
        "allow":[{"operation":operation,"actions":actions}]})
}
fn governance(org: Id, operation: &str) -> Json {
    rule("org", org, "self", "key", operation, &["manage"])
}
fn read_key(id: Id) -> Json {
    rule("key", id, "self", "key", LIST, &["read"])
}
async fn issue(
    world: &World,
    owner: Id,
    name: &str,
    node: Option<Id>,
    scope: Option<KeyScope>,
) -> Key {
    if let Some(scope) = &scope {
        assert!(scope.validate().is_empty());
        assert!(node.is_none());
    }
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    let scopes = if scope.is_some() {
        Vec::new()
    } else {
        vec!["*".to_owned()]
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: owner,
            name,
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: node,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::days(1)),
        },
        scope
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .unwrap()
            .as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn scoped(world: &World, owner: Id, rules: Vec<Json>) -> Key {
    issue(
        world,
        owner,
        "Inventory caller",
        None,
        Some(serde_json::from_value(json!({"rules":rules})).unwrap()),
    )
    .await
}
async fn list(
    world: &World,
    token: &str,
    org: Id,
    limit: Option<u32>,
    cursor: Option<String>,
) -> RawReply {
    world
        .app
        .call(OrgTokenList, &[&org])
        .token(token)
        .query(&OrgTokenListQuery { limit, cursor })
        .send_bytes()
        .await
}
async fn refuse(world: &World, token: &str, org: Id, target: Id) -> RawReply {
    world
        .app
        .call(OrgTokenRefuse, &[&org, &target])
        .token(token)
        .send_bytes()
        .await
}
fn value(reply: &RawReply) -> Json {
    serde_json::from_slice(&reply.raw).unwrap()
}
fn status(reply: RawReply, expected: StatusCode) {
    assert_eq!(
        reply.status,
        expected,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
}
fn page(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    value(&reply)
}
fn selected(page: &Json, id: Id) -> &Json {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(id))
        .unwrap_or_else(|| panic!("key {id} missing from {page}"))
}
async fn no_native_work(world: &World) {
    let count: i64 =
        sqlx::query_scalar("SELECT (SELECT count(*) FROM jobs)+(SELECT count(*) FROM commands)")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "token inventory/refusal queues no native work");
}
async fn refusal_count(world: &World, org: Id, key: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM api_token_refusals WHERE org_id=$1 AND token_id=$2")
        .bind(org)
        .bind(key)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

struct Scene {
    company: resources::Company,
    management: Id,
    first: Key,
    second: Key,
}
impl Scene {
    async fn new(world: &World) -> Self {
        let company = resources::company(world).await;
        let management = access::testing::add_entry(
            &mut *world.conn().await,
            &access::PageKey::new("node", company.acme, "live"),
            "any",
            "members.manage",
            ("user", Some(company.viewer.id()), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
        let first = issue(world, company.boss.id(), "Selected key", None, None).await;
        let second = issue(world, company.admin.id(), "Unread key sentinel", None, None).await;
        Self {
            company,
            management,
            first,
            second,
        }
    }
}

#[tokio::test]
async fn org_inventory_needs_governance_and_the_actual_keys_read_without_exposing_other_keys() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let caller = scoped(
        &world,
        s.company.viewer.id(),
        vec![governance(s.company.acme, LIST), read_key(s.first.id)],
    )
    .await;
    let ordinary = page(list(&world, &s.company.viewer.token, s.company.acme, None, None).await);
    let legacy = issue(
        &world,
        s.company.viewer.id(),
        "Legacy inspector",
        None,
        None,
    )
    .await;
    let legacy_page = page(list(&world, &legacy.secret, s.company.acme, None, None).await);
    assert_eq!(
        selected(&legacy_page, s.first.id),
        selected(&ordinary, s.first.id)
    );
    let limited = page(list(&world, &caller.secret, s.company.acme, None, None).await);
    assert_eq!(limited["items"].as_array().unwrap().len(), 1);
    assert_eq!(&limited["items"][0], selected(&ordinary, s.first.id));
    assert!(limited["next_cursor"].is_null());
    assert!(!limited.to_string().contains("Unread key sentinel"));
    assert!(!limited.to_string().contains(&s.first.secret));
    assert!(!limited.to_string().contains(&s.second.secret));
    let no_key_read = scoped(
        &world,
        s.company.viewer.id(),
        vec![governance(s.company.acme, LIST)],
    )
    .await;
    assert_eq!(
        page(list(&world, &no_key_read.secret, s.company.acme, None, None).await),
        json!({"items":[],"next_cursor":null})
    );
    let mut policy: KeyScope = serde_json::from_value(json!({"rules":[rule(
        "org",s.company.stranger.person.personal_org,"subtree","org","NodeGet",&["read"]
    )]}))
    .unwrap();
    policy.constraints.push(policy.rules.clone());
    policy.excluded_places.push(KeyPlaceExclusion::Refused {
        place: PlaceRef::Node(s.company.stranger.person.personal_org),
    });
    let configured = issue(
        &world,
        s.company.boss.id(),
        "Literal rule references",
        None,
        Some(policy),
    )
    .await;
    let inspector = scoped(
        &world,
        s.company.viewer.id(),
        vec![governance(s.company.acme, LIST), read_key(configured.id)],
    )
    .await;
    let ordinary = page(list(&world, &s.company.viewer.token, s.company.acme, None, None).await);
    let shown = page(list(&world, &inspector.secret, s.company.acme, None, None).await);
    assert_eq!(&shown["items"][0], selected(&ordinary, configured.id));
    assert!(
        !shown["items"][0]["constraints"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !shown["items"][0]["excluded_places"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    no_native_work(&world).await;
}

#[tokio::test]
async fn key_owners_are_real_accounts_and_only_service_accounts_have_org_placement() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let service = world
        .app
        .call(OrgServiceAccountCreate, &[&s.company.acme])
        .token(&s.company.boss.token)
        .body(&OrgServiceAccountCreateBody {
            username: "robot".into(),
            display_name: "Robot".into(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    let robot = issue(&world, service.user.id, "Service key", None, None).await;
    let by_org = scoped(
        &world,
        s.company.viewer.id(),
        vec![
            governance(s.company.acme, LIST),
            rule("org", s.company.acme, "subtree", "key", LIST, &["read"]),
        ],
    )
    .await;
    let rows = page(list(&world, &by_org.secret, s.company.acme, None, None).await);
    assert_eq!(rows["items"].as_array().unwrap().len(), 1);
    assert_eq!(rows["items"][0]["id"], json!(robot.id));
    assert_eq!(rows["items"][0]["service"], true);
    let account_self = scoped(
        &world,
        s.company.viewer.id(),
        vec![
            governance(s.company.acme, LIST),
            rule(
                "account",
                s.company.boss.id(),
                "self",
                "key",
                LIST,
                &["read"],
            ),
        ],
    )
    .await;
    assert!(
        page(list(&world, &account_self.secret, s.company.acme, None, None).await)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let account_subtree = scoped(
        &world,
        s.company.viewer.id(),
        vec![
            governance(s.company.acme, LIST),
            rule(
                "account",
                s.company.boss.id(),
                "subtree",
                "key",
                LIST,
                &["read"],
            ),
        ],
    )
    .await;
    let rows = page(list(&world, &account_subtree.secret, s.company.acme, None, None).await);
    assert_eq!(rows["items"].as_array().unwrap().len(), 1);
    assert_eq!(rows["items"][0]["id"], json!(s.first.id));
    no_native_work(&world).await;
}

#[tokio::test]
async fn scoped_inventory_pages_skip_unread_candidates_and_report_real_exhaustion() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let mut candidates = vec![s.first.id, s.second.id];
    for index in 0..4 {
        candidates.push(
            issue(
                &world,
                s.company.boss.id(),
                &format!("Candidate {index}"),
                None,
                None,
            )
            .await
            .id,
        );
    }
    candidates.sort_unstable();
    let permitted = &candidates[candidates.len() - 2..];
    let caller = scoped(
        &world,
        s.company.viewer.id(),
        vec![
            governance(s.company.acme, LIST),
            read_key(permitted[0]),
            read_key(permitted[1]),
        ],
    )
    .await;
    let first = page(list(&world, &caller.secret, s.company.acme, Some(1), None).await);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["id"], json!(permitted[0]));
    assert_eq!(first["next_cursor"], permitted[0].to_string());
    let second = page(
        list(
            &world,
            &caller.secret,
            s.company.acme,
            Some(1),
            first["next_cursor"].as_str().map(str::to_owned),
        )
        .await,
    );
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["id"], json!(permitted[1]));
    assert!(second["next_cursor"].is_null());
    let empty = page(
        list(
            &world,
            &caller.secret,
            s.company.acme,
            Some(1),
            Some(permitted[1].to_string()),
        )
        .await,
    );
    assert_eq!(empty, json!({"items":[],"next_cursor":null}));
    no_native_work(&world).await;
}

#[tokio::test]
async fn restriction_labels_follow_existing_inventory_entitlement_and_this_operations_read() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let private = places::make_project(
        &world,
        &s.company.boss,
        s.company.acme,
        "Private constraint",
    )
    .await
    .base
    .id;
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &private])
        .token(&s.company.boss.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .full_permission_page()
        .ok();
    let subject = issue(
        &world,
        s.company.boss.id(),
        "Private constraint is a literal key name",
        Some(private),
        None,
    )
    .await;
    let mut conn = world.conn().await;
    let principal = Principal::load(
        &mut conn,
        Caller::session(s.company.viewer.id()),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    assert!(
        !orch_authz::nodes::check_named(
            &mut conn,
            &principal,
            private,
            orch_api_core::middleware::authorization::VIEW,
            orch_authz::Act::Use,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);
    let ordinary = page(list(&world, &s.company.viewer.token, s.company.acme, None, None).await);
    assert_eq!(
        selected(&ordinary, subject.id)["node"]["name"],
        "Private constraint"
    );
    // Existing inventory management exposes this restriction label without
    // granting Node View. A new scope binder must not add a universal View gate.
    let without = scoped(
        &world,
        s.company.viewer.id(),
        vec![
            governance(s.company.acme, LIST),
            read_key(subject.id),
            rule(
                "project",
                private,
                "self",
                "project",
                "NodeGet.read",
                &["read"],
            ),
        ],
    )
    .await;
    let hidden = page(list(&world, &without.secret, s.company.acme, None, None).await);
    assert_eq!(hidden["items"][0]["node"]["id"], json!(private));
    assert!(hidden["items"][0]["node"]["name"].is_null());
    assert_eq!(hidden["items"][0]["node"]["redacted"], true);
    assert_eq!(
        hidden["items"][0]["name"],
        "Private constraint is a literal key name"
    );
    let complete = scoped(
        &world,
        s.company.viewer.id(),
        vec![
            governance(s.company.acme, LIST),
            read_key(subject.id),
            rule("project", private, "self", "project", LIST, &["read"]),
        ],
    )
    .await;
    let full = page(list(&world, &complete.secret, s.company.acme, None, None).await);
    assert_eq!(&full["items"][0], selected(&ordinary, subject.id));
    no_native_work(&world).await;
}

#[tokio::test]
async fn filtered_inventory_empty_is_not_another_operation_action_or_org_permission() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    for bad in [
        governance(s.company.acme, REFUSE),
        rule("org", s.company.acme, "self", "key", LIST, &["read"]),
        rule("org", s.company.acme, "self", "org", LIST, &["manage"]),
    ] {
        let caller = scoped(&world, s.company.viewer.id(), vec![bad]).await;
        status(
            list(&world, &caller.secret, s.company.acme, None, None).await,
            StatusCode::FORBIDDEN,
        );
    }
    let no_ordinary_right = scoped(
        &world,
        s.company.stranger.id(),
        vec![governance(s.company.acme, LIST), read_key(s.first.id)],
    )
    .await;
    let denied = list(
        &world,
        &no_ordinary_right.secret,
        s.company.acme,
        None,
        None,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let problem = value(&denied);
    assert_eq!(problem["code"], json!(ErrorCode::AuthzDenied));
    assert_eq!(problem["required"], "members.manage", "{problem}");
    no_native_work(&world).await;
}

#[tokio::test]
async fn a_real_empty_org_inventory_still_has_its_exact_governance_scope() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let owner = places::person(&world, "empty-owner").await;
    let empty = places::make_org(&world, &owner, "Empty", "empty")
        .await
        .base
        .id;
    access::testing::add_entry(
        &mut *world.conn().await,
        &access::PageKey::new("node", empty, "live"),
        "any",
        "members.manage",
        ("user", Some(s.company.viewer.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    // The existing candidate query uses membership/roles/service custody, not
    // arbitrary permission entries. This direct manager is not a stored candidate.
    assert!(
        orch_db::queries::tokens::reaching(&mut *world.conn().await, empty, world.now(), None, 1)
            .await
            .unwrap()
            .is_empty()
    );
    let expected = page(list(&world, &s.company.viewer.token, empty, None, None).await);
    assert_eq!(expected, json!({"items":[],"next_cursor":null}));
    let caller = scoped(&world, s.company.viewer.id(), vec![governance(empty, LIST)]).await;
    assert_eq!(
        page(list(&world, &caller.secret, empty, None, None).await),
        expected
    );
    let wrong = scoped(
        &world,
        s.company.viewer.id(),
        vec![governance(empty, REFUSE)],
    )
    .await;
    status(
        list(&world, &wrong.secret, empty, None, None).await,
        StatusCode::FORBIDDEN,
    );
    no_native_work(&world).await;
}

#[tokio::test]
async fn refusal_is_one_current_org_pair_and_does_not_globally_revoke_the_key() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let caller = scoped(
        &world,
        s.company.viewer.id(),
        vec![governance(s.company.acme, REFUSE)],
    )
    .await;
    status(
        refuse(&world, &caller.secret, s.company.acme, s.first.id).await,
        StatusCode::NO_CONTENT,
    );
    status(
        refuse(&world, &caller.secret, s.company.acme, s.first.id).await,
        StatusCode::NO_CONTENT,
    );
    assert_eq!(refusal_count(&world, s.company.acme, s.first.id).await, 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='auth.token_refused' AND object_id=$1",
    )
    .bind(s.first.id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "a duplicate accepted refusal has no second effect/audit"
    );
    let revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM api_tokens WHERE id=$1")
            .bind(s.first.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(!revoked);
    let within = world
        .app
        .call(NodeGet, &[&s.company.acme])
        .query(&DeletedInspectionQuery::default())
        .token(&s.first.secret)
        .send_bytes()
        .await;
    assert_eq!(within.status, StatusCode::FORBIDDEN);
    assert_eq!(value(&within)["code"], "token.refused_here");
    status(
        world
            .app
            .call(MeGet, &[])
            .token(&s.first.secret)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let foreign = issue(&world, s.company.stranger.id(), "Foreign key", None, None).await;
    status(
        refuse(&world, &caller.secret, s.company.acme, foreign.id).await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(refusal_count(&world, s.company.acme, foreign.id).await, 0);
    no_native_work(&world).await;
}

#[tokio::test]
async fn self_refusal_keeps_the_accepted_empty_receipt_and_withdraws_only_that_org() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let caller = scoped(
        &world,
        s.company.viewer.id(),
        vec![
            governance(s.company.acme, REFUSE),
            rule(
                "org",
                s.company.acme,
                "self",
                "org",
                "NodeGet.read",
                &["read"],
            ),
            rule(
                "account",
                s.company.viewer.id(),
                "self",
                "account",
                "MeGet",
                &["read"],
            ),
        ],
    )
    .await;
    // Prove that this exact canonical probe is admitted before the only change:
    // accepting this key's refusal in this org.
    status(
        world
            .app
            .call(NodeGet, &[&s.company.acme])
            .query(&DeletedInspectionQuery::default())
            .token(&caller.secret)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let accepted = refuse(&world, &caller.secret, s.company.acme, caller.id).await;
    assert_eq!(accepted.status, StatusCode::NO_CONTENT);
    assert!(accepted.raw.is_empty());
    assert_eq!(refusal_count(&world, s.company.acme, caller.id).await, 1);
    let globally_revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM api_tokens WHERE id=$1")
            .bind(caller.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(!globally_revoked);
    let within = world
        .app
        .call(NodeGet, &[&s.company.acme])
        .query(&DeletedInspectionQuery::default())
        .token(&caller.secret)
        .send_bytes()
        .await;
    assert_eq!(within.status, StatusCode::FORBIDDEN);
    // Modern typed admission checks the current place exclusion before the
    // ordinary legacy error projection can produce token.refused_here.
    assert_eq!(value(&within)["code"], json!(ErrorCode::AuthzDenied));
    status(
        world
            .app
            .call(MeGet, &[])
            .token(&caller.secret)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    no_native_work(&world).await;
}

#[tokio::test]
async fn current_key_and_members_management_cuts_are_rechecked_before_new_refusal() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let caller = scoped(
        &world,
        s.company.viewer.id(),
        vec![
            governance(s.company.acme, LIST),
            read_key(s.first.id),
            governance(s.company.acme, REFUSE),
        ],
    )
    .await;
    assert_eq!(
        page(list(&world, &caller.secret, s.company.acme, None, None).await)["items"][0]["id"],
        json!(s.first.id)
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(caller.id)
        .bind(serde_json::to_value(KeyScope::default()).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    status(
        refuse(&world, &caller.secret, s.company.acme, s.first.id).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(refusal_count(&world, s.company.acme, s.first.id).await, 0);
    assert!(
        identity::revoke_api_token(
            &mut *world.conn().await,
            caller.id,
            s.company.viewer.id(),
            world.now()
        )
        .await
        .unwrap()
    );
    status(
        refuse(&world, &caller.secret, s.company.acme, s.first.id).await,
        StatusCode::UNAUTHORIZED,
    );
    let current = scoped(
        &world,
        s.company.viewer.id(),
        vec![governance(s.company.acme, REFUSE)],
    )
    .await;
    assert!(
        access::remove_entry(
            &mut *world.conn().await,
            s.management,
            s.company.boss.id(),
            world.now()
        )
        .await
        .unwrap()
    );
    orch_authz::epoch::bump(&mut *world.conn().await)
        .await
        .unwrap();
    let denied = refuse(&world, &current.secret, s.company.acme, s.first.id).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let problem = value(&denied);
    assert_eq!(problem["code"], json!(ErrorCode::AuthzDenied));
    assert_eq!(problem["required"], "members.manage", "{problem}");
    assert_eq!(refusal_count(&world, s.company.acme, s.first.id).await, 0);
    no_native_work(&world).await;
}

#[tokio::test]
async fn expiry_legacy_subtree_and_place_refusal_are_not_bypassed_by_org_management() {
    let world = World::new().await;
    let s = Scene::new(&world).await;
    let expired = scoped(
        &world,
        s.company.viewer.id(),
        vec![governance(s.company.acme, REFUSE)],
    )
    .await;
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(expired.id)
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    status(
        refuse(&world, &expired.secret, s.company.acme, s.first.id).await,
        StatusCode::UNAUTHORIZED,
    );
    let project = places::make_project(&world, &s.company.boss, s.company.acme, "Narrow")
        .await
        .base
        .id;
    let narrow = issue(
        &world,
        s.company.viewer.id(),
        "Legacy subtree",
        Some(project),
        None,
    )
    .await;
    status(
        refuse(&world, &narrow.secret, s.company.acme, s.first.id).await,
        StatusCode::FORBIDDEN,
    );
    let mut policy: KeyScope =
        serde_json::from_value(json!({"rules":[governance(s.company.acme,REFUSE)]})).unwrap();
    policy.excluded_places.push(KeyPlaceExclusion::Refused {
        place: PlaceRef::Node(s.company.acme),
    });
    let excluded = issue(
        &world,
        s.company.viewer.id(),
        "Excluded org",
        None,
        Some(policy),
    )
    .await;
    status(
        refuse(&world, &excluded.secret, s.company.acme, s.first.id).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(refusal_count(&world, s.company.acme, s.first.id).await, 0);
    no_native_work(&world).await;
}
