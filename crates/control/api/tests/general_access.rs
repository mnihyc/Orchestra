//! General audiences at creation and a local edit of one inherited row, over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::access::{
    AccessChoice, EffectiveAccessGet, EffectiveAccessGetQuery, NodeDefaultsUpdate, PageChange,
    PermissionCatalogueGet, PermissionCatalogueGetReply, PermissionPageGet, PermissionPageUpdate,
    ThreadAccess,
};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, NodeGet, ThreadCreate};
use orch_types::Id;
use rig::{MODEL, Scene};
use serde_json::json;
use std::collections::HashSet;

fn preset(catalogue: &PermissionCatalogueGetReply, topic: Id, level: &str) -> PageChange {
    if level == "defaults" {
        return serde_json::from_value(json!({ "whole": "reset" })).unwrap();
    }
    let highest = match level {
        "view" => 1,
        "interact" => 2,
        "manage" => 3,
        _ => 0,
    };
    let mut held = HashSet::new();
    for choice in [
        AccessChoice::FullView,
        AccessChoice::FullInteract,
        AccessChoice::FullManage,
    ]
    .into_iter()
    .take(highest)
    {
        held.extend(
            catalogue
                .choices
                .iter()
                .find(|entry| entry.key == choice)
                .unwrap()
                .permissions["thread"]
                .iter()
                .cloned(),
        );
    }
    let rows: Vec<_> = catalogue
        .permissions
        .iter()
        .filter(|def| def.on.iter().any(|kind| kind == "thread") && def.name != "access.own")
        .map(|def| {
            let mut row = json!({ "permission": def.name, "inherit": false });
            if held.contains(&def.name) {
                row["add"] =
                    json!([{ "holder": { "kind": "role", "role": "viewer", "at": topic } }]);
            }
            row
        })
        .collect();
    serde_json::from_value(json!({ "rows": rows })).unwrap()
}

async fn held(s: &Scene, node: Id, user: Id) -> HashSet<String> {
    s.rig
        .world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &node])
        .token(&s.ada.token)
        .query(&EffectiveAccessGetQuery { user_id: user })
        .send()
        .await
        .ok()
        .permissions
        .into_iter()
        .map(|entry| entry.permission)
        .collect()
}

#[tokio::test]
async fn every_general_preset_at_creation_gives_a_viewer_role_member_exactly_its_lines() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let catalogue = s
        .rig
        .world
        .app
        .call(PermissionCatalogueGet, &[])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    for level in ["defaults", "view", "interact", "manage", "private"] {
        let thread = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&topic])
            .token(&s.ada.token)
            .body(&CreateThread {
                name: Some(format!("General {level}")),
                binding: Some(CreateThreadBinding {
                    agent_id: s.on.agent,
                    provider_id: s.on.provider,
                    arrangement: None,
                }),
                model: Some(MODEL.into()),
                environment: Some(EnvironmentChoice::Use(s.on.environment)),
                access: Some(ThreadAccess::Change(preset(&catalogue, topic, level))),
                ..CreateThread::default()
            })
            .send()
            .await
            .ok();
        let viewer = held(&s, thread.base.id, s.cai.id()).await;
        for (permission, expected) in [
            ("thread.view", level != "private"),
            ("thread.interact", matches!(level, "interact" | "manage")),
            ("thread.manage", level == "manage"),
            ("chat.view", level != "private"),
            (
                "chat.interact",
                matches!(level, "defaults" | "interact" | "manage"),
            ),
            ("chat.manage", level == "manage"),
            ("doc.view", level != "private"),
            ("doc.edit", matches!(level, "interact" | "manage")),
            ("doc.manage", level == "manage"),
        ] {
            assert_eq!(
                viewer.contains(permission),
                expected,
                "{level}: {permission} in {viewer:?}"
            );
        }
        if level != "private" {
            s.rig
                .world
                .app
                .call(NodeGet, &[&thread.base.id])
                .token(&s.cai.token)
                .send()
                .await
                .ok();
        }
        let page = s
            .rig
            .world
            .app
            .call(PermissionPageGet, &[&"nodes", &thread.base.id])
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        assert!(
            page.rows
                .iter()
                .all(|row| row.inherit == (level == "defaults")),
            "{level}: every row is settled at creation"
        );
        assert_eq!(page.owners.len(), 1, "its maker remains the owner");
    }
}

#[tokio::test]
async fn a_person_added_keeps_their_own_level_when_general_access_is_lowered() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    let topic = s.tree.topic.base.id;
    let catalogue = s
        .rig
        .world
        .app
        .call(PermissionCatalogueGet, &[])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let mut manage = preset(&catalogue, topic, "manage");
    manage.name = Some(
        serde_json::from_value(
            json!([{ "choice": "full_manage", "holders": [{ "kind": "user", "id": s.eve.id() }] }]),
        )
        .unwrap(),
    );
    let page = s
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread.base.id])
        .token(&s.ada.token)
        .body(&manage)
        .send()
        .await
        .ok();
    let mut view = preset(&catalogue, topic, "view");
    view.expected_revision = Some(page.revision);
    for row in view.rows.as_mut().unwrap() {
        let old = page
            .rows
            .iter()
            .find(|entry| entry.permission == row.permission)
            .unwrap();
        let general: Vec<_> = old.entries.iter().filter(|entry| matches!(&entry.holder, orch_api_contract::access::Holder::Role { role: orch_api_contract::access::Role::Viewer, at: Some(at), .. } if at.id == topic)).collect();
        if row.add.is_some() {
            row.add = None;
            row.update = Some(serde_json::from_value(json!(general.iter().map(|entry| json!({ "entry_id": entry.id, "expires_at": null, "options": {} })).collect::<Vec<_>>())).unwrap());
        } else {
            row.remove = Some(general.iter().map(|entry| entry.id).collect());
        }
    }
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread.base.id])
        .token(&s.ada.token)
        .body(&view)
        .send()
        .await
        .ok();
    let viewer = held(&s, thread.base.id, s.cai.id()).await;
    assert!(viewer.contains("thread.view"));
    assert!(!viewer.contains("thread.interact"));
    assert!(!viewer.contains("thread.manage"));
    assert!(
        held(&s, thread.base.id, s.eve.id())
            .await
            .contains("thread.manage"),
        "the person named on this page keeps their level"
    );
}

#[tokio::test]
async fn editing_one_followed_row_freezes_its_other_holders_and_other_rows_follow_later_changes() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let defaults: PageChange = serde_json::from_value(json!({ "rows": [{ "permission": "thread.interact", "profile": "managed", "inherit": false, "add": [
        { "holder": { "kind": "role", "role": "interactor" } }, { "holder": { "kind": "role", "role": "developer" } }
    ] }] })).unwrap();
    s.rig
        .world
        .app
        .call(NodeDefaultsUpdate, &[&topic])
        .token(&s.ada.token)
        .body(&defaults)
        .send()
        .await
        .ok();
    let thread = s.thread().await;
    let page = s
        .rig
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &thread.base.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let edit: PageChange = serde_json::from_value(json!({ "expected_revision": page.revision, "rows": [{ "permission": "thread.interact", "inherit": false, "add": [{ "holder": { "kind": "role", "role": "developer" } }] }] })).unwrap();
    let local = s
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread.base.id])
        .token(&s.ada.token)
        .body(&edit)
        .send()
        .await
        .ok();
    assert!(
        !local
            .rows
            .iter()
            .find(|row| row.permission == "thread.interact")
            .unwrap()
            .inherit
    );
    assert!(
        local
            .rows
            .iter()
            .find(|row| row.permission == "doc.view")
            .unwrap()
            .inherit
    );
    assert!(
        held(&s, thread.base.id, s.cai.id())
            .await
            .contains("doc.view")
    );
    let later: PageChange = serde_json::from_value(json!({ "rows": [
        { "permission": "thread.interact", "profile": "managed", "add": [{ "holder": { "kind": "role", "role": "viewer" } }] },
        { "permission": "doc.view", "inherit": false, "add": [{ "holder": { "kind": "role", "role": "developer" } }] }
    ] })).unwrap();
    s.rig
        .world
        .app
        .call(NodeDefaultsUpdate, &[&topic])
        .token(&s.ada.token)
        .body(&later)
        .send()
        .await
        .ok();
    let viewer = held(&s, thread.base.id, s.cai.id()).await;
    assert!(
        !viewer.contains("thread.interact"),
        "the edited row no longer follows a later Viewer grant"
    );
    assert!(
        !viewer.contains("doc.view"),
        "the untouched row still follows the later restriction"
    );
    assert!(
        held(&s, thread.base.id, s.dan.id())
            .await
            .contains("thread.interact"),
        "the frozen Developer holder is kept"
    );
}

#[tokio::test]
async fn an_inherited_entry_exposes_its_end_for_a_local_edit_and_keeps_it_when_copied() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let end = s.rig.world.now() + chrono::Duration::days(1);
    let defaults: PageChange = serde_json::from_value(json!({ "rows": [{ "permission": "doc.edit", "inherit": false, "add": [
        { "holder": { "kind": "role", "role": "interactor" } },
        { "holder": { "kind": "user", "id": s.cai.id() }, "expires_at": end, "note": "Until tomorrow" }
    ] }] })).unwrap();
    s.rig
        .world
        .app
        .call(NodeDefaultsUpdate, &[&topic])
        .token(&s.ada.token)
        .body(&defaults)
        .send()
        .await
        .ok();
    let thread = s.thread().await;
    let before = s
        .rig
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &thread.base.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let inherited = &before
        .rows
        .iter()
        .find(|row| row.permission == "doc.edit")
        .unwrap()
        .inherited
        .as_ref()
        .unwrap()
        .entries;
    let eve = inherited.iter().find(|entry| matches!(&entry.holder, orch_api_contract::access::Holder::User { user } if user.id == s.cai.id())).unwrap();
    assert_eq!(eve.expires_at.map(|time| time.get()), Some(end));
    let edit: PageChange = serde_json::from_value(json!({ "expected_revision": before.revision, "rows": [{ "permission": "doc.edit", "inherit": false, "add": [{
        "holder": { "kind": "user", "id": s.cai.id() }, "options": eve.options, "expires_at": eve.expires_at, "note": eve.note
    }] }] })).unwrap();
    let after = s
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread.base.id])
        .token(&s.ada.token)
        .body(&edit)
        .send()
        .await
        .ok();
    let kept = &after
        .rows
        .iter()
        .find(|row| row.permission == "doc.edit")
        .unwrap()
        .entries[0];
    assert_eq!(kept.expires_at, eve.expires_at);
    assert_eq!(kept.options, eve.options);
    assert_eq!(kept.note, "Until tomorrow");
    assert!(
        held(&s, thread.base.id, s.cai.id())
            .await
            .contains("doc.edit")
    );
    s.rig.world.advance(chrono::Duration::days(1));
    assert!(
        !held(&s, thread.base.id, s.cai.id())
            .await
            .contains("doc.edit"),
        "copying an inherited grant must not extend its life"
    );
}
