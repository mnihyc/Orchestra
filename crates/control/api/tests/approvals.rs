//! What a harness asks before it acts waits as an approval: listed for those who can decide it and for those who may read its thread, read, decided by whoever manages the thread (the first decision wins), and handed to the harness once; and a request for access is taken back by the person who asked.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::approvals::{
    Approval, ApprovalCancel, ApprovalDecide, ApprovalDecision, ApprovalGet, ApprovalKind,
    ApprovalList, ApprovalListQuery, ApprovalListState, ApprovalRequester, ApprovalState,
    HarnessAnswersDecision, HarnessOptionDecision,
};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::threads::{ThreadState, ThreadSyncGet, ThreadSyncGetQuery};
use orch_link_contract::commands::CommandKind;
use orch_types::{Id, new_id};
use places::{Who, missing_permissions};
use rig::{Scene, command_asked, message_began, message_grew, turn_began};
use serde_json::json;

fn allow() -> ApprovalDecision {
    ApprovalDecision::Option(HarnessOptionDecision {
        option_id: "allow_once".to_owned(),
        note: None,
    })
}

#[tokio::test]
async fn an_environments_ports_manager_decides_port_place_but_its_thread_manager_cannot() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::ERROR)
        .try_init();
    let s = Scene::new().await;
    let manager = s.rig.person("threadmanager").await;
    let porter = s.rig.person("portsmanager").await;
    for person in [&manager, &porter] {
        places::join(
            &s.rig.world,
            s.tree.org.base.id,
            s.tree.org.base.id,
            person,
            "viewer",
            &s.ada,
        )
        .await;
    }
    let thread = s.thread().await.base.id;
    s.rig
        .lets_use("node", thread, "thread.manage", &manager, json!({}))
        .await;
    s.rig
        .lets_use(
            "environment",
            s.on.environment,
            "ports.manage",
            &porter,
            json!({}),
        )
        .await;
    let environment = {
        let mut conn = s.rig.world.conn().await;
        let row =
            orch_db::queries::environments::thread_engine::environment(&mut conn, s.on.environment)
                .await
                .unwrap()
                .unwrap();
        orch_engine::views::environment_ref_json(
            &mut conn,
            &row,
            orch_types::Time::new(s.rig.world.now()),
        )
        .await
        .unwrap()
    };
    let id = new_id();
    let request = json!({ "capability": "port_place", "label": "Open a port to a place", "risk": "medium", "reason": "Show the work",
        "environment": environment,
        "host": { "id": new_id(), "name": "Host", "address": "fixture-host" }, "consequences": ["The named place's viewers may reach this port"] });
    serde_json::from_value::<orch_api_contract::approvals::CapabilityRequest>(request.clone())
        .expect("the fixture must store the current capability contract");
    sqlx::query("INSERT INTO approvals(id,kind,node_id,thread_id,environment_id,requester_thread_id,title,request,required_permission,approver_scope,created_at,updated_at)
        VALUES($1,'capability',$2,$2,$3,$2,'Open the port',$4,'ports.manage','environment',$5,$5)")
        .bind(id).bind(thread).bind(s.on.environment).bind(request).bind(s.rig.world.now()).execute(&mut *s.rig.world.conn().await).await.unwrap();
    let decision = ApprovalDecision::Capability(orch_api_contract::approvals::CapabilityDecision {
        approve: true,
        note: None,
    });
    let refused = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&id])
        .token(&manager.token)
        .body(&decision)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "ports.manage");
    let decided = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&id])
        .token(&porter.token)
        .body(&decision)
        .send()
        .await
        .ok();
    assert_eq!(decided.approval.state, ApprovalState::Approved);
    assert_eq!(
        decided.approval.decided_by.as_ref().unwrap().id,
        porter.id()
    );
}

#[tokio::test]
async fn a_thread_manager_without_the_capabilitys_approver_scope_cannot_decide_it() {
    let s = Scene::new().await;
    let manager = s.rig.person("threadmanager").await;
    places::join(
        &s.rig.world,
        s.tree.org.base.id,
        s.tree.org.base.id,
        &manager,
        "viewer",
        &s.ada,
    )
    .await;
    let thread = s.thread().await.base.id;
    s.rig
        .lets_use("node", thread, "thread.manage", &manager, json!({}))
        .await;
    let access = places::read(&s.rig.world, &manager, thread).await;
    assert!(
        matches!(access, orch_api_contract::nodes::Node::Thread(thread) if thread.base.access.permissions.contains(&"thread.manage".to_owned()))
    );
    let id = new_id();
    sqlx::query("INSERT INTO approvals(id,kind,node_id,thread_id,environment_id,requester_thread_id,title,request,required_permission,approver_scope,created_at,updated_at)
        VALUES($1,'capability',$2,$2,$3,$2,'Open the boundary',$4,'','system',$5,$5)")
        .bind(id).bind(thread).bind(s.on.environment).bind(json!({ "capability": "network.open" })).bind(s.rig.world.now())
        .execute(&mut *s.rig.world.conn().await).await.unwrap();
    for decision in [
        ApprovalDecision::Capability(orch_api_contract::approvals::CapabilityDecision {
            approve: true,
            note: None,
        }),
        allow(),
    ] {
        let refused = s
            .rig
            .world
            .app
            .call(ApprovalDecide, &[&id])
            .token(&manager.token)
            .body(&decision)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
        let state: String = sqlx::query_scalar("SELECT state FROM approvals WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *s.rig.world.conn().await)
            .await
            .unwrap();
        assert_eq!(
            state, "pending",
            "neither a boundary answer nor a harness answer may cross the authority boundary"
        );
    }
}

/// A turn that runs and has asked whether a command may run.
async fn asking(s: &Scene, thread: Id, refs: &[&str]) -> orch_link_contract::commands::TurnStart {
    let sent = s
        .say(
            &s.ada,
            thread,
            "Clean up",
            orch_api_contract::threads::Delivery::Queue,
            "ask",
        )
        .await;
    assert!(sent.status.is_success());
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, turn_began(start.turn_id)).await;
    s.rig.say(thread, message_began(start.turn_id, "m")).await;
    s.rig.say(thread, message_grew("m", 0, "Cleaning")).await;
    for each in refs {
        s.rig.say(thread, command_asked(start.turn_id, each)).await;
    }
    s.rig.wait_state(thread, "waiting_approval").await;
    let wanted = i64::try_from(refs.len()).unwrap();
    rig::eventually("every request is kept", || async move {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM approvals WHERE thread_id = $1 AND kind = 'harness'",
        )
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
        count == wanted
    })
    .await;
    start
}

async fn list(s: &Scene, who: &Who, query: ApprovalListQuery) -> Page<Approval> {
    s.rig
        .world
        .app
        .call(ApprovalList, &[])
        .token(&who.token)
        .query(&query)
        .send()
        .await
        .ok()
}

fn scope(scope: &str) -> ApprovalListQuery {
    ApprovalListQuery {
        scope: Some(scope.to_owned()),
        ..ApprovalListQuery::default()
    }
}

#[tokio::test]
async fn an_approval_is_listed_for_those_who_can_decide_it_and_read_by_those_who_can_read_the_thread()
 {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    asking(&s, id, &["r1"]).await;

    let mine = list(&s, &s.ada, ApprovalListQuery::default()).await;
    assert_eq!(mine.items.len(), 1);
    let approval = &mine.items[0];
    assert_eq!(approval.request.kind(), ApprovalKind::Harness);
    assert_eq!(approval.state, ApprovalState::Pending);
    assert_eq!(approval.thread_id, Some(id));
    assert_eq!(approval.required_permission, "thread.manage");
    assert!(approval.can_decide);
    assert!(matches!(&approval.requester, ApprovalRequester::Thread(thread) if thread.id == id));
    // A developer can decide without receiving an owner's notification.
    assert!(
        list(&s, &s.eve, ApprovalListQuery::default())
            .await
            .items
            .is_empty()
    );
    let others = list(
        &s,
        &s.eve,
        ApprovalListQuery {
            scope: Some("others".into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(others.items.len(), 1);
    assert!(others.items[0].can_decide);
    assert!(
        list(&s, &s.cai, ApprovalListQuery::default())
            .await
            .items
            .is_empty()
    );
    assert!(
        list(&s, &s.bea, ApprovalListQuery::default())
            .await
            .items
            .is_empty()
    );
    let read = s
        .rig
        .world
        .app
        .call(ApprovalGet, &[&approval.id])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert!(!read.can_decide, "she reads it and may not answer it");
    let refused = s
        .rig
        .world
        .app
        .call(ApprovalGet, &[&approval.id])
        .token(&s.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["thread.view"]);
    let none = s
        .rig
        .world
        .app
        .call(ApprovalGet, &[&new_id()])
        .token(&s.ada.token)
        .send()
        .await
        .problem();
    assert_eq!(none.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_first_decision_wins_the_harness_is_told_once_and_the_turn_goes_on() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    asking(&s, id, &["r1"]).await;
    let waiting = list(&s, &s.ada, ApprovalListQuery::default())
        .await
        .items
        .remove(0);
    let decide = |who: &Who, decision: ApprovalDecision| {
        let call = s
            .rig
            .world
            .app
            .call(ApprovalDecide, &[&waiting.id])
            .token(&who.token)
            .body(&decision);
        async move { call.send().await }
    };
    let decided = decide(&s.ada, allow()).await.ok();
    assert_eq!(decided.state, ApprovalState::Approved);
    assert_eq!(
        decided.decided_by.as_ref().map(|by| by.username.as_str()),
        Some("ada")
    );
    assert!(!decided.decided_automatically);
    rig::eventually("the harness is told", || {
        let told = !s.rig.seen.of_kind(CommandKind::ApprovalResolve).is_empty();
        async move { told }
    })
    .await;
    s.rig.wait_state(id, "running").await;
    // A second decision finds it decided, and is shown how.
    let late = decide(
        &s.eve,
        ApprovalDecision::Option(HarnessOptionDecision {
            option_id: "deny".to_owned(),
            note: None,
        }),
    )
    .await
    .problem();
    assert_eq!(late.code, ErrorCode::ApprovalAlreadyDecided);
    assert_eq!(late.extra["current"]["state"], "approved");
    assert_eq!(
        s.rig.seen.of_kind(CommandKind::ApprovalResolve).len(),
        1,
        "the harness was told once"
    );
    assert!(
        list(&s, &s.ada, ApprovalListQuery::default())
            .await
            .items
            .is_empty()
    );
    let decided_list = list(
        &s,
        &s.ada,
        ApprovalListQuery {
            state: Some(ApprovalListState::Decided),
            ..ApprovalListQuery::default()
        },
    )
    .await;
    assert_eq!(decided_list.items.len(), 1);
}

#[tokio::test]
async fn an_option_that_denies_denies_and_a_decision_that_does_not_fit_the_request_is_refused() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    asking(&s, id, &["r1"]).await;
    let waiting = list(&s, &s.ada, ApprovalListQuery::default())
        .await
        .items
        .remove(0);
    let decide = |decision: ApprovalDecision| {
        let call = s
            .rig
            .world
            .app
            .call(ApprovalDecide, &[&waiting.id])
            .token(&s.ada.token)
            .body(&decision);
        async move { call.send().await }
    };
    let wrong = decide(ApprovalDecision::Answers(HarnessAnswersDecision {
        answers: std::collections::BTreeMap::new(),
        note: None,
    }))
    .await
    .problem();
    assert_eq!(wrong.code, ErrorCode::RequestInvalid);
    let unknown = decide(ApprovalDecision::Option(HarnessOptionDecision {
        option_id: "grant_all".to_owned(),
        note: None,
    }))
    .await
    .problem();
    assert_eq!(unknown.code, ErrorCode::RequestInvalid);
    let denied = decide(ApprovalDecision::Option(HarnessOptionDecision {
        option_id: "deny".to_owned(),
        note: Some("not now".to_owned()),
    }))
    .await
    .ok();
    assert_eq!(denied.state, ApprovalState::Denied);
}

#[tokio::test]
async fn answering_what_the_agent_asks_is_managing_the_thread_and_driving_it_is_not_enough() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    asking(&s, id, &["r1"]).await;
    let waiting = list(&s, &s.ada, ApprovalListQuery::default())
        .await
        .items
        .remove(0);
    for who in [&s.cai, &s.bea] {
        let refused = s
            .rig
            .world
            .app
            .call(ApprovalDecide, &[&waiting.id])
            .token(&who.token)
            .body(&allow())
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{who:?}");
        assert!(missing_permissions(&refused).contains(&"thread.manage".to_owned()));
    }
    let synced = s
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&id])
        .token(&s.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(synced.thread.state, ThreadState::WaitingApproval);
    assert_eq!(synced.thread.pending_approvals, 1, "nothing was decided");
}

#[tokio::test]
async fn a_list_is_narrowed_by_scope_and_kind_and_pages_newest_first() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    asking(&s, id, &["r1", "r2"]).await;
    let all = list(&s, &s.ada, ApprovalListQuery::default()).await;
    assert_eq!(all.items.len(), 2);
    assert!(
        all.items[0].created_at >= all.items[1].created_at,
        "newest first"
    );
    let first = list(
        &s,
        &s.ada,
        ApprovalListQuery {
            limit: Some(1),
            ..ApprovalListQuery::default()
        },
    )
    .await;
    assert_eq!(first.items.len(), 1);
    let cursor = first.next_cursor.clone().expect("another page follows");
    let second = list(
        &s,
        &s.ada,
        ApprovalListQuery {
            limit: Some(1),
            cursor: Some(cursor),
            ..ApprovalListQuery::default()
        },
    )
    .await;
    assert_eq!(second.items.len(), 1);
    assert_ne!(second.items[0].id, first.items[0].id);
    assert!(second.next_cursor.is_none());
    // Attached to a place, they are listed to whoever may view the place and its threads.
    let topic = format!("node:{}", s.tree.topic.base.id);
    assert_eq!(list(&s, &s.cai, scope(&topic)).await.items.len(), 2);
    assert!(list(&s, &s.ada, scope("requested")).await.items.is_empty());
    let kind = list(
        &s,
        &s.ada,
        ApprovalListQuery {
            kind: Some(ApprovalKind::Access),
            ..ApprovalListQuery::default()
        },
    )
    .await;
    assert!(kind.items.is_empty());
    let nowhere = s
        .rig
        .world
        .app
        .call(ApprovalList, &[])
        .token(&s.bea.token)
        .query(&scope(&topic))
        .send()
        .await
        .problem();
    assert_eq!(
        nowhere.code,
        ErrorCode::AuthzDenied,
        "she may not view the place"
    );
    let odd = s
        .rig
        .world
        .app
        .call(ApprovalList, &[])
        .token(&s.ada.token)
        .query(&scope("everybody"))
        .send()
        .await
        .problem();
    assert_eq!(odd.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_request_for_access_is_taken_back_by_the_person_who_asked_and_decided_only_by_one_who_could_give_it()
 {
    let s = Scene::new().await;
    let request = new_id();
    sqlx::query(
        "INSERT INTO approvals (id, kind, state, node_id, requester_user_id, title, request, required_permission,
                                created_at, updated_at)
         VALUES ($1, 'access', 'pending', $2, $3, 'Access to Login', $4, 'access.manage', now(), now())",
    )
    .bind(request)
    .bind(s.tree.topic.base.id)
    .bind(s.bea.id())
    .bind(json!({
        "permissions": ["topic.interact"], "options": null, "reason": "please",
        "holder": { "kind": "user", "user": {
            "id": s.bea.id(), "kind": "human", "username": "bea", "display_name": "bea",
            "avatar": { "text": "B", "color": 1 } } },
        "for_thread": null
    }))
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let app = &s.rig.world.app;
    // The person who asked reads it and lists it as asked by them.
    let asked = list(&s, &s.bea, scope("requested")).await;
    assert_eq!(asked.items.len(), 1);
    assert_eq!(asked.items[0].request.kind(), ApprovalKind::Access);
    assert!(
        app.call(ApprovalGet, &[&request])
            .token(&s.bea.token)
            .send()
            .await
            .body
            .is_ok()
    );
    // Nobody else takes it back, and only somebody who could have given what is asked decides it.
    let other = app
        .call(ApprovalCancel, &[&request])
        .token(&s.ada.token)
        .send()
        .await
        .problem();
    assert_eq!(other.code, ErrorCode::AuthzDenied);
    let decide = app
        .call(ApprovalDecide, &[&request])
        .token(&s.cai.token)
        .raw_body(&json!({ "approve": true }))
        .send()
        .await
        .problem();
    assert_eq!(decide.code, ErrorCode::AuthzDenied);
    let taken = app
        .call(ApprovalCancel, &[&request])
        .token(&s.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(taken.state, ApprovalState::Cancelled);
    let again = app
        .call(ApprovalCancel, &[&request])
        .token(&s.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::ApprovalAlreadyDecided);
    // What a harness asks is no request for access.
    let id = s.thread().await.base.id;
    asking(&s, id, &["r1"]).await;
    let waiting = list(&s, &s.ada, ApprovalListQuery::default())
        .await
        .items
        .remove(0);
    let harness = app
        .call(ApprovalCancel, &[&waiting.id])
        .token(&s.ada.token)
        .send()
        .await
        .problem();
    assert_eq!(harness.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_question_keeps_invalid_answers_pending_and_exposes_answered_in_every_http_projection() {
    use orch_api_contract::threads::{Delivery, ItemKind, ItemStatus};
    use orch_link_contract::events::{ApprovalRequested, Event};
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    assert!(
        s.say(&s.ada, thread, "Ask a question", Delivery::Queue, "ask")
            .await
            .status
            .is_success()
    );
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, turn_began(start.turn_id)).await;
    s.rig.say(thread, Event::ApprovalRequested(ApprovalRequested {
        turn_id:start.turn_id,request_ref:"http-question".into(),title:"Choose a branch".into(),
        request:serde_json::from_value(json!({"request_kind":"user_input","options":[],"detail":{
            "questions":[{"id":"branch","header":"Branch","question":"Which branch?","multiple":false,"allow_other":false,
                "options":[{"label":"main","description":"Main"},{"label":"work","description":"Work"}]}]
        }})).unwrap(),
    })).await;
    s.rig.wait_state(thread, "waiting_approval").await;
    let waiting = list(&s, &s.ada, ApprovalListQuery::default())
        .await
        .items
        .remove(0);
    let valid: ApprovalDecision =
        serde_json::from_value(json!({"answers":{"branch":["main"]}})).unwrap();
    let denied = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&waiting.id])
        .token(&s.cai.token)
        .body(&valid)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    for answers in [
        json!({}),
        json!({"unknown":["main"]}),
        json!({"branch":[]}),
        json!({"branch":["unoffered"]}),
        json!({"branch":["main","work"]}),
    ] {
        let invalid: ApprovalDecision = serde_json::from_value(json!({"answers":answers})).unwrap();
        let refusal = s
            .rig
            .world
            .app
            .call(ApprovalDecide, &[&waiting.id])
            .token(&s.ada.token)
            .body(&invalid)
            .send()
            .await
            .problem();
        assert_eq!(refusal.code, ErrorCode::RequestInvalid);
        let unchanged = s
            .rig
            .world
            .app
            .call(ApprovalGet, &[&waiting.id])
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        assert_eq!(unchanged.state, ApprovalState::Pending);
        assert!(s.rig.seen.of_kind(CommandKind::ApprovalResolve).is_empty());
    }
    let answered = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&waiting.id])
        .token(&s.ada.token)
        .body(&valid)
        .send()
        .await
        .ok();
    assert_eq!(answered.state, ApprovalState::Answered);
    assert!(!answered.can_decide && !answered.decided_automatically);
    let read = s
        .rig
        .world
        .app
        .call(ApprovalGet, &[&waiting.id])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(read.state, ApprovalState::Answered);
    assert_eq!(read.decision, Some(json!({"answers":{"branch":["main"]}})));
    let decided = list(
        &s,
        &s.ada,
        ApprovalListQuery {
            state: Some(ApprovalListState::Decided),
            ..ApprovalListQuery::default()
        },
    )
    .await;
    assert_eq!(decided.items.len(), 1);
    assert_eq!(decided.items[0].state, ApprovalState::Answered);
    assert!(
        list(&s, &s.ada, ApprovalListQuery::default())
            .await
            .items
            .is_empty()
    );
    let sync = s
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&s.cai.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    let item = sync
        .items
        .iter()
        .find(|item| item.kind() == ItemKind::Approval)
        .unwrap();
    assert_eq!(item.status, ItemStatus::Answered);
    let late = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&waiting.id])
        .token(&s.eve.token)
        .body(&valid)
        .send()
        .await
        .problem();
    assert_eq!(late.code, ErrorCode::ApprovalAlreadyDecided);
    assert_eq!(late.extra["current"]["state"], "answered");
}
