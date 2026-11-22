//! The ordinary home operation stays inside its current root organization.
use super::*;

async fn native_root(world: &World, owner: &Who, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = new_id();
    orch_db::queries::hierarchy::testing::insert_node(&mut conn, id, "org", None, name, owner.id())
        .await
        .unwrap();
    access::testing::insert_member(&mut conn, id, owner.id(), id)
        .await
        .unwrap();
    access::testing::name_owner(&mut conn, id, ("user", owner.id()), world.now())
        .await
        .unwrap();
    id
}

async fn native_project(world: &World, owner: &Who, root: Id, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = new_id();
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        id,
        "project",
        Some(root),
        name,
        owner.id(),
    )
    .await
    .unwrap();
    orch_authz::pages::start(
        &mut conn,
        orch_access_contract::pages::StoredKind::Node,
        id,
        owner.id(),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
}

async fn home_audits(world: &World, kind: &str, id: Id) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='resource.home_changed' AND object_kind=$1 AND object_id=$2",
    )
    .bind(kind)
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn an_ordinary_home_change_accepts_the_same_root_and_refuses_another_root_without_changes() {
    let world = World::new().await;
    let owner = person(&world, "home-rule-owner").await;
    let source = native_root(&world, &owner, "Source root").await;
    let other = native_root(&world, &owner, "Other root").await;
    let same_home = native_project(&world, &owner, source, "Same root home").await;
    let other_home = native_project(&world, &owner, other, "Other root home").await;
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
        let id = resource(&world, &owner, source, kind, &format!("move-{kind}")).await;
        let original = row(&world, kind, id).await;
        let permitted = home(
            &world,
            &owner.token,
            kind,
            id,
            same_home,
            original["revision"].as_i64(),
        )
        .await;
        assert_eq!(
            permitted.status,
            StatusCode::OK,
            "{kind}: {}",
            String::from_utf8_lossy(&permitted.raw)
        );
        let before = row(&world, kind, id).await;
        assert_eq!(before["org_id"], json!(source), "{kind}");
        assert_eq!(before["owner_id"], original["owner_id"], "{kind}");
        assert_eq!(
            before[if kind == "environment" {
                "home_node_id"
            } else {
                "home_id"
            }],
            json!(same_home),
            "{kind}"
        );
        let audits = home_audits(&world, kind, id).await;
        let refused = home(
            &world,
            &owner.token,
            kind,
            id,
            other_home,
            before["revision"].as_i64(),
        )
        .await;
        assert_eq!(refused.status, StatusCode::CONFLICT, "{kind}");
        let problem = refused.problem();
        assert_eq!(
            problem.code,
            orch_api_contract::common::ErrorCode::NodeWrongParent,
            "{kind}"
        );
        assert_eq!(problem.extra["next_step"], "move_to_another_org");
        assert_eq!(problem.extra["field"], "home_id");
        assert_eq!(row(&world, kind, id).await, before, "{kind}");
        assert_eq!(home_audits(&world, kind, id).await, audits, "{kind}");
    }
}

#[tokio::test]
async fn a_server_hosts_new_home_requires_confirmation_when_the_same_role_entry_reaches_other_people()
 {
    let world = World::new().await;
    let owner = person(&world, "server-home-owner").await;
    let other = person(&world, "server-home-colleague").await;
    let root = native_root(&world, &owner, "Server organization").await;
    let project = native_project(&world, &owner, root, "Colleague project").await;
    let id = resource(&world, &owner, root, "host", "Server host").await;
    let mut conn = world.conn().await;
    access::testing::insert_member(&mut conn, root, other.id(), root)
        .await
        .unwrap();
    access::testing::name_owner(&mut conn, project, ("user", other.id()), world.now())
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET bundled_with_server=true WHERE id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap();
    access::testing::set_row(
        &mut conn,
        &access::PageKey::new("host", id, "live"),
        "any",
        "host.use",
        true,
        false,
    )
    .await
    .unwrap();
    access::testing::add_entry(
        &mut conn,
        &access::PageKey::new("system", access::SYSTEM_ID, "defaults"),
        "any",
        "host.use",
        ("role", None, Some("admin")),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let original = row(&world, "host", id).await;
    let body = ResourceHomeSetBody {
        home_id: project,
        expected_revision: original["revision"].as_i64(),
    };
    let refused = world
        .app
        .call(ObjectHomeSet, &[&"hosts", &id])
        .token(&owner.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        orch_api_contract::common::ErrorCode::ConflictConfirmationRequired
    );
    assert_eq!(
        row(&world, "host", id).await,
        original,
        "the unconfirmed role-context change rolls back"
    );
    let proof = refused.extra["server_host_use"]["confirmation"]
        .as_str()
        .unwrap();
    world
        .app
        .call(ObjectHomeSet, &[&"hosts", &id])
        .token(&owner.token)
        .header("orch-server-host-use-confirmed", proof)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(row(&world, "host", id).await["home_id"], json!(project));
}
