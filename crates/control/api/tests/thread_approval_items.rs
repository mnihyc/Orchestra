//! A native question arrives with its item, personalized for its reader, and is answered in place.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed test panics"
)]
mod places;
mod rig;
mod support;

use orch_api_contract::approvals::{
    ApprovalDecide, ApprovalDecision, ApprovalState, HarnessAnswersDecision,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{
    Delivery, ItemPayload, OutlineKind, ThreadItemGet, ThreadOutlineGet, ThreadSync, ThreadSyncGet,
    ThreadSyncGetQuery,
};
use orch_link_contract::events::{ApprovalRequested, Event};
use orch_types::Id;
use places::Who;
use rig::Scene;
use serde_json::json;
use std::collections::BTreeMap;

async fn sync(s: &Scene, who: &Who, thread: Id) -> ThreadSync {
    s.rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&who.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_question_is_in_its_item_for_the_manager_and_viewer_with_only_the_managers_authority() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    assert!(
        s.say(
            &s.ada,
            thread,
            "Choose the scope",
            Delivery::Queue,
            "question-item"
        )
        .await
        .status
        .is_success()
    );
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, rig::turn_began(start.turn_id)).await;
    s.rig.say(thread, Event::ApprovalRequested(ApprovalRequested {
        turn_id: start.turn_id, request_ref: "question-1".to_owned(), title: "Choose the scope".to_owned(),
        request: serde_json::from_value(json!({ "request_kind": "user_input", "options": [], "detail": { "questions": [
            { "id": "scope", "header": "Scope", "question": "Which part should I change?", "multiple": false, "options": null, "allow_other": true }
        ] } })).unwrap(),
    })).await;
    s.rig.wait_state(thread, "waiting_approval").await;

    let owner = sync(&s, &s.ada, thread).await;
    let item = owner
        .items
        .iter()
        .find(|item| matches!(&item.payload, ItemPayload::Approval(_)))
        .unwrap();
    let ItemPayload::Approval(payload) = &item.payload else {
        unreachable!()
    };
    let request = payload
        .approval
        .as_ref()
        .expect("the question needs no second HTTP read");
    assert_eq!(request.title, "Choose the scope");
    assert!(request.can_decide);
    let encoded = serde_json::to_value(request).unwrap();
    assert_eq!(
        encoded["request"]["detail"]["questions"][0]["question"],
        "Which part should I change?"
    );
    let outline = s
        .rig
        .world
        .app
        .call(ThreadOutlineGet, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let title = outline
        .entries
        .iter()
        .find(|entry| entry.kind == OutlineKind::Approval && entry.item_id == item.id)
        .unwrap();
    assert_eq!(
        title.title, request.title,
        "the outline names the same question as its card"
    );

    let viewer = sync(&s, &s.cai, thread).await;
    let ItemPayload::Approval(read) = &viewer
        .items
        .iter()
        .find(|found| found.id == item.id)
        .unwrap()
        .payload
    else {
        unreachable!()
    };
    assert!(
        !read.approval.as_ref().unwrap().can_decide,
        "one reader's snapshot is never shared with another"
    );
    assert_eq!(read.approval.as_ref().unwrap().id, request.id);
    let single = s
        .rig
        .world
        .app
        .call(ThreadItemGet, &[&thread, &item.id])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    let ItemPayload::Approval(single) = single.payload else {
        unreachable!()
    };
    assert!(!single.approval.unwrap().can_decide);

    let answer = ApprovalDecision::Answers(HarnessAnswersDecision {
        answers: BTreeMap::from([("scope".to_owned(), vec!["Login".to_owned()])]),
        note: None,
    });
    let refused = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&request.id])
        .token(&s.cai.token)
        .body(&answer)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let answered = s
        .rig
        .world
        .app
        .call(ApprovalDecide, &[&request.id])
        .token(&s.ada.token)
        .body(&answer)
        .send()
        .await
        .ok();
    assert_eq!(answered.approval.state, ApprovalState::Answered);
    let current = sync(&s, &s.ada, thread).await;
    let ItemPayload::Approval(current) = &current
        .items
        .iter()
        .find(|found| found.id == item.id)
        .unwrap()
        .payload
    else {
        unreachable!()
    };
    assert_eq!(
        current.approval.as_ref().unwrap().state,
        ApprovalState::Answered
    );
    let denied = s
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&s.bea.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
}
