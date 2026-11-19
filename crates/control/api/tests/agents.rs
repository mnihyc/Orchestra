//! Agent profiles over HTTP: listing what the caller may use or manage, reading, making one in an org, changing, switching off and deleting.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    Agent, AgentDelete, AgentGet, AgentList, AgentListQuery, AgentUpdate, AgentUpdateBody,
    OrgAgentCreate, OrgAgentCreateBody,
};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::Id;
use places::{Who, make_department, missing_permissions};
use resources::{company, name_on_page, name_person};
use serde_json::json;
use support::{Statements, World};

async fn make(world: &World, by: &Who, org: Id, name: &str) -> Agent {
    world
        .app
        .call(OrgAgentCreate, &[&org])
        .token(&by.token)
        .body(&OrgAgentCreateBody {
            name: name.to_owned(),
            harness_kind: "codex".to_owned(),
            profile: json!({ "harness_version": "1.4", "config": { "model": "x" } }),
        })
        .send()
        .await
        .ok()
}

async fn list(world: &World, by: &Who) -> Vec<String> {
    world
        .app
        .call(AgentList, &[])
        .token(&by.token)
        .query(&AgentListQuery::default())
        .send()
        .await
        .ok()
        .into_iter()
        .map(|agent| match agent {
            orch_api_contract::common::Listed::Hidden(row) => row.name,
            orch_api_contract::common::Listed::Visible(row) => row.content.base.name,
        })
        .collect()
}

#[tokio::test]
async fn an_admin_of_an_org_makes_a_profile_that_is_theirs_alone_to_begin_with() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.admin, c.acme, "Team Codex").await;
    assert_eq!(made.base.name, "Team Codex");
    assert_eq!(made.base.harness_kind, "codex");
    assert_eq!(made.base.address, "acme.admin.Team Codex");
    assert!(!made.base.disabled);
    assert_eq!(made.owner.as_ref().unwrap().username, "admin");
    assert_eq!(made.home.as_ref().unwrap().id, c.acme);
    assert_eq!(made.org_id, Some(c.acme));
    assert_eq!(made.profile["harness_version"], "1.4");
    assert_eq!(made.revision, 1);
    assert_eq!(made.providers.as_deref(), Some(&[][..]));
    assert_eq!(made.models.as_deref(), Some(&[][..]));
    assert!(made.access.permissions.iter().any(|p| p == "agent.manage"));
    // Their own, and nobody else's: the owners of an org name nobody for it.
    assert_eq!(list(&world, &c.admin).await, ["Team Codex"]);
    assert!(list(&world, &c.viewer).await.is_empty());
    assert!(list(&world, &c.stranger).await.is_empty());
    let audited: Vec<(String, Option<Id>, Option<Id>)> = sqlx::query_as(
        "SELECT action, object_id, node_id FROM audit_log WHERE action = 'agent.created'",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        vec![("agent.created".to_owned(), Some(made.base.id), Some(c.acme))]
    );
}

#[tokio::test]
async fn a_profile_is_made_only_by_whoever_makes_the_rules_of_the_org() {
    let world = World::new().await;
    let c = company(&world).await;
    for (who, name) in [(&c.viewer, "viewer"), (&c.stranger, "stranger")] {
        let refused = world
            .app
            .call(OrgAgentCreate, &[&c.acme])
            .token(&who.token)
            .body(&OrgAgentCreateBody {
                name: "Sneaky".to_owned(),
                harness_kind: "codex".to_owned(),
                profile: json!({}),
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{name}");
        assert!(
            missing_permissions(&refused).contains(&"settings.manage".to_owned()),
            "{name}: {refused:?}"
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM agents WHERE name = 'Sneaky'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    // A department is an org: the same rule holds there, from the org above.
    let research = make_department(&world, &c.boss, c.acme, "Research", "research").await;
    let inside = make(&world, &c.admin, research.base.id, "Lab Codex").await;
    assert_eq!(inside.home.as_ref().unwrap().id, research.base.id);
    assert_eq!(inside.org_id, Some(c.acme));
    assert_eq!(inside.base.address, "acme.research.admin.Lab Codex");
}

#[tokio::test]
async fn what_is_wrong_with_a_new_profile_is_told_member_by_member() {
    let world = World::new().await;
    let c = company(&world).await;
    let bad = |name: &str, kind: &str, profile: serde_json::Value| OrgAgentCreateBody {
        name: name.to_owned(),
        harness_kind: kind.to_owned(),
        profile,
    };
    for (body, path) in [
        (bad("  ", "codex", json!({})), "name"),
        (bad(&"n".repeat(81), "codex", json!({})), "name"),
        (bad("Fine", "Not A Kind", json!({})), "harness_kind"),
        (bad("Fine", "codex", json!("text")), "profile"),
        (
            bad("Fine", "codex", json!({ "colour": 1 })),
            "profile.colour",
        ),
        // How a harness is attached is the same choice on every machine; an existing home that is no
        // absolute path is what is refused there.
        (
            bad(
                "Fine",
                "codex",
                json!({ "attach": { "home": { "kind": "existing_directory", "path": "../someone-else" } } }),
            ),
            "profile.attach.home.path",
        ),
    ] {
        let reply = world
            .app
            .call(OrgAgentCreate, &[&c.acme])
            .token(&c.boss.token)
            .body(&body)
            .send()
            .await;
        assert!(!reply.status.is_success(), "{path}: {:?}", reply.body);
        let refused = reply.problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{path}");
        assert_eq!(refused.extra["errors"][0]["path"], path);
    }
    let project = places::make_project(&world, &c.boss, c.acme, "Web").await;
    let not_an_org = world
        .app
        .call(OrgAgentCreate, &[&project.base.id])
        .token(&c.boss.token)
        .body(&bad("Fine", "codex", json!({})))
        .send()
        .await
        .problem();
    assert_eq!(not_an_org.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_profile_is_read_by_whoever_holds_any_permission_on_it_and_by_nobody_else() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.admin, c.acme, "Team Codex").await;
    let id = made.base.id;
    let read = world
        .app
        .call(AgentGet, &[&id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(read.base.id, id);
    // The viewer is named only to look at it: they read it, and find it among the profiles they
    // hold something on, where it gives them nothing to use.
    name_person(&world, "agent", id, "agent.view", c.viewer.id()).await;
    let looked = world
        .app
        .call(AgentGet, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(looked.access.permissions, ["agent.view"]);
    assert!(
        list(&world, &c.viewer)
            .await
            .contains(&"Team Codex".to_owned())
    );
    let refused = world
        .app
        .call(AgentGet, &[&id])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "agent.view");
    assert_eq!(missing_permissions(&refused), ["agent.view"], "{refused:?}");
    let nobody = world.app.call(AgentGet, &[&id]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    let gone = world
        .app
        .call(AgentGet, &[&orch_types::new_id()])
        .token(&c.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_list_offers_the_profiles_one_may_use_or_manage_and_narrows_to_an_org() {
    let world = World::new().await;
    let c = company(&world).await;
    let research = make_department(&world, &c.boss, c.acme, "Research", "research").await;
    let main = make(&world, &c.boss, c.acme, "Main").await;
    let lab = make(&world, &c.boss, research.base.id, "Lab").await;
    // A profile of the system, which everybody may use.
    let mut conn = world.conn().await;
    let system = fixtures::insert_resource(&mut conn, "agent", None, None, "Built-in")
        .await
        .unwrap();
    drop(conn);
    name_on_page(
        &world,
        "agent",
        system,
        "agent.use",
        ("everyone", None, None),
    )
    .await;
    name_person(&world, "agent", main.base.id, "agent.use", c.viewer.id()).await;
    name_person(&world, "agent", lab.base.id, "agent.manage", c.viewer.id()).await;
    let mut names = list(&world, &c.viewer).await;
    names.sort();
    assert_eq!(names, ["Built-in", "Lab", "Main"]);
    assert_eq!(list(&world, &c.stranger).await, ["Built-in"]);
    // Narrowed to the department: what lies inside it, and the system's.
    let narrowed = world
        .app
        .call(AgentList, &[])
        .token(&c.viewer.token)
        .query(&AgentListQuery {
            org_id: Some(research.base.id),
        })
        .send()
        .await
        .ok();
    let narrowed: Vec<_> = narrowed
        .into_iter()
        .filter_map(orch_api_contract::common::Listed::into_visible)
        .collect();
    let mut names: Vec<_> = narrowed.iter().map(|a| a.base.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["Built-in", "Lab"]);
    let system_one = narrowed.iter().find(|a| a.base.name == "Built-in").unwrap();
    assert!(system_one.home.is_none() && system_one.owner.is_none());
    let nobody = world.app.call(AgentList, &[]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_list_of_profiles_costs_the_same_statements_for_a_few_and_for_many() {
    let world = World::new().await;
    let c = company(&world).await;
    for index in 0..2 {
        make(&world, &c.boss, c.acme, &format!("First {index}")).await;
    }
    async fn measure(world: &World, who: &Who) -> (usize, usize) {
        let call = world
            .app
            .call(AgentList, &[])
            .token(&who.token)
            .query(&AgentListQuery::default());
        let before = Statements::now();
        let listed = call.send().await.ok();
        (Statements::now() - before, listed.len())
    }
    let (few, few_count) = measure(&world, &c.boss).await;
    for index in 0..10 {
        make(&world, &c.boss, c.acme, &format!("More {index}")).await;
    }
    let (many, many_count) = measure(&world, &c.boss).await;
    assert_eq!((few_count, many_count), (2, 12));
    assert_eq!(few, many, "{few} statements for 2 profiles, {many} for 12");
}

#[tokio::test]
async fn a_profile_is_changed_by_whoever_manages_it_and_not_by_an_admin_of_its_home_alone() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.boss, c.acme, "Main").await;
    let id = made.base.id;
    // Descriptor overrides operate on an actual typed declaration, not an unreported profile.
    let declaration = json!({"kind":"codex","adapter_version":1,
        "harness":{"name":"Codex","version":"1.4","supported_range":"*","status":"supported"},"arrangements":[]});
    serde_json::from_value::<orch_agent_contract::descriptor::AgentDescriptor>(declaration.clone())
        .unwrap();
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(id)
        .bind(declaration)
        .execute(&world.db.pool)
        .await
        .unwrap();
    // An admin administers the page; it does not manage the profile until the page says so.
    let refused = world
        .app
        .call(AgentUpdate, &[&id])
        .token(&c.admin.token)
        .body(&AgentUpdateBody {
            name: Some("Mine".to_owned()),
            profile: None,
            descriptor_overrides: None,
            disabled: None,
            expected_revision: 1,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "agent.manage");
    assert_eq!(missing_permissions(&refused), ["agent.manage"]);
    let unchanged = world
        .app
        .call(AgentGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(unchanged.base.name, "Main");
    name_person(&world, "agent", id, "agent.manage", c.admin.id()).await;
    let changed = world
        .app
        .call(AgentUpdate, &[&id])
        .token(&c.admin.token)
        .body(&AgentUpdateBody {
            name: Some("  Team Main ".to_owned()),
            profile: Some(json!({ "harness_version": "2.0" })),
            // An override is merged into the typed declaration: it names one of its members.
            descriptor_overrides: Some(json!({ "harness": { "name": "Our Codex" } })),
            disabled: None,
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    assert_eq!(changed.base.name, "Team Main");
    assert_eq!(changed.profile["harness_version"], "2.0");
    assert_eq!(changed.descriptor["harness"]["name"], "Our Codex");
    assert_eq!(changed.descriptor["harness"]["version"], "1.4");
    assert_eq!(changed.descriptor_overridden, ["harness"]);
    assert_eq!(changed.revision, 2);
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action LIKE 'agent.%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audited, ["agent.created", "agent.updated"]);
}

#[tokio::test]
async fn a_change_against_an_old_revision_is_refused_with_what_the_profile_is_now() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.boss, c.acme, "Main").await;
    let id = made.base.id;
    let rename = |name: &str, revision: i64| AgentUpdateBody {
        name: Some(name.to_owned()),
        profile: None,
        descriptor_overrides: None,
        disabled: None,
        expected_revision: revision,
    };
    world
        .app
        .call(AgentUpdate, &[&id])
        .token(&c.boss.token)
        .body(&rename("Second", 1))
        .send()
        .await
        .ok();
    let stale = world
        .app
        .call(AgentUpdate, &[&id])
        .token(&c.boss.token)
        .body(&rename("Third", 1))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["name"], "Second");
    assert_eq!(stale.extra["current"]["revision"], 2);
    // A change that asks for nothing is the profile as it is, and is stale just the same.
    let nothing = |revision: i64| AgentUpdateBody {
        name: None,
        profile: None,
        descriptor_overrides: None,
        disabled: None,
        expected_revision: revision,
    };
    let same = world
        .app
        .call(AgentUpdate, &[&id])
        .token(&c.boss.token)
        .body(&nothing(2))
        .send()
        .await
        .ok();
    assert_eq!(same.revision, 2);
    let old = world
        .app
        .call(AgentUpdate, &[&id])
        .token(&c.boss.token)
        .body(&nothing(1))
        .send()
        .await
        .problem();
    assert_eq!(old.code, ErrorCode::ConflictStaleRevision);
}

#[tokio::test]
async fn a_profile_that_is_switched_off_says_so_and_comes_back_when_switched_on() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.boss, c.acme, "Main").await;
    let id = made.base.id;
    let toggle = |disabled: bool, revision: i64| AgentUpdateBody {
        name: None,
        profile: None,
        descriptor_overrides: None,
        disabled: Some(disabled),
        expected_revision: revision,
    };
    let off = world
        .app
        .call(AgentUpdate, &[&id])
        .token(&c.boss.token)
        .body(&toggle(true, 1))
        .send()
        .await
        .ok();
    assert!(off.base.disabled);
    // Switched off, it keeps its page and is still listed to the people who manage it.
    assert_eq!(list(&world, &c.boss).await, ["Main"]);
    let on = world
        .app
        .call(AgentUpdate, &[&id])
        .token(&c.boss.token)
        .body(&toggle(false, 2))
        .send()
        .await
        .ok();
    assert!(!on.base.disabled);
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action IN ('agent.disabled', 'agent.enabled') ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audited, ["agent.disabled", "agent.enabled"]);
    // Setting it to what it is writes no line.
    world
        .app
        .call(AgentUpdate, &[&id])
        .token(&c.boss.token)
        .body(&toggle(false, 3))
        .send()
        .await
        .ok();
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action IN ('agent.disabled', 'agent.enabled')",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(after, 2);
}

#[tokio::test]
async fn deleting_a_profile_keeps_a_providers_binding_and_never_erases_the_provider() {
    let world = World::new().await;
    let c = company(&world).await;
    let main = make(&world, &c.boss, c.acme, "Main").await.base.id;
    let spare = make(&world, &c.boss, c.acme, "Spare").await.base.id;
    let refused = world
        .app
        .call(AgentDelete, &[&spare])
        .token(&c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["agent.manage"]);
    // A provider keeps its reference for an explicit later switch; it does not prevent deletion.
    let mut conn = world.conn().await;
    let provider =
        fixtures::insert_subscription(&mut conn, c.boss.id(), c.acme, "Team Pro", "codex")
            .await
            .unwrap();
    sqlx::query("UPDATE providers SET agent_id = $2 WHERE id = $1")
        .bind(provider)
        .bind(main)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let deleted_bound = world
        .app
        .call(AgentDelete, &[&main])
        .token(&c.boss.token)
        .send()
        .await;
    assert_eq!(deleted_bound.status, 204, "{:?}", deleted_bound.body);
    let kept: (Id, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT agent_id,deleted_at FROM providers WHERE id=$1")
            .bind(provider)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        kept,
        (main, None),
        "deleting the profile keeps its provider and the explicit reference"
    );
    let deleted = world
        .app
        .call(AgentDelete, &[&spare])
        .token(&c.boss.token)
        .send()
        .await;
    assert_eq!(deleted.status, 204, "{:?}", deleted.body);
    let gone = world
        .app
        .call(AgentGet, &[&spare])
        .token(&c.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
    assert_eq!(
        list(&world, &c.boss).await,
        Vec::<String>::new(),
        "deleted profiles are not listed"
    );
    let audited: Vec<(String, Option<Id>)> =
        sqlx::query_as("SELECT action, object_id FROM audit_log WHERE action = 'agent.deleted'")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    let mut audited = audited;
    audited.sort_unstable();
    let mut expected = vec![
        ("agent.deleted".to_owned(), Some(main)),
        ("agent.deleted".to_owned(), Some(spare)),
    ];
    expected.sort_unstable();
    assert_eq!(
        audited, expected,
        "one record per deletion, none for the refused call"
    );
}
