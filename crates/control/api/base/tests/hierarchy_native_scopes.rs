//! Hierarchy readers and creators keep native destinations, windows and optional content.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod support;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::nodes::{OrgTreeGet, OrgTreeGetQuery};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;
async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native hierarchy",
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
fn rule(kind: &str, id: Id, reach: &str, operation: &str, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":[action]}]})
}
fn value(reply: orch_api_core::testing::RawReply) -> Json {
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}
#[tokio::test]
async fn tree_scope_filters_before_the_native_window_without_borrowing_another_settings_operation()
{
    let world = World::new().await;
    let owner = places::person(&world, "treeowner").await;
    let org = places::make_org(&world, &owner, "Tree source", "tree-source")
        .await
        .base
        .id;
    for i in 0..5 {
        places::make_project(&world, &owner, org, &format!("Earlier {i}")).await;
    }
    let selected = places::make_project(&world, &owner, org, "Selected leaf")
        .await
        .base
        .id;
    let query = OrgTreeGetQuery {
        limit: Some(1),
        ..Default::default()
    };
    let ordinary = value(
        world
            .app
            .call(OrgTreeGet, &[&org])
            .token(&owner.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["nodes"].as_array().unwrap().len(), 1);
    assert!(ordinary["next_cursor"].is_string());
    let scoped = key(
        &world,
        owner.id(),
        vec![rule("project", selected, "self", "OrgTreeGet", "read")],
    )
    .await;
    let result = value(
        world
            .app
            .call(OrgTreeGet, &[&org])
            .token(&scoped)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(result["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(result["nodes"][0]["id"], json!(selected));
    assert_eq!(result["nodes"][0]["name"], "Selected leaf");
    assert!(result["next_cursor"].is_null());
    assert!(result["settings"].is_null());
    let whole = key(
        &world,
        owner.id(),
        vec![rule("org", org, "subtree", "OrgTreeGet", "read")],
    )
    .await;
    let ordinary = value(
        world
            .app
            .call(OrgTreeGet, &[&org])
            .token(&owner.token)
            .query(&OrgTreeGetQuery::default())
            .send_bytes()
            .await,
    );
    let result = value(
        world
            .app
            .call(OrgTreeGet, &[&org])
            .token(&whole)
            .query(&OrgTreeGetQuery::default())
            .send_bytes()
            .await,
    );
    assert!(ordinary["settings"].is_array());
    assert_eq!(
        result, ordinary,
        "one native tree/settings producer for both surfaces"
    );
}
#[tokio::test]
async fn incremental_tree_scope_keeps_native_gone_markers_and_never_names_unselected_ancestors() {
    let world = World::new().await;
    let owner = places::person(&world, "treechanges").await;
    let org = places::make_org(&world, &owner, "Change source", "change-source")
        .await
        .base
        .id;
    let project = places::make_project(&world, &owner, org, "Kept project")
        .await
        .base
        .id;
    let scoped = key(
        &world,
        owner.id(),
        vec![rule("project", project, "self", "OrgTreeGet", "read")],
    )
    .await;
    let query = OrgTreeGetQuery {
        since: Some(0),
        ..Default::default()
    };
    let original = value(
        world
            .app
            .call(OrgTreeGet, &[&org])
            .token(&scoped)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(original["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(original["nodes"][0]["id"], json!(project));
    assert!(original["settings"].is_null());
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(project)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let gone = value(
        world
            .app
            .call(OrgTreeGet, &[&org])
            .token(&scoped)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(gone["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(gone["nodes"][0]["id"], json!(project));
    assert_eq!(gone["nodes"][0]["deleted"], true);
    assert_eq!(gone["nodes"][0]["name"], "");
    assert!(
        !serde_json::to_string(&gone)
            .unwrap()
            .contains("Change source")
    );
}

#[tokio::test]
async fn root_creation_uses_the_actual_account_and_consumes_one_native_invitation_with_a_receipt() {
    use orch_api_contract::nodes::{OrgCreate, OrgCreateBody};
    let world = World::new().await;
    let owner = places::person(&world, "rootmaker").await;
    places::invite_to_make_org(&world, &owner).await;
    let scoped = key(
        &world,
        owner.id(),
        vec![rule("account", owner.id(), "self", "OrgCreate", "manage")],
    )
    .await;
    let result = value(
        world
            .app
            .call(OrgCreate, &[])
            .token(&scoped)
            .body(&OrgCreateBody {
                name: "New keyed org".into(),
                slug: "new-keyed-org".into(),
            })
            .send_bytes()
            .await,
    );
    assert_eq!(result["projection"], "receipt");
    assert_eq!(result["kind"], "org");
    assert!(result.get("name").is_none());
    let id: Id = serde_json::from_value(result["node_id"].clone()).unwrap();
    let (name, parent): (String, Option<Id>) =
        sqlx::query_as("SELECT name,parent_id FROM nodes WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(name, "New keyed org");
    assert!(parent.is_none());
    let general: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nodes WHERE parent_id=$1 AND kind='project' AND name='General'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(general, 1);
    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM org_members WHERE user_id=$1 AND org_id=$2)",
    )
    .bind(owner.id())
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert!(member);
    assert_eq!(
        world
            .app
            .call(OrgCreate, &[])
            .token(&scoped)
            .body(&OrgCreateBody {
                name: "Second org".into(),
                slug: "second-keyed-org".into()
            })
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::OrgCreationDisabled
    );
    places::invite_to_make_org(&world, &owner).await;
    let wrong = key(
        &world,
        owner.id(),
        vec![rule("account", new_id(), "self", "OrgCreate", "manage")],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(OrgCreate, &[])
            .token(&wrong)
            .body(&OrgCreateBody {
                name: "Wrong account".into(),
                slug: "wrong-account-org".into()
            })
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    let still_invited: bool =
        sqlx::query_scalar("SELECT org_invited_at IS NOT NULL FROM users WHERE id=$1")
            .bind(owner.id())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(still_invited);
}
#[tokio::test]
async fn child_creation_keeps_its_native_atomic_general_and_separates_parent_manage_from_created_content()
 {
    use orch_api_contract::nodes::{OrgChildCreate, OrgChildCreateBody};
    let world = World::new().await;
    let owner = places::person(&world, "departmentmaker").await;
    let org = places::make_org(&world, &owner, "Departments", "departments")
        .await
        .base
        .id;
    let manage = key(
        &world,
        owner.id(),
        vec![rule("org", org, "self", "OrgChildCreate", "manage")],
    )
    .await;
    let body = OrgChildCreateBody {
        name: "Keyed child".into(),
        slug: "keyed-child".into(),
        description: Some("Private child body".into()),
        general: Some(true),
        access: None,
    };
    let receipt = value(
        world
            .app
            .call(OrgChildCreate, &[&org])
            .token(&manage)
            .body(&body)
            .send_bytes()
            .await,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert!(receipt.get("name").is_none());
    let id: Id = serde_json::from_value(receipt["node_id"].clone()).unwrap();
    let general: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nodes WHERE parent_id=$1 AND kind='project' AND name='General'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(general, 1);
    let full = key(
        &world,
        owner.id(),
        vec![
            rule("org", org, "self", "OrgChildCreate", "manage"),
            rule("org", org, "subtree", "OrgChildCreate", "read"),
        ],
    )
    .await;
    let result = value(
        world
            .app
            .call(OrgChildCreate, &[&org])
            .token(&full)
            .body(&OrgChildCreateBody {
                name: "Readable child".into(),
                slug: "readable-child".into(),
                ..body.clone()
            })
            .send_bytes()
            .await,
    );
    assert_eq!(result["name"], "Readable child");
    assert_eq!(result["address"], "departments.readable-child");
    assert_eq!(result["project_count"], 1);
    let collision = world
        .app
        .call(OrgChildCreate, &[&org])
        .token(&manage)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(
        collision.problem().code,
        orch_api_contract::common::ErrorCode::ConflictExists
    );
    assert!(collision.problem().extra.get("existing").is_none());
    assert!(!String::from_utf8_lossy(&collision.raw).contains("Keyed child"));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nodes WHERE parent_id=$1 AND kind='org' AND name='Keyed child'",
    )
    .bind(org)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "the conflict never repeats the compound creation");
}
const WAIT_NATIVE: std::time::Duration = std::time::Duration::from_secs(30);
#[tokio::test]
async fn root_creation_rechecks_the_key_after_the_native_invitation_row_wait_before_any_effect() {
    use orch_api_contract::nodes::{OrgCreate, OrgCreateBody};
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "waitmaker").await;
        places::invite_to_make_org(&world, &owner).await;
        let scoped = key(
            &world,
            owner.id(),
            vec![rule("account", owner.id(), "self", "OrgCreate", "manage")],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        // SHARE coexists with identity KEY SHARE but blocks the native invitation's NO KEY UPDATE.
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR SHARE")
            .bind(owner.id())
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let action = tokio::spawn(async move {
            let request = app
                .call(OrgCreate, &[])
                .token(&scoped)
                .body(&OrgCreateBody {
                    name: "Waited org".into(),
                    slug: "waited-org".into(),
                });
            request.send_bytes().await
        });
        let reached=tokio::time::timeout(WAIT_NATIVE,async {loop {
            if action.is_finished() {break false;}
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT org_invited_by,org_invited_at FROM users%FOR NO KEY UPDATE%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked {break true;}tokio::task::yield_now().await;
        }}).await.unwrap();
        assert!(reached, "the actual native invitation row must be reached");
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT_NATIVE, action)
            .await
            .unwrap()
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE name='Waited org'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        if expire {
            assert!(matches!(reply.status.as_u16(), 401 | 403));
            assert_eq!(count, 0);
            let invited: bool =
                sqlx::query_scalar("SELECT org_invited_at IS NOT NULL FROM users WHERE id=$1")
                    .bind(owner.id())
                    .fetch_one(&world.db.pool)
                    .await
                    .unwrap();
            assert!(invited);
        } else {
            let receipt = value(reply);
            assert_eq!(receipt["projection"], "receipt");
            assert_eq!(count, 1);
        }
    }
}
