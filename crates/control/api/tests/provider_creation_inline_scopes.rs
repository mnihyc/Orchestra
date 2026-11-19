//! Inline sign-in keeps the creator's origin and existing postcommit partial-success semantics.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod resources;
mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::resources::{CreateProvider, ProviderCreate, ProviderLoginStartBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources::provider_runtime};
use orch_engine::provider_setup::{LoginAuthority, Pending};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

async fn key(world: &World, user: Id, home: Id) -> String {
    let scope = json!({"rules":[{"target":{"kind":"org","id":home,"reach":"self"},"resources":["provider"],"allow":[{"operation":"ProviderCreate","actions":["manage"]}]}]});
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Inline creator",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    token
}

async fn agent(world: &World, user: Id, home: Id, machine: Option<Id>) -> Id {
    let mut conn = world.conn().await;
    let id = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "agent",
        Some(user),
        Some(home),
        "Cold inline profile",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Agent.into(),
            id,
        },
        user,
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE agents SET machine_id=$2,descriptor='{}' WHERE id=$1")
        .bind(id)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    id
}

fn body(home: Id, agent: Id, runtime: Option<Id>, name: &str) -> CreateProvider {
    CreateProvider::Subscription {
        runtime_machine_id: runtime,
        key_machine_id: None,
        name: name.into(),
        home_id: home,
        access: None,
        arrangements: None,
        runtime_config: None,
        agent_id: agent,
        config: None,
        sign_in: Some(ProviderLoginStartBody {
            method: "device_code".into(),
            values: None,
        }),
        sources: None,
    }
}

async fn create(world: &World, token: &str, body: &CreateProvider) -> Json {
    let reply = world
        .app
        .call(ProviderCreate, &[])
        .token(token)
        .body(body)
        .send_bytes()
        .await;
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn id(reply: &Json) -> Id {
    serde_json::from_value(
        reply
            .get("provider_id")
            .or_else(|| reply.get("id"))
            .unwrap()
            .clone(),
    )
    .unwrap()
}

#[tokio::test]
async fn cold_inline_sign_in_retains_provider_create_without_borrowing_login_start() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let profile = agent(&world, c.boss.id(), c.acme, None).await;
    let ordinary = create(
        &world,
        &c.boss.token,
        &body(c.acme, profile, None, "Ordinary cold inline"),
    )
    .await;
    let mut conn = world.conn().await;
    assert!(
        provider_runtime::state(&mut conn, id(&ordinary))
            .await
            .unwrap()
            .unwrap()
            .login_pending
            .is_some(),
        "ordinary cold sign-in really waits for its runtime"
    );
    drop(conn);
    let token = key(&world, c.boss.id(), c.acme).await;
    let reply = create(
        &world,
        &token,
        &body(c.acme, profile, None, "Scoped cold inline"),
    )
    .await;
    assert_eq!(reply["projection"], "receipt");
    assert!(
        reply.get("warning").is_none(),
        "a valid cold sign-in is retained, not replaced by a retry warning: {reply}"
    );
    let provider = id(&reply);
    let mut conn = world.conn().await;
    let state = provider_runtime::state(&mut conn, provider)
        .await
        .unwrap()
        .unwrap();
    let pending: Pending =
        serde_json::from_slice(state.login_pending.as_ref().expect("cold sign-in pending"))
            .unwrap();
    assert_eq!(pending.method, "device_code");
    assert_eq!(
        pending.authority.as_ref().unwrap().operation(),
        "ProviderCreate"
    );
    assert_eq!(pending.by, Some(c.boss.id()));
    let active = LoginAuthority::load(&mut conn, provider)
        .await
        .unwrap()
        .unwrap();
    assert!(
        active
            .authorized(&mut conn, provider, Time::new(world.now()))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn keeper_refusal_of_optional_inline_sign_in_keeps_the_created_provider_and_safe_warning() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let machine =
        resources::machine_forwarding_to(&world, c.boss.id(), &["https://api.example.com/v1"])
            .await;
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET home_id=$2,org_id=(SELECT org_id FROM nodes WHERE id=$2),online=true,roles=ARRAY['agent','provider'],facts=$3 WHERE id=$1")
        .bind(machine).bind(c.acme).bind(json!({"adapters":[{"kind":"fake","default_home_owned":false,"needs_binary":false}],"policy":{"provider.signin":false,"provider.arrangements":["separated","key_separated"]},"provider_runtime_descriptor":descriptor})).execute(&world.db.pool).await.unwrap();
    let profile = agent(&world, c.boss.id(), c.acme, Some(machine)).await;
    let token = key(&world, c.boss.id(), c.acme).await;
    for (credential, name) in [
        (&c.boss.token, "Ordinary refused inline"),
        (&token, "Scoped refused inline"),
    ] {
        let reply = create(
            &world,
            credential,
            &body(c.acme, profile, Some(machine), name),
        )
        .await;
        let provider = id(&reply);
        assert_eq!(
            reply["warning"],
            "The provider was created. Open Sign-in and keys to start its sign-in."
        );
        let mut conn = world.conn().await;
        let state = provider_runtime::state(&mut conn, provider)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.machine_id, Some(machine));
        assert!(state.login_pending.is_none());
        assert!(
            provider_runtime::login_secrets(&mut conn, provider)
                .await
                .unwrap()
                .is_empty(),
            "a locally refused sign-in seals no pending values"
        );
    }
}

#[tokio::test]
async fn a_legacy_provider_manage_key_keeps_its_ordinary_managed_creation_premise() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: c.boss.id(),
            name: "Legacy provider management",
            token_hash: &token_hash(&token),
            scopes: &["provider.manage".to_owned()],
            node_id: None,
            at: world.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    let request = CreateProvider::Api {
        runtime_machine_id: None,
        key_machine_id: None,
        name: "Legacy narrow managed creation".into(),
        home_id: c.acme,
        access: None,
        arrangements: None,
        runtime_config: None,
        upstreams: vec![orch_api_contract::resources::Upstream {
            protocol: "openai-responses".into(),
            base_url: "https://api.example.com/v1".into(),
            options: json!({}),
        }],
        api_key: None,
        secrets: None,
        sources: None,
    };
    let reply = create(&world, &token, &request).await;
    let provider = id(&reply);
    let mut conn = world.conn().await;
    assert!(
        provider_runtime::state(&mut conn, provider)
            .await
            .unwrap()
            .is_some()
    );
    let queued:i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE provider_id=$1 AND kind='provider_provision' AND plan->>'mode'='make'").bind(provider).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        queued, 1,
        "the same accepted creation keeps its ordinary managed Make"
    );
}
