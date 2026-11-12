//! Three stored machine readers keep exact scope and their existing ordinary visibility.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::machine_programs::MachineProgramsCommand;
use orch_api_contract::resources::{
    MachineExecutorList, MachineGet, MachinePolicyGet, MachineProgramsChange, MachineProgramsGet,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_link_contract::policy::LocalPolicy;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

#[derive(Clone, Copy, Debug)]
enum Reader {
    Executors,
    Programs,
    Policy,
}
const READERS: [Reader; 3] = [Reader::Executors, Reader::Programs, Reader::Policy];
impl Reader {
    fn operation(self) -> &'static str {
        match self {
            Self::Executors => "MachineExecutorList",
            Self::Programs => "MachineProgramsGet",
            Self::Policy => "MachinePolicyGet",
        }
    }
    async fn read(self, world: &World, token: &str, machine: Id) -> RawReply {
        match self {
            Self::Executors => {
                world
                    .app
                    .call(MachineExecutorList, &[&machine])
                    .token(token)
                    .send_bytes()
                    .await
            }
            Self::Programs => {
                world
                    .app
                    .call(MachineProgramsGet, &[&machine])
                    .token(token)
                    .send_bytes()
                    .await
            }
            Self::Policy => {
                world
                    .app
                    .call(MachinePolicyGet, &[&machine])
                    .token(token)
                    .send_bytes()
                    .await
            }
        }
    }
}
struct Key {
    id: Id,
    secret: String,
}
async fn issue(world: &World, user: Id, rules: Option<Vec<Json>>) -> Key {
    let scope = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let scopes = if scope.is_none() {
        vec!["*".to_owned()]
    } else {
        Vec::new()
    };
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Machine leaf reader control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn rule(reader: Reader, kind: &str, id: Id, reach: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":["machine"],
        "allow":[{"operation":reader.operation(),"actions":["read"]}]})
}
fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn refused(reply: RawReply, status: StatusCode) {
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
}

// Full content must match; only these existing capability projections may narrow for a key.
fn same_content(reader: Reader, key: Json, ordinary: &Json) {
    let mut key = key;
    let mut ordinary = ordinary.clone();
    match reader {
        Reader::Executors => {}
        Reader::Programs => {
            let allowed = key["can_manage"].as_bool().unwrap();
            let ordinary_allowed = ordinary["can_manage"].as_bool().unwrap();
            assert!(
                !allowed || ordinary_allowed,
                "a key cannot manufacture machine management"
            );
            key.as_object_mut().unwrap().remove("can_manage");
            ordinary.as_object_mut().unwrap().remove("can_manage");
        }
        Reader::Policy => {
            let key_rows = key["keys"].as_array_mut().unwrap();
            let ordinary_rows = ordinary["keys"].as_array_mut().unwrap();
            assert_eq!(key_rows.len(), ordinary_rows.len());
            for (key, ordinary) in key_rows.iter_mut().zip(ordinary_rows) {
                assert_eq!(key["key"], ordinary["key"]);
                assert!(
                    !key["writable"].as_bool().unwrap() || ordinary["writable"].as_bool().unwrap(),
                    "a key cannot manufacture a writable policy row"
                );
                key.as_object_mut().unwrap().remove("writable");
                ordinary.as_object_mut().unwrap().remove("writable");
            }
        }
    }
    assert_eq!(key, ordinary);
}
async fn machine(world: &World, owner: Id) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_persons_machine(&mut conn, owner, "Stored leaf machine")
        .await
        .unwrap();
    let mut policy = LocalPolicy::default();
    policy.drivers.configurations.insert(
        "fixture".into(),
        json!({"socket":"/fixture/private-driver.sock"}),
    );
    sqlx::query("UPDATE machines SET roles=ARRAY['environment','agent','provider'],facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(id).bind(serde_json::to_value(policy).unwrap()).execute(&mut *conn).await.unwrap();
    id
}
async fn hosted(world: &World, company: &resources::Company, share: bool) -> (Id, Id) {
    let physical = machine(world, company.boss.id()).await;
    sqlx::query(
        "UPDATE machines SET roles=ARRAY['node','environment','agent','provider'] WHERE id=$1",
    )
    .bind(physical)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let host = new_id();
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO hosts(id,machine_id,owner_id,home_id,org_id,name) SELECT $1,$2,$3,id,org_id,'Leaf facade host' FROM nodes WHERE id=$4")
        .bind(host).bind(physical).bind(company.boss.id()).bind(company.acme).execute(&mut *conn).await.unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id: host,
        },
        company.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    if share {
        resources::name_person(world, "host", host, "host.use", company.viewer.id()).await;
    }
    (physical, host)
}
async fn no_commands(world: &World) {
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        commands, 0,
        "stored reads and refused mutations issue no native command"
    );
}

async fn exact_leaf_read(reader: Reader) {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let personal = machine(&world, company.boss.id()).await;
    let (physical, host) = hosted(&world, &company, true).await;
    for (machine, target, kind, actor) in [
        (personal, personal, "machine", &company.boss),
        (physical, host, "host", &company.viewer),
    ] {
        let legacy = issue(&world, actor.id(), None).await;
        let ordinary = shown(reader.read(&world, &actor.token, machine).await);
        assert_eq!(
            shown(reader.read(&world, &legacy.secret, machine).await),
            ordinary,
            "equal ordinary authority retains legacy full projection"
        );
        let key = issue(
            &world,
            actor.id(),
            Some(vec![rule(reader, kind, target, "self")]),
        )
        .await;
        same_content(
            reader,
            shown(reader.read(&world, &key.secret, machine).await),
            &ordinary,
        );
        refused(
            world
                .app
                .call(MachineProgramsChange, &[&machine])
                .token(&key.secret)
                .body(&MachineProgramsCommand::Reload)
                .send_bytes()
                .await,
            StatusCode::FORBIDDEN,
        );
    }
    no_commands(&world).await;
}

#[tokio::test]
async fn executor_leaf_read_keeps_personal_and_hosted_content_without_native_work() {
    exact_leaf_read(Reader::Executors).await;
}

#[tokio::test]
async fn programs_leaf_read_keeps_personal_and_hosted_content_without_native_work() {
    exact_leaf_read(Reader::Programs).await;
}

#[tokio::test]
async fn policy_leaf_read_keeps_personal_and_hosted_content_without_native_work() {
    exact_leaf_read(Reader::Policy).await;
}

#[tokio::test]
async fn leaf_admission_does_not_borrow_another_operation_resource_action_or_subject() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let personal = machine(&world, company.boss.id()).await;
    let other = machine(&world, company.boss.id()).await;
    for reader in READERS {
        shown(reader.read(&world, &company.boss.token, personal).await);
        let correct = rule(reader, "machine", personal, "self");
        let mut operation = correct.clone();
        operation["allow"][0]["operation"] = json!("MachineGet");
        let mut resource = correct.clone();
        resource["resources"] = json!(["host"]);
        let mut action = correct.clone();
        action["allow"][0]["actions"] = json!(["write"]);
        for asked in [
            operation,
            resource,
            action,
            rule(reader, "machine", other, "self"),
        ] {
            // Stored structurally valid restrictions still cannot lend a different facet/action.
            let key = issue(&world, company.boss.id(), Some(vec![asked])).await;
            refused(
                reader.read(&world, &key.secret, personal).await,
                StatusCode::FORBIDDEN,
            );
        }
        refused(
            reader.read(&world, &company.stranger.token, personal).await,
            StatusCode::NOT_FOUND,
        );
        let stranger = issue(&world, company.stranger.id(), Some(vec![correct])).await;
        refused(
            reader.read(&world, &stranger.secret, personal).await,
            StatusCode::NOT_FOUND,
        );
    }
    no_commands(&world).await;
}

#[tokio::test]
async fn hosted_leaf_targets_follow_actual_host_ancestry_and_current_key_restrictions() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (physical, host) = hosted(&world, &company, true).await;
    for reader in READERS {
        let ordinary = shown(reader.read(&world, &company.viewer.token, physical).await);
        for asked in [
            rule(reader, "host", host, "self"),
            rule(reader, "org", company.acme, "subtree"),
        ] {
            let key = issue(&world, company.viewer.id(), Some(vec![asked])).await;
            same_content(
                reader,
                shown(reader.read(&world, &key.secret, physical).await),
                &ordinary,
            );
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key.id)
                .bind(json!(KeyScope::default()))
                .execute(&world.db.pool)
                .await
                .unwrap();
            refused(
                reader.read(&world, &key.secret, physical).await,
                StatusCode::FORBIDDEN,
            );
        }
        for asked in [
            rule(reader, "machine", physical, "self"),
            rule(reader, "org", company.acme, "self"),
        ] {
            let key = issue(&world, company.viewer.id(), Some(vec![asked])).await;
            refused(
                reader.read(&world, &key.secret, physical).await,
                StatusCode::FORBIDDEN,
            );
        }
        let key = issue(
            &world,
            company.viewer.id(),
            Some(vec![rule(reader, "host", host, "self")]),
        )
        .await;
        identity::revoke_api_token(
            &mut *world.conn().await,
            key.id,
            company.viewer.id(),
            world.now(),
        )
        .await
        .unwrap();
        refused(
            reader.read(&world, &key.secret, physical).await,
            StatusCode::UNAUTHORIZED,
        );
    }
    no_commands(&world).await;
}

#[tokio::test]
async fn policy_section_read_preserves_environment_manager_access_without_machine_view() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let (physical, host) = hosted(&world, &company, false).await;
    let mut conn = world.conn().await;
    let environment = fixtures::insert_resource(
        &mut conn,
        "environment",
        Some(company.boss.id()),
        Some(company.acme),
        "Managed work",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE environments SET placement='connected',template_id=NULL,machine_id=$2 WHERE id=$1",
    )
    .bind(environment)
    .bind(physical)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    resources::name_person(
        &world,
        "environment",
        environment,
        "environment.manage",
        company.viewer.id(),
    )
    .await;
    let ordinary = shown(
        Reader::Policy
            .read(&world, &company.viewer.token, physical)
            .await,
    );
    let rows = ordinary["keys"].as_array().unwrap();
    assert!(rows.iter().any(|row| row["key"] == "environment.terminal"));
    assert!(rows.iter().all(|row| row["writable"] == false));
    assert!(
        !rows
            .iter()
            .any(|row| row["key"] == "policy.delegated" || row["key"] == "provider.upstreams")
    );
    assert!(rows.iter().any(|row| row["key"] == "drivers"
        && row["value"]["configurations"]["fixture"]["socket"] == "/fixture/private-driver.sock"));
    refused(
        world
            .app
            .call(MachineGet, &[&physical])
            .token(&company.viewer.token)
            .send_bytes()
            .await,
        StatusCode::NOT_FOUND,
    );
    let key = issue(
        &world,
        company.viewer.id(),
        Some(vec![rule(Reader::Policy, "host", host, "self")]),
    )
    .await;
    same_content(
        Reader::Policy,
        shown(Reader::Policy.read(&world, &key.secret, physical).await),
        &ordinary,
    );
    refused(
        world
            .app
            .call(MachineProgramsChange, &[&physical])
            .token(&key.secret)
            .body(&MachineProgramsCommand::Reload)
            .send_bytes()
            .await,
        StatusCode::FORBIDDEN,
    );
    no_commands(&world).await;
}
