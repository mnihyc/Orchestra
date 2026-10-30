//! Replacement enrollment tokens issued only to a machine's owner or the managers of its host.
//!
//! A browser administrator explicitly names themself on a person's machine page before managing it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use orch_api_contract::access::{
    Holder, HolderInput, PageChange, PageChangeAdd, PageChangeRow, PermissionPageGet,
    PermissionPagePreview, PermissionPageUpdate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    MachineEnrollmentAgainTokenCreate, MachineGet, MachineRevoke, MachineRolesSet,
    MachineRolesSetBody, MachineStatus,
};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::Id;
use places::{Who, admin, make_org, person, who};
use serde_json::json;
use support::World;

async fn owned_machine(world: &World, owner: &Who) -> Id {
    let mut conn = world.conn().await;
    let machine = fixtures::insert_persons_machine(&mut conn, owner.id(), "personal laptop")
        .await
        .unwrap();
    machine
}

async fn confirmed_management(world: &World, administrator: &Who, machine: Id) {
    let page = world
        .app
        .call(PermissionPageGet, &[&"machines", &machine])
        .token(&administrator.token)
        .send()
        .await
        .ok();
    let change = PageChange {
        expected_revision: Some(page.revision),
        rows: Some(vec![PageChangeRow {
            permission: "machine.manage".into(),
            profile: None,
            inherit: None,
            locked: None,
            add: Some(vec![PageChangeAdd {
                holder: HolderInput::User {
                    id: administrator.id(),
                },
                options: None,
                expires_at: None,
                note: None,
            }]),
            update: None,
            remove: None,
        }]),
        ..PageChange::default()
    };
    world
        .app
        .call(PermissionPagePreview, &[&"machines", &machine])
        .token(&administrator.token)
        .body(&change)
        .send()
        .await
        .ok();
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE details->'object'->>'id'=$1 AND override",
    )
    .bind(machine.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(before, 0, "reviewing an override grants nothing");
    let page = world
        .app
        .call(PermissionPageUpdate, &[&"machines", &machine])
        .token(&administrator.token)
        .body(&change)
        .send()
        .await
        .ok();
    let row = page
        .rows
        .iter()
        .find(|row| row.permission == "machine.manage")
        .unwrap();
    let named = row
        .entries
        .iter()
        .find(
            |entry| matches!(&entry.holder, Holder::User { user } if user.id == administrator.id()),
        )
        .expect("the machine's page names the administrator");
    let marked: (bool, bool, serde_json::Value, String) = sqlx::query_as("SELECT override,as_admin,details->'object',details->>'permission' FROM audit_log WHERE object_id=$1 AND actor_id=$2 AND action='permission.added'")
        .bind(named.id).bind(administrator.id()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        marked,
        (
            true,
            true,
            json!({"kind":"machine","id":machine}),
            "machine.manage".into()
        )
    );
}

#[tokio::test]
async fn a_bound_replacement_command_keeps_the_owned_connectors_named_directory_and_foreground_mode()
 {
    let world = World::new().await;
    let owner = person(&world, "named-owner").await;
    let mut conn = world.conn().await;
    let machine = fixtures::insert_persons_machine(&mut conn, owner.id(), "named laptop")
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET roles=ARRAY['agent'],facts=jsonb_build_object('installation',jsonb_build_object('data_dir',$2::text,'service',null,'executable','/tmp/connector bin/orch-connector')) WHERE id=$1")
        .bind(machine).bind("/tmp/owner's named machine").execute(&mut *conn).await.unwrap();
    drop(conn);
    let token = world
        .app
        .call(MachineEnrollmentAgainTokenCreate, &[&machine])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert!(token.commands.linux.contains(" --roles 'agent'"));
    assert!(
        token
            .commands
            .linux
            .contains(" --data-dir '/tmp/owner'\\''s named machine'")
    );
    assert!(
        token
            .commands
            .windows
            .contains(" -DataDir '/tmp/owner''s named machine'")
    );
    assert!(
        token
            .commands
            .linux
            .contains(" --no-service && '/tmp/connector bin/orch-connector' --data-dir ")
    );
    let target: (Id, Id) =
        sqlx::query_as("SELECT created_by,again_machine_id FROM enrollment_tokens")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(target, (owner.id(), machine));
}

#[tokio::test]
async fn an_admin_reenrolls_a_persons_machine_after_the_confirmed_override_names_them_and_records_it()
 {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let root = admin(&world, "root").await;
    let off = who(&world, "admin-mode-off", true).await;
    let api_token = admin_support::api_token(&world, &root).await;
    let machine = owned_machine(&world, &ada).await;
    let roles = MachineRolesSetBody {
        roles: vec!["agent".into(), "provider".into()],
    };
    for denied in [&bea.token, &off.token, &api_token, &root.token] {
        let problem = world
            .app
            .call(MachineEnrollmentAgainTokenCreate, &[&machine])
            .token(denied)
            .send()
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::NotFound);
        assert_eq!(
            world
                .app
                .call(MachineRevoke, &[&machine])
                .token(denied)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            world
                .app
                .call(MachineRolesSet, &[&machine])
                .token(denied)
                .body(&roles)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::NotFound
        );
    }
    let before = world
        .app
        .call(MachineGet, &[&machine])
        .token(&root.token)
        .send()
        .await
        .ok();
    assert!(
        before
            .access
            .as_ref()
            .unwrap()
            .permissions
            .contains(&"access.manage".into())
    );
    assert!(
        !before
            .access
            .as_ref()
            .unwrap()
            .permissions
            .contains(&"machine.manage".into())
    );
    let effects: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM enrollment_tokens),(SELECT count(*) FROM commands)",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(effects, (0, 0));
    confirmed_management(&world, &root, machine).await;
    let token = world
        .app
        .call(MachineEnrollmentAgainTokenCreate, &[&machine])
        .token(&root.token)
        .send()
        .await
        .ok();
    assert!(token.commands.linux.contains(&token.token));
    let target: (Id, Id) =
        sqlx::query_as("SELECT created_by, again_machine_id FROM enrollment_tokens")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(target, (root.id(), machine));
}

#[tokio::test]
async fn removing_a_persons_machine_after_an_admin_override_keeps_the_named_grant_and_actor() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let root = admin(&world, "root").await;
    let machine = owned_machine(&world, &owner).await;
    confirmed_management(&world, &root, machine).await;
    let removed = world
        .app
        .call(MachineRevoke, &[&machine])
        .token(&root.token)
        .send()
        .await
        .ok();
    assert_eq!(removed.status, MachineStatus::Revoked);
    let record: (Id, bool) = sqlx::query_as(
        "SELECT actor_id,as_admin FROM audit_log WHERE object_id=$1 AND action='machine.revoked'",
    )
    .bind(machine)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(record, (root.id(), true));
    let page = world
        .app
        .call(PermissionPageGet, &[&"machines", &machine])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert!(
        page.rows
            .iter()
            .find(|row| row.permission == "machine.manage")
            .unwrap()
            .entries
            .iter()
            .any(|entry| matches!(&entry.holder,Holder::User{user} if user.id==root.id()))
    );
}

#[tokio::test]
async fn a_machine_with_a_personal_owner_is_not_system_infrastructure_when_its_home_is_absent() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let root = admin(&world, "root").await;
    let machine = owned_machine(&world, &owner).await;
    sqlx::query("UPDATE machines SET home_id=NULL,org_id=NULL WHERE id=$1")
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let shown = world
        .app
        .call(MachineGet, &[&machine])
        .token(&root.token)
        .send()
        .await
        .ok();
    let access = shown.access.unwrap();
    assert!(access.permissions.contains(&"access.manage".into()));
    assert!(!access.permissions.contains(&"machine.manage".into()));
    assert!(!access.permissions.contains(&"machine.use".into()));
    assert_eq!(
        world
            .app
            .call(MachineEnrollmentAgainTokenCreate, &[&machine])
            .token(&root.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    confirmed_management(&world, &root, machine).await;
}

#[tokio::test]
async fn an_admins_named_machine_grant_still_needs_the_local_role_handover() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let root = admin(&world, "root").await;
    let machine = owned_machine(&world, &owner).await;
    confirmed_management(&world, &root, machine).await;
    orch_db::queries::resources::set_presence(
        &mut *world.conn().await,
        machine,
        true,
        None,
        world.now(),
    )
    .await
    .unwrap();
    let local = orch_link_contract::policy::LocalPolicy::default();
    sqlx::query("UPDATE machines SET roles=ARRAY['agent'],facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1").bind(machine).bind(json!(local)).execute(&world.db.pool).await.unwrap();
    let request = MachineRolesSetBody {
        roles: vec!["agent".into(), "provider".into()],
    };
    let denied = world
        .app
        .call(MachineRolesSet, &[&machine])
        .token(&root.token)
        .body(&request)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::RequestInvalid);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM commands")
            .fetch_one(&world.db.pool)
            .await
            .unwrap(),
        0
    );
    let handed = orch_link_contract::policy::LocalPolicy::resolve(
        &["machine.roles".into()],
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(machine)
        .bind(json!(handed))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let pool = world.db.pool.clone();
    let answer = tokio::spawn(async move {
        // Match the ordinary API machine fixture's ten-second native-response window.
        const POLLS: usize = 400;
        const POLL: std::time::Duration = std::time::Duration::from_millis(25);
        for _ in 0..POLLS {
            if sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE machine_id=$1 AND state IN ('queued','sent','accepted')").bind(machine).bind(json!({"roles":["agent","provider"],"reconnecting":true})).execute(&pool).await.unwrap().rows_affected()>0{return;}
            tokio::time::sleep(POLL).await;
        }
        panic!("the role command was not dispatched");
    });
    let result = world
        .app
        .call(MachineRolesSet, &[&machine])
        .token(&root.token)
        .body(&request)
        .send()
        .await
        .ok();
    answer.await.unwrap();
    assert_eq!(result.requested_roles, request.roles);
    assert!(result.reconnecting);
    let record:(Id,bool)=sqlx::query_as("SELECT actor_id,as_admin FROM audit_log WHERE object_id=$1 AND action='machine.roles_changed'").bind(machine).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(record, (root.id(), true));
}

#[tokio::test]
async fn a_host_replacement_token_needs_manage_on_that_host() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Office", "office").await.base.id;
    let mut conn = world.conn().await;
    let host = fixtures::insert_resource(&mut conn, "host", Some(ada.id()), Some(org), "Node")
        .await
        .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id = $1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let denied = world
        .app
        .call(MachineEnrollmentAgainTokenCreate, &[&machine])
        .token(&bea.token)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("host", host, "live"),
        "any",
        "host.manage",
        ("user", Some(bea.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('installation',jsonb_build_object('data_dir',$2::text,'config_file',$3::text,'service',null,'executable','/usr/local/bin/orch-node')) WHERE id=$1")
        .bind(machine).bind("/tmp/node identity").bind("/tmp/custom node.toml").execute(&world.db.pool).await.unwrap();
    let token = world
        .app
        .call(MachineEnrollmentAgainTokenCreate, &[&machine])
        .token(&bea.token)
        .send()
        .await
        .ok();
    assert!(
        token
            .commands
            .linux
            .contains(" --data-dir '/tmp/node identity'")
    );
    assert!(
        token
            .commands
            .linux
            .contains(" --config '/tmp/custom node.toml'")
    );
    // A host's machine is enrolled again by its node's installer.
    assert!(
        token
            .commands
            .linux
            .contains("/api/v1/downloads/install-node.sh"),
        "{}",
        token.commands.linux
    );
    let target: (Id, Id) =
        sqlx::query_as("SELECT created_by, again_machine_id FROM enrollment_tokens")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(target, (bea.id(), machine));
}
