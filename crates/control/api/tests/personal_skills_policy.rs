//! The knowledge read explains inherited personal-skill policy without collecting private skills.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding case fails by panicking"
)]
mod places;
mod rig;
mod support;

use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeGet;
use orch_api_contract::settings::{NodeSettingList, NodeSettingSet, NodeSettingSetBody};
use orch_api_contract::share::{PersonalSkillsPolicy, ThreadSharedGet, ThreadSharedGetReply};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_settings::Target;
use orch_types::{Id, new_id};
use rig::Scene;
use serde_json::json;
use support::FullPermissionPageReply;

const KEY: &str = "agents.personal_skills";

async fn read(s: &Scene, token: &str, thread: Id) -> ThreadSharedGetReply {
    s.rig
        .world
        .app
        .call(ThreadSharedGet, &[&thread])
        .token(token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn inherited_personal_skills_off_reports_the_actual_false_contributor_and_changes_no_selection()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let initial = read(&s, &s.ada.token, thread).await;
    assert_eq!(initial.personal_skills, Some(PersonalSkillsPolicy::Allowed));
    // A later permissive row cannot erase the parent's All-policy restriction.
    s.rig
        .world
        .set_at(Target::Node(s.tree.topic.base.id), KEY, json!(true))
        .await;
    s.rig
        .world
        .set_at(Target::Node(s.tree.project.base.id), KEY, json!(false))
        .await;
    let before = s
        .rig
        .world
        .app
        .app
        .engine
        .skills_for_thread(thread)
        .await
        .unwrap();
    let commands = s.rig.commands.issued_total();
    let shown = read(&s, &s.ada.token, thread).await;
    let Some(PersonalSkillsPolicy::Off { reason, sources }) = &shown.personal_skills else {
        panic!("the native policy must explain Off: {shown:?}");
    };
    assert_eq!(reason, "policy_off");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].node.as_ref().unwrap().id, s.tree.project.base.id);
    assert_ne!(sources[0].node.as_ref().unwrap().id, s.tree.topic.base.id);
    assert_eq!(shown.shares, initial.shares);
    assert_eq!(shown.read_first_paths, initial.read_first_paths);
    assert_eq!(
        s.rig
            .world
            .app
            .app
            .engine
            .skills_for_thread(thread)
            .await
            .unwrap(),
        before
    );
    assert_eq!(s.rig.commands.issued_total(), commands);
    let settings = s
        .rig
        .world
        .app
        .call(NodeSettingList, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(
        settings.iter().all(|row| row.key != KEY),
        "thread-editable settings retain their scope"
    );
}

#[tokio::test]
async fn a_thread_viewer_reads_personal_policy_without_manage_or_private_ancestor_names() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let viewer = places::person(&s.rig.world, "policy-reader").await;
    s.rig.world.app.call(PermissionPageUpdate, &[&"nodes", &thread])
        .token(&s.ada.token)
        .raw_body(&json!({"name":[{"holders":[{"kind":"user","id":viewer.id()}],"choice":"full_view","export":false}]}))
        .send().await.full_permission_page().ok();
    s.rig
        .world
        .set_at(Target::Node(s.tree.project.base.id), KEY, json!(false))
        .await;
    let denied = s
        .rig
        .world
        .app
        .call(NodeGet, &[&s.tree.project.base.id])
        .token(&viewer.token)
        .send()
        .await
        .problem();
    assert!(matches!(
        denied.code,
        ErrorCode::AuthzDenied | ErrorCode::NotFound
    ));
    let denied = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&s.tree.project.base.id, &KEY])
        .token(&viewer.token)
        .body(&NodeSettingSetBody {
            value: json!(true),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert!(matches!(
        denied.code,
        ErrorCode::AuthzDenied | ErrorCode::NotFound
    ));
    let reply = read(&s, &viewer.token, thread).await;
    let Some(PersonalSkillsPolicy::Off { reason, sources }) = reply.personal_skills else {
        panic!("the effective restriction remains readable");
    };
    assert_eq!(reason, "policy_off");
    assert!(!sources.is_empty());
    assert!(sources.iter().all(|source| source.node.is_none()));
    let public = serde_json::to_string(&sources).unwrap();
    assert!(!public.contains(&s.tree.project.base.id.to_string()));
    assert!(!public.contains(&s.tree.project.base.name));
    assert!(!public.contains(&s.ada.id().to_string()));
}

#[tokio::test]
async fn a_scoped_knowledge_read_never_borrows_ancestor_names_from_its_account() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.rig
        .world
        .set_at(Target::Node(s.tree.project.base.id), KEY, json!(false))
        .await;
    s.rig
        .world
        .set_at(Target::Node(s.tree.topic.base.id), KEY, json!(false))
        .await;
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"thread","id":thread,"reach":"self"},
        "allow":[{"operation":"ThreadSharedGet","actions":["read"]}]
    }]}))
    .unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: s.ada.id(),
            name: "Narrow knowledge read",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    let reply = read(&s, &token, thread).await;
    let Some(PersonalSkillsPolicy::Off { sources, .. }) = reply.personal_skills else {
        panic!("the actual thread policy is returned to its admitted reader");
    };
    assert_eq!(
        sources.len(),
        1,
        "private contributors do not disclose a source count"
    );
    assert!(sources[0].node.is_none());
    let public = serde_json::to_string(&sources).unwrap();
    assert!(!public.contains(&s.tree.project.base.id.to_string()));
    assert!(!public.contains(&s.tree.topic.base.id.to_string()));
    assert!(!public.contains(&s.tree.project.base.name));
}

#[tokio::test]
async fn knowledge_policy_metadata_adds_no_thread_write_scope_or_policy_lock() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let _ = read(&s, &s.ada.token, thread).await;
    let denied = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&thread, &KEY])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(false),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::RequestInvalid);
    let denied = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&s.tree.project.base.id, &KEY])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(false),
            locked: Some(true),
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        denied.code,
        ErrorCode::RequestInvalid,
        "All-policy inheritance is not a parameter lock"
    );
}
