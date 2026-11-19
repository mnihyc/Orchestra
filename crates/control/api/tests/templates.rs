//! Templates over HTTP: listing what the caller may use or manage, reading, making one for an org or for the system, changing, switching off and deleting one that nothing was made from.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    Template, TemplateCheck, TemplateCheckBody, TemplateCreate, TemplateCreateBody, TemplateDelete,
    TemplateGet, TemplateList, TemplateListQuery, TemplateUpdate, TemplateUpdateBody, UnitKind,
};
use orch_types::Id;
use places::{Who, admin, make_project, missing_permissions};
use resources::{company, name_person};
use serde_json::json;
use support::{Statements, World};

fn body(name: &str, home: Option<Id>, driver: &str) -> TemplateCreateBody {
    TemplateCreateBody {
        name: name.to_owned(),
        description: Some("A place to work".to_owned()),
        home_id: home,
        kind: None,
        driver: driver.to_owned(),
        spec: json!({ "image": "ubuntu:24.04", "cpus": 2 }),
        access: None,
    }
}

#[tokio::test]
async fn checking_a_configuration_returns_the_same_field_problems_as_saving_without_writing_anything()
 {
    let world = World::new().await;
    let company = company(&world).await;
    let mut conn = world.conn().await;
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(company.boss.id()),
        Some(company.acme),
        "preview-host",
    )
    .await
    .unwrap();
    let schema = json!({"type":"object","additionalProperties":false,"required":["image"],"properties":{
        "image":{"type":"string","minLength":1},
        "resources":{"type":"object","properties":{"memory_mb":{"type":"integer","minimum":1}}},
        "after_up":{"type":"array"},"plugins":{"type":"array"}
    }});
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1").bind(host).bind(json!({"preview-driver":{"name":"preview-driver","version":"1","spec_schemas":{"environment":schema,"provider":schema}}})).execute(&mut *conn).await.unwrap();
    drop(conn);
    let before: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM env_templates), (SELECT count(*) FROM audit_log)",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    let draft = TemplateCheckBody {
        driver: "preview-driver".into(),
        kind: UnitKind::Environment,
        spec: json!({"image":"", "resources":{"memory_mb":0}}),
    };
    let checked = world
        .app
        .call(TemplateCheck, &[])
        .token(&company.boss.token)
        .body(&draft)
        .send()
        .await
        .ok();
    assert!(!checked.valid);
    assert!(checked.errors.iter().any(|error| error.pointer == "/image"));
    assert!(
        checked
            .errors
            .iter()
            .any(|error| error.pointer == "/resources/memory_mb")
    );
    assert!(checked.errors.iter().all(|error| !error.message.is_empty()));
    let mut save = body("Draft only", Some(company.acme), "preview-driver");
    save.spec = draft.spec.clone();
    let refused = world
        .app
        .call(TemplateCreate, &[])
        .token(&company.boss.token)
        .body(&save)
        .send()
        .await
        .problem();
    for error in &checked.errors {
        assert!(
            refused.extra["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|fault| fault["path"] == format!("spec{}", error.pointer)
                    && fault["message"] == error.message)
        );
    }
    for kind in [UnitKind::Environment, UnitKind::Provider] {
        let accepted = TemplateCheckBody {
            kind,
            spec: json!({"image":"node:22-bookworm"}),
            ..draft.clone()
        };
        let reply = world
            .app
            .call(TemplateCheck, &[])
            .token(&company.boss.token)
            .body(&accepted)
            .send()
            .await
            .ok();
        assert!(reply.valid && reply.errors.is_empty());
    }
    let commands = TemplateCheckBody {
        spec: json!({"image":"node:22-bookworm","after_up":[{"name":"same","command":["first"]},{"name":"same","command":["second"]}]}),
        ..draft.clone()
    };
    let checked = world
        .app
        .call(TemplateCheck, &[])
        .token(&company.boss.token)
        .body(&commands)
        .send()
        .await
        .ok();
    assert!(!checked.valid);
    assert!(
        checked
            .errors
            .iter()
            .any(|error| error.pointer == "/after_up" && error.message.contains("unique"))
    );
    let nobody = world
        .app
        .call(TemplateCheck, &[])
        .body(&draft)
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    let after: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM env_templates), (SELECT count(*) FROM audit_log)",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        after, before,
        "neither checks nor a failed save create a template or an audit row"
    );
}

async fn make(world: &World, by: &Who, body: &TemplateCreateBody) -> Template {
    // These route tests use a reported driver double; save-time validation needs its declaration.
    let mut conn = world.conn().await;
    let reported = orch_db::queries::resources::binding::host_drivers(&mut conn)
        .await
        .unwrap();
    if !reported
        .iter()
        .any(|drivers| drivers.get(&body.driver).is_some())
    {
        let host = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "host",
            Some(by.id()),
            body.home_id,
            "schema-host",
        )
        .await
        .unwrap();
        sqlx::query("UPDATE hosts SET drivers=jsonb_build_object($2::text,$3::jsonb) WHERE id=$1")
            .bind(host).bind(&body.driver).bind(json!({"name":body.driver,"version":"0.0.0","spec_schemas":{"environment":{"type":"object"},"provider":{"type":"object"}}}))
            .execute(&mut *conn).await.unwrap();
    }
    drop(conn);
    world
        .app
        .call(TemplateCreate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
        .ok()
}

async fn list(world: &World, by: &Who, query: &TemplateListQuery) -> Vec<String> {
    world
        .app
        .call(TemplateList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
        .into_iter()
        .map(|template| match template {
            orch_api_contract::common::Listed::Hidden(row) => row.name,
            orch_api_contract::common::Listed::Visible(row) => row.content.name,
        })
        .collect()
}

fn rename(name: &str, revision: i64) -> TemplateUpdateBody {
    TemplateUpdateBody {
        name: Some(name.to_owned()),
        description: None,
        spec: None,
        enabled: None,
        expected_revision: revision,
    }
}

#[tokio::test]
async fn an_admin_of_an_org_makes_a_template_that_is_theirs_and_it_is_read_back() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.admin, &body("Ubuntu", Some(c.acme), "docker")).await;
    assert_eq!(made.name, "Ubuntu");
    assert_eq!(made.description, "A place to work");
    assert_eq!(
        made.kind,
        UnitKind::Environment,
        "environments when left out"
    );
    assert_eq!(made.driver, "docker");
    assert_eq!(made.spec["image"], "ubuntu:24.04");
    assert_eq!(made.address, "acme.admin.Ubuntu");
    assert_eq!(made.owner.as_ref().unwrap().username, "admin");
    assert_eq!(made.home.as_ref().unwrap().id, c.acme);
    assert_eq!(made.org_id, Some(c.acme));
    assert!(made.enabled);
    assert_eq!(made.units, 0);
    assert_eq!(made.revision, 1);
    assert!(
        made.access
            .permissions
            .iter()
            .any(|p| p == "template.manage")
    );
    let read = world
        .app
        .call(TemplateGet, &[&made.id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(read, made);
    let audited: Vec<(String, Option<Id>, Option<Id>)> = sqlx::query_as(
        "SELECT action, object_id, node_id FROM audit_log WHERE action = 'template.created'",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        vec![("template.created".to_owned(), Some(made.id), Some(c.acme))]
    );
    let nobody = world
        .app
        .call(TemplateGet, &[&made.id])
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_member_makes_a_template_where_the_org_lets_members_and_otherwise_whoever_makes_its_rules()
 {
    let world = World::new().await;
    let c = company(&world).await;
    // As shipped, any member makes one as their own; somebody who is no member does not.
    let member = make(&world, &c.viewer, &body("Member's", Some(c.acme), "docker")).await;
    assert_eq!(member.owner.as_ref().unwrap().username, "viewer");
    let refused = world
        .app
        .call(TemplateCreate, &[])
        .token(&c.stranger.token)
        .body(&body("Sneaky", Some(c.acme), "docker"))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // An org that leaves the kind out for its members keeps it to whoever makes its rules.
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "resources.created_by_members",
            json!(["provider"]),
        )
        .await;
    let kept = world
        .app
        .call(TemplateCreate, &[])
        .token(&c.viewer.token)
        .body(&body("Sneaky", Some(c.acme), "docker"))
        .send()
        .await
        .problem();
    assert_eq!(kept.code, ErrorCode::AuthzDenied);
    assert!(
        missing_permissions(&kept).contains(&"settings.manage".to_owned()),
        "{kept:?}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM env_templates WHERE name = 'Sneaky'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "nothing was stored by a refusal");
    let by_admin = make(&world, &c.admin, &body("Admin's", Some(c.acme), "docker")).await;
    assert_eq!(by_admin.owner.as_ref().unwrap().username, "admin");
}

#[tokio::test]
async fn a_template_with_no_home_is_the_systems_and_only_a_system_admin_makes_it() {
    let world = World::new().await;
    let c = company(&world).await;
    let root = admin(&world, "root").await;
    let refused = world
        .app
        .call(TemplateCreate, &[])
        .token(&c.boss.token)
        .body(&body("Everyone's", None, "docker"))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "system.admin");
    let made = make(&world, &root, &body("Everyone's", None, "docker")).await;
    assert!(made.home.is_none() && made.org_id.is_none());
    assert_eq!(made.owner.as_ref().unwrap().username, "root");
    assert_eq!(made.address, "system.Everyone's");
    let audited: Option<Option<Id>> = sqlx::query_scalar(
        "SELECT node_id FROM audit_log WHERE action = 'template.created' AND object_id = $1",
    )
    .bind(made.id)
    .fetch_optional(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        Some(None),
        "an act of the system belongs to no org"
    );
}

#[tokio::test]
async fn a_driver_the_system_keeps_to_its_admins_is_saved_by_them_alone() {
    let world = World::new().await;
    let c = company(&world).await;
    let root = admin(&world, "root").await;
    // What they make is theirs only in an org they belong to.
    places::join(&world, c.acme, c.acme, &root, "viewer", &c.boss).await;
    // The shipped setting keeps the `host` driver to the admins of the system.
    let refused = world
        .app
        .call(TemplateCreate, &[])
        .token(&c.boss.token)
        .body(&body("On the host", Some(c.acme), "host"))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "system.admin");
    let made = make(&world, &root, &body("On the host", Some(c.acme), "host")).await;
    assert_eq!(made.driver, "host");
    assert_eq!(made.home.as_ref().unwrap().id, c.acme);
    // Where the system lists none, the org's admin saves it too.
    world.set("templates.restricted_drivers", json!([])).await;
    let then = make(&world, &c.boss, &body("Open now", Some(c.acme), "host")).await;
    assert_eq!(then.driver, "host");
}

#[tokio::test]
async fn what_is_wrong_with_a_new_template_is_told_member_by_member() {
    let world = World::new().await;
    let c = company(&world).await;
    let project = make_project(&world, &c.boss, c.acme, "Web").await;
    let bad = |edit: &dyn Fn(&mut TemplateCreateBody)| {
        let mut each = body("Fine", Some(c.acme), "docker");
        edit(&mut each);
        each
    };
    let cases: Vec<(TemplateCreateBody, &str)> = vec![
        (bad(&|each| each.name = "  ".to_owned()), "name"),
        (bad(&|each| each.name = "n".repeat(81)), "name"),
        (bad(&|each| each.driver = "Docker Inc".to_owned()), "driver"),
        (bad(&|each| each.driver = String::new()), "driver"),
        (bad(&|each| each.spec = json!(["a list"])), "spec"),
        (bad(&|each| each.home_id = Some(project.base.id)), "home_id"),
    ];
    for (each, path) in cases {
        let refused = world
            .app
            .call(TemplateCreate, &[])
            .token(&c.boss.token)
            .body(&each)
            .send()
            .await
            .problem();
        assert_eq!(
            refused.code,
            ErrorCode::RequestInvalid,
            "{path}: {refused:?}"
        );
        assert_eq!(refused.extra["errors"][0]["path"], path);
    }
    let gone = body("Fine", Some(orch_types::new_id()), "docker");
    let missing = world
        .app
        .call(TemplateCreate, &[])
        .token(&c.boss.token)
        .body(&gone)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_template_is_listed_for_whoever_may_use_or_manage_it_and_narrows_to_an_org_and_a_kind() {
    let world = World::new().await;
    let c = company(&world).await;
    let root = admin(&world, "root").await;
    let environment = make(&world, &c.boss, &body("Ubuntu", Some(c.acme), "docker")).await;
    let mut for_providers = body("Runtime", Some(c.acme), "docker");
    for_providers.kind = Some(UnitKind::Provider);
    let runtime = make(&world, &c.boss, &for_providers).await;
    let systems = make(&world, &root, &body("Shared", None, "docker")).await;
    let all = TemplateListQuery::default();
    let sorted = |mut names: Vec<String>| {
        names.sort();
        names
    };
    // Whoever makes one manages it; nobody else is offered it until its page names them.
    assert_eq!(
        sorted(list(&world, &c.boss, &all).await),
        ["Runtime", "Ubuntu"]
    );
    // A system admin with As admin on holds View and Manage on every template natively.
    assert_eq!(
        sorted(list(&world, &root, &all).await),
        ["Runtime", "Shared", "Ubuntu"]
    );
    assert!(list(&world, &c.stranger, &all).await.is_empty());
    assert!(list(&world, &c.viewer, &all).await.is_empty());
    name_person(
        &world,
        "template",
        environment.id,
        "template.use",
        c.viewer.id(),
    )
    .await;
    assert_eq!(list(&world, &c.viewer, &all).await, ["Ubuntu"]);
    // Whoever may use a template may read it: any permission on it is enough.
    let read = world
        .app
        .call(TemplateGet, &[&environment.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(read.name, "Ubuntu");
    name_person(&world, "template", systems.id, "template.use", c.boss.id()).await;
    assert_eq!(
        sorted(list(&world, &c.boss, &all).await),
        ["Runtime", "Shared", "Ubuntu"]
    );
    // Narrowing: by the pool a template is for, and by org, which keeps the system's beside the org's.
    let providers = TemplateListQuery {
        kind: Some(UnitKind::Provider),
        ..TemplateListQuery::default()
    };
    assert_eq!(list(&world, &c.boss, &providers).await, ["Runtime"]);
    let environments = TemplateListQuery {
        kind: Some(UnitKind::Environment),
        ..TemplateListQuery::default()
    };
    assert_eq!(
        sorted(list(&world, &c.boss, &environments).await),
        ["Shared", "Ubuntu"]
    );
    let in_org = TemplateListQuery {
        org_id: Some(c.acme),
        ..TemplateListQuery::default()
    };
    assert_eq!(
        sorted(list(&world, &c.boss, &in_org).await),
        ["Runtime", "Shared", "Ubuntu"]
    );
    let other = places::make_org(&world, &c.stranger, "Other", "other").await;
    let elsewhere = TemplateListQuery {
        org_id: Some(other.base.id),
        ..TemplateListQuery::default()
    };
    assert_eq!(
        list(&world, &c.boss, &elsewhere).await,
        ["Shared"],
        "the system's are in every org, and nothing of Acme's is in another"
    );
    let nobody = world.app.call(TemplateList, &[]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    let _ = runtime;
}

#[tokio::test]
async fn a_list_of_templates_costs_the_same_statements_for_a_few_and_for_many() {
    let world = World::new().await;
    let c = company(&world).await;
    for index in 0..2 {
        make(
            &world,
            &c.boss,
            &body(&format!("First {index}"), Some(c.acme), "docker"),
        )
        .await;
    }
    async fn measure(world: &World, who: &Who) -> (usize, usize) {
        let call = world
            .app
            .call(TemplateList, &[])
            .token(&who.token)
            .query(&TemplateListQuery::default());
        let before = Statements::now();
        let listed = call.send().await.ok();
        (Statements::now() - before, listed.len())
    }
    let (few, few_count) = measure(&world, &c.boss).await;
    for index in 0..10 {
        make(
            &world,
            &c.boss,
            &body(&format!("More {index}"), Some(c.acme), "docker"),
        )
        .await;
    }
    let (many, many_count) = measure(&world, &c.boss).await;
    assert_eq!((few_count, many_count), (2, 12));
    assert_eq!(few, many, "{few} statements for 2 templates, {many} for 12");
}

#[tokio::test]
async fn a_template_is_read_by_whoever_holds_any_permission_on_it_and_by_nobody_else() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.admin, &body("Ubuntu", Some(c.acme), "docker")).await;
    let id = made.id;
    let refused = world
        .app
        .call(TemplateGet, &[&id])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "template.view");
    assert_eq!(
        missing_permissions(&refused),
        ["template.view"],
        "{refused:?}"
    );
    name_person(&world, "template", id, "template.view", c.viewer.id()).await;
    let looked = world
        .app
        .call(TemplateGet, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(looked.access.permissions, ["template.view"]);
    assert_eq!(looked.spec, made.spec);
    // Only to look at: it is among the templates they hold something on, with nothing to use.
    assert!(
        list(&world, &c.viewer, &TemplateListQuery::default())
            .await
            .contains(&"Ubuntu".to_owned())
    );
    let unknown = world
        .app
        .call(TemplateGet, &[&orch_types::new_id()])
        .token(&c.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_template_is_changed_by_whoever_manages_it_and_not_by_one_who_only_uses_it() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.boss, &body("Ubuntu", Some(c.acme), "docker")).await;
    let id = made.id;
    name_person(&world, "template", id, "template.use", c.viewer.id()).await;
    let refused = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.viewer.token)
        .body(&rename("Mine", 1))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "template.manage");
    assert_eq!(missing_permissions(&refused), ["template.manage"]);
    // An admin of the home administers its page; the template is managed once the page says so.
    let admins = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.admin.token)
        .body(&rename("Admin's", 1))
        .send()
        .await
        .problem();
    assert_eq!(admins.code, ErrorCode::AuthzDenied);
    let unchanged = world
        .app
        .call(TemplateGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(unchanged.name, "Ubuntu");
    name_person(&world, "template", id, "template.manage", c.admin.id()).await;
    let changed = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.admin.token)
        .body(&TemplateUpdateBody {
            name: Some("  Ubuntu LTS ".to_owned()),
            description: Some("Long term".to_owned()),
            spec: Some(json!({ "image": "ubuntu:26.04" })),
            enabled: None,
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    assert_eq!(changed.name, "Ubuntu LTS");
    assert_eq!(changed.description, "Long term");
    assert_eq!(changed.spec, json!({ "image": "ubuntu:26.04" }));
    assert_eq!(
        (changed.kind, changed.driver.as_str()),
        (made.kind, "docker")
    );
    assert_eq!(changed.revision, 2);
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action LIKE 'template.%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audited, ["template.created", "template.updated"]);
}

#[tokio::test]
async fn a_template_is_switched_off_and_on_and_a_change_against_an_old_revision_is_refused() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.boss, &body("Ubuntu", Some(c.acme), "docker")).await;
    let id = made.id;
    let off = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.boss.token)
        .body(&TemplateUpdateBody {
            enabled: Some(false),
            ..rename("Ubuntu", 1)
        })
        .send()
        .await
        .ok();
    assert!(!off.enabled);
    assert_eq!(off.revision, 2);
    let stale = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.boss.token)
        .body(&rename("Late", 1))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["name"], "Ubuntu");
    assert_eq!(stale.extra["current"]["revision"], 2);
    // A change that asks for nothing is the template as it is, and is stale just the same.
    let nothing = |revision: i64| TemplateUpdateBody {
        name: None,
        ..rename("", revision)
    };
    let same = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.boss.token)
        .body(&nothing(2))
        .send()
        .await
        .ok();
    assert_eq!(same.revision, 2, "nothing asked, nothing changed");
    let old = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.boss.token)
        .body(&nothing(1))
        .send()
        .await
        .problem();
    assert_eq!(old.code, ErrorCode::ConflictStaleRevision);
    let on = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.boss.token)
        .body(&TemplateUpdateBody {
            enabled: Some(true),
            ..nothing(2)
        })
        .send()
        .await
        .ok();
    assert!(on.enabled);
    assert_eq!(on.revision, 3);
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action LIKE 'template.%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        ["template.created", "template.disabled", "template.enabled"]
    );
    // What is wrong with a change is told member by member, and nothing is stored.
    let bad_name = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.boss.token)
        .body(&rename(" ", 3))
        .send()
        .await
        .problem();
    assert_eq!(bad_name.code, ErrorCode::RequestInvalid);
    assert_eq!(bad_name.extra["errors"][0]["path"], "name");
    let bad_spec = world
        .app
        .call(TemplateUpdate, &[&id])
        .token(&c.boss.token)
        .body(&TemplateUpdateBody {
            spec: Some(json!("text")),
            ..nothing(3)
        })
        .send()
        .await
        .problem();
    assert_eq!(bad_spec.extra["errors"][0]["path"], "spec");
}

#[tokio::test]
async fn a_template_that_units_were_made_from_is_kept_and_one_that_nothing_uses_is_deleted() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = make(&world, &c.boss, &body("Ubuntu", Some(c.acme), "docker")).await;
    let id = made.id;
    // An environment made from it stands.
    let environment = orch_types::new_id();
    sqlx::query(
        "INSERT INTO environments (id, name, owner_id, org_id, home_node_id, placement, template_id)
         VALUES ($1, 'From it', $2, $3, $3, 'managed', $4)",
    )
    .bind(environment)
    .bind(c.boss.id())
    .bind(c.acme)
    .bind(id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let kept = world
        .app
        .call(TemplateDelete, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(kept.code, ErrorCode::ResourceInUse);
    assert_eq!(kept.extra["units"], 1);
    let still = world
        .app
        .call(TemplateGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(still.units, 1);
    // The unit goes; the template can follow, for its manager only.
    sqlx::query("UPDATE environments SET deleted_at = now() WHERE id = $1")
        .bind(environment)
        .execute(&world.db.pool)
        .await
        .unwrap();
    name_person(&world, "template", id, "template.use", c.viewer.id()).await;
    let refused = world
        .app
        .call(TemplateDelete, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "template.manage");
    world
        .app
        .call(TemplateDelete, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    let gone = world
        .app
        .call(TemplateGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
    assert!(
        list(&world, &c.boss, &TemplateListQuery::default())
            .await
            .is_empty()
    );
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log WHERE action LIKE 'template.%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audited, ["template.created", "template.deleted"]);
}

#[tokio::test]
async fn who_may_use_a_new_template_is_the_makers_choice_and_the_defaults_of_the_home_when_left_out()
 {
    let world = World::new().await;
    let c = company(&world).await;
    let mut only_me = body("Private", Some(c.acme), "docker");
    only_me.access = Some(NewResourceAccess::OnlyMe);
    make(&world, &c.boss, &only_me).await;
    assert!(
        list(&world, &c.viewer, &TemplateListQuery::default())
            .await
            .is_empty()
    );
    let mut home = body("Shared with the org", Some(c.acme), "docker");
    home.access = Some(NewResourceAccess::Home);
    make(&world, &c.boss, &home).await;
    assert_eq!(
        list(&world, &c.viewer, &TemplateListQuery::default()).await,
        ["Shared with the org"]
    );
    assert!(
        list(&world, &c.stranger, &TemplateListQuery::default())
            .await
            .is_empty()
    );
}
