//! The lists the dialogs and pages read: the adapters the system has, the capabilities an environment may be given, the drivers the hosts report, the settings of a provider and the threads that are bound to a provider.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::approvals::{ApproverScope, Risk};
use orch_api_contract::common::{ErrorCode, PageQuery};
use orch_api_contract::resources::{
    CapabilityDefaultDecision, CapabilityList, CreateProvider, DriverList, HarnessKindList,
    ProviderCreate, ProviderThreadList, ProviderThreadListReply, ProviderUpdate,
    ProviderUpdateBody, Upstream,
};
use orch_api_contract::settings::{ProviderSettingList, ResolvedSetting, SettingScope};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_db::secrets::MasterKey;
use orch_settings::Target;
use orch_types::{Id, Json};
use places::{Who, make_org, make_project, make_topic, missing_permissions};
use resources::{Company, company, name_person};
use serde_json::json;
use support::World;

// ---------------------------------------------------------------- the adapters

/// A profile of the system for a harness, which has reported `descriptor` (nothing: it never ran).
async fn profile_of_the_system(world: &World, kind: &str, name: &str, descriptor: Json) {
    sqlx::query("INSERT INTO agents (id, name, harness_kind, descriptor) VALUES ($1, $2, $3, $4)")
        .bind(orch_types::new_id())
        .bind(name)
        .bind(kind)
        .bind(descriptor)
        .execute(&world.db.pool)
        .await
        .unwrap();
}

/// A machine of `owner`'s whose connector reported `facts`.
async fn machine_reporting(world: &World, owner: &Who, name: &str, facts: Json) -> Id {
    let mut conn = world.conn().await;
    let machine = fixtures::insert_persons_machine(&mut conn, owner.id(), name)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET facts = $2 WHERE id = $1")
        .bind(machine)
        .bind(facts)
        .execute(&mut *conn)
        .await
        .unwrap();
    machine
}

#[tokio::test]
async fn the_adapters_of_the_system_are_the_kinds_its_profiles_run_and_its_machines_can_drive() {
    let world = World::new().await;
    let c = company(&world).await;
    profile_of_the_system(
        &world,
        "codex",
        "Codex",
        json!({ "kind": "codex", "harness": { "name": "Codex CLI", "version": "0.160.1" } }),
    )
    .await;
    // A profile that never ran has nothing to declare yet; one made for an org is nobody's adapter.
    profile_of_the_system(&world, "claude-code", "Claude Code", json!({})).await;
    machine_reporting(
        &world,
        &c.boss,
        "laptop",
        json!({
            "adapters": [
                { "kind": "codex", "adapter_version": 1, "builtin": true },
                { "kind": "fake", "adapter_version": 1, "builtin": true }
            ],
            "harnesses": [
                { "kind": "codex", "path": "/opt/boss/bin/codex", "version": "0.99.0", "daemon": false },
                { "kind": "codex", "path": "/usr/local/bin/codex", "version": "0.160.1", "daemon": false },
                { "kind": "ghost", "path": "/opt/ghost", "version": "9.9.9", "daemon": false }
            ]
        }),
    )
    .await;
    // Anybody signed in reads the list, whose machine it was is nobody's business.
    let reply = world
        .app
        .call(HarnessKindList, &[])
        .token(&c.stranger.token)
        .send()
        .await;
    let raw = String::from_utf8_lossy(&reply.raw).into_owned();
    let kinds = reply.ok();
    assert_eq!(
        kinds
            .iter()
            .map(|each| each.kind.as_str())
            .collect::<Vec<_>>(),
        ["claude-code", "codex", "fake"],
        "a harness that no adapter drives is not an adapter of the system's"
    );
    assert!(
        !raw.contains("/opt/boss") && !raw.contains("laptop"),
        "{raw}"
    );
    let codex = &kinds[1];
    assert_eq!(codex.title, "Codex CLI", "the adapter names itself");
    assert_eq!(
        codex
            .builds
            .iter()
            .map(|build| (build.version.as_str(), build.is_default))
            .collect::<Vec<_>>(),
        [("0.160.1", true), ("0.99.0", false)],
        "one build is named once, and with no profile naming another the newest is what runs"
    );
    assert_eq!(
        codex.descriptor.as_ref().expect("it reported one")["harness"]["name"],
        "Codex CLI"
    );
    assert_eq!(
        codex.harness.name, "Codex CLI",
        "the name in a payload is the one the adapter declared"
    );
    assert_eq!(codex.harness.kind, "codex");
    assert!(
        codex
            .program
            .as_ref()
            .is_some_and(|program| program.builtin),
        "a program of the registry says what its manifest says"
    );
    let never_ran = &kinds[0];
    assert_eq!(
        never_ran.title, "Claude Code",
        "the profile of the system names it"
    );
    assert!(never_ran.builds.is_empty());
    assert_eq!(
        never_ran.descriptor, None,
        "a kind no profile has reported is not described by an empty object"
    );
    assert_eq!(never_ran.harness.name, "Claude Code");
    assert_eq!(never_ran.harness.in_language("zh-CN"), "Claude Code");
    let only_driven = &kinds[2];
    assert_eq!(
        only_driven.title, "Example agent",
        "the manifest of a program names it where no profile does"
    );
    assert_eq!(only_driven.harness.in_language("zh-CN"), "示例代理");
}

#[tokio::test]
async fn the_builds_a_release_lists_are_the_builds_of_the_adapters_it_has_and_its_default_is_the_one_that_runs()
 {
    // The release lists two builds of Codex (the older the default) and one of a harness no adapter drives.
    let directory = tempfile::tempdir().unwrap();
    let payload = directory.path().join("connectors");
    std::fs::create_dir(&payload).unwrap();
    let listed = |kind: &str, version: &str, default: bool| {
        json!({ "kind": kind, "version": version, "os": "linux", "arch": "x86_64",
                "url": format!("https://example.com/{kind}-{version}"), "sha256": "a".repeat(64),
                "is_default": default, "source": "release" })
    };
    std::fs::write(
        payload.join("version.json"),
        json!({ "version": "1.4.0", "harness_builds": [
            listed("codex", "0.162.1", true), listed("codex", "0.163.0", false),
            listed("claude-code", "2.1.296", true), listed("ghost", "9.9.9", true)
        ] })
        .to_string(),
    )
    .unwrap();
    let mut config = orch_config::ServerConfig::default();
    config.database.migrations_dir = directory
        .path()
        .join("migrations")
        .to_str()
        .unwrap()
        .to_owned();
    let world = World::with(config).await;
    let c = company(&world).await;
    profile_of_the_system(&world, "codex", "Codex", json!({})).await;
    profile_of_the_system(&world, "claude-code", "Claude Code", json!({})).await;
    // A machine found a newer one than the release's default; that does not make it the one that runs.
    machine_reporting(
        &world,
        &c.boss,
        "laptop",
        json!({
            "adapters": [{ "kind": "codex", "adapter_version": 1, "builtin": true }],
            "harnesses": [{ "kind": "codex", "path": "/usr/local/bin/codex", "version": "0.170.0", "daemon": false }]
        }),
    )
    .await;
    let kinds = world
        .app
        .call(HarnessKindList, &[])
        .token(&c.stranger.token)
        .send()
        .await
        .ok();
    let builds = |kind: &str| {
        kinds
            .iter()
            .find(|each| each.kind == kind)
            .map(|each| {
                each.builds
                    .iter()
                    .map(|build| (build.version.clone(), build.is_default))
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    assert_eq!(
        kinds
            .iter()
            .map(|each| each.kind.as_str())
            .collect::<Vec<_>>(),
        ["claude-code", "codex"],
        "a harness that no adapter drives is not listed for its builds either"
    );
    assert_eq!(
        builds("codex"),
        [
            ("0.170.0".to_owned(), false),
            ("0.163.0".to_owned(), false),
            ("0.162.1".to_owned(), true)
        ],
        "the default is the one the release names, whatever else there is"
    );
    assert_eq!(builds("claude-code"), [("2.1.296".to_owned(), true)]);
}

#[tokio::test]
async fn a_machine_lists_the_executors_it_can_offer_those_of_its_release_and_the_plugins_it_reported()
 {
    use orch_api_contract::programs::MachineExecutorOrigin;
    use orch_api_contract::resources::MachineExecutorList;
    let world = World::new().await;
    let c = company(&world).await;
    let machine = machine_reporting(
        &world,
        &c.boss,
        "laptop",
        json!({ "plugins": [
            { "kind": "executor", "name": "acme-tools", "version": "1.0.0", "state": "running",
              "executor": {
                "kind": "acme-exec", "title": "Acme tools", "titles": { "zh-CN": "Acme 工具" },
                "protocol": "mcp", "tool_sets": [ { "name": "all", "summary": "Everything it has." } ],
                "sandbox_modes": ["open"], "attribution": null } },
            { "kind": "executor", "name": "wants-a-shipped-kind", "version": "1.0.0", "state": "running",
              "executor": {
                "kind": "orch-exec", "title": "Not Orch's", "protocol": "mcp", "tool_sets": [],
                "sandbox_modes": ["open"], "attribution": null } },
            { "kind": "executor", "name": "has-not-said", "version": "", "state": "registered" },
            { "kind": "provider", "name": "a-source", "version": "1.0.0", "state": "running" }
        ] }),
    )
    .await;
    let listed = world
        .app
        .call(MachineExecutorList, &[&machine])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    let kinds: Vec<_> = listed
        .iter()
        .map(|each| (each.executor.kind.as_str(), each.origin))
        .collect();
    assert_eq!(
        kinds,
        [
            ("codex-exec-server", MachineExecutorOrigin::Builtin),
            ("orch-exec", MachineExecutorOrigin::Builtin),
            ("acme-exec", MachineExecutorOrigin::Plugin),
        ],
        "the release's executors, then the plugin's; a plugin does not take a shipped kind, and one that has not said what it is is not offered"
    );
    let acme = &listed[2];
    assert_eq!(acme.state.as_deref(), Some("running"));
    assert_eq!(acme.executor.title.get("zh-CN"), "Acme 工具");
    assert_eq!(acme.executor.offers[0].value, "acme-exec:all");
    assert_eq!(
        listed[1].executor.offers[1].value, "orch-exec:plain",
        "the tool sets of a shipped executor are offered by name"
    );
    let stranger = world
        .app
        .call(MachineExecutorList, &[&machine])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_adapters_are_read_by_whoever_is_signed_in_and_by_nobody_else() {
    let world = World::new().await;
    let c = company(&world).await;
    let nobody = world.app.call(HarnessKindList, &[]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    let empty = world
        .app
        .call(HarnessKindList, &[])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert!(
        empty.is_empty(),
        "a system with no profile and no machine has no adapter"
    );
}

async fn how_many_adapters(world: &World, by: &Who) -> usize {
    world
        .app
        .call(HarnessKindList, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
        .len()
}

#[tokio::test]
async fn a_machine_that_was_removed_no_longer_says_what_it_can_drive() {
    let world = World::new().await;
    let c = company(&world).await;
    let machine = machine_reporting(
        &world,
        &c.boss,
        "laptop",
        json!({ "adapters": [{ "kind": "fake", "adapter_version": 1, "builtin": true }], "harnesses": [] }),
    )
    .await;
    assert_eq!(how_many_adapters(&world, &c.boss).await, 1);
    sqlx::query("UPDATE machines SET deleted_at = now() WHERE id = $1")
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(how_many_adapters(&world, &c.boss).await, 0);
}

// ---------------------------------------------------------------- the capabilities and the drivers

async fn a_capability(world: &World, key: &str, risk: &str, decision: &str, scope: &str) {
    sqlx::query(
        "INSERT INTO capability_defs (key, label, description, risk, driver_patch, default_decision,
                                      default_approver_scope, builtin, revision)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 3)",
    )
    .bind(key)
    .bind(format!("The {key}"))
    .bind(format!("Lets an environment use the {key}."))
    .bind(risk)
    .bind(json!({ "docker": { "devices": [key] } }))
    .bind(decision)
    .bind(scope)
    .bind(key == "gpu")
    .execute(&world.db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_capabilities_are_listed_by_key_with_what_each_does_for_a_driver() {
    let world = World::new().await;
    let c = company(&world).await;
    a_capability(&world, "gpu", "high", "ask", "host").await;
    a_capability(&world, "debugger", "medium", "allow", "org").await;
    let listed = world
        .app
        .call(CapabilityList, &[])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(
        listed
            .iter()
            .map(|each| each.key.as_str())
            .collect::<Vec<_>>(),
        ["debugger", "gpu"]
    );
    let gpu = &listed[1];
    assert_eq!(gpu.label, "The gpu");
    assert_eq!(gpu.risk, Risk::High);
    assert_eq!(gpu.default_decision, CapabilityDefaultDecision::Ask);
    assert_eq!(gpu.default_approver_scope, ApproverScope::Host);
    assert!(gpu.builtin && !listed[0].builtin);
    assert_eq!(gpu.revision, 3);
    assert_eq!(gpu.driver_patch["docker"], json!({ "devices": ["gpu"] }));
    let nobody = world.app.call(CapabilityList, &[]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

/// A host of the system whose node reported `drivers`.
async fn host_reporting(world: &World, name: &str, drivers: Json) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(&mut conn, "host", None, None, name)
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET drivers = $2 WHERE id = $1")
        .bind(id)
        .bind(drivers)
        .execute(&mut *conn)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn the_drivers_are_the_ones_the_hosts_report_counted_by_host_and_described_by_the_newest_build()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let docker = |version: &str, title: &str| {
        json!({ "docker": { "name": "docker", "version": version, "title": title,
                            "spec_schemas": { "environment": { "type": "object" } } } })
    };
    host_reporting(&world, "first", docker("1.9.0", "Docker")).await;
    host_reporting(&world, "second", docker("1.10.0", "Docker engine")).await;
    let third = host_reporting(
        &world,
        "third",
        json!({ "docker": { "name": "docker", "version": "1.2.0", "title": "Old Docker" },
                "host": { "name": "host", "version": "0.1.0", "title": "A directory of the node" } }),
    )
    .await;
    let drivers = world
        .app
        .call(DriverList, &[])
        .token(&c.stranger.token)
        .send()
        .await
        .ok();
    assert_eq!(
        drivers
            .iter()
            .map(|each| (each.name.as_str(), each.hosts))
            .collect::<Vec<_>>(),
        [("docker", 3), ("host", 1)]
    );
    let docker = &drivers[0];
    assert_eq!(docker.title, "Docker engine", "1.10.0 is newer than 1.9.0");
    assert_eq!(
        docker.descriptor["spec_schemas"]["environment"]["type"],
        "object"
    );
    assert_eq!(drivers[1].title, "A directory of the node");
    // A host whose machine was removed no longer reports anything.
    sqlx::query("UPDATE machines SET deleted_at = now() WHERE id = (SELECT machine_id FROM hosts WHERE id = $1)")
        .bind(third)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let after = world
        .app
        .call(DriverList, &[])
        .token(&c.stranger.token)
        .send()
        .await
        .ok();
    assert_eq!(
        after
            .iter()
            .map(|each| (each.name.as_str(), each.hosts))
            .collect::<Vec<_>>(),
        [("docker", 2)]
    );
    let nobody = world.app.call(DriverList, &[]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

// ---------------------------------------------------------------- what is known of a provider

fn api_body(name: &str, home: Id) -> CreateProvider {
    CreateProvider::Api {
        runtime_machine_id: None,
        name: name.to_owned(),
        home_id: home,
        access: Some(NewResourceAccess::OnlyMe),
        arrangements: None,
        runtime_config: None,
        upstreams: vec![Upstream {
            protocol: "openai-responses".to_owned(),
            base_url: "https://api.example.com/v1".to_owned(),
            options: json!({}),
        }],
        api_key: Some("sk-live-do-not-show".to_owned()),
        secrets: None,
        key_machine_id: None,
        sources: None,
    }
}

/// The key every provider of these tests is sealed under.
const MASTER: [u8; 32] = [7; 32];

async fn a_provider(world: &World, c: &Company) -> Id {
    install_master_key(MasterKey::new(MASTER));
    world
        .set("providers.allow_http_upstreams", json!(false))
        .await;
    world
        .app
        .call(ProviderCreate, &[])
        .token(&c.boss.token)
        .body(&api_body("Team key", c.acme))
        .send()
        .await
        .ok()
        .provider
        .base
        .id
}

async fn settings_of(world: &World, by: &Who, provider: Id) -> Vec<ResolvedSetting> {
    world
        .app
        .call(ProviderSettingList, &[&provider])
        .token(&by.token)
        .send()
        .await
        .ok()
}

fn setting<'a>(all: &'a [ResolvedSetting], key: &str) -> &'a ResolvedSetting {
    all.iter().find(|each| each.key == key).expect(key)
}

#[tokio::test]
async fn the_settings_of_a_provider_are_what_its_home_resolves_with_its_own_values_on_top() {
    let world = World::new().await;
    let c = company(&world).await;
    let provider = a_provider(&world, &c).await;
    let idle = "runtime.idle_stop_minutes";
    let before = settings_of(&world, &c.boss, provider).await;
    assert_eq!(setting(&before, idle).source.scope, SettingScope::Default);
    world.set_at(Target::Node(c.acme), idle, json!(60)).await;
    let from_the_org = settings_of(&world, &c.boss, provider).await;
    let resolved = setting(&from_the_org, idle);
    assert_eq!(resolved.value, Some(json!(60)));
    assert_eq!(resolved.source.scope, SettingScope::Org);
    assert_eq!(
        resolved.source.node.as_ref().map(|node| node.id),
        Some(c.acme)
    );
    // What the provider keeps for itself is applied last, and reads back as its own.
    let revision = world
        .app
        .call(orch_api_contract::resources::ProviderGet, &[&provider])
        .token(&c.boss.token)
        .send()
        .await
        .ok()
        .revision;
    world
        .app
        .call(ProviderUpdate, &[&provider])
        .token(&c.boss.token)
        .body(&ProviderUpdateBody {
            name: None,
            upstreams: None,
            config: None,
            arrangements: None,
            runtime_config: Some(json!({ idle: 15 })),
            expected_revision: revision,
        })
        .send()
        .await
        .ok();
    let own = settings_of(&world, &c.boss, provider).await;
    let resolved = setting(&own, idle);
    assert_eq!(resolved.value, Some(json!(15)));
    assert_eq!(resolved.source.scope, SettingScope::ProviderOwn);
    // Only keys valid on a provider's runtime are there.
    assert!(own.iter().any(|each| each.key == "runtime.template_id"));
    assert!(
        !own.iter().any(|each| each.key == "agent.model"),
        "a key of a thread is no key of a provider"
    );
}

#[tokio::test]
async fn the_settings_of_a_provider_are_written_by_whoever_manages_it_and_read_by_whoever_holds_any_permission()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let provider = a_provider(&world, &c).await;
    name_person(&world, "provider", provider, "provider.use", c.viewer.id()).await;
    let idle = "runtime.idle_stop_minutes";
    let manager = settings_of(&world, &c.boss, provider).await;
    assert!(setting(&manager, idle).writable);
    let user = settings_of(&world, &c.viewer, provider).await;
    assert!(
        user.iter().all(|each| !each.writable),
        "a person who may only use the provider writes nothing of its runtime"
    );
    let refused = world
        .app
        .call(ProviderSettingList, &[&provider])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&refused),
        ["provider.view"],
        "{refused:?}"
    );
    let nobody = world
        .app
        .call(ProviderSettingList, &[&provider])
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    let gone = world
        .app
        .call(ProviderSettingList, &[&orch_types::new_id()])
        .token(&c.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------- the threads that are bound to a provider

/// A thread of `owner`'s in `topic`, bound to `provider`.
async fn a_thread_on(world: &World, topic: Id, name: &str, owner: &Who, provider: Id) -> Id {
    let thread = orch_types::new_id();
    let mut conn = world.conn().await;
    insert_node(&mut conn, thread, "thread", Some(topic), name, owner.id())
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET provider_id = $2, state = 'running' WHERE node_id = $1")
        .bind(thread)
        .bind(provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    thread
}

async fn threads_of(
    world: &World,
    by: &Who,
    provider: Id,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> ProviderThreadListReply {
    world
        .app
        .call(ProviderThreadList, &[&provider])
        .token(&by.token)
        .query(&PageQuery {
            limit,
            cursor: cursor.map(str::to_owned),
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn the_threads_on_a_provider_are_those_the_caller_may_view_of_whatever_org_and_the_rest_are_counted()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let provider = a_provider(&world, &c).await;
    // The stranger works with the provider from an org of their own.
    name_person(
        &world,
        "provider",
        provider,
        "provider.use",
        c.stranger.id(),
    )
    .await;
    let project = make_project(&world, &c.boss, c.acme, "Web").await;
    let topic = make_topic(&world, &c.boss, project.base.id, "Login").await;
    let first = a_thread_on(&world, topic.base.id, "Try", &c.boss, provider).await;
    let second = a_thread_on(&world, topic.base.id, "Again", &c.boss, provider).await;
    let other = make_org(&world, &c.stranger, "Other", "other").await;
    let other_project = make_project(&world, &c.stranger, other.base.id, "Mobile").await;
    let other_topic = make_topic(&world, &c.stranger, other_project.base.id, "Push").await;
    let foreign = a_thread_on(
        &world,
        other_topic.base.id,
        "Elsewhere",
        &c.stranger,
        provider,
    )
    .await;

    let seen_by_boss = threads_of(&world, &c.boss, provider, None, None).await;
    assert_eq!(
        seen_by_boss
            .items
            .iter()
            .map(|each| each.thread.id)
            .collect::<Vec<_>>(),
        [first, second]
    );
    assert_eq!(
        seen_by_boss.hidden, 1,
        "the thread of another org is counted and not shown"
    );
    assert!(seen_by_boss.next_cursor.is_none());
    let one = &seen_by_boss.items[0];
    assert_eq!(one.owner.username, "boss");
    assert_eq!(one.topic.id, topic.base.id);
    assert_eq!(one.org.id, c.acme);
    assert_eq!(one.state, "running");
    assert!(one.blocked_reason.is_none());

    let seen_by_stranger = threads_of(&world, &c.stranger, provider, None, None).await;
    assert_eq!(
        seen_by_stranger
            .items
            .iter()
            .map(|each| each.thread.id)
            .collect::<Vec<_>>(),
        [foreign]
    );
    assert_eq!(
        seen_by_stranger.items[0].org.id, other.base.id,
        "the org of the thread, which is not the provider's home"
    );
    assert_eq!(seen_by_stranger.hidden, 2);
}

#[tokio::test]
async fn a_long_list_of_threads_on_a_provider_comes_in_pages_that_do_not_repeat_or_skip() {
    let world = World::new().await;
    let c = company(&world).await;
    let provider = a_provider(&world, &c).await;
    let project = make_project(&world, &c.boss, c.acme, "Web").await;
    let topic = make_topic(&world, &c.boss, project.base.id, "Login").await;
    let mut made = Vec::new();
    for index in 0..5 {
        made.push(
            a_thread_on(
                &world,
                topic.base.id,
                &format!("Try {index}"),
                &c.boss,
                provider,
            )
            .await,
        );
    }
    let mut collected = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = threads_of(&world, &c.boss, provider, Some(2), cursor.as_deref()).await;
        assert!(page.items.len() <= 2);
        collected.extend(page.items.iter().map(|each| each.thread.id));
        pages += 1;
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(collected, made);
    let bad = world
        .app
        .call(ProviderThreadList, &[&provider])
        .token(&c.boss.token)
        .query(&PageQuery {
            limit: None,
            cursor: Some("not a cursor".to_owned()),
        })
        .send()
        .await
        .problem();
    assert_eq!(bad.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn the_threads_on_a_provider_are_listed_to_whoever_holds_any_permission_on_it_and_to_nobody_else()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let provider = a_provider(&world, &c).await;
    let refused = world
        .app
        .call(ProviderThreadList, &[&provider])
        .token(&c.stranger.token)
        .query(&PageQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&refused),
        ["provider.view"],
        "{refused:?}"
    );
    let nobody = world
        .app
        .call(ProviderThreadList, &[&provider])
        .query(&PageQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    // A provider that is gone is not there to ask about, whoever asks.
    let gone = world
        .app
        .call(ProviderThreadList, &[&orch_types::new_id()])
        .token(&c.boss.token)
        .query(&PageQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
    // One who only looks at the provider sees the threads of their own org.
    name_person(&world, "provider", provider, "provider.view", c.viewer.id()).await;
    let project = make_project(&world, &c.boss, c.acme, "Web").await;
    let topic = make_topic(&world, &c.boss, project.base.id, "Login").await;
    let thread = a_thread_on(&world, topic.base.id, "Try", &c.boss, provider).await;
    let looked = threads_of(&world, &c.viewer, provider, None, None).await;
    assert_eq!(
        looked
            .items
            .iter()
            .map(|each| each.thread.id)
            .collect::<Vec<_>>(),
        [thread]
    );
}
