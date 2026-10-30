//! What a system admin keeps for the whole system in Management: the definitions of capabilities, the builds of harnesses, the agent profiles that belong to no org, and the single sign-in methods.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use admin_support::{Cast, assert_no_system_admin, cast, lines_of, refused_to};
use orch_api_contract::access::Role;
use orch_api_contract::admin::{
    AdminAgentCreate, AdminCapabilityDelete, AdminCapabilityList, AdminCapabilitySet,
    AdminHarnessList, AdminHarnessSet, AdminIdentityProviderCreate, AdminIdentityProviderDelete,
    AdminIdentityProviderList, AdminIdentityProviderUpdate, AdminOrgUpdate, AdminUserCreate,
    AdminUserCreateBody, HarnessArch, HarnessBuild, HarnessBuildSource, IdentityProvider,
    IdentityProviderInput, IdentityProviderUpdateBody, OrgLimitsChange,
};
use orch_api_contract::approvals::{ApproverScope, Risk};
use orch_api_contract::common::{BASE_PATH, ErrorCode};
use orch_api_contract::resources::{
    Agent, AgentGet, CapabilityDef, CapabilityDefaultDecision, MachineOs, OrgAgentCreateBody,
};
use orch_api_core::testing::Reply;
use orch_config::ServerConfig;
use orch_db::secrets::MasterKey;
use orch_types::{Id, new_id};
use places::Who;
use serde_json::json;
use std::collections::BTreeMap;
use support::World;

async fn sql(world: &World, statement: &str) {
    sqlx::query(sqlx::AssertSqlSafe(statement.to_owned()))
        .execute(&world.db.pool)
        .await
        .unwrap();
}

// ---- capabilities ----

fn gpu() -> CapabilityDef {
    CapabilityDef {
        key: "gpu".into(),
        label: "A graphics card".into(),
        description: "Lets an environment use the graphics card of its host.".into(),
        risk: Risk::High,
        driver_patch: BTreeMap::from([("docker".to_owned(), json!({ "devices": ["/dev/dri"] }))]),
        default_decision: CapabilityDefaultDecision::Ask,
        default_approver_scope: ApproverScope::Org,
        builtin: false,
        revision: 0,
    }
}

async fn define(world: &World, by: &Who, def: &CapabilityDef) -> Reply<CapabilityDef> {
    world
        .app
        .call(AdminCapabilitySet, &[&def.key])
        .token(&by.token)
        .body(def)
        .send()
        .await
}

async fn capabilities(world: &World, by: &Who) -> Vec<CapabilityDef> {
    world
        .app
        .call(AdminCapabilityList, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
}

async fn remove_capability(world: &World, by: &Who, key: &str) -> Reply<CapabilityDef> {
    world
        .app
        .call(AdminCapabilityDelete, &[&key])
        .token(&by.token)
        .send()
        .await
}

#[tokio::test]
async fn a_capability_is_added_changed_by_the_revision_that_was_seen_and_removed_and_each_act_leaves_a_line()
 {
    let world = World::new().await;
    let c = cast(&world).await;

    let added = define(&world, &c.root, &gpu()).await.ok();
    let changed = define(
        &world,
        &c.root,
        &CapabilityDef {
            label: "Graphics".into(),
            builtin: true,
            revision: added.revision,
            ..gpu()
        },
    )
    .await
    .ok();
    let stale = define(
        &world,
        &c.root,
        &CapabilityDef {
            label: "Too late".into(),
            revision: added.revision,
            ..gpu()
        },
    )
    .await
    .problem();
    let listed = capabilities(&world, &c.root).await;
    let removed = remove_capability(&world, &c.root, "gpu").await.ok();
    let again = remove_capability(&world, &c.root, "gpu").await.problem();

    assert_eq!(
        (added.key.as_str(), added.revision, added.builtin),
        ("gpu", 1, false)
    );
    assert_eq!(
        added.driver_patch["docker"],
        json!({ "devices": ["/dev/dri"] })
    );
    assert_eq!(added.risk, Risk::High);
    assert_eq!(
        (changed.label.as_str(), changed.revision, changed.builtin),
        ("Graphics", 2, false),
        "a body cannot make a definition ship with the system"
    );
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["revision"], json!(2));
    assert_eq!(stale.extra["current"]["label"], json!("Graphics"));
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].label, "Graphics",
        "the stale write changed nothing"
    );
    assert_eq!(removed.key, "gpu");
    assert!(capabilities(&world, &c.root).await.is_empty());
    assert_eq!(again.code, ErrorCode::NotFound);
    let lines = lines_of(&world, "capability_def.updated").await;
    let how: Vec<_> = lines
        .iter()
        .map(|line| line.details["how"].as_str().unwrap())
        .collect();
    assert_eq!(
        how,
        ["added", "replaced", "removed"],
        "one line for each act that changed something"
    );
    assert!(lines.iter().all(|line| line.actor_id == Some(c.root.id())));
}

#[tokio::test]
async fn a_capability_that_ships_is_changed_and_stays_and_one_that_an_environment_holds_is_kept() {
    let world = World::new().await;
    let c = cast(&world).await;
    sql(
        &world,
        "INSERT INTO capability_defs (key, label, description, risk, driver_patch, default_decision,
                                      default_approver_scope, builtin)
         VALUES ('net_admin', 'Network administration', 'Changes the network of its environment.',
                 'high', '{\"docker\":{\"cap_add\":[\"NET_ADMIN\"]}}', 'ask', 'host', true)",
    )
    .await;
    let shipped = capabilities(&world, &c.root).await.remove(0);

    let denied = define(
        &world,
        &c.root,
        &CapabilityDef {
            default_decision: CapabilityDefaultDecision::Deny,
            ..shipped.clone()
        },
    )
    .await
    .ok();
    let kept = remove_capability(&world, &c.root, "net_admin")
        .await
        .problem();

    assert!(shipped.builtin);
    assert_eq!(denied.default_decision, CapabilityDefaultDecision::Deny);
    assert!(denied.builtin, "it still ships");
    assert_eq!(kept.code, ErrorCode::RequestInvalid);
    assert_eq!(kept.extra["errors"][0]["path"], json!("key"));
    assert_eq!(capabilities(&world, &c.root).await.len(), 1);

    define(&world, &c.root, &gpu()).await.ok();
    let environment = {
        let mut conn = world.conn().await;
        orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "environment",
            Some(c.boss.id()),
            Some(c.acme),
            "Work",
        )
        .await
        .unwrap()
    };
    sqlx::query("UPDATE environments SET granted_capabilities = ARRAY['gpu'] WHERE id = $1")
        .bind(environment)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let held = remove_capability(&world, &c.root, "gpu").await.problem();
    sql(
        &world,
        "UPDATE environments SET granted_capabilities = '{}'",
    )
    .await;
    let free = remove_capability(&world, &c.root, "gpu").await.ok();

    assert_eq!(held.code, ErrorCode::ResourceInUse);
    assert_eq!(held.extra["environments"], json!(1));
    assert_eq!(free.key, "gpu", "it goes once nothing holds it");
}

#[tokio::test]
async fn a_definition_that_does_not_fit_is_refused_and_changes_nothing() {
    let world = World::new().await;
    let c = cast(&world).await;
    let at = |key: &'static str, def: CapabilityDef| {
        let (world, root) = (&world, &c.root);
        async move {
            world
                .app
                .call(AdminCapabilitySet, &[&key])
                .token(&root.token)
                .body(&def)
                .send()
                .await
                .problem()
        }
    };

    let other_key = at(
        "gpu",
        CapabilityDef {
            key: "tpu".into(),
            ..gpu()
        },
    )
    .await;
    let no_label = at(
        "gpu",
        CapabilityDef {
            label: "  ".into(),
            ..gpu()
        },
    )
    .await;
    let long = at(
        "gpu",
        CapabilityDef {
            description: "x".repeat(1_001),
            ..gpu()
        },
    )
    .await;
    let patch = at(
        "gpu",
        CapabilityDef {
            driver_patch: BTreeMap::from([("docker".to_owned(), json!(5))]),
            ..gpu()
        },
    )
    .await;
    let no_such_key = at(
        "GPU",
        CapabilityDef {
            key: "GPU".into(),
            ..gpu()
        },
    )
    .await;

    for (problem, path) in [
        (&other_key, "key"),
        (&no_label, "label"),
        (&long, "description"),
        (&patch, "driver_patch.docker"),
    ] {
        assert_eq!(problem.code, ErrorCode::RequestInvalid, "{path}");
        assert_eq!(problem.extra["errors"][0]["path"], json!(path));
    }
    assert_eq!(no_such_key.code, ErrorCode::NotFound, "a key is lower case");
    assert!(capabilities(&world, &c.root).await.is_empty());
    assert!(lines_of(&world, "capability_def.updated").await.is_empty());
}

// ---- harness builds ----

const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn build(version: &str, default: bool) -> HarnessBuild {
    HarnessBuild {
        kind: "codex".into(),
        version: version.into(),
        os: MachineOs::Linux,
        arch: HarnessArch::X86_64,
        url: format!("https://example.com/codex-{version}"),
        sha256: SHA_B.into(),
        is_default: default,
        source: HarnessBuildSource::Release,
        status: orch_api_contract::admin::HarnessBuildStatus::Untested,
        check: None,
        carry: None,
    }
}

async fn add_build(world: &World, by: &Who, build: &HarnessBuild) -> Reply<Vec<HarnessBuild>> {
    world
        .app
        .call(AdminHarnessSet, &[&build.kind, &build.version])
        .token(&by.token)
        .body(build)
        .send()
        .await
}

async fn builds(world: &World, by: &Who) -> Vec<HarnessBuild> {
    world
        .app
        .call(AdminHarnessList, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
}

/// A world whose release lists two builds of a harness in its manifest.
fn release_with_builds() -> (tempfile::TempDir, ServerConfig) {
    let directory = tempfile::tempdir().unwrap();
    let payload = directory.path().join("connectors");
    std::fs::create_dir(&payload).unwrap();
    let manifest = json!({
        "version": "1.4.0",
        "harness_builds": [
            { "kind": "codex", "version": "1.0.0", "os": "linux", "arch": "x86_64",
              "url": "https://example.com/codex-1.0.0", "sha256": SHA_A,
              "is_default": true, "source": "release" },
            { "kind": "codex", "version": "0.9.0", "os": "linux", "arch": "x86_64",
              "url": "https://example.com/codex-0.9.0", "sha256": SHA_A,
              "is_default": false, "source": "release" },
            { "this": "is no build" }
        ]
    });
    std::fs::write(payload.join("version.json"), manifest.to_string()).unwrap();
    let mut config = ServerConfig::default();
    config.database.migrations_dir = directory
        .path()
        .join("migrations")
        .to_str()
        .unwrap()
        .to_owned();
    (directory, config)
}

fn versions(all: &[HarnessBuild]) -> Vec<(&str, HarnessBuildSource, bool)> {
    all.iter()
        .map(|each| (each.version.as_str(), each.source, each.is_default))
        .collect()
}

#[tokio::test]
async fn the_builds_a_release_lists_are_read_from_its_manifest_and_an_added_build_takes_the_default()
 {
    let (_release, config) = release_with_builds();
    let world = World::with(config).await;
    let c = cast(&world).await;
    use HarnessBuildSource::{Admin, Release};

    let from_release = builds(&world, &c.root).await;
    let with_new = add_build(&world, &c.root, &build("1.1.0", true)).await.ok();
    let replaced = add_build(
        &world,
        &c.root,
        &HarnessBuild {
            url: "https://mirror.example.com/codex-1.0.0".into(),
            ..build("1.0.0", false)
        },
    )
    .await
    .ok();
    let newest = add_build(&world, &c.root, &build("1.2.0", true)).await.ok();

    assert_eq!(
        versions(&from_release),
        [("0.9.0", Release, false), ("1.0.0", Release, true)],
        "what is no build is not listed"
    );
    assert_eq!(
        versions(&with_new),
        [
            ("0.9.0", Release, false),
            ("1.0.0", Release, false),
            ("1.1.0", Admin, true)
        ],
        "one default for a harness on a system and a processor, and the one added is it"
    );
    assert_eq!(
        replaced.len(),
        3,
        "a build of the same name is replaced, not added"
    );
    let one = replaced
        .iter()
        .find(|each| each.version == "1.0.0")
        .unwrap();
    assert_eq!(one.source, Admin);
    assert_eq!(one.url, "https://mirror.example.com/codex-1.0.0");
    assert_eq!(
        versions(&newest),
        [
            ("0.9.0", Release, false),
            ("1.0.0", Admin, false),
            ("1.1.0", Admin, false),
            ("1.2.0", Admin, true)
        ],
        "the build added last takes the default from the one before"
    );
    assert_eq!(builds(&world, &c.root).await, newest);
    let lines: Vec<_> = lines_of(&world, "system.settings_updated")
        .await
        .into_iter()
        .filter(|line| line.details["what"] == json!("harness_build"))
        .collect();
    assert_eq!(lines.len(), 3, "each build added leaves its line");
    assert_eq!(lines[0].details["version"], json!("1.1.0"));
}

#[tokio::test]
async fn a_unit_is_told_to_fetch_the_default_build_of_each_harness_it_is_to_run_and_a_mirror_an_admin_set_takes_the_place_of_the_vendors()
 {
    use orch_api_core::hosting::{BuildCatalog, ReleaseBuilds};
    let (release, config) = release_with_builds();
    let world = World::with(config).await;
    let c = cast(&world).await;
    let catalog = ReleaseBuilds::at(
        release.path().join("connectors"),
        "https://orch.example.com",
    );
    let kinds = ["codex".to_owned(), "claude-code".to_owned()];
    let fetched = |tools: Vec<orch_engine::hosting::ToolBuild>| {
        tools
            .into_iter()
            .map(|tool| (tool.name, tool.version, tool.url))
            .collect::<Vec<_>>()
    };
    let mut conn = world.conn().await;

    let from_release = catalog
        .harness_builds(&mut conn, "linux", "x86_64", &kinds)
        .await
        .unwrap();
    assert_eq!(
        fetched(from_release),
        [(
            "codex".to_owned(),
            "1.0.0".to_owned(),
            "https://example.com/codex-1.0.0".to_owned()
        )],
        "the default of the harness the release lists; the one it lists nothing of is left out"
    );
    assert!(
        catalog
            .harness_builds(&mut conn, "linux", "aarch64", &kinds)
            .await
            .unwrap()
            .is_empty(),
        "a host of a processor the release lists no build for is given none"
    );
    assert!(
        catalog
            .harness_builds(&mut conn, "linux", "x86_64", &[])
            .await
            .unwrap()
            .is_empty(),
        "a unit that is to run no harness is asked for none"
    );

    // A system admin points the release's build at a mirror, and adds a newer one that is the default.
    add_build(
        &world,
        &c.root,
        &HarnessBuild {
            url: "https://mirror.example.com/codex-1.0.0".into(),
            ..build("1.0.0", true)
        },
    )
    .await
    .ok();
    let mirrored = catalog
        .harness_builds(&mut conn, "linux", "x86_64", &kinds)
        .await
        .unwrap();
    assert_eq!(
        fetched(mirrored)[0].2,
        "https://mirror.example.com/codex-1.0.0"
    );
    add_build(&world, &c.root, &build("1.1.0", true)).await.ok();
    let newest = catalog
        .harness_builds(&mut conn, "linux", "x86_64", &kinds)
        .await
        .unwrap();
    assert_eq!(
        fetched(newest)[0].1,
        "1.1.0",
        "an added default is the default"
    );
}

#[tokio::test]
async fn without_a_manifest_the_builds_are_those_that_were_added() {
    let world = World::new().await;
    let c = cast(&world).await;

    let none = builds(&world, &c.root).await;
    let one = add_build(&world, &c.root, &build("1.0.0", true)).await.ok();

    assert!(none.is_empty());
    assert_eq!(one.len(), 1);
    assert_eq!(
        one[0].source,
        HarnessBuildSource::Admin,
        "whatever the body claimed"
    );
}

#[tokio::test]
async fn a_build_that_does_not_fit_what_the_address_names_or_what_can_be_fetched_is_refused() {
    let world = World::new().await;
    let c = cast(&world).await;
    let put = |kind: &'static str, version: &'static str, body: HarnessBuild| {
        let (world, root) = (&world, &c.root);
        async move {
            world
                .app
                .call(AdminHarnessSet, &[&kind, &version])
                .token(&root.token)
                .body(&body)
                .send()
                .await
                .problem()
        }
    };

    let other_kind = put(
        "codex",
        "1.0.0",
        HarnessBuild {
            kind: "claude".into(),
            ..build("1.0.0", false)
        },
    )
    .await;
    let other_version = put("codex", "1.0.0", build("1.0.1", false)).await;
    let ftp = put(
        "codex",
        "1.0.0",
        HarnessBuild {
            url: "ftp://example.com/c".into(),
            ..build("1.0.0", false)
        },
    )
    .await;
    let spaced = put(
        "codex",
        "1.0.0",
        HarnessBuild {
            url: "https://example.com/a b".into(),
            ..build("1.0.0", false)
        },
    )
    .await;
    let short = put(
        "codex",
        "1.0.0",
        HarnessBuild {
            sha256: "abc".into(),
            ..build("1.0.0", false)
        },
    )
    .await;
    let upper = put(
        "codex",
        "1.0.0",
        HarnessBuild {
            sha256: SHA_A.to_uppercase(),
            ..build("1.0.0", false)
        },
    )
    .await;
    let odd_kind = put(
        "_codex",
        "1.0.0",
        HarnessBuild {
            kind: "_codex".into(),
            ..build("1.0.0", false)
        },
    )
    .await;

    for (problem, path) in [
        (&other_kind, "kind"),
        (&other_version, "version"),
        (&ftp, "url"),
        (&spaced, "url"),
        (&short, "sha256"),
        (&upper, "sha256"),
        (&odd_kind, "kind"),
    ] {
        assert_eq!(problem.code, ErrorCode::RequestInvalid, "{path}");
        assert_eq!(
            problem.extra["errors"][0]["path"],
            json!(path),
            "{problem:?}"
        );
    }
    assert!(builds(&world, &c.root).await.is_empty());
}

// ---- profiles of the system ----

fn profile(name: &str) -> OrgAgentCreateBody {
    OrgAgentCreateBody {
        name: name.into(),
        harness_kind: "codex".into(),
        profile: json!({ "harness_version": "1.0.0", "env": { "MODE": "plain" } }),
    }
}

async fn make_profile(world: &World, by: &Who, body: &OrgAgentCreateBody) -> Reply<Agent> {
    world
        .app
        .call(AdminAgentCreate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

#[tokio::test]
async fn a_profile_of_the_system_belongs_to_no_org_and_no_person_and_system_admins_hold_it() {
    let world = World::new().await;
    let c = cast(&world).await;

    let made = make_profile(&world, &c.root, &profile("  Shared Codex "))
        .await
        .ok();
    let read = world
        .app
        .call(AgentGet, &[&made.base.id])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    let hidden = world
        .app
        .call(AgentGet, &[&made.base.id])
        .token(&c.ada.token)
        .send_bytes()
        .await;

    assert_eq!(made.base.name, "Shared Codex");
    assert_eq!(made.base.harness_kind, "codex");
    assert!(made.owner.is_none() && made.org_id.is_none() && made.home.is_none());
    assert_eq!(made.profile["harness_version"], json!("1.0.0"));
    assert!(made.access.administers, "a system admin holds it");
    assert!(!made.access.permissions.is_empty());
    assert_eq!(read.base.id, made.base.id);
    assert!(
        !hidden.status.is_success(),
        "an ordinary person does not see it"
    );
    let lines = lines_of(&world, "agent.created").await;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].actor_id, Some(c.root.id()));
    assert_eq!(lines[0].details["system"], json!(true));
}

#[tokio::test]
async fn a_profile_that_is_not_one_is_refused() {
    let world = World::new().await;
    let c = cast(&world).await;
    let refused = |body: OrgAgentCreateBody| {
        let (world, root) = (&world, &c.root);
        async move { make_profile(world, root, &body).await.problem() }
    };

    let no_name = refused(OrgAgentCreateBody {
        name: "  ".into(),
        ..profile("x")
    })
    .await;
    let bad_kind = refused(OrgAgentCreateBody {
        harness_kind: "Co Dex".into(),
        ..profile("x")
    })
    .await;
    let stray = refused(OrgAgentCreateBody {
        profile: json!({ "bogus": 1 }),
        ..profile("x")
    })
    .await;
    let attached = refused(OrgAgentCreateBody {
        profile: json!({ "attach": { "mode": "spawn" } }),
        ..profile("x")
    })
    .await;
    let not_an_object = refused(OrgAgentCreateBody {
        profile: json!([1]),
        ..profile("x")
    })
    .await;

    for (problem, path) in [
        (&no_name, "name"),
        (&bad_kind, "harness_kind"),
        (&stray, "profile.bogus"),
        (&attached, "profile.attach"),
        (&not_an_object, "profile"),
    ] {
        assert_eq!(
            problem.code,
            ErrorCode::RequestInvalid,
            "{path}: {problem:?}"
        );
        assert_eq!(
            problem.extra["errors"][0]["path"],
            json!(path),
            "{problem:?}"
        );
    }
    assert!(lines_of(&world, "agent.created").await.is_empty());
}

// ---- identity providers ----

fn corp() -> IdentityProviderInput {
    IdentityProviderInput {
        key: "corp".into(),
        name: "Corporate sign-in".into(),
        issuer: "https://id.example.com".into(),
        client_id: "orch".into(),
        client_secret: Some("s3cret-value".into()),
        scopes: None,
        claim_subject: None,
        claim_email: None,
        claim_name: None,
        claim_username: None,
        allowed_email_domains: None,
        auto_register: None,
        default_org_id: None,
        default_role: None,
        enabled: None,
    }
}

async fn add_provider(
    world: &World,
    by: &Who,
    input: &IdentityProviderInput,
) -> Reply<IdentityProvider> {
    world
        .app
        .call(AdminIdentityProviderCreate, &[])
        .token(&by.token)
        .body(input)
        .send()
        .await
}

async fn change_provider(
    world: &World,
    by: &Who,
    id: Id,
    body: &IdentityProviderUpdateBody,
) -> Reply<IdentityProvider> {
    world
        .app
        .call(AdminIdentityProviderUpdate, &[&id])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

async fn providers(world: &World, by: &Who) -> Vec<IdentityProvider> {
    world
        .app
        .call(AdminIdentityProviderList, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
}

/// What is kept sealed for the provider, opened with the key of these tests.
async fn kept_secret(world: &World, provider: Id) -> Option<Vec<u8>> {
    let sealed: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT sealed FROM secrets
          WHERE owner_kind = 'identity_provider' AND owner_id = $1 AND name = 'client_secret'",
    )
    .bind(provider)
    .fetch_optional(&world.db.pool)
    .await
    .unwrap();
    sealed.map(|sealed| {
        assert!(
            !sealed.windows(6).any(|part| part == b"s3cret"),
            "it is kept sealed"
        );
        MasterKey::new([7; 32])
            .open(
                &sealed,
                format!("secrets/identity_provider/{provider}/client_secret").as_bytes(),
            )
            .expect("it opens under the system's key, bound to its own row")
    })
}

fn nothing_changed(revision: i64) -> IdentityProviderUpdateBody {
    IdentityProviderUpdateBody {
        expected_revision: revision,
        ..IdentityProviderUpdateBody::default()
    }
}

#[tokio::test]
async fn an_identity_provider_is_added_with_the_usual_claims_and_its_secret_is_kept_sealed_and_never_read_back()
 {
    let world = World::new().await;
    let c = cast(&world).await;

    let reply = add_provider(&world, &c.root, &corp()).await;
    let raw = String::from_utf8(reply.raw.to_vec()).unwrap();
    let made = reply.ok();
    let listed = world
        .app
        .call(AdminIdentityProviderList, &[])
        .token(&c.root.token)
        .send()
        .await;
    let listed_raw = String::from_utf8(listed.raw.to_vec()).unwrap();

    assert_eq!(
        (made.key.as_str(), made.name.as_str()),
        ("corp", "Corporate sign-in")
    );
    assert_eq!(made.scopes, ["openid", "profile", "email"]);
    assert_eq!(
        (
            made.claim_subject.as_str(),
            made.claim_email.as_str(),
            made.claim_name.as_str(),
            made.claim_username.as_str()
        ),
        ("sub", "email", "name", "preferred_username")
    );
    assert!(made.enabled && !made.auto_register);
    assert!(made.allowed_email_domains.is_empty());
    assert!(made.default_org_id.is_none() && made.default_role.is_none());
    assert!(made.client_secret_set);
    assert_eq!(made.revision, 1);
    assert_eq!(
        made.callback_url,
        format!(
            "{}{BASE_PATH}/auth/oidc/corp/callback",
            world.app.app.config.system.public_url.trim_end_matches('/')
        ),
        "the address the provider is told to call back"
    );
    assert!(
        !raw.contains("s3cret") && !listed_raw.contains("s3cret"),
        "the secret is write-only"
    );
    assert_eq!(kept_secret(&world, made.id).await.unwrap(), b"s3cret-value");
    assert_eq!(listed.ok(), std::slice::from_ref(&made));
    let lines = lines_of(&world, "identity_provider.created").await;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].actor_id, Some(c.root.id()));
    assert_eq!(lines[0].object_id, Some(made.id));
    assert!(
        !lines[0].details.to_string().contains("s3cret"),
        "nor does the trail know it"
    );
}

#[tokio::test]
async fn a_provider_without_a_secret_says_so_and_one_can_be_given_later() {
    let world = World::new().await;
    let c = cast(&world).await;

    let made = add_provider(
        &world,
        &c.root,
        &IdentityProviderInput {
            client_secret: None,
            ..corp()
        },
    )
    .await
    .ok();
    let given = change_provider(
        &world,
        &c.root,
        made.id,
        &IdentityProviderUpdateBody {
            client_secret: Some("s3cret-later".into()),
            ..nothing_changed(made.revision)
        },
    )
    .await
    .ok();

    assert!(!made.client_secret_set);
    assert!(given.client_secret_set);
    assert_eq!(kept_secret(&world, made.id).await.unwrap(), b"s3cret-later");
}

#[tokio::test]
async fn a_key_taken_a_key_that_cannot_appear_in_an_address_and_an_issuer_that_is_no_https_address_are_refused()
 {
    let world = World::new().await;
    let c = cast(&world).await;
    add_provider(&world, &c.root, &corp()).await.ok();
    let refused = |input: IdentityProviderInput| {
        let (world, root) = (&world, &c.root);
        async move { add_provider(world, root, &input).await.problem() }
    };

    let taken = refused(IdentityProviderInput {
        name: "Again".into(),
        ..corp()
    })
    .await;
    let capital = refused(IdentityProviderInput {
        key: "Corp".into(),
        ..corp()
    })
    .await;
    let short = refused(IdentityProviderInput {
        key: "c".into(),
        ..corp()
    })
    .await;
    let plain = refused(IdentityProviderInput {
        key: "plain".into(),
        issuer: "http://id.example.com".into(),
        ..corp()
    })
    .await;
    let query = refused(IdentityProviderInput {
        key: "query".into(),
        issuer: "https://id.example.com/?a=1".into(),
        ..corp()
    })
    .await;
    let no_name = refused(IdentityProviderInput {
        key: "nameless".into(),
        name: " ".into(),
        ..corp()
    })
    .await;
    let no_openid = refused(IdentityProviderInput {
        key: "scoped".into(),
        scopes: Some(vec!["email".into()]),
        ..corp()
    })
    .await;
    let domain = refused(IdentityProviderInput {
        key: "domain".into(),
        allowed_email_domains: Some(vec!["not a domain".into()]),
        ..corp()
    })
    .await;

    assert_eq!(taken.code, ErrorCode::ConflictExists);
    for (problem, path) in [
        (&capital, "key"),
        (&short, "key"),
        (&plain, "issuer"),
        (&query, "issuer"),
        (&no_name, "name"),
        (&no_openid, "scopes"),
        (&domain, "allowed_email_domains[0]"),
    ] {
        assert_eq!(
            problem.code,
            ErrorCode::RequestInvalid,
            "{path}: {problem:?}"
        );
        assert_eq!(
            problem.extra["errors"][0]["path"],
            json!(path),
            "{problem:?}"
        );
    }
    assert_eq!(providers(&world, &c.root).await.len(), 1);
    assert_eq!(lines_of(&world, "identity_provider.created").await.len(), 1);
}

#[tokio::test]
async fn the_org_a_new_person_joins_is_one_that_takes_members_and_a_role_needs_it() {
    let world = World::new().await;
    let c = cast(&world).await;
    let refused = |input: IdentityProviderInput| {
        let (world, root) = (&world, &c.root);
        async move { add_provider(world, root, &input).await.problem() }
    };

    let role_alone = refused(IdentityProviderInput {
        default_role: Some(Role::Viewer),
        ..corp()
    })
    .await;
    let nowhere = refused(IdentityProviderInput {
        default_org_id: Some(new_id()),
        ..corp()
    })
    .await;
    world
        .app
        .call(AdminOrgUpdate, &[&c.acme])
        .token(&c.root.token)
        .body(&OrgLimitsChange {
            closed: Some(true),
            flat: None,
        })
        .send()
        .await
        .ok();
    let closed = refused(IdentityProviderInput {
        default_org_id: Some(c.acme),
        ..corp()
    })
    .await;
    world
        .app
        .call(AdminOrgUpdate, &[&c.acme])
        .token(&c.root.token)
        .body(&OrgLimitsChange {
            closed: Some(false),
            flat: None,
        })
        .send()
        .await
        .ok();
    let open = add_provider(
        &world,
        &c.root,
        &IdentityProviderInput {
            default_org_id: Some(c.acme),
            default_role: Some(Role::Developer),
            auto_register: Some(true),
            ..corp()
        },
    )
    .await
    .ok();

    assert_eq!(role_alone.extra["errors"][0]["path"], json!("default_role"));
    assert_eq!(nowhere.extra["errors"][0]["path"], json!("default_org_id"));
    assert_eq!(closed.code, ErrorCode::OrgClosed);
    assert_eq!(open.default_org_id, Some(c.acme));
    assert_eq!(open.default_role, Some(Role::Developer));
    assert!(open.auto_register);
}

#[tokio::test]
async fn a_provider_is_changed_by_the_revision_that_was_seen_and_what_is_left_out_stays() {
    let world = World::new().await;
    let c = cast(&world).await;
    let made = add_provider(&world, &c.root, &corp()).await.ok();

    let changed = change_provider(
        &world,
        &c.root,
        made.id,
        &IdentityProviderUpdateBody {
            name: Some("Company".into()),
            enabled: Some(false),
            allowed_email_domains: Some(vec!["Example.COM".into()]),
            default_org_id: Some(Some(c.acme)),
            default_role: Some(Some(Role::Viewer)),
            ..nothing_changed(made.revision)
        },
    )
    .await
    .ok();
    let stale = change_provider(
        &world,
        &c.root,
        made.id,
        &IdentityProviderUpdateBody {
            name: Some("Late".into()),
            ..nothing_changed(made.revision)
        },
    )
    .await
    .problem();
    let cleared = change_provider(
        &world,
        &c.root,
        made.id,
        &IdentityProviderUpdateBody {
            default_org_id: Some(None),
            default_role: Some(None),
            ..nothing_changed(changed.revision)
        },
    )
    .await
    .ok();
    let unchanged = change_provider(&world, &c.root, made.id, &nothing_changed(cleared.revision))
        .await
        .ok();

    assert_eq!(changed.revision, 2);
    assert_eq!((changed.name.as_str(), changed.enabled), ("Company", false));
    assert_eq!(
        changed.allowed_email_domains,
        ["example.com"],
        "a domain is kept in lower case"
    );
    assert_eq!(changed.default_org_id, Some(c.acme));
    assert_eq!(changed.default_role, Some(Role::Viewer));
    assert_eq!(
        (
            changed.key.as_str(),
            changed.issuer.as_str(),
            changed.client_id.as_str()
        ),
        ("corp", "https://id.example.com", "orch"),
        "what was left out stays"
    );
    assert!(changed.client_secret_set, "so does the secret");
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["revision"], json!(2));
    assert_eq!(stale.extra["current"]["name"], json!("Company"));
    assert!(
        cleared.default_org_id.is_none() && cleared.default_role.is_none(),
        "null takes them away"
    );
    assert_eq!(cleared.revision, 3);
    assert_eq!(unchanged.revision, 3, "saying nothing changes nothing");
    let lines = lines_of(&world, "identity_provider.updated").await;
    assert_eq!(
        lines.len(),
        2,
        "the change that changed nothing leaves no line"
    );
    assert!(
        lines[0].details["changed"]
            .as_array()
            .unwrap()
            .contains(&json!("enabled"))
    );
}

#[tokio::test]
async fn a_key_may_be_changed_but_not_to_one_that_another_provider_has() {
    let world = World::new().await;
    let c = cast(&world).await;
    let first = add_provider(&world, &c.root, &corp()).await.ok();
    let second = add_provider(
        &world,
        &c.root,
        &IdentityProviderInput {
            key: "other".into(),
            ..corp()
        },
    )
    .await
    .ok();

    let taken = change_provider(
        &world,
        &c.root,
        second.id,
        &IdentityProviderUpdateBody {
            key: Some("corp".into()),
            ..nothing_changed(second.revision)
        },
    )
    .await
    .problem();
    let renamed = change_provider(
        &world,
        &c.root,
        first.id,
        &IdentityProviderUpdateBody {
            key: Some("company".into()),
            ..nothing_changed(first.revision)
        },
    )
    .await
    .ok();

    assert_eq!(taken.code, ErrorCode::ConflictExists);
    assert_eq!(renamed.key, "company");
    assert!(
        renamed
            .callback_url
            .ends_with("/auth/oidc/company/callback")
    );
}

#[tokio::test]
async fn a_provider_that_is_the_only_way_somebody_signs_in_is_not_removed() {
    let world = World::new().await;
    let c = cast(&world).await;
    let corp_provider = add_provider(&world, &c.root, &corp()).await.ok();
    let other = add_provider(
        &world,
        &c.root,
        &IdentityProviderInput {
            key: "other".into(),
            ..corp()
        },
    )
    .await
    .ok();
    let dan = world
        .app
        .call(AdminUserCreate, &[])
        .token(&c.root.token)
        .body(&AdminUserCreateBody {
            username: "dan".into(),
            display_name: "Dan".into(),
            email: None,
            is_system_admin: None,
            personal_org: None,
        })
        .send()
        .await
        .ok()
        .user;
    sqlx::query(
        "INSERT INTO user_identities (identity_provider_id, subject, user_id) VALUES ($1, 'dan-1', $2)",
    )
    .bind(corp_provider.id)
    .bind(dan.base.id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let delete = |id: Id| {
        let (world, root) = (&world, &c.root);
        async move {
            world
                .app
                .call(AdminIdentityProviderDelete, &[&id])
                .token(&root.token)
                .send()
                .await
        }
    };

    let shut_out = delete(corp_provider.id).await.problem();
    let elsewhere: u64 = {
        sqlx::query(
            "INSERT INTO user_identities (identity_provider_id, subject, user_id) VALUES ($1, 'dan-2', $2)",
        )
        .bind(other.id)
        .bind(dan.base.id)
        .execute(&world.db.pool)
        .await
        .unwrap()
        .rows_affected()
    };
    let removed = delete(corp_provider.id).await;

    assert_eq!(shut_out.code, ErrorCode::ResourceInUse);
    assert_eq!(shut_out.extra["users"], json!(["dan"]));
    assert_eq!(shut_out.extra["more"], json!(0));
    assert_eq!(elsewhere, 1);
    assert_eq!(removed.status, 204, "he can sign in through the other one");
    let left = providers(&world, &c.root).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].key, "other");
    assert!(
        kept_secret(&world, corp_provider.id).await.is_none(),
        "its secret goes with it"
    );
    assert!(kept_secret(&world, other.id).await.is_some());
    assert_eq!(
        delete(corp_provider.id).await.problem().code,
        ErrorCode::NotFound
    );
    let lines = lines_of(&world, "identity_provider.deleted").await;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].object_id, Some(corp_provider.id));
}

#[tokio::test]
async fn a_provider_nobody_depends_on_is_removed_when_the_person_has_a_password() {
    let world = World::new().await;
    let c = cast(&world).await;
    let made = add_provider(&world, &c.root, &corp()).await.ok();
    sqlx::query(
        "INSERT INTO user_identities (identity_provider_id, subject, user_id) VALUES ($1, 'ada-1', $2)",
    )
    .bind(made.id)
    .bind(c.ada.id())
    .execute(&world.db.pool)
    .await
    .unwrap();

    let removed = world
        .app
        .call(AdminIdentityProviderDelete, &[&made.id])
        .token(&c.root.token)
        .send()
        .await;

    assert_eq!(removed.status, 204, "ada has a password of her own");
    assert!(providers(&world, &c.root).await.is_empty());
}

// ---- who may ----

#[tokio::test]
async fn somebody_who_is_no_system_admin_is_refused_the_catalogue_the_builds_the_profiles_and_the_providers_and_so_is_an_orgs_admin()
 {
    let world = World::new().await;
    let c: Cast = cast(&world).await;
    let capability = serde_json::to_value(gpu()).unwrap();
    let harness = serde_json::to_value(build("1.0.0", false)).unwrap();
    let agent = serde_json::to_value(profile("Shared")).unwrap();
    let provider = serde_json::to_value(corp()).unwrap();
    let change = json!({ "name": "Other", "expected_revision": 1 });
    let id = new_id();

    for (who, name) in [(&c.boss, "an org's admin"), (&c.ada, "an ordinary person")] {
        let problems = [
            refused_to(&world, AdminCapabilityList, &[], who, None, "").await,
            refused_to(
                &world,
                AdminCapabilitySet,
                &[&"gpu"],
                who,
                Some(capability.clone()),
                "",
            )
            .await,
            refused_to(&world, AdminCapabilityDelete, &[&"gpu"], who, None, "").await,
            refused_to(&world, AdminHarnessList, &[], who, None, "").await,
            refused_to(
                &world,
                AdminHarnessSet,
                &[&"codex", &"1.0.0"],
                who,
                Some(harness.clone()),
                "",
            )
            .await,
            refused_to(&world, AdminAgentCreate, &[], who, Some(agent.clone()), "").await,
            refused_to(&world, AdminIdentityProviderList, &[], who, None, "").await,
            refused_to(
                &world,
                AdminIdentityProviderCreate,
                &[],
                who,
                Some(provider.clone()),
                "",
            )
            .await,
            refused_to(
                &world,
                AdminIdentityProviderUpdate,
                &[&id],
                who,
                Some(change.clone()),
                "",
            )
            .await,
            refused_to(&world, AdminIdentityProviderDelete, &[&id], who, None, "").await,
        ];
        for problem in &problems {
            assert_no_system_admin(problem, name);
        }
    }
    assert!(capabilities(&world, &c.root).await.is_empty());
    assert!(builds(&world, &c.root).await.is_empty());
    assert!(providers(&world, &c.root).await.is_empty());
}
