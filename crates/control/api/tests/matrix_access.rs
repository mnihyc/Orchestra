//! The combination matrix of general access, roles and holders: a thread made with each of the five general
//! access choices (as its topic decides, everyone can view, can take part, can manage, only people
//! added), read for a person who holds each role (none, viewer, interactor, developer) at the org, the project or the
//! topic, directly or through a group that holds it at the topic, while the role is in force, after it expired and
//! after it was taken away.
//!
//! Expected: under "as the topic decides" the thread follows the shipped defaults, where a row naming a role names
//! everyone whose role is that one or higher at the node; under the three general choices everyone who
//! holds a role at the topic (roles reach downward; a group's members hold its role) holds the lines up
//! to the level chosen and none above; under "only people added" nobody but the owners; an expired or
//! removed role holds nothing. A person who may view the thread
//! reads it; one who may not is refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "../../engine/tests/matrix_kit/mod.rs"]
mod matrix_kit;
mod places;
mod rig;
mod support;

use matrix_kit::{Case, Matrix, Report, dim, failure};
use orch_api_contract::access::{
    AccessChoice, EffectiveAccessGet, EffectiveAccessGetQuery, PageChange, PermissionCatalogueGet,
    PermissionCatalogueGetReply, ThreadAccess,
};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, NodeGet, ThreadCreate};
use orch_db::queries::access::testing as access_rows;
use orch_types::Id;
use rig::{MODEL, Scene};
use serde_json::json;
use std::collections::HashSet;

const SOURCE: &str = "General access, role hierarchy, downward inheritance, group membership and expired or removed grants";

const THREAD_LINES: [&str; 3] = ["thread.view", "thread.interact", "thread.manage"];

/// The page change a choice makes, as the new-thread dialog sends it (the same as `tests/general_access.rs`).
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

fn rank(role: &str) -> u8 {
    match role {
        "viewer" => 1,
        "interactor" => 2,
        "developer" => 3,
        "admin" => 4,
        _ => 0,
    }
}

/// The thread permissions implied by the general-access choice and inherited role hierarchy.
fn expected(case: &Case) -> HashSet<&'static str> {
    let role = if case.is("state", "active") {
        case.get("role")
    } else {
        "none"
    };
    if role == "none" {
        return HashSet::new();
    }
    match case.get("preset") {
        "private" => HashSet::new(),
        "defaults" => THREAD_LINES
            .into_iter()
            .filter(|line| {
                orch_access_contract::pages::shipped()
                    .iter()
                    .filter(|entry| entry.permission.as_str() == *line)
                    .any(|entry| rank(entry.role.as_str()) <= rank(role))
            })
            .collect(),
        level => {
            let up_to = match level {
                "view" => 1,
                "interact" => 2,
                _ => 3,
            };
            THREAD_LINES.into_iter().take(up_to).collect()
        }
    }
}

async fn run(
    scene: &Scene,
    catalogue: &PermissionCatalogueGetReply,
    case: &Case,
) -> Result<(), matrix_kit::Failure> {
    let world = &scene.rig.world;
    let began = std::time::Instant::now();
    let org = scene.tree.org.base.id;
    let topic = scene.tree.topic.base.id;
    let who = places::person(world, &format!("mx{}", case.tag())).await;
    {
        let mut conn = world.conn().await;
        access_rows::insert_member(&mut conn, org, who.id(), org)
            .await
            .unwrap();
        let role = case.get("role");
        if role != "none" {
            let expires = case
                .is("state", "expired")
                .then(|| world.now() - chrono::Duration::days(1));
            let given = match case.get("held_at") {
                "org" => access_rows::give_role(
                    &mut conn,
                    org,
                    "user",
                    who.id(),
                    role,
                    scene.ada.id(),
                    expires,
                )
                .await
                .unwrap(),
                "project" => access_rows::give_role(
                    &mut conn,
                    scene.tree.project.base.id,
                    "user",
                    who.id(),
                    role,
                    scene.ada.id(),
                    expires,
                )
                .await
                .unwrap(),
                "topic" => access_rows::give_role(
                    &mut conn,
                    topic,
                    "user",
                    who.id(),
                    role,
                    scene.ada.id(),
                    expires,
                )
                .await
                .unwrap(),
                _ => {
                    let group = access_rows::insert_group(
                        &mut conn,
                        org,
                        &format!("group {}", case.tag()),
                        scene.ada.id(),
                    )
                    .await
                    .unwrap();
                    access_rows::add_to_group(&mut conn, group, who.id())
                        .await
                        .unwrap();
                    access_rows::give_role(
                        &mut conn,
                        topic,
                        "group",
                        group,
                        role,
                        scene.ada.id(),
                        expires,
                    )
                    .await
                    .unwrap()
                }
            };
            if case.is("state", "removed") {
                access_rows::take_role_away(&mut conn, given, scene.ada.id(), world.now())
                    .await
                    .unwrap();
            }
        }
    }
    let thread = world
        .app
        .call(ThreadCreate, &[&topic])
        .token(&scene.ada.token)
        .body(&CreateThread {
            name: Some(format!("General {}", case.tag())),
            binding: Some(CreateThreadBinding {
                agent_id: scene.on.agent,
                provider_id: scene.on.provider,
                arrangement: None,
            }),
            model: Some(MODEL.into()),
            environment: Some(EnvironmentChoice::Use(scene.on.environment)),
            access: Some(ThreadAccess::Change(preset(
                catalogue,
                topic,
                case.get("preset"),
            ))),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    let made = began.elapsed();
    let held: HashSet<String> = world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &thread.base.id])
        .token(&scene.ada.token)
        .query(&EffectiveAccessGetQuery { user_id: who.id() })
        .send()
        .await
        .ok()
        .permissions
        .into_iter()
        .map(|entry| entry.permission)
        .filter(|permission| THREAD_LINES.contains(&permission.as_str()))
        .collect();
    let want: HashSet<String> = expected(case).into_iter().map(str::to_owned).collect();
    if held != want {
        let mut want: Vec<_> = want.into_iter().collect();
        let mut held: Vec<_> = held.into_iter().collect();
        want.sort();
        held.sort();
        return Err(failure(format!("{want:?}"), SOURCE, format!("{held:?}")));
    }
    let read = world
        .app
        .call(NodeGet, &[&thread.base.id])
        .token(&who.token)
        .send()
        .await;
    if std::env::var_os("ORCH_MATRIX_TIMES").is_some() {
        eprintln!(
            "{}: made in {made:?}, all in {:?}",
            case.id(),
            began.elapsed()
        );
    }
    let may_view = want.contains("thread.view");
    if read.status.is_success() != may_view {
        return Err(failure(
            format!(
                "reading the thread {}",
                if may_view { "allowed" } else { "refused" }
            ),
            SOURCE,
            format!("HTTP {}", read.status),
        ));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_general_access_choice_gives_each_role_wherever_it_is_held_exactly_its_lines() {
    let matrix = Matrix::new(
        "access",
        &[
            dim(
                "preset",
                &["defaults", "view", "interact", "manage", "private"],
            ),
            dim("role", &["none", "viewer", "interactor", "developer"]),
            dim("held_at", &["org", "project", "topic", "group"]),
            dim("state", &["active", "expired", "removed"]),
        ],
        // Without a role there is nowhere it is held and nothing to expire.
        |pick| {
            !(pick.is("role", "none")
                && (!pick.is("held_at", "org") || !pick.is("state", "active")))
        },
    );
    // The product is small: every combination runs (as the pair cover and a product of all four).
    let mut cases = matrix.cases(
        13,
        &[&["preset", "role", "held_at", "state"]],
        20_261_015,
        0,
    );
    cases.retain(|case| case.kind == "full");
    let began = std::time::Instant::now();
    let scene = Scene::new().await;
    if std::env::var_os("ORCH_MATRIX_TIMES").is_some() {
        eprintln!("the scene in {:?}", began.elapsed());
    }
    let catalogue = scene
        .rig
        .world
        .app
        .call(PermissionCatalogueGet, &[])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let mut report = Report::default();
    for case in &cases {
        let outcome = run(&scene, &catalogue, case).await;
        report.record(case, outcome);
    }
    report.finish("access");
}
