//! What a person holds in a tree, ending a membership, and moving a member's domain, over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::access::{NodeRoleList, Role};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{
    GroupMemberAdd, GroupMemberList, Member, OrgGroupCreate, OrgGroupCreateBody, OrgMemberList,
    OrgMemberListQuery,
};
use orch_api_contract::nodes::{Node, NodeGet};
use orch_api_contract::ownership::{
    Holdings, MemberMoveBody, MemberMovePlan, NodeTransfer, OrgMemberHoldings, OrgMemberMove,
    OrgMemberMovePreview, OrgMemberRemove, OrgMemberRemoveBody, TransferBody,
};
use orch_api_core::testing::Reply;
use orch_db::queries::resources::testing::insert_machine;
use orch_types::Id;
use places::{
    Who, first_missing, give_role, join, make_department, make_org, make_project, make_topic,
    missing_permissions, person, read,
};
use serde_json::json;
use support::{Statements, World};

struct Setup {
    boss: Who,
    dan: Who,
    eve: Who,
    acme: Id,
    research: Id,
    sales: Id,
    web: Id,
}

/// Acme, owned by Boss; Dan, a viewer at the root; Eve, a developer whose
/// domain is the department Research; a department Sales, and a project Web
/// at the root.
async fn setup(world: &World) -> Setup {
    let boss = person(world, "boss").await;
    let dan = person(world, "dan").await;
    let eve = person(world, "eve").await;
    let acme = make_org(world, &boss, "Acme", "acme").await;
    let research = make_department(world, &boss, acme.base.id, "Research", "research").await;
    let sales = make_department(world, &boss, acme.base.id, "Sales", "sales").await;
    join(world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    join(
        world,
        acme.base.id,
        research.base.id,
        &eve,
        "developer",
        &boss,
    )
    .await;
    let web = make_project(world, &boss, acme.base.id, "Web").await;
    Setup {
        boss,
        dan,
        eve,
        acme: acme.base.id,
        research: research.base.id,
        sales: sales.base.id,
        web: web.base.id,
    }
}

#[tokio::test]
async fn a_department_without_domain_members_still_names_its_inherited_role_holders() {
    let world = World::new().await;
    let s = setup(&world).await;
    assert!(
        members(&world, &s.boss, s.sales).await.is_empty(),
        "domain membership keeps its precise meaning"
    );
    let roles = world
        .app
        .call(NodeRoleList, &[&s.sales])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    let dan = roles
        .above
        .iter()
        .find(|held| {
            held.subject
                .user
                .as_ref()
                .is_some_and(|user| user.id == s.dan.id())
        })
        .expect("the root's viewer also reaches the department");
    assert_eq!(dan.node.id, s.acme);
    assert_eq!(dan.role, Role::Viewer);
    assert!(!dan.subject.user.as_ref().unwrap().display_name.is_empty());
    assert!(
        !roles.above.iter().any(|held| held
            .subject
            .user
            .as_ref()
            .is_some_and(|user| user.id == s.eve.id())),
        "a sibling department's member is not inherited"
    );
    let department = read(&world, &s.boss, s.sales).await;
    assert_eq!(owner_of(&department).0, "boss");
}

async fn holdings(
    world: &World,
    who: &Who,
    org: Id,
    user: &dyn std::fmt::Display,
) -> Reply<Holdings> {
    world
        .app
        .call(OrgMemberHoldings, &[&org, user])
        .token(&who.token)
        .send()
        .await
}

async fn remove(world: &World, who: &Who, org: Id, user: &dyn std::fmt::Display) -> Reply<()> {
    world
        .app
        .call(OrgMemberRemove, &[&org, user])
        .token(&who.token)
        .body(&OrgMemberRemoveBody::default())
        .send()
        .await
}

async fn members(world: &World, who: &Who, org: Id) -> Vec<Member> {
    world
        .app
        .call(OrgMemberList, &[&org])
        .token(&who.token)
        .query(&OrgMemberListQuery::default())
        .send()
        .await
        .ok()
        .items
}

fn names(list: &[Member]) -> Vec<&str> {
    list.iter()
        .map(|member| member.user.username.as_str())
        .collect()
}

fn owner_of(node: &Node) -> (&str, i64) {
    match node {
        Node::Org(node) => (node.base.owner.username.as_str(), node.base.owners),
        Node::Project(node) => (node.base.owner.username.as_str(), node.base.owners),
        Node::Topic(node) => (node.base.owner.username.as_str(), node.base.owners),
        Node::Thread(node) => (node.base.owner.username.as_str(), node.base.owners),
    }
}

async fn preview(
    world: &World,
    who: &Who,
    org: Id,
    user: Id,
    body: &MemberMoveBody,
) -> Reply<MemberMovePlan> {
    world
        .app
        .call(OrgMemberMovePreview, &[&org, &user])
        .token(&who.token)
        .body(body)
        .send()
        .await
}

async fn move_to(
    world: &World,
    who: &Who,
    org: Id,
    user: Id,
    body: &MemberMoveBody,
) -> Reply<Member> {
    world
        .app
        .call(OrgMemberMove, &[&org, &user])
        .token(&who.token)
        .body(body)
        .send()
        .await
}

fn to(org: Id, role: Option<Role>) -> MemberMoveBody {
    MemberMoveBody { to: org, role }
}

/// What Eve owns: a project of her own in Research with a topic in it, a
/// project she shares with Boss, and a department inside Research with a
/// project in it. Boss makes each and hands it to her.
struct Owned {
    lab: Id,
    run: Id,
    shared: Id,
    sub: Id,
    deep: Id,
}

async fn hand_to_eve(world: &World, s: &Setup, node: Id, keep_previous: bool) {
    world
        .app
        .call(NodeTransfer, &[&node])
        .token(&s.boss.token)
        .body(&TransferBody {
            user_id: s.eve.id(),
            keep_previous: Some(keep_previous),
        })
        .send()
        .await
        .ok();
}

async fn eve_owns(world: &World, s: &Setup) -> Owned {
    let lab = make_project(world, &s.boss, s.research, "Lab")
        .await
        .base
        .id;
    hand_to_eve(world, s, lab, false).await;
    let run = make_topic(world, &s.boss, lab, "Run").await.base.id;
    hand_to_eve(world, s, run, false).await;
    let shared = make_project(world, &s.boss, s.research, "Shared")
        .await
        .base
        .id;
    hand_to_eve(world, s, shared, true).await;
    let sub = make_department(world, &s.boss, s.research, "Sub", "sub")
        .await
        .base
        .id;
    hand_to_eve(world, s, sub, false).await;
    let deep = make_project(world, &s.boss, sub, "Deep").await.base.id;
    hand_to_eve(world, s, deep, false).await;
    Owned {
        lab,
        run,
        shared,
        sub,
        deep,
    }
}

#[tokio::test]
async fn the_holdings_of_a_member_count_what_they_own_alone_what_they_share_and_what_names_them() {
    let world = World::new().await;
    let s = setup(&world).await;
    eve_owns(&world, &s).await;
    give_role(&world, s.web, &s.eve, "viewer", &s.boss).await;
    let held = holdings(&world, &s.boss, s.acme, &s.eve.id()).await.ok();
    assert_eq!(
        (
            held.nodes.orgs,
            held.nodes.projects,
            held.nodes.topics,
            held.nodes.threads,
            held.nodes.documents
        ),
        (1, 2, 1, 0, 0),
        "Sub, Lab and Deep, and the topic Run, are hers alone"
    );
    assert_eq!(held.shared, 1, "Shared has Boss beside her");
    assert_eq!(held.entries, 2, "her role at Research and the one at Web");
    assert!(held.resources.is_empty() && held.machine_registrations.is_empty());
    assert_eq!(held.pending, 0);
    // She sees her own, as `me`.
    let own = holdings(&world, &s.eve, s.acme, &"me").await.ok();
    assert_eq!(own, held);
    // The org may be any of the tree: the answer is about the tree.
    let from_below = holdings(&world, &s.boss, s.research, &s.eve.id())
        .await
        .ok();
    assert_eq!(from_below, held);
    // A viewer reads nobody else's; the refusal says what is lacking and who can give it.
    let refused = holdings(&world, &s.dan, s.acme, &s.eve.id())
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&refused),
        vec!["members.manage".to_owned()]
    );
    assert_eq!(first_missing(&refused)["ask"][0]["username"], "boss");
    // Somebody who is no member of the tree has nothing in it.
    let stranger = person(&world, "stranger").await;
    let nothing = holdings(&world, &s.boss, s.acme, &stranger.id())
        .await
        .problem();
    assert_eq!(nothing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_holdings_preview_names_managed_resources_registrations_shared_ownership_and_pending_work()
 {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ResourceKind;
    use orch_authz::ownership::{NewAccess, start_resource};
    use orch_db::queries::resources::access_fixtures as fixtures;
    let world = World::new().await;
    let s = setup(&world).await;
    let mut conn = world.conn().await;
    let sole = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(s.eve.id()),
        Some(s.research),
        "Private key",
    )
    .await
    .unwrap();
    let shared = fixtures::insert_resource(
        &mut conn,
        "template",
        Some(s.eve.id()),
        Some(s.research),
        "Shared image",
    )
    .await
    .unwrap();
    for (kind, id) in [
        (ResourceKind::Provider, sole),
        (ResourceKind::Template, shared),
    ] {
        start_resource(
            &mut conn,
            ObjectRef {
                kind: kind.into(),
                id,
            },
            s.eve.id(),
            NewAccess::OnlyMe,
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
    }
    let machine = insert_machine(&mut conn, "Personal", "connected")
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
        .bind(machine)
        .bind(s.eve.id())
        .execute(&mut *conn)
        .await
        .unwrap();
    let registered = fixtures::insert_resource(
        &mut conn,
        "environment",
        Some(s.boss.id()),
        Some(s.research),
        "Personal work",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET placement='connected',machine_id=$2 WHERE id=$1")
        .bind(registered)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: registered,
        },
        s.boss.id(),
        NewAccess::OnlyMe,
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    for node in [s.research, s.eve.person.personal_org] {
        sqlx::query("INSERT INTO approvals(id,kind,node_id,requester_user_id,title,request,required_permission)
            VALUES($1,'access',$2,$3,'Need access','{}','node.view')")
            .bind(orch_types::new_id()).bind(node).bind(s.eve.id()).execute(&mut *conn).await.unwrap();
    }
    drop(conn);
    resources::name_person(&world, "template", shared, "access.own", s.boss.id()).await;
    let preview = holdings(&world, &s.boss, s.acme, &s.eve.id()).await.ok();
    assert_eq!(preview.resources.len(), 1);
    assert_eq!(preview.resources[0].object.id, sole);
    assert_eq!(preview.resources[0].object.name, "Private key");
    assert!(
        !preview.resources[0].personal_sign_in,
        "an API key is not a personal subscription sign-in"
    );
    assert_eq!(preview.shared_by_kind.resources, 1);
    assert_eq!(preview.shared, 1);
    assert_eq!(preview.machine_registrations.len(), 1);
    assert_eq!(preview.machine_registrations[0].object.id, registered);
    assert_eq!(preview.machine_registrations[0].machine.id, machine);
    assert_eq!(
        preview.pending, 1,
        "the personal org's request is outside this membership"
    );
    assert_eq!(holdings(&world, &s.eve, s.acme, &"me").await.ok(), preview);
    assert_eq!(
        holdings(&world, &s.dan, s.acme, &s.eve.id())
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM approvals WHERE requester_user_id=$1 AND state='pending'",
    )
    .bind(s.eve.id())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(pending, 2, "a preview has no effects");
}

#[tokio::test]
async fn the_holdings_preview_counts_inherited_mount_and_mcp_uses_and_respects_explicit_false() {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ResourceKind;
    use orch_authz::ownership::{NewAccess, start_resource};
    use orch_db::queries::resources::access_fixtures as fixtures;
    use serde_json::json;
    let world = World::new().await;
    let s = setup(&world).await;
    let topic = make_topic(&world, &s.boss, s.web, "Usage").await.base.id;
    let mut conn = world.conn().await;
    let mount = fixtures::insert_resource(
        &mut conn,
        "mount",
        Some(s.eve.id()),
        Some(s.research),
        // A mount's and an MCP server's name is how agents and paths name it: lower case, no spaces.
        "dataset",
    )
    .await
    .unwrap();
    let mcp = fixtures::insert_resource(
        &mut conn,
        "mcp_server",
        Some(s.eve.id()),
        Some(s.research),
        "tools",
    )
    .await
    .unwrap();
    for (kind, id) in [(ResourceKind::Mount, mount), (ResourceKind::McpServer, mcp)] {
        start_resource(
            &mut conn,
            ObjectRef {
                kind: kind.into(),
                id,
            },
            s.eve.id(),
            NewAccess::OnlyMe,
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
    }
    for (key, id) in [("env.mounts", mount), ("mcp.enabled", mcp)] {
        sqlx::query("INSERT INTO settings(node_id,key,value,updated_by) VALUES($1,$2,$3,$4)")
            .bind(s.research)
            .bind(key)
            .bind(json!({(id.to_string()):true}))
            .bind(s.eve.id())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    for selected in [true, false] {
        let environment = fixtures::insert_resource(
            &mut conn,
            "environment",
            Some(s.eve.id()),
            Some(s.research),
            if selected { "Inherited" } else { "Excluded" },
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Environment.into(),
                id: environment,
            },
            s.eve.id(),
            NewAccess::OnlyMe,
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
        if !selected {
            sqlx::query("UPDATE environments SET config=$2 WHERE id=$1").bind(environment)
                .bind(json!({"env.mounts":{(mount.to_string()):false},"mcp.enabled":{(mcp.to_string()):false}})).execute(&mut *conn).await.unwrap();
        }
        let thread = orch_types::new_id();
        // A node names who made it, as its owner here.
        sqlx::query(
            "INSERT INTO nodes(id,org_id,kind,parent_id,owner_id,created_by,name,path)
            SELECT $1,org_id,'thread',id,$3,$3,$4,path || $1::uuid FROM nodes WHERE id=$2",
        )
        .bind(thread)
        .bind(topic)
        .bind(s.boss.id())
        .bind(format!("Use {selected}"))
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO threads(node_id,topic_id,environment_id) VALUES($1,$2,$3)")
            .bind(thread)
            .bind(topic)
            .bind(environment)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    drop(conn);
    let preview = holdings(&world, &s.boss, s.acme, &s.eve.id()).await.ok();
    assert_eq!(
        preview
            .resources
            .iter()
            .find(|resource| resource.object.id == mount)
            .unwrap()
            .threads,
        1
    );
    assert_eq!(
        preview
            .resources
            .iter()
            .find(|resource| resource.object.id == mcp)
            .unwrap()
            .threads,
        1
    );
}

#[tokio::test]
async fn ending_membership_applies_each_resource_fate_and_preserves_personal_machines_and_other_owners()
 {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ResourceKind;
    use orch_api_contract::ownership::ResourceFate;
    use orch_authz::ownership::{NewAccess, start_resource};
    use orch_db::queries::resources::access_fixtures as fixtures;
    use serde_json::json;
    let world = World::new().await;
    let s = setup(&world).await;
    let mut conn = world.conn().await;
    let keep = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(s.eve.id()),
        Some(s.research),
        "Kept sign-in",
    )
    .await
    .unwrap();
    let disable = fixtures::insert_resource(
        &mut conn,
        "template",
        Some(s.eve.id()),
        Some(s.research),
        "Disabled image",
    )
    .await
    .unwrap();
    let remove = fixtures::insert_resource(
        &mut conn,
        "mcp_server",
        Some(s.eve.id()),
        Some(s.research),
        // An MCP server's name is how agents name it: lower case, no spaces.
        "removed-tools",
    )
    .await
    .unwrap();
    let shared = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(s.eve.id()),
        Some(s.research),
        "Shared provider",
    )
    .await
    .unwrap();
    for (kind, id) in [
        (ResourceKind::Provider, keep),
        (ResourceKind::Template, disable),
        (ResourceKind::McpServer, remove),
        (ResourceKind::Provider, shared),
    ] {
        start_resource(
            &mut conn,
            ObjectRef {
                kind: kind.into(),
                id,
            },
            s.eve.id(),
            NewAccess::OnlyMe,
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
    }
    let key = orch_db::queries::settings::put_secret(
        &mut conn,
        "provider",
        keep,
        "api_key",
        b"unchanged sealed value",
        Some(s.eve.id()),
        world.now(),
    )
    .await
    .unwrap();
    let machine = insert_machine(&mut conn, "Personal", "connected")
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
        .bind(machine)
        .bind(s.eve.id())
        .execute(&mut *conn)
        .await
        .unwrap();
    let personal = fixtures::insert_environment(
        &mut conn,
        s.boss.id(),
        Some(s.research),
        "Personal workspace",
        "connected",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET machine_id=$2 WHERE id=$1")
        .bind(personal)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: personal,
        },
        s.boss.id(),
        NewAccess::OnlyMe,
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    let request = orch_types::new_id();
    sqlx::query(
        "INSERT INTO approvals(id,kind,node_id,requester_user_id,title,request,required_permission)
        VALUES($1,'access',$2,$3,'Access','{}','node.view')",
    )
    .bind(request)
    .bind(s.research)
    .bind(s.eve.id())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    resources::name_person(&world, "provider", shared, "access.own", s.boss.id()).await;
    world
        .app
        .call(OrgMemberRemove, &[&s.acme, &s.eve.id()])
        .token(&s.boss.token)
        .body(&OrgMemberRemoveBody {
            resources: Some(std::collections::BTreeMap::from([
                (disable.to_string(), ResourceFate::Disable),
                (remove.to_string(), ResourceFate::Remove),
            ])),
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    let kept: (Id, String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT owner_id,status,deleted_at FROM providers WHERE id=$1")
            .bind(keep)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(kept, (s.boss.id(), "ready".to_owned(), None));
    let kept_key = orch_db::queries::settings::secret(&mut conn, "provider", keep, "api_key")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept_key.id, key.id);
    assert_eq!(
        kept_key.sealed.as_slice(),
        b"unchanged sealed value".as_slice()
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT enabled FROM env_templates WHERE id=$1")
            .bind(disable)
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT deleted_at IS NOT NULL FROM mcp_servers WHERE id=$1")
            .bind(remove)
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, Id>("SELECT owner_id FROM providers WHERE id=$1")
            .bind(shared)
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        s.boss.id()
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT deleted_at IS NOT NULL FROM environments WHERE id=$1"
        )
        .bind(personal)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT deleted_at IS NULL AND status='active' FROM machines WHERE id=$1"
        )
        .bind(machine)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM approvals WHERE id=$1")
            .bind(request)
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        "cancelled"
    );
    let detach: serde_json::Value =
        sqlx::query_scalar("SELECT body FROM commands WHERE kind='env.detach' AND machine_id=$1")
            .bind(machine)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(detach["environment_id"], json!(personal));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commands WHERE kind IN ('node.runtime.remove','fs.delete')"
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn an_outside_resource_choice_refuses_the_whole_membership_ending() {
    use orch_api_contract::ownership::ResourceFate;
    let world = World::new().await;
    let s = setup(&world).await;
    let before = members(&world, &s.boss, s.acme).await.len();
    let refused = world
        .app
        .call(OrgMemberRemove, &[&s.acme, &s.eve.id()])
        .token(&s.boss.token)
        .body(&OrgMemberRemoveBody {
            resources: Some(std::collections::BTreeMap::from([(
                orch_types::new_id().to_string(),
                ResourceFate::Remove,
            )])),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(members(&world, &s.boss, s.acme).await.len(), before);
    assert!(
        world
            .audit()
            .await
            .iter()
            .all(|action| action != "member.removed")
    );
}

#[tokio::test]
async fn department_resource_choices_disable_restore_or_rehome_without_removing_data_or_starting_work()
 {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ResourceKind;
    use orch_api_contract::ownership::{
        DeletedResources, NodeDelete, NodeDeleteBody, NodeRestore, NodeRestoreBody,
    };
    use orch_authz::ownership::{NewAccess, start_resource};
    use orch_db::queries::resources::access_fixtures as fixtures;
    let world = World::new().await;
    let s = setup(&world).await;
    let mut conn = world.conn().await;
    let enabled = fixtures::insert_resource(
        &mut conn,
        "template",
        Some(s.boss.id()),
        Some(s.research),
        "Available",
    )
    .await
    .unwrap();
    let already_off = fixtures::insert_resource(
        &mut conn,
        "template",
        Some(s.boss.id()),
        Some(s.research),
        "Already off",
    )
    .await
    .unwrap();
    for id in [enabled, already_off] {
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Template.into(),
                id,
            },
            s.boss.id(),
            NewAccess::Defaults,
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
    }
    sqlx::query("UPDATE env_templates SET enabled=false WHERE id=$1")
        .bind(already_off)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let deleted = world
        .app
        .call(NodeDelete, &[&s.research])
        .token(&s.boss.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await
        .ok();
    for id in [enabled, already_off] {
        let state: (bool, bool) =
            sqlx::query_as("SELECT enabled,deleted_at IS NULL FROM env_templates WHERE id=$1")
                .bind(id)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(state, (false, true));
    }
    world
        .app
        .call(NodeRestore, &[&s.research])
        .token(&s.boss.token)
        .body(&NodeRestoreBody {
            expected_deletion_id: deleted.deletion_id,
            undo_delete: None,
        })
        .send()
        .await
        .ok();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT enabled FROM env_templates WHERE id=$1")
            .bind(enabled)
            .fetch_one(&world.db.pool)
            .await
            .unwrap()
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT enabled FROM env_templates WHERE id=$1")
            .bind(already_off)
            .fetch_one(&world.db.pool)
            .await
            .unwrap()
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM commands WHERE kind IN ('node.runtime.start','turn.start','node.runtime.remove')")
        .fetch_one(&world.db.pool).await.unwrap(),0);

    let moved = fixtures::insert_resource(
        &mut *world.conn().await,
        "provider",
        Some(s.boss.id()),
        Some(s.sales),
        "Moved provider",
    )
    .await
    .unwrap();
    start_resource(
        &mut *world.conn().await,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
            id: moved,
        },
        s.boss.id(),
        NewAccess::Defaults,
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    world
        .app
        .call(NodeDelete, &[&s.sales])
        .token(&s.boss.token)
        .body(&NodeDeleteBody {
            resources: Some(DeletedResources::ToParent),
            ..NodeDeleteBody::default()
        })
        .send()
        .await
        .ok();
    let state: (Id, String, bool) =
        sqlx::query_as("SELECT home_id,status,deleted_at IS NULL FROM providers WHERE id=$1")
            .bind(moved)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(state, (s.acme, "ready".to_owned(), true));
}

#[tokio::test]
async fn ending_a_membership_passes_what_was_owned_alone_to_the_first_owner_of_the_org_it_lies_in()
{
    let world = World::new().await;
    let s = setup(&world).await;
    let owned = eve_owns(&world, &s).await;
    give_role(&world, s.web, &s.eve, "viewer", &s.boss).await;
    let crew = world
        .app
        .call(OrgGroupCreate, &[&s.acme])
        .token(&s.boss.token)
        .body(&OrgGroupCreateBody {
            name: "Crew".to_owned(),
            description: None,
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(GroupMemberAdd, &[&crew.id, &s.eve.id()])
        .token(&s.boss.token)
        .send()
        .await;
    // Dan reads a thing of Eve's and can end nothing.
    let refused = remove(&world, &s.dan, s.acme, &s.eve.id()).await.problem();
    assert_eq!(refused.extra["required"], "members.manage");
    assert_eq!(
        members(&world, &s.boss, s.acme).await.len(),
        3,
        "a refused removal removes nobody"
    );
    let gone = remove(&world, &s.boss, s.acme, &s.eve.id()).await;
    assert_eq!(gone.status, 204);
    // What was hers alone is Boss's, the first owner of Research, and the
    // department she was first owner of passed before what lay in it.
    for (node, what) in [
        (owned.lab, "a project of hers"),
        (owned.run, "a topic of hers"),
        (owned.sub, "a department of hers"),
        (owned.deep, "a project in that department"),
    ] {
        let after = read(&world, &s.boss, node).await;
        assert_eq!(owner_of(&after), ("boss", 1), "{what}");
    }
    // What she shared stays with the one she shared it with.
    assert_eq!(
        owner_of(&read(&world, &s.boss, owned.shared).await),
        ("boss", 1)
    );
    // She is no member, holds nothing, and is in no group.
    assert_eq!(
        names(&members(&world, &s.boss, s.acme).await),
        vec!["boss", "dan"]
    );
    let refused = world
        .app
        .call(NodeGet, &[&s.web])
        .token(&s.eve.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let roles = world
        .app
        .call(NodeRoleList, &[&s.web])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert!(
        roles.own.iter().all(|role| role
            .subject
            .user
            .as_ref()
            .is_none_or(|user| user.username != "eve")),
        "{:?}",
        roles.own
    );
    let people = world
        .app
        .call(GroupMemberList, &[&crew.id])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert!(people.is_empty());
    let again = remove(&world, &s.boss, s.acme, &s.eve.id()).await.problem();
    assert_eq!(again.code, ErrorCode::NotFound, "she is no member now");
    let query = orch_api_contract::nodes::NodeAuditListQuery {
        action: Some("member.removed".to_owned()),
        ..orch_api_contract::nodes::NodeAuditListQuery::default()
    };
    let audit = world
        .app
        .call(orch_api_contract::nodes::NodeAuditList, &[&s.acme])
        .token(&s.boss.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        audit.items.len(),
        1,
        "the org's audit reader sees one membership ending"
    );
    assert_eq!(
        audit.items[0].details["user"],
        serde_json::json!(s.eve.id())
    );
    assert!(
        matches!(&audit.items[0].actor,orch_api_contract::nodes::AuditActor::User { user } if user.id==s.boss.id())
    );
    let refused = world
        .app
        .call(orch_api_contract::nodes::NodeAuditList, &[&s.acme])
        .token(&s.dan.token)
        .query(&query)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["audit.view"]);
}

#[tokio::test]
async fn a_member_leaves_by_themself_and_the_last_owner_of_a_root_org_names_another_first() {
    let world = World::new().await;
    let s = setup(&world).await;
    // Dan, who owns nothing, leaves as `me`.
    let left = remove(&world, &s.dan, s.acme, &"me").await;
    assert_eq!(left.status, 204);
    let refused = world
        .app
        .call(NodeGet, &[&s.web])
        .token(&s.dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // Boss owns Acme alone: not leaving, and not removed by an admin either.
    let refused = remove(&world, &s.boss, s.acme, &"me").await.problem();
    assert_eq!(refused.code, ErrorCode::OwnerLast);
    let admin = person(&world, "adm").await;
    join(&world, s.acme, s.acme, &admin, "admin", &s.boss).await;
    let refused = remove(&world, &admin, s.acme, &s.boss.id()).await.problem();
    assert_eq!(refused.code, ErrorCode::OwnerLast);
    assert_eq!(members(&world, &s.boss, s.acme).await.len(), 3);
    // Eve is made an owner first, and then Boss may go: Eve is first.
    world
        .app
        .call(NodeTransfer, &[&s.acme])
        .token(&s.boss.token)
        .body(&TransferBody {
            user_id: s.eve.id(),
            keep_previous: Some(true),
        })
        .send()
        .await
        .ok();
    let left = remove(&world, &s.boss, s.acme, &"me").await;
    assert_eq!(left.status, 204);
    let acme = read(&world, &s.eve, s.acme).await;
    assert_eq!(owner_of(&acme), ("eve", 1));
}

#[tokio::test]
async fn the_first_owner_who_leaves_hands_the_first_place_to_the_one_longest_in_the_row() {
    let world = World::new().await;
    let s = setup(&world).await;
    let owned = eve_owns(&world, &s).await;
    // Shared has Eve first and Boss second: Boss is first after her.
    let before = read(&world, &s.boss, owned.shared).await;
    assert_eq!(owner_of(&before), ("eve", 2));
    remove(&world, &s.boss, s.acme, &s.eve.id()).await.ok();
    let after = read(&world, &s.boss, owned.shared).await;
    assert_eq!(owner_of(&after), ("boss", 1));
}

#[tokio::test]
async fn whoever_manages_the_members_at_the_domain_or_above_it_ends_a_membership_and_nobody_else_does()
 {
    let world = World::new().await;
    let s = setup(&world).await;
    let fay = person(&world, "fay").await;
    join(&world, s.acme, s.sales, &fay, "admin", &s.boss).await;
    // An admin of Sales manages nothing in Research.
    let refused = remove(&world, &fay, s.acme, &s.eve.id()).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["object"]["id"], serde_json::json!(s.research));
    assert_eq!(members(&world, &s.boss, s.acme).await.len(), 4);
    // An admin of Research does, and so does one of the root.
    let gus = person(&world, "gus").await;
    join(&world, s.acme, s.research, &gus, "admin", &s.boss).await;
    assert_eq!(remove(&world, &gus, s.acme, &s.eve.id()).await.status, 204);
    let hal = person(&world, "hal").await;
    join(&world, s.acme, s.research, &hal, "viewer", &s.boss).await;
    let root_admin = person(&world, "adm").await;
    join(&world, s.acme, s.acme, &root_admin, "admin", &s.boss).await;
    assert_eq!(
        remove(&world, &root_admin, s.acme, &hal.id()).await.status,
        204
    );
    // The request names the org by any of the tree.
    let ira = person(&world, "ira").await;
    join(&world, s.acme, s.research, &ira, "viewer", &s.boss).await;
    assert_eq!(
        remove(&world, &s.boss, s.research, &ira.id()).await.status,
        204
    );
}

#[tokio::test]
async fn a_member_moves_to_another_org_of_the_tree_with_the_role_they_held_and_what_names_them_stays()
 {
    let world = World::new().await;
    let s = setup(&world).await;
    let owned = eve_owns(&world, &s).await;
    let sales_project = make_project(&world, &s.boss, s.sales, "Pipeline").await;
    give_role(&world, s.web, &s.eve, "viewer", &s.boss).await;
    let plan = preview(&world, &s.boss, s.acme, s.eve.id(), &to(s.sales, None))
        .await
        .ok();
    assert_eq!(plan.user.username, "eve");
    assert_eq!((plan.from.id, plan.to.id), (s.research, s.sales));
    assert_eq!(
        plan.role,
        Role::Developer,
        "the role they held, when none is asked for"
    );
    // Research holds Research, Lab, Run, Shared, Sub and Deep; Eve owns
    // Lab, Run, Sub and Deep in them, which she reaches as their owner.
    assert!(plan.loses.nodes >= 1, "{:?}", plan.loses);
    assert_eq!(plan.gains.nodes, 2, "Sales itself and Pipeline");
    assert_eq!(
        (
            plan.stays.nodes.orgs,
            plan.stays.nodes.projects,
            plan.stays.nodes.topics
        ),
        (1, 3, 1),
        "Sub, Lab, Shared and Deep, and the topic Run, stay hers where they are"
    );
    assert_eq!(plan.stays.roles.len(), 1, "the viewer role at Web");
    assert!(plan.waiting.is_empty() && plan.missing.is_empty());
    // Nothing happened by planning.
    assert_eq!(
        members(&world, &s.boss, s.research).await.len(),
        1,
        "Eve is still in Research"
    );
    let moved = move_to(&world, &s.boss, s.acme, s.eve.id(), &to(s.sales, None))
        .await
        .ok();
    assert_eq!(moved.domain.id, s.sales);
    assert_eq!(moved.role, Role::Developer);
    assert_eq!(moved.address, "acme.sales.eve");
    let notices: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT data FROM notifications
        WHERE user_id = $1 AND kind = 'member.moved'",
    )
    .bind(s.eve.id())
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0]["from"], json!(s.research));
    assert_eq!(notices[0]["to"], json!(s.sales));
    assert!(members(&world, &s.boss, s.research).await.is_empty());
    // She reaches Sales now, keeps what is hers where it is, and keeps the role named at Web.
    assert!(matches!(
        read(&world, &s.eve, sales_project.base.id).await,
        Node::Project(_)
    ));
    assert_eq!(owner_of(&read(&world, &s.eve, owned.lab).await), ("eve", 1));
    assert!(matches!(
        read(&world, &s.eve, s.web).await,
        Node::Project(_)
    ));
    let audit = world.audit().await;
    assert!(audit.iter().any(|action| action == "member.moved"));
    // The role at the old domain is gone and the new one is the same role.
    let roles = world
        .app
        .call(NodeRoleList, &[&s.research])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert!(roles.own.iter().all(|role| {
        role.subject
            .user
            .as_ref()
            .is_none_or(|user| user.username != "eve")
    }));
}

#[tokio::test]
async fn the_role_asked_for_is_given_unless_they_hold_a_higher_one_at_the_new_domain_already() {
    let world = World::new().await;
    let s = setup(&world).await;
    // Another role than theirs.
    let plan = preview(
        &world,
        &s.boss,
        s.acme,
        s.eve.id(),
        &to(s.sales, Some(Role::Viewer)),
    )
    .await
    .ok();
    assert_eq!(plan.role, Role::Viewer);
    let admin_plan = preview(
        &world,
        &s.boss,
        s.acme,
        s.eve.id(),
        &to(s.sales, Some(Role::Admin)),
    )
    .await
    .ok();
    assert_eq!(admin_plan.role, Role::Admin);
    // Admin at Sales by name already: the higher stays.
    give_role(&world, s.sales, &s.eve, "admin", &s.boss).await;
    let kept = preview(
        &world,
        &s.boss,
        s.acme,
        s.eve.id(),
        &to(s.sales, Some(Role::Viewer)),
    )
    .await
    .ok();
    assert_eq!(kept.role, Role::Admin);
    let moved = move_to(
        &world,
        &s.boss,
        s.acme,
        s.eve.id(),
        &to(s.sales, Some(Role::Viewer)),
    )
    .await
    .ok();
    assert_eq!(moved.role, Role::Admin);
    // Back to the root as an Admin, by an owner: the plan says the role, and it is given.
    let back = move_to(
        &world,
        &s.boss,
        s.acme,
        s.eve.id(),
        &to(s.acme, Some(Role::Admin)),
    )
    .await
    .ok();
    assert_eq!((back.domain.id, back.role), (s.acme, Role::Admin));
}

#[tokio::test]
async fn a_move_is_refused_to_the_same_domain_to_another_tree_and_to_what_is_no_org() {
    let world = World::new().await;
    let s = setup(&world).await;
    let beta = make_org(&world, &s.boss, "Beta", "beta").await;
    let beta_project = make_project(&world, &s.boss, beta.base.id, "Other").await;
    for (target, code, why) in [
        (
            s.research,
            ErrorCode::MemberSameDomain,
            "their domain already",
        ),
        (
            beta.base.id,
            ErrorCode::NodeWrongParent,
            "an org of another tree",
        ),
        (
            beta_project.base.id,
            ErrorCode::NodeWrongParent,
            "a project of another tree",
        ),
        (s.web, ErrorCode::NodeWrongParent, "a project of this tree"),
    ] {
        let body = to(target, None);
        assert_eq!(
            preview(&world, &s.boss, s.acme, s.eve.id(), &body)
                .await
                .problem()
                .code,
            code,
            "preview: {why}"
        );
        assert_eq!(
            move_to(&world, &s.boss, s.acme, s.eve.id(), &body)
                .await
                .problem()
                .code,
            code,
            "move: {why}"
        );
    }
    let nowhere = move_to(
        &world,
        &s.boss,
        s.acme,
        s.eve.id(),
        &to(orch_types::new_id(), None),
    )
    .await
    .problem();
    assert_eq!(nowhere.code, ErrorCode::NotFound);
    let nobody = person(&world, "nobody").await;
    let nothing = move_to(&world, &s.boss, s.acme, nobody.id(), &to(s.sales, None))
        .await
        .problem();
    assert_eq!(
        nothing.code,
        ErrorCode::NotFound,
        "somebody who is no member moves nowhere"
    );
}

#[tokio::test]
async fn a_move_takes_the_right_at_both_ends_and_the_plan_says_what_lacks_at_the_other_one() {
    let world = World::new().await;
    let s = setup(&world).await;
    let fay = person(&world, "fay").await;
    join(&world, s.acme, s.sales, &fay, "admin", &s.boss).await;
    // A viewer neither plans nor moves.
    let refused = preview(&world, &s.dan, s.acme, s.eve.id(), &to(s.sales, None))
        .await
        .problem();
    assert_eq!(refused.extra["required"], "members.manage");
    let refused = move_to(&world, &s.dan, s.acme, s.eve.id(), &to(s.sales, None))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&refused),
        vec!["members.manage".to_owned()]
    );
    assert_eq!(first_missing(&refused)["ask"][0]["username"], "boss");
    // An admin of Sales holds the right at the end Eve would join only: the
    // plan answers, and says what is lacking at Research and who holds it.
    let plan = preview(&world, &fay, s.acme, s.eve.id(), &to(s.sales, None))
        .await
        .ok();
    assert_eq!(plan.missing.len(), 1);
    assert_eq!(plan.missing[0].object.id, s.research);
    assert_eq!(
        plan.missing[0].permissions,
        vec!["members.manage".to_owned()]
    );
    assert!(
        plan.missing[0]
            .ask
            .iter()
            .any(|user| user.username == "boss")
    );
    let refused = move_to(&world, &fay, s.acme, s.eve.id(), &to(s.sales, None))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["object"]["id"], serde_json::json!(s.research));
    assert_eq!(
        members(&world, &s.boss, s.sales).await.len(),
        1,
        "only Fay is in Sales: nothing moved"
    );
}

#[tokio::test]
async fn the_holdings_and_the_plan_cost_the_same_statements_whatever_the_person_holds() {
    let world = World::new().await;
    let s = setup(&world).await;
    async fn measure(world: &World, s: &Setup) -> (usize, usize, i64, usize) {
        let before = Statements::now();
        let held = holdings(world, &s.boss, s.acme, &s.eve.id()).await.ok();
        let middle = Statements::now();
        let plan = preview(world, &s.boss, s.acme, s.eve.id(), &to(s.sales, None))
            .await
            .ok();
        let after = Statements::now();
        (
            middle - before,
            after - middle,
            held.entries,
            plan.stays.roles.len(),
        )
    }
    give_role(&world, s.web, &s.eve, "viewer", &s.boss).await;
    let (few_held, few_plan, entries, roles) = measure(&world, &s).await;
    assert_eq!((entries, roles), (2, 1));
    for index in 0..4 {
        let project = make_project(&world, &s.boss, s.acme, &format!("More {index}")).await;
        give_role(&world, project.base.id, &s.eve, "viewer", &s.boss).await;
        let hers = make_project(&world, &s.boss, s.research, &format!("Hers {index}")).await;
        hand_to_eve(&world, &s, hers.base.id, false).await;
    }
    let (many_held, many_plan, entries, roles) = measure(&world, &s).await;
    assert_eq!((entries, roles), (6, 5));
    assert_eq!(
        few_held, many_held,
        "{few_held} statements with 2 entries, {many_held} with 6"
    );
    assert_eq!(
        few_plan, many_plan,
        "{few_plan} statements with 1 role elsewhere, {many_plan} with 5"
    );
}
