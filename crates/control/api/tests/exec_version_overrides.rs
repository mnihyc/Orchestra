//! Environment consent is a human manager's decision for current confirmed program facts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;
use support::FullTokenReply as _;

use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate, SensitiveProof};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{
    OrgServiceAccountCreate, OrgServiceAccountCreateBody, ServiceAccountTokenCreate,
};
use orch_api_contract::environment::{
    Environment, EnvironmentGet, EnvironmentUpdate, EnvironmentUpdateBody,
};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const KEY: &str = "exec.version_overrides";
const PROGRAM: &str = "/controlled/bin/codex";

fn update(current: &Environment, overrides: Option<Json>) -> EnvironmentUpdateBody {
    let mut config = current.config.clone();
    if let Some(overrides) = overrides {
        config[KEY] = overrides;
    } else {
        config.as_object_mut().unwrap().remove(KEY);
    }
    EnvironmentUpdateBody {
        data_sources: None,
        name: None,
        config: Some(config),
        remote_write: None,
        terminal: None,
        browser: None,
        vnc: None,
        home_node_id: None,
        expected_revision: current.revision,
    }
}

fn named_token(name: &str, scopes: &[&str]) -> ApiTokenCreateBody {
    ApiTokenCreateBody {
        rules: None,
        name: name.into(),
        scopes: Some(scopes.iter().map(|scope| (*scope).into()).collect()),
        node_id: None,
        expires_at: None,
        proof: Some(SensitiveProof {
            password: Some(support::PASSWORD.into()),
            ..Default::default()
        }),
    }
}

async fn environment(world: &World, owner: Id, home: Id) -> (Id, Id) {
    let mut conn = world.conn().await;
    let id =
        fixtures::insert_environment(&mut conn, owner, Some(home), "Connected tools", "connected")
            .await
            .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id,
        },
        owner,
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM environments WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
        .bind(id)
        .bind(json!({"env.variables":{"PRESERVED":"yes"},"exec.offers":["codex-exec-server"]}))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    report(world, machine, "0.165.0").await;
    (id, machine)
}

async fn report(world: &World, machine: Id, version: &str) {
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1").bind(machine).bind(json!({
        "policy":{"agent.harnesses":[{"kind":"codex","path":PROGRAM}]},
        "harnesses":[{"kind":"codex","path":PROGRAM,"version":version,"daemon":false}],
        "discovered_harnesses":[{"kind":"codex","path":"/controlled/unconfirmed","version":version,"daemon":false}]
    })).execute(&world.db.pool).await.unwrap();
}

async fn consent_state(world: &World, environment: Id) -> (Json, i64, i64) {
    sqlx::query_as(
        "SELECT config,revision,(SELECT count(*) FROM audit_log) FROM environments WHERE id=$1",
    )
    .bind(environment)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn only_a_human_environment_manager_can_create_or_remove_program_consent() {
    install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    orch_api_core::routes::settings::install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    let world = World::new().await;
    let owner = places::person(&world, "ada").await;
    let reader = places::person(&world, "reader").await;
    let org = places::make_org(&world, &owner, "Workshop", "workshop").await;
    let service = world
        .app
        .call(OrgServiceAccountCreate, &[&org.base.id])
        .token(&owner.token)
        .body(&OrgServiceAccountCreateBody {
            username: "agent_service".into(),
            display_name: "Agent service".into(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    let service_token = world
        .app
        .call(ServiceAccountTokenCreate, &[&service.user.id])
        .token(&owner.token)
        .body(&named_token("Agent service", &["*"]))
        .send()
        .await
        .full_token()
        .ok()
        .token
        .unwrap();
    let scoped = world
        .app
        .call(MeTokenCreate, &[])
        .token(&owner.token)
        .body(&named_token("Thread-only automation", &["thread.interact"]))
        .send()
        .await
        .full_token()
        .ok()
        .token
        .unwrap();
    let owner_pat = world
        .app
        .call(MeTokenCreate, &[])
        .token(&owner.token)
        .body(&named_token(
            "Environment manager",
            &["environment.view", "environment.manage"],
        ))
        .send()
        .await
        .full_token()
        .ok()
        .token
        .unwrap();
    let (id, _) = environment(&world, owner.id(), org.base.id).await;
    resources::name_person(&world, "environment", id, "environment.view", reader.id()).await;
    resources::name_person(
        &world,
        "environment",
        id,
        "environment.manage",
        service.user.id,
    )
    .await;
    world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&service_token)
        .send()
        .await
        .ok();
    let current = world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&owner.token)
        .send()
        .await
        .ok();
    let proposed = json!({"codex":{"path":PROGRAM,"version":"0.165.0","by":new_id(),"at":"not a client timestamp"}});
    let before = consent_state(&world, id).await;
    for token in [&reader.token, &service_token, &scoped] {
        let denied = world
            .app
            .call(EnvironmentUpdate, &[&id])
            .token(token)
            .body(&update(&current, Some(proposed.clone())))
            .send()
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::AuthzDenied);
        assert_eq!(
            consent_state(&world, id).await,
            before,
            "denied consent changes no configuration, revision or audit row"
        );
    }
    let allowed = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&owner_pat)
        .body(&update(&current, Some(proposed)))
        .send()
        .await
        .ok();
    assert_eq!(allowed.config[KEY]["codex"]["by"], json!(owner.id()));
    assert_eq!(
        allowed.config[KEY]["codex"]["at"],
        json!(Time::new(world.now()))
    );
    assert_eq!(allowed.config["env.variables"]["PRESERVED"], "yes");
    let mut unrelated = update(&allowed, Some(allowed.config[KEY].clone()));
    unrelated.config.as_mut().unwrap()["env.variables"]["PRESERVED"] = json!("service edit");
    let preserved = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&service_token)
        .body(&unrelated)
        .send()
        .await
        .ok();
    assert_eq!(preserved.config[KEY], allowed.config[KEY]);
    assert_eq!(
        preserved.config["env.variables"]["PRESERVED"],
        "service edit"
    );
    let before_removal = consent_state(&world, id).await;
    let denied = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&service_token)
        .body(&update(&preserved, None))
        .send()
        .await
        .problem();
    assert_eq!(
        denied.code,
        ErrorCode::AuthzDenied,
        "omission cannot bypass the human consent writer"
    );
    assert_eq!(consent_state(&world, id).await, before_removal);
    let removed = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&owner.token)
        .body(&update(&preserved, None))
        .send()
        .await
        .ok();
    assert!(removed.config.get(KEY).is_none());
    assert_eq!(removed.config["env.variables"]["PRESERVED"], "service edit");
    let audit: Vec<Json> =
        sqlx::query_scalar("SELECT details FROM audit_log WHERE action='environment.exec_changed'")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(audit.len(), 2);
    assert!(
        audit
            .iter()
            .any(|entry| entry["version_overrides"]["after"]["codex"]
                == allowed.config[KEY]["codex"])
    );
    assert!(
        audit
            .iter()
            .any(|entry| entry["version_overrides"]["before"]["codex"]
                == allowed.config[KEY]["codex"]
                && entry["version_overrides"]["after"] == json!({}))
    );
}

#[tokio::test]
async fn consent_uses_current_confirmed_facts_and_unknown_is_an_explicit_program_version() {
    install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    orch_api_core::routes::settings::install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    let world = World::new().await;
    let owner = places::person(&world, "ada").await;
    let org = places::make_org(&world, &owner, "Workshop", "workshop").await;
    let (id, machine) = environment(&world, owner.id(), org.base.id).await;
    let current = world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&owner.token)
        .send()
        .await
        .ok();
    for (path, version) in [("/controlled/unconfirmed", "0.165.0"), (PROGRAM, "0.166.0")] {
        let denied = world
            .app
            .call(EnvironmentUpdate, &[&id])
            .token(&owner.token)
            .body(&update(
                &current,
                Some(json!({"codex":{"path":path,"version":version}})),
            ))
            .send()
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::RequestInvalid);
    }
    report(&world, machine, "unknown").await;
    let allowed = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&owner.token)
        .body(&update(
            &current,
            Some(json!({"codex":{"path":PROGRAM,"version":"unknown"}})),
        ))
        .send()
        .await
        .ok();
    assert_eq!(allowed.config[KEY]["codex"]["version"], "unknown");
    assert_eq!(allowed.config[KEY]["codex"]["by"], json!(owner.id()));
    let mut forged = allowed.config[KEY].clone();
    forged["codex"]["by"] = json!(new_id());
    forged["codex"]["at"] = json!("not a client timestamp");
    let same = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&owner.token)
        .body(&update(&allowed, Some(forged)))
        .send()
        .await
        .ok();
    assert_eq!(
        same.config[KEY], allowed.config[KEY],
        "unchanged tuples keep their original attribution"
    );
    let cleared = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&owner.token)
        .body(&update(&same, Some(json!({}))))
        .send()
        .await
        .ok();
    assert_eq!(cleared.config[KEY], json!({}));
}

#[tokio::test]
async fn readers_see_current_consent_after_its_writer_loses_manage_and_is_deactivated() {
    install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    orch_api_core::routes::settings::install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    let world = World::new().await;
    let owner = places::person(&world, "ada").await;
    let manager = places::person(&world, "manager").await;
    let reader = places::person(&world, "reader").await;
    let org = places::make_org(&world, &owner, "Workshop", "workshop").await;
    let (id, machine) = environment(&world, owner.id(), org.base.id).await;
    let grant = resources::name_person(
        &world,
        "environment",
        id,
        "environment.manage",
        manager.id(),
    )
    .await;
    resources::name_person(&world, "environment", id, "environment.view", reader.id()).await;
    let current = world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&manager.token)
        .send()
        .await
        .ok();
    let allowed = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&manager.token)
        .body(&update(
            &current,
            Some(json!({"codex":{"path":PROGRAM,"version":"0.165.0"}})),
        ))
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    assert!(
        orch_db::queries::access::remove_entry(&mut conn, grant, owner.id(), world.now())
            .await
            .unwrap()
    );
    drop(conn);
    let denied = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&manager.token)
        .body(&EnvironmentUpdateBody {
            config: None,
            name: Some("Cannot change it".into()),
            ..update(&allowed, None)
        })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
        .bind(manager.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let shown = world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&reader.token)
        .send()
        .await
        .ok();
    assert!(
        shown.config.get(KEY).is_none(),
        "readers need no whole configuration"
    );
    assert!(shown.config.get("env.variables").is_none());
    let projected = serde_json::to_value(shown).unwrap();
    let program = &projected["exec"]["machine_programs"][0];
    assert_eq!(program["harness_kind"], "codex");
    assert_eq!(program["path"], PROGRAM);
    assert_eq!(program["version"], "0.165.0");
    assert_eq!(program["approval"]["by"]["id"], json!(manager.id()));
    assert_eq!(
        program["approval"]["at"],
        allowed.config[KEY]["codex"]["at"]
    );
    report(&world, machine, "0.166.0").await;
    let shown = world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&reader.token)
        .send()
        .await
        .ok();
    let projected = serde_json::to_value(shown).unwrap();
    let program = &projected["exec"]["machine_programs"][0];
    assert_eq!(program["version"], "0.166.0");
    assert_eq!(
        program["approval"],
        Json::Null,
        "a changed program does not inherit old consent"
    );
}
