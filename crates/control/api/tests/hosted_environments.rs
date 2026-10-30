//! Environments made from a template on a host, over HTTP: where one could be made, making one and the job that makes it, a topic's own, starting and stopping, keeping awake and keeping from removal, the fleet of a place, and what a host's manager sees on it and takes away.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod rig;
mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::binding::NewEnvironment;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{
    Environment, EnvironmentCreate, EnvironmentDelete, EnvironmentDeleteQuery, EnvironmentGet,
    EnvironmentKeep, EnvironmentKeepAwake, EnvironmentStart, EnvironmentStatus, EnvironmentStop,
    EnvironmentStopBody, HostReason, NodeEnvironmentList, NodeEnvironmentListQuery, Place,
};
use orch_api_contract::nodes::JobState;
use orch_api_contract::resources::{
    HostRuntimeDelete, HostRuntimeDeleteQuery, HostRuntimeList, HostRuntimeListQuery, UnitKind,
};
use orch_api_contract::topics::{
    TopicDefaultEnvironmentSet, TopicDefaultEnvironmentSetBody, TopicEnvironmentList,
    TopicPlaceList,
};
use orch_api_contract::workspace::{Job, JobCancel, JobGet};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::{Id, Time};
use places::{Who, make_project, make_topic, missing_permissions};
use resources::{Company, company};
use serde_json::json;
use support::World;

/// What a team has to make environments with: an org with its people, a topic, a host and a template
/// that the admin owns.
struct Team {
    c: Company,
    topic: Id,
    host: Id,
    template: Id,
}

/// A host named `name`, at home in `home`, owned by `owner`, that reports a Docker driver and `memory_mb`.
async fn a_host(world: &World, owner: &Who, home: Id, name: &str, memory_mb: i64) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(&mut conn, "host", Some(owner.id()), Some(home), name)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id,
        },
        owner.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE hosts SET drivers = $2, capacity = $3, storages = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(json!({ "docker": {
        "name": "docker", "title": "Docker",
        "spec_schemas": { "environment": {}, "provider": {} },
        "runtime_classes": ["runc"], "sources": ["image", "build", "archive"]
    }}))
    .bind(json!({ "cpus": 8, "memory_mb": memory_mb, "disk_gb": 500 }))
    .bind(json!([{ "name": "fast", "default": true, "capacity_gb": 500, "free_gb": 300, "private_views": false }]))
    .execute(&mut *conn)
    .await
    .unwrap();
    // Its node holds a session, and has reported the drivers its operator lets it offer.
    sqlx::query(
        "UPDATE machines SET online = true,
                facts = jsonb_set(facts, '{policy}', COALESCE(facts -> 'policy', '{}') || $2)
          WHERE id = (SELECT machine_id FROM hosts WHERE id = $1)",
    )
    .bind(id)
    .bind(json!({
        "drivers": serde_json::to_value(orch_link_contract::policy::MachineDrivers::managed())
            .unwrap()
    }))
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

/// A template of environments that a Docker host makes from a small image.
async fn a_template(world: &World, owner: &Who, home: Id, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(&mut conn, "template", Some(owner.id()), Some(home), name)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Template.into(),
            id,
        },
        owner.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE env_templates SET driver = 'docker', spec = $2 WHERE id = $1")
        .bind(id)
        .bind(json!({
            "image": "busybox:latest", "runtime": "runc",
            "user": { "name": "agent", "uid": 1000, "gid": 1000 },
            "resources": { "cpu_millis": 2000, "memory_mb": 2048, "pids": 512, "disk_gb": 20 },
            "needs": { "cpu_millis": 500, "memory_mb": 1024, "disk_gb": 5 },
            "exec": { "offers": ["orch-exec"], "orch_tools": true },
            "colocated_harnesses": [], "host_labels": [], "init": null
        }))
        .execute(&mut *conn)
        .await
        .unwrap();
    id
}

async fn team(world: &World) -> Team {
    let c = company(world).await;
    let project = make_project(world, &c.boss, c.acme, "Web").await;
    let topic = make_topic(world, &c.boss, project.base.id, "Login")
        .await
        .base
        .id;
    let host = a_host(world, &c.admin, c.acme, "Build box", 16_384).await;
    let template = a_template(world, &c.admin, c.acme, "Small").await;
    Team {
        c,
        topic,
        host,
        template,
    }
}

fn wanted(team: &Team, name: &str) -> NewEnvironment {
    NewEnvironment {
        name: Some(name.to_owned()),
        home_node_id: Some(team.topic),
        template_id: Some(team.template),
        ..NewEnvironment::default()
    }
}

/// A reported forwarded harness: the template's executor promise is checked again at actual admission.
async fn start_binding(world: &World, t: &Team) -> orch_api_contract::nodes::CreateThreadBinding {
    let owner = &t.c.admin;
    let mut conn = world.conn().await;
    let agent = fixtures::insert_resource(
        &mut conn,
        "agent",
        Some(owner.id()),
        Some(t.c.acme),
        "Atomic agent",
    )
    .await
    .unwrap();
    let provider = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(owner.id()),
        Some(t.c.acme),
        "Atomic provider",
    )
    .await
    .unwrap();
    for (kind, id) in [
        (ResourceKind::Agent, agent),
        (ResourceKind::Provider, provider),
    ] {
        start_resource(
            &mut conn,
            ObjectRef {
                kind: kind.into(),
                id,
            },
            owner.id(),
            NewAccess::Defaults,
            Time::new(world.now()),
        )
        .await
        .unwrap();
    }
    sqlx::query("UPDATE agents SET harness_kind='fake',descriptor=$2 WHERE id=$1")
        .bind(agent)
        .bind(rig::descriptor())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET kind='api',harness_kind=NULL,agent_id=NULL,machine_id=NULL,key_machine_id=NULL,protocols=ARRAY['openai-responses'],status='ready' WHERE id=$1")
        .bind(provider).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES($1,$2,'atomic-1','Atomic model','{}',true,'manual',$3)")
        .bind(orch_types::new_id()).bind(provider).bind(owner.id()).execute(&mut *conn).await.unwrap();
    drop(conn);
    world
        .set_at(
            orch_settings::Target::Node(t.topic),
            "env.template_id",
            json!(t.template),
        )
        .await;
    orch_api_contract::nodes::CreateThreadBinding {
        agent_id: agent,
        provider_id: provider,
        arrangement: Some(orch_api_contract::binding::ArrangementType::Separated),
    }
}

async fn creation_counts(world: &World) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM environments),(SELECT count(*) FROM jobs),(SELECT count(*) FROM threads),(SELECT count(*) FROM nodes WHERE kind='topic')")
        .fetch_one(&world.db.pool).await.unwrap()
}

#[tokio::test]
async fn a_new_environment_and_its_thread_are_accepted_together_before_executor_facts_exist() {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, ThreadCreate};
    let world = World::new().await;
    let t = team(&world).await;
    let binding = start_binding(&world, &t).await;
    let mut environment = wanted(&t, "Atomic workspace");
    environment.config = Some(json!({"env.lifetime":"thread"}));
    let made = world
        .app
        .call(ThreadCreate, &[&t.topic])
        .token(&t.c.admin.token)
        .body(&CreateThread {
            binding: Some(binding.clone()),
            model: Some("atomic-1".into()),
            environment: Some(EnvironmentChoice::New(Box::new(environment))),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let workspace = made.environment.as_ref().unwrap();
    assert_eq!(made.binding.as_ref().unwrap().agent.id, binding.agent_id);
    assert_eq!(
        made.binding.as_ref().unwrap().provider.id,
        binding.provider_id
    );
    let (facts, creator): (serde_json::Value, Option<Id>) = sqlx::query_as("SELECT e.facts,l.thread_id FROM environments e JOIN environment_lifetimes l ON l.environment_id=e.id WHERE e.id=$1")
        .bind(workspace.id).fetch_one(&world.db.pool).await.unwrap();
    assert!(
        facts.get("executors").is_none(),
        "admission uses the declared template, not nonexistent live facts"
    );
    assert_eq!(creator, Some(made.base.id));
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id=$1")
        .bind(workspace.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(
        jobs > 0,
        "the accepted workspace has its durable provisioning job"
    );
}

#[tokio::test]
async fn a_template_change_after_preview_refuses_the_whole_start_without_an_environment_or_job() {
    use orch_api_contract::binding::{
        EnvironmentChoice, NodeBindingOptionsGet, NodeBindingOptionsGetQuery,
    };
    use orch_api_contract::nodes::{CreateThread, ThreadCreate};
    let world = World::new().await;
    let t = team(&world).await;
    let binding = start_binding(&world, &t).await;
    let preview = world
        .app
        .call(NodeBindingOptionsGet, &[&t.topic])
        .token(&t.c.admin.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_new: true,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        preview
            .providers
            .iter()
            .find(|row| row.provider.id == binding.provider_id)
            .unwrap()
            .agents
            .iter()
            .find(|row| row.agent.id == binding.agent_id)
            .unwrap()
            .usable
    );
    sqlx::query("UPDATE env_templates SET spec=jsonb_set(spec,'{exec,offers}','[]') WHERE id=$1")
        .bind(t.template)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let before = creation_counts(&world).await;
    let refused = world
        .app
        .call(ThreadCreate, &[&t.topic])
        .token(&t.c.admin.token)
        .body(&CreateThread {
            binding: Some(binding),
            model: Some("atomic-1".into()),
            environment: Some(EnvironmentChoice::New(Box::new(wanted(
                &t,
                "Must not remain",
            )))),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::BindingIncompatible);
    assert_eq!(refused.extra["reason"], json!("executor_missing"));
    assert_eq!(creation_counts(&world).await, before);
}

#[tokio::test]
async fn an_invalid_model_or_directory_rolls_back_the_environment_and_its_provisioning_job() {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, ThreadCreate};
    let world = World::new().await;
    let t = team(&world).await;
    let binding = start_binding(&world, &t).await;
    let before = creation_counts(&world).await;
    for bad_directory in [false, true] {
        let refused = world
            .app
            .call(ThreadCreate, &[&t.topic])
            .token(&t.c.admin.token)
            .body(&CreateThread {
                binding: Some(binding.clone()),
                model: Some(
                    if bad_directory {
                        "atomic-1"
                    } else {
                        "not-a-model"
                    }
                    .into(),
                ),
                directory: bad_directory.then(|| "../outside".to_owned()),
                environment: Some(EnvironmentChoice::New(Box::new(wanted(
                    &t,
                    "Must not remain",
                )))),
                ..Default::default()
            })
            .send()
            .await
            .problem();
        assert_eq!(
            refused.code,
            if bad_directory {
                ErrorCode::RequestInvalid
            } else {
                ErrorCode::ModelNotAllowed
            }
        );
        assert_eq!(creation_counts(&world).await, before);
    }
}

#[tokio::test]
async fn an_invalid_first_message_rolls_back_the_new_environment_thread_and_jobs() {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, FirstInput, ThreadCreate};
    let world = World::new().await;
    let t = team(&world).await;
    let binding = start_binding(&world, &t).await;
    let before = creation_counts(&world).await;
    let refused = world
        .app
        .call(ThreadCreate, &[&t.topic])
        .token(&t.c.admin.token)
        .body(&CreateThread {
            binding: Some(binding),
            model: Some("atomic-1".into()),
            environment: Some(EnvironmentChoice::New(Box::new(wanted(
                &t,
                "Must not remain",
            )))),
            first_input: Some(FirstInput {
                content: vec![],
                client_id: "empty-first".into(),
            }),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(creation_counts(&world).await, before);
}

#[tokio::test]
async fn a_project_admits_its_topic_environment_and_first_thread_in_the_same_action() {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, TopicCreate, TopicCreateBody, TopicCreateReply};
    let world = World::new().await;
    let t = team(&world).await;
    let binding = start_binding(&world, &t).await;
    let project: Id = sqlx::query_scalar("SELECT parent_id FROM nodes WHERE id=$1")
        .bind(t.topic)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let mut environment = wanted(&t, "Project start");
    environment.home_node_id = None;
    let reply = world
        .app
        .call(TopicCreate, &[&project])
        .token(&t.c.admin.token)
        .body(&TopicCreateBody {
            first_thread: Some(CreateThread {
                binding: Some(binding.clone()),
                model: Some("atomic-1".into()),
                environment: Some(EnvironmentChoice::New(Box::new(environment))),
                ..Default::default()
            }),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let TopicCreateReply::WithThread(made) = reply else {
        panic!("the accepted topic must carry its first thread")
    };
    assert_eq!(made.topic.base.parent_id, Some(project));
    assert_eq!(made.thread.topic_id, made.topic.base.id);
    assert_eq!(
        made.thread.binding.as_ref().unwrap().agent.id,
        binding.agent_id
    );
    let workspace = made.thread.environment.as_ref().unwrap();
    let home: Id = sqlx::query_scalar("SELECT home_node_id FROM environments WHERE id=$1")
        .bind(workspace.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        home, made.topic.base.id,
        "an automatic workspace belongs to the actual admitted topic"
    );
}

#[tokio::test]
async fn a_refused_first_thread_leaves_no_project_topic_environment_or_job() {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, TopicCreate, TopicCreateBody};
    let world = World::new().await;
    let t = team(&world).await;
    let binding = start_binding(&world, &t).await;
    let project: Id = sqlx::query_scalar("SELECT parent_id FROM nodes WHERE id=$1")
        .bind(t.topic)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let mut environment = wanted(&t, "Must not remain");
    environment.home_node_id = None;
    let before = creation_counts(&world).await;
    let refused = world
        .app
        .call(TopicCreate, &[&project])
        .token(&t.c.admin.token)
        .body(&TopicCreateBody {
            first_thread: Some(CreateThread {
                binding: Some(binding),
                model: Some("not-a-model".into()),
                environment: Some(EnvironmentChoice::New(Box::new(environment))),
                ..Default::default()
            }),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ModelNotAllowed);
    assert_eq!(
        creation_counts(&world).await,
        before,
        "even the enclosing topic is rolled back"
    );
}

#[tokio::test]
async fn an_explicit_empty_live_executor_report_overrules_the_templates_initial_promise() {
    use orch_api_contract::binding::{
        EnvironmentChoice, NodeBindingOptionsGet, NodeBindingOptionsGetQuery,
    };
    use orch_api_contract::nodes::{CreateThread, ThreadCreate};
    let world = World::new().await;
    let t = team(&world).await;
    let binding = start_binding(&world, &t).await;
    let environment = make(&world, &t, &t.c.admin, "Reported empty").await;
    sqlx::query("UPDATE environments SET facts=$2 WHERE id=$1")
        .bind(environment.base.id)
        .bind(json!({"executors":[]}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let options = world
        .app
        .call(NodeBindingOptionsGet, &[&t.topic])
        .token(&t.c.admin.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_id: Some(environment.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let agent = options
        .providers
        .iter()
        .find(|row| row.provider.id == binding.provider_id)
        .unwrap()
        .agents
        .iter()
        .find(|row| row.agent.id == binding.agent_id)
        .unwrap();
    assert!(!agent.usable);
    assert!(
        agent
            .arrangements
            .iter()
            .all(|option| !option.available || !option.permitted)
    );
    world
        .set_at(
            orch_settings::Target::Node(t.topic),
            "env.default_id",
            json!(environment.base.id),
        )
        .await;
    let fresh = world
        .app
        .call(NodeBindingOptionsGet, &[&t.topic])
        .token(&t.c.admin.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_new: true,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        fresh
            .providers
            .iter()
            .find(|row| row.provider.id == binding.provider_id)
            .unwrap()
            .agents
            .iter()
            .find(|row| row.agent.id == binding.agent_id)
            .unwrap()
            .usable,
        "explicit New judges the template, not the existing shared default's live empty report"
    );
    let ambiguous = world
        .app
        .call(NodeBindingOptionsGet, &[&t.topic])
        .token(&t.c.admin.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_new: true,
            environment_id: Some(environment.base.id),
        })
        .send()
        .await
        .problem();
    assert_eq!(ambiguous.code, ErrorCode::RequestInvalid);
    let before = creation_counts(&world).await;
    let refused = world
        .app
        .call(ThreadCreate, &[&t.topic])
        .token(&t.c.admin.token)
        .body(&CreateThread {
            binding: Some(binding),
            model: Some("atomic-1".into()),
            environment: Some(EnvironmentChoice::Use(environment.base.id)),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::BindingIncompatible);
    assert_eq!(refused.extra["reason"], json!("executor_missing"));
    assert_eq!(creation_counts(&world).await, before);
}

#[tokio::test]
async fn temporary_creation_reports_the_places_idle_time_and_keep_makes_it_lasting_in_one_reply() {
    let world = World::new().await;
    let t = team(&world).await;
    let mut request = wanted(&t, "Disposable work");
    request.config = Some(json!({"env.lifetime":"temporary"}));
    let made = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(made.home.id, t.topic);
    assert_eq!(made.config["env.lifetime"], json!("temporary"));
    assert_eq!(made.temporary.as_ref().unwrap().idle_hours, 24);
    sqlx::query("UPDATE jobs SET state='cancelled',finished_at=$2 WHERE environment_id=$1")
        .bind(made.base.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let shown = world
        .app
        .call(EnvironmentGet, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(
        shown.temporary.unwrap().expires_at,
        Some(Time::new(world.now() + chrono::Duration::hours(24)))
    );
    let denied = world
        .app
        .call(EnvironmentKeep, &[&made.base.id])
        .token(&t.c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let kept = world
        .app
        .call(EnvironmentKeep, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert!(kept.temporary.is_none() && kept.remove_at.is_none());
    assert_eq!(kept.config["env.lifetime"], json!("lasting"));
    let again = world
        .app
        .call(EnvironmentKeep, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(again.revision, kept.revision, "Keep is safe to retry");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM environment_lifetimes WHERE environment_id=$1")
            .bind(made.base.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    world
        .set_at(
            orch_settings::Target::Node(t.topic),
            "env.temporary_idle_hours",
            json!(6),
        )
        .await;
    request.name = Some("Six hour work".into());
    let shorter = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(shorter.temporary.unwrap().idle_hours, 6);
}

#[tokio::test]
async fn creation_in_an_explicit_place_works_with_multiple_orgs_and_a_missing_place_is_a_field_fault()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let mut request = wanted(&t, "Placed work");
    request.home_node_id = None;
    let refused = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&request)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(
        refused.extra["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fault| fault["path"] == "home_node_id")
    );
    request.home_node_id = Some(t.topic);
    request.host_id = Some(Some(t.host));
    let made = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(made.home.id, t.topic);
    assert_eq!(made.template.unwrap().id, t.template);
    // The host it was asked for is its own value; it is shown as its host once its unit has volumes
    // there.
    assert_eq!(made.config["env.host_id"], json!(t.host));
    assert!(made.host.is_none());
}

#[tokio::test]
async fn keep_cannot_turn_a_failed_but_already_started_removal_into_a_false_promise() {
    let world = World::new().await;
    let t = team(&world).await;
    let mut request = wanted(&t, "Removal already began");
    request.config = Some(json!({"env.lifetime":"temporary"}));
    let made = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&request)
        .send()
        .await
        .ok();
    sqlx::query("UPDATE jobs SET state='failed',finished_at=$2 WHERE environment_id=$1")
        .bind(made.base.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET removal_phase='removing',remove_at=$2 WHERE id=$1")
        .bind(made.base.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let refused = world
        .app
        .call(EnvironmentKeep, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::PlanBlocked);
    let config: serde_json::Value =
        sqlx::query_scalar("SELECT config FROM environments WHERE id=$1")
            .bind(made.base.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(config["env.lifetime"], json!("temporary"));
}

async fn initial_source(world: &World, t: &Team, foreign: bool) -> Id {
    let owner = if foreign { &t.c.stranger } else { &t.c.admin };
    let topic = if foreign {
        let project = make_project(world, owner, owner.person.personal_org, "Other files").await;
        make_topic(world, owner, project.base.id, "Other source")
            .await
            .base
            .id
    } else {
        t.topic
    };
    let thread = orch_types::new_id();
    let mut conn = world.conn().await;
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        thread,
        "thread",
        Some(topic),
        "Source",
        owner.id(),
    )
    .await
    .unwrap();
    let environment = orch_db::queries::hierarchy::testing::put_thread_in_environment(
        &mut conn,
        thread,
        "connected",
        owner.id(),
        topic,
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: environment,
        },
        owner.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET status='running',workspace_root='/work',machine_id=(SELECT machine_id FROM hosts WHERE id=$2),generation=1,facts='{}' WHERE id=$1")
        .bind(environment).bind(t.host).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE threads SET workspace_dir='selected' WHERE node_id=$1")
        .bind(thread)
        .execute(&mut *conn)
        .await
        .unwrap();
    thread
}

#[tokio::test]
async fn initial_files_accept_a_readable_selection_and_refuse_traversal_or_foreign_references_before_creation()
 {
    use orch_api_contract::{binding::NewEnvironmentFiles, forks::FileSelection};
    let world = World::new().await;
    let t = team(&world).await;
    let source = initial_source(&world, &t, false).await;
    let mut request = wanted(&t, "Selected files");
    request.files = Some(NewEnvironmentFiles::FromThread {
        from_thread: source,
        selection: Some(FileSelection::Selection {
            include: vec!["uploads/report.txt".into(), "shares/reference.md".into()],
            exclude: Some(vec!["shares/cache".into()]),
        }),
    });
    let made = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(made.base.status, EnvironmentStatus::Provisioning);
    let row = orch_db::queries::jobs::job(&mut *world.conn().await, made.job.unwrap().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.plan["initial_files"]["thread"], json!(source));
    assert_eq!(
        row.plan["initial_files"]["selection"]["include"],
        json!(["uploads/report.txt", "shares/reference.md"])
    );
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM environments")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    for path in ["../private", "/etc/passwd", "a/../../b", "a\\b"] {
        let mut request = wanted(&t, "Refused path");
        request.files = Some(NewEnvironmentFiles::FromThread {
            from_thread: source,
            selection: Some(FileSelection::Selection {
                include: vec![path.into()],
                exclude: None,
            }),
        });
        let reply = world
            .app
            .call(EnvironmentCreate, &[])
            .token(&t.c.admin.token)
            .body(&request)
            .send()
            .await;
        assert_eq!(reply.status, 400, "{path}: {:?}", reply.body);
    }
    let foreign = initial_source(&world, &t, true).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM environments")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let jobs_before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let mut request = wanted(&t, "Refused foreign source");
    request.files = Some(NewEnvironmentFiles::FromThread {
        from_thread: foreign,
        selection: None,
    });
    let reply = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&request)
        .send()
        .await;
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM environments")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let jobs_after: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!((after, jobs_after), (count, jobs_before));
    assert_eq!(
        count,
        before + 1,
        "the only extra environment was the foreign source fixture"
    );
}

#[tokio::test]
async fn an_initial_snapshot_reference_has_to_exist_and_a_hash_is_not_an_initial_file_authority() {
    use orch_api_contract::binding::NewEnvironmentFiles;
    let world = World::new().await;
    let t = team(&world).await;
    let mut request = wanted(&t, "No such snapshot");
    request.files = Some(NewEnvironmentFiles::Snapshot {
        snapshot_id: orch_types::new_id(),
    });
    assert_eq!(
        world
            .app
            .call(EnvironmentCreate, &[])
            .token(&t.c.admin.token)
            .body(&request)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let mut raw = serde_json::to_value(wanted(&t, "A hash is not authority")).unwrap();
    raw["files"] = json!({"blob":"a".repeat(64)});
    assert_eq!(
        world
            .app
            .call(EnvironmentCreate, &[])
            .token(&t.c.admin.token)
            .raw_body(&raw)
            .send()
            .await
            .status,
        400
    );
}

#[tokio::test]
async fn restart_is_one_manager_job_on_the_existing_unit_and_rejects_an_overlap_or_invalid_mode() {
    use orch_api_contract::environment::{
        EnvironmentRestart, EnvironmentRestartBody, EnvironmentRestartMode,
    };
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Restart this unit").await;
    let runtime = orch_types::new_id();
    let mut conn = world.conn().await;
    sqlx::query("UPDATE jobs SET state='cancelled',finished_at=$2 WHERE environment_id=$1")
        .bind(made.base.id)
        .bind(world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET status='running',generation=1 WHERE id=$1")
        .bind(made.base.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,generation,state,volumes)
        VALUES($1,'environment',$2,'restart-unit',$3,'docker',$4,$5,1,'running','{}')")
        .bind(runtime).bind(t.host).bind(made.base.id).bind(t.template).bind([1_u8;32].as_slice()).execute(&mut *conn).await.unwrap();
    drop(conn);
    let refused = world
        .app
        .call(EnvironmentRestart, &[&made.base.id])
        .token(&t.c.viewer.token)
        .body(&EnvironmentRestartBody::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let malformed = world
        .app
        .call(EnvironmentRestart, &[&made.base.id])
        .token(&t.c.admin.token)
        .raw_body(&json!({"mode":"force"}))
        .send()
        .await;
    assert_eq!(malformed.status, 400);
    let reply = world
        .app
        .call(EnvironmentRestart, &[&made.base.id])
        .token(&t.c.admin.token)
        .body(&EnvironmentRestartBody {
            mode: EnvironmentRestartMode::Wait,
        })
        .send()
        .await;
    assert_eq!(reply.status, 202);
    let job = reply.ok();
    assert_eq!(job.kind, "environment_restart");
    let row = orch_db::queries::jobs::job(&mut *world.conn().await, job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.plan["runtime"], json!(runtime));
    assert_eq!(row.plan["restart_mode"], "wait");
    assert_eq!(row.created_by, Some(t.c.admin.id()));
    let overlap = world
        .app
        .call(EnvironmentRestart, &[&made.base.id])
        .token(&t.c.admin.token)
        .body(&EnvironmentRestartBody {
            mode: EnvironmentRestartMode::Interrupt,
        })
        .send()
        .await
        .problem();
    assert_eq!(overlap.code, ErrorCode::EnvironmentUnavailable);
    let units: i64 = sqlx::query_scalar("SELECT count(*) FROM runtimes WHERE environment_id=$1")
        .bind(made.base.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(units, 1, "accepting restart allocates no successor");
}

async fn make(world: &World, team: &Team, by: &Who, name: &str) -> Environment {
    world
        .app
        .call(EnvironmentCreate, &[])
        .token(&by.token)
        .body(&wanted(team, name))
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn creation_commits_its_owned_environment_and_job_before_a_slow_response_read() {
    let world = World::new().await;
    let t = team(&world).await;
    let mut blocker = world.db.pool.begin().await.unwrap();
    // This table is read only while assembling the response, after admission and its writes.
    sqlx::query("LOCK TABLE component_updates IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let making = make(&world, &t, &t.c.admin, "Committed before response");
    let inspect = async {
        rig::eventually("the response's blocked component update read", || async {
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE 'SELECT machine_id,recommended_version,state,detail,command_id FROM component_updates%')")
                .fetch_one(&world.db.pool).await.unwrap()
        }).await;
        let committed: (Id, Id) = sqlx::query_as(
            "SELECT id,owner_id FROM environments WHERE name='Committed before response'",
        )
        .fetch_one(&world.db.pool)
        .await
        .expect("slow presentation must not retain the creation transaction");
        assert_eq!(committed.1, t.c.admin.id());
        let jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_provision'",
        )
        .bind(committed.0)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(
            jobs, 1,
            "the environment and its durable job commit together"
        );
        blocker.rollback().await.unwrap();
    };
    let (made, ()) = tokio::join!(making, inspect);
    assert_eq!(made.base.name, "Committed before response");
}

#[tokio::test]
async fn confirmed_immediate_delete_returns_its_durable_job_and_no_predicted_expired_purge_date() {
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Delete now").await;
    sqlx::query("UPDATE jobs SET state='cancelled',finished_at=$2 WHERE environment_id=$1")
        .bind(made.base.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let call = || {
        let request = world
            .app
            .call(EnvironmentDelete, &[&made.base.id])
            .token(&t.c.admin.token)
            .query(&EnvironmentDeleteQuery {
                confirm: "Delete now".into(),
                at: None,
            });
        async move { request.send().await }
    };
    let reply = call().await;
    assert_eq!(reply.status, 202);
    let accepted: Environment = reply.ok();
    let promise = accepted.removal.as_ref().unwrap();
    assert_eq!(
        promise.phase,
        orch_api_contract::environment::EnvironmentRemovalPhase::Removing
    );
    assert!(promise.deleted_at.is_none() && promise.purge_at.is_none());
    let job = promise.job.as_ref().unwrap();
    assert_eq!(job.kind, "environment_remove");
    assert_eq!(accepted.job.as_ref().unwrap().id, job.id);
    let repeated: Environment = call().await.ok();
    assert_eq!(
        repeated.removal.as_ref().unwrap().job.as_ref().unwrap().id,
        job.id
    );
    assert_eq!(
        repeated.removal.as_ref().unwrap().revision,
        promise.revision
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_remove'",
    )
    .bind(made.base.id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn a_person_makes_an_environment_on_a_host_they_may_use_and_is_given_the_job_that_makes_its_unit()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Main").await;
    assert_eq!(made.base.name, "Main");
    assert_eq!(made.base.owner.username, "admin");
    assert_eq!(made.base.status, EnvironmentStatus::Provisioning);
    assert_eq!(made.home.id, t.topic);
    let job = made.job.expect("the job that makes the unit");
    assert_eq!(job.kind, "environment_provision");
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.percent, 0);
    // The job is read by its own address, with its phases in the order they are carried out.
    let read: Job = world
        .app
        .call(JobGet, &[&job.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(read.environment_id, Some(made.base.id));
    let phases: Vec<&str> = read
        .progress
        .phases
        .iter()
        .map(|phase| phase.name.as_str())
        .collect();
    assert_eq!(
        phases,
        [
            "schedule", "image", "volumes", "tools", "create", "enroll", "attach", "files",
            "after_up", "verify", "commit"
        ]
    );
    assert_eq!(read.created_by.unwrap().username, "admin");
    // The environment is theirs, and read back it shows the same job.
    let again: Environment = world
        .app
        .call(EnvironmentGet, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(again.job.unwrap().id, job.id);
    assert!(
        again
            .access
            .permissions
            .iter()
            .any(|word| word == "environment.manage")
    );
}

#[tokio::test]
async fn a_person_cannot_make_two_environments_under_one_name_in_an_org() {
    let world = World::new().await;
    let t = team(&world).await;
    let first = make(&world, &t, &t.c.admin, "Main").await;
    let again = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&wanted(&t, "main"))
        .send()
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::ConflictExists);
    assert_eq!(again.extra["existing_id"], json!(first.base.id));
}

#[tokio::test]
async fn somebody_who_may_not_interact_in_the_place_is_refused_with_what_is_missing() {
    let world = World::new().await;
    let t = team(&world).await;
    let refused = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.viewer.token)
        .body(&wanted(&t, "Main"))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(
        missing_permissions(&refused).contains(&"topic.interact".to_owned()),
        "{refused:?}"
    );
    let nothing: i64 = sqlx::query_scalar("SELECT count(*) FROM environments")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(nothing, 0, "a refusal makes nothing");
}

#[tokio::test]
async fn a_host_that_is_named_and_cannot_take_the_environment_is_not_replaced_by_another() {
    let world = World::new().await;
    let t = team(&world).await;
    let busy = a_host(&world, &t.c.admin, t.c.acme, "Busy box", 100_000).await;
    sqlx::query("UPDATE hosts SET schedulable = false WHERE id = $1")
        .bind(busy)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut asking = wanted(&t, "Main");
    asking.host_id = Some(Some(busy));
    let blocked = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&asking)
        .send()
        .await
        .problem();
    assert_eq!(blocked.code, ErrorCode::PlanBlocked);
    assert_eq!(blocked.extra["reason"], "no_host");
    // The detail is the code's sentence; the host is named among what blocks.
    assert_eq!(
        blocked.detail,
        orch_api_contract::messages::for_code(ErrorCode::PlanBlocked)
    );
    assert_eq!(
        blocked.extra["host_blockers"][0]["host"]["name"],
        "Busy box"
    );
    assert_eq!(
        blocked.extra["host_blockers"].as_array().unwrap().len(),
        1,
        "a named plan reports only its selected host"
    );
    assert_eq!(blocked.extra["host_blockers"][0]["host"]["id"], json!(busy));
    // Left to the scheduler, the host that can is used.
    let made = make(&world, &t, &t.c.admin, "Main").await;
    assert_eq!(made.base.status, EnvironmentStatus::Provisioning);
}

#[tokio::test]
async fn a_blocked_creation_names_each_authorized_offline_host_and_its_first_observed_time_without_making_anything()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let second = a_host(&world, &t.c.admin, t.c.acme, "docker-2", 16_384).await;
    let since = world.now() - chrono::Duration::minutes(20);
    {
        let mut conn = world.conn().await;
        sqlx::query("UPDATE hosts SET name='docker-1' WHERE id=$1")
            .bind(t.host)
            .execute(&mut *conn)
            .await
            .unwrap();
        let policy = orch_link_contract::policy::LocalPolicy::installed(
            orch_link_contract::policy::PolicyDefaults::Managed,
        );
        for host in [t.host, second] {
            let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
                .bind(host)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            sqlx::query("UPDATE machines SET facts=facts || $2 WHERE id=$1")
                .bind(machine)
                .bind(json!({"policy":policy.reported()}))
                .execute(&mut *conn)
                .await
                .unwrap();
            orch_db::queries::resources::set_presence(&mut conn, machine, false, None, since)
                .await
                .unwrap();
            orch_db::queries::resources::set_presence(
                &mut conn,
                machine,
                false,
                None,
                since + chrono::Duration::minutes(1),
            )
            .await
            .unwrap();
        }
    }
    let before: (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM environments),(SELECT count(*) FROM jobs)")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let refused = world
        .app
        .call(EnvironmentCreate, &[])
        .token(&t.c.admin.token)
        .body(&wanted(&t, "Blocked by offline hosts"))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::PlanBlocked);
    let hosts = refused.extra["host_blockers"].as_array().unwrap();
    assert_eq!(hosts.len(), 2);
    for name in ["docker-1", "docker-2"] {
        let host = hosts
            .iter()
            .find(|host| host["host"]["name"] == name)
            .unwrap();
        assert_eq!(host["online"], false);
        assert_eq!(host["offline_since"], json!(Time::new(since)));
        assert!(host["message"].as_str().unwrap().contains("offline"));
    }
    let after: (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM environments),(SELECT count(*) FROM jobs)")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(after, before, "a refused plan never creates a unit or job");
}

#[tokio::test]
async fn a_job_is_read_by_those_who_may_see_what_it_is_about_and_by_nobody_else() {
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Main").await;
    let job = made.job.unwrap().id;
    let stranger = world
        .app
        .call(JobGet, &[&job])
        .token(&t.c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
    let unknown = world
        .app
        .call(JobGet, &[&orch_types::new_id()])
        .token(&t.c.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_job_is_stopped_by_whoever_manages_its_environment_and_a_removal_is_not_stopped() {
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Main").await;
    let job = made.job.unwrap().id;
    let refused = world
        .app
        .call(JobCancel, &[&job])
        .token(&t.c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let asked: Job = world
        .app
        .call(JobCancel, &[&job])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(asked.id, job);
    let requested: bool = sqlx::query_scalar("SELECT plan ? 'cancel' FROM jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(
        requested,
        "the request to stop is written on the job, for whichever process runs it"
    );
    // A removal is withdrawn before it begins, never stopped under way.
    let removal = {
        let mut conn = world.conn().await;
        sqlx::query("UPDATE jobs SET state = 'succeeded', finished_at = now() WHERE id = $1")
            .bind(job)
            .execute(&mut *conn)
            .await
            .unwrap();
        world
            .app
            .app
            .engine
            .schedule_environment_removal(
                &mut conn,
                made.base.id,
                Some(t.c.admin.id()),
                world.now(),
                world.now(),
            )
            .await
            .unwrap();
        orch_engine::jobs::remove::queue(&mut conn, world.now(), made.base.id, None)
            .await
            .unwrap()
    };
    let stopping = world
        .app
        .call(JobCancel, &[&removal.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(stopping.code, ErrorCode::PlanBlocked);
}

#[tokio::test]
async fn the_places_of_a_topic_are_the_hosts_the_caller_may_use_with_why_one_cannot_be_chosen_and_the_environments_they_manage()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let small = a_host(&world, &t.c.admin, t.c.acme, "Small box", 8_192).await;
    sqlx::query("UPDATE hosts SET schedulable = false WHERE id = $1")
        .bind(small)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let made = make(&world, &t, &t.c.admin, "Main").await;
    let listed = world
        .app
        .call(TopicPlaceList, &[&t.topic])
        .token(&t.c.admin.token)
        .query(&orch_api_contract::environment::PlaceListQuery {
            template_id: Some(t.template),
            config: None,
        })
        .send()
        .await
        .ok();
    let hosts: Vec<_> = listed
        .iter()
        .filter_map(|place| match place {
            Place::Host(host) => Some(host),
            Place::Environment(_) => None,
        })
        .collect();
    assert_eq!(hosts.len(), 2);
    assert_eq!(
        hosts[0].host.name, "Build box",
        "the most free memory first"
    );
    assert!(hosts[0].usable && hosts[0].automatic);
    assert_eq!(hosts[0].owner.as_ref().unwrap().username, "admin");
    assert_eq!(hosts[0].host.address, "acme.admin.Build box");
    assert_eq!(hosts[0].capacity.memory_mb, 16_384);
    assert!(!hosts[1].usable && !hosts[1].automatic);
    assert_eq!(hosts[1].reasons, [HostReason::Unschedulable]);
    let environments: Vec<_> = listed
        .iter()
        .filter_map(|place| match place {
            Place::Environment(each) => Some(each),
            Place::Host(_) => None,
        })
        .collect();
    assert_eq!(
        environments.len(),
        1,
        "the environments the caller manages follow the hosts"
    );
    assert_eq!(environments[0].environment.id, made.base.id);
    // The viewer may not interact in the place: they are not shown where one could be made.
    let refused = world
        .app
        .call(TopicPlaceList, &[&t.topic])
        .token(&t.c.viewer.token)
        .query(&orch_api_contract::environment::PlaceListQuery {
            template_id: Some(t.template),
            config: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn a_topic_names_the_environment_that_new_threads_join_and_lists_its_environments_with_that_one_first()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let first = make(&world, &t, &t.c.admin, "First").await;
    let second = make(&world, &t, &t.c.admin, "Second").await;
    let set = |environment: Option<Id>, who: &Who| {
        let call = world
            .app
            .call(TopicDefaultEnvironmentSet, &[&t.topic])
            .token(&who.token)
            .body(&TopicDefaultEnvironmentSetBody {
                environment_id: environment,
            });
        async move { call.send().await }
    };
    let topic = set(Some(second.base.id), &t.c.admin).await.ok();
    assert_eq!(topic.default_environment_id, Some(second.base.id));
    let listed = world
        .app
        .call(TopicEnvironmentList, &[&t.topic])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(
        listed
            .iter()
            .map(|each| each.base.id)
            .collect::<Vec<_>>()
            .first(),
        Some(&second.base.id),
        "the default first"
    );
    assert_ne!(first.base.id, second.base.id);
    // Only whoever manages the topic names its default, and nothing changes for the rest.
    let refused = set(Some(first.base.id), &t.c.viewer).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let still = world
        .app
        .call(TopicEnvironmentList, &[&t.topic])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(still[0].base.id, second.base.id);
    let cleared = set(None, &t.c.admin).await.ok();
    assert_eq!(cleared.default_environment_id, None);
}

#[tokio::test]
async fn an_environment_that_is_stopped_is_started_by_a_job_and_one_that_is_up_is_told_so() {
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Main").await;
    let open = made.job.unwrap().id;
    // The making is under way: starting it shows that job, and makes no second.
    let started: Job = world
        .app
        .call(EnvironmentStart, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(started.id, open);
    // Up, there is no job to show.
    sqlx::query("UPDATE jobs SET state = 'succeeded', finished_at = now() WHERE id = $1")
        .bind(open)
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET status = 'running' WHERE id = $1")
        .bind(made.base.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let up = world
        .app
        .call(EnvironmentStart, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(up.code, ErrorCode::ConflictExists);
    // Stopped, and without a unit to start, it is made anew by a job of its own.
    sqlx::query("UPDATE environments SET status = 'stopped' WHERE id = $1")
        .bind(made.base.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let anew: Job = world
        .app
        .call(EnvironmentStart, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert_ne!(anew.id, open);
    assert_eq!(anew.kind, "environment_provision");
    // Somebody who does not manage it cannot start it.
    let refused = world
        .app
        .call(EnvironmentStart, &[&made.base.id])
        .token(&t.c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn an_environment_without_a_unit_is_stopped_at_once_and_a_directory_on_a_machine_is_not_stopped_from_here()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Main").await;
    sqlx::query(
        "UPDATE jobs SET state = 'cancelled', finished_at = now() WHERE environment_id = $1",
    )
    .bind(made.base.id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET status = 'running' WHERE id = $1")
        .bind(made.base.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let stopped: Environment = world
        .app
        .call(EnvironmentStop, &[&made.base.id])
        .token(&t.c.admin.token)
        .body(&EnvironmentStopBody { interrupt: false })
        .send()
        .await
        .ok();
    assert_eq!(stopped.base.status, EnvironmentStatus::Stopped);
    sqlx::query("UPDATE environments SET placement = 'connected', template_id = NULL, status = 'running' WHERE id = $1")
        .bind(made.base.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let refused = world
        .app
        .call(EnvironmentStop, &[&made.base.id])
        .token(&t.c.admin.token)
        .body(&EnvironmentStopBody { interrupt: false })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::PlanBlocked);
}

#[tokio::test]
async fn keeping_awake_counts_as_activity_and_keeping_withdraws_a_removal_that_has_not_begun() {
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Main").await;
    sqlx::query(
        "UPDATE jobs SET state = 'cancelled', finished_at = now() WHERE environment_id = $1",
    )
    .bind(made.base.id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let kept: Environment = world
        .app
        .call(EnvironmentKeepAwake, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert!(kept.last_activity_at.is_some(), "it counts as activity now");
    // A removal is scheduled for it, and withdrawn.
    let scheduled: Environment = world
        .app
        .call(EnvironmentDelete, &[&made.base.id])
        .token(&t.c.admin.token)
        .query(&EnvironmentDeleteQuery {
            confirm: "Main".to_owned(),
            at: Some(Time::new(world.now() + chrono::Duration::days(1))),
        })
        .send()
        .await
        .ok();
    assert!(scheduled.remove_at.is_some());
    let kept: Environment = world
        .app
        .call(EnvironmentKeep, &[&made.base.id])
        .token(&t.c.admin.token)
        .send()
        .await
        .ok();
    assert!(kept.remove_at.is_none() && kept.remove_reason.is_none());
    // Somebody who does not manage it cannot keep it awake.
    let refused = world
        .app
        .call(EnvironmentKeepAwake, &[&made.base.id])
        .token(&t.c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn the_fleet_of_a_place_lists_its_environments_with_their_hosts_and_the_hosts_the_caller_manages()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Main").await;
    let fleet = world
        .app
        .call(NodeEnvironmentList, &[&t.topic])
        .token(&t.c.admin.token)
        .query(&NodeEnvironmentListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(fleet.entries.len(), 1);
    let entry = &fleet.entries[0];
    assert_eq!(entry.environment.id, made.base.id);
    assert_eq!(entry.template.as_ref().unwrap().name, "Small");
    assert!(entry.manageable);
    assert_eq!(fleet.hosts.len(), 1);
    assert_eq!(fleet.hosts[0].name, "Build box");
    assert_eq!(fleet.hosts[0].memory_capacity_mb, 16_384);
    assert!(fleet.hosts[0].schedulable);
    let none = world
        .app
        .call(NodeEnvironmentList, &[&t.topic])
        .token(&t.c.admin.token)
        .query(&NodeEnvironmentListQuery {
            status: Some(EnvironmentStatus::Running),
            ..NodeEnvironmentListQuery::default()
        })
        .send()
        .await
        .ok();
    assert!(none.entries.is_empty(), "only those in the state asked for");
}

#[tokio::test]
async fn a_hosts_manager_sees_everything_that_runs_on_it_and_takes_a_unit_away_after_naming_it() {
    let world = World::new().await;
    let t = team(&world).await;
    let made = make(&world, &t, &t.c.admin, "Main").await;
    sqlx::query(
        "UPDATE jobs SET state = 'cancelled', finished_at = now() WHERE environment_id = $1",
    )
    .bind(made.base.id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let runtime = orch_types::new_id();
    sqlx::query(
        "INSERT INTO runtimes (id, kind, host_id, unit_name, environment_id, driver, template_id, spec_hash, state, volumes)
         VALUES ($1, 'environment', $2, 'orch-environment-0123456789ab', $3, 'docker', $4, $5, 'running', '{}')",
    )
    .bind(runtime)
    .bind(t.host)
    .bind(made.base.id)
    .bind(t.template)
    .bind([0_u8; 32].as_slice())
    .execute(&world.db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET status = 'running' WHERE id = $1")
        .bind(made.base.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let listed = world
        .app
        .call(HostRuntimeList, &[&t.host])
        .token(&t.c.admin.token)
        .query(&HostRuntimeListQuery {
            kind: Some(UnitKind::Environment),
        })
        .send()
        .await
        .ok();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].unit_name, "orch-environment-0123456789ab");
    assert_eq!(listed[0].environment.as_ref().unwrap().id, made.base.id);
    assert_eq!(listed[0].template.name, "Small");
    let providers = world
        .app
        .call(HostRuntimeList, &[&t.host])
        .token(&t.c.admin.token)
        .query(&HostRuntimeListQuery {
            kind: Some(UnitKind::Provider),
        })
        .send()
        .await
        .ok();
    assert!(providers.is_empty());
    // Somebody who does not manage the host sees nothing of it.
    let refused = world
        .app
        .call(HostRuntimeList, &[&t.host])
        .token(&t.c.viewer.token)
        .query(&HostRuntimeListQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // The unit's name is asked for, to be sure; then the unit goes and the environment stands stopped.
    let wrong = world
        .app
        .call(HostRuntimeDelete, &[&t.host, &runtime])
        .token(&t.c.admin.token)
        .query(&HostRuntimeDeleteQuery {
            confirm: "another".to_owned(),
            volumes: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::RequestInvalid);
    world
        .app
        .call(HostRuntimeDelete, &[&t.host, &runtime])
        .token(&t.c.admin.token)
        .query(&HostRuntimeDeleteQuery {
            confirm: "orch-environment-0123456789ab".to_owned(),
            volumes: None,
        })
        .send()
        .await
        .ok();
    let (state, status): (String, String) = sqlx::query_as(
        "SELECT r.state, e.status FROM runtimes r JOIN environments e ON e.id = r.environment_id WHERE r.id = $1",
    )
    .bind(runtime)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!((state.as_str(), status.as_str()), ("retired", "stopped"));
}
