//! Resource lists use Everything's access filter, bounded rich views and keyset order through the real route.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_api_contract::{
    common::{ErrorCode, Page},
    everything::{
        EverythingDirection, EverythingList, EverythingListQuery, EverythingResource,
        EverythingRow, EverythingSort,
    },
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures;
use orch_types::{Id, Time};
use places::{Who, admin, join, make_org, person};
use support::World;

async fn resource(world: &World, owner: &Who, home: Id, kind: &str, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = access_fixtures::insert_resource(&mut conn, kind, Some(owner.id()), Some(home), name)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: kind.parse::<ResourceKind>().unwrap().into(),
            id,
        },
        owner.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    if kind == "mcp_server" {
        sqlx::query("UPDATE mcp_servers SET spec=$2 WHERE id=$1")
            .bind(id)
            .bind(serde_json::json!({"transport":"stdio","command":["test-mcp"]}))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    if kind == "provider" {
        sqlx::query("UPDATE providers SET upstreams='[{\"protocol\":\"openai-responses\",\"base_url\":\"http://api.test\",\"options\":{}}]'::jsonb WHERE id=$1")
            .bind(id).execute(&mut *conn).await.unwrap();
    }
    id
}

fn query(kind: &str) -> EverythingListQuery {
    EverythingListQuery {
        parent: Some("resources".to_owned()),
        kinds: Some(kind.to_owned()),
        resource_details: Some(true),
        limit: Some(1),
        ..Default::default()
    }
}

async fn read(world: &World, by: &Who, query: &EverythingListQuery) -> Page<EverythingRow> {
    eprintln!(
        "resource inventory kind={:?} view={:?}",
        query.kinds, query.resource_view
    );
    world
        .app
        .call(EverythingList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn the_landing_counts_each_authorized_kind_without_loading_rich_pages_or_disclosing_private_resources()
 {
    use orch_api_contract::everything::{ResourceCounts, ResourcesSummaryGet};
    let world = World::new().await;
    let owner = person(&world, "summary-owner").await;
    let outsider = person(&world, "summary-outsider").await;
    let root = make_org(&world, &owner, "Summary home", "summary-home")
        .await
        .base
        .id;
    let before: ResourceCounts = world
        .app
        .call(ResourcesSummaryGet, &[])
        .token(&owner.token)
        .send()
        .await
        .ok();
    let outside_before: ResourceCounts = world
        .app
        .call(ResourcesSummaryGet, &[])
        .token(&outsider.token)
        .send()
        .await
        .ok();
    for kind in [
        "agent",
        "provider",
        "environment",
        "host",
        "machine",
        "template",
        "mcp_server",
        "mount",
    ] {
        // A mount's and an MCP server's name is how agents and paths name it: lower case, no spaces.
        resource(&world, &owner, root, kind, &format!("summary-{kind}")).await;
    }
    let own: ResourceCounts = world
        .app
        .call(ResourcesSummaryGet, &[])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert_eq!(own.agents, before.agents + 1);
    assert_eq!(own.providers, before.providers + 1);
    assert_eq!(own.environments, before.environments + 1);
    assert_eq!(own.hosts, before.hosts + 1);
    // A host's physical machine is also an entry in Machines, rather than a runtime instance.
    assert_eq!(own.machines, before.machines + 2);
    assert_eq!(own.templates, before.templates + 1);
    assert_eq!(own.mcp, before.mcp + 1);
    assert_eq!(own.mounts, before.mounts + 1);
    let hidden: ResourceCounts = world
        .app
        .call(ResourcesSummaryGet, &[])
        .token(&outsider.token)
        .send()
        .await
        .ok();
    assert_eq!(
        hidden, outside_before,
        "none of the private resources changes another person's counts"
    );
    let native = admin(&world, "summary-native").await;
    let elevated: ResourceCounts = world
        .app
        .call(ResourcesSummaryGet, &[])
        .token(&native.token)
        .send()
        .await
        .ok();
    world
        .app
        .call(orch_api_contract::auth::MeAdminModeSet, &[])
        .token(&native.token)
        .body(&orch_api_contract::auth::AdminModeSetBody { as_admin: false })
        .send()
        .await
        .ok();
    let off: ResourceCounts = world
        .app
        .call(ResourcesSummaryGet, &[])
        .token(&native.token)
        .send()
        .await
        .ok();
    assert!(
        off.providers < elevated.providers,
        "ending native reach also cuts aggregate counts"
    );
    assert!(off.agents < elevated.agents);
}

#[tokio::test]
async fn every_resource_kind_keeps_its_existing_readable_view_and_owner_inside_the_bounded_page() {
    let world = World::new().await;
    let owner = person(&world, "inventory-owner").await;
    let root = make_org(&world, &owner, "Inventory", "inventory")
        .await
        .base
        .id;
    for kind in [
        "agent",
        "provider",
        "environment",
        "host",
        "machine",
        "template",
        "mcp_server",
        "mount",
    ] {
        let name = format!("inventory-{}", kind.replace('_', "-"));
        let id = resource(&world, &owner, root, kind, &name).await;
        let ask = EverythingListQuery {
            q: Some(name.clone()),
            ..query(kind)
        };
        let page = read(&world, &owner, &ask).await;
        assert_eq!(page.items.len(), 1, "{kind}");
        let item = &page.items[0];
        assert_eq!(item.id, id.to_string());
        assert!(
            item.admin_access.is_none(),
            "ordinary ownership stays ordinary"
        );
        assert_eq!(item.owner.as_ref().unwrap().id, owner.id());
        let wire = serde_json::to_value(item.resource.as_ref().unwrap()).unwrap();
        assert_eq!(wire["kind"], kind);
        assert_eq!(wire["value"]["name"], name);
        assert_eq!(wire["value"]["owner"]["id"], owner.id().to_string());
        let ordinary = read(
            &world,
            &owner,
            &EverythingListQuery {
                resource_details: None,
                ..ask
            },
        )
        .await;
        assert!(
            ordinary.items.iter().all(|row| row.resource.is_none()),
            "the hierarchy does not hydrate resource screens unless asked"
        );
    }
}

#[tokio::test]
async fn resource_state_harness_home_and_name_are_filtered_before_the_page_and_the_cursor_keeps_the_view()
 {
    let world = World::new().await;
    let owner = person(&world, "page-owner").await;
    let root = make_org(&world, &owner, "Page home", "page-home")
        .await
        .base
        .id;
    // More than the normal fifty rows precede the sole disabled match.
    for number in 0..51 {
        resource(&world, &owner, root, "agent", &format!("Alpha {number:03}")).await;
    }
    let needle = resource(&world, &owner, root, "agent", "Zulu target").await;
    sqlx::query("UPDATE agents SET disabled=true WHERE id=$1")
        .bind(needle)
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    let ask = EverythingListQuery {
        resource_state: Some("disabled".to_owned()),
        harness: Some("fake".to_owned()),
        home: Some(root),
        ..query("agent")
    };
    let page = read(&world, &owner, &ask).await;
    assert_eq!(page.items[0].id, needle.to_string());
    assert!(
        matches!(&page.items[0].resource, Some(EverythingResource::Agent(agent)) if agent.base.disabled && agent.base.harness_kind == "fake")
    );
    assert!(page.next_cursor.is_none());
    let first = read(
        &world,
        &owner,
        &EverythingListQuery {
            q: Some("Page home".to_owned()),
            ..query("agent")
        },
    )
    .await;
    assert_eq!(
        first.items.len(),
        1,
        "the resource search also uses its visible home label"
    );
    let cursor = first.next_cursor.unwrap();
    let next = read(
        &world,
        &owner,
        &EverythingListQuery {
            q: Some("Page home".to_owned()),
            cursor: Some(cursor.clone()),
            ..query("agent")
        },
    )
    .await;
    assert_ne!(first.items[0].id, next.items[0].id);
    let refused = world
        .app
        .call(EverythingList, &[])
        .token(&owner.token)
        .query(&EverythingListQuery {
            cursor: Some(cursor),
            resource_state: Some("disabled".to_owned()),
            ..query("agent")
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_private_resource_never_gets_a_rich_view_without_view_and_native_reach_keeps_the_same_marker()
 {
    let world = World::new().await;
    let owner = person(&world, "private-owner").await;
    let root = make_org(&world, &owner, "Private home", "private-home")
        .await
        .base
        .id;
    let id = resource(&world, &owner, root, "agent", "Private profile").await;
    let outsider = person(&world, "outsider").await;
    let ask = EverythingListQuery {
        q: Some("Private profile".to_owned()),
        ..query("agent")
    };
    assert!(read(&world, &outsider, &ask).await.items.is_empty());
    {
        let mut conn = world.conn().await;
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("agent", id, "live"),
            "any",
            "access.manage",
            ("user", Some(outsider.id()), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
    }
    let name_only = EverythingListQuery {
        resource_details: None,
        ..ask.clone()
    };
    let named = read(&world, &outsider, &name_only).await;
    assert!(
        named
            .items
            .iter()
            .any(|row| row.id == id.to_string() && row.resource.is_none())
    );
    assert!(
        read(&world, &outsider, &ask).await.items.is_empty(),
        "naming a permission page never authorizes its resource view"
    );
    assert!(
        read(
            &world,
            &outsider,
            &EverythingListQuery {
                resource_state: Some("enabled".to_owned()),
                ..name_only
            }
        )
        .await
        .items
        .is_empty(),
        "a state predicate cannot reveal private metadata"
    );
    let native = admin(&world, "native").await;
    let page = read(&world, &native, &ask).await;
    let item = page
        .items
        .iter()
        .find(|row| row.id == id.to_string())
        .unwrap();
    let Some(EverythingResource::Agent(view)) = &item.resource else {
        panic!("a readable agent needs its ordinary agent view");
    };
    assert!(item.admin_access.is_some());
    assert_eq!(item.admin_access, view.access.admin_access);
    assert_eq!(view.owner.as_ref().unwrap().id, owner.id());
    world
        .app
        .call(orch_api_contract::auth::MeAdminModeSet, &[])
        .token(&native.token)
        .body(&orch_api_contract::auth::AdminModeSetBody { as_admin: false })
        .send()
        .await
        .ok();
    assert!(read(&world, &native, &ask).await.items.is_empty());
}

#[tokio::test]
async fn resource_kind_filters_templates_and_directory_origins_before_hydration() {
    let world = World::new().await;
    let owner = person(&world, "kind-owner").await;
    let root = make_org(&world, &owner, "Kinds", "kinds").await.base.id;
    resource(&world, &owner, root, "template", "Alpha environment recipe").await;
    let provider = resource(&world, &owner, root, "template", "Zulu provider recipe").await;
    sqlx::query("UPDATE env_templates SET kind='provider' WHERE id=$1")
        .bind(provider)
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    let templates = read(
        &world,
        &owner,
        &EverythingListQuery {
            resource_kind: Some("provider".to_owned()),
            ..query("template")
        },
    )
    .await;
    assert_eq!(templates.items[0].id, provider.to_string());
    resource(
        &world,
        &owner,
        root,
        "environment",
        "Alpha template environment",
    )
    .await;
    let directory = {
        let mut conn = world.conn().await;
        let id = access_fixtures::insert_environment(
            &mut conn,
            owner.id(),
            Some(root),
            "Zulu directory",
            "connected",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Environment.into(),
                id,
            },
            owner.id(),
            NewAccess::OnlyMe,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        id
    };
    let environments = read(
        &world,
        &owner,
        &EverythingListQuery {
            resource_kind: Some("machine".to_owned()),
            ..query("environment")
        },
    )
    .await;
    assert_eq!(environments.items[0].id, directory.to_string());
    assert!(environments.next_cursor.is_none());
}

#[tokio::test]
async fn provider_views_use_the_existing_ownership_use_and_manage_policy_before_paging() {
    use orch_api_contract::resources::ProviderListScope;
    let world = World::new().await;
    let owner = person(&world, "provider-owner").await;
    let other = person(&world, "provider-other").await;
    let root = make_org(&world, &owner, "Provider home", "provider-home")
        .await
        .base
        .id;
    join(&world, root, root, &other, "viewer", &owner).await;
    let mine = resource(&world, &owner, root, "provider", "Zulu owner").await;
    let shared = resource(&world, &other, root, "provider", "Beta shared").await;
    // Levels nest inside an area: Use gives View. A provider the owner may see and not use is
    // what the usable view leaves out before it pages.
    let hidden = resource(&world, &other, root, "provider", "Alpha hidden").await;
    {
        let mut conn = world.conn().await;
        for (id, permission) in [
            (shared, "provider.view"),
            (shared, "provider.use"),
            (hidden, "provider.view"),
        ] {
            orch_db::queries::access::testing::add_entry(
                &mut conn,
                &orch_db::queries::access::PageKey::new("provider", id, "live"),
                "any",
                permission,
                ("user", Some(owner.id()), None),
                None,
                world.now(),
            )
            .await
            .unwrap();
        }
    }
    for scope in [ProviderListScope::Mine, ProviderListScope::Managed] {
        let page = read(
            &world,
            &owner,
            &EverythingListQuery {
                resource_view: Some(scope),
                ..query("provider")
            },
        )
        .await;
        assert_eq!(page.items[0].id, mine.to_string());
        assert!(page.next_cursor.is_none());
    }
    let ask = EverythingListQuery {
        resource_view: Some(ProviderListScope::Usable),
        ..query("provider")
    };
    let first = read(&world, &owner, &ask).await;
    assert_eq!(first.items[0].id, shared.to_string());
    let second = read(
        &world,
        &owner,
        &EverythingListQuery {
            cursor: first.next_cursor,
            ..ask
        },
    )
    .await;
    assert_eq!(second.items[0].id, mine.to_string());
    assert!(
        second.next_cursor.is_none(),
        "View without Use takes no slot of the usable view"
    );
}

#[tokio::test]
async fn every_resource_view_nests_owned_managed_use_and_view_rows_before_pagination() {
    let _diagnostics = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_test_writer()
            .finish(),
    );
    use orch_api_contract::resources::ProviderListScope;
    let world = World::new().await;
    let reader = person(&world, "library-reader").await;
    let giver = person(&world, "library-giver").await;
    let root = make_org(&world, &reader, "Library", "library")
        .await
        .base
        .id;
    join(&world, root, root, &giver, "viewer", &reader).await;
    for kind in [
        "provider",
        "agent",
        "host",
        "template",
        "mcp_server",
        "mount",
        "skill",
        "environment",
    ] {
        let own = resource(&world, &reader, root, kind, &format!("own-{kind}")).await;
        let managed = resource(&world, &giver, root, kind, &format!("managed-{kind}")).await;
        let view = resource(&world, &giver, root, kind, &format!("view-{kind}")).await;
        let prefix = if kind == "mcp_server" { "mcp" } else { kind };
        let use_row = if kind == "environment" {
            None
        } else {
            Some(resource(&world, &giver, root, kind, &format!("use-{kind}")).await)
        };
        {
            let mut conn = world.conn().await;
            for (id, level) in [(managed, "manage"), (view, "view")]
                .into_iter()
                .chain(use_row.map(|id| (id, "use")))
            {
                orch_db::queries::access::testing::add_entry(
                    &mut conn,
                    &orch_db::queries::access::PageKey::new(kind, id, "live"),
                    "any",
                    &format!("{prefix}.{level}"),
                    ("user", Some(reader.id()), None),
                    None,
                    world.now(),
                )
                .await
                .unwrap();
            }
        }
        for (scope, expected) in [
            (ProviderListScope::Mine, vec![own]),
            (ProviderListScope::Managed, vec![own, managed]),
            (
                ProviderListScope::Viewable,
                [vec![own, managed, view], use_row.into_iter().collect()].concat(),
            ),
        ] {
            let page = read(
                &world,
                &reader,
                &EverythingListQuery {
                    resource_view: Some(scope),
                    limit: Some(50),
                    ..query(kind)
                },
            )
            .await;
            let got: std::collections::HashSet<_> = page.items.into_iter().map(|r| r.id).collect();
            assert_eq!(
                got,
                expected.into_iter().map(|id| id.to_string()).collect(),
                "{kind} {scope:?}"
            );
        }
        if let Some(used) = use_row {
            let page = read(
                &world,
                &reader,
                &EverythingListQuery {
                    resource_view: Some(ProviderListScope::Usable),
                    limit: Some(50),
                    ..query(kind)
                },
            )
            .await;
            assert_eq!(page.items.len(), 3, "{kind}: mine, managed and use");
            assert!(page.items.iter().any(|r| r.id == used.to_string()));
        }
    }
}

#[tokio::test]
async fn a_skill_library_snapshot_redacts_legacy_sources_without_writing_and_ordinary_reads_still_seal_them()
 {
    use orch_api_contract::skills::{SkillGet, SkillOrigin};
    let world = World::new().await;
    let owner = person(&world, "legacy-library-reader").await;
    let root = make_org(
        &world,
        &owner,
        "Legacy source library",
        "legacy-source-library",
    )
    .await
    .base
    .id;
    let skill = resource(&world, &owner, root, "skill", "legacy-source").await;
    let origin = serde_json::to_value(SkillOrigin::Url {
        url: "https://example.test/skill.zip?private=inventory-redaction-test".to_owned(),
        folder: None,
        host_id: orch_types::new_id(),
    })
    .unwrap();
    {
        let mut conn = world.conn().await;
        sqlx::query("UPDATE skills SET origin=$2 WHERE id=$1")
            .bind(skill)
            .bind(&origin)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let page = read(&world, &owner, &query("skill")).await;
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].resource.is_some());
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains("inventory-redaction-test")
    );
    {
        let mut conn = world.conn().await;
        let stored: (serde_json::Value, Option<Id>) =
            sqlx::query_as("SELECT origin,source_url_secret_id FROM skills WHERE id=$1")
                .bind(skill)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(
            stored.0, origin,
            "the read-only inventory never rewrites a source"
        );
        assert_eq!(stored.1, None);
    }
    let shown = world
        .app
        .call(SkillGet, &[&skill])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert!(
        !serde_json::to_string(&shown)
            .unwrap()
            .contains("inventory-redaction-test")
    );
    let mut conn = world.conn().await;
    let sealed: bool =
        sqlx::query_scalar("SELECT source_url_secret_id IS NOT NULL FROM skills WHERE id=$1")
            .bind(skill)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(
        sealed,
        "the ordinary skill read retains legacy source sealing"
    );
}

#[tokio::test]
async fn an_all_resources_view_requires_an_explicit_administered_home() {
    use orch_api_contract::resources::ProviderListScope;
    let world = World::new().await;
    let owner = person(&world, "library-admin").await;
    let viewer = person(&world, "library-viewer").await;
    let root = make_org(
        &world,
        &owner,
        "Administered library",
        "administered-library",
    )
    .await
    .base
    .id;
    join(&world, root, root, &viewer, "viewer", &owner).await;
    let template = resource(&world, &owner, root, "template", "private-template").await;
    let missing = EverythingListQuery {
        resource_view: Some(ProviderListScope::All),
        ..query("template")
    };
    let no_home = world
        .app
        .call(EverythingList, &[])
        .token(&owner.token)
        .query(&missing)
        .send()
        .await
        .problem();
    assert_eq!(no_home.code, ErrorCode::RequestInvalid);
    let scoped = EverythingListQuery {
        home: Some(root),
        ..missing
    };
    let denied = world
        .app
        .call(EverythingList, &[])
        .token(&viewer.token)
        .query(&scoped)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::NotFound);
    let inactive = world
        .app
        .call(EverythingList, &[])
        .token(&owner.token)
        .query(&scoped)
        .send()
        .await
        .problem();
    assert_eq!(
        inactive.code,
        ErrorCode::NotFound,
        "All remains an explicit administrative act"
    );
    places::admin_mode(&world, &owner, true).await;
    let allowed = read(&world, &owner, &scoped).await;
    assert_eq!(allowed.items[0].id, template.to_string());
}

#[tokio::test]
async fn recorded_use_order_is_numeric_and_applies_before_the_cursor_and_unknown_mount_use_is_absent()
 {
    use orch_api_contract::resource_facts::ResourceUseKind;
    use orch_engine::resource_facts::library::{Report, record};
    let world = World::new().await;
    let reader = person(&world, "library-order").await;
    let root = make_org(&world, &reader, "Uses", "uses").await.base.id;
    let small = resource(&world, &reader, root, "template", "alpha").await;
    let popular = resource(&world, &reader, root, "template", "zulu").await;
    {
        let mut conn = world.conn().await;
        for (id, count) in [(small, 2), (popular, 12)] {
            for call in 0..count {
                record(
                    &mut conn,
                    &Report {
                        turn: None,
                        object: ObjectRef {
                            kind: orch_access_contract::permissions::ObjectKind::Template,
                            id,
                        },
                        version: None,
                        kind: ResourceUseKind::EnvironmentMade,
                        source: &format!("native-{call}"),
                        person: Some(reader.id()),
                        at: Time::new(world.now()),
                    },
                )
                .await
                .unwrap();
            }
        }
    }
    let ask = EverythingListQuery {
        sort: Some(EverythingSort::Use),
        direction: Some(EverythingDirection::Desc),
        ..query("template")
    };
    let first = read(&world, &reader, &ask).await;
    assert_eq!(first.items[0].id, popular.to_string());
    assert_eq!(
        first.items[0]
            .library
            .as_ref()
            .unwrap()
            .uses
            .as_ref()
            .unwrap()
            .all_time,
        Some(12)
    );
    let second = read(
        &world,
        &reader,
        &EverythingListQuery {
            cursor: first.next_cursor,
            ..ask
        },
    )
    .await;
    assert_eq!(second.items[0].id, small.to_string());
    let mount = resource(&world, &reader, root, "mount", "data").await;
    let mounts = read(&world, &reader, &query("mount")).await;
    assert_eq!(mounts.items[0].id, mount.to_string());
    assert!(mounts.items[0].library.as_ref().unwrap().uses.is_none());
}

#[tokio::test]
async fn library_names_remain_private_and_the_same_endpoint_refuses_a_resource_without_view() {
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::library::{LibraryReadBody, LibraryTarget, ResourceLibraryRead};
    let world = World::new().await;
    let reader = person(&world, "library-private-reader").await;
    let giver = person(&world, "library-private-giver").await;
    let home = make_org(&world, &reader, "Visible library", "visible-library")
        .await
        .base
        .id;
    let secret = make_org(&world, &giver, "Private library", "private-library")
        .await
        .base
        .id;
    let provider = resource(&world, &reader, home, "provider", "Visible provider").await;
    let project = places::make_project(&world, &giver, secret, "Private project").await;
    let topic = places::make_topic(&world, &giver, project.base.id, "Private topic").await;
    let private_thread = orch_types::new_id();
    {
        let mut conn = world.conn().await;
        sqlx::query("INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by) SELECT $1,'thread',id,org_id,path||$1,'Never expose this title',$3,$3 FROM nodes WHERE id=$2").bind(private_thread).bind(topic.base.id).bind(giver.id()).execute(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO threads(node_id,topic_id,provider_id) VALUES($1,$2,$3)")
            .bind(private_thread)
            .bind(topic.base.id)
            .bind(provider)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let target = LibraryTarget {
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
        version_id: None,
    };
    let reply = world
        .app
        .call(ResourceLibraryRead, &[])
        .token(&reader.token)
        .body(&LibraryReadBody {
            targets: vec![target.clone()],
        })
        .send()
        .await
        .ok();
    assert_eq!(reply[0].facts.dependents[0].count, 1);
    assert_eq!(reply[0].facts.dependents[0].hidden, 1);
    assert!(reply[0].facts.dependents[0].visible.is_empty());
    let encoded = serde_json::to_string(&reply).unwrap();
    assert!(!encoded.contains("Never expose this title"));
    assert!(!encoded.contains(&private_thread.to_string()));
    let denied = world
        .app
        .call(ResourceLibraryRead, &[])
        .token(&giver.token)
        .body(&LibraryReadBody {
            targets: vec![target],
        })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn resource_figures_refuse_an_unbounded_batch_and_accept_one_page() {
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::library::{LibraryReadBody, LibraryTarget, ResourceLibraryRead};
    let world = World::new().await;
    let reader = person(&world, "library-batch-reader").await;
    let home = make_org(&world, &reader, "Batch library", "batch-library")
        .await
        .base
        .id;
    let provider = resource(&world, &reader, home, "provider", "Paged provider").await;
    let target = LibraryTarget {
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
        version_id: None,
    };
    let maximum = usize::try_from(orch_api_contract::common::MAX_LIMIT).unwrap();
    let refused = world
        .app
        .call(ResourceLibraryRead, &[])
        .token(&reader.token)
        .body(&LibraryReadBody {
            targets: vec![target.clone(); maximum + 1],
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let accepted = world
        .app
        .call(ResourceLibraryRead, &[])
        .token(&reader.token)
        .body(&LibraryReadBody {
            targets: vec![target.clone(); maximum],
        })
        .send()
        .await
        .ok();
    assert_eq!(accepted.len(), maximum);
    assert!(accepted.iter().all(|entry| entry.target == target));
}

#[tokio::test]
async fn library_people_lists_only_readable_profiles_without_changing_the_aggregate() {
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::library::{LibraryPeopleBody, LibraryTarget, ResourceLibraryPeopleRead};
    let world = World::new().await;
    let reader = person(&world, "library-people-reader").await;
    let private = person(&world, "library-people-private").await;
    let home = make_org(&world, &reader, "People library", "people-library")
        .await
        .base
        .id;
    let template = resource(&world, &reader, home, "template", "team-template").await;
    {
        let mut conn = world.conn().await;
        sqlx::query("INSERT INTO user_profiles(user_id,audience) VALUES($1,'private') ON CONFLICT(user_id) DO UPDATE SET audience='private'").bind(private.id()).execute(&mut *conn).await.unwrap();
        for by in [reader.id(), private.id()] {
            orch_engine::resource_facts::library::created(
                &mut conn,
                template,
                None,
                orch_types::new_id(),
                false,
                Some(by),
                Time::new(world.now()),
            )
            .await
            .unwrap();
        }
        // Another execution unit on the same subject cannot duplicate the people behind this figure.
        orch_engine::resource_facts::library::created(
            &mut conn,
            template,
            None,
            orch_types::new_id(),
            true,
            Some(reader.id()),
            Time::new(world.now()),
        )
        .await
        .unwrap();
    }
    let people = world
        .app
        .call(ResourceLibraryPeopleRead, &[])
        .token(&reader.token)
        .body(&LibraryPeopleBody {
            target: LibraryTarget {
                object: ObjectRef {
                    kind: ObjectKind::Template,
                    id: template,
                },
                version_id: None,
            },
            cursor: None,
            recent: false,
        })
        .send()
        .await
        .ok();
    assert_eq!(people.items.len(), 1);
    assert_eq!(people.items[0].id, reader.id());
    assert_eq!(people.hidden, 1);
    assert!(
        !serde_json::to_string(&people)
            .unwrap()
            .contains(&private.id().to_string())
    );
    let page = read(&world, &reader, &query("template")).await;
    assert_eq!(
        page.items[0]
            .library
            .as_ref()
            .unwrap()
            .uses
            .as_ref()
            .unwrap()
            .people,
        Some(2)
    );
}

#[tokio::test]
async fn an_environments_own_tool_definition_uses_its_parent_view_for_library_facts() {
    let _diagnostics = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_test_writer()
            .finish(),
    );
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::library::{LibraryReadBody, LibraryTarget, ResourceLibraryRead};
    let world = World::new().await;
    let reader = person(&world, "own-tools-reader").await;
    let stranger = person(&world, "own-tools-stranger").await;
    let root = make_org(&world, &reader, "Own tools", "own-tools")
        .await
        .base
        .id;
    let environment = resource(&world, &reader, root, "environment", "Workspace").await;
    let preset = resource(&world, &reader, root, "mcp_server", "source-tools").await;
    let server = orch_types::new_id();
    {
        let mut conn = world.conn().await;
        sqlx::query("INSERT INTO mcp_servers(id,name,environment_id,spec) SELECT $1,'own-tools',$2,spec FROM mcp_servers WHERE id=$3").bind(server).bind(environment).bind(preset).execute(&mut *conn).await.unwrap();
    }
    let target = LibraryTarget {
        object: ObjectRef {
            kind: ObjectKind::McpServer,
            id: server,
        },
        version_id: None,
    };
    let facts = world
        .app
        .call(ResourceLibraryRead, &[])
        .token(&reader.token)
        .body(&LibraryReadBody {
            targets: vec![target.clone()],
        })
        .send()
        .await
        .ok();
    assert_eq!(
        facts[0].facts.uses.as_ref().unwrap().kind,
        orch_api_contract::resource_facts::ResourceUseKind::McpCall
    );
    let listed = world
        .app
        .call(
            orch_api_contract::environment::EnvironmentMcpGet,
            &[&environment],
        )
        .token(&reader.token)
        .send()
        .await
        .ok();
    assert!(
        listed
            .servers
            .iter()
            .any(|row| row.server.id == server && row.library.is_some())
    );
    let denied = world
        .app
        .call(ResourceLibraryRead, &[])
        .token(&stranger.token)
        .body(&LibraryReadBody {
            targets: vec![target],
        })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::NotFound);
}

async fn running_environment(world: &World, owner: &Who, home: Id, host: Id, memory: i64) {
    let environment = resource(world, owner, home, "environment", "Load unit").await;
    let runtime = orch_types::new_id();
    let mut conn = world.conn().await;
    sqlx::query("UPDATE environments SET config=jsonb_build_object('env.needs',jsonb_build_object('memory_mb',$2::bigint)) WHERE id=$1")
        .bind(environment).bind(memory).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,environment_id,unit_name,driver,template_id,spec_hash,state)
      SELECT $1,'environment',$2,e.id,$3,'test',e.template_id,decode(repeat('00',32),'hex'),'running' FROM environments e WHERE e.id=$4")
        .bind(runtime).bind(host).bind(format!("inventory-{runtime}")).bind(environment).execute(&mut *conn).await.unwrap();
}

async fn running_provider(world: &World, owner: &Who, home: Id, host: Id, memory: i64) {
    let provider = resource(world, owner, home, "provider", "Load provider").await;
    let template = resource(world, owner, home, "template", "Provider unit").await;
    let runtime = orch_types::new_id();
    let mut conn = world.conn().await;
    sqlx::query("UPDATE env_templates SET kind='provider' WHERE id=$1")
        .bind(template)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET runtime_config=jsonb_build_object('runtime.needs',jsonb_build_object('memory_mb',$2::bigint)) WHERE id=$1")
        .bind(provider).bind(memory).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,provider_id,unit_name,driver,template_id,spec_hash,state)
      VALUES($1,'provider',$2,$3,$4,'test',$5,decode(repeat('00',32),'hex'),'running')")
        .bind(runtime).bind(host).bind(provider).bind(format!("inventory-{runtime}")).bind(template).execute(&mut *conn).await.unwrap();
}

#[tokio::test]
async fn hosts_page_by_the_same_memory_load_as_their_meter_and_by_running_units_with_unknown_load_last()
 {
    let world = World::new().await;
    let owner = person(&world, "host-owner").await;
    let root = make_org(&world, &owner, "Hosts", "hosts").await.base.id;
    let loaded = resource(&world, &owner, root, "host", "Alpha loaded").await;
    let busy = resource(&world, &owner, root, "host", "Zulu busy").await;
    let unknown = resource(&world, &owner, root, "host", "Beta unknown").await;
    sqlx::query("UPDATE hosts SET capacity='{\"memory_mb\":1000}'::jsonb WHERE id=ANY($1)")
        .bind([loaded, busy].as_slice())
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    running_environment(&world, &owner, root, loaded, 800).await;
    for _ in 0..3 {
        running_provider(&world, &owner, root, busy, 100).await;
    }
    for (sort, expected) in [
        (EverythingSort::Load, [loaded, busy, unknown]),
        (EverythingSort::Running, [busy, loaded, unknown]),
    ] {
        let ask = EverythingListQuery {
            sort: Some(sort),
            direction: Some(EverythingDirection::Desc),
            ..query("host")
        };
        let mut cursor = None;
        let mut found = Vec::new();
        loop {
            let page = read(
                &world,
                &owner,
                &EverythingListQuery {
                    cursor,
                    ..ask.clone()
                },
            )
            .await;
            assert_eq!(page.items.len(), 1);
            if let Some(EverythingResource::Host(host)) = &page.items[0].resource {
                if host.base.id == busy {
                    assert_eq!(host.running.provider, 3);
                    assert_eq!(host.running.environment, 0);
                }
                if host.base.id == loaded {
                    assert_eq!(host.declared.memory_mb, 800);
                }
            }
            found.push(page.items[0].id.clone());
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(found, expected.map(|id| id.to_string()));
    }
}
