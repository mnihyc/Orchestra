//! What people tell the threads of a topic or of a project, listed and made through the API; and a capability of an environment asked for by an agent, decided by the people of its scope and taken back by whoever manages the environment.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::approvals::{
    Approval, ApprovalDecide, ApprovalDecision, ApprovalList, ApprovalListQuery, ApprovalState,
    CapabilityDecision,
};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::environment::EnvironmentCapabilityRevoke;
use orch_api_contract::threads::{AnnouncementCategory, Delivery};
use orch_api_contract::topics::{
    Announcement, AnnouncementCreateBody, AnnouncementDelivery, AnnouncementSendAudience,
    AnnouncementTargets, AnnouncementTo, NodeAnnouncementCreate, NodeAnnouncementList,
    NodeAnnouncementListQuery, PlaceAnnouncementCreateReply,
};
use orch_link_contract::commands::{Command, CommandKind, OrchTool};
use orch_link_contract::events::{Event, OrchToolCall};
use orch_types::{Id, new_id};
use places::Who;
use rig::Scene;
use serde_json::json;

/// Starts a turn of `thread` and has the harness begin it: what an agent calls acts only for the turn
/// the thread runs now.
async fn running_turn(s: &Scene, thread: Id) -> Id {
    s.say(&s.ada, thread, "work", Delivery::Queue, "work")
        .await
        .ok();
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, rig::turn_began(start.turn_id)).await;
    s.rig.wait_turn(start.turn_id, "running").await;
    start.turn_id
}

fn announcement(title: &str) -> AnnouncementCreateBody {
    AnnouncementCreateBody {
        title: title.to_owned(),
        body: "Details follow.".to_owned(),
        category: None,
        severity: None,
        targets: None,
        delivery: None,
        fallback: None,
        wake_idle: None,
    }
}

async fn tell_the_topic(s: &Scene, by: &Who, body: &AnnouncementCreateBody) -> Announcement {
    s.rig
        .world
        .app
        .call(NodeAnnouncementCreate, &[&s.tree.topic.base.id])
        .token(&by.token)
        .body(body)
        .send()
        .await
        .ok()
        .announcements
        .into_iter()
        .next()
        .unwrap()
}

async fn listed(s: &Scene, by: &Who, query: &NodeAnnouncementListQuery) -> Page<Announcement> {
    s.rig
        .world
        .app
        .call(NodeAnnouncementList, &[&s.tree.topic.base.id])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_person_who_may_start_work_in_a_topic_tells_its_threads_something_and_the_list_shows_what_it_did()
 {
    let s = Scene::new().await;
    let (first, second) = (
        s.thread_by(&s.ada, "Build").await,
        s.thread_by(&s.ada, "Test").await,
    );
    let made = tell_the_topic(&s, &s.ada, &announcement("The build moved")).await;
    assert_eq!(made.title, "The build moved");
    assert_eq!(made.category, AnnouncementCategory::Notice);
    assert_eq!(
        made.targets,
        AnnouncementTargets::Audience(orch_api_contract::topics::AnnouncementAudience::All)
    );
    assert_eq!(
        made.delivery,
        AnnouncementDelivery::Steer,
        "the topic's setting, as shipped"
    );
    // Both threads are idle, so both are told at their next turn.
    assert_eq!(made.delivered.deferred, 2);
    for thread in [first.base.id, second.base.id] {
        let lines: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'announcement'",
        )
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
        assert_eq!(lines, 1);
    }
    let page = listed(&s, &s.ada, &NodeAnnouncementListQuery::default()).await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, made.id);
    assert_eq!(
        page.items[0].delivered, made.delivered,
        "counted from what it left in the threads"
    );
    assert!(page.next_cursor.is_none());
}

#[tokio::test]
async fn somebody_who_cannot_start_work_in_the_topic_is_refused_and_told_what_they_lack() {
    let s = Scene::new().await;
    let _ = s.thread_by(&s.ada, "Build").await;
    let refused = s
        .rig
        .world
        .app
        .call(NodeAnnouncementCreate, &[&s.tree.topic.base.id])
        .token(&s.cai.token)
        .body(&announcement("Hello"))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(places::told(&refused)["required"], "topic.interact");
    let nothing: i64 = sqlx::query_scalar("SELECT count(*) FROM announcements")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(nothing, 0);
}

#[tokio::test]
async fn an_announcement_to_the_active_threads_leaves_the_others_without_even_a_notice() {
    let s = Scene::new().await;
    let (working, idle) = (
        s.thread_by(&s.ada, "Working").await,
        s.thread_by(&s.ada, "Idle").await,
    );
    let sent = s
        .say(
            &s.ada,
            working.base.id,
            "Go",
            orch_api_contract::threads::Delivery::Queue,
            "go",
        )
        .await;
    assert!(sent.status.is_success());
    let start = s.rig.turn_start(working.base.id, 0).await;
    s.rig
        .say(working.base.id, rig::turn_began(start.turn_id))
        .await;
    s.rig.wait_state(working.base.id, "running").await;
    let mut body = announcement("Only for those at work");
    body.targets = Some(AnnouncementSendAudience::Active);
    body.delivery = Some(AnnouncementDelivery::NextTurn);
    let made = tell_the_topic(&s, &s.ada, &body).await;
    assert_eq!(made.delivered.queued, 1);
    assert_eq!(made.delivered.deferred, 0);
    let notices: serde_json::Value =
        sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id = $1")
            .bind(idle.base.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(notices, json!([]));
}

#[tokio::test]
async fn a_project_announcement_needs_interact_and_goes_to_every_open_topic() {
    let s = Scene::new().await;
    let _ = s.thread_by(&s.ada, "Build").await;
    let other = places::make_topic(&s.rig.world, &s.ada, s.tree.project.base.id, "Second").await;
    let done = places::make_topic(&s.rig.world, &s.ada, s.tree.project.base.id, "Finished").await;
    sqlx::query("UPDATE topics SET status = 'done' WHERE node_id = $1")
        .bind(done.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let project = s.tree.project.base.id;
    let refused = s
        .rig
        .world
        .app
        .call(NodeAnnouncementCreate, &[&project])
        .token(&s.cai.token)
        .body(&announcement("To the project"))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(places::told(&refused)["required"], "project.interact");

    let made: PlaceAnnouncementCreateReply = s
        .rig
        .world
        .app
        .call(NodeAnnouncementCreate, &[&project])
        .token(&s.ada.token)
        .body(&announcement("To the project"))
        .send()
        .await
        .ok();
    assert_eq!(
        made.announcements.len(),
        2,
        "the topic with a thread and the open one without"
    );
    assert_eq!(made.topics_skipped, 1, "the finished topic is left out");
    assert!(
        made.announcements
            .iter()
            .all(|each| each.group_id == Some(made.group_id))
    );
    let topics: Vec<Id> = made
        .announcements
        .iter()
        .map(|each| each.topic_id)
        .collect();
    assert!(topics.contains(&s.tree.topic.base.id) && topics.contains(&other.base.id));

    let lines: Page<Announcement> = s
        .rig
        .world
        .app
        .call(NodeAnnouncementList, &[&project])
        .token(&s.ada.token)
        .query(&NodeAnnouncementListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        lines.items.len(),
        2,
        "one delivery row for each readable topic"
    );
    assert!(
        lines
            .items
            .iter()
            .all(|line| line.group_id == Some(made.group_id))
    );
    assert_eq!(
        lines
            .items
            .iter()
            .map(|line| line.delivered.deferred)
            .sum::<i64>(),
        1
    );
}

#[tokio::test]
async fn what_an_agent_announced_to_one_thread_or_to_followers_is_listed_only_when_everything_is_asked_for()
 {
    let s = Scene::new().await;
    s.rig.tools_of_agents(json!("mcp")).await;
    let (build, _test) = (
        s.thread_by(&s.ada, "Build").await,
        s.thread_by(&s.ada, "Test").await,
    );
    let turn = running_turn(&s, build.base.id).await;
    let call_ref = new_id().to_string();
    s.rig
        .say(
            build.base.id,
            Event::OrchToolCall(OrchToolCall {
                turn_id: turn,
                call_ref: call_ref.clone(),
                attempt: None,
                tool: OrchTool::Announce,
                arguments: json!({ "title": "Built", "category": "milestone" }),
            }),
        )
        .await;
    rig::eventually("the answer", || {
        let seen = s
            .rig
            .seen
            .of_kind(CommandKind::OrchToolResult)
            .into_iter()
            .any(|command| matches!(command, Command::OrchToolResult(result) if result.call_ref == call_ref));
        async move { seen }
    })
    .await;
    let threads_only = listed(&s, &s.ada, &NodeAnnouncementListQuery::default()).await;
    assert!(
        threads_only.items.is_empty(),
        "a milestone that only followers heard of"
    );
    let everything = listed(
        &s,
        &s.ada,
        &NodeAnnouncementListQuery {
            to: Some(AnnouncementTo::All),
            ..NodeAnnouncementListQuery::default()
        },
    )
    .await;
    assert_eq!(everything.items.len(), 1);
    assert_eq!(
        everything.items[0].category,
        AnnouncementCategory::Milestone
    );
    assert_eq!(
        everything.items[0].targets,
        AnnouncementTargets::Audience(orch_api_contract::topics::AnnouncementAudience::Nobody)
    );
    // Somebody who may not view the thread it came from does not see it among everything.
    let stranger = listed(
        &s,
        &s.cai,
        &NodeAnnouncementListQuery {
            to: Some(AnnouncementTo::All),
            ..NodeAnnouncementListQuery::default()
        },
    )
    .await;
    assert_eq!(
        stranger.items.len(),
        1,
        "Cai may read the threads of the org"
    );
}

// ---------------------------------------------------------------- capabilities

/// The scene's environment, made from a template that allows `kvm` and `ptrace`, and the catalogue.
async fn with_capabilities(s: &Scene) {
    // A prospective unit learns its capabilities from an available driver's report. No host is
    // selected yet, so this report gives it no physical machine's policy or host manager.
    let mut conn = s.rig.db.acquire().await.unwrap();
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        None,
        None,
        "Capability host",
    )
    .await
    .unwrap();
    let mut descriptor = serde_json::to_value(
        orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
            "1.0.0",
            orch_driver_contract::descriptor::EnvironmentDataStorage {
                made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Volume),
                existing_directory: false,
                reason: None,
            },
        ),
    )
    .unwrap();
    descriptor["name"] = json!("docker");
    descriptor["lifecycle"] = json!("unit");
    descriptor["confinement"] = json!("isolated");
    descriptor["capabilities"] = json!({"ptrace":{},"kvm":{}});
    sqlx::query("UPDATE hosts SET drivers=jsonb_build_object('docker',$2::jsonb) WHERE id=$1")
        .bind(host)
        .bind(descriptor)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    for (key, label, decision) in [
        ("ptrace", "Debugger access", "allow"),
        ("kvm", "KVM virtualisation", "ask"),
    ] {
        sqlx::query(
            "INSERT INTO capability_defs (key, label, description, risk, driver_patch, default_decision, default_approver_scope)
             VALUES ($1, $2, 'A thing.', 'high', '{\"docker\": {\"cap_add\": [\"X\"]}}', $3, 'host')",
        )
        .bind(key)
        .bind(label)
        .bind(decision)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE env_templates SET driver = 'docker',
                spec = '{\"allowed_capabilities\": [\"ptrace\", \"kvm\"], \"base_capabilities\": []}'
          WHERE id = (SELECT template_id FROM environments WHERE id = $1)",
    )
    .bind(s.on.environment)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    s.rig.tools_of_agents(json!("mcp")).await;
}

async fn pending_for(s: &Scene, who: &Who) -> Page<Approval> {
    s.rig
        .world
        .app
        .call(ApprovalList, &[])
        .token(&who.token)
        .query(&ApprovalListQuery::default())
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_request_for_a_capability_is_listed_for_the_people_of_its_scope_and_decided_by_them_alone()
 {
    let s = Scene::new().await;
    with_capabilities(&s).await;
    let thread = s.thread().await.base.id;
    // Whoever decides is there before the agent asks: a change of who may do what while the
    // machine's events are applied refuses them, and the machine sends them again only after its
    // stall time.
    let admin = places::admin(&s.rig.world, "root").await;
    let turn = running_turn(&s, thread).await;
    let call_ref = new_id().to_string();
    s.rig
        .say(
            thread,
            Event::OrchToolCall(OrchToolCall {
                turn_id: turn,
                call_ref: call_ref.clone(),
                attempt: None,
                tool: OrchTool::RequestCapability,
                arguments: json!({ "capability": "kvm", "reason": "I run virtual machines for tests" }),
            }),
        )
        .await;
    rig::eventually("the request is raised", || async {
        !pending_for(&s, &admin).await.items.is_empty()
    })
    .await;
    let waiting = pending_for(&s, &admin).await;
    assert_eq!(waiting.items.len(), 1);
    let approval = &waiting.items[0];
    assert!(
        approval.can_decide,
        "the system's admin decides what is decided on a machine's side"
    );
    assert_eq!(approval.thread_id, Some(thread));
    // The owner of the thread, the topic and the environment is neither sent it nor allowed to decide it.
    assert!(pending_for(&s, &s.ada).await.items.is_empty());
    let refused = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&approval.id])
        .token(&s.ada.token)
        .body(&ApprovalDecision::Capability(CapabilityDecision {
            approve: true,
            note: None,
        }))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let decided = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&approval.id])
        .token(&admin.token)
        .body(&ApprovalDecision::Capability(CapabilityDecision {
            approve: true,
            note: Some("fine".to_owned()),
        }))
        .send()
        .await
        .ok();
    assert_eq!(decided.state, ApprovalState::Approved);
    let granted: Vec<String> =
        sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id = $1")
            .bind(s.on.environment)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(granted, ["kvm"]);
}

#[tokio::test]
async fn whoever_manages_an_environment_takes_a_capability_back_and_before_its_first_unit_no_job_is_needed()
 {
    let s = Scene::new().await;
    with_capabilities(&s).await;
    sqlx::query("UPDATE environments SET granted_capabilities = ARRAY['ptrace'] WHERE id = $1")
        .bind(s.on.environment)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let refused = s
        .rig
        .world
        .app
        .call(EnvironmentCapabilityRevoke, &[&s.on.environment, &"ptrace"])
        .token(&s.cai.token)
        .send()
        .await
        .problem();
    assert!(
        matches!(refused.code, ErrorCode::AuthzDenied | ErrorCode::NotFound),
        "{refused:?}"
    );
    let job = s
        .rig
        .world
        .app
        .call(EnvironmentCapabilityRevoke, &[&s.on.environment, &"ptrace"])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(job.status.as_u16(), 202);
    // The scene's environment has no unit yet: what it is given applies when its first unit is
    // made, and there is no unit to make anew.
    assert!(job.ok().is_none(), "there is no unit to replace");
    let granted: Vec<String> =
        sqlx::query_scalar("SELECT granted_capabilities FROM environments WHERE id = $1")
            .bind(s.on.environment)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert!(granted.is_empty());
    let again = s
        .rig
        .world
        .app
        .call(EnvironmentCapabilityRevoke, &[&s.on.environment, &"ptrace"])
        .token(&s.ada.token)
        .send()
        .await
        .problem();
    assert_eq!(
        again.code,
        ErrorCode::NotFound,
        "nothing is there to take back"
    );
}
