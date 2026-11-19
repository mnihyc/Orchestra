//! The existing provider creation writers use their actual home and safe response projection.
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
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    CreateProvider, MachineProviderCreate, MachineProviderCreateBody, ProviderCreate, Upstream,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity;
use orch_db::secrets::MasterKey;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

const SECRET: &str = "synthetic-provider-secret-not-in-reply";
fn rule(
    operation: &str,
    kind: &str,
    id: Id,
    reach: &str,
    resources: &[&str],
    actions: &[&str],
) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":resources,
        "allow":[{"operation":operation,"actions":actions}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let policy: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(policy.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Provider creator boundary",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(policy)),
    )
    .await
    .unwrap();
    token
}
async fn world() -> World {
    install_master_key(MasterKey::new([7; 32]));
    World::new().await
}
fn api(home: Id, name: &str) -> CreateProvider {
    CreateProvider::Api {
        runtime_machine_id: None,
        key_machine_id: None,
        name: name.into(),
        home_id: home,
        access: None,
        arrangements: None,
        runtime_config: None,
        upstreams: vec![Upstream {
            protocol: "openai-responses".into(),
            base_url: "https://api.example.com/v1".into(),
            options: json!({}),
        }],
        api_key: Some(SECRET.into()),
        secrets: None,
        sources: None,
    }
}
async fn create(world: &World, token: &str, body: &CreateProvider) -> RawReply {
    world
        .app
        .call(ProviderCreate, &[])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
fn success(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn count(world: &World, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM providers WHERE name=$1")
        .bind(name)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
async fn receipt(world: &World, reply: &RawReply, name: &str) -> Id {
    let body = success(reply);
    assert_eq!(body["projection"], "receipt");
    let id: Id = serde_json::from_value(body["provider_id"].clone()).unwrap();
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(body["revision"], revision);
    assert!(
        body.as_object().unwrap().keys().all(|key| [
            "projection",
            "provider_id",
            "revision",
            "warning"
        ]
        .contains(&key.as_str()))
    );
    assert_eq!(count(world, name).await, 1);
    assert!(!String::from_utf8_lossy(&reply.raw).contains(SECRET));
    id
}
async fn directory(world: &World, by: &places::Who, home: Id) -> Id {
    let machine =
        resources::machine_forwarding_to(world, by.id(), &["https://api.example.com/v1"]).await;
    let policy = orch_link_contract::policy::LocalPolicy {
        provider_upstreams: vec!["https://api.example.com/v1".into()],
        ..Default::default()
    };
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET home_id=$2,org_id=(SELECT org_id FROM nodes WHERE id=$2),online=true,roles=ARRAY['agent','provider'],facts=$3 WHERE id=$1")
        .bind(machine).bind(home).bind(json!({"adapters":[{"kind":"fake","default_home_owned":true,"needs_binary":false}],"policy":policy,"provider_runtime_descriptor":descriptor}))
        .execute(&world.db.pool).await.unwrap();
    machine
}
async fn agent(world: &World, by: &places::Who, home: Id, machine: Option<Id>) -> Id {
    let mut conn = world.conn().await;
    let id = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "agent",
        Some(by.id()),
        Some(home),
        "Creator native profile",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Agent.into(),
            id,
        },
        by.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE agents SET machine_id=$2 WHERE id=$1")
        .bind(id)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    id
}
fn subscription(home: Id, agent: Id, runtime: Option<Id>) -> CreateProvider {
    CreateProvider::Subscription {
        runtime_machine_id: runtime,
        key_machine_id: None,
        name: "Scoped native creator".into(),
        home_id: home,
        access: None,
        arrangements: None,
        runtime_config: None,
        agent_id: agent,
        config: None,
        sign_in: None,
        sources: None,
    }
}

#[tokio::test]
async fn home_manage_creates_a_real_provider_with_only_its_exact_receipt() {
    let world = world().await;
    let c = resources::company(&world).await;
    let body = api(c.acme, "Scoped managed API creator");
    success(&create(&world, &c.boss.token, &api(c.acme, "Ordinary API premise")).await);
    let token = key(
        &world,
        c.boss.id(),
        vec![rule(
            "ProviderCreate",
            "org",
            c.acme,
            "self",
            &["provider"],
            &["manage"],
        )],
    )
    .await;
    let id = receipt(
        &world,
        &create(&world, &token, &body).await,
        "Scoped managed API creator",
    )
    .await;
    let state:(Id,Id,bool)=sqlx::query_as("SELECT home_id,owner_id,EXISTS(SELECT 1 FROM jobs WHERE provider_id=providers.id AND kind='provider_provision') FROM providers WHERE id=$1")
        .bind(id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(state, (c.acme, c.boss.id(), true));
}

#[tokio::test]
async fn same_operation_provider_read_returns_full_but_home_names_need_their_own_read() {
    let world = world().await;
    let c = resources::company(&world).await;
    let provider_rule = rule(
        "ProviderCreate",
        "org",
        c.acme,
        "subtree",
        &["provider"],
        &["manage", "read"],
    );
    let token = key(&world, c.boss.id(), vec![provider_rule.clone()]).await;
    let hidden = success(&create(&world, &token, &api(c.acme, "Unqualified home name")).await);
    assert!(hidden.get("projection").is_none());
    assert!(hidden["id"].is_string());
    assert!(hidden["home"]["name"].is_null());
    assert_eq!(hidden["home"]["redacted"], true);
    assert!(!String::from_utf8_lossy(&serde_json::to_vec(&hidden).unwrap()).contains(SECRET));
    let token = key(
        &world,
        c.boss.id(),
        vec![
            provider_rule,
            rule("ProviderCreate", "org", c.acme, "self", &["org"], &["read"]),
        ],
    )
    .await;
    let full = success(&create(&world, &token, &api(c.acme, "Qualified home name")).await);
    assert_eq!(full["home"]["name"], "Acme");
    assert_eq!(full["home"]["redacted"], false);
}

#[tokio::test]
async fn another_operation_read_does_not_release_creation_content() {
    let world = world().await;
    let c = resources::company(&world).await;
    let token = key(
        &world,
        c.boss.id(),
        vec![
            rule(
                "ProviderCreate",
                "org",
                c.acme,
                "self",
                &["provider"],
                &["manage"],
            ),
            rule(
                "ProviderGet",
                "org",
                c.acme,
                "subtree",
                &["provider"],
                &["read"],
            ),
        ],
    )
    .await;
    receipt(
        &world,
        &create(
            &world,
            &token,
            &api(c.acme, "Other Get is not creator Read"),
        )
        .await,
        "Other Get is not creator Read",
    )
    .await;
}

#[tokio::test]
async fn creation_scope_does_not_cross_homes_or_replace_current_ordinary_authority() {
    let world = world().await;
    let c = resources::company(&world).await;
    let wrong = key(
        &world,
        c.boss.id(),
        vec![rule(
            "ProviderCreate",
            "org",
            new_id(),
            "self",
            &["provider"],
            &["manage"],
        )],
    )
    .await;
    let reply = create(&world, &wrong, &api(c.acme, "Wrong scoped home")).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(count(&world, "Wrong scoped home").await, 0);
    let powerless = key(
        &world,
        c.stranger.id(),
        vec![rule(
            "ProviderCreate",
            "org",
            c.acme,
            "self",
            &["provider"],
            &["manage"],
        )],
    )
    .await;
    let reply = create(&world, &powerless, &api(c.acme, "Powerless scoped creator")).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(count(&world, "Powerless scoped creator").await, 0);
}

#[tokio::test]
async fn selected_directory_machines_keep_the_same_use_and_local_policy_checks() {
    let world = world().await;
    let c = resources::company(&world).await;
    let machine = directory(&world, &c.boss, c.acme).await;
    let token = key(
        &world,
        c.boss.id(),
        vec![rule(
            "ProviderCreate",
            "org",
            c.acme,
            "self",
            &["provider"],
            &["manage"],
        )],
    )
    .await;
    let mut body = api(c.acme, "Scoped directory API");
    if let CreateProvider::Api {
        runtime_machine_id,
        key_machine_id,
        ..
    } = &mut body
    {
        *runtime_machine_id = Some(machine);
        *key_machine_id = Some(machine);
    }
    let id = receipt(
        &world,
        &create(&world, &token, &body).await,
        "Scoped directory API",
    )
    .await;
    let locations: (Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT machine_id,key_machine_id FROM providers WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(locations, (Some(machine), Some(machine)));
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,provider.upstreams}','[]'::jsonb) WHERE id=$1").bind(machine).execute(&world.db.pool).await.unwrap();
    if let CreateProvider::Api { name, .. } = &mut body {
        *name = "Refused local endpoint".into();
    }
    assert_eq!(
        create(&world, &token, &body).await.problem().code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(count(&world, "Refused local endpoint").await, 0);
}

#[tokio::test]
async fn native_creation_keeps_the_profiles_inferred_keeper_and_ordinary_agent_use() {
    let world = world().await;
    let c = resources::company(&world).await;
    let machine = directory(&world, &c.boss, c.acme).await;
    let agent = agent(&world, &c.boss, c.acme, Some(machine)).await;
    let token = key(
        &world,
        c.boss.id(),
        vec![rule(
            "ProviderCreate",
            "org",
            c.acme,
            "self",
            &["provider"],
            &["manage"],
        )],
    )
    .await;
    let body = subscription(c.acme, agent, Some(machine));
    let id = receipt(
        &world,
        &create(&world, &token, &body).await,
        "Scoped native creator",
    )
    .await;
    let keeper: Option<Id> = sqlx::query_scalar("SELECT key_machine_id FROM providers WHERE id=$1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(keeper, Some(machine));
    let powerless = key(
        &world,
        c.admin.id(),
        vec![rule(
            "ProviderCreate",
            "org",
            c.acme,
            "self",
            &["provider"],
            &["manage"],
        )],
    )
    .await;
    let before = count(&world, "Scoped native creator").await;
    assert_eq!(
        create(&world, &powerless, &body).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(count(&world, "Scoped native creator").await, before);
}

#[tokio::test]
async fn machine_registration_requires_its_path_write_and_keeps_the_existing_home_rule() {
    let world = world().await;
    let c = resources::company(&world).await;
    let machine = directory(&world, &c.boss, c.acme).await;
    let body = MachineProviderCreateBody::Api {
        home_id: c.acme,
        name: "Scoped machine registration".into(),
        upstreams: vec![Upstream {
            protocol: "openai-responses".into(),
            base_url: "https://api.example.com/v1".into(),
            options: json!({}),
        }],
        sources: None,
    };
    let home = rule(
        "MachineProviderCreate",
        "org",
        c.acme,
        "self",
        &["provider"],
        &["manage"],
    );
    let missing = key(&world, c.boss.id(), vec![home.clone()]).await;
    assert_eq!(
        world
            .app
            .call(MachineProviderCreate, &[&machine])
            .token(&missing)
            .body(&body)
            .send_bytes()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(count(&world, "Scoped machine registration").await, 0);
    let token = key(
        &world,
        c.boss.id(),
        vec![
            home,
            rule(
                "MachineProviderCreate",
                "machine",
                machine,
                "self",
                &["machine"],
                &["write"],
            ),
        ],
    )
    .await;
    let reply = world
        .app
        .call(MachineProviderCreate, &[&machine])
        .token(&token)
        .body(&body)
        .send_bytes()
        .await;
    receipt(&world, &reply, "Scoped machine registration").await;
}

#[tokio::test]
async fn legacy_and_browser_creation_keep_their_original_full_wire_shape() {
    let world = world().await;
    let c = resources::company(&world).await;
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: c.boss.id(),
            name: "Legacy provider creation",
            token_hash: &token_hash(&token),
            scopes: &["*".to_owned()],
            node_id: None,
            at: world.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    for (index, token) in [c.boss.token.as_str(), token.as_str()]
        .into_iter()
        .enumerate()
    {
        let body = success(
            &create(
                &world,
                token,
                &api(c.acme, &format!("Ordinary create {index}")),
            )
            .await,
        );
        assert!(body.get("projection").is_none());
        assert!(body["id"].is_string());
        assert!(body["home"].is_object());
    }
}

async fn create_native_route(
    world: &World,
    token: &str,
    home: Id,
    profile: Id,
    machine: Option<Id>,
) -> RawReply {
    match machine {
        Some(machine) => {
            world
                .app
                .call(MachineProviderCreate, &[&machine])
                .token(token)
                .body(&MachineProviderCreateBody::Subscription {
                    home_id: home,
                    name: "Scoped native creator".into(),
                    agent_id: profile,
                    sources: None,
                })
                .send_bytes()
                .await
        }
        None => create(world, token, &subscription(home, profile, None)).await,
    }
}

#[tokio::test]
async fn creation_use_refusals_keep_browser_details_and_bound_private_names_for_a_key() {
    const AGENT_NAME: &str = "PRIVATE-CREATOR-AGENT-REFUSAL-SENTINEL";
    const MACHINE_NAME: &str = "PRIVATE-CREATOR-MACHINE-REFUSAL-SENTINEL";
    let world = world().await;
    let c = resources::company(&world).await;
    let machine = directory(&world, &c.boss, c.acme).await;
    let profile = agent(&world, &c.boss, c.acme, Some(machine)).await;
    sqlx::query("UPDATE agents SET name=$2 WHERE id=$1")
        .bind(profile)
        .bind(AGENT_NAME)
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET name=$2 WHERE id=$1")
        .bind(machine)
        .bind(MACHINE_NAME)
        .execute(&world.db.pool)
        .await
        .unwrap();
    // Ordinary View deliberately permits the browser to know the profile's name,
    // while Use stays absent. The operation key delegates no source-content Read.
    resources::name_person(&world, "agent", profile, "agent.view", c.admin.id()).await;
    let token = key(
        &world,
        c.admin.id(),
        vec![
            rule(
                "ProviderCreate",
                "org",
                c.acme,
                "self",
                &["provider"],
                &["manage"],
            ),
            rule(
                "MachineProviderCreate",
                "org",
                c.acme,
                "self",
                &["provider"],
                &["manage"],
            ),
            rule(
                "MachineProviderCreate",
                "machine",
                machine,
                "self",
                &["machine"],
                &["write"],
            ),
        ],
    )
    .await;
    for path_machine in [None, Some(machine)] {
        let browser =
            create_native_route(&world, &c.admin.token, c.acme, profile, path_machine).await;
        assert_eq!(
            browser.status,
            StatusCode::FORBIDDEN,
            "ordinary AgentUse premise: {}",
            String::from_utf8_lossy(&browser.raw)
        );
        assert!(
            String::from_utf8_lossy(&browser.raw).contains(AGENT_NAME),
            "the ordinary named refusal is intentional"
        );
        let scoped = create_native_route(&world, &token, c.acme, profile, path_machine).await;
        assert_eq!(
            scoped.status,
            StatusCode::FORBIDDEN,
            "{}",
            String::from_utf8_lossy(&scoped.raw)
        );
        let problem: Json = serde_json::from_slice(&scoped.raw).unwrap();
        assert_eq!(problem["code"], "authz.denied");
        if let Some(required) = problem.get("required") {
            assert_eq!(required, "agent.use");
        }
        let encoded = String::from_utf8_lossy(&scoped.raw);
        assert!(
            !encoded.contains(AGENT_NAME),
            "a creation key without content Read gets no private profile name: {encoded}"
        );
        assert!(!encoded.contains(MACHINE_NAME));
        assert!(
            problem.get("missing").is_none(),
            "the existing limited refusal carries no named grantors"
        );
    }
    assert_eq!(count(&world, "Scoped native creator").await, 0);

    // The existing machine facade hides a missing Use grant as 404. Keep that
    // ordinary information boundary instead of changing it to a named 403.
    resources::name_person(&world, "agent", profile, "agent.use", c.admin.id()).await;
    for path_machine in [None, Some(machine)] {
        for credential in [&c.admin.token, &token] {
            let hidden =
                create_native_route(&world, credential, c.acme, profile, path_machine).await;
            assert_eq!(
                hidden.status,
                StatusCode::NOT_FOUND,
                "ordinary MachineUse premise: {}",
                String::from_utf8_lossy(&hidden.raw)
            );
            let encoded = String::from_utf8_lossy(&hidden.raw);
            assert!(!encoded.contains(AGENT_NAME));
            assert!(!encoded.contains(MACHINE_NAME));
        }
    }
    assert_eq!(count(&world, "Scoped native creator").await, 0);
}

#[tokio::test]
async fn creation_refusal_uses_the_current_key_mode_after_the_real_identity_wait() {
    use tower::ServiceExt;
    const PRIVATE_AGENT: &str = "PRIVATE-CONVERTED-CREATOR-AGENT";
    const PRIVATE_MACHINE: &str = "PRIVATE-CONVERTED-CREATOR-MACHINE";
    // Same bounded wait budget as the existing identity-currentness controls.
    const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
    let world = world().await;
    let c = resources::company(&world).await;
    let machine = directory(&world, &c.boss, c.acme).await;
    let profile = agent(&world, &c.boss, c.acme, Some(machine)).await;
    sqlx::query("UPDATE agents SET name=$2 WHERE id=$1")
        .bind(profile)
        .bind(PRIVATE_AGENT)
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET name=$2 WHERE id=$1")
        .bind(machine)
        .bind(PRIVATE_MACHINE)
        .execute(&world.db.pool)
        .await
        .unwrap();
    resources::name_person(&world, "agent", profile, "agent.view", c.admin.id()).await;
    for path_machine in [None, Some(machine)] {
        let token = new_token(API_TOKEN_PREFIX);
        let token_id = new_id();
        identity::create_api_token(
            &mut *world.conn().await,
            &identity::NewApiToken {
                id: token_id,
                user_id: c.admin.id(),
                name: "Converting creator credential",
                token_hash: &token_hash(&token),
                scopes: &["*".to_owned()],
                node_id: None,
                at: world.now(),
                expires_at: None,
            },
        )
        .await
        .unwrap();
        let ordinary = create_native_route(&world, &token, c.acme, profile, path_machine).await;
        assert_eq!(
            ordinary.status,
            StatusCode::FORBIDDEN,
            "untouched legacy premise: {}",
            String::from_utf8_lossy(&ordinary.raw)
        );
        assert!(
            String::from_utf8_lossy(&ordinary.raw).contains(PRIVATE_AGENT),
            "legacy refusal remains named before conversion"
        );

        let mut blocker = world.db.pool.begin().await.unwrap();
        orch_db::queries::transaction_guard::lock_person(&mut blocker, c.admin.id())
            .await
            .unwrap()
            .unwrap();
        sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
            .bind(token_id)
            .bind(c.admin.id())
            .execute(&mut *blocker)
            .await
            .unwrap();
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let mut registry = orch_api_core::registry::Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let app = registry
            .into_router_for(orch_api_contract::surface::HttpSurface::Key)
            .with_state(world.app.app.clone());
        let (path, body) = match path_machine {
            Some(machine) => (
                format!("/machines/{machine}/providers"),
                serde_json::to_vec(&MachineProviderCreateBody::Subscription {
                    home_id: c.acme,
                    name: "Scoped native creator".into(),
                    agent_id: profile,
                    sources: None,
                })
                .unwrap(),
            ),
            None => (
                "/providers".to_owned(),
                serde_json::to_vec(&subscription(c.acme, profile, None)).unwrap(),
            ),
        };
        let request = http::Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap();
        let mut action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
        let observed = tokio::time::timeout(WAIT,async {
            loop {
                if action.is_finished() {break false;}
                // This prefix uniquely identifies the existing combined identity guard;
                // it stays visible even when pg_stat_activity truncates a long statement.
                let waiting:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE 'WITH watched AS MATERIALIZED (%')")
                    .bind(blocker_pid).fetch_one(&world.db.pool).await.unwrap();
                if waiting {break true;}
                tokio::task::yield_now().await;
            }
        }).await.unwrap_or(false);
        if !observed {
            action.abort();
            let _ = action.await;
            blocker.rollback().await.unwrap();
            assert!(
                observed,
                "the request must actually wait at its guarded identity checkpoint"
            );
            return;
        }
        let operation = if path_machine.is_some() {
            "MachineProviderCreate"
        } else {
            "ProviderCreate"
        };
        let mut rules = vec![rule(
            operation,
            "org",
            c.acme,
            "self",
            &["provider"],
            &["manage"],
        )];
        if let Some(machine) = path_machine {
            rules.push(rule(
                operation,
                "machine",
                machine,
                "self",
                &["machine"],
                &["write"],
            ));
        }
        sqlx::query("UPDATE api_tokens SET scopes='{}'::text[],operation_scope=$2 WHERE id=$1")
            .bind(token_id)
            .bind(json!({"rules":rules}))
            .execute(&mut *blocker)
            .await
            .unwrap();
        blocker.commit().await.unwrap();
        let result = tokio::time::timeout(WAIT, &mut action).await;
        if result.is_err() {
            action.abort();
            let _ = action.await;
        }
        let response = result.expect("released creation completes").unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let raw = axum::body::to_bytes(
            response.into_body(),
            orch_api_contract::common::MAX_JSON_BYTES,
        )
        .await
        .unwrap();
        let problem: Json = serde_json::from_slice(&raw).unwrap();
        assert_eq!(problem["code"], "authz.denied");
        let encoded = String::from_utf8_lossy(&raw);
        assert!(
            !encoded.contains(PRIVATE_AGENT),
            "current modern mode bounds the postguard refusal: {encoded}"
        );
        assert!(!encoded.contains(PRIVATE_MACHINE));
        assert!(problem.get("missing").is_none());
        assert_eq!(count(&world, "Scoped native creator").await, 0);
    }
}

#[tokio::test]
async fn hosted_machine_creation_uses_the_canonical_host_subject_and_keeps_the_physical_binding() {
    let world = world().await;
    let c = resources::company(&world).await;
    let machine = directory(&world, &c.boss, c.acme).await;
    let host = new_id();
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO hosts(id,machine_id,owner_id,home_id,org_id,name) SELECT $1,$2,$3,id,org_id,'Canonical provider host' FROM nodes WHERE id=$4")
        .bind(host).bind(machine).bind(c.boss.id()).bind(c.acme).execute(&mut *conn).await.unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id: host,
        },
        c.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    resources::name_person(&world, "host", host, "host.use", c.admin.id()).await;
    // The same ordinary writer can use this hosted directory through HostUse;
    // the actor is not the physical machine's owner.
    let mut ordinary = api(c.acme, "Ordinary hosted provider");
    if let CreateProvider::Api {
        runtime_machine_id,
        key_machine_id,
        api_key,
        ..
    } = &mut ordinary
    {
        *runtime_machine_id = Some(machine);
        *key_machine_id = Some(machine);
        *api_key = None;
    }
    success(&create(&world, &c.admin.token, &ordinary).await);

    let home_rule = rule(
        "MachineProviderCreate",
        "org",
        c.acme,
        "self",
        &["provider"],
        &["manage"],
    );
    let token = key(
        &world,
        c.admin.id(),
        vec![
            home_rule.clone(),
            rule(
                "MachineProviderCreate",
                "host",
                host,
                "self",
                &["machine"],
                &["write"],
            ),
        ],
    )
    .await;
    let mut body = MachineProviderCreateBody::Api {
        home_id: c.acme,
        name: "Scoped hosted provider".into(),
        upstreams: vec![Upstream {
            protocol: "openai-responses".into(),
            base_url: "https://api.example.com/v1".into(),
            options: json!({}),
        }],
        sources: None,
    };
    let reply = world
        .app
        .call(MachineProviderCreate, &[&machine])
        .token(&token)
        .body(&body)
        .send_bytes()
        .await;
    let provider = receipt(&world, &reply, "Scoped hosted provider").await;
    let actual: (Option<Id>, Option<Id>, Id) =
        sqlx::query_as("SELECT machine_id,key_machine_id,owner_id FROM providers WHERE id=$1")
            .bind(provider)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(actual, (Some(machine), Some(machine), c.admin.id()));
    assert_ne!(
        machine, host,
        "the scope subject and actual native machine are distinct"
    );
    let jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE provider_id=$1 AND kind='provider_provision'",
    )
    .bind(provider)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        jobs, 0,
        "registering an existing directory makes no managed unit"
    );

    if let MachineProviderCreateBody::Api { name, .. } = &mut body {
        *name = "Browser hosted provider".into();
    }
    success(
        &world
            .app
            .call(MachineProviderCreate, &[&machine])
            .token(&c.admin.token)
            .body(&body)
            .send_bytes()
            .await,
    );
    if let MachineProviderCreateBody::Api { name, .. } = &mut body {
        *name = "Wrong physical subject".into();
    }
    let wrong = key(
        &world,
        c.admin.id(),
        vec![
            home_rule,
            rule(
                "MachineProviderCreate",
                "machine",
                machine,
                "self",
                &["machine"],
                &["write"],
            ),
        ],
    )
    .await;
    let refused = world
        .app
        .call(MachineProviderCreate, &[&machine])
        .token(&wrong)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "a structurally-valid physical ID does not replace the canonical Host subject: {}",
        String::from_utf8_lossy(&refused.raw)
    );
    assert_eq!(count(&world, "Wrong physical subject").await, 0);
}
