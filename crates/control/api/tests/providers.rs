//! Providers on a key over HTTP: making one with its key sealed, listing, reading, changing, switching off, deleting, and the key and the secret headers, none of which any request returns.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::binding::ArrangementType;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    CreateProvider, Provider, ProviderCreate, ProviderDelete, ProviderDisable, ProviderEnable,
    ProviderGet, ProviderKeySet, ProviderKeySetBody, ProviderList, ProviderListQuery,
    ProviderListScope, ProviderModelCreate, ProviderModelCreateBody, ProviderSecretSet,
    ProviderSecretSetBody, ProviderStatus, ProviderUpdate, ProviderUpdateBody, Upstream,
};
use orch_api_core::routes::resources::sealing::{binding, install_master_key};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::secrets::MasterKey;
use orch_settings::Target;
use orch_types::{Id, Json};
use places::{Who, make_department, make_project, make_topic, missing_permissions};
use resources::{Company, company, name_on_page, name_person};
use serde_json::json;
use support::{Statements, World};

/// The key every test seals under.
const MASTER: [u8; 32] = [7; 32];
/// A key and a secret header that no request may ever show.
const KEY: &str = "sk-live-do-not-show-0001";
const GATEWAY_SECRET: &str = "gw-do-not-show-0002";

fn endpoint(protocol: &str, base_url: &str, options: Json) -> Upstream {
    Upstream {
        protocol: protocol.to_owned(),
        base_url: base_url.to_owned(),
        options,
    }
}

fn api_body(name: &str, home: Id) -> CreateProvider {
    CreateProvider::Api {
        runtime_machine_id: None,
        name: name.to_owned(),
        home_id: home,
        access: None,
        arrangements: None,
        runtime_config: None,
        upstreams: vec![endpoint(
            "openai-responses",
            "https://api.example.com/v1",
            json!({}),
        )],
        api_key: Some(KEY.to_owned()),
        secrets: None,
        key_machine_id: None,
        sources: None,
    }
}

async fn world() -> World {
    install_master_key(MasterKey::new(MASTER));
    World::new().await
}

#[tokio::test]
async fn a_key_can_be_added_after_creation_without_treating_an_absent_key_as_no_authentication() {
    let world = world().await;
    let c = company(&world).await;
    let mut body = api_body("Key added afterwards", c.acme);
    if let CreateProvider::Api { api_key, .. } = &mut body {
        *api_key = None;
    }
    let made = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.boss.token)
        .body(&body)
        .send()
        .await
        .ok()
        .provider;
    let shown = serde_json::to_value(&made).unwrap();
    assert_eq!(shown["status"], "needs_setup");
    assert!(!made.key_set);
    assert_ne!(made.facts.credential_verified, Some(true));
    assert_ne!(
        shown["upstreams"][0]["options"]["auth"], "none",
        "omitting a key does not choose unauthenticated model access"
    );
    world
        .app
        .call(ProviderKeySet, &[&made.base.id])
        .token(&c.boss.token)
        .body(&ProviderKeySetBody {
            api_key: KEY.to_owned(),
        })
        .send()
        .await
        .ok();
    let saved = world
        .app
        .call(ProviderGet, &[&made.base.id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert!(
        saved.key_set,
        "Sign-in and keys can finish this provider later"
    );
    assert!(!serde_json::to_string(&saved).unwrap().contains(KEY));
}

/// A real machine permission page with declared provider-directory services and no sign-in permission in borrowed homes.
async fn directory_keeper(world: &World, by: &Who, home: Id, name: &str) -> Id {
    // Its page is started as enrolling starts it: its owner's alone.
    let machine =
        resources::machine_forwarding_to(world, by.id(), &["https://api.example.com/v1"]).await;
    let mut conn = world.db.pool.acquire().await.unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/allowed".into()],
        provider_upstreams: vec!["https://api.example.com/v1".into()],
        ..orch_link_contract::policy::LocalPolicy::default()
    };
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET name=$2,home_id=$3,org_id=(SELECT org_id FROM nodes WHERE id=$3),online=true,roles=ARRAY['agent','provider'],facts=$4 WHERE id=$1")
        .bind(machine).bind(name).bind(home).bind(json!({"adapters":[{"kind":"fake","default_home_owned":true,"needs_binary":false}],"policy":policy,"provider_runtime_descriptor":descriptor}))
        .execute(&mut *conn).await.unwrap();
    machine
}

#[tokio::test]
async fn a_key_and_a_native_sign_in_have_independent_keepers_and_directory_runtimes_in_one_creation_flow()
 {
    let world = world().await;
    let c = company(&world).await;
    let keeper = directory_keeper(&world, &c.boss, c.acme, "Credential keeper").await;
    let runtime = directory_keeper(&world, &c.boss, c.acme, "Harness runtime").await;
    let mut api = api_body("Separated locations", c.acme);
    if let CreateProvider::Api {
        runtime_machine_id,
        key_machine_id,
        ..
    } = &mut api
    {
        *runtime_machine_id = Some(runtime);
        *key_machine_id = Some(keeper);
    }
    let made = make(&world, &c.boss, &api).await;
    assert_eq!(
        made.credential_keeper.as_ref().map(|machine| machine.id),
        Some(keeper)
    );
    assert_eq!(
        made.machine.as_ref().map(|machine| machine.id),
        Some(runtime)
    );
    assert_eq!(
        made.base.placement,
        orch_api_contract::environment::Placement::Connected
    );
    assert!(
        made.key_set,
        "a supplied key stays sealed, even when its keeper differs from the runtime"
    );
    world
        .app
        .call(ProviderKeySet, &[&made.base.id])
        .token(&c.boss.token)
        .body(&ProviderKeySetBody {
            api_key: "rotated-without-moving-the-keeper".into(),
        })
        .send()
        .await
        .ok();
    let locations: (Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT key_machine_id,machine_id FROM providers WHERE id=$1")
            .bind(made.base.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        locations,
        (Some(keeper), Some(runtime)),
        "rotation changes the credential, never either location"
    );
    let mut local = api.clone();
    if let CreateProvider::Api {
        name,
        api_key,
        key_machine_id,
        ..
    } = &mut local
    {
        *name = "Local key at the runtime's default keeper".into();
        *api_key = None;
        *key_machine_id = None;
    }
    let local = make(&world, &c.boss, &local).await;
    assert_eq!(
        local.credential_keeper.as_ref().map(|machine| machine.id),
        Some(runtime),
        "an omitted keeper is established once at creation"
    );
    let agent = {
        let mut conn = world.db.pool.acquire().await.unwrap();
        let agent = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "agent",
            Some(c.boss.id()),
            Some(c.acme),
            "Native profile",
        )
        .await
        .unwrap();
        orch_authz::ownership::start_resource(
            &mut conn,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ResourceKind::Agent.into(),
                id: agent,
            },
            c.boss.id(),
            orch_authz::ownership::NewAccess::OnlyMe,
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
        agent
    };
    let native = CreateProvider::Subscription {
        name: "Native in its keeper's home".into(),
        home_id: c.acme,
        agent_id: agent,
        access: None,
        arrangements: None,
        runtime_machine_id: Some(runtime),
        key_machine_id: Some(keeper),
        config: None,
        sign_in: None,
        sources: None,
        runtime_config: Some(
            json!({"runtime.home":{"kind":"existing_directory","path":"/allowed/runtime-home"}}),
        ),
    };
    let made = make(&world, &c.boss, &native).await;
    assert_eq!(
        made.credential_keeper.as_ref().map(|machine| machine.id),
        Some(keeper)
    );
    assert_eq!(
        made.machine.as_ref().map(|machine| machine.id),
        Some(runtime)
    );
    assert!(
        made.runtime_support.unwrap().sign_in.unwrap().home_owned,
        "the other runtime's borrowed path does not redirect the native keeper's own selected home"
    );
    let units: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE provider_id=$1 AND kind='provider_provision'",
    )
    .bind(made.base.id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        units, 0,
        "selecting a declared directory runtime never silently makes a unit"
    );
    sqlx::query("UPDATE agents SET machine_id=$2 WHERE id=$1")
        .bind(agent)
        .bind(keeper)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut registered = native.clone();
    if let CreateProvider::Subscription {
        name,
        key_machine_id,
        ..
    } = &mut registered
    {
        *name = "Registered profile's default keeper".into();
        *key_machine_id = None;
    }
    let defaulted = make(&world, &c.boss, &registered).await;
    assert_eq!(
        defaulted
            .credential_keeper
            .as_ref()
            .map(|machine| machine.id),
        Some(keeper)
    );
    assert_eq!(
        defaulted.machine.as_ref().map(|machine| machine.id),
        Some(runtime),
        "a profile's source location never chooses its runtime"
    );
    if let CreateProvider::Subscription { key_machine_id, .. } = &mut registered {
        *key_machine_id = Some(runtime);
    }
    let refused = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.boss.token)
        .body(&registered)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::RequestInvalid,
        "a machine-bound native profile cannot be moved by naming another keeper"
    );
}

#[tokio::test]
async fn a_runtime_home_uses_the_physical_machines_roots_and_the_provider_manager_guard() {
    let world = world().await;
    let c = company(&world).await;
    let made = make(&world, &c.boss, &api_body("Home choice", c.acme)).await;
    let machine = resources::machine_forwarding_to(&world, c.boss.id(), &[]).await;
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv/harness".into()],
        ..orch_link_contract::policy::LocalPolicy::default()
    };
    sqlx::query(
        "UPDATE machines SET roles=ARRAY['agent','provider']::text[],facts=facts || $2 WHERE id=$1",
    )
    .bind(machine)
    .bind(json!({"provider_runtime_descriptor":descriptor,"policy":policy}))
    .execute(&world.db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE providers SET machine_id=$2 WHERE id=$1")
        .bind(made.base.id)
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let body = |home| ProviderUpdateBody {
        name: None,
        upstreams: None,
        config: None,
        arrangements: None,
        expected_revision: made.revision,
        runtime_config: Some(json!({"runtime.home":home})),
    };
    for path in ["/outside/account", "/srv/harness/../outside"] {
        let refused = world
            .app
            .call(ProviderUpdate, &[&made.base.id])
            .token(&c.boss.token)
            .body(&body(json!({"kind":"existing_directory","path":path})))
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
    }
    let chosen = json!({"kind":"existing_directory","path":"/srv/harness/account"});
    let refused = world
        .app
        .call(ProviderUpdate, &[&made.base.id])
        .token(&c.viewer.token)
        .body(&body(chosen.clone()))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let changed = world
        .app
        .call(ProviderUpdate, &[&made.base.id])
        .token(&c.boss.token)
        .body(&body(chosen.clone()))
        .send()
        .await
        .ok()
        .provider;
    assert_eq!(changed.runtime_config["runtime.home"], chosen);
    let support = changed.runtime_support.unwrap();
    assert_eq!(support.machine_id, Some(machine));
    assert_eq!(
        support.lifecycle,
        Some(orch_driver_contract::descriptor::Lifecycle::Attach)
    );
    assert!(
        support.data_storage.unwrap().existing_directory,
        "harness homes do not require the unrelated environment role"
    );
}

#[tokio::test]
async fn provider_managers_can_keep_commands_but_only_owners_can_add_replace_or_remove_them() {
    let world = world().await;
    let c = company(&world).await;
    let made = make(&world, &c.boss, &api_body("Owner's commands", c.acme)).await;
    let id = made.base.id;
    name_person(&world, "provider", id, "provider.manage", c.viewer.id()).await;
    let settings = world
        .app
        .call(orch_api_contract::settings::ProviderSettingList, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert!(
        !settings
            .iter()
            .find(|setting| setting.key == "runtime.after_up")
            .unwrap()
            .writable
    );
    assert!(
        settings
            .iter()
            .find(|setting| setting.key == "runtime.needs")
            .unwrap()
            .writable
    );
    let settings = world
        .app
        .call(orch_api_contract::settings::ProviderSettingList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert!(
        settings
            .iter()
            .find(|setting| setting.key == "runtime.after_up")
            .unwrap()
            .writable
    );
    let commands = json!({"10-helper":{"command":["/opt/helper","a b"],"timeout_secs":30}});
    let body = |revision, config| ProviderUpdateBody {
        name: None,
        upstreams: None,
        config: None,
        arrangements: None,
        expected_revision: revision,
        runtime_config: Some(config),
    };
    let refused = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&c.viewer.token)
        .body(&body(
            made.revision,
            json!({"runtime.after_up":commands.clone()}),
        ))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "owner");
    let owned=world.app.call(ProviderUpdate,&[&id]).token(&c.boss.token)
        .body(&body(made.revision,json!({"runtime.after_up":commands.clone(),"runtime.storage":{"state":"fast"},"runtime.needs":{"memory_mb":512}})))
        .send().await.ok().provider;
    assert_eq!(owned.runtime_config["runtime.after_up"], commands);
    for config in [
        json!({}),
        json!({"runtime.after_up":{"10-helper":{"command":["different"]}}}),
        json!({"runtime.after_up":commands.clone(),"runtime.template_id":orch_types::new_id()}),
    ] {
        let refused = world
            .app
            .call(ProviderUpdate, &[&id])
            .token(&c.viewer.token)
            .body(&body(owned.revision, config))
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
        assert_eq!(refused.extra["required"], "owner");
    }
    let managed = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&c.viewer.token)
        .body(&body(
            owned.revision,
            json!({"runtime.after_up":commands,"runtime.resources":{"memory_mb":1024}}),
        ))
        .send()
        .await
        .ok()
        .provider;
    assert_eq!(
        managed.runtime_config["runtime.resources"]["memory_mb"],
        1024
    );
    let cleared = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&c.boss.token)
        .body(&body(managed.revision, json!({})))
        .send()
        .await
        .ok()
        .provider;
    assert!(cleared.runtime_config.get("runtime.after_up").is_none());
}

#[tokio::test]
async fn a_providers_arrangement_choices_and_writes_use_its_credential_keepers_policy() {
    let world = world().await;
    let c = company(&world).await;
    let keeper = directory_keeper(&world, &c.boss, c.acme, "Credential keeper").await;
    let mut body = api_body("Policy-aware provider", c.acme);
    if let CreateProvider::Api {
        runtime_machine_id,
        key_machine_id,
        ..
    } = &mut body
    {
        *runtime_machine_id = Some(keeper);
        *key_machine_id = Some(keeper);
    }
    let provider = make(&world, &c.boss, &body).await;
    let available: Vec<_> = provider
        .arrangements
        .iter()
        .filter(|consent| consent.available)
        .map(|consent| consent.kind)
        .collect();
    assert_eq!(
        available,
        [ArrangementType::Separated, ArrangementType::KeySeparated]
    );
    let change = ProviderUpdateBody {
        name: None,
        upstreams: None,
        config: None,
        runtime_config: None,
        arrangements: Some(vec![
            ArrangementType::Separated,
            ArrangementType::CredentialShared,
        ]),
        expected_revision: provider.revision,
    };
    let refused = world
        .app
        .call(ProviderUpdate, &[&provider.base.id])
        .token(&c.boss.token)
        .body(&change)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let current = world
        .app
        .call(ProviderGet, &[&provider.base.id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(
        current.revision, provider.revision,
        "a rejected enable changes neither the choice nor its revision"
    );
    assert!(
        !current
            .arrangements
            .iter()
            .any(|consent| consent.kind == ArrangementType::CredentialShared && consent.allowed)
    );
    if let CreateProvider::Api {
        name, arrangements, ..
    } = &mut body
    {
        *name = "Rejected export".into();
        *arrangements = change.arrangements;
    }
    assert_eq!(
        world
            .app
            .call(ProviderCreate, &[])
            .token(&c.boss.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM providers WHERE name='Rejected export'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "failed creation rolls back its registration");
}

async fn make(world: &World, by: &Who, body: &CreateProvider) -> Provider {
    world
        .app
        .call(ProviderCreate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
        .ok()
        .provider
}

async fn names(world: &World, by: &Who, scope: Option<ProviderListScope>) -> Vec<String> {
    let mut listed: Vec<String> = world
        .app
        .call(ProviderList, &[])
        .token(&by.token)
        .query(&ProviderListQuery {
            org_id: None,
            scope,
            ..ProviderListQuery::default()
        })
        .send()
        .await
        .ok()
        .items
        .into_iter()
        .map(|provider| match provider {
            orch_api_contract::common::Listed::Hidden(row) => row.name,
            orch_api_contract::common::Listed::Visible(row) => row.content.base.name,
        })
        .collect();
    listed.sort();
    listed
}

async fn add_model(world: &World, by: &Who, provider: Id, name: &str) {
    world
        .app
        .call(ProviderModelCreate, &[&provider])
        .token(&by.token)
        .body(&ProviderModelCreateBody {
            model_name: name.to_owned(),
            display_name: None,
            parameters: None,
            groups: None,
        })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn runtime_variables_and_secret_names_are_editable_but_the_sealed_value_is_never_read_back() {
    for placement in ["managed", "connected"] {
        install_master_key(MasterKey::new(MASTER));
        let world = World::new().await;
        let c = company(&world).await;
        let made = make(&world, &c.boss, &api_body("Runtime variables", c.acme)).await;
        if placement == "connected" {
            let mut conn = world.db.pool.acquire().await.unwrap();
            let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
                &mut conn,
                c.boss.id(),
                "Provider directory",
            )
            .await
            .unwrap();
            sqlx::query("UPDATE providers SET machine_id=$2 WHERE id=$1")
                .bind(made.base.id)
                .bind(machine)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        let updated = world.app.call(ProviderUpdate, &[&made.base.id]).token(&c.boss.token)
        .body(&ProviderUpdateBody { name:None,upstreams:None,config:None,arrangements:None,expected_revision:made.revision,
            runtime_config:Some(json!({"runtime.variables":{"REGION":"east"},"runtime.secret_names":["SERVICE_TOKEN"]})) })
        .send().await.ok().provider;
        assert_eq!(
            updated.runtime_config["runtime.variables"]["REGION"],
            "east"
        );
        world
            .app
            .call(
                ProviderSecretSet,
                &[&made.base.id, &"runtime:SERVICE_TOKEN".to_owned()],
            )
            .token(&c.boss.token)
            .body(&ProviderSecretSetBody {
                value: GATEWAY_SECRET.into(),
            })
            .send()
            .await
            .ok();
        let read = world
            .app
            .call(ProviderGet, &[&made.base.id])
            .token(&c.boss.token)
            .send()
            .await
            .ok();
        assert!(
            read.secrets_set
                .contains(&"runtime:SERVICE_TOKEN".to_owned())
        );
        assert!(
            !serde_json::to_string(&read)
                .unwrap()
                .contains(GATEWAY_SECRET)
        );
        let mut conn = world.db.pool.acquire().await.unwrap();
        let stored = orch_db::queries::settings::secret(
            &mut conn,
            "provider",
            made.base.id,
            "runtime:SERVICE_TOKEN",
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!String::from_utf8_lossy(&stored.sealed).contains(GATEWAY_SECRET));
    }
}

#[tokio::test]
async fn a_member_makes_a_provider_on_a_key_that_is_sealed_and_theirs_alone() {
    let world = world().await;
    let c = company(&world).await;
    let mut body = api_body("Team key", c.acme);
    if let CreateProvider::Api {
        upstreams, secrets, ..
    } = &mut body
    {
        upstreams[0].options = json!({ "secret_headers": ["x-gateway-key"] });
        *secrets = Some([("x-gateway-key".to_owned(), GATEWAY_SECRET.to_owned())].into());
    }
    let reply = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await;
    let raw = String::from_utf8_lossy(&reply.raw).into_owned();
    let made = reply.ok();
    assert!(!raw.contains(KEY) && !raw.contains(GATEWAY_SECRET), "{raw}");
    let provider = made.provider;
    assert_eq!(provider.base.name, "Team key");
    assert_eq!(provider.owner.username, "viewer");
    assert_eq!(provider.home.id, c.acme);
    assert_eq!(provider.org_id, c.acme);
    assert!(provider.key_set);
    assert_eq!(provider.secrets_set, ["x-gateway-key"]);
    assert_eq!(provider.protocols, ["openai-responses"]);
    assert_eq!(
        provider.base.status,
        ProviderStatus::NeedsSetup,
        "no model yet"
    );
    assert_eq!(provider.base.address, "acme.viewer.Team key");
    assert_eq!(provider.revision, 1);
    assert_eq!(provider.owners, 1);
    assert!(
        provider
            .access
            .permissions
            .iter()
            .any(|p| p == "provider.manage")
    );
    assert!(made.warning.is_none());
    let allowed: Vec<_> = provider
        .arrangements
        .iter()
        .filter(|each| each.allowed)
        .map(|each| each.kind)
        .collect();
    assert_eq!(
        allowed,
        [ArrangementType::Separated, ArrangementType::KeySeparated]
    );
    // Sealed: stored under the master key and bound to its own row, and nowhere in the clear.
    let stored: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT name, sealed FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1 ORDER BY name",
    )
    .bind(provider.base.id)
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["api_key", "header:x-gateway-key"]
    );
    let master = MasterKey::new(MASTER);
    for (name, sealed) in &stored {
        assert!(!String::from_utf8_lossy(sealed).contains("do-not-show"));
        let opened = master
            .open(sealed, &binding("provider", provider.base.id, name))
            .unwrap();
        let expected = if name == "api_key" {
            KEY
        } else {
            GATEWAY_SECRET
        };
        assert_eq!(opened, expected.as_bytes());
        assert!(
            master
                .open(sealed, &binding("provider", Id::nil(), name))
                .is_err(),
            "a sealed value does not open for another row"
        );
    }
    let audited: Vec<(String, Json)> =
        sqlx::query_as("SELECT action, details FROM audit_log WHERE action LIKE 'provider.%'")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].0, "provider.created");
    assert!(!audited[0].1.to_string().contains("do-not-show"));
}

#[tokio::test]
async fn runtime_support_is_the_machine_declaration_and_policy_even_when_the_display_placement_says_managed()
 {
    let world = world().await;
    let c = company(&world).await;
    let made = make(
        &world,
        &c.boss,
        &api_body("Declared directory runtime", c.acme),
    )
    .await;
    let machine = {
        let mut conn = world.db.pool.acquire().await.unwrap();
        orch_db::queries::resources::access_fixtures::insert_persons_machine(
            &mut conn,
            c.boss.id(),
            "Directory machine",
        )
        .await
        .unwrap()
    };
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        Default::default(),
    );
    sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1").bind(machine)
        .bind(json!({"environment_descriptor":descriptor,"policy":orch_link_contract::policy::LocalPolicy::default()}))
        .execute(&world.db.pool).await.unwrap();
    sqlx::query("UPDATE providers SET machine_id=$2 WHERE id=$1")
        .bind(made.base.id)
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let read = world
        .app
        .call(ProviderGet, &[&made.base.id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    let support = read.runtime_support.unwrap();
    assert_eq!(
        support.lifecycle,
        Some(orch_driver_contract::descriptor::Lifecycle::Attach)
    );
    assert_eq!(support.after_up, Some(false));
    assert_eq!(
        support.commands_after_start,
        Some(descriptor.commands_after_start)
    );
}

#[tokio::test]
async fn no_request_about_a_provider_returns_its_key_or_a_secret_header() {
    let world = world().await;
    let c = company(&world).await;
    let mut body = api_body("Team key", c.acme);
    if let CreateProvider::Api {
        upstreams, secrets, ..
    } = &mut body
    {
        upstreams[0].options = json!({ "secret_headers": ["x-gateway-key"] });
        *secrets = Some([("x-gateway-key".to_owned(), GATEWAY_SECRET.to_owned())].into());
    }
    let provider = make(&world, &c.boss, &body).await;
    let id = provider.base.id;
    let mut raws: Vec<(String, bytes::Bytes)> = Vec::new();
    macro_rules! keep {
        ($what:expr, $reply:expr) => {{
            let reply = $reply;
            raws.push(($what.to_owned(), reply.raw.clone()));
            reply
        }};
    }
    keep!(
        "get",
        world
            .app
            .call(ProviderGet, &[&id])
            .token(&c.boss.token)
            .send()
            .await
    );
    keep!(
        "list",
        world
            .app
            .call(ProviderList, &[])
            .token(&c.boss.token)
            .query(&ProviderListQuery::default())
            .send()
            .await
    );
    keep!(
        "key set",
        world
            .app
            .call(ProviderKeySet, &[&id])
            .token(&c.boss.token)
            .body(&ProviderKeySetBody {
                api_key: KEY.to_owned()
            })
            .send()
            .await
    );
    keep!(
        "secret set",
        world
            .app
            .call(ProviderSecretSet, &[&id, &"x-gateway-key"])
            .token(&c.boss.token)
            .body(&ProviderSecretSetBody {
                value: GATEWAY_SECRET.to_owned()
            })
            .send()
            .await
    );
    keep!(
        "update",
        world
            .app
            .call(ProviderUpdate, &[&id])
            .token(&c.boss.token)
            .body(&ProviderUpdateBody {
                name: Some("Renamed".into()),
                upstreams: None,
                config: None,
                arrangements: None,
                runtime_config: None,
                expected_revision: 3
            })
            .send()
            .await
    );
    keep!(
        "stale update",
        world
            .app
            .call(ProviderUpdate, &[&id])
            .token(&c.boss.token)
            .body(&ProviderUpdateBody {
                name: Some("Again".into()),
                upstreams: None,
                config: None,
                arrangements: None,
                runtime_config: None,
                expected_revision: 1
            })
            .send()
            .await
    );
    keep!(
        "disable",
        world
            .app
            .call(ProviderDisable, &[&id])
            .token(&c.boss.token)
            .send()
            .await
    );
    keep!(
        "enable",
        world
            .app
            .call(ProviderEnable, &[&id])
            .token(&c.boss.token)
            .send()
            .await
    );
    // A refusal and a bad request that carried a key do not echo it.
    let mut bad = api_body("Bad", c.acme);
    if let CreateProvider::Api { upstreams, .. } = &mut bad {
        upstreams[0].base_url = "ftp://nowhere".to_owned();
    }
    keep!(
        "bad create",
        world
            .app
            .call(ProviderCreate, &[])
            .token(&c.boss.token)
            .body(&bad)
            .send()
            .await
    );
    keep!(
        "refused get",
        world
            .app
            .call(ProviderGet, &[&id])
            .token(&c.stranger.token)
            .send()
            .await
    );
    keep!(
        "refused key set",
        world
            .app
            .call(ProviderKeySet, &[&id])
            .token(&c.stranger.token)
            .body(&ProviderKeySetBody {
                api_key: KEY.to_owned()
            })
            .send()
            .await
    );
    for (what, raw) in raws {
        let text = String::from_utf8_lossy(&raw);
        assert!(
            !text.contains(KEY) && !text.contains(GATEWAY_SECRET) && !text.contains("do-not-show"),
            "{what} returned a secret: {text}"
        );
    }
    let audited: Vec<Json> = sqlx::query_scalar("SELECT details FROM audit_log")
        .fetch_all(&world.db.pool)
        .await
        .unwrap();
    assert!(
        audited
            .iter()
            .all(|row| !row.to_string().contains("do-not-show"))
    );
}

#[tokio::test]
async fn a_provider_is_ready_when_it_has_a_key_and_an_enabled_model_and_not_before() {
    let world = world().await;
    let c = company(&world).await;
    let provider = make(&world, &c.boss, &api_body("Team key", c.acme)).await;
    let id = provider.base.id;
    let status = |world: &World| {
        let world = world.app.clone();
        let token = c.boss.token.clone();
        async move {
            world
                .call(ProviderGet, &[&id])
                .token(&token)
                .send()
                .await
                .ok()
                .base
                .status
        }
    };
    assert_eq!(status(&world).await, ProviderStatus::NeedsSetup);
    add_model(&world, &c.boss, id, "gpt-6-sol").await;
    assert_eq!(status(&world).await, ProviderStatus::Ready);
    // A provider that takes no key is ready as soon as it has a model.
    let keyless = CreateProvider::Api {
        runtime_machine_id: None,
        name: "Local".to_owned(),
        home_id: c.acme,
        access: None,
        arrangements: None,
        runtime_config: None,
        upstreams: vec![endpoint(
            "openai-responses",
            "https://models.internal/v1",
            json!({ "auth": "none" }),
        )],
        api_key: None,
        secrets: None,
        key_machine_id: None,
        sources: None,
    };
    let local = make(&world, &c.boss, &keyless).await;
    assert_eq!(local.base.status, ProviderStatus::NeedsSetup);
    assert!(!local.key_set);
    add_model(&world, &c.boss, local.base.id, "llama").await;
    let ready = world
        .app
        .call(ProviderGet, &[&local.base.id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(ready.base.status, ProviderStatus::Ready);
    assert!(!ready.key_set);
}

#[tokio::test]
async fn what_is_wrong_with_a_new_provider_is_told_member_by_member() {
    let world = world().await;
    let c = company(&world).await;
    let attempt = |mutate: Box<dyn Fn(&mut CreateProvider)>| {
        let mut body = api_body("Fine", c.acme);
        mutate(&mut body);
        body
    };
    let cases: Vec<(&str, CreateProvider)> = vec![
        (
            "name",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { name, .. } = b {
                    *name = "  ".into()
                }
            })),
        ),
        (
            "api_key",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { api_key, .. } = b {
                    *api_key = Some(String::new())
                }
            })),
        ),
        // The member is named as the request names it on the wire.
        (
            "keeper_machine_id",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { key_machine_id, .. } = b {
                    *key_machine_id = Some(orch_types::new_id())
                }
            })),
        ),
        (
            "upstreams",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { upstreams, .. } = b {
                    upstreams.clear()
                }
            })),
        ),
        (
            "upstreams[0].base_url",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { upstreams, .. } = b {
                    upstreams[0].base_url = "http://insecure.example".into()
                }
            })),
        ),
        (
            "upstreams[0].protocol",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { upstreams, .. } = b {
                    upstreams[0].protocol = "gopher".into()
                }
            })),
        ),
        (
            "upstreams[0].options.colour",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { upstreams, .. } = b {
                    upstreams[0].options = json!({ "colour": "red" })
                }
            })),
        ),
        (
            "secrets.nope",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { secrets, .. } = b {
                    *secrets = Some([("nope".to_owned(), "x".to_owned())].into())
                }
            })),
        ),
        (
            "arrangements",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { arrangements, .. } = b {
                    *arrangements = Some(vec![])
                }
            })),
        ),
        (
            "runtime_config.runtime.nope",
            attempt(Box::new(|b| {
                if let CreateProvider::Api { runtime_config, .. } = b {
                    *runtime_config = Some(json!({ "runtime.nope": 1 }))
                }
            })),
        ),
    ];
    for (path, body) in cases {
        let refused = world
            .app
            .call(ProviderCreate, &[])
            .token(&c.boss.token)
            .body(&body)
            .send()
            .await
            .problem();
        assert_eq!(
            refused.code,
            ErrorCode::RequestInvalid,
            "{path}: {refused:?}"
        );
        let text = refused.extra["errors"].to_string();
        assert!(text.contains(path), "{path} not in {text}");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM providers")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "nothing was made");
    // A subscription of a profile that is not there is refused, naming the member.
    let subscription = CreateProvider::Subscription {
        runtime_machine_id: None,
        key_machine_id: None,
        name: "Team Pro".to_owned(),
        home_id: c.acme,
        access: None,
        arrangements: None,
        runtime_config: None,
        agent_id: orch_types::new_id(),
        config: None,
        sign_in: None,
        sources: None,
    };
    let refused = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.boss.token)
        .body(&subscription)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn http_is_taken_where_the_policy_says_so_and_the_reply_warns_that_the_key_travels_in_clear()
{
    let world = world().await;
    let c = company(&world).await;
    let mut body = api_body("Plain", c.acme);
    if let CreateProvider::Api { upstreams, .. } = &mut body {
        upstreams[0].base_url = "http://models.internal:8080".to_owned();
    }
    let refused = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.boss.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    world
        .set_at(
            Target::Node(c.acme),
            "providers.allow_http_upstreams",
            json!(true),
        )
        .await;
    let made = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.boss.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(made.warning.unwrap().contains("clear"));
}

#[tokio::test]
async fn who_may_make_a_provider_where_is_the_engines_to_say() {
    let world = world().await;
    let c = company(&world).await;
    let research = make_department(&world, &c.boss, c.acme, "Research", "research").await;
    // A stranger is no member of the org.
    let refused = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.stranger.token)
        .body(&api_body("Sneaky", c.acme))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // A member makes one in their domain, and in an org inside it; the viewer's domain is Acme.
    let inside = make(
        &world,
        &c.viewer,
        &api_body("In research", research.base.id),
    )
    .await;
    assert_eq!(inside.home.id, research.base.id);
    assert_eq!(inside.base.address, "acme.research.viewer.In research");
    // Where the kind is left out for members, it takes whoever makes the org's rules.
    world
        .set_at(
            Target::Node(c.acme),
            "resources.created_by_members",
            json!(["host", "template"]),
        )
        .await;
    let refused = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.viewer.token)
        .body(&api_body("Member's", c.acme))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(
        missing_permissions(&refused).contains(&"settings.manage".to_owned()),
        "{refused:?}"
    );
    let by_admin = make(&world, &c.admin, &api_body("Admin's", c.acme)).await;
    assert_eq!(by_admin.owner.username, "admin");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM providers")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn a_provider_somebody_owns_is_not_listed_for_a_stranger_and_is_shown_to_a_user_without_its_inside()
 {
    let world = world().await;
    let c = company(&world).await;
    let mut body = api_body("Team key", c.acme);
    if let CreateProvider::Api {
        upstreams, secrets, ..
    } = &mut body
    {
        upstreams[0].options = json!({ "secret_headers": ["x-gateway-key"] });
        *secrets = Some([("x-gateway-key".to_owned(), GATEWAY_SECRET.to_owned())].into());
    }
    let provider = make(&world, &c.boss, &body).await;
    let id = provider.base.id;
    assert_eq!(names(&world, &c.boss, None).await, ["Team key"]);
    assert!(names(&world, &c.stranger, None).await.is_empty());
    assert!(names(&world, &c.viewer, None).await.is_empty());
    let refused = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "provider.view");
    assert_eq!(missing_permissions(&refused), ["provider.view"]);
    let nobody = world.app.call(ProviderGet, &[&id]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    // Named in the Use row, the viewer sees it, runs work on it, and does not see its inside.
    name_person(&world, "provider", id, "provider.use", c.viewer.id()).await;
    assert_eq!(names(&world, &c.viewer, None).await, ["Team key"]);
    assert_eq!(
        names(&world, &c.viewer, Some(ProviderListScope::Usable)).await,
        ["Team key"]
    );
    assert!(
        names(&world, &c.viewer, Some(ProviderListScope::Managed))
            .await
            .is_empty()
    );
    assert!(
        names(&world, &c.viewer, Some(ProviderListScope::Mine))
            .await
            .is_empty()
    );
    let seen = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(seen.base.name, "Team key");
    assert_eq!(seen.owner.username, "boss");
    assert!(seen.key_set, "whether it has a key is not a secret");
    assert!(seen.upstreams.is_empty() && seen.secrets_set.is_empty());
    assert_eq!(seen.runtime_config, json!({}));
    assert!(seen.machine.is_none() && seen.harnesses.is_empty());
    // The owner sees what they manage.
    let own = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(own.upstreams.len(), 1);
    assert_eq!(own.secrets_set, ["x-gateway-key"]);
    assert_eq!(
        names(&world, &c.boss, Some(ProviderListScope::Mine)).await,
        ["Team key"]
    );
    assert_eq!(
        names(&world, &c.boss, Some(ProviderListScope::Managed)).await,
        ["Team key"]
    );
}

#[tokio::test]
async fn a_list_of_providers_narrows_to_an_org_and_costs_the_same_statements_for_a_few_and_for_many()
 {
    let world = world().await;
    let c = company(&world).await;
    let research = make_department(&world, &c.boss, c.acme, "Research", "research").await;
    make(&world, &c.boss, &api_body("Acme one", c.acme)).await;
    make(&world, &c.boss, &api_body("Lab one", research.base.id)).await;
    async fn measure(world: &World, who: &Who) -> (usize, usize) {
        let call = world
            .app
            .call(ProviderList, &[])
            .token(&who.token)
            .query(&ProviderListQuery::default());
        let before = Statements::now();
        let listed = call.send().await.ok();
        (Statements::now() - before, listed.items.len())
    }
    let (few, few_count) = measure(&world, &c.boss).await;
    for index in 0..48 {
        make(&world, &c.boss, &api_body(&format!("More {index}"), c.acme)).await;
    }
    let (many, many_count) = measure(&world, &c.boss).await;
    assert_eq!((few_count, many_count), (2, 50));
    assert_eq!(few, many, "{few} statements for 2 providers, {many} for 50");
    let narrowed = world
        .app
        .call(ProviderList, &[])
        .token(&c.boss.token)
        .query(&ProviderListQuery {
            org_id: Some(research.base.id),
            scope: None,
            ..ProviderListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        narrowed
            .items
            .iter()
            .filter_map(|p| match p {
                orch_api_contract::common::Listed::Visible(row) =>
                    Some(row.content.base.name.as_str()),
                orch_api_contract::common::Listed::Hidden(_) => None,
            })
            .collect::<Vec<_>>(),
        ["Lab one"]
    );
    let mut cursor = None;
    let mut cursors = std::collections::HashSet::new();
    let mut providers = std::collections::HashSet::new();
    loop {
        let page = world
            .app
            .call(ProviderList, &[])
            .token(&c.boss.token)
            .query(&ProviderListQuery {
                limit: Some(7),
                cursor: cursor.clone(),
                ..ProviderListQuery::default()
            })
            .send()
            .await
            .ok();
        assert!(page.items.len() <= 7);
        for item in page.items {
            let id = match item {
                orch_api_contract::common::Listed::Visible(row) => row.content.base.id,
                orch_api_contract::common::Listed::Hidden(row) => row.id,
            };
            assert!(providers.insert(id), "a provider appears on only one page");
        }
        let Some(next) = page.next_cursor else { break };
        assert!(cursors.insert(next.clone()), "a cursor advances");
        cursor = Some(next);
    }
    assert_eq!(providers.len(), 50, "every provider remains reachable");

    let mut cursor = None;
    cursors.clear();
    let mut found_lab = false;
    let mut empty_with_next = false;
    loop {
        let page = world
            .app
            .call(ProviderList, &[])
            .token(&c.boss.token)
            .query(&ProviderListQuery {
                org_id: Some(research.base.id),
                limit: Some(7),
                cursor,
                ..ProviderListQuery::default()
            })
            .send()
            .await
            .ok();
        found_lab |= page.items.iter().any(|item| {
            item.as_visible()
                .is_some_and(|row| row.base.name == "Lab one")
        });
        empty_with_next |= page.items.is_empty() && page.next_cursor.is_some();
        let Some(next) = page.next_cursor else { break };
        assert!(cursors.insert(next.clone()), "a filtered cursor advances");
        cursor = Some(next);
    }
    assert!(found_lab, "a filtered result survives candidate pages");
    assert!(empty_with_next, "an empty filtered page retains its cursor");
}

#[tokio::test]
async fn a_provider_is_changed_by_whoever_manages_it_and_a_change_against_an_old_revision_is_refused()
 {
    let world = world().await;
    let c = company(&world).await;
    let provider = make(&world, &c.boss, &api_body("Team key", c.acme)).await;
    let id = provider.base.id;
    let change = |revision: i64| ProviderUpdateBody {
        name: Some("Renamed".to_owned()),
        upstreams: Some(vec![
            endpoint("openai-responses", "https://api.example.com/v1", json!({})),
            endpoint(
                "anthropic-messages",
                "https://api.example.com/anthropic",
                json!({ "auth": "x-api-key" }),
            ),
        ]),
        config: None,
        arrangements: Some(vec![
            ArrangementType::Separated,
            ArrangementType::CredentialShared,
        ]),
        runtime_config: Some(json!({ "runtime.update": "manual" })),
        expected_revision: revision,
    };
    let refused = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&c.viewer.token)
        .body(&change(1))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    let changed = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&c.boss.token)
        .body(&change(1))
        .send()
        .await
        .ok();
    assert_eq!(changed.provider.base.name, "Renamed");
    assert_eq!(
        changed.provider.protocols,
        ["openai-responses", "anthropic-messages"]
    );
    assert_eq!(changed.provider.upstreams.len(), 2);
    assert_eq!(changed.provider.runtime_config["runtime.update"], "manual");
    assert!(changed.provider.revision > 1);
    assert!(
        changed.warning.unwrap().contains("copy of the key"),
        "a copy of the credential may now leave"
    );
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action LIKE 'provider.%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        [
            "provider.created",
            "provider.updated",
            "provider.arrangements_changed"
        ]
    );
    let stale = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&c.boss.token)
        .body(&change(1))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["name"], "Renamed");
    // Taking away every type, or naming an endpoint twice, is not a change.
    let none = ProviderUpdateBody {
        arrangements: Some(vec![]),
        ..change(changed.provider.revision)
    };
    let invalid = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&c.boss.token)
        .body(&none)
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn the_secret_header_of_an_endpoint_that_goes_is_forgotten_with_it() {
    let world = world().await;
    let c = company(&world).await;
    let mut body = api_body("Team key", c.acme);
    if let CreateProvider::Api {
        upstreams, secrets, ..
    } = &mut body
    {
        upstreams[0].options = json!({ "secret_headers": ["x-gateway-key"] });
        *secrets = Some([("x-gateway-key".to_owned(), GATEWAY_SECRET.to_owned())].into());
    }
    let provider = make(&world, &c.boss, &body).await;
    let id = provider.base.id;
    let changed = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&c.boss.token)
        .body(&ProviderUpdateBody {
            name: None,
            upstreams: Some(vec![endpoint(
                "openai-responses",
                "https://api.example.com/v1",
                json!({}),
            )]),
            config: None,
            arrangements: None,
            runtime_config: None,
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    assert!(changed.provider.secrets_set.is_empty());
    assert!(changed.provider.key_set, "the key stays");
    let left: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1",
    )
    .bind(id)
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(left, ["api_key"]);
}

#[tokio::test]
async fn a_key_is_written_again_by_whoever_manages_the_provider_and_the_value_replaces_the_old_one()
{
    let world = world().await;
    let c = company(&world).await;
    let provider = make(&world, &c.boss, &api_body("Team key", c.acme)).await;
    let id = provider.base.id;
    let refused = world
        .app
        .call(ProviderKeySet, &[&id])
        .token(&c.viewer.token)
        .body(&ProviderKeySetBody {
            api_key: "sk-intruder".to_owned(),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    let done = world
        .app
        .call(ProviderKeySet, &[&id])
        .token(&c.boss.token)
        .body(&ProviderKeySetBody {
            api_key: "sk-second-0003".to_owned(),
        })
        .send()
        .await;
    assert_eq!(done.status, 200, "{:?}", done.body);
    let sealed: Vec<u8> = sqlx::query_scalar(
        "SELECT sealed FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1 AND name = 'api_key'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    let opened = MasterKey::new(MASTER)
        .open(&sealed, &binding("provider", id, "api_key"))
        .unwrap();
    assert_eq!(opened, b"sk-second-0003");
    let rotated: Vec<bool> = sqlx::query_scalar(
        "SELECT rotated_at IS NOT NULL FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1",
    )
    .bind(id)
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(rotated, [true]);
    let audited: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_log WHERE action = 'provider.key_rotated'")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(audited.len(), 1);
    for bad in ["", &"k".repeat(70_000)] {
        let invalid = world
            .app
            .call(ProviderKeySet, &[&id])
            .token(&c.boss.token)
            .body(&ProviderKeySetBody {
                api_key: bad.to_owned(),
            })
            .send()
            .await
            .problem();
        assert_eq!(invalid.code, ErrorCode::RequestInvalid);
    }
    // A secret header is written only if an endpoint lists it.
    let unknown = world
        .app
        .call(ProviderSecretSet, &[&id, &"x-nope"])
        .token(&c.boss.token)
        .body(&ProviderSecretSetBody {
            value: "v".to_owned(),
        })
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_provider_is_switched_off_and_on_by_whoever_manages_it_and_its_status_comes_back_from_what_it_has()
 {
    let world = world().await;
    let c = company(&world).await;
    let provider = make(&world, &c.boss, &api_body("Team key", c.acme)).await;
    let id = provider.base.id;
    add_model(&world, &c.boss, id, "gpt-6-sol").await;
    let refused = world
        .app
        .call(ProviderDisable, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    let off = world
        .app
        .call(ProviderDisable, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(off.base.status, ProviderStatus::Disabled);
    // Switched off, a model added does not wake it.
    add_model(&world, &c.boss, id, "gpt-6-luna").await;
    let still = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(still.base.status, ProviderStatus::Disabled);
    let on = world
        .app
        .call(ProviderEnable, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(on.base.status, ProviderStatus::Ready);
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action IN ('provider.disabled', 'provider.enabled') ORDER BY at, id",
    )
    .fetch_all(&world.db.pool).await.unwrap();
    assert_eq!(audited, ["provider.disabled", "provider.enabled"]);
}

/// A thread of `owner` in a topic of the org, bound to `provider`.
async fn bind_thread(world: &World, c: &Company, provider: Id) -> Id {
    let project = make_project(world, &c.boss, c.acme, "Web").await;
    let topic = make_topic(world, &c.boss, project.base.id, "Login").await;
    let thread = orch_types::new_id();
    let mut conn = world.conn().await;
    insert_node(
        &mut conn,
        thread,
        "thread",
        Some(topic.base.id),
        "Try",
        c.boss.id(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET provider_id = $2 WHERE node_id = $1")
        .bind(thread)
        .bind(provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    thread
}

#[tokio::test]
async fn deleting_a_provider_erases_its_secrets_without_detaching_or_deleting_its_bound_threads() {
    let world = world().await;
    let c = company(&world).await;
    let busy = make(&world, &c.boss, &api_body("Busy", c.acme))
        .await
        .base
        .id;
    let idle = make(&world, &c.boss, &api_body("Idle", c.acme))
        .await
        .base
        .id;
    let thread = bind_thread(&world, &c, busy).await;
    let keep = make(&world, &c.boss, &api_body("Keep", c.acme))
        .await
        .base
        .id;
    sqlx::query("UPDATE providers SET login_pending=$2,login_pending_at=$3 WHERE id=$1")
        .bind(busy)
        .bind(b"sealed-pending-fixture".as_slice())
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let refused = world
        .app
        .call(ProviderDelete, &[&idle])
        .token(&c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    let deleted_bound = world
        .app
        .call(ProviderDelete, &[&busy])
        .token(&c.boss.token)
        .send()
        .await;
    assert_eq!(deleted_bound.status, 204, "{:?}", deleted_bound.body);
    let binding: (Id,Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as("SELECT t.provider_id,n.deleted_at FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.node_id=$1")
        .bind(thread).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        binding,
        (busy, None),
        "its conversation and binding stay available to switch later"
    );
    let pending: bool = sqlx::query_scalar("SELECT login_pending IS NOT NULL OR login_pending_at IS NOT NULL OR login_state IS NOT NULL FROM providers WHERE id=$1")
        .bind(busy).fetch_one(&world.db.pool).await.unwrap();
    assert!(
        !pending,
        "pending native sign-in material is erased even before a runtime exists"
    );
    let deleted = world
        .app
        .call(ProviderDelete, &[&idle])
        .token(&c.boss.token)
        .send()
        .await;
    assert_eq!(deleted.status, 204, "{:?}", deleted.body);
    let gone = world
        .app
        .call(ProviderGet, &[&idle])
        .token(&c.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
    assert_eq!(names(&world, &c.boss, None).await, ["Keep"]);
    let keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1",
    )
    .bind(idle)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(keys, 0, "its key is deleted with it");
    let kept: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1",
    )
    .bind(keep)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(kept, 1, "another provider's key stays");
    let bound_keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM secrets WHERE owner_kind='provider' AND owner_id=$1",
    )
    .bind(busy)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        bound_keys, 0,
        "bound threads do not retain a deleted provider's key"
    );
    let audited: Vec<Option<Id>> =
        sqlx::query_scalar("SELECT object_id FROM audit_log WHERE action = 'provider.deleted'")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    let mut audited = audited;
    audited.sort_unstable();
    let mut expected = vec![Some(busy), Some(idle)];
    expected.sort_unstable();
    assert_eq!(
        audited, expected,
        "one record per deletion, none for the refused call"
    );
}

#[tokio::test]
async fn the_owner_row_of_a_new_provider_names_its_maker_and_a_choice_of_who_else_uses_it_is_kept()
{
    let world = world().await;
    let c = company(&world).await;
    let mut only_me = api_body("Only me", c.acme);
    let mut home = api_body("Home", c.acme);
    if let CreateProvider::Api { access, .. } = &mut only_me {
        *access = Some(orch_api_contract::access::NewResourceAccess::OnlyMe);
    }
    if let CreateProvider::Api { access, .. } = &mut home {
        *access = Some(orch_api_contract::access::NewResourceAccess::Home);
    }
    make(&world, &c.boss, &only_me).await;
    make(&world, &c.boss, &home).await;
    // "Everyone in the home" names the viewers of Acme in the Use row.
    assert_eq!(
        names(&world, &c.viewer, Some(ProviderListScope::Usable)).await,
        ["Home"]
    );
    assert!(names(&world, &c.stranger, None).await.is_empty());
    let _ = name_on_page; // naming people by hand is done in the other tests
}
