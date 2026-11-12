//! Actual scoped-key node creation, using raw replies that also compile before receipt DTOs exist.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod rig;
mod support;

use axum::body::{Body, to_bytes};
use chrono::Duration;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::{ErrorCode, MAX_JSON_BYTES};
use orch_api_contract::nodes::{NodeGet, ProjectChildCreate, ProjectCreate, TopicCreate};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_db::queries::{access, hierarchy, identity, transaction_guard};
use orch_settings::Target;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::time::Duration as Wait;
use support::{Statements, World};
use tower::ServiceExt;

// Same scheduling watchdog as the qualified personal-state lock rendezvous; lock state decides admission.
const LOCK_WAIT: Wait = Wait::from_secs(30);

#[derive(Clone, Copy, Debug)]
enum Create {
    Project,
    Folder,
    Topic,
}

impl Create {
    const ALL: [Self; 3] = [Self::Project, Self::Folder, Self::Topic];
    fn operation(self) -> &'static str {
        match self {
            Self::Project => "ProjectCreate",
            Self::Folder => "ProjectChildCreate",
            Self::Topic => "TopicCreate.create",
        }
    }
    fn kind(self) -> &'static str {
        match self {
            Self::Project | Self::Folder => "project",
            Self::Topic => "topic",
        }
    }
    fn parent_kind(self) -> &'static str {
        match self {
            Self::Project => "org",
            Self::Folder | Self::Topic => "project",
        }
    }
    fn path(self, parent: Id) -> String {
        match self {
            Self::Project => format!("/orgs/{parent}/projects"),
            Self::Folder => format!("/projects/{parent}/projects"),
            Self::Topic => format!("/projects/{parent}/topics"),
        }
    }
    async fn call(self, world: &World, token: &str, parent: Id, body: &Json) -> RawReply {
        match self {
            Self::Project => {
                world
                    .app
                    .call(ProjectCreate, &[&parent])
                    .token(token)
                    .raw_body(body)
                    .send_bytes()
                    .await
            }
            Self::Folder => {
                world
                    .app
                    .call(ProjectChildCreate, &[&parent])
                    .token(token)
                    .raw_body(body)
                    .send_bytes()
                    .await
            }
            Self::Topic => {
                world
                    .app
                    .call(TopicCreate, &[&parent])
                    .token(token)
                    .raw_body(body)
                    .send_bytes()
                    .await
            }
        }
    }
}

struct Key {
    id: Id,
    secret: String,
}

async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Node creation operation control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            // Only the ordinary short fixture lifetime is needed; no clock advance admits this key.
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}

fn rule(
    kind: &str,
    parent: Id,
    reach: &str,
    family: &str,
    operation: &str,
    actions: &[&str],
) -> Json {
    json!({"target":{"kind":kind,"id":parent,"reach":reach},"resources":[family],
        "allow":[{"operation":operation,"actions":actions}]})
}

fn write_rule(create: Create, parent: Id) -> Json {
    rule(
        create.parent_kind(),
        parent,
        "self",
        create.kind(),
        create.operation(),
        &["write"],
    )
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

fn id(value: &Json) -> Id {
    serde_json::from_value(value.clone()).unwrap()
}

// Count committed business effects rather than transport activity/audits of refused requests.
async fn counts(world: &World) -> (i64, i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM nodes),
        (SELECT count(*) FROM nodes WHERE kind='project'),
        (SELECT count(*) FROM nodes WHERE kind='topic'),
        (SELECT count(*) FROM threads),(SELECT count(*) FROM chats),
        (SELECT count(*) FROM audit_log WHERE action='node.created')",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

fn one_created(
    before: (i64, i64, i64, i64, i64, i64),
    after: (i64, i64, i64, i64, i64, i64),
    create: Create,
) {
    assert_eq!(
        after,
        (
            before.0 + 1,
            before.1 + i64::from(create.kind() == "project"),
            before.2 + i64::from(create.kind() == "topic"),
            before.3,
            before.4 + 1,
            before.5 + 1
        )
    );
}

async fn ordinary_node(world: &World, owner: &places::Who, node: Id) -> Json {
    success(
        &world
            .app
            .call(NodeGet, &[&node])
            .token(&owner.token)
            .send_bytes()
            .await,
    )
}

async fn setup(world: &World) -> (places::Who, Id, Id) {
    let owner = places::person(world, "owner").await;
    let org = places::make_org(world, &owner, "Creation team", "creation-team")
        .await
        .base
        .id;
    let project = places::make_project(world, &owner, org, "Parent project")
        .await
        .base
        .id;
    (owner, org, project)
}

fn parent(create: Create, org: Id, project: Id) -> Id {
    match create {
        Create::Project => org,
        Create::Folder | Create::Topic => project,
    }
}

#[tokio::test]
async fn write_only_creates_bind_the_actual_parent_and_return_only_the_created_receipt() {
    let world = World::new().await;
    let (owner, org, project) = setup(&world).await;
    for create in Create::ALL {
        let parent = parent(create, org, project);
        let writer = key(&world, owner.id(), vec![write_rule(create, parent)]).await;
        let name = format!("Private creation sentinel {}", create.operation());
        let before = counts(&world).await;
        let (reply, queries) =
            Statements::measure(create.call(&world, &writer.secret, parent, &json!({"name":name})))
                .await;
        let value = success(&reply);
        let made = id(&value["node_id"]);
        assert_eq!(
            value,
            json!({"projection":"receipt","node_id":made,"kind":create.kind()})
        );
        assert!(
            queries > 0,
            "the actual HTTP request must execute its database writer"
        );
        eprintln!(
            "{} receipt database statements: {queries}",
            create.operation()
        );
        one_created(before, counts(&world).await, create);
        let stored: (String, Option<Id>, Id) =
            sqlx::query_as("SELECT kind,parent_id,owner_id FROM nodes WHERE id=$1")
                .bind(made)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(stored, (create.kind().into(), Some(parent), owner.id()));
        let full = ordinary_node(&world, &owner, made).await;
        assert_eq!(full["name"], name);
        assert_eq!(full["owner"]["id"], json!(owner.id()));
        assert_eq!(full["owners"], 1);
        assert_eq!(full["page"], "follows");
        assert!(!id(&full["chat_id"]).is_nil());
    }
}

#[tokio::test]
async fn only_same_operation_read_on_the_new_subtree_releases_the_ordinary_full_reply() {
    let world = World::new().await;
    let (owner, org, project) = setup(&world).await;
    for create in Create::ALL {
        let parent = parent(create, org, project);
        let full = key(
            &world,
            owner.id(),
            vec![
                write_rule(create, parent),
                rule(
                    create.parent_kind(),
                    parent,
                    "subtree",
                    create.kind(),
                    create.operation(),
                    &["read"],
                ),
            ],
        )
        .await;
        let (reply, queries) = Statements::measure(create.call(
            &world,
            &full.secret,
            parent,
            &json!({"name":"Visible created node"}),
        ))
        .await;
        let value = success(&reply);
        assert!(value.get("projection").is_none() && value.get("node_id").is_none());
        let made = id(&value["id"]);
        let ordinary = ordinary_node(&world, &owner, made).await;
        for field in [
            "id",
            "kind",
            "parent_id",
            "org_id",
            "name",
            "description",
            "owner",
            "page",
            "chat_id",
            "revision",
        ] {
            assert_eq!(
                value[field],
                ordinary[field],
                "{} full field {field}",
                create.operation()
            );
        }
        assert!(value["path"].is_array() && value["access"].is_object());
        for source in value["path"].as_array().unwrap() {
            if id(&source["id"]) == made {
                continue;
            }
            // A folder's Project Read also covers its project parent. Org names,
            // and a topic's Project names, need their own source-family Read.
            let named = matches!(create, Create::Folder) && id(&source["id"]) == parent;
            assert_eq!(source["name"].is_string(), named);
            assert_eq!(source["redacted"], json!(!named));
        }
        let mut named_rules = vec![
            write_rule(create, parent),
            rule(
                create.parent_kind(),
                parent,
                "subtree",
                create.kind(),
                create.operation(),
                &["read"],
            ),
            rule("org", org, "self", "org", create.operation(), &["read"]),
        ];
        if !matches!(create, Create::Project) {
            named_rules.push(rule(
                "project",
                project,
                "self",
                "project",
                create.operation(),
                &["read"],
            ));
        }
        let names = key(&world, owner.id(), named_rules).await;
        let named = success(
            &create
                .call(
                    &world,
                    &names.secret,
                    parent,
                    &json!({"name":"Named ancestry"}),
                )
                .await,
        );
        let ordinary_named = ordinary_node(&world, &owner, id(&named["id"])).await;
        assert_eq!(
            named["path"], ordinary_named["path"],
            "same-operation source Read restores current ancestor names"
        );
        assert!(queries > 0);
        eprintln!("{} full database statements: {queries}", create.operation());
        // An independent NodeGet read cannot supply this operation's optional response read.
        let unrelated_read = key(
            &world,
            owner.id(),
            vec![
                write_rule(create, parent),
                rule(
                    create.parent_kind(),
                    parent,
                    "subtree",
                    create.kind(),
                    "NodeGet.read",
                    &["read"],
                ),
            ],
        )
        .await;
        let restricted = success(
            &create
                .call(
                    &world,
                    &unrelated_read.secret,
                    parent,
                    &json!({"name":"Must stay in receipt"}),
                )
                .await,
        );
        assert_eq!(
            restricted,
            json!({"projection":"receipt","node_id":id(&restricted["node_id"]),"kind":create.kind()})
        );
        // Read of the parent itself does not reach an object only created by this request.
        let parent_read = key(
            &world,
            owner.id(),
            vec![
                write_rule(create, parent),
                rule(
                    create.parent_kind(),
                    parent,
                    "self",
                    create.kind(),
                    create.operation(),
                    &["read"],
                ),
            ],
        )
        .await;
        assert_eq!(
            success(
                &create
                    .call(
                        &world,
                        &parent_read.secret,
                        parent,
                        &json!({"name":"Parent self read only"})
                    )
                    .await
            )["projection"],
            "receipt"
        );
    }
}

#[tokio::test]
async fn another_parent_wrong_business_family_and_same_id_wrong_kind_create_nothing() {
    let world = World::new().await;
    let (owner, org, project) = setup(&world).await;
    for create in Create::ALL {
        let parent = parent(create, org, project);
        let wrong_kind = if create.parent_kind() == "org" {
            "project"
        } else {
            "org"
        };
        let wrong_family = if create.kind() == "project" {
            "topic"
        } else {
            "project"
        };
        for given in [
            rule(
                create.parent_kind(),
                new_id(),
                "self",
                create.kind(),
                create.operation(),
                &["write"],
            ),
            rule(
                create.parent_kind(),
                parent,
                "self",
                wrong_family,
                create.operation(),
                &["write"],
            ),
            rule(
                wrong_kind,
                parent,
                "self",
                create.kind(),
                create.operation(),
                &["write"],
            ),
        ] {
            let token = key(&world, owner.id(), vec![given]).await;
            let before = counts(&world).await;
            let reply = create
                .call(
                    &world,
                    &token.secret,
                    parent,
                    &json!({"name":"Must not exist"}),
                )
                .await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
            assert_eq!(counts(&world).await, before);
        }
        // The endpoint's actual parent-kind requirement is independent of a key
        // whose selector truthfully names that other existing node kind.
        let (wrong_parent, actual_kind) = if matches!(create, Create::Project) {
            (project, "project")
        } else {
            (org, "org")
        };
        let token = key(
            &world,
            owner.id(),
            vec![rule(
                actual_kind,
                wrong_parent,
                "self",
                create.kind(),
                create.operation(),
                &["write"],
            )],
        )
        .await;
        let before = counts(&world).await;
        let refused = create
            .call(
                &world,
                &token.secret,
                wrong_parent,
                &json!({"name":"Wrong actual parent kind"}),
            )
            .await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND);
        assert_eq!(refused.problem().code, ErrorCode::NotFound);
        assert_eq!(counts(&world).await, before);
    }
}

#[tokio::test]
async fn scoped_creation_preserves_ordinary_authority_suspension_and_archived_parent_rules() {
    let world = World::new().await;
    let (owner, org, project) = setup(&world).await;
    let viewer = places::person(&world, "viewer").await;
    places::join(&world, org, org, &viewer, "viewer", &owner).await;
    for create in Create::ALL {
        let parent = parent(create, org, project);
        let token = key(&world, viewer.id(), vec![write_rule(create, parent)]).await;
        let before = counts(&world).await;
        let reply = create
            .call(
                &world,
                &token.secret,
                parent,
                &json!({"name":"No ordinary authority"}),
            )
            .await;
        assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
        assert_eq!(counts(&world).await, before);
    }
    access::testing::suspend_in(&mut *world.conn().await, org, viewer.id(), owner.id())
        .await
        .unwrap();
    let token = key(
        &world,
        viewer.id(),
        vec![write_rule(Create::Topic, project)],
    )
    .await;
    let before = counts(&world).await;
    let refused = Create::Topic
        .call(&world, &token.secret, project, &json!({"name":"Suspended"}))
        .await;
    assert_eq!(refused.problem().code, ErrorCode::OrgSuspended);
    assert_eq!(counts(&world).await, before);
    hierarchy::archive(&mut *world.conn().await, project, owner.id(), world.now())
        .await
        .unwrap();
    for create in [Create::Folder, Create::Topic] {
        let token = key(&world, owner.id(), vec![write_rule(create, project)]).await;
        let reply = create
            .call(&world, &token.secret, project, &json!({"name":"Archived"}))
            .await;
        assert_eq!(reply.problem().code, ErrorCode::NodeArchived);
        assert_eq!(counts(&world).await, before);
    }
}

#[tokio::test]
async fn private_defaults_closed_org_and_required_factor_setup_keep_their_existing_meanings() {
    let world = World::new().await;
    let (owner, org, project) = setup(&world).await;
    sqlx::query("UPDATE orgs SET closed=TRUE WHERE node_id=$1")
        .bind(org)
        .execute(&world.db.pool)
        .await
        .unwrap();
    // Closed stops new membership; it does not forbid an existing owner from creating work.
    for create in Create::ALL {
        let parent = parent(create, org, project);
        let token = key(&world, owner.id(), vec![write_rule(create, parent)]).await;
        let reply = create
            .call(
                &world,
                &token.secret,
                parent,
                &json!({"name":"Private created node","access":"private"}),
            )
            .await;
        let made = id(&success(&reply)["node_id"]);
        let shown = ordinary_node(&world, &owner, made).await;
        assert_eq!(shown["page"], "restricted");
        assert_eq!(shown["owner"]["id"], json!(owner.id()));
    }
    let token = key(&world, owner.id(), vec![write_rule(Create::Topic, project)]).await;
    let made = id(&success(
        &Create::Topic
            .call(&world, &token.secret, project, &json!({}))
            .await,
    )["node_id"]);
    let shown = ordinary_node(&world, &owner, made).await;
    assert_eq!(shown["name"], "New topic");
    assert_eq!(shown["title_source"], "auto");
    assert_eq!(shown["thread_count"], 0);
    world
        .set_at(Target::Node(org), "auth.second_step", json!("required"))
        .await;
    let before = counts(&world).await;
    for credential in [&token.secret, &owner.token] {
        let refused = Create::Topic
            .call(
                &world,
                credential,
                project,
                &json!({"name":"Needs factor setup"}),
            )
            .await;
        assert_eq!(refused.problem().code, ErrorCode::AuthSecondStepSetup);
        assert_eq!(counts(&world).await, before);
    }
}

#[tokio::test]
async fn first_thread_creation_checks_thread_authority_and_preserves_scoped_receipts_and_browser_content()
 {
    let scene = rig::Scene::new().await;
    let world = &scene.rig.world;
    let project = scene.tree.project.base.id;
    let token=key(world,scene.ada.id(),vec![json!({"target":{"kind":"project","id":project,"reach":"subtree"},
        "resources":["topic"],"allow":[{"operation":"TopicCreate.with_thread","actions":["write","read"]}]})]).await;
    let body = json!({"name":"Topic with ordinary first thread","first_thread":{
        "binding":{"agent_id":scene.on.agent,"provider_id":scene.on.provider},
        "model":rig::MODEL,"environment":{"use":scene.on.environment}
    }});
    let before = counts(world).await;
    let turns = scene.rig.commands.issued_of("turn.start");
    let opens = scene.rig.commands.issued_of("session.open");
    let refused = Create::Topic
        .call(world, &token.secret, project, &body)
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(counts(world).await, before);
    assert_eq!(scene.rig.commands.issued_of("turn.start"), turns);
    assert_eq!(scene.rig.commands.issued_of("session.open"), opens);
    let complete = key(
        world,
        scene.ada.id(),
        vec![json!({
            "target":{"kind":"project","id":project,"reach":"subtree"},
            "resources":["topic","thread"],
            "allow":[{"operation":"TopicCreate.with_thread","actions":["write","read"]}]
        })],
    )
    .await;
    let scoped = success(
        &Create::Topic
            .call(world, &complete.secret, project, &body)
            .await,
    );
    assert_eq!(scoped["projection"], "receipt");
    assert_eq!(scoped.as_object().unwrap().len(), 4);
    assert!(scoped.get("topic").is_none() && scoped.get("thread").is_none());
    let scoped_topic = id(&scoped["topic_id"]);
    let scoped_thread = id(&scoped["thread_id"]);
    let stored_topic: Id = sqlx::query_scalar("SELECT parent_id FROM nodes WHERE id=$1")
        .bind(scoped_topic)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(stored_topic, project);
    let stored_thread_topic: Id =
        sqlx::query_scalar("SELECT topic_id FROM threads WHERE node_id=$1")
            .bind(scoped_thread)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(stored_thread_topic, scoped_topic);
    let ordinary = success(
        &Create::Topic
            .call(world, &scene.ada.token, project, &body)
            .await,
    );
    let topic = id(&ordinary["topic"]["id"]);
    let thread = id(&ordinary["thread"]["id"]);
    assert_eq!(ordinary["topic"]["parent_id"], json!(project));
    assert_eq!(ordinary["thread"]["parent_id"], json!(topic));
    let stored: Id = sqlx::query_scalar("SELECT topic_id FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, topic);
}

async fn fresh_create(create: Create, narrow: bool) {
    let world = World::new().await;
    let (owner, org, project) = setup(&world).await;
    let parent = parent(create, org, project);
    let writer = key(&world, owner.id(), vec![write_rule(create, parent)]).await;
    let before = counts(&world).await;
    let mut blocker = world.db.pool.begin().await.unwrap();
    transaction_guard::lock_person(&mut blocker, owner.id())
        .await
        .unwrap()
        .unwrap();
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(writer.id)
        .bind(owner.id())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(create.path(parent))
        .header("authorization", format!("Bearer {}", writer.secret))
        .header("content-type", "application/json")
        .body(Body::from(json!({"name":"Guarded creation"}).to_string()))
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(world.app.app.clone());
    let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    tokio::time::timeout(LOCK_WAIT,async {
        loop {
            if action.is_finished() { break; }
            let waits:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if waits { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the real request either completes or waits at its guarded identity");
    if narrow {
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1 AND user_id=$3")
            .bind(writer.id)
            .bind(serde_json::to_value(KeyScope::default()).unwrap())
            .bind(owner.id())
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    blocker.commit().await.unwrap();
    let response = tokio::time::timeout(LOCK_WAIT, action)
        .await
        .unwrap()
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    if narrow {
        assert_eq!(status, StatusCode::FORBIDDEN);
        let problem: orch_api_contract::common::Problem = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem.code, ErrorCode::AuthzDenied);
        assert_eq!(
            counts(&world).await,
            before,
            "{} wrote after scope narrowing",
            create.operation()
        );
    } else {
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let value: Json = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value,
            json!({"projection":"receipt","node_id":id(&value["node_id"]),"kind":create.kind()})
        );
        one_created(before, counts(&world).await, create);
    }
}

#[tokio::test]
async fn creation_rechecks_current_key_scope_at_its_existing_action_guard() {
    for create in Create::ALL {
        fresh_create(create, false).await;
        fresh_create(create, true).await;
    }
}

#[tokio::test]
async fn topic_creation_requires_its_own_read_of_the_actual_visible_default_environment() {
    let scene = rig::Scene::new().await;
    let world = &scene.rig.world;
    let project = scene.tree.project.base.id;
    let environment = scene.on.environment;
    world
        .set_at(Target::Node(project), "env.default_id", json!(environment))
        .await;
    let base = vec![
        write_rule(Create::Topic, project),
        rule(
            "project",
            project,
            "subtree",
            "topic",
            "TopicCreate.create",
            &["read"],
        ),
    ];
    for unrelated_environment_read in [false, true] {
        let mut rules = base.clone();
        if unrelated_environment_read {
            rules.push(rule(
                "environment",
                environment,
                "self",
                "environment",
                "EnvironmentGet.read",
                &["read"],
            ));
        }
        let token = key(world, scene.ada.id(), rules).await;
        let reply = success(
            &Create::Topic
                .call(
                    world,
                    &token.secret,
                    project,
                    &json!({"name":"Environment source held"}),
                )
                .await,
        );
        let made = id(&reply["node_id"]);
        assert_eq!(
            reply,
            json!({"projection":"receipt","node_id":made,"kind":"topic"})
        );
        let ordinary = ordinary_node(world, &scene.ada, made).await;
        assert_eq!(ordinary["default_environment_id"], json!(environment));
        assert!(
            ordinary["environments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["id"] == json!(environment)),
            "the ordinary reader must actually expose the inherited environment for this control"
        );
    }
    let mut rules = base;
    rules.push(rule(
        "environment",
        environment,
        "self",
        "environment",
        "TopicCreate.create",
        &["read"],
    ));
    let token = key(world, scene.ada.id(), rules).await;
    let full = success(
        &Create::Topic
            .call(
                world,
                &token.secret,
                project,
                &json!({"name":"Environment source admitted"}),
            )
            .await,
    );
    assert!(full.get("projection").is_none());
    let ordinary = ordinary_node(world, &scene.ada, id(&full["id"])).await;
    assert_eq!(
        full["default_environment_id"],
        ordinary["default_environment_id"]
    );
    assert_eq!(full["environments"], ordinary["environments"]);
}
