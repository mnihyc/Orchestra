//! What happens in a thread reaches the right people end to end: an approval is in the Inbox and the badge of whoever can decide it and of nobody else, a turn that failed is in its owner's Inbox with the link that opens it, opening a thread reads what it told, and a destination at a topic is sent what happens below it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod outside;
mod places;
mod rig;
mod support;

use orch_api_contract::approvals::{ApprovalDecide, ApprovalDecision, HarnessOptionDecision};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{CreateThread, ThreadCreate};
use orch_api_contract::notifications::{
    NodeDestinationCreate, NodeDestinationCreateBody, Notification, NotificationGrouping,
    NotificationKind, NotificationList, NotificationListQuery, NotificationRead,
    NotificationReadBody, NotificationSummaryGet,
};
use orch_api_contract::threads::Delivery;
use orch_db::queries::hierarchy;
use orch_link_contract::commands::TurnStart;
use orch_link_contract::events::TurnOutcome;
use orch_types::Id;
use places::Who;
use rig::{Scene, command_asked, turn_began, turn_ended};
use serde_json::json;
use std::collections::BTreeMap;

async fn terminal_followup(s: &Scene, turn: Id) {
    rig::eventually("the terminal notification transaction", || async {
        sqlx::query_scalar::<_, bool>("SELECT end_work IS NULL FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap()
    })
    .await;
}

async fn summary(s: &Scene, who: &Who) -> (i64, i64) {
    let counted = s
        .rig
        .world
        .app
        .call(NotificationSummaryGet, &[])
        .token(&who.token)
        .send()
        .await
        .ok();
    (counted.unread, counted.approvals_pending)
}

async fn inbox(s: &Scene, who: &Who, kind: NotificationKind) -> Vec<Notification> {
    s.rig
        .world
        .app
        .call(NotificationList, &[])
        .token(&who.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok()
        .items
        .into_iter()
        .filter(|each| each.kind == kind)
        .collect()
}

/// A turn that runs and has asked whether a command may run.
async fn asking(s: &Scene, thread: Id, request_ref: &str) -> TurnStart {
    let sent = s
        .say(&s.ada, thread, "Clean up", Delivery::Queue, "ask")
        .await;
    assert!(sent.status.is_success());
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, turn_began(start.turn_id)).await;
    s.rig
        .say(thread, command_asked(start.turn_id, request_ref))
        .await;
    s.rig.wait_state(thread, "waiting_approval").await;
    start
}

#[tokio::test]
async fn an_approval_is_in_the_inbox_and_the_badge_of_whoever_can_decide_it_and_of_nobody_else() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    // Cai may read the thread and follows it at all, and cannot decide what its agent asks.
    {
        let mut conn = s.rig.world.conn().await;
        hierarchy::put_subscription(
            &mut conn,
            s.cai.id(),
            thread,
            "all",
            "manual",
            s.rig.world.now(),
        )
        .await
        .unwrap();
    }
    asking(&s, thread, "r1").await;

    for who in [&s.ada, &s.eve, &s.dan] {
        let told = inbox(&s, who, NotificationKind::ApprovalRequested).await;
        assert_eq!(told.len(), 1, "{}", who.person.username);
        assert_eq!(told[0].thread_id, Some(thread));
        // It opens the approval itself, where it is decided.
        let approval = told[0].approval_id.unwrap();
        assert_eq!(told[0].url, format!("/approvals?approval={approval}"));
        assert_eq!(told[0].title, "Fix login asks for approval");
        assert_eq!(summary(&s, who).await, (1, 1), "{}", who.person.username);
    }
    for who in [&s.cai, &s.bea] {
        assert!(
            inbox(&s, who, NotificationKind::ApprovalRequested)
                .await
                .is_empty(),
            "{}",
            who.person.username
        );
        assert_eq!(summary(&s, who).await.1, 0, "nothing for them to decide");
    }

    // Eve answers it; for everybody it is done and nothing waits.
    let approval = inbox(&s, &s.ada, NotificationKind::ApprovalRequested).await[0]
        .approval_id
        .unwrap();
    {
        let mut conn = s.rig.world.conn().await;
        sqlx::query("UPDATE users SET display_name='Eve Reviewer' WHERE id=$1")
            .bind(s.eve.id())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let decided = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&approval])
        .token(&s.eve.token)
        .body(&ApprovalDecision::Option(HarnessOptionDecision {
            option_id: "allow_once".to_owned(),
            note: None,
        }))
        .send()
        .await;
    assert!(decided.status.is_success(), "{:?}", decided.body);
    for who in [&s.ada, &s.eve, &s.dan] {
        assert_eq!(summary(&s, who).await, (0, 0), "{}", who.person.username);
        let told = inbox(&s, who, NotificationKind::ApprovalRequested).await;
        assert!(told[0].read_at.is_some(), "the row stays, read");
        assert_eq!(
            told[0].actor.as_ref().map(|person| person.id),
            Some(s.eve.id()),
            "the row now names its actual decider"
        );
        assert_eq!(
            told[0]
                .actor
                .as_ref()
                .map(|person| person.display_name.as_str()),
            Some("Eve Reviewer"),
            "current display name rather than the username or an old stored name"
        );
        assert_eq!(told[0].data["approval_state"], "approved");
        let details = told[0]
            .details
            .as_ref()
            .expect("the same notice carries current action facts");
        assert_eq!(details.parameters["approval_state"], "approved");
        assert_eq!(details.parameters["decided_by"], json!(s.eve.id()));
        assert!(
            !told[0].waiting_for_me,
            "a decided row no longer asks for a decision"
        );
    }
    let grouped = s
        .rig
        .world
        .app
        .call(NotificationList, &[])
        .token(&s.ada.token)
        .query(&NotificationListQuery {
            group_by: Some(NotificationGrouping::Place),
            ..NotificationListQuery::default()
        })
        .send()
        .await
        .ok();
    let same = grouped
        .items
        .iter()
        .find(|row| row.approval_id == Some(approval))
        .expect("grouping keeps the same settled notice");
    assert_eq!(
        same.actor.as_ref().map(|person| person.id),
        Some(s.eve.id())
    );
    assert_eq!(same.data["approval_state"], "approved");
    assert!(!same.waiting_for_me);
    let mut conn = s.rig.world.conn().await;
    let stored: orch_types::Json = sqlx::query_scalar("SELECT data FROM notifications WHERE user_id=$1 AND approval_id=$2 AND kind='approval.requested'")
        .bind(s.ada.id()).bind(approval).fetch_one(&mut *conn).await.unwrap();
    assert!(
        stored.get("approval_state").is_none(),
        "a notification read projects the approval's current state without writing historical rows"
    );
}

#[tokio::test]
async fn a_turn_that_failed_is_in_the_inbox_of_its_owner_with_the_link_that_opens_it_and_opening_the_thread_reads_it()
 {
    let s = Scene::new().await;
    let thread = s.thread().await;
    let id = thread.base.id;
    let sent = s.say(&s.ada, id, "Fix it", Delivery::Queue, "go").await;
    assert!(sent.status.is_success());
    let start = s.rig.turn_start(id, 0).await;
    s.rig.say(id, turn_began(start.turn_id)).await;
    s.rig
        .say(id, turn_ended(start.turn_id, TurnOutcome::Failed))
        .await;
    s.rig.wait_state(id, "idle").await;
    terminal_followup(&s, start.turn_id).await;

    let failed = inbox(&s, &s.ada, NotificationKind::TurnFailed).await;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].title, "Fix login: turn failed");
    assert_eq!(failed[0].thread_id, Some(id));
    assert_eq!(failed[0].url, format!("/thread/{id}"));
    assert_eq!(
        failed[0]
            .node
            .as_ref()
            .and_then(|node| node.name.as_deref()),
        Some("Fix login")
    );
    assert_eq!(summary(&s, &s.ada).await, (1, 0));
    assert!(
        inbox(&s, &s.eve, NotificationKind::TurnFailed)
            .await
            .is_empty(),
        "Eve follows nothing here"
    );

    // Opening the thread: the client reads what it told.
    let read = s
        .rig
        .world
        .app
        .call(NotificationRead, &[])
        .token(&s.ada.token)
        .body(&NotificationReadBody {
            node_id: Some(id),
            ..NotificationReadBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(read.unread, 0);
    assert!(
        inbox(&s, &s.ada, NotificationKind::TurnFailed).await[0]
            .read_at
            .is_some()
    );
}

#[tokio::test]
async fn a_destination_at_the_topic_is_sent_what_fails_below_it_and_cannot_be_set_at_a_thread() {
    let s = Scene::new().await;
    outside::install();
    let thread = s.thread().await.base.id;
    let far = outside::FarSide::taking().await;
    s.rig
        .world
        .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
        .await;
    let body = NodeDestinationCreateBody {
        channel: None,
        name: "Ops room".to_owned(),
        config: json!({ "url": far.url }),
        secrets: Some(BTreeMap::new()),
        templates: None,
        kinds: None,
        level: None,
        enabled: None,
    };
    let at_topic = s
        .rig
        .world
        .app
        .call(NodeDestinationCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let at_thread = s
        .rig
        .world
        .app
        .call(NodeDestinationCreate, &[&thread])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(
        at_thread.code,
        ErrorCode::RequestInvalid,
        "destinations are set at an org, a project or a topic"
    );

    let sent = s.say(&s.ada, thread, "Fix it", Delivery::Queue, "go").await;
    assert!(sent.status.is_success());
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, turn_began(start.turn_id)).await;
    s.rig
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Failed))
        .await;
    s.rig.wait_state(thread, "idle").await;
    terminal_followup(&s, start.turn_id).await;

    let rows: Vec<(Option<Id>, serde_json::Value)> = sqlx::query_as(
        "SELECT destination_id, payload FROM outbox WHERE destination_id IS NOT NULL",
    )
    .fetch_all(&s.rig.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "a failure is for the level it was set at, once"
    );
    assert_eq!(rows[0].0, Some(at_topic.id));
    assert_eq!(rows[0].1["notifications"][0]["kind"], "turn.failed");
    assert_eq!(rows[0].1["notifications"][0]["id"], serde_json::Value::Null);
}

#[tokio::test]
async fn a_thread_made_over_http_is_followed_by_its_maker() {
    let s = Scene::new().await;
    let thread = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.eve.token)
        .body(&CreateThread {
            name: Some("Eve's".to_owned()),
            binding: Some(orch_api_contract::nodes::CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: s.on.provider,
                arrangement: None,
            }),
            model: Some(rig::MODEL.to_owned()),
            environment: Some(orch_api_contract::binding::EnvironmentChoice::Use(
                s.on.environment,
            )),
            directory: Some("web".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    let level: Option<(String, String)> = sqlx::query_as(
        "SELECT level, source FROM subscriptions WHERE user_id = $1 AND node_id = $2",
    )
    .bind(s.eve.id())
    .bind(thread.base.id)
    .fetch_optional(&s.rig.world.db.pool)
    .await
    .unwrap();
    assert_eq!(level, Some(("all".to_owned(), "auto".to_owned())));
}
