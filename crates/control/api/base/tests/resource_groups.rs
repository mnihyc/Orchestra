//! Display grouping uses actual resource homes, native tag definitions and current authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions"
)]

#[path = "resource_groups/home_rules.rs"]
mod home_rules;
mod places;
mod support;

use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ResourceKind};
use orch_api_contract::{
    directory::{NodeTagCreate, NodeTagCreateBody},
    everything::{EverythingList, EverythingListQuery},
    resource_groups::{ObjectHomeSet, ObjectTagsSet, ResourceHomeSetBody, ResourceTagsSetBody},
};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{
    access,
    identity::{self, NewApiToken},
    resources::access_fixtures,
};
use orch_types::{Id, Json, Time, new_id};
use places::{Who, make_org, make_project, person};
use serde_json::json;
use support::World;

fn plural(kind: &str) -> &'static str {
    match kind {
        "provider" => "providers",
        "agent" => "agents",
        "environment" => "environments",
        "host" => "hosts",
        "machine" => "machines",
        "template" => "templates",
        "mcp_server" => "mcp-servers",
        "mount" => "mounts",
        "skill" => "skills",
        _ => panic!("unknown fixture resource"),
    }
}
fn table(kind: &str) -> &'static str {
    match kind {
        "provider" => "providers",
        "agent" => "agents",
        "environment" => "environments",
        "host" => "hosts",
        "machine" => "machines",
        "template" => "env_templates",
        "mcp_server" => "mcp_servers",
        "mount" => "mounts",
        "skill" => "skills",
        _ => panic!("unknown fixture resource"),
    }
}
async fn resource(world: &World, owner: &Who, home: Id, kind: &str, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = access_fixtures::insert_resource(&mut conn, kind, Some(owner.id()), Some(home), name)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: kind.parse::<ResourceKind>().unwrap().into(),
            id,
        },
        owner.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
}
async fn row(world: &World, kind: &str, id: Id) -> Json {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT to_jsonb(r) FROM {} r WHERE id=$1",
        table(kind)
    )))
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}
async fn home(
    world: &World,
    token: &str,
    kind: &str,
    id: Id,
    to: Id,
    expected: Option<i64>,
) -> RawReply {
    world
        .app
        .call(ObjectHomeSet, &[&plural(kind), &id])
        .token(token)
        .body(&ResourceHomeSetBody {
            home_id: to,
            expected_revision: expected,
        })
        .send_bytes()
        .await
}
async fn tags(world: &World, token: &str, kind: &str, id: Id, ids: Vec<Id>) -> RawReply {
    world
        .app
        .call(ObjectTagsSet, &[&plural(kind), &id])
        .token(token)
        .body(&ResourceTagsSetBody {
            tag_ids: ids,
            expected_revision: None,
        })
        .send_bytes()
        .await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn key(world: &World, owner: &Who, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: owner.id(),
            name: "Resource grouping control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}
fn rule(operation: &str, kind: &str, id: Id, resources: &[&str], action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":resources,"allow":[{"operation":operation,"actions":[action]}]})
}
async fn tag(world: &World, owner: &Who, home: Id, name: &str) -> Id {
    world
        .app
        .call(NodeTagCreate, &[&home])
        .token(&owner.token)
        .body(&NodeTagCreateBody {
            name: name.into(),
            color: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the tag creator receives the full tag")
        .id
}

#[tokio::test]
async fn every_flat_resource_can_move_home_without_changing_its_owner_or_origin() {
    let world = World::new().await;
    let owner = person(&world, "home-move-owner").await;
    let from = make_org(&world, &owner, "Old home", "old-home")
        .await
        .base
        .id;
    let target_root = from;
    let target = make_project(&world, &owner, target_root, "Guests")
        .await
        .base
        .id;
    for kind in [
        "provider",
        "agent",
        "environment",
        "host",
        "machine",
        "template",
        "mcp_server",
        "mount",
        "skill",
    ] {
        let id = resource(&world, &owner, from, kind, &format!("home-{kind}")).await;
        let before = row(&world, kind, id).await;
        let answer = accepted(
            home(
                &world,
                &owner.token,
                kind,
                id,
                target,
                before["revision"].as_i64(),
            )
            .await,
        );
        assert_eq!(
            answer,
            json!({"projection":"receipt","object_id":id,"kind":kind,"home_id":target,"changed":true})
        );
        let after = row(&world, kind, id).await;
        assert_eq!(after["owner_id"], before["owner_id"], "{kind}");
        assert_eq!(after.get("machine_id"), before.get("machine_id"), "{kind}");
        assert_eq!(
            after.get("key_machine_id"),
            before.get("key_machine_id"),
            "{kind}"
        );
        assert_eq!(after.get("spec"), before.get("spec"), "{kind}");
        assert_eq!(
            after[if kind == "environment" {
                "home_node_id"
            } else {
                "home_id"
            }],
            json!(target)
        );
        assert_eq!(after["org_id"], json!(target_root));
        assert!(!after["home_overridden_at"].is_null());
        let repeated = accepted(home(&world, &owner.token, kind, id, target, None).await);
        assert_eq!(repeated["changed"], false);
        assert_eq!(
            row(&world, kind, id).await["home_overridden_at"],
            after["home_overridden_at"]
        );
        let audits:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_kind=$1 AND object_id=$2 AND action='resource.home_changed'")
            .bind(kind).bind(id).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(audits, 1);
    }
}

#[tokio::test]
async fn a_resource_manager_can_move_it_but_needs_management_of_the_actual_destination() {
    let world = World::new().await;
    let owner = person(&world, "resource-owner").await;
    let manager = person(&world, "resource-manager").await;
    let source = make_org(&world, &owner, "Source", "source").await.base.id;
    places::join(&world, source, source, &manager, "viewer", &owner).await;
    let target = places::make_department(
        &world,
        &owner,
        source,
        "Managed destination",
        "managed-destination",
    )
    .await
    .base
    .id;
    access::testing::add_entry(
        &mut *world.conn().await,
        &access::PageKey::new("node", target, "live"),
        "any",
        "org.manage",
        ("user", Some(manager.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    let id = resource(&world, &owner, source, "template", "manager-moved").await;
    assert!(matches!(
        home(&world, &manager.token, "template", id, target, None)
            .await
            .status,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    access::testing::add_entry(
        &mut *world.conn().await,
        &access::PageKey::new("template", id, "live"),
        "any",
        "template.manage",
        ("user", Some(manager.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    assert_eq!(
        home(&world, &manager.token, "template", id, source, None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    accepted(home(&world, &manager.token, "template", id, target, None).await);
    assert_eq!(
        row(&world, "template", id).await["owner_id"],
        json!(owner.id())
    );
}

#[tokio::test]
async fn scoped_home_moves_require_the_exact_operation_source_and_destination_manage() {
    let world = World::new().await;
    let owner = person(&world, "scoped-home-owner").await;
    let source = make_org(&world, &owner, "Scoped source", "scoped-source")
        .await
        .base
        .id;
    let target = places::make_department(&world, &owner, source, "Scoped target", "scoped-target")
        .await
        .base
        .id;
    let id = resource(&world, &owner, source, "template", "scope-move").await;
    for rules in [
        vec![rule(
            "ObjectHomeSet",
            "template",
            id,
            &["template"],
            "manage",
        )],
        vec![rule("ObjectHomeSet", "org", target, &["org"], "manage")],
        vec![
            rule("ObjectTransfer", "template", id, &["template"], "manage"),
            rule("ObjectHomeSet", "org", target, &["org"], "manage"),
        ],
        vec![
            rule("ObjectHomeSet", "template", id, &["template"], "manage"),
            rule("ObjectHomeSet", "org", source, &["org"], "manage"),
        ],
    ] {
        let token = key(&world, &owner, rules).await;
        assert_eq!(
            home(&world, &token, "template", id, target, None)
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(row(&world, "template", id).await["home_id"], json!(source));
    }
    let token = key(
        &world,
        &owner,
        vec![
            rule("ObjectHomeSet", "template", id, &["template"], "manage"),
            rule("ObjectHomeSet", "org", target, &["org"], "manage"),
        ],
    )
    .await;
    let expected = row(&world, "template", id).await["revision"]
        .as_i64()
        .unwrap();
    assert_eq!(
        home(&world, &token, "template", id, target, Some(expected - 1))
            .await
            .status,
        StatusCode::CONFLICT
    );
    let result = accepted(home(&world, &token, "template", id, target, Some(expected)).await);
    assert_eq!(result["projection"], "receipt");
    assert!(result.get("owner").is_none());
    assert!(result.get("name").is_none());
}

#[tokio::test]
async fn resource_tags_reuse_place_definitions_and_stay_visible_after_a_home_move() {
    let world = World::new().await;
    let owner = person(&world, "resource-tags-owner").await;
    let source = make_org(&world, &owner, "Tag source", "tag-source")
        .await
        .base
        .id;
    let target = places::make_department(&world, &owner, source, "Tag target", "tag-target")
        .await
        .base
        .id;
    let old = tag(&world, &owner, source, "Guest").await;
    let new = tag(&world, &owner, target, "Chosen").await;
    for kind in [
        "provider",
        "agent",
        "environment",
        "host",
        "machine",
        "template",
        "mcp_server",
        "mount",
        "skill",
    ] {
        let id = resource(&world, &owner, source, kind, &format!("tagged-{kind}")).await;
        assert_eq!(
            tags(&world, &owner.token, kind, id, vec![new]).await.status,
            StatusCode::BAD_REQUEST
        );
        accepted(tags(&world, &owner.token, kind, id, vec![old, old]).await);
        accepted(home(&world, &owner.token, kind, id, target, None).await);
        accepted(tags(&world, &owner.token, kind, id, vec![old, new]).await);
        let page = world
            .app
            .call(EverythingList, &[])
            .token(&owner.token)
            .query(&EverythingListQuery {
                parent: Some("resources".into()),
                kinds: Some(kind.into()),
                q: Some(format!("tagged-{kind}")),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        let shown = page
            .items
            .iter()
            .find(|row| row.id == id.to_string())
            .unwrap();
        let assigned: std::collections::BTreeSet<_> = shown.tags.iter().map(|tag| tag.id).collect();
        assert_eq!(assigned, std::collections::BTreeSet::from([old, new]));
        assert_eq!(
            shown.grouping.as_ref().unwrap().home.as_ref().unwrap().id,
            target
        );
        let token = key(
            &world,
            &owner,
            vec![rule("ObjectTagsSet", kind, id, &[kind], "manage")],
        )
        .await;
        let cleared = accepted(tags(&world, &token, kind, id, Vec::new()).await);
        assert_eq!(cleared["projection"], "receipt");
        assert_eq!(cleared["tag_ids"], json!([]));
    }
}

#[tokio::test]
async fn a_home_move_checks_raw_expiry_after_the_actual_destination_lock_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = person(&world, "waiting-home-owner").await;
        let source = make_org(&world, &owner, "Waiting source", "waiting-source")
            .await
            .base
            .id;
        let target =
            places::make_department(&world, &owner, source, "Waiting target", "waiting-target")
                .await
                .base
                .id;
        let id = resource(&world, &owner, source, "template", "waiting-move").await;
        let token = key(
            &world,
            &owner,
            vec![
                rule("ObjectHomeSet", "template", id, &["template"], "manage"),
                rule("ObjectHomeSet", "org", target, &["org"], "manage"),
            ],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(target)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let pending = tokio::spawn(async move {
            let request = app
                .call(ObjectHomeSet, &[&"templates", &id])
                .token(&token)
                .body(&ResourceHomeSetBody {
                    home_id: target,
                    expected_revision: None,
                });
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {
                assert!(!pending.is_finished(),"the actual destination wait must be reached");
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM nodes%FOR SHARE%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if waiting {break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the native destination row must block");
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert!(matches!(
                result.status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ));
            let current = row(&world, "template", id).await;
            assert_eq!(current["home_id"], json!(source));
            assert!(current["home_overridden_at"].is_null());
        } else {
            accepted(result);
        }
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='resource.home_changed'",
        )
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(audits, i64::from(!expire));
    }
}

#[tokio::test]
async fn purging_one_resource_cleans_only_its_tag_associations() {
    let world = World::new().await;
    let owner = person(&world, "tag-lifecycle-owner").await;
    let root = make_org(&world, &owner, "Tag lifecycle", "tag-lifecycle")
        .await
        .base
        .id;
    let definition = tag(&world, &owner, root, "Kept definition").await;
    let gone = resource(&world, &owner, root, "template", "purged-tagged").await;
    let kept = resource(&world, &owner, root, "template", "kept-tagged").await;
    accepted(tags(&world, &owner.token, "template", gone, vec![definition]).await);
    accepted(tags(&world, &owner.token, "template", kept, vec![definition]).await);
    sqlx::query("DELETE FROM env_templates WHERE id=$1")
        .bind(gone)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let rows: Vec<Id> = sqlx::query_scalar("SELECT resource_id FROM resource_tags WHERE tag_id=$1")
        .bind(definition)
        .fetch_all(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(rows, vec![kept]);
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tags WHERE id=$1)")
        .bind(definition)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(exists);
}

#[tokio::test]
async fn resource_name_choices_keep_their_homes_and_bind_origin_labels_to_the_consuming_operation()
{
    use orch_api_contract::search::{NameSearchList, NameSearchListQuery};
    let world = World::new().await;
    let owner = person(&world, "name-group-owner").await;
    let org = make_org(&world, &owner, "Name group home", "name-group-home")
        .await
        .base
        .id;
    for kind in [
        "provider",
        "agent",
        "environment",
        "host",
        "machine",
        "template",
        "mcp_server",
        "mount",
        "skill",
    ] {
        let id = resource(
            &world,
            &owner,
            org,
            kind,
            &format!("grouping-needle-{kind}"),
        )
        .await;
        let answer = accepted(
            world
                .app
                .call(NameSearchList, &[])
                .token(&owner.token)
                .query(&NameSearchListQuery {
                    q: Some("grouping-needle".into()),
                    kinds: Some(kind.into()),
                    ..Default::default()
                })
                .send_bytes()
                .await,
        );
        let hit = answer["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|hit| hit["id"] == json!(id))
            .unwrap();
        assert_eq!(hit["home"]["id"], json!(org), "{kind}");
        assert_eq!(hit["home"]["redacted"], false, "{kind}");
        assert!(hit.get("origin_machine").is_some(), "{kind}");
        assert_eq!(
            hit["availability"],
            json!([]),
            "names without an execution context make no availability claim"
        );
    }
    let machine = resource(&world, &owner, org, "machine", "Physical needle").await;
    let provider = resource(&world, &owner, org, "provider", "Origin needle").await;
    sqlx::query("UPDATE providers SET key_machine_id=$2 WHERE id=$1")
        .bind(provider)
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let query = NameSearchListQuery {
        q: Some("Origin needle".into()),
        kinds: Some("provider".into()),
        ..Default::default()
    };
    let selected = |kind: &str, id: Id, operation: &str| json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]});
    for same_operation in [true, false] {
        let secret = key(
            &world,
            &owner,
            vec![
                selected("provider", provider, "NameSearchList"),
                selected(
                    "org",
                    org,
                    if same_operation {
                        "NameSearchList"
                    } else {
                        "OrgGet"
                    },
                ),
                selected(
                    "machine",
                    machine,
                    if same_operation {
                        "NameSearchList"
                    } else {
                        "MachineGet"
                    },
                ),
            ],
        )
        .await;
        let answer = accepted(
            world
                .app
                .call(NameSearchList, &[])
                .token(&secret)
                .query(&query)
                .send_bytes()
                .await,
        );
        let hit = &answer["items"][0];
        assert_eq!(hit["id"], json!(provider));
        if same_operation {
            assert_eq!(hit["home"]["redacted"], false);
            assert_eq!(hit["origin_machine"]["id"], json!(machine));
        } else {
            assert_eq!(hit["home"]["redacted"], true);
            assert!(hit["origin_machine"].is_null());
        }
    }
}

#[tokio::test]
async fn every_flat_home_route_refuses_another_root_and_points_to_the_atomic_move() {
    let world = World::new().await;
    let owner = person(&world, "cross-root-home-owner").await;
    let source = make_org(&world, &owner, "Source", "source").await.base.id;
    let target = make_org(&world, &owner, "Other root", "other-root")
        .await
        .base
        .id;
    for kind in [
        "provider",
        "agent",
        "environment",
        "host",
        "machine",
        "template",
        "mcp_server",
        "mount",
        "skill",
    ] {
        let id = resource(&world, &owner, source, kind, &format!("cross-root-{kind}")).await;
        let before = row(&world, kind, id).await;
        let refused = home(&world, &owner.token, kind, id, target, None).await;
        assert_eq!(
            refused.status,
            StatusCode::BAD_REQUEST,
            "{kind}: {:?}",
            refused
        );
        assert_eq!(
            refused.problem().code,
            orch_api_contract::common::ErrorCode::NodeWrongParent
        );
        let problem: Json = serde_json::from_slice(&refused.raw).unwrap();
        assert!(
            serde_json::to_string(&problem)
                .unwrap()
                .contains("move_to_another_org")
        );
        let after = row(&world, kind, id).await;
        assert_eq!(
            after, before,
            "{kind}: a refused cross-root home change must not write"
        );
        let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_kind=$1 AND object_id=$2 AND action='resource.home_changed'")
            .bind(kind).bind(id).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(audits, 0);
    }
}
