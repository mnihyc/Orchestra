//! Sharing reviews use the ordinary producers, show current effects and refusals, and publish no writes or work.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod rig;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    Holder, NodeRoleCreate, NodeRoleList, PermissionPageUpdate, SharePreview, SharePreviewBody,
    SharePreviewCreate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{GroupHolderAdd, OrgGroupCreate};
use orch_api_contract::documents::NodeDocumentCreate;
use orch_api_contract::nodes::NodeGet;
use orch_api_contract::share::{ShareAgentCreate, ShareDocumentsLevel};
use orch_types::Id;
use places::Who;
use serde_json::{Value, json};
use std::sync::Arc;

struct NoParcelWrites;
#[async_trait::async_trait]
impl orch_engine::sharing::Parcels for NoParcelWrites {
    async fn keep(
        &self,
        _conn: &mut orch_db::Conn,
        _id: Id,
        _path: &str,
        _files: &orch_engine::copies::Files,
    ) -> Result<Option<Vec<u8>>, orch_engine::sharing::ParcelFault> {
        panic!("a preview must not keep copy bytes");
    }
    async fn withdraw(
        &self,
        _conn: &mut orch_db::Conn,
        _copy: &orch_db::queries::shares::CopyRow,
    ) -> Result<(), orch_engine::sharing::ParcelFault> {
        panic!("a preview must not withdraw a copy");
    }
    async fn write_file(
        &self,
        _conn: &mut orch_db::Conn,
        _thread: Id,
        _path: &str,
        _bytes: &[u8],
    ) -> Result<orch_engine::sharing::FileWritten, orch_engine::sharing::ParcelFault> {
        panic!("a preview must not write into a workspace");
    }
}

async fn read(desk: &Desk, by: &Who, body: Value) -> SharePreview {
    desk.world
        .app
        .call(SharePreviewCreate, &[&"nodes", &desk.topic])
        .token(&by.token)
        .raw_body(&body)
        .send()
        .await
        .ok()
}
async fn private_with_reader(desk: &Desk, reader: &Who) {
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .ok();
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .raw_body(
            &json!({"name":[{"holders":[{"kind":"user","id":reader.id()}],"choice":"full_view"}]}),
        )
        .send()
        .await
        .ok();
}
async fn durable_counts(desk: &Desk) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM permission_entries),(SELECT count(*) FROM audit_log),(SELECT count(*) FROM thread_inputs)")
        .fetch_one(&desk.world.db.pool).await.unwrap()
}

#[tokio::test]
async fn a_holder_can_preview_the_same_real_share_without_being_a_page_manager_or_applying_it() {
    let desk = Desk::new().await;
    let sharer = desk.member("sharer", "viewer").await;
    let recipient = desk.member("recipient", "viewer").await;
    private_with_reader(&desk, &sharer).await;
    let change =
        json!({"name":[{"holders":[{"kind":"user","id":recipient.id()}],"choice":"full_view"}]});
    let counts = durable_counts(&desk).await;
    let preview = read(&desk, &sharer, json!({"kind":"page","change":change})).await;
    assert!(preview.allowed);
    assert!(preview.refusal.is_none());
    assert!(
        preview
            .effects
            .gaining
            .iter()
            .any(|person| person.user.id == recipient.id()
                && person.permissions.contains(&"topic.view".to_owned()))
    );
    assert_eq!(durable_counts(&desk).await, counts);
    let before = desk
        .world
        .app
        .call(NodeGet, &[&desk.topic])
        .token(&recipient.token)
        .send()
        .await;
    assert!(!before.status.is_success());
    let mut applying = change;
    applying["expected_revision"] = json!(preview.revision.unwrap());
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &desk.topic])
        .token(&sharer.token)
        .raw_body(&applying)
        .send()
        .await
        .ok();
    desk.world
        .app
        .call(NodeGet, &[&desk.topic])
        .token(&recipient.token)
        .send()
        .await
        .ok();
    let held = read(
        &desk,
        &sharer,
        json!({"kind":"page","change":change_for(recipient.id())}),
    )
    .await;
    assert!(held.effects.gaining.is_empty());
    assert!(
        held.already
            .iter()
            .any(|person| person.user.id == recipient.id()
                && person.permissions.contains(&"topic.view".to_owned()))
    );
}

fn change_for(person: Id) -> Value {
    json!({"name":[{"holders":[{"kind":"user","id":person}],"choice":"full_view"}]})
}

#[tokio::test]
async fn higher_unheld_rights_are_a_blocker_and_losing_the_sharing_policy_ends_preview_authority() {
    let desk = Desk::new().await;
    let sharer = desk.member("sharer", "viewer").await;
    let recipient = desk.member("recipient", "viewer").await;
    private_with_reader(&desk, &sharer).await;
    let counts = durable_counts(&desk).await;
    let denied=read(&desk,&sharer,json!({"kind":"page","change":{"name":[{"holders":[{"kind":"user","id":recipient.id()}],"choice":"full_manage"}]}})).await;
    assert!(!denied.allowed);
    assert!(denied.refusal.is_some());
    assert!(denied.holders.is_empty() && denied.effects.gaining.is_empty());
    assert_eq!(durable_counts(&desk).await, counts);
    desk.world
        .set_at(
            orch_settings::Target::Node(desk.topic),
            "sharing.allow_holders",
            json!(false),
        )
        .await;
    let refused = desk
        .world
        .app
        .call(SharePreviewCreate, &[&"nodes", &desk.topic])
        .token(&sharer.token)
        .raw_body(&json!({"kind":"page","change":change_for(recipient.id())}))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let hidden = desk
        .world
        .app
        .call(SharePreviewCreate, &[&"nodes", &desk.topic])
        .token(&recipient.token)
        .raw_body(&json!({"kind":"page","change":change_for(sharer.id())}))
        .send()
        .await;
    assert!(!hidden.status.is_success());
}

#[tokio::test]
async fn a_group_preview_names_real_people_once_and_a_role_preview_is_not_a_role_assignment() {
    let desk = Desk::new().await;
    let one = desk.member("one", "viewer").await;
    let two = desk.member("two", "viewer").await;
    private_with_reader(&desk, &one).await;
    let group = desk
        .world
        .app
        .call(OrgGroupCreate, &[&desk.org])
        .token(&desk.ada.token)
        .raw_body(&json!({"name":"Crew"}))
        .send()
        .await
        .ok();
    for person in [&one, &two] {
        desk.world
            .app
            .call(GroupHolderAdd, &[&group.id])
            .token(&desk.ada.token)
            .raw_body(&json!({"kind":"user","id":person.id()}))
            .send()
            .await
            .ok();
    }
    let preview=read(&desk,&one,json!({"kind":"page","change":{"name":[{"holders":[{"kind":"group","id":group.id}],"choice":"full_view"}]}})).await;
    assert!(preview.allowed);
    assert!(preview.holders.iter().any(|holder| matches!(holder,Holder::Group {group:shown} if shown.id == group.id && shown.member_count == 2)));
    assert_eq!(
        preview
            .effects
            .gaining
            .iter()
            .filter(|person| person.user.id == two.id())
            .count(),
        1
    );
    assert!(
        preview
            .already
            .iter()
            .any(|person| person.user.id == one.id())
    );
    let give = json!({"subject":{"kind":"user","id":two.id()},"role":"developer"});
    let planned = read(&desk, &desk.ada, json!({"kind":"role","give":give})).await;
    assert!(planned.allowed);
    assert_eq!(
        planned.effects.pages, 1,
        "the source page is not counted again among its descendants"
    );
    let roles = desk
        .world
        .app
        .call(NodeRoleList, &[&desk.topic])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(!roles.own.iter().any(|role| role.subject.id == two.id()));
    desk.world
        .app
        .call(NodeRoleCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .raw_body(&give)
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn an_agent_preview_uses_the_live_producer_and_never_wakes_or_accepts_an_input() {
    let s = rig::Scene::new().await;
    // Only an agent whose harness takes Orch's tools can be shared with (the live producer refuses
    // `no_tools` otherwise), as in the tests of shares.
    s.rig.tools_of_agents(json!("mcp")).await;
    let receiver = s.thread_by(&s.ada, "Reader").await;
    let id = s.tree.topic.base.id;
    let share = json!({"what":{"node_id":id},"inside":false,"download":false,"read_first":true,"parts":{"documents":"read"},"mode":"live","to":[{"thread_id":receiver.base.id}],"client_id":"review-only"});
    let wakeups = s.rig.commands.issued_of("turn.start");
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries")
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    let body: SharePreviewBody =
        serde_json::from_value(json!({"kind":"agents","share":share})).unwrap();
    let preview = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"nodes", &id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(preview.allowed, "{preview:?}");
    assert_eq!(preview.agents.len(), 1);
    assert_eq!(
        preview.agents[0].parts.documents,
        Some(ShareDocumentsLevel::Read)
    );
    assert!(!preview.agents[0].inside && !preview.agents[0].future_threads);
    assert!(
        preview
            .rows
            .iter()
            .any(|row| row.permission == "doc.view"
                && row.after.iter().any(|entry| entry.read_first))
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries")
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(s.rig.commands.issued_of("turn.start"), wakeups);
    s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.ada.token)
        .raw_body(&share)
        .send()
        .await
        .ok();
    let again = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"nodes", &id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(
        again.agents[0].already_shared.documents,
        Some(ShareDocumentsLevel::Read)
    );
}

#[tokio::test]
async fn port_capability_policy_is_shown_for_later_use_and_does_not_become_a_second_holder_gate() {
    let s = rig::Scene::new().await;
    // The capability definitions an installation seeds; a world made for a test has none of its own.
    {
        let mut tx = s.rig.world.db.pool.begin().await.unwrap();
        orch_db::seeds::seed(
            &mut tx,
            &orch_db::seeds::Release {
                environment_image: "registry.example/orch-test-env:1",
                environment_image_digest: None,
                adapters: &[],
            },
            s.rig.world.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    s.rig
        .world
        .set(
            "capability.policy",
            json!({"port_host":{"decision":"deny","approver_scope":"environment"}}),
        )
        .await;
    let preview=s.rig.world.app.call(SharePreviewCreate,&[&"environments",&s.on.environment]).token(&s.ada.token)
        .raw_body(&json!({"kind":"page","change":{"rows":[{"permission":"ports.manage","add":[{"holder":{"kind":"user","id":s.cai.id()}}]}]}})).send().await.ok();
    assert!(
        preview.allowed,
        "a port-use denial does not forbid giving the ports.manage row"
    );
    let policy = preview
        .rules
        .iter()
        .find(|rule| rule.key == "capability.policy.port_host")
        .unwrap();
    assert_eq!(policy.value["decision"], "deny");
    assert!(!policy.sharing);
}

#[tokio::test]
async fn a_copy_review_measures_real_material_and_checks_its_comment_without_keeping_or_delivering_it()
 {
    let s = rig::Scene::new().await;
    let receiver = s.thread_by(&s.ada, "Copy reader").await;
    let id = s.tree.topic.base.id;
    let mut document = desk::body("Checklist");
    document.blocks = Some(vec![desk::paragraph(
        "Read the checklist before changing the workspace.",
    )]);
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&id])
        .token(&s.ada.token)
        .body(&document)
        .send()
        .await
        .ok();
    s.rig
        .world
        .app
        .app
        .engine
        .attach_parcels(Arc::new(NoParcelWrites));
    let before:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM agent_shares),(SELECT count(*) FROM thread_inputs),(SELECT count(*) FROM audit_log)")
        .fetch_one(&s.rig.world.db.pool).await.unwrap();
    let wakeups = s.rig.commands.issued_of("turn.start");
    let share = json!({"what":{"node_id":id},"parts":{"documents":"read"},"mode":"snapshot","to":[{"thread_id":receiver.base.id}],"comment":"Please read this copy","client_id":"unaccepted-copy-review"});
    let preview = s
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"nodes", &id])
        .token(&s.ada.token)
        .raw_body(&json!({"kind":"agents","share":share}))
        .send()
        .await
        .ok();
    assert!(preview.allowed);
    let size = preview.agents[0].copy.as_ref().unwrap();
    assert!(
        size.files >= 2 && size.bytes > 0,
        "the copy contains the real document as well as its index"
    );
    assert!(!size.already_sent);
    let after:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM agent_shares),(SELECT count(*) FROM thread_inputs),(SELECT count(*) FROM audit_log)")
        .fetch_one(&s.rig.world.db.pool).await.unwrap();
    assert_eq!(before, after);
    assert_eq!(s.rig.commands.issued_of("turn.start"), wakeups);
}

#[tokio::test]
async fn a_review_keeps_the_real_proposed_option_bounds_and_expiry_on_each_row() {
    let s = rig::Scene::new().await;
    let until = s.rig.world.now() + chrono::Duration::days(1);
    let preview=s.rig.world.app.call(SharePreviewCreate,&[&"providers",&s.on.provider]).token(&s.ada.token)
        .raw_body(&json!({"kind":"page","change":{"rows":[{"permission":"provider.use","add":[{"holder":{"kind":"user","id":s.cai.id()},"options":{"models":[rig::MODEL]},"expires_at":orch_types::Time::new(until)}]}]}})).send().await.ok();
    assert!(preview.allowed);
    let row = preview
        .rows
        .iter()
        .find(|row| row.permission == "provider.use")
        .unwrap();
    let entry = row
        .after
        .iter()
        .find(|entry| matches!(&entry.holder,Holder::User {user} if user.id == s.cai.id()))
        .unwrap();
    assert_eq!(entry.options["models"], json!([rig::MODEL]));
    assert_eq!(entry.expires_at, Some(orch_types::Time::new(until)));
    let before:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='provider' AND object_id=$1 AND holder_kind='user' AND holder_id=$2")
        .bind(s.on.provider).bind(s.cai.id()).fetch_one(&s.rig.world.db.pool).await.unwrap();
    assert_eq!(before, 0, "trial identifiers are not accepted entries");
}
