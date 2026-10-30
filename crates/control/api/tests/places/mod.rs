//! A small world for the tests of the hierarchy over HTTP: people with a bearer token each, an org they make through the API, and the members and roles a test needs, written as the database would hold them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::support::{Person, World};
use chrono::Duration;
use orch_api_contract::common::Problem;
use orch_api_contract::nodes::{
    Node, NodeChildList, NodeChildListQuery, NodeGet, Org, OrgChildCreate, OrgChildCreateBody,
    OrgCreate, OrgCreateBody, Project, ProjectCreate, ProjectCreateBody, Topic, TopicCreate,
    TopicCreateBody, TopicCreateReply,
};
use orch_api_core::middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::access::testing as access_rows;
use orch_db::queries::identity::testing as user_rows;
use orch_types::{Id, new_id};

/// Reviews an ordinary archive act for the same caller; negative authority tests still reach the act's guard.
pub async fn archive_body(
    world: &World,
    token: &str,
    node: Id,
    archiving: bool,
) -> orch_api_contract::nodes::NodeArchiveBody {
    use orch_api_contract::nodes::{NodeArchivePreview, NodeUnarchivePreview};
    let reply = if archiving {
        world
            .app
            .call(NodeArchivePreview, &[&node])
            .token(token)
            .send()
            .await
    } else {
        world
            .app
            .call(NodeUnarchivePreview, &[&node])
            .token(token)
            .send()
            .await
    };
    if reply.status != 200 {
        return orch_api_contract::nodes::NodeArchiveBody {
            preflight_id: Id::nil(),
            stamp: String::new(),
            expected_revision: 0,
        };
    }
    let plan = reply.ok();
    orch_api_contract::nodes::NodeArchiveBody {
        preflight_id: plan.preflight_id,
        stamp: plan.stamp,
        expected_revision: plan.revision,
    }
}
use serde_json::{Value, json};

/// A person of a test, with a bearer token that is good for a long while, so that a test may move the clock.
#[derive(Debug, Clone)]
pub struct Who {
    pub person: Person,
    pub token: String,
}

impl Who {
    pub fn id(&self) -> Id {
        self.person.id
    }
}

/// Makes a person with a session, and with the flag of a system admin where
/// `admin` says so.
pub async fn who(world: &World, username: &str, admin: bool) -> Who {
    let person = world.person(username).await;
    if admin {
        world.make_admin(&person).await;
    }
    let token = new_token(ACCESS_TOKEN_PREFIX);
    let mut conn = world.conn().await;
    user_rows::insert_session(
        &mut conn,
        new_id(),
        person.id,
        &token_hash(&token),
        world.now() + Duration::days(400),
    )
    .await
    .unwrap();
    Who { person, token }
}

/// A person who is no admin.
pub async fn person(world: &World, username: &str) -> Who {
    who(world, username, false).await
}

/// A system admin.
pub async fn admin(world: &World, username: &str) -> Who {
    let by = who(world, username, true).await;
    // This fixture explicitly asks for an administrative browser. Real setup,
    // sign-in and `who(..., true)` still create sessions whose switch is off.
    admin_mode(world, &by, true).await;
    by
}

/// Turns the As admin switch of `by`'s session on or off. Whoever administers a place (the system,
/// an org, a department) acts as its admin only while the switch is on; off, only their own pages
/// count.
pub async fn admin_mode(world: &World, by: &Who, on: bool) {
    world
        .app
        .call(orch_api_contract::auth::MeAdminModeSet, &[])
        .token(&by.token)
        .body(&orch_api_contract::auth::AdminModeSetBody { as_admin: on })
        .send()
        .await
        .ok();
}

/// Gives `by` an unused invitation to make one team org, as a system admin does from Users: as Orch
/// ships, only a system admin and somebody one invited make a team org. One held stays as it was.
pub async fn invite_to_make_org(world: &World, by: &Who) {
    sqlx::query(
        "UPDATE users SET org_invited_by = id, org_invited_at = $2
          WHERE id = $1 AND org_invited_at IS NULL",
    )
    .bind(by.person.id)
    .bind(world.now())
    .execute(&world.db.pool)
    .await
    .unwrap();
}

/// Makes a team's root org through the API, as `by`, who is invited to make one first.
pub async fn make_org(world: &World, by: &Who, name: &str, slug: &str) -> Org {
    invite_to_make_org(world, by).await;
    world
        .app
        .call(OrgCreate, &[])
        .token(&by.token)
        .body(&OrgCreateBody {
            name: name.to_owned(),
            slug: slug.to_owned(),
        })
        .send()
        .await
        .ok()
}

/// Makes an org inside `parent`, as `by`.
pub async fn make_department(world: &World, by: &Who, parent: Id, name: &str, slug: &str) -> Org {
    world
        .app
        .call(OrgChildCreate, &[&parent])
        .token(&by.token)
        .body(&OrgChildCreateBody {
            name: name.to_owned(),
            slug: slug.to_owned(),
            description: None,
            general: None,
            access: None,
        })
        .send()
        .await
        .ok()
}

/// Makes a project in `org`, as `by`.
pub async fn make_project(world: &World, by: &Who, org: Id, name: &str) -> Project {
    world
        .app
        .call(ProjectCreate, &[&org])
        .token(&by.token)
        .body(&ProjectCreateBody {
            name: name.to_owned(),
            description: None,
            access: None,
        })
        .send()
        .await
        .ok()
}

/// Makes a topic in `project`, as `by`.
pub async fn make_topic(world: &World, by: &Who, project: Id, name: &str) -> Topic {
    let reply = world
        .app
        .call(TopicCreate, &[&project])
        .token(&by.token)
        .body(&TopicCreateBody {
            name: Some(name.to_owned()),
            ..TopicCreateBody::default()
        })
        .send()
        .await
        .ok();
    match reply {
        TopicCreateReply::Topic(topic) => topic,
        TopicCreateReply::WithThread(both) => both.topic,
    }
}

/// Makes `member` a member of the tree of `root` with the role `role` at its
/// domain `domain`, as an invitation would have.
pub async fn join(world: &World, root: Id, domain: Id, member: &Who, role: &str, by: &Who) {
    let mut conn = world.conn().await;
    access_rows::insert_member(&mut conn, root, member.id(), domain)
        .await
        .unwrap();
    access_rows::give_role(&mut conn, domain, "user", member.id(), role, by.id(), None)
        .await
        .unwrap();
}

/// Gives `member` a role in a place, as a person named there.
pub async fn give_role(world: &World, place: Id, member: &Who, role: &str, by: &Who) {
    let mut conn = world.conn().await;
    access_rows::give_role(&mut conn, place, "user", member.id(), role, by.id(), None)
        .await
        .unwrap();
}

/// A node as `by` reads it.
pub async fn read(world: &World, by: &Who, id: Id) -> Node {
    world
        .app
        .call(NodeGet, &[&id])
        .token(&by.token)
        .send()
        .await
        .ok()
}

/// The identifiers of what lies directly inside `parent`, as `by` sees it.
pub async fn children(world: &World, by: &Who, parent: Id) -> Vec<Id> {
    world
        .app
        .call(NodeChildList, &[&parent])
        .token(&by.token)
        .query(&NodeChildListQuery::default())
        .send()
        .await
        .ok()
        .items
        .iter()
        .filter_map(orch_api_contract::common::Listed::as_visible)
        .map(orch_api_core::routes::nodes::node_id)
        .collect()
}

/// What a refusal says, as a plain value.
pub fn told(problem: &Problem) -> Value {
    serde_json::to_value(problem).unwrap()
}

/// The words of the permissions a refusal says are missing, object by object.
pub fn missing_permissions(problem: &Problem) -> Vec<String> {
    problem
        .extra
        .get("missing")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|missing| {
            missing["permissions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|word| word.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The first thing a refusal says is missing.
pub fn first_missing(problem: &Problem) -> Value {
    problem
        .extra
        .get("missing")
        .and_then(Value::as_array)
        .and_then(|list| list.first().cloned())
        .unwrap_or_else(|| json!(null))
}
