//! Asking for access over HTTP: made from a refusal for what is missing, sent to the nearest who can give each thing, decided by any of them, the person who asked told, run out, and giving nothing beyond what was asked.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use chrono::Duration;
use desk::Desk;
use orch_api_contract::access::{
    AccessOptions, AccessRequestCreate, AccessRequestCreateBody, AccessRequestGet,
    AccessRequestItem, AccessRequestObject, EffectiveAccessGet, EffectiveAccessGetQuery,
    HolderInput, NodeDefaultsUpdate, PageChange, PageChangeAdd, PageChangeRow, PermissionPageGet,
    PermissionPageUpdate,
};
use orch_api_contract::approvals::{
    AccessDecision, Approval, ApprovalCancel, ApprovalDecide, ApprovalDecision,
    ApprovalDecisionReply, ApprovalGet, ApprovalKind, ApprovalList, ApprovalListQuery,
    ApprovalListState, ApprovalRequest, ApprovalState,
};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::nodes::{Node, NodeUpdate, NodeUpdateBody};
use orch_api_contract::notifications::{NotificationKind, NotificationList, NotificationListQuery};
use orch_api_contract::ownership::{NodeTransfer, TransferBody};
use orch_api_contract::resources::{CreateProvider, ProviderCreate, Upstream};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::secrets::MasterKey;
use orch_types::{Id, Time};
use places::{Who, first_missing, give_role, make_project, make_topic, person, read};
use serde_json::json;

/// Acme with its owner Ada, an admin Dee, a developer Pam who owns the project Pam's, a viewer
/// Rae, a stranger Sam, and a system admin Root.
struct Firm {
    desk: Desk,
    dee: Who,
    pam: Who,
    rae: Who,
    sam: Who,
    root: Who,
    project: Id,
}

async fn firm() -> Firm {
    install_master_key(MasterKey::new([7; 32]));
    let desk = Desk::new().await;
    let dee = desk.member("dee", "admin").await;
    let pam = desk.member("pam", "developer").await;
    let rae = desk.member("rae", "viewer").await;
    let sam = desk.stranger("sam").await;
    let root = places::admin(&desk.world, "root").await;
    // The project is made by Acme's owner, who then hands it to Pam alone.
    let project = make_project(&desk.world, &desk.ada, desk.org, "Pam's")
        .await
        .base
        .id;
    desk.world
        .app
        .call(NodeTransfer, &[&project])
        .token(&desk.ada.token)
        .body(&TransferBody {
            user_id: pam.id(),
            keep_previous: Some(false),
        })
        .send()
        .await
        .ok();
    Firm {
        desk,
        dee,
        pam,
        rae,
        sam,
        root,
        project,
    }
}

fn item(kind: &str, id: Id, permissions: &[&str]) -> AccessRequestItem {
    AccessRequestItem {
        label: None,
        object: AccessRequestObject {
            kind: kind.to_owned(),
            id,
        },
        permissions: permissions.iter().map(|one| (*one).to_owned()).collect(),
        options: None,
        holder: None,
    }
}

async fn ask(
    firm: &Firm,
    by: &Who,
    items: Vec<AccessRequestItem>,
) -> orch_api_core::testing::Reply<orch_api_contract::access::AccessRequestCreateReply> {
    firm.desk
        .world
        .app
        .call(AccessRequestCreate, &[])
        .token(&by.token)
        .body(&AccessRequestCreateBody {
            items,
            reason: "I need it".to_owned(),
            for_thread: None,
        })
        .send()
        .await
}

fn approve() -> AccessDecision {
    AccessDecision {
        approve: true,
        permissions: None,
        options: None,
        expires_at: None,
        note: None,
        quick_duration: None,
    }
}

#[tokio::test]
async fn asking_for_a_selected_person_keeps_the_asker_and_gives_exactly_that_person_the_held_row() {
    let f = firm().await;
    let topic = make_topic(&f.desk.world, &f.pam, f.project, "Selected person")
        .await
        .base
        .id;
    let mut wanted = item("topic", topic, &["topic.interact"]);
    wanted.holder = Some(HolderInput::User { id: f.sam.id() });
    let raised = ask(&f, &f.pam, vec![wanted]).await.ok();
    let approval = &raised.approvals[0];
    assert!(
        matches!(&approval.requester, orch_api_contract::approvals::ApprovalRequester::User(user) if user.id == f.pam.id())
    );
    let ApprovalRequest::Access(request) = &approval.request else {
        panic!("access approval");
    };
    assert!(
        matches!(&request.holder, orch_api_contract::access::Holder::User { user } if user.id == f.sam.id())
    );
    // A requester may already hold the row: the selected recipient is the one asking would benefit.
    assert!(
        holds(&f, "nodes", topic, &f.pam)
            .await
            .contains(&"topic.interact".to_owned())
    );
    // Ada gives it as the org's admin, with As admin on.
    places::admin_mode(&f.desk.world, &f.desk.ada, true).await;
    decide(&f, &f.desk.ada, approval.id, approve()).await.ok();
    assert!(
        holds(&f, "nodes", topic, &f.sam)
            .await
            .contains(&"topic.interact".to_owned())
    );
    let page = f
        .desk
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &topic])
        .token(&f.pam.token)
        .send()
        .await
        .ok();
    assert!(page.rows.iter().filter(|row| row.permission == "topic.interact")
        .flat_map(|row| &row.entries).any(|entry| entry.source == orch_api_contract::access::EntrySource::Request
            && matches!(&entry.holder, orch_api_contract::access::Holder::User { user } if user.id == f.sam.id())));
}

async fn decide(
    firm: &Firm,
    by: &Who,
    approval: Id,
    decision: AccessDecision,
) -> orch_api_core::testing::Reply<ApprovalDecisionReply> {
    firm.desk
        .world
        .app
        .call(ApprovalDecide, &[&approval])
        .token(&by.token)
        .body(&ApprovalDecision::Access(decision))
        .send()
        .await
}

async fn waiting_for(firm: &Firm, by: &Who, all: bool) -> Vec<Id> {
    let page: Page<Approval> = firm
        .desk
        .world
        .app
        .call(ApprovalList, &[])
        .token(&by.token)
        .query(&ApprovalListQuery {
            all: all.then_some(true),
            ..ApprovalListQuery::default()
        })
        .send()
        .await
        .ok();
    page.items.iter().map(|approval| approval.id).collect()
}

async fn inbox(firm: &Firm, who: &Who) -> Vec<NotificationKind> {
    firm.desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&who.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok()
        .items
        .iter()
        .map(|each| each.kind)
        .collect()
}

async fn is_complete(firm: &Firm, batch: Id) -> bool {
    firm.desk
        .world
        .app
        .call(AccessRequestGet, &[&batch])
        .token(&firm.rae.token)
        .send()
        .await
        .ok()
        .complete
}

async fn holds(firm: &Firm, node: &str, id: Id, who: &Who) -> Vec<String> {
    firm.desk
        .world
        .app
        .call(EffectiveAccessGet, &[&node, &id])
        .token(&who.token)
        .query(&EffectiveAccessGetQuery { user_id: who.id() })
        .send()
        .await
        .ok()
        .permissions
        .into_iter()
        .map(|held| held.permission)
        .collect()
}

#[tokio::test]
async fn somebody_who_is_refused_asks_for_what_is_missing_and_it_is_sent_to_the_nearest_who_can_give_it()
 {
    let firm = firm().await;
    let made = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok();
    assert_eq!(made.approvals.len(), 1);
    let approval = &made.approvals[0];
    assert_eq!(approval.request.kind(), ApprovalKind::Access);
    assert_eq!(approval.state, ApprovalState::Pending);
    assert_eq!(approval.batch_id, Some(made.batch_id));
    assert_eq!(approval.required_permission, "access.manage");
    assert!(!approval.can_decide, "the person who asked does not decide");
    let ApprovalRequest::Access(request) = &approval.request else {
        panic!("{approval:?}");
    };
    assert_eq!(request.permissions, ["project.interact"]);
    assert_eq!(request.reason, "I need it");
    // The owner of the project is sent it, and so are the admins of the org it lies in, the first
    // place above it that has any, who list and decide it with As admin on; the system's admin may
    // decide and is sent nothing.
    places::admin_mode(&firm.desk.world, &firm.dee, true).await;
    places::admin_mode(&firm.desk.world, &firm.desk.ada, true).await;
    assert_eq!(waiting_for(&firm, &firm.pam, false).await, [approval.id]);
    assert!(waiting_for(&firm, &firm.dee, false).await.is_empty());
    assert!(waiting_for(&firm, &firm.desk.ada, false).await.is_empty());
    assert_eq!(waiting_for(&firm, &firm.dee, true).await, [approval.id]);
    assert_eq!(
        waiting_for(&firm, &firm.desk.ada, true).await,
        [approval.id]
    );
    let root_mine = waiting_for(&firm, &firm.root, false).await;
    assert!(
        root_mine.is_empty(),
        "the system's admin is sent nothing: {root_mine:?}"
    );
    assert_eq!(waiting_for(&firm, &firm.root, true).await, [approval.id]);
    assert!(waiting_for(&firm, &firm.sam, true).await.is_empty());
    assert!(waiting_for(&firm, &firm.rae, true).await.is_empty());
    for told in [&firm.pam] {
        assert!(
            inbox(&firm, told)
                .await
                .contains(&NotificationKind::AccessRequested)
        );
    }
    for not_told in [&firm.dee, &firm.desk.ada, &firm.root, &firm.rae, &firm.sam] {
        assert!(
            !inbox(&firm, not_told)
                .await
                .contains(&NotificationKind::AccessRequested)
        );
    }
}

#[tokio::test]
async fn opening_a_request_names_its_person_and_exact_item_and_a_decision_updates_even_its_already_read_notice()
 {
    use orch_api_contract::notifications::{
        NotificationRead, NotificationSummaryGet, NotificationSummaryGetQuery,
    };
    let f = firm().await;
    let made = ask(
        &f,
        &f.rae,
        vec![item("project", f.project, &["project.interact"])],
    )
    .await
    .ok();
    let approval = made.approvals[0].id;
    let page = f
        .desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&f.pam.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok();
    let notice = page
        .items
        .iter()
        .find(|row| row.approval_id == Some(approval))
        .unwrap();
    assert_eq!(
        notice.actor.as_ref().map(|actor| actor.id),
        Some(f.rae.id())
    );
    // World::person creates this fixture with its username as its display name.
    assert_eq!(
        notice
            .actor
            .as_ref()
            .map(|actor| actor.display_name.as_str()),
        Some(f.rae.person.username.as_str())
    );
    assert_eq!(notice.url, format!("/approvals?approval={approval}"));
    let details = &notice
        .details
        .as_ref()
        .expect("a structured notification")
        .parameters;
    assert_eq!(details["permissions"], json!(["project.interact"]));
    assert!(
        details["reason"].is_string(),
        "the reason stays authored text, separate from localized permission words"
    );
    let exact = f
        .desk
        .world
        .app
        .call(ApprovalGet, &[&approval])
        .token(&f.pam.token)
        .send()
        .await
        .ok();
    assert_eq!(exact.id, approval);
    assert!(exact.can_decide);
    f.desk
        .world
        .app
        .call(NotificationRead, &[])
        .token(&f.pam.token)
        .raw_body(&json!({"ids":[notice.id]}))
        .send()
        .await
        .ok();
    let opened = f
        .desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&f.pam.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok();
    let opened = opened.items.iter().find(|row| row.id == notice.id).unwrap();
    let before = f
        .desk
        .world
        .app
        .call(NotificationSummaryGet, &[])
        .token(&f.pam.token)
        .query(&NotificationSummaryGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(before.approvals_pending, 1);
    f.desk.world.advance(Duration::seconds(2));
    decide(&f, &f.pam, approval, approve()).await.ok();
    let after = f
        .desk
        .world
        .app
        .call(NotificationSummaryGet, &[])
        .token(&f.pam.token)
        .query(&NotificationSummaryGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(after.approvals_pending, 0);
    assert_eq!(after.unread, before.unread);
    let current = f
        .desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&f.pam.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok();
    let current = current
        .items
        .iter()
        .find(|row| row.id == notice.id)
        .unwrap();
    assert!(
        current.inbox_seq > opened.inbox_seq,
        "the same live row carries the new decision/count cut"
    );
    assert_eq!(current.read_at, opened.read_at);
    assert!(!current.waiting_for_me);
}

#[tokio::test]
async fn any_of_those_who_could_have_given_it_decides_and_a_stranger_does_not() {
    let firm = firm().await;
    let approval = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok()
    .approvals
    .remove(0);
    let refused = decide(&firm, &firm.sam, approval.id, approve())
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(
        !holds(&firm, "nodes", firm.project, &firm.rae)
            .await
            .contains(&"project.interact".to_owned())
    );
    // The system's admin is further up, is sent nothing and may decide.
    let decided = decide(&firm, &firm.root, approval.id, approve()).await.ok();
    assert_eq!(decided.state, ApprovalState::Approved);
    assert_eq!(decided.decided_by.as_ref().unwrap().username, "root");
    assert!(
        holds(&firm, "nodes", firm.project, &firm.rae)
            .await
            .contains(&"project.interact".to_owned())
    );
    let late = decide(&firm, &firm.pam, approval.id, approve())
        .await
        .problem();
    assert_eq!(late.code, ErrorCode::ApprovalAlreadyDecided);
}

#[tokio::test]
async fn approving_gives_what_was_asked_and_nothing_beyond_it() {
    let firm = firm().await;
    let approval = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok()
    .approvals
    .remove(0);
    let beyond = decide(
        &firm,
        &firm.pam,
        approval.id,
        AccessDecision {
            permissions: Some(vec!["project.manage".to_owned()]),
            ..approve()
        },
    )
    .await
    .problem();
    assert_eq!(beyond.code, ErrorCode::RequestInvalid, "{beyond:?}");
    let none = decide(
        &firm,
        &firm.pam,
        approval.id,
        AccessDecision {
            permissions: Some(Vec::new()),
            ..approve()
        },
    )
    .await
    .problem();
    assert_eq!(none.code, ErrorCode::RequestInvalid);
    decide(&firm, &firm.pam, approval.id, approve()).await.ok();
    let held = holds(&firm, "nodes", firm.project, &firm.rae).await;
    assert!(held.contains(&"project.interact".to_owned()));
    assert!(!held.contains(&"project.manage".to_owned()), "{held:?}");
    let page = firm
        .desk
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &firm.project])
        .token(&firm.pam.token)
        .send()
        .await
        .ok();
    let entry = page
        .rows
        .iter()
        .flat_map(|row| row.entries.iter())
        .find(|entry| matches!(&entry.holder, orch_api_contract::access::Holder::User { user } if user.username == "rae"))
        .expect("named on the page");
    assert_eq!(
        serde_json::to_value(entry.source).unwrap(),
        json!("request"),
        "the entry says it came from an approved request"
    );
    assert_eq!(entry.created_by.as_ref().unwrap().username, "pam");
}

#[tokio::test]
async fn an_approver_may_give_fewer_permissions_narrower_options_and_an_end() {
    let firm = firm().await;
    let provider = firm
        .desk
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&firm.pam.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Team key".to_owned(),
            home_id: firm.desk.org,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://api.example.com/v1".to_owned(),
                options: json!({}),
            }],
            api_key: Some("sk-test".to_owned()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider
        .base
        .id;
    let mut wanted = item("provider", provider, &["provider.use"]);
    wanted.options = Some(AccessOptions {
        models: Some(vec!["gpt-a".to_owned(), "gpt-b".to_owned()]),
        efforts: Some(vec!["high".to_owned()]),
    });
    let approval = ask(&firm, &firm.rae, vec![wanted])
        .await
        .ok()
        .approvals
        .remove(0);
    let wider = decide(
        &firm,
        &firm.pam,
        approval.id,
        AccessDecision {
            options: Some(json!({ "models": ["gpt-a", "gpt-z"] })),
            ..approve()
        },
    )
    .await
    .problem();
    assert_eq!(
        wider.code,
        ErrorCode::RequestInvalid,
        "wider than asked: {wider:?}"
    );
    let past = decide(
        &firm,
        &firm.pam,
        approval.id,
        AccessDecision {
            expires_at: Some(Time::new(firm.desk.world.now() - Duration::hours(1))),
            ..approve()
        },
    )
    .await
    .problem();
    assert_eq!(
        past.code,
        ErrorCode::RequestInvalid,
        "an end in the past: {past:?}"
    );
    let end = Time::new(firm.desk.world.now() + Duration::days(3));
    decide(
        &firm,
        &firm.pam,
        approval.id,
        AccessDecision {
            options: Some(json!({ "models": ["gpt-a"] })),
            expires_at: Some(end),
            ..approve()
        },
    )
    .await
    .ok();
    let page = firm
        .desk
        .world
        .app
        .call(PermissionPageGet, &[&"providers", &provider])
        .token(&firm.pam.token)
        .send()
        .await
        .ok();
    let use_row = page
        .rows
        .iter()
        .find(|row| row.permission == "provider.use")
        .unwrap();
    let entry = use_row.entries.first().expect("rae is named");
    assert_eq!(entry.options["models"], json!(["gpt-a"]));
    assert_eq!(
        entry.options["efforts"],
        json!(["high"]),
        "what was asked and not narrowed stays"
    );
    assert_eq!(entry.expires_at, Some(end));
    firm.desk.world.advance(Duration::days(4));
    let after = firm
        .desk
        .world
        .app
        .call(EffectiveAccessGet, &[&"providers", &provider])
        .token(&firm.rae.token)
        .query(&EffectiveAccessGetQuery {
            user_id: firm.rae.id(),
        })
        .send()
        .await;
    assert_eq!(
        after.problem().code,
        ErrorCode::AuthzDenied,
        "after its end it counts for nothing: she holds nothing on the provider"
    );
}

#[tokio::test]
async fn the_person_who_asked_is_told_of_each_decision_and_once_more_when_everything_is_held() {
    let firm = firm().await;
    let topic = places::make_topic(&firm.desk.world, &firm.pam, firm.project, "Plans")
        .await
        .base
        .id;
    let made = ask(
        &firm,
        &firm.rae,
        vec![
            item("project", firm.project, &["project.interact"]),
            item("topic", topic, &["topic.interact"]),
        ],
    )
    .await
    .ok();
    assert_eq!(made.approvals.len(), 2);
    assert!(!is_complete(&firm, made.batch_id).await);
    decide(&firm, &firm.pam, made.approvals[0].id, approve())
        .await
        .ok();
    let told = inbox(&firm, &firm.rae).await;
    assert_eq!(
        told.iter()
            .filter(|kind| **kind == NotificationKind::AccessDecided)
            .count(),
        1,
        "one decision of two"
    );
    assert!(
        !is_complete(&firm, made.batch_id).await,
        "one of the two is held"
    );
    decide(
        &firm,
        &firm.pam,
        made.approvals[1].id,
        AccessDecision {
            note: Some("welcome".to_owned()),
            ..approve()
        },
    )
    .await
    .ok();
    assert!(is_complete(&firm, made.batch_id).await);
    let all = firm
        .desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&firm.rae.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok();
    let decided: Vec<&str> = all
        .items
        .iter()
        .filter(|each| each.kind == NotificationKind::AccessDecided)
        .map(|each| each.title.as_str())
        .collect();
    assert_eq!(
        decided.len(),
        3,
        "two decisions and the word that everything is held: {decided:?}"
    );
    assert!(
        decided
            .iter()
            .any(|title| title.contains("Everything you asked for"))
    );
    // What asked the owner to decide is read once it is decided.
    let pam_inbox = firm
        .desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&firm.pam.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        pam_inbox
            .items
            .iter()
            .filter(|each| each.kind == NotificationKind::AccessRequested)
            .all(|each| each.read_at.is_some())
    );
}

#[tokio::test]
async fn a_denied_request_gives_nothing_and_carries_the_note_to_the_person_who_asked() {
    let firm = firm().await;
    let approval = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok()
    .approvals
    .remove(0);
    let denied = decide(
        &firm,
        &firm.pam,
        approval.id,
        AccessDecision {
            approve: false,
            note: Some("not this quarter".to_owned()),
            ..approve()
        },
    )
    .await
    .ok();
    assert_eq!(denied.state, ApprovalState::Denied);
    assert!(
        !holds(&firm, "nodes", firm.project, &firm.rae)
            .await
            .contains(&"project.interact".to_owned())
    );
    let all = firm
        .desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&firm.rae.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok();
    let told = all
        .items
        .iter()
        .find(|each| each.kind == NotificationKind::AccessDecided)
        .expect("told");
    assert!(told.title.contains("declined"), "{}", told.title);
    assert_eq!(told.body, "not this quarter");
}

#[tokio::test]
async fn a_request_that_nobody_answers_runs_out_and_is_not_decided_late() {
    let firm = firm().await;
    let approval = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok()
    .approvals
    .remove(0);
    firm.desk.world.advance(Duration::days(15));
    let read = firm
        .desk
        .world
        .app
        .call(ApprovalGet, &[&approval.id])
        .token(&firm.rae.token)
        .send()
        .await
        .ok();
    assert_eq!(read.state, ApprovalState::Expired);
    assert!(
        waiting_for(&firm, &firm.pam, true).await.is_empty(),
        "no longer waiting"
    );
    let late = decide(&firm, &firm.pam, approval.id, approve())
        .await
        .problem();
    assert_eq!(late.code, ErrorCode::ApprovalAlreadyDecided);
    assert!(
        !holds(&firm, "nodes", firm.project, &firm.rae)
            .await
            .contains(&"project.interact".to_owned())
    );
    firm.desk
        .world
        .app
        .app
        .engine
        .expire_access_requests()
        .await
        .unwrap();
    let decided: Page<Approval> = firm
        .desk
        .world
        .app
        .call(ApprovalList, &[])
        .token(&firm.rae.token)
        .query(&ApprovalListQuery {
            state: Some(ApprovalListState::Decided),
            scope: Some("requested".to_owned()),
            ..ApprovalListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(decided.items.len(), 1);
    assert_eq!(decided.items[0].state, ApprovalState::Expired);
    // Having run out, it no longer counts against what a person may have waiting.
    firm.desk
        .world
        .set("access.max_pending_requests", json!(1))
        .await;
    ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok();
}

#[tokio::test]
async fn a_person_may_have_only_so_many_batches_waiting_and_a_batch_counts_once() {
    let firm = firm().await;
    firm.desk
        .world
        .set("access.max_pending_requests", json!(2))
        .await;
    let topic = places::make_topic(&firm.desk.world, &firm.pam, firm.project, "Plans")
        .await
        .base
        .id;
    let second_topic =
        places::make_topic(&firm.desk.world, &firm.pam, firm.project, "Another plan")
            .await
            .base
            .id;
    let third_topic = places::make_topic(&firm.desk.world, &firm.pam, firm.project, "A third plan")
        .await
        .base
        .id;
    let first = ask(
        &firm,
        &firm.rae,
        vec![
            item("project", firm.project, &["project.interact"]),
            item("topic", topic, &["topic.interact"]),
        ],
    )
    .await
    .ok();
    ask(
        &firm,
        &firm.rae,
        vec![item("topic", second_topic, &["topic.interact"])],
    )
    .await
    .ok();
    let third = ask(
        &firm,
        &firm.rae,
        vec![item("topic", third_topic, &["topic.interact"])],
    )
    .await
    .problem();
    assert_eq!(third.code, ErrorCode::AccessTooManyRequests, "{third:?}");
    assert_eq!(third.extra["limit"], 2, "how many may wait");
    assert_eq!(third.extra["waiting"], 2, "and how many wait");
    assert_eq!(third.status, 409);
    firm.desk
        .world
        .app
        .call(ApprovalCancel, &[&first.approvals[0].id])
        .token(&firm.rae.token)
        .send()
        .await
        .ok();
    let still = ask(
        &firm,
        &firm.rae,
        vec![item("topic", third_topic, &["topic.interact"])],
    )
    .await
    .problem();
    assert_eq!(
        still.code,
        ErrorCode::AccessTooManyRequests,
        "the other approval of the batch still waits"
    );
    firm.desk
        .world
        .app
        .call(ApprovalCancel, &[&first.approvals[1].id])
        .token(&firm.rae.token)
        .send()
        .await
        .ok();
    ask(
        &firm,
        &firm.rae,
        vec![item("topic", third_topic, &["topic.interact"])],
    )
    .await
    .ok();
}

#[tokio::test]
async fn what_is_held_what_is_closed_and_what_is_made_up_is_not_sent() {
    let firm = firm().await;
    let held = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.view"])],
    )
    .await
    .problem();
    assert_eq!(
        held.code,
        ErrorCode::RequestInvalid,
        "held already: {held:?}"
    );
    let own = ask(
        &firm,
        &firm.pam,
        vec![item("project", firm.project, &["project.manage"])],
    )
    .await
    .problem();
    assert_eq!(
        own.code,
        ErrorCode::RequestInvalid,
        "an owner holds it: {own:?}"
    );
    let owner_row = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["access.own"])],
    )
    .await
    .problem();
    assert_eq!(
        owner_row.code,
        ErrorCode::RequestInvalid,
        "nobody asks to own"
    );
    let unknown = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.fly"])],
    )
    .await
    .problem();
    assert_eq!(unknown.code, ErrorCode::RequestInvalid);
    let wrong_kind = ask(
        &firm,
        &firm.rae,
        vec![item("thread", firm.project, &["thread.interact"])],
    )
    .await
    .problem();
    assert_eq!(
        wrong_kind.code,
        ErrorCode::NotFound,
        "a project is not a thread"
    );
    let nothing = ask(
        &firm,
        &firm.rae,
        vec![item("project", Id::nil(), &["project.interact"])],
    )
    .await
    .problem();
    assert_eq!(nothing.code, ErrorCode::NotFound);
    let a_chat = ask(
        &firm,
        &firm.rae,
        vec![item("chat", firm.project, &["chat.view"])],
    )
    .await
    .problem();
    assert_eq!(a_chat.code, ErrorCode::RequestInvalid);
    let empty = ask(&firm, &firm.rae, Vec::new()).await.problem();
    assert_eq!(empty.code, ErrorCode::RequestInvalid);
    let mut nine = Vec::new();
    for _ in 0..9 {
        nine.push(item("project", firm.project, &["project.interact"]));
    }
    assert_eq!(
        ask(&firm, &firm.rae, nine).await.problem().code,
        ErrorCode::RequestInvalid
    );
    // A lock above decides the row: nobody gives it on this page.
    firm.desk
        .world
        .app
        .call(NodeDefaultsUpdate, &[&firm.desk.org])
        .token(&firm.desk.ada.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "project.interact".to_owned(),
                profile: None,
                inherit: None,
                locked: Some(true),
                add: None,
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let closed = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .problem();
    assert_eq!(closed.code, ErrorCode::RequestInvalid);
    assert_eq!(closed.extra["closed"], "locked", "{closed:?}");
}

#[tokio::test]
async fn only_the_person_who_asked_reads_the_batch() {
    let firm = firm().await;
    let made = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok();
    for other in [&firm.pam, &firm.dee, &firm.sam] {
        let refused = firm
            .desk
            .world
            .app
            .call(AccessRequestGet, &[&made.batch_id])
            .token(&other.token)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::NotFound);
    }
    let own = firm
        .desk
        .world
        .app
        .call(AccessRequestGet, &[&made.batch_id])
        .token(&firm.rae.token)
        .send()
        .await
        .ok();
    assert_eq!(own.approvals.len(), 1);
    // The owner reads the approval itself, with the name of what it is about.
    let seen = firm
        .desk
        .world
        .app
        .call(ApprovalGet, &[&made.approvals[0].id])
        .token(&firm.pam.token)
        .send()
        .await
        .ok();
    assert!(seen.can_decide);
    assert_eq!(seen.object.as_ref().unwrap().name, "Pam's");
    let stranger = firm
        .desk
        .world
        .app
        .call(ApprovalGet, &[&made.approvals[0].id])
        .token(&firm.sam.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn nobody_decides_their_own_request_and_the_work_in_a_place_is_asked_for_only_by_whoever_manages_it()
 {
    let firm = firm().await;
    let approval = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok()
    .approvals
    .remove(0);
    let own = decide(&firm, &firm.rae, approval.id, approve())
        .await
        .problem();
    assert_eq!(own.code, ErrorCode::AuthzDenied);
    let withdrawn = firm
        .desk
        .world
        .app
        .call(ApprovalCancel, &[&approval.id])
        .token(&firm.rae.token)
        .send()
        .await
        .ok();
    assert_eq!(withdrawn.state, ApprovalState::Cancelled);
    let mut work = item("project", firm.project, &["project.interact"]);
    work.holder = Some(HolderInput::Node {
        id: firm.desk.topic,
    });
    let refused = ask(&firm, &firm.rae, vec![work]).await.problem();
    assert_eq!(
        refused.code,
        ErrorCode::RequestInvalid,
        "project.interact takes no work: {refused:?}"
    );
    // A request may name another person, whom its entry would name; the asker stays its author.
    let mut other = item("project", firm.project, &["project.interact"]);
    other.holder = Some(HolderInput::User { id: firm.sam.id() });
    let for_sam = ask(&firm, &firm.rae, vec![other])
        .await
        .ok()
        .approvals
        .remove(0);
    assert!(
        matches!(&for_sam.requester, orch_api_contract::approvals::ApprovalRequester::User(user) if user.id == firm.rae.id())
    );
    let ApprovalRequest::Access(request) = &for_sam.request else {
        panic!("{for_sam:?}");
    };
    assert!(
        matches!(&request.holder, orch_api_contract::access::Holder::User { user } if user.id == firm.sam.id())
    );
}

#[tokio::test]
async fn a_request_taken_back_is_read_by_those_it_was_sent_to_and_cannot_be_decided() {
    let firm = firm().await;
    let approval = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok()
    .approvals
    .remove(0);
    let back = firm
        .desk
        .world
        .app
        .call(ApprovalCancel, &[&approval.id])
        .token(&firm.rae.token)
        .send()
        .await
        .ok();
    assert_eq!(back.state, ApprovalState::Cancelled);
    let late = decide(&firm, &firm.pam, approval.id, approve())
        .await
        .problem();
    assert_eq!(late.code, ErrorCode::ApprovalAlreadyDecided);
    let pam_inbox = firm
        .desk
        .world
        .app
        .call(NotificationList, &[])
        .token(&firm.pam.token)
        .query(&NotificationListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        pam_inbox
            .items
            .iter()
            .filter(|each| each.kind == NotificationKind::AccessRequested)
            .all(|each| each.read_at.is_some()),
        "nothing is left to do about it"
    );
}

#[tokio::test]
async fn somebody_from_another_org_is_named_only_as_the_approver_may_name_an_outsider() {
    let firm = firm().await;
    // Bob is a viewer who owns a document: owning it is all that lets him name an outsider on it.
    let bob = firm.desk.member("bob", "viewer").await;
    let document = firm.desk.make(&bob, "Bob's notes").await.base.id;
    let olga = person(&firm.desk.world, "olga").await;
    let approval = ask(
        &firm,
        &olga,
        vec![item("document", document, &["doc.view"])],
    )
    .await
    .ok()
    .approvals
    .remove(0);
    // Where the policy lets owners name outsiders, the owner does; where it does not, he cannot,
    // and the admin of the org, who holds the right to name an outsider, can.
    firm.desk
        .world
        .set_at(
            orch_settings::Target::Node(firm.desk.org),
            "sharing.allow_external",
            json!(false),
        )
        .await;
    let refused = decide(&firm, &bob, approval.id, approve()).await.problem();
    assert_eq!(
        refused.code,
        ErrorCode::SharingExternalForbidden,
        "{refused:?}"
    );
    places::admin_mode(&firm.desk.world, &firm.dee, true).await;
    let decided = decide(&firm, &firm.dee, approval.id, approve()).await.ok();
    assert_eq!(decided.state, ApprovalState::Approved);
    assert!(
        holds(&firm, "documents", document, &olga)
            .await
            .contains(&"doc.view".to_owned())
    );
}

#[tokio::test]
async fn a_request_for_a_document_goes_to_the_people_who_may_change_its_page() {
    let firm = firm().await;
    let document = firm.desk.make(&firm.pam, "Plan").await.base.id;
    let made = ask(
        &firm,
        &firm.sam,
        vec![item("document", document, &["doc.view"])],
    )
    .await
    .ok();
    let approval = &made.approvals[0];
    let ApprovalRequest::Access(request) = &approval.request else {
        panic!("{approval:?}");
    };
    assert_eq!(request.permissions, ["doc.view"]);
    assert_eq!(waiting_for(&firm, &firm.pam, false).await, [approval.id]);
    decide(&firm, &firm.pam, approval.id, approve()).await.ok();
    assert!(
        holds(&firm, "documents", document, &firm.sam)
            .await
            .contains(&"doc.view".to_owned())
    );
    let page = firm
        .desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document])
        .token(&firm.sam.token)
        .body(&PageChange::default())
        .send()
        .await;
    assert!(
        page.body.is_err(),
        "viewing a document is not changing its page"
    );
}

#[tokio::test]
async fn nobody_who_is_not_signed_in_asks_reads_or_decides() {
    let firm = firm().await;
    let made = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok();
    let app = &firm.desk.world.app;
    let asked = app
        .call(AccessRequestCreate, &[])
        .body(&AccessRequestCreateBody {
            items: vec![item("project", firm.project, &["project.interact"])],
            reason: String::new(),
            for_thread: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(asked.code, ErrorCode::AuthRequired);
    let read = app
        .call(AccessRequestGet, &[&made.batch_id])
        .send()
        .await
        .problem();
    assert_eq!(read.code, ErrorCode::AuthRequired);
    let decided = app
        .call(ApprovalDecide, &[&made.approvals[0].id])
        .body(&ApprovalDecision::Access(approve()))
        .send()
        .await
        .problem();
    assert_eq!(decided.code, ErrorCode::AuthRequired);
}

async fn card(firm: &Firm, by: &Who, approval: Id) -> Approval {
    firm.desk
        .world
        .app
        .call(ApprovalGet, &[&approval])
        .token(&by.token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn somebody_who_manages_the_page_decides_only_what_they_could_have_given_themself() {
    let firm = firm().await;
    // Pam, who owns the project, lets Tom change its page. He is a Viewer of Acme, so he holds
    // nothing of the project but reading it.
    let tom = firm.desk.member("tom", "viewer").await;
    firm.desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &firm.project])
        .token(&firm.pam.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "access.manage".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::User { id: tom.id() },
                    options: None,
                    expires_at: None,
                    note: None,
                }]),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let approval = ask(
        &firm,
        &firm.rae,
        vec![item("project", firm.project, &["project.interact"])],
    )
    .await
    .ok()
    .approvals
    .remove(0);
    // He changes the page, so that the request was sent to him and he reads it; but its card says
    // beforehand that he cannot answer it, as it says that the owner can, and it does not wait for
    // him among the requests he can decide.
    assert!(!card(&firm, &tom, approval.id).await.can_decide);
    assert!(card(&firm, &firm.pam, approval.id).await.can_decide);
    assert!(!waiting_for(&firm, &tom, false).await.contains(&approval.id));
    assert!(
        waiting_for(&firm, &firm.pam, false)
            .await
            .contains(&approval.id)
    );
    let refused = decide(&firm, &tom, approval.id, approve()).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        refused.extra["required"], "project.interact",
        "he is refused for what he lacks, not for the page"
    );
    // Nothing was decided and nothing was given.
    assert_eq!(
        card(&firm, &firm.pam, approval.id).await.state,
        ApprovalState::Pending
    );
    assert!(
        !holds(&firm, "nodes", firm.project, &firm.rae)
            .await
            .contains(&"project.interact".to_owned())
    );
    // Once he holds what is asked, as a Developer of the project, he gives it.
    give_role(&firm.desk.world, firm.project, &tom, "developer", &firm.pam).await;
    assert!(card(&firm, &tom, approval.id).await.can_decide);
    let decided = decide(&firm, &tom, approval.id, approve()).await.ok();
    assert_eq!(decided.state, ApprovalState::Approved);
    assert!(
        holds(&firm, "nodes", firm.project, &firm.rae)
            .await
            .contains(&"project.interact".to_owned())
    );
}

#[tokio::test]
async fn a_request_made_from_a_refusal_asks_for_what_is_missing_on_the_thing_that_refused_and_opens_nothing_else()
 {
    let firm = firm().await;
    let plans = make_topic(&firm.desk.world, &firm.pam, firm.project, "Plans")
        .await
        .base
        .id;
    let other = make_topic(&firm.desk.world, &firm.pam, firm.project, "Other")
        .await
        .base
        .id;
    // Rae, who reads everything, is refused the renaming of the topic: the refusal names the topic,
    // not the project it lies in, and the permission of the topic that is missing.
    let revision = match read(&firm.desk.world, &firm.pam, plans).await {
        Node::Topic(topic) => topic.base.revision,
        other => panic!("{other:?}"),
    };
    let refused = firm
        .desk
        .world
        .app
        .call(NodeUpdate, &[&plans])
        .token(&firm.rae.token)
        .body(&NodeUpdateBody {
            readme_document_id: None,
            color: None,
            custom_color: None,
            name: Some("Plan".to_owned()),
            description: None,
            expected_revision: revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let missing = first_missing(&refused);
    assert_eq!(missing["object"]["kind"], "topic");
    assert_eq!(missing["object"]["id"], json!(plans));
    assert_eq!(missing["permissions"], json!(["topic.manage"]));
    // The request is made from that entry, as it stands.
    let kind = missing["object"]["kind"].as_str().unwrap().to_owned();
    let wanted: Vec<String> = missing["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|word| word.as_str().unwrap().to_owned())
        .collect();
    let wanted: Vec<&str> = wanted.iter().map(String::as_str).collect();
    let approval = ask(&firm, &firm.rae, vec![item(&kind, plans, &wanted)])
        .await
        .ok()
        .approvals
        .remove(0);
    let ApprovalRequest::Access(request) = &approval.request else {
        panic!("{approval:?}");
    };
    assert_eq!(request.permissions, ["topic.manage"]);
    assert_eq!(
        approval.object.as_ref().map(|object| object.id),
        Some(plans),
        "it is asked on the topic"
    );
    decide(&firm, &firm.pam, approval.id, approve()).await.ok();
    // Rae now manages that topic, and neither the project above it nor the topic beside it.
    assert!(
        holds(&firm, "nodes", plans, &firm.rae)
            .await
            .contains(&"topic.manage".to_owned())
    );
    assert!(
        !holds(&firm, "nodes", other, &firm.rae)
            .await
            .contains(&"topic.manage".to_owned())
    );
    assert!(
        !holds(&firm, "nodes", firm.project, &firm.rae)
            .await
            .iter()
            .any(|word| word == "project.manage" || word == "topic.manage")
    );
    let revision = match read(&firm.desk.world, &firm.pam, plans).await {
        Node::Topic(topic) => topic.base.revision,
        other => panic!("{other:?}"),
    };
    let renamed = firm
        .desk
        .world
        .app
        .call(NodeUpdate, &[&plans])
        .token(&firm.rae.token)
        .body(&NodeUpdateBody {
            readme_document_id: None,
            color: None,
            custom_color: None,
            name: Some("Plan".to_owned()),
            description: None,
            expected_revision: revision,
        })
        .send()
        .await;
    assert!(renamed.body.is_ok(), "{renamed:?}");
}
