//! Environments over HTTP: a directory of a person's machine registered by its owner, listing and reading by what the caller holds, changing, switching off, removing, and the environment's own settings and secrets.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::binding::NewEnvironmentAccess;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{
    Environment, EnvironmentDelete, EnvironmentDeleteQuery, EnvironmentDisable, EnvironmentEnable,
    EnvironmentGet, EnvironmentHolding, EnvironmentList, EnvironmentListQuery, EnvironmentStatus,
    EnvironmentUpdate, EnvironmentUpdateBody, MachineEnvironmentCreate,
    MachineEnvironmentCreateBody,
};
use orch_api_contract::settings::{
    EnvironmentSecretDelete, EnvironmentSecretList, EnvironmentSecretSet, EnvironmentSettingList,
    NodeSecretSetBody,
};
use orch_api_core::routes::resources::sealing::{binding, install_master_key};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::secrets::MasterKey;
use orch_link_contract::policy::LocalPolicy;
use orch_types::Id;
use places::{Who, make_project, make_topic, missing_permissions};
use resources::{Company, company, name_person};
use serde_json::json;
use support::{Statements, World};

async fn world() -> World {
    install_master_key(MasterKey::new([7; 32]));
    orch_api_core::routes::settings::install_master_key(MasterKey::new([7; 32]));
    World::new().await
}

/// A machine of `owner`'s whose local policy lets environments be under `/srv/ada` and its exec
/// node offer `plain`.
async fn machine(world: &World, owner: &Who) -> Id {
    let mut conn = world.conn().await;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut conn,
        owner.id(),
        "laptop",
    )
    .await
    .unwrap();
    // The fixture starts its live Owners page as enrollment does: its owner's alone.
    let policy = LocalPolicy {
        environment_roots: vec!["/srv/ada".to_owned()],
        exec_offers: vec!["plain".to_owned()],
        ..LocalPolicy::default()
    };
    sqlx::query(
        "UPDATE machines SET facts = jsonb_build_object('policy', $2::jsonb,
            'environment_descriptor', jsonb_build_object('lifecycle','attach')) WHERE id = $1",
    )
    .bind(machine)
    .bind(serde_json::to_value(&policy).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    machine
}

/// These positive switch cases need the operator's ceiling as well as the resource's own choice.
async fn offer_switches(world: &World, machine: Id) {
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',
        (facts->'policy') || jsonb_build_object('environment.remote_write',true,'environment.terminal',true)) WHERE id=$1")
        .bind(machine).execute(&world.db.pool).await.unwrap();
}

fn body(home: Id, name: &str, root: &str) -> MachineEnvironmentCreateBody {
    MachineEnvironmentCreateBody {
        data_sources: None,
        home_id: home,
        name: name.to_owned(),
        root: root.to_owned(),
        home_node_id: None,
        remote_write: None,
        terminal: None,
        access: None,
    }
}

async fn register(
    world: &World,
    by: &Who,
    machine: Id,
    body: &MachineEnvironmentCreateBody,
) -> Environment {
    world
        .app
        .call(MachineEnvironmentCreate, &[&machine])
        .token(&by.token)
        .body(body)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn workspace_and_home_choices_use_the_same_machine_roots_and_reject_an_outside_home() {
    use orch_driver_contract::runtime::{EnvironmentDataSource, EnvironmentDataSources};
    let world = world().await;
    let c = company(&world).await;
    let laptop = machine(&world, &c.admin).await;
    let mut wanted = body(c.acme, "Chosen data", "");
    wanted.data_sources = Some(EnvironmentDataSources {
        workspace: EnvironmentDataSource::MadeByOrch,
        home: EnvironmentDataSource::ExistingDirectory {
            path: "/outside/home".into(),
        },
    });
    let refused = world
        .app
        .call(MachineEnvironmentCreate, &[&laptop])
        .token(&c.admin.token)
        .body(&wanted)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(refused.extra["errors"].to_string().contains("data_sources"));
    wanted.data_sources.as_mut().unwrap().home = EnvironmentDataSource::ExistingDirectory {
        path: "/srv/ada/home".into(),
    };
    let made = world
        .app
        .call(MachineEnvironmentCreate, &[&laptop])
        .token(&c.admin.token)
        .body(&wanted)
        .send()
        .await
        .ok();
    assert_eq!(made.data_sources, wanted.data_sources);
    assert_eq!(
        made.config["env.data_sources"]["workspace"]["kind"],
        "made_by_orch"
    );
    let hidden = world
        .app
        .call(MachineEnvironmentCreate, &[&laptop])
        .token(&c.stranger.token)
        .body(&wanted)
        .send()
        .await
        .problem();
    assert_eq!(hidden.code, ErrorCode::NotFound);
}

async fn names(world: &World, by: &Who, query: EnvironmentListQuery) -> Vec<String> {
    let mut listed: Vec<String> = world
        .app
        .call(EnvironmentList, &[])
        .token(&by.token)
        .query(&query)
        .send()
        .await
        .ok()
        .into_iter()
        .map(|environment| match environment {
            orch_api_contract::common::Listed::Hidden(row) => row.name,
            orch_api_contract::common::Listed::Visible(row) => row.content.base.name,
        })
        .collect();
    listed.sort();
    listed
}

/// The company with its admin's laptop: the environments of these tests are the admin's.
async fn setup(world: &World) -> (Company, Id) {
    let c = company(world).await;
    let laptop = machine(world, &c.admin).await;
    (c, laptop)
}

#[tokio::test]
async fn the_owner_of_a_machine_registers_a_directory_of_it_as_an_environment_of_theirs() {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let made = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Work", "/srv/ada/work"),
    )
    .await;
    assert_eq!(made.base.name, "Work");
    assert_eq!(made.base.status, EnvironmentStatus::NotMade);
    assert_eq!(made.base.owner.username, "admin");
    assert_eq!(made.base.address, "acme.admin.work");
    assert_eq!(made.machine.as_ref().unwrap().id, laptop);
    assert_eq!(made.home.id, c.acme);
    assert_eq!(made.org_id, c.acme);
    assert_eq!(made.workspace_root, "/srv/ada/work");
    assert!(!made.remote_write && !made.terminal, "both start off");
    assert!(made.template.is_none() && made.host.is_none());
    assert_eq!(
        made.exec.machine_offers.as_deref(),
        Some(&["plain".to_owned()][..])
    );
    assert!(
        made.access
            .permissions
            .iter()
            .any(|p| p == "environment.manage")
    );
    let audited: Vec<(String, Option<Id>, Option<Id>)> = sqlx::query_as(
        "SELECT action, object_id, node_id FROM audit_log WHERE action = 'environment.created'",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        vec![(
            "environment.created".to_owned(),
            Some(made.base.id),
            Some(c.acme)
        )]
    );
    assert_eq!(
        names(&world, &c.admin, EnvironmentListQuery::default()).await,
        ["Work"]
    );
    assert!(
        names(&world, &c.stranger, EnvironmentListQuery::default())
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_directory_is_registered_by_the_machines_owner_in_their_domain_inside_the_machines_roots()
{
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let attempt = |who: &Who, machine: Id, body: MachineEnvironmentCreateBody| {
        let call = world
            .app
            .call(MachineEnvironmentCreate, &[&machine])
            .token(&who.token)
            .body(&body);
        async move { call.send().await.problem() }
    };
    // Somebody else's machine is not there for the caller.
    let stranger = attempt(&c.viewer, laptop, body(c.acme, "Work", "/srv/ada/work")).await;
    assert_eq!(stranger.code, ErrorCode::NotFound);
    // A root the machine's local policy does not list, a path that is not absolute, one that climbs.
    for (root, path) in [
        ("/etc", "root"),
        ("work", "root"),
        ("/srv/ada/../root", "root"),
    ] {
        let refused = attempt(&c.admin, laptop, body(c.acme, "Work", root)).await;
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{root}");
        assert!(refused.extra["errors"].to_string().contains(path));
    }
    let empty = attempt(&c.admin, laptop, body(c.acme, "  ", "/srv/ada/work")).await;
    assert_eq!(empty.code, ErrorCode::RequestInvalid);
    // An org outside the caller's domain: the stranger is no member of Acme.
    let other = machine(&world, &c.stranger).await;
    let outside = attempt(&c.stranger, other, body(c.acme, "Work", "/srv/ada/work")).await;
    assert_eq!(outside.code, ErrorCode::RequestInvalid);
    assert!(outside.extra["errors"].to_string().contains("home_id"));
    // A project or topic of another org is no home node.
    let project = make_project(&world, &c.boss, c.acme, "Web").await;
    let other_org = places::make_org(&world, &c.admin, "Other", "other").await;
    let mut wrong = body(other_org.base.id, "Work", "/srv/ada/work");
    wrong.home_node_id = Some(project.base.id);
    let refused = attempt(&c.admin, laptop, wrong).await;
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let nothing: i64 = sqlx::query_scalar("SELECT count(*) FROM environments")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(nothing, 0);
}

#[tokio::test]
async fn an_environment_takes_its_settings_from_a_project_or_topic_of_its_org_and_is_the_makers_to_begin_with()
 {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    offer_switches(&world, laptop).await;
    let project = make_project(&world, &c.boss, c.acme, "Web").await;
    let topic = make_topic(&world, &c.boss, project.base.id, "Login").await;
    // The admin of the org may interact with the topic: they administer the tree.
    let mut at_topic = body(c.acme, "At topic", "/srv/ada/work");
    at_topic.home_node_id = Some(topic.base.id);
    at_topic.access = Some(NewEnvironmentAccess::OnlyMe);
    at_topic.terminal = Some(true);
    let made = register(&world, &c.admin, laptop, &at_topic).await;
    assert_eq!(made.home.id, topic.base.id);
    assert_eq!(made.org_id, c.acme);
    assert!(made.terminal);
    let again = world
        .app
        .call(MachineEnvironmentCreate, &[&laptop])
        .token(&c.admin.token)
        .body(&body(c.acme, "AT TOPIC", "/srv/ada/other"))
        .send()
        .await
        .problem();
    assert_eq!(
        again.code,
        ErrorCode::ConflictExists,
        "a name is taken whatever its case"
    );
    assert_eq!(again.extra["existing_id"], json!(made.base.id));
}

#[tokio::test]
async fn an_environment_is_read_by_whoever_holds_something_on_it_and_what_it_holds_is_the_managers()
{
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let made = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Work", "/srv/ada/work"),
    )
    .await;
    let id = made.base.id;
    let own = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&c.admin.token)
        .body(&EnvironmentUpdateBody {
            name: None,
            config: Some(json!({ "exec.offers": ["plain"] })),
            data_sources: None,
            remote_write: None,
            terminal: None,
            browser: None,
            vnc: None,
            home_node_id: None,
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    assert_eq!(own.config["exec.offers"], json!(["plain"]));
    // As shipped, the viewers of the org see it; they do not see how it is configured.
    let seen = world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&c.viewer.token)
        .send()
        .await;
    if let Ok(seen) = &seen.body {
        assert_eq!(seen.config, json!({}));
        assert!(
            !seen
                .access
                .permissions
                .iter()
                .any(|p| p == "environment.manage")
        );
    }
    name_person(
        &world,
        "environment",
        id,
        "environment.view",
        c.stranger.id(),
    )
    .await;
    let looked = world
        .app
        .call(EnvironmentGet, &[&id])
        .token(&c.stranger.token)
        .send()
        .await
        .ok();
    assert_eq!(looked.base.id, id);
    assert_eq!(
        looked.config,
        json!({}),
        "its configuration is for those who manage it"
    );
    assert_eq!(looked.access.permissions, ["environment.view"]);
    let nobody = world
        .app
        .call(EnvironmentGet, &[&id])
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    let missing = world
        .app
        .call(EnvironmentGet, &[&orch_types::new_id()])
        .token(&c.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn an_environment_that_nobody_may_see_is_refused_with_what_is_missing() {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    // A person outside the org holds nothing on it.
    let made = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Work", "/srv/ada/work"),
    )
    .await;
    let refused = world
        .app
        .call(EnvironmentGet, &[&made.base.id])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "environment.view");
    assert_eq!(missing_permissions(&refused), ["environment.view"]);
}

#[tokio::test]
async fn a_list_of_environments_narrows_by_what_is_held_and_by_org_and_costs_the_same_statements_for_a_few_and_for_many()
 {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let research = places::make_department(&world, &c.boss, c.acme, "Research", "research").await;
    register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "One", "/srv/ada/one"),
    )
    .await;
    let lab = register(
        &world,
        &c.admin,
        laptop,
        &body(research.base.id, "Lab", "/srv/ada/lab"),
    )
    .await;
    // As shipped the viewers of the org see its environments; a stranger sees what is named for them.
    assert_eq!(
        names(&world, &c.viewer, EnvironmentListQuery::default()).await,
        ["Lab", "One"]
    );
    name_person(
        &world,
        "environment",
        lab.base.id,
        "environment.view",
        c.stranger.id(),
    )
    .await;
    assert_eq!(
        names(&world, &c.stranger, EnvironmentListQuery::default()).await,
        ["Lab"]
    );
    assert!(
        names(
            &world,
            &c.viewer,
            EnvironmentListQuery {
                org_id: None,
                holding: Some(EnvironmentHolding::Manage)
            }
        )
        .await
        .is_empty()
    );
    assert_eq!(
        names(
            &world,
            &c.admin,
            EnvironmentListQuery {
                org_id: Some(research.base.id),
                holding: None
            }
        )
        .await,
        ["Lab"]
    );
    async fn measure(world: &World, who: &Who) -> (usize, usize) {
        let call = world
            .app
            .call(EnvironmentList, &[])
            .token(&who.token)
            .query(&EnvironmentListQuery::default());
        let before = Statements::now();
        let listed = call.send().await.ok();
        (Statements::now() - before, listed.len())
    }
    let (few, few_count) = measure(&world, &c.admin).await;
    for index in 0..9 {
        register(
            &world,
            &c.admin,
            laptop,
            &body(
                c.acme,
                &format!("More {index}"),
                &format!("/srv/ada/m{index}"),
            ),
        )
        .await;
    }
    let (many, many_count) = measure(&world, &c.admin).await;
    assert_eq!((few_count, many_count), (2, 11));
    assert_eq!(
        few, many,
        "{few} statements for 2 environments, {many} for 11"
    );
}

#[tokio::test]
async fn eleven_environments_in_different_departments_have_one_grouped_audience_read_not_one_walk_per_chip()
 {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    async fn measured(world: &World, who: &Who) -> (usize, Vec<Environment>) {
        let before = Statements::now();
        let rows = world
            .app
            .call(EnvironmentList, &[])
            .token(&who.token)
            .send()
            .await
            .ok();
        let statements = Statements::now() - before;
        (
            statements,
            rows.into_iter()
                .filter_map(|row| match row {
                    orch_api_contract::common::Listed::Visible(row) => Some(row.content),
                    orch_api_contract::common::Listed::Hidden(_) => None,
                })
                .collect(),
        )
    }
    register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "First", "/srv/ada/first"),
    )
    .await;
    // A viewer's list includes the role chips; an owner's early path could omit them.
    let (one, first) = measured(&world, &c.viewer).await;
    assert_eq!(first.len(), 1);
    for index in 0..10 {
        let department = places::make_department(
            &world,
            &c.boss,
            c.acme,
            &format!("Department {index}"),
            &format!("dept{index}"),
        )
        .await
        .base
        .id;
        register(
            &world,
            &c.admin,
            laptop,
            &body(
                department,
                &format!("Work {index}"),
                &format!("/srv/ada/work{index}"),
            ),
        )
        .await;
    }
    let (eleven, rows) = measured(&world, &c.viewer).await;
    assert_eq!(rows.len(), 11);
    assert_eq!(
        one, eleven,
        "eleven distinct home-role chips and owners must not add statements"
    );
    let mut conn = world.conn().await;
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(c.admin.id()),
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    for row in rows {
        let detailed = orch_authz::environments::reach(
            &mut conn,
            &principal,
            row.base.id,
            orch_authz::Policy::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            row.reach,
            i64::try_from(detailed.people.len()).unwrap() + detailed.hidden
        );
    }
}

#[tokio::test]
async fn an_environment_is_changed_by_whoever_manages_it_within_what_its_machine_offers() {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let made = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Work", "/srv/ada/work"),
    )
    .await;
    let id = made.base.id;
    offer_switches(&world, laptop).await;
    let change = |revision: i64| EnvironmentUpdateBody {
        data_sources: None,
        name: Some("  Renamed ".to_owned()),
        config: Some(json!({ "exec.offers": ["plain"], "exec.orch_tools": false })),
        remote_write: Some(true),
        terminal: Some(true),
        browser: None,
        vnc: None,
        home_node_id: None,
        expected_revision: revision,
    };
    name_person(&world, "environment", id, "environment.view", c.viewer.id()).await;
    let refused = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&c.viewer.token)
        .body(&change(1))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["environment.manage"]);
    let changed = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&c.admin.token)
        .body(&change(1))
        .send()
        .await
        .ok();
    assert_eq!(changed.base.name, "Renamed");
    assert!(changed.remote_write && changed.terminal);
    assert_eq!(changed.config["exec.offers"], json!(["plain"]));
    assert!(!changed.exec.orch_tools);
    assert_eq!(changed.revision, 2);
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action LIKE 'environment.%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        [
            "environment.created",
            "environment.updated",
            "environment.exec_changed"
        ]
    );
    let stale = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&c.admin.token)
        .body(&change(1))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["name"], "Renamed");
    // What its machine does not offer, and what is no setting of an environment, are refused.
    let beyond = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&c.admin.token)
        .body(&EnvironmentUpdateBody {
            name: None,
            config: Some(json!({ "exec.offers": ["plain", "docker"] })),
            data_sources: None,
            remote_write: None,
            terminal: None,
            browser: None,
            vnc: None,
            home_node_id: None,
            expected_revision: 2,
        })
        .send()
        .await
        .problem();
    assert_eq!(beyond.code, ErrorCode::PolicyExecNotOffered);
    assert_eq!(beyond.extra["offers"], json!(["plain"]));
    for config in [
        json!({ "no.such.key": 1 }),
        json!({ "agent.model": "x" }),
        json!({ "exec.orch_tools": "yes" }),
        json!([]),
    ] {
        let invalid = world
            .app
            .call(EnvironmentUpdate, &[&id])
            .token(&c.admin.token)
            .body(&EnvironmentUpdateBody {
                name: None,
                config: Some(config.clone()),
                data_sources: None,
                remote_write: None,
                terminal: None,
                browser: None,
                vnc: None,
                home_node_id: None,
                expected_revision: 2,
            })
            .send()
            .await
            .problem();
        assert_eq!(invalid.code, ErrorCode::RequestInvalid, "{config}");
    }
}

#[tokio::test]
async fn another_home_is_taken_only_by_whoever_may_interact_there_and_only_in_the_same_org() {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let made = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Work", "/srv/ada/work"),
    )
    .await;
    let id = made.base.id;
    let project = make_project(&world, &c.boss, c.acme, "Web").await;
    let moved = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&c.admin.token)
        .body(&EnvironmentUpdateBody {
            name: None,
            config: None,
            remote_write: None,
            terminal: None,
            browser: None,
            vnc: None,
            home_node_id: Some(project.base.id),
            data_sources: None,
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    assert_eq!(moved.home.id, project.base.id);
    let other = places::make_org(&world, &c.admin, "Other", "other").await;
    let outside = world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&c.admin.token)
        .body(&EnvironmentUpdateBody {
            name: None,
            config: None,
            remote_write: None,
            terminal: None,
            browser: None,
            vnc: None,
            home_node_id: Some(other.base.id),
            data_sources: None,
            expected_revision: 2,
        })
        .send()
        .await
        .problem();
    assert_eq!(outside.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn an_environment_is_switched_off_and_on_by_whoever_manages_it() {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let made = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Work", "/srv/ada/work"),
    )
    .await;
    let id = made.base.id;
    name_person(&world, "environment", id, "environment.view", c.viewer.id()).await;
    let refused = world
        .app
        .call(EnvironmentDisable, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["environment.manage"]);
    let off = world
        .app
        .call(EnvironmentDisable, &[&id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(off.base.status, EnvironmentStatus::Disabled);
    let again = world
        .app
        .call(EnvironmentDisable, &[&id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(
        again.revision, off.revision,
        "switching off what is off changes nothing"
    );
    let on = world
        .app
        .call(EnvironmentEnable, &[&id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(on.base.status, EnvironmentStatus::NotMade);
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action IN ('environment.disabled', 'environment.enabled') ORDER BY at, id",
    )
    .fetch_all(&world.db.pool).await.unwrap();
    assert_eq!(audited, ["environment.disabled", "environment.enabled"]);
}

#[tokio::test]
async fn confirmed_directory_removal_settles_its_binding_and_forgets_only_the_registration() {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let busy = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Busy", "/srv/ada/busy"),
    )
    .await
    .base
    .id;
    let idle = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Idle", "/srv/ada/idle"),
    )
    .await
    .base
    .id;
    let project = make_project(&world, &c.boss, c.acme, "Web").await;
    let topic = make_topic(&world, &c.boss, project.base.id, "Login").await;
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
    sqlx::query("UPDATE threads SET environment_id = $2 WHERE node_id = $1")
        .bind(thread)
        .bind(busy)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let query = |name: &str| EnvironmentDeleteQuery {
        confirm: name.to_owned(),
        at: None,
    };
    let wrong = world
        .app
        .call(EnvironmentDelete, &[&idle])
        .token(&c.admin.token)
        .query(&query("Nope"))
        .send()
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::RequestInvalid);
    let removed_busy = world
        .app
        .call(EnvironmentDelete, &[&busy])
        .token(&c.admin.token)
        .query(&query("Busy"))
        .send()
        .await
        .ok();
    assert_eq!(removed_busy.base.status, EnvironmentStatus::Removed);
    assert!(removed_busy.remove_at.is_some());
    let binding: (Option<orch_types::Id>, String, Option<String>) =
        sqlx::query_as("SELECT environment_id,state,blocked_reason FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        binding,
        (None, "blocked".to_owned(), Some("unbound".to_owned()))
    );
    let removed = world
        .app
        .call(EnvironmentDelete, &[&idle])
        .token(&c.admin.token)
        .query(&query("Idle"))
        .send()
        .await
        .ok();
    assert!(removed.remove_at.is_some());
    assert!(
        names(&world, &c.admin, EnvironmentListQuery::default())
            .await
            .is_empty()
    );
    let gone = world
        .app
        .call(EnvironmentGet, &[&idle])
        .token(&c.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
    let gone_busy = world
        .app
        .call(EnvironmentGet, &[&busy])
        .token(&c.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(gone_busy.code, ErrorCode::NotFound);
    let native_removals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='unit.remove' AND machine_id=$1",
    )
    .bind(laptop)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(native_removals, 0, "only the registrations were forgotten");
    let audited: Vec<Option<Id>> = sqlx::query_scalar(
        "SELECT object_id FROM audit_log WHERE action = 'environment.deleted' ORDER BY at,id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audited, [Some(busy), Some(idle)]);
}

#[tokio::test]
async fn an_environments_secrets_are_sealed_listed_by_name_only_and_written_by_whoever_manages_it()
{
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let made = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Work", "/srv/ada/work"),
    )
    .await;
    let id = made.base.id;
    name_person(&world, "environment", id, "environment.view", c.viewer.id()).await;
    let value = "token-do-not-show-9";
    let refused = world
        .app
        .call(EnvironmentSecretSet, &[&id, &"GITHUB_TOKEN"])
        .token(&c.viewer.token)
        .body(&NodeSecretSetBody {
            value: value.to_owned(),
        })
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["environment.manage"]);
    let set = world
        .app
        .call(EnvironmentSecretSet, &[&id, &"GITHUB_TOKEN"])
        .token(&c.admin.token)
        .body(&NodeSecretSetBody {
            value: value.to_owned(),
        })
        .send()
        .await;
    assert!(!String::from_utf8_lossy(&set.raw).contains(value));
    assert_eq!(set.ok().name, "GITHUB_TOKEN");
    let listed = world
        .app
        .call(EnvironmentSecretList, &[&id])
        .token(&c.admin.token)
        .send()
        .await;
    assert!(!String::from_utf8_lossy(&listed.raw).contains(value));
    assert_eq!(
        listed
            .ok()
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["GITHUB_TOKEN"]
    );
    let sealed: Vec<u8> = sqlx::query_scalar(
        "SELECT sealed FROM secrets WHERE owner_kind = 'environment' AND owner_id = $1 AND name = 'GITHUB_TOKEN'",
    )
    .bind(id).fetch_one(&world.db.pool).await.unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("do-not-show"));
    assert_eq!(
        MasterKey::new([7; 32])
            .open(&sealed, &binding("environment", id, "GITHUB_TOKEN"))
            .unwrap(),
        value.as_bytes()
    );
    let removed = world
        .app
        .call(EnvironmentSecretDelete, &[&id, &"GITHUB_TOKEN"])
        .token(&c.admin.token)
        .send()
        .await;
    assert_eq!(removed.status, 204, "{:?}", removed.body);
    let none = world
        .app
        .call(EnvironmentSecretList, &[&id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert!(none.is_empty());
    let bad = world
        .app
        .call(EnvironmentSecretSet, &[&id, &"lower-case"])
        .token(&c.admin.token)
        .body(&NodeSecretSetBody {
            value: "x".to_owned(),
        })
        .send()
        .await
        .problem();
    assert_eq!(bad.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn the_settings_of_an_environment_are_what_its_home_resolves_and_then_its_own() {
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let made = register(
        &world,
        &c.admin,
        laptop,
        &body(c.acme, "Work", "/srv/ada/work"),
    )
    .await;
    let id = made.base.id;
    world
        .app
        .call(EnvironmentUpdate, &[&id])
        .token(&c.admin.token)
        .body(&EnvironmentUpdateBody {
            name: None,
            config: Some(json!({ "exec.orch_tools": false })),
            data_sources: None,
            remote_write: None,
            terminal: None,
            browser: None,
            vnc: None,
            home_node_id: None,
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    let settings = world
        .app
        .call(EnvironmentSettingList, &[&id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert!(!settings.is_empty());
    let tools = settings
        .iter()
        .find(|s| s.key == "exec.orch_tools")
        .unwrap();
    assert_eq!(
        tools.value,
        Some(json!(false)),
        "its own value comes after what its home resolves"
    );
    let stranger = world
        .app
        .call(EnvironmentSettingList, &[&id])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&stranger), ["environment.view"]);
}

#[tokio::test]
async fn program_versions_resolve_system_org_project_and_environment_and_honor_an_ancestor_lock() {
    use orch_db::queries::settings::{self as rows, Stored};
    let world = world().await;
    let (c, laptop) = setup(&world).await;
    let project = make_project(&world, &c.boss, c.acme, "Programs").await;
    let mut registration = body(c.acme, "Pinned", "/srv/ada/pinned");
    registration.home_node_id = Some(project.base.id);
    let environment = register(&world, &c.admin, laptop, &registration).await;
    let key = "agents.harness_versions";
    for (node, version) in [
        (None, "0.160.0"),
        (Some(c.acme), "0.161.0"),
        (Some(project.base.id), "0.162.0"),
    ] {
        rows::put(
            &mut *world.conn().await,
            node,
            key,
            &Stored {
                value: &json!({"codex":{"use":"exact","version":version}}),
                locked: false,
                by: Some(c.admin.id()),
                at: world.now(),
            },
            None,
        )
        .await
        .unwrap();
    }
    let app = &world.app;
    let token = &c.admin.token;
    let get = |id| async move {
        app.call(EnvironmentSettingList, &[&id])
            .token(token)
            .send()
            .await
            .ok()
    };
    let inherited = get(environment.base.id).await;
    assert_eq!(
        inherited
            .iter()
            .find(|setting| setting.key == key)
            .unwrap()
            .value
            .as_ref()
            .unwrap()["codex"]["version"],
        "0.162.0"
    );
    world
        .app
        .call(EnvironmentUpdate, &[&environment.base.id])
        .token(&c.admin.token)
        .body(&EnvironmentUpdateBody {
            name: None,
            config: Some(json!({key:{"codex":{"use":"exact","version":"0.159.0"}}})),
            data_sources: None,
            remote_write: None,
            terminal: None,
            browser: None,
            vnc: None,
            home_node_id: None,
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    let own = get(environment.base.id).await;
    assert_eq!(
        own.iter()
            .find(|setting| setting.key == key)
            .unwrap()
            .value
            .as_ref()
            .unwrap()["codex"]["version"],
        "0.159.0"
    );
    rows::put(
        &mut *world.conn().await,
        Some(c.acme),
        key,
        &Stored {
            value: &json!({"codex":{"use":"shipped"}}),
            locked: true,
            by: Some(c.admin.id()),
            at: world.now(),
        },
        None,
    )
    .await
    .unwrap();
    let locked = get(environment.base.id).await;
    assert_eq!(
        locked
            .iter()
            .find(|setting| setting.key == key)
            .unwrap()
            .value
            .as_ref()
            .unwrap()["codex"]["use"],
        "shipped"
    );
}

// Permission to use a physical machine, ownership of a registered
// environment, and inherent management of that machine's environments are distinct.
mod delegated_machine_registration {
    use super::*;
    use orch_api_contract::access::{
        HolderInput, PageChange, PageChangeOwners, PageChangeRow, PermissionPageUpdate,
    };

    struct Scene {
        world: World,
        company: Company,
        machine: Id,
        home: Id,
    }

    async fn scene() -> Scene {
        let world = world().await;
        let company = company(&world).await;
        let machine = machine(&world, &company.admin).await;
        // The creator owns their home; the machine owner is no member of it.
        // This prevents org administration from masking inherent machine authority.
        let home = places::make_org(&world, &company.stranger, "Creator home", "creator-home")
            .await
            .base
            .id;
        Scene {
            world,
            company,
            machine,
            home,
        }
    }

    async fn grant(s: &Scene, maximum: Option<u32>) -> Id {
        let entry = name_person(
            &s.world,
            "machine",
            s.machine,
            "machine.use",
            s.company.stranger.id(),
        )
        .await;
        if let Some(maximum) = maximum {
            sqlx::query("UPDATE permission_entries SET options=$2 WHERE id=$1")
                .bind(entry)
                .bind(json!({ "max_environments": maximum }))
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        entry
    }

    fn request(s: &Scene, name: &str) -> MachineEnvironmentCreateBody {
        let mut wanted = body(s.home, name, &format!("/srv/ada/{name}"));
        wanted.access = Some(NewEnvironmentAccess::OnlyMe);
        wanted
    }

    async fn create(s: &Scene, name: &str) -> Environment {
        register(&s.world, &s.company.stranger, s.machine, &request(s, name)).await
    }

    async fn canonical_owner(s: &Scene, environment: Id, owner: Id) {
        let stored: Id = sqlx::query_scalar("SELECT owner_id FROM environments WHERE id=$1")
            .bind(environment)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        assert_eq!(stored, owner, "the stored first owner agrees with the page");
        let owners: Vec<Id> = sqlx::query_scalar(
            "SELECT holder_id FROM permission_entries WHERE object_kind='environment'
             AND object_id=$1 AND page='live' AND permission='access.own'
             AND holder_kind='user' AND removed_at IS NULL ORDER BY holder_id",
        )
        .bind(environment)
        .fetch_all(&s.world.db.pool)
        .await
        .unwrap();
        assert_eq!(
            owners,
            [owner],
            "machine management adds no permanent owner"
        );
    }

    async fn machine_owner_can_manage(s: &Scene, environment: Id) -> Environment {
        let shown = s
            .world
            .app
            .call(EnvironmentGet, &[&environment])
            .token(&s.company.admin.token)
            .send()
            .await
            .ok();
        assert!(
            shown
                .access
                .permissions
                .iter()
                .any(|p| p == "environment.manage")
        );
        let changed = s
            .world
            .app
            .call(EnvironmentUpdate, &[&environment])
            .token(&s.company.admin.token)
            .body(&EnvironmentUpdateBody {
                data_sources: None,
                name: Some(format!("{} managed", shown.base.name)),
                config: None,
                remote_write: None,
                terminal: None,
                browser: None,
                vnc: None,
                home_node_id: None,
                expected_revision: shown.revision,
            })
            .send()
            .await
            .ok();
        assert_eq!(changed.revision, shown.revision + 1);
        changed
    }

    #[tokio::test]
    async fn shared_use_registers_for_its_creator_while_the_machine_owner_inherently_manages() {
        let s = scene().await;
        let before = s
            .world
            .app
            .call(MachineEnvironmentCreate, &[&s.machine])
            .token(&s.company.stranger.token)
            .body(&request(&s, "without-grant"))
            .send()
            .await
            .problem();
        assert_eq!(before.code, ErrorCode::NotFound);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM environments")
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);

        grant(&s, None).await;
        let made = create(&s, "delegated").await;
        assert_eq!(made.base.owner.id, s.company.stranger.id());
        assert_eq!(made.home.id, s.home);
        assert_eq!(made.machine.as_ref().unwrap().id, s.machine);
        assert!(
            made.access
                .permissions
                .iter()
                .any(|p| p == "environment.manage")
        );
        canonical_owner(&s, made.base.id, s.company.stranger.id()).await;
        let changed = machine_owner_can_manage(&s, made.base.id).await;
        assert_eq!(changed.base.owner.id, s.company.stranger.id());
        canonical_owner(&s, made.base.id, s.company.stranger.id()).await;
        let unrelated = s
            .world
            .app
            .call(EnvironmentGet, &[&made.base.id])
            .token(&s.company.viewer.token)
            .send()
            .await
            .problem();
        assert_eq!(unrelated.code, ErrorCode::AuthzDenied);
        assert_eq!(unrelated.extra["required"], "environment.view");
        assert_eq!(missing_permissions(&unrelated), ["environment.view"]);
        let owner_memberships: i64 =
            sqlx::query_scalar("SELECT count(*) FROM org_members WHERE org_id=$1 AND user_id=$2")
                .bind(s.home)
                .bind(s.company.admin.id())
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        assert_eq!(
            owner_memberships, 0,
            "management does not invent membership"
        );
    }

    #[tokio::test]
    async fn machine_use_keeps_entry_provenance_and_refuses_a_registration_above_its_limit() {
        let s = scene().await;
        let entry = grant(&s, Some(1)).await;
        let made = create(&s, "first").await;
        // The existing creation-entry field supplies both quotas and withdrawal review.
        let used_entry: Option<Id> =
            sqlx::query_scalar("SELECT host_entry_id FROM environments WHERE id=$1")
                .bind(made.base.id)
                .fetch_one(&s.world.db.pool)
                .await
                .unwrap();
        assert_eq!(used_entry, Some(entry));
        let refused = s
            .world
            .app
            .call(MachineEnvironmentCreate, &[&s.machine])
            .token(&s.company.stranger.token)
            .body(&request(&s, "above-limit"))
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::NotFound);
        let registered: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM environments WHERE machine_id=$1 AND deleted_at IS NULL",
        )
        .bind(s.machine)
        .fetch_all(&s.world.db.pool)
        .await
        .unwrap();
        assert_eq!(registered, [made.base.id]);
    }

    #[tokio::test]
    async fn concurrent_registrations_cannot_both_take_the_last_machine_use_slot() {
        let s = scene().await;
        let entry = grant(&s, Some(2)).await;
        create(&s, "existing").await;
        let left_body = request(&s, "left");
        let right_body = request(&s, "right");
        let left = s
            .world
            .app
            .call(MachineEnvironmentCreate, &[&s.machine])
            .token(&s.company.stranger.token)
            .body(&left_body);
        let right = s
            .world
            .app
            .call(MachineEnvironmentCreate, &[&s.machine])
            .token(&s.company.stranger.token)
            .body(&right_body);
        let (left, right) = tokio::join!(left.send(), right.send());
        let successes = usize::from(left.body.is_ok()) + usize::from(right.body.is_ok());
        assert_eq!(
            successes, 1,
            "exactly one of the two creations owns the remaining slot"
        );
        let (refused, loser) = if left.body.is_ok() {
            (right.problem(), right_body)
        } else {
            (left.problem(), left_body)
        };
        assert!(matches!(
            refused.code,
            ErrorCode::NotFound | ErrorCode::ConflictStaleRevision
        ));
        // A retryable concurrent authority conflict is not quota success: a fresh
        // request must now hit the completed entry's ordinary refusal.
        let retried = s
            .world
            .app
            .call(MachineEnvironmentCreate, &[&s.machine])
            .token(&s.company.stranger.token)
            .body(&loser)
            .send()
            .await
            .problem();
        assert_eq!(retried.code, ErrorCode::NotFound);
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM environments WHERE host_entry_id=$1 AND deleted_at IS NULL",
        )
        .bind(entry)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn withdrawing_machine_use_stops_new_registration_without_rewriting_existing_owners() {
        let s = scene().await;
        let entry = grant(&s, None).await;
        let made = create(&s, "kept").await;
        s.world
            .app
            .call(PermissionPageUpdate, &[&"machines", &s.machine])
            .token(&s.company.admin.token)
            .body(&PageChange {
                rows: Some(vec![PageChangeRow {
                    permission: "machine.use".into(),
                    profile: None,
                    inherit: None,
                    locked: None,
                    add: None,
                    update: None,
                    remove: Some(vec![entry]),
                }]),
                ..PageChange::default()
            })
            .send()
            .await
            .ok();
        let refused = s
            .world
            .app
            .call(MachineEnvironmentCreate, &[&s.machine])
            .token(&s.company.stranger.token)
            .body(&request(&s, "after-withdrawal"))
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::NotFound);
        canonical_owner(&s, made.base.id, s.company.stranger.id()).await;
        machine_owner_can_manage(&s, made.base.id).await;
        canonical_owner(&s, made.base.id, s.company.stranger.id()).await;
    }

    #[tokio::test]
    async fn changing_environment_first_owner_preserves_the_machine_owners_management() {
        let s = scene().await;
        grant(&s, None).await;
        let made = create(&s, "transfer").await;
        places::join(
            &s.world,
            s.home,
            s.home,
            &s.company.viewer,
            "viewer",
            &s.company.stranger,
        )
        .await;
        let previous_entry: Id = sqlx::query_scalar(
            "SELECT id FROM permission_entries WHERE object_kind='environment' AND object_id=$1
             AND permission='access.own' AND page='live' AND holder_id=$2 AND removed_at IS NULL",
        )
        .bind(made.base.id)
        .bind(s.company.stranger.id())
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
        s.world
            .app
            .call(PermissionPageUpdate, &[&"environments", &made.base.id])
            .token(&s.company.stranger.token)
            .body(&PageChange {
                owners: Some(PageChangeOwners {
                    add: Some(vec![HolderInput::User {
                        id: s.company.viewer.id(),
                    }]),
                    remove: Some(vec![previous_entry]),
                    first: Some(s.company.viewer.id()),
                }),
                ..PageChange::default()
            })
            .send()
            .await
            .ok();
        canonical_owner(&s, made.base.id, s.company.viewer.id()).await;
        let shown = s
            .world
            .app
            .call(EnvironmentGet, &[&made.base.id])
            .token(&s.company.viewer.token)
            .send()
            .await
            .ok();
        assert!(
            shown
                .access
                .permissions
                .iter()
                .any(|p| p == "environment.manage")
        );
        machine_owner_can_manage(&s, made.base.id).await;
        canonical_owner(&s, made.base.id, s.company.viewer.id()).await;
    }
}
