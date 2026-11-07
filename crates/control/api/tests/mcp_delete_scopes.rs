//! Native MCP deletion, its actual governors and atomically accepted private-copy cleanup.
#![recursion_limit = "256"]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ResourceKind};
use orch_api_contract::resources::McpServerDelete;
use orch_api_core::{
    dispatch::StoredCommand,
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
    thread_access::AuthzAccess,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{
    identity, link,
    resources::{
        access_fixtures as fixtures,
        mcp::{self as rows, McpRow},
    },
};
use orch_engine::commands::CommandAuthority;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
async fn key(world: &World, user: Id, rules: Vec<Json>, seconds: i64) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "MCP deletion",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::seconds(seconds)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["manage"]}]})
}
async fn start(world: &World, kind: ResourceKind, id: Id, owner: Id) {
    start_resource(
        &mut *world.conn().await,
        ObjectRef {
            kind: kind.into(),
            id,
        },
        owner,
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
}
async fn server(world: &World, owner: Id, environment: Option<Id>, machine: Option<Id>) -> Id {
    let id = new_id();
    rows::insert(&mut *world.conn().await,&McpRow {
        id,name:"native-tool".into(),description:"Private retained definition".into(),org_id:None,home_id:None,
        owner_id:Some(owner),machine_id:machine,environment_id:environment,
        spec:match machine {
            Some(_)=>json!({"transport":"stdio","imported":{"source":"fixture","scope":"user","name":"native-tool","digest":"copy-one"}}),
            None=>json!({"transport":"stdio","command":["private-command"]}),
        },imported:machine.is_some(),enabled:true,revision:1,
    },world.now()).await.unwrap();
    if environment.is_none() {
        start(world, ResourceKind::McpServer, id, owner).await;
    }
    id
}
async fn delete(world: &World, token: &str, id: Id) -> RawReply {
    world
        .app
        .call(McpServerDelete, &[&id])
        .token(token)
        .send_bytes()
        .await
}
async fn live(world: &World, id: Id) -> bool {
    sqlx::query_scalar("SELECT deleted_at IS NULL FROM mcp_servers WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
fn receipt(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(
        reply.raw.is_empty(),
        "Manage-only deletion returns no retained definition"
    );
}
async fn hosted(world: &World, owner: Id) -> (Id, Id) {
    let host = fixtures::insert_resource(
        &mut *world.conn().await,
        "host",
        Some(owner),
        None,
        "Copy host",
    )
    .await
    .unwrap();
    start(world, ResourceKind::Host, host, owner).await;
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=true,roles=ARRAY['node','environment'] WHERE id=$1")
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    (host, machine)
}
async fn saved(world: &World, server: Id) -> link::CommandWithAuthority {
    let id: Id = sqlx::query_scalar(
        "SELECT id FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
    )
    .bind(server.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    link::command_with_authority(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
}
async fn current(world: &World, saved: &link::CommandWithAuthority) -> bool {
    let authority: CommandAuthority =
        serde_json::from_value(saved.authority.clone().unwrap()).unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    let allowed = authority
        .current_callers(
            &mut tx,
            &saved.command,
            world.app.app.clock.as_ref(),
            &AuthzAccess::new(world.app.app.clock.clone()),
        )
        .await
        .unwrap()
        .is_some();
    tx.commit().await.unwrap();
    allowed
}

#[tokio::test]
async fn mcp_delete_keeps_manage_only_receipts_exact_operations_and_current_ordinary_authority() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = server(&world, company.boss.id(), None, None).await;
    for (user, rules) in [
        (
            company.boss.id(),
            vec![rule("McpServerUpdate", "mcp_server", id)],
        ),
        (
            company.boss.id(),
            vec![rule("McpServerDelete", "environment", id)],
        ),
        (
            company.boss.id(),
            vec![rule("McpServerDelete", "mcp_server", new_id())],
        ),
        (
            company.stranger.id(),
            vec![rule("McpServerDelete", "mcp_server", id)],
        ),
    ] {
        let denied = key(&world, user, rules, 3600).await;
        assert_eq!(
            delete(&world, &denied.secret, id).await.status,
            StatusCode::FORBIDDEN
        );
        assert!(live(&world, id).await);
    }
    let accepted = key(
        &world,
        company.boss.id(),
        vec![rule("McpServerDelete", "mcp_server", id)],
        3600,
    )
    .await;
    receipt(delete(&world, &accepted.secret, id).await);
    assert!(!live(&world, id).await);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.forget'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn mcp_delete_of_an_own_server_uses_its_real_environment_governor_and_business_family() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let environment = fixtures::insert_resource(
        &mut *world.conn().await,
        "environment",
        Some(company.boss.id()),
        Some(company.acme),
        "Owner environment",
    )
    .await
    .unwrap();
    start(
        &world,
        ResourceKind::Environment,
        environment,
        company.boss.id(),
    )
    .await;
    let id = server(&world, company.boss.id(), Some(environment), None).await;
    let preset_key = key(
        &world,
        company.boss.id(),
        vec![rule("McpServerDelete", "mcp_server", id)],
        3600,
    )
    .await;
    assert_eq!(
        delete(&world, &preset_key.secret, id).await.status,
        StatusCode::FORBIDDEN
    );
    assert!(live(&world, id).await);
    let mut own = rule("McpServerDelete", "environment", environment);
    own["resources"] = json!(["mcp_server"]);
    let accepted = key(&world, company.boss.id(), vec![own], 3600).await;
    receipt(delete(&world, &accepted.secret, id).await);
    assert!(!live(&world, id).await);
}

#[tokio::test]
async fn mcp_delete_binds_each_actual_copy_supplier_and_keeps_the_host_governor() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = hosted(&world, company.boss.id()).await;
    let id = server(&world, company.boss.id(), None, Some(machine)).await;
    let primary = rule("McpServerDelete", "mcp_server", id);
    for rules in [
        vec![primary.clone()],
        vec![primary.clone(), rule("McpServerDelete", "machine", machine)],
    ] {
        let denied = key(&world, company.boss.id(), rules, 3600).await;
        assert_eq!(
            delete(&world, &denied.secret, id).await.status,
            StatusCode::FORBIDDEN
        );
        assert!(live(&world, id).await);
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.forget'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
    let mut copy = rule("McpServerDelete", "host", host);
    copy["resources"] = json!(["machine"]);
    let accepted = key(&world, company.boss.id(), vec![primary, copy], 3600).await;
    let answer = resources::answer(&world, machine, json!({}));
    receipt(delete(&world, &accepted.secret, id).await);
    answer.await.unwrap();
    let saved = saved(&world, id).await;
    assert_eq!(saved.command.body, json!({"server_id":id}));
    let authority = saved.authority.as_ref().unwrap();
    assert_eq!(authority["kind"], "mcp_deletion_cleanup");
    assert_eq!(
        authority["authority"]["origin"]["operation"],
        "McpServerDelete"
    );
    assert!(authority.to_string().contains(&accepted.id.to_string()));
    assert!(!authority.to_string().contains(&accepted.secret));
    assert!(current(&world, &saved).await);
}

#[tokio::test]
async fn mcp_delete_keeps_offline_copies_out_of_its_native_forget_selection() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (_, machine) = hosted(&world, company.boss.id()).await;
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let id = server(&world, company.boss.id(), None, Some(machine)).await;
    let accepted = key(
        &world,
        company.boss.id(),
        vec![rule("McpServerDelete", "mcp_server", id)],
        3600,
    )
    .await;
    receipt(delete(&world, &accepted.secret, id).await);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.forget'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn mcp_delete_refuses_expiry_after_the_real_definition_lock_without_deletion_or_cleanup() {
    for expire in [false, true] {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let id = server(&world, company.boss.id(), None, None).await;
        let accepted = key(
            &world,
            company.boss.id(),
            vec![rule("McpServerDelete", "mcp_server", id)],
            3600,
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        rows::lock_definition(&mut blocker, id).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let request = tokio::spawn(async move {
            let call = app.call(McpServerDelete, &[&id]).token(&accepted.secret);
            call.send_bytes().await
        });
        tokio::time::timeout(WAIT,async {
            loop {
                assert!(!request.is_finished(),"the native delete must reach its definition lock");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%mcp.definition:%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}tokio::task::yield_now().await;
            }
        }).await.expect("the actual native definition-lock query is waiting");
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(live(&world, id).await);
        } else {
            receipt(reply);
            assert!(!live(&world, id).await);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.forget'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn committed_mcp_cleanup_is_exact_reducing_work_and_cannot_admit_a_fresh_command() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (host, machine) = hosted(&world, company.boss.id()).await;
    let id = server(&world, company.boss.id(), None, Some(machine)).await;
    let accepted = key(
        &world,
        company.boss.id(),
        vec![
            rule("McpServerDelete", "mcp_server", id),
            rule("McpServerDelete", "host", host),
        ],
        1,
    )
    .await;
    let answer = resources::answer(&world, machine, json!({}));
    receipt(delete(&world, &accepted.secret, id).await);
    answer.await.unwrap();
    let saved = saved(&world, id).await;
    assert!(current(&world, &saved).await);
    world.advance(chrono::Duration::seconds(2));
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(accepted.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        current(&world, &saved).await,
        "the exact removal of an already deleted private copy is owned cleanup, not fresh credential authority"
    );
    let fresh = new_id();
    let authority: CommandAuthority =
        serde_json::from_value(saved.authority.clone().unwrap()).unwrap();
    let refused = world
        .app
        .app
        .link
        .dispatcher()
        .issue_stored(StoredCommand {
            id: Some(fresh),
            machine,
            kind: saved.command.kind.clone(),
            key: saved.command.serial_key().map(str::to_owned),
            deadline: Some(Time::new(saved.command.deadline_at)),
            body: saved.command.body.clone(),
            authority: Some(authority),
        })
        .await;
    assert!(refused.is_err());
    assert!(
        link::command(&mut *world.conn().await, fresh)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("UPDATE mcp_servers SET deleted_at=NULL WHERE id=$1")
        .bind(id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        !current(&world, &saved).await,
        "an active definition cannot borrow its old deletion receipt"
    );
}

#[tokio::test]
async fn mcp_delete_serializes_its_actual_secret_footprint_before_any_deletion_or_native_queue() {
    for expire in [false, true] {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let (host, machine) = hosted(&world, company.boss.id()).await;
        let id = server(&world, company.boss.id(), None, Some(machine)).await;
        let sealed = orch_db::secrets::MasterKey::generate()
            .seal(
                b"fixture value",
                &orch_api_core::routes::resources::sealing::binding("mcp_server", id, "API_KEY"),
            )
            .unwrap();
        orch_db::queries::settings::put_secret(
            &mut *world.conn().await,
            "mcp_server",
            id,
            "API_KEY",
            &sealed,
            Some(company.boss.id()),
            world.now(),
        )
        .await
        .unwrap();
        let accepted = key(
            &world,
            company.boss.id(),
            vec![
                rule("McpServerDelete", "mcp_server", id),
                rule("McpServerDelete", "host", host),
            ],
            3600,
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query(
            "SELECT id FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let request = tokio::spawn(async move {
            let call = app.call(McpServerDelete, &[&id]).token(&accepted.secret);
            call.send_bytes().await
        });
        tokio::time::timeout(WAIT,async {
            loop {
                assert!(!request.is_finished(),"the native deletion must serialize its actual secret before writing");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT name FROM secrets%FOR UPDATE%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked {break;}tokio::task::yield_now().await;
            }
        }).await.expect("the actual native secret source lock is waiting");
        assert!(live(&world, id).await);
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.forget'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        let answer = if expire {
            None
        } else {
            Some(resources::answer(&world, machine, json!({})))
        };
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1",
        )
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(live(&world, id).await);
            assert_eq!(remaining, 1);
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.forget'")
                    .fetch_one(&world.db.pool)
                    .await
                    .unwrap();
            assert_eq!(count, 0);
        } else {
            receipt(reply);
            assert!(!live(&world, id).await);
            assert_eq!(remaining, 0);
            answer.unwrap().await.unwrap();
            assert!(current(&world, &saved(&world, id).await).await);
        }
    }
}
