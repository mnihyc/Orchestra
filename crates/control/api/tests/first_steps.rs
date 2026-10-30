//! First steps are served from readable stored facts and save only the caller's portable choices.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::{
    HolderInput, NodeDefaultsUpdate, PageChange, PageChangeAdd, PageChangeRow, Role,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::first_steps::{
    FirstStepsChoice, FirstStepsFact, FirstStepsGetQuery, FirstStepsId, FirstStepsMode,
    MeFirstStepsGet, MeFirstStepsSet,
};
use orch_api_contract::personal::MeStateSet;
use places::{make_org, person};
use serde_json::json;
use support::World;

#[tokio::test]
async fn an_org_home_reads_real_first_steps_without_a_provider_or_model_probe() {
    let world = World::new().await;
    let owner = person(&world, "first-owner").await;
    let org = make_org(&world, &owner, "First team", "first-team").await;
    let facts = world
        .app
        .call(MeFirstStepsGet, &[])
        .token(&owner.token)
        .query(&FirstStepsGetQuery {
            org_id: Some(org.base.id),
        })
        .send()
        .await
        .ok();
    assert_eq!(facts.org.unwrap().id, org.base.id);
    assert_eq!(facts.work_mode, None);
    assert!(
        facts
            .steps
            .iter()
            .any(|step| step.id == FirstStepsId::Team && step.fact == FirstStepsFact::ChooseOrg)
    );
    assert!(
        facts
            .steps
            .iter()
            .any(|step| step.id == FirstStepsId::Work && step.fact == FirstStepsFact::NotDone)
    );
    assert!(!facts.steps.iter().any(|step| step.id == FirstStepsId::Away));
    assert_eq!(facts.invitation_roles.len(), 4);
    assert!(
        facts
            .invitation_roles
            .iter()
            .all(|role| role.model_access_required)
    );
}

#[tokio::test]
async fn invitation_summaries_include_creation_rights_given_without_contribution() {
    let world = World::new().await;
    let owner = person(&world, "first-owner").await;
    let org = make_org(&world, &owner, "First team", "first-team").await;
    let rows = ["project.create_topics", "topic.start_threads"]
        .into_iter()
        .map(|permission| PageChangeRow {
            permission: permission.to_owned(),
            profile: None,
            inherit: Some(false),
            locked: None,
            add: Some(vec![PageChangeAdd {
                holder: HolderInput::Role {
                    role: Role::Viewer,
                    at: None,
                },
                options: None,
                expires_at: None,
                note: None,
            }]),
            update: None,
            remove: None,
        })
        .collect();
    world
        .app
        .call(NodeDefaultsUpdate, &[&org.base.id])
        .token(&owner.token)
        .body(&PageChange {
            rows: Some(rows),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let facts = world
        .app
        .call(MeFirstStepsGet, &[])
        .token(&owner.token)
        .query(&FirstStepsGetQuery {
            org_id: Some(org.base.id),
        })
        .send()
        .await
        .ok();
    let viewer = facts
        .invitation_roles
        .iter()
        .find(|summary| summary.role == Role::Viewer)
        .unwrap();
    assert!(viewer.start_topics);
    assert!(viewer.start_threads);
}

#[tokio::test]
async fn preferences_preserve_other_help_and_navigation_and_are_private_to_the_author() {
    let world = World::new().await;
    let owner = person(&world, "first-owner").await;
    let outsider = person(&world, "first-outsider").await;
    let org = make_org(&world, &owner, "First team", "first-team").await;
    world
        .app
        .call(MeStateSet, &[&"help"])
        .token(&owner.token)
        .body(&json!({"tutorials":{"one":"done"}}))
        .send()
        .await
        .ok();
    world
        .app
        .call(MeStateSet, &[&"sidebar"])
        .token(&owner.token)
        .body(&json!({"folded":["resources"]}))
        .send()
        .await
        .ok();
    let choice = FirstStepsChoice {
        org_id: Some(org.base.id),
        work_mode: Some(FirstStepsMode::Team),
        dismissed: true,
    };
    let state = world
        .app
        .call(MeFirstStepsSet, &[])
        .token(&owner.token)
        .body(&choice)
        .send()
        .await
        .ok();
    assert_eq!(state["help"]["tutorials"], json!({"one":"done"}));
    assert_eq!(state["sidebar"]["folded"], json!(["resources"]));
    assert_eq!(state["help"]["first_steps"]["dismissed"], true);
    assert_eq!(state["sidebar"]["last_org_id"], json!(org.base.id));
    let read = world
        .app
        .call(MeFirstStepsGet, &[])
        .token(&owner.token)
        .query(&FirstStepsGetQuery {
            org_id: Some(org.base.id),
        })
        .send()
        .await
        .ok();
    assert_eq!(read.work_mode, Some(FirstStepsMode::Team));
    let refused = world
        .app
        .call(MeFirstStepsGet, &[])
        .token(&outsider.token)
        .query(&FirstStepsGetQuery {
            org_id: Some(org.base.id),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let refused = world
        .app
        .call(MeFirstStepsSet, &[])
        .token(&outsider.token)
        .body(&choice)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}
