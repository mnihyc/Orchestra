//! The native page/ownership family uses actual current targets and its own optional Read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod rig;
mod support;

use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::{PermissionPageGet, PermissionPageUpdate};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::document_tree::{
    DocumentContainerCreate, DocumentContainerCreateBody, DocumentTreeItem,
};
use orch_api_contract::ownership::{NodeTransfer, ObjectTransfer};
use orch_api_contract::resources::{CreateProvider, ProviderCreate, Upstream};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use places::{Who, make_project};
use serde_json::json;
use support::World;

struct Key {
    id: Id,
    secret: String,
}

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, owner: Id, rules: Vec<Json>) -> Key {
    credential(world, owner, Some(rules), Duration::hours(1)).await
}

async fn credential(world: &World, owner: Id, rules: Option<Vec<Json>>, lifetime: Duration) -> Key {
    let scope = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    let legacy_scopes = if scope.is_none() {
        vec!["*".to_owned()]
    } else {
        Vec::new()
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: owner,
            name: "Page/transfer family control",
            token_hash: &token_hash(&secret),
            scopes: &legacy_scopes,
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + lifetime),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}

struct Office {
    desk: desk::Desk,
    receiver: Who,
}

#[derive(Clone, Copy)]
struct Target {
    plural: &'static str,
    kind: &'static str,
    id: Id,
}

impl Office {
    async fn new() -> Self {
        orch_api_core::routes::resources::sealing::install_master_key(
            orch_db::secrets::MasterKey::new([36; 32]),
        );
        let desk = desk::Desk::new().await;
        let receiver = desk.member("page-receiver", "viewer").await;
        Self { desk, receiver }
    }
    async fn target(&self, kind: &'static str, name: &str) -> Target {
        let d = &self.desk;
        match kind {
            "project" => Target {
                plural: "nodes",
                kind,
                id: make_project(&d.world, &d.ada, d.org, name).await.base.id,
            },
            "document" => Target {
                plural: "documents",
                kind,
                id: d.make(&d.ada, name).await.base.id,
            },
            "document_container" => {
                let made = d
                    .world
                    .app
                    .call(DocumentContainerCreate, &[&d.topic])
                    .token(&d.ada.token)
                    .body(&DocumentContainerCreateBody {
                        parent_container_id: None,
                        name: name.into(),
                        client_id: new_id().to_string(),
                    })
                    .send()
                    .await
                    .ok()
                    .into_full()
                    .unwrap();
                let Some(DocumentTreeItem::Container(id)) =
                    made.entries.first().map(|entry| entry.item)
                else {
                    panic!("native producer must create a real folder")
                };
                Target {
                    plural: "document-containers",
                    kind,
                    id,
                }
            }
            "provider" => {
                let made = d
                    .world
                    .app
                    .call(ProviderCreate, &[])
                    .token(&d.ada.token)
                    .body(&CreateProvider::Api {
                        runtime_machine_id: None,
                        name: name.into(),
                        home_id: d.org,
                        access: None,
                        arrangements: None,
                        runtime_config: None,
                        upstreams: vec![Upstream {
                            protocol: "openai-responses".into(),
                            base_url: "https://page-transfer.invalid/v1".into(),
                            options: json!({}),
                        }],
                        api_key: Some("sk-page-control".into()),
                        secrets: None,
                        key_machine_id: None,
                        sources: None,
                    })
                    .send()
                    .await
                    .ok()
                    .into_full()
                    .unwrap();
                Target {
                    plural: "providers",
                    kind,
                    id: made.provider.base.id,
                }
            }
            _ => unreachable!(),
        }
    }
    async fn key(&self, rules: Vec<Json>) -> Key {
        key(&self.desk.world, self.desk.ada.id(), rules).await
    }
}

async fn update(world: &World, token: &str, target: Target, body: Json) -> RawReply {
    world
        .app
        .call(PermissionPageUpdate, &[&target.plural, &target.id])
        .token(token)
        .raw_body(&body)
        .send_bytes()
        .await
}
async fn transfer(
    world: &World,
    token: &str,
    target: Target,
    receiver: Id,
    keep: bool,
) -> RawReply {
    let body = json!({"user_id":receiver,"keep_previous":keep});
    if target.plural == "nodes" {
        world
            .app
            .call(NodeTransfer, &[&target.id])
            .token(token)
            .raw_body(&body)
            .send_bytes()
            .await
    } else {
        world
            .app
            .call(ObjectTransfer, &[&target.plural, &target.id])
            .token(token)
            .raw_body(&body)
            .send_bytes()
            .await
    }
}
fn success(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn page(world: &World, token: &str, target: Target) -> Json {
    success(
        world
            .app
            .call(PermissionPageGet, &[&target.plural, &target.id])
            .token(token)
            .send_bytes()
            .await,
    )
}
fn permission(target: Target) -> &'static str {
    match target.kind {
        "project" => "project.view",
        "document" | "document_container" => "doc.view",
        "provider" => "provider.view",
        _ => unreachable!(),
    }
}
fn body(target: Target, variant: &str, receiver: Id, note: &str) -> Json {
    match variant {
        "holders" => {
            json!({"rows":[{"permission":permission(target),"add":[{"holder":{"kind":"user","id":receiver},"note":note}]}]})
        }
        "page" => json!({"whole":"private"}),
        "owners" => json!({"owners":{"add":[{"kind":"user","id":receiver}]}}),
        _ => unreachable!(),
    }
}
async fn row_snapshot(world: &World, target: Target) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('rows',(SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY permission,profile),'[]'::jsonb) FROM permission_rows r WHERE object_id=$1),'entries',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY id),'[]'::jsonb) FROM permission_entries e WHERE object_id=$1),'audit',(SELECT count(*) FROM audit_log WHERE object_id=$1))")
        .bind(target.id).fetch_one(&world.db.pool).await.unwrap()
}
async fn first_owner(world: &World, target: Target) -> Id {
    let query = match target.kind {
        "project" => "SELECT owner_id FROM nodes WHERE id=$1",
        "document" => "SELECT owner_id FROM documents WHERE id=$1",
        "document_container" => "SELECT owner_id FROM document_containers WHERE id=$1",
        "provider" => "SELECT owner_id FROM providers WHERE id=$1",
        _ => unreachable!(),
    };
    sqlx::query_scalar(query)
        .bind(target.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn page_family_all_body_variants_use_native_node_document_and_resource_writers() {
    let d = Office::new().await;
    let mut failures = Vec::new();
    for kind in ["project", "document", "provider"] {
        for variant in ["holders", "page", "owners"] {
            let ordinary = d.target(kind, &format!("Native {kind} {variant}")).await;
            let native = success(
                update(
                    &d.desk.world,
                    &d.desk.ada.token,
                    ordinary,
                    body(ordinary, variant, d.receiver.id(), "native holder note"),
                )
                .await,
            );
            assert_eq!(
                native["object"]["id"],
                json!(ordinary.id),
                "native full page premise"
            );
            let target = d.target(kind, &format!("Scoped {kind} {variant}")).await;
            let operation = format!("PermissionPageUpdate.{variant}");
            let token = d
                .key(vec![rule(&operation, kind, target.id, &["manage", "read"])])
                .await;
            let response = update(
                &d.desk.world,
                &token.secret,
                target,
                body(target, variant, d.receiver.id(), "scoped holder note"),
            )
            .await;
            if response.status != StatusCode::OK {
                failures.push(format!(
                    "{operation}/{kind}: HTTP{} {}",
                    response.status,
                    String::from_utf8_lossy(&response.raw)
                ));
                continue;
            }
            let value = success(response);
            assert_eq!(
                value["object"]["id"],
                json!(target.id),
                "full with own Read"
            );
            let stored = page(&d.desk.world, &d.desk.ada.token, target).await;
            assert_eq!(value["revision"], stored["revision"]);
            if variant == "holders" {
                assert!(stored.to_string().contains("scoped holder note"));
            }
            if variant == "page" {
                assert_eq!(
                    stored["state"], native["state"],
                    "native page state is preserved"
                );
                assert_eq!(
                    stored["state"],
                    if kind == "project" {
                        "restricted"
                    } else {
                        "custom"
                    }
                );
                let view = stored["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["permission"] == permission(target))
                    .expect("native content View row");
                assert_eq!(
                    view["inherit"], false,
                    "the native private write cut View inheritance"
                );
            }
            if variant == "owners" {
                assert!(
                    stored["owners"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry["holder"]["user"]["id"] == json!(d.receiver.id()))
                );
            }
        }
    }
    assert!(
        failures.is_empty(),
        "all actual modern family attempts:\n{}",
        failures.join("\n")
    );
}

#[tokio::test]
async fn node_transfer_preserves_native_full_results_and_manage_only_receipts() {
    let d = Office::new().await;
    let native = d.target("project", "Native node transfer").await;
    let value = success(
        transfer(
            &d.desk.world,
            &d.desk.ada.token,
            native,
            d.receiver.id(),
            true,
        )
        .await,
    );
    let _: orch_api_contract::nodes::Node = serde_json::from_value(value).unwrap();
    assert_eq!(first_owner(&d.desk.world, native).await, d.receiver.id());

    for read in [true, false] {
        let target = d.target("project", "Scoped node transfer").await;
        let actions: &[&str] = if read {
            &["manage", "read"]
        } else {
            &["manage"]
        };
        let token = d
            .key(vec![rule("NodeTransfer", target.kind, target.id, actions)])
            .await;
        let value =
            success(transfer(&d.desk.world, &token.secret, target, d.receiver.id(), true).await);
        assert_eq!(first_owner(&d.desk.world, target).await, d.receiver.id());
        if read {
            let _: orch_api_contract::nodes::Node = serde_json::from_value(value).unwrap();
        } else {
            assert_eq!(
                value,
                json!({
                    "projection": "receipt",
                    "object_id": target.id,
                    "kind": "project",
                    "owner_id": d.receiver.id(),
                    "changed": true,
                }),
                "accepted ownership facts contain no existing node content"
            );
        }
    }
}

#[tokio::test]
async fn transfer_family_uses_native_node_resource_document_and_folder_owner_changes() {
    let d = Office::new().await;
    let mut failures = Vec::new();
    for kind in ["project", "provider", "document", "document_container"] {
        let native = d.target(kind, &format!("Native transfer {kind}")).await;
        success(
            transfer(
                &d.desk.world,
                &d.desk.ada.token,
                native,
                d.receiver.id(),
                true,
            )
            .await,
        );
        assert_eq!(first_owner(&d.desk.world, native).await, d.receiver.id());
        let target = d.target(kind, &format!("Scoped transfer {kind}")).await;
        let operation = if kind == "project" {
            "NodeTransfer"
        } else {
            "ObjectTransfer"
        };
        let token = d
            .key(vec![rule(operation, kind, target.id, &["manage", "read"])])
            .await;
        let reply = transfer(&d.desk.world, &token.secret, target, d.receiver.id(), true).await;
        if reply.status != StatusCode::OK {
            failures.push(format!(
                "{operation}/{kind}: HTTP{} {}",
                reply.status,
                String::from_utf8_lossy(&reply.raw)
            ));
            continue;
        }
        let value = success(reply);
        assert_ne!(
            value["projection"], "receipt",
            "sufficient own Read keeps native full"
        );
        if target.plural == "nodes" {
            let _: orch_api_contract::nodes::Node = serde_json::from_value(value).unwrap();
        } else {
            let _: orch_api_contract::resources::Resource = serde_json::from_value(value).unwrap();
        }
        assert_eq!(first_owner(&d.desk.world, target).await, d.receiver.id());
    }
    assert!(
        failures.is_empty(),
        "all actual modern transfer attempts:\n{}",
        failures.join("\n")
    );
}

#[tokio::test]
async fn manage_only_page_write_and_stale_error_keep_existing_acl_content_private() {
    let d = Office::new().await;
    let target = d.target("document", "Protected page title").await;
    let seeded = success(
        update(
            &d.desk.world,
            &d.desk.ada.token,
            target,
            body(
                target,
                "holders",
                d.receiver.id(),
                "protected existing grant note",
            ),
        )
        .await,
    );
    let before = seeded["revision"].as_i64().unwrap();
    let writer = d
        .key(vec![rule(
            "PermissionPageUpdate.holders",
            target.kind,
            target.id,
            &["manage"],
        )])
        .await;
    let value = success(update(&d.desk.world, &writer.secret, target, json!({"expected_revision":before,"rows":[{"permission":"doc.edit","add":[{"holder":{"kind":"user","id":d.receiver.id()},"note":"caller supplied grant"}]}]})).await);
    assert_eq!(
        value,
        json!({"projection":"receipt","object_id":target.id,"kind":"document","revision":before+1,"defaults":false})
    );
    let current = page(&d.desk.world, &d.desk.ada.token, target).await;
    assert!(current.to_string().contains("caller supplied grant"));
    let snapshot = row_snapshot(&d.desk.world, target).await;
    let stale = update(&d.desk.world, &writer.secret, target, json!({"expected_revision":before,"rows":[{"permission":"doc.view","add":[{"holder":{"kind":"user","id":d.receiver.id()},"note":"must not commit"}]}]})).await;
    assert_eq!(stale.problem().code, ErrorCode::ConflictStaleRevision);
    assert!(stale.problem().extra.get("current").is_none());
    let raw = String::from_utf8_lossy(&stale.raw);
    for protected in [
        "Protected page title",
        "protected existing grant note",
        "caller supplied grant",
    ] {
        assert!(!raw.contains(protected), "stale content: {raw}");
    }
    assert_eq!(row_snapshot(&d.desk.world, target).await, snapshot);
}

#[tokio::test]
async fn full_group_source_is_not_empty_or_borrowed_from_another_permission_operation() {
    let d = Office::new().await;
    let group = d
        .desk
        .world
        .app
        .call(OrgGroupCreate, &[&d.desk.org])
        .token(&d.desk.ada.token)
        .body(&OrgGroupCreateBody {
            name: "Protected page group".into(),
            description: Some("Protected group description".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    let target = d.target("project", "Group page").await;
    let write = json!({"rows":[{"permission":"project.view","add":[{"holder":{"kind":"group","id":group.id},"note":"first group grant"}]}]});
    let ordinary = success(update(&d.desk.world, &d.desk.ada.token, target, write.clone()).await);
    assert!(
        ordinary.to_string().contains("Protected group description"),
        "actual ordinary Group DTO premise"
    );
    let unrelated = d
        .key(vec![
            rule(
                "PermissionPageUpdate.holders",
                target.kind,
                target.id,
                &["manage", "read"],
            ),
            rule("PermissionPageGet.view", "group", group.id, &["read"]),
        ])
        .await;
    let response = success(update(&d.desk.world, &unrelated.secret, target, json!({"rows":[{"permission":"project.interact","add":[{"holder":{"kind":"group","id":group.id},"note":"kept without full group Read"}]}]})).await);
    assert_eq!(response["projection"], "receipt");
    assert!(!response.to_string().contains("Protected page group"));
    let own = d
        .key(vec![
            rule(
                "PermissionPageUpdate.holders",
                target.kind,
                target.id,
                &["manage", "read"],
            ),
            rule("PermissionPageUpdate.holders", "group", group.id, &["read"]),
            rule("PermissionPageUpdate.holders", "org", d.desk.org, &["read"]),
        ])
        .await;
    let full = success(update(&d.desk.world, &own.secret, target, json!({"rows":[{"permission":"project.view","add":[{"holder":{"kind":"user","id":d.receiver.id()},"note":"now read full group"}]}]})).await);
    let got = full["rows"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row["entries"].as_array().into_iter().flatten())
        .find(|entry| entry["holder"]["kind"] == "group")
        .expect("full Group holder is kept");
    assert_eq!(
        got["holder"]["group"],
        ordinary["rows"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row["entries"].as_array().into_iter().flatten())
            .find(|entry| entry["holder"]["kind"] == "group")
            .unwrap()["holder"]["group"]
    );
    assert!(
        page(&d.desk.world, &d.desk.ada.token, target)
            .await
            .to_string()
            .contains("kept without full group Read")
    );
}

#[tokio::test]
async fn transfer_removing_the_caller_keeps_the_effect_and_returns_only_its_own_acknowledgement() {
    let d = Office::new().await;
    // Ordinary transfer proves active-member/owner prerequisites without relying on retained Read.
    let native = d.target("provider", "Native ownership premise").await;
    success(
        transfer(
            &d.desk.world,
            &d.desk.ada.token,
            native,
            d.receiver.id(),
            true,
        )
        .await,
    );
    let target = d.target("provider", "Private transferred provider").await;
    success(
        update(
            &d.desk.world,
            &d.desk.ada.token,
            target,
            json!({"whole":"private"}),
        )
        .await,
    );
    let writer = d
        .key(vec![rule(
            "ObjectTransfer",
            target.kind,
            target.id,
            &["manage"],
        )])
        .await;
    let value = success(
        transfer(
            &d.desk.world,
            &writer.secret,
            target,
            d.receiver.id(),
            false,
        )
        .await,
    );
    assert_eq!(
        value,
        json!({"projection":"receipt","object_id":target.id,"kind":"provider","owner_id":d.receiver.id(),"changed":true})
    );
    assert_eq!(first_owner(&d.desk.world, target).await, d.receiver.id());
    let owners = page(&d.desk.world, &d.receiver.token, target).await;
    assert!(
        !owners["owners"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["holder"]["user"]["id"] == json!(d.desk.ada.id()))
    );
}

#[tokio::test]
async fn body_variant_target_and_native_giving_authority_are_independent_refusal_boundaries() {
    let d = Office::new().await;
    let target = d.target("document", "Delegation boundary").await;
    let other = d.target("document", "Other target").await;
    let holder = d
        .key(vec![rule(
            "PermissionPageUpdate.holders",
            target.kind,
            target.id,
            &["manage"],
        )])
        .await;
    for (selected, change) in [
        (target, json!({"whole":"private"})),
        (
            target,
            json!({"owners":{"add":[{"kind":"user","id":d.receiver.id()}]}}),
        ),
        (
            other,
            body(other, "holders", d.receiver.id(), "outside target"),
        ),
    ] {
        let before = row_snapshot(&d.desk.world, selected).await;
        let reply = update(&d.desk.world, &holder.secret, selected, change).await;
        assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
        assert_eq!(row_snapshot(&d.desk.world, selected).await, before);
    }
    // Read-only ordinary recipient can share Read, but cannot give Edit they do not hold.
    success(update(&d.desk.world, &d.desk.ada.token, target, json!({"whole":"private","rows":[{"permission":"doc.view","add":[{"holder":{"kind":"user","id":d.receiver.id()}}]}]})).await);
    let nongranter = key(
        &d.desk.world,
        d.receiver.id(),
        vec![rule(
            "PermissionPageUpdate.holders",
            target.kind,
            target.id,
            &["manage"],
        )],
    )
    .await;
    let third = d.desk.member("native-read-recipient", "viewer").await;
    let fourth = d.desk.member("scoped-read-recipient", "viewer").await;
    let native_share = success(
        update(
            &d.desk.world,
            &d.receiver.token,
            target,
            body(target, "holders", third.id(), "native holder sharing"),
        )
        .await,
    );
    assert!(native_share.to_string().contains("native holder sharing"));
    let scoped_share = success(
        update(
            &d.desk.world,
            &nongranter.secret,
            target,
            body(target, "holders", fourth.id(), "scoped holder sharing"),
        )
        .await,
    );
    assert_eq!(scoped_share["projection"], "receipt");
    assert!(
        page(&d.desk.world, &d.desk.ada.token, target)
            .await
            .to_string()
            .contains("scoped holder sharing"),
        "scope Manage must not invent ordinary access.manage for a native holder-share"
    );
    let body = json!({"rows":[{"permission":"doc.edit","add":[{"holder":{"kind":"user","id":d.desk.ada.id()}}]}]});
    let before = row_snapshot(&d.desk.world, target).await;
    let native = update(&d.desk.world, &d.receiver.token, target, body.clone()).await;
    assert_eq!(
        native.problem().code,
        ErrorCode::AuthzDenied,
        "ordinary native nongranter premise"
    );
    let scoped = update(&d.desk.world, &nongranter.secret, target, body).await;
    assert_eq!(scoped.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(row_snapshot(&d.desk.world, target).await, before);
}

// A finite rendezvous guard detects a missing native wait rather than sleeping across expiry.
const RENDEZVOUS: std::time::Duration = std::time::Duration::from_secs(20);
async fn blocked(
    world: &World,
    blocker: i32,
    pending: &tokio::task::JoinHandle<RawReply>,
    identity: bool,
) -> bool {
    tokio::time::timeout(RENDEZVOUS, async {
        loop {
            let reached: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pg_blocking_pids(pid) @> ARRAY[$1]::integer[] AND wait_event_type='Lock' AND CASE WHEN $2 THEN lower(query) LIKE '%orch_authz_saw%' ELSE lower(query) LIKE '%nodes%for update%' END)")
                .bind(blocker).bind(identity).fetch_one(&world.db.pool).await.unwrap();
            if reached { return true; }
            if pending.is_finished() { return false; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap_or(false)
}

#[tokio::test]
async fn page_native_target_wait_rechecks_the_same_credential_before_the_first_page_effect() {
    let d = Office::new().await;
    let target = d.target("project", "Actual page wait").await;
    let native = d.target("project", "Native page wait premise").await;
    success(
        update(
            &d.desk.world,
            &d.desk.ada.token,
            native,
            json!({"whole":"private"}),
        )
        .await,
    );
    let token = credential(&d.desk.world, d.desk.ada.id(), None, Duration::seconds(1)).await;
    let before = row_snapshot(&d.desk.world, target).await;
    let mut identity = d.desk.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 FOR UPDATE")
        .bind(token.id)
        .fetch_one(&mut *identity)
        .await
        .unwrap();
    let identity_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *identity)
        .await
        .unwrap();
    let app = d.desk.world.app.clone();
    let pending = tokio::spawn(async move { update_app(app, token.secret, target).await });
    if !blocked(&d.desk.world, identity_pid, &pending, true).await {
        pending.abort();
        let result = pending.await;
        identity.rollback().await.unwrap();
        panic!("real native action guard was not reached: {result:?}");
    }
    let mut row = d.desk.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
        .bind(target.id)
        .fetch_one(&mut *row)
        .await
        .unwrap();
    let row_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *row)
        .await
        .unwrap();
    identity.rollback().await.unwrap();
    if !blocked(&d.desk.world, row_pid, &pending, false).await {
        pending.abort();
        let result = pending.await;
        row.rollback().await.unwrap();
        panic!("the actual native page target-row wait was not reached: {result:?}");
    }
    d.desk.world.advance(Duration::seconds(1));
    row.rollback().await.unwrap();
    let response = tokio::time::timeout(RENDEZVOUS, pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row_snapshot(&d.desk.world, target).await,
        before,
        "expired native page writer changed actual rows; HTTP{} {}",
        response.status,
        String::from_utf8_lossy(&response.raw)
    );
    assert_eq!(response.problem().code, ErrorCode::AuthInvalidToken);
}
async fn update_app(
    app: orch_api_core::testing::TestApp,
    token: String,
    target: Target,
) -> RawReply {
    let object = target.plural.to_owned();
    let id = target.id.to_string();
    let request = app
        .call(PermissionPageUpdate, &[&object, &id])
        .token(&token)
        .raw_body(&json!({"whole":"private"}));
    request.send_bytes().await
}

#[tokio::test]
async fn native_and_scoped_page_grants_need_recipient_view_without_agent_control_or_work() {
    use orch_api_contract::access::{EffectiveAccessGet, EffectiveAccessGetQuery};
    use orch_api_contract::documents::{NewDocumentAccess, NodeDocumentCreate};
    let s = rig::Scene::new().await;
    let world = &s.rig.world;
    let thread = s.thread().await.base.id;
    let held = world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &thread])
        .token(&s.ada.token)
        .query(&EffectiveAccessGetQuery {
            user_id: s.cai.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        held.permissions
            .iter()
            .any(|row| row.permission == "thread.view"),
        "real ordinary recipient visibility premise"
    );
    assert!(
        !held
            .permissions
            .iter()
            .any(|row| row.permission == "thread.interact"),
        "pure giver has no recipient control premise"
    );
    let mut docs = Vec::new();
    for title in ["Native pure page loan", "Scoped pure page loan"] {
        let mut body = desk::body(title);
        body.access = Some(NewDocumentAccess::Private);
        let document = world
            .app
            .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
            .token(&s.cai.token)
            .body(&body)
            .send()
            .await
            .ok()
            .into_full()
            .unwrap();
        assert_eq!(
            document.base.owner.id,
            s.cai.id(),
            "the actual giver owns its source"
        );
        docs.push(Target {
            plural: "documents",
            kind: "document",
            id: document.base.id,
        });
    }
    let execution = || async {
        sqlx::query_scalar::<_,Json>("SELECT jsonb_build_object('state',state,'turn',current_turn_id,'notices',pending_notices,'inputs',(SELECT count(*) FROM thread_inputs WHERE thread_id=$1),'turns',(SELECT count(*) FROM turns WHERE thread_id=$1)) FROM threads WHERE node_id=$1").bind(thread).fetch_one(&world.db.pool).await.unwrap()
    };
    let before = execution().await;
    let wire_before = s.rig.seen.all().len();
    let body = || json!({"rows":[{"permission":"doc.manage","add":[{"holder":{"kind":"agents","id":thread},"note":"pure current-giver page loan"}]}]});
    let native = update(world, &s.cai.token, docs[0], body()).await;
    let token = key(
        world,
        s.cai.id(),
        vec![
            rule(
                "PermissionPageUpdate.holders",
                "document",
                docs[1].id,
                &["manage"],
            ),
            rule("PermissionPageUpdate.holders", "thread", thread, &["read"]),
        ],
    )
    .await;
    let scoped = update(world, &token.secret, docs[1], body()).await;
    assert_eq!(
        execution().await,
        before,
        "a pure page gift submits no input or turn"
    );
    assert_eq!(
        s.rig.seen.all().len(),
        wire_before,
        "a pure gift issues no native command"
    );
    s.rig.engine.stop();
    // Attempt both actual surfaces even when the first native gate exposes the contradiction.
    let statuses = [native.status, scoped.status];
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::OK],
        "approved pure gift; native{} {}; scoped{} {}",
        native.status,
        String::from_utf8_lossy(&native.raw),
        scoped.status,
        String::from_utf8_lossy(&scoped.raw)
    );
    for target in docs {
        let stored: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND permission='doc.manage' AND holder_kind='agents' AND holder_id=$2 AND created_by=$3 AND removed_at IS NULL)").bind(target.id).bind(thread).bind(s.cai.id()).fetch_one(&world.db.pool).await.unwrap();
        assert!(
            stored,
            "the existing page writer records its real giver and agent holder"
        );
    }
}
