//! Joining text is reviewed and recorded by the existing invitation transaction.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
#[path = "org_terms/home_rules.rs"]
mod home_rules;
mod places;
mod support;

#[path = "org_terms/chat_home.rs"]
mod chat_home;

use crate::support::FreshInvitationCreateReply as InvitationCreateReply;
use chrono::{DateTime, Utc};
use orch_api_contract::access::Role;
use orch_api_contract::auth::{AuthRegister, AuthRegisterBody, AuthRegisterReply};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{OrgMemberList, OrgMemberListQuery};
use orch_api_contract::users::{
    InvitationAccept, InvitationAcceptBody, InvitationAcceptReply, InvitationCreate,
    InvitationCreateBody, InvitationKind, InvitationLookup, InvitationTerms,
};
use orch_api_core::testing::Reply;
use orch_settings::Target;
use orch_types::Id;
use places::{Who, make_department, make_org, make_project, person};
use serde_json::json;
use sha2::{Digest, Sha256};
use support::{FullInvitationReply as _, PASSWORD, World};

fn offer(kind: InvitationKind, target: Id) -> InvitationCreateBody {
    InvitationCreateBody {
        kind,
        org_id: (kind == InvitationKind::Org).then_some(target),
        node_id: (kind == InvitationKind::Node).then_some(target),
        chat_id: (kind == InvitationKind::Chat).then_some(target),
        role: None,
        permissions: None,
        email: None,
        max_uses: None,
        expires_in_minutes: None,
        note: None,
    }
}

async fn invite(world: &World, by: &Who, body: &InvitationCreateBody) -> InvitationCreateReply {
    world
        .app
        .call(InvitationCreate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
        .full_invitation()
        .ok()
}

async fn terms(world: &World, token: &str) -> Option<InvitationTerms> {
    world
        .app
        .call(InvitationLookup, &[&token])
        .send()
        .await
        .ok()
        .terms
}

async fn accept(
    world: &World,
    by: &Who,
    token: &str,
    hash: Option<&str>,
) -> Reply<InvitationAcceptReply> {
    world
        .app
        .call(InvitationAccept, &[])
        .token(&by.token)
        .body(&InvitationAcceptBody {
            token: token.to_owned(),
            accepted_terms_hash: hash.map(str::to_owned),
        })
        .send()
        .await
        .full_invitation()
}

async fn untouched(world: &World, invitation: Id, root: Id, user: Id) {
    let counts: (i32, i64, i64, i64) = sqlx::query_as(
        "SELECT uses,
          (SELECT count(*) FROM invitation_uses WHERE invitation_id=$1),
          (SELECT count(*) FROM org_members WHERE org_id=$2 AND user_id=$3),
          (SELECT count(*) FROM audit_log WHERE action='member.terms_accepted' AND actor_id=$3)
         FROM invitations WHERE id=$1",
    )
    .bind(invitation)
    .bind(root)
    .bind(user)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0, 0, 0));
}

async fn audit_acceptance(
    world: &World,
    invitation: Id,
    org: Id,
    user: Id,
    user_name: &str,
    hash: &str,
) {
    let rows: Vec<(Id, Id, serde_json::Value)> = sqlx::query_as(
        "SELECT actor_id,object_id,details FROM audit_log WHERE action='member.terms_accepted'
           AND details->>'invitation_id'=$1",
    )
    .bind(invitation.to_string())
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].0, rows[0].1), (user, user));
    assert_eq!(
        rows[0].2,
        json!({"org_id":org,"user_id":user,"user_name":user_name,"terms_hash":hash,"invitation_id":invitation})
    );
}

#[tokio::test]
async fn missing_and_stale_acknowledgements_do_nothing_and_current_terms_join_once() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let guest = person(&world, "guest").await;
    let root = make_org(&world, &owner, "Office", "office").await.base.id;
    world
        .set_at(
            Target::Node(root),
            "org.terms",
            json!("# Office\nKeep shared work private."),
        )
        .await;
    let invitation = invite(&world, &owner, &offer(InvitationKind::Org, root)).await;
    let shown = terms(&world, &invitation.token).await.unwrap();
    assert_eq!(
        shown.hash,
        hex::encode(Sha256::digest(shown.text.as_bytes()))
    );
    for hash in [None, Some("outdated")] {
        let reply = accept(&world, &guest, &invitation.token, hash).await;
        assert_eq!(reply.status, 409);
        let problem = reply.problem();
        assert_eq!(problem.code, ErrorCode::InvitationTermsNotAccepted);
        assert_eq!(problem.extra["terms"], json!(shown));
        untouched(&world, invitation.invitation.id, root, guest.id()).await;
    }
    world.advance(chrono::Duration::minutes(1));
    let accepted = accept(&world, &guest, &invitation.token, Some(&shown.hash))
        .await
        .ok();
    assert_eq!(accepted.org.unwrap().base.id, root);
    let saved: (Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT terms_hash,terms_accepted_at FROM org_members WHERE org_id=$1 AND user_id=$2",
    )
    .bind(root)
    .bind(guest.id())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(saved, (Some(shown.hash.clone()), Some(world.now())));
    audit_acceptance(
        &world,
        invitation.invitation.id,
        root,
        guest.id(),
        &guest.person.username,
        &shown.hash,
    )
    .await;
    let members = world
        .app
        .call(OrgMemberList, &[&root])
        .token(&owner.token)
        .query(&OrgMemberListQuery::default())
        .send()
        .await
        .ok();
    let member = members
        .items
        .iter()
        .find(|m| m.user.id == guest.id())
        .unwrap();
    assert_eq!(member.terms_hash.as_deref(), Some(shown.hash.as_str()));
    assert_eq!(
        member.terms_accepted_at.map(|at| at.get()),
        Some(world.now())
    );
    let members = world
        .app
        .call(OrgMemberList, &[&root])
        .token(&guest.token)
        .query(&OrgMemberListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        members
            .items
            .iter()
            .all(|m| m.terms_hash.is_none() && m.terms_accepted_at.is_none())
    );
    assert_eq!(
        accept(&world, &guest, &invitation.token, Some(&shown.hash))
            .await
            .problem()
            .code,
        ErrorCode::Gone
    );
    audit_acceptance(
        &world,
        invitation.invitation.id,
        root,
        guest.id(),
        &guest.person.username,
        &shown.hash,
    )
    .await;
    world
        .set_at(
            Target::Node(root),
            "org.terms",
            json!("Changed for new people"),
        )
        .await;
    let members = world
        .app
        .call(OrgMemberList, &[&root])
        .token(&owner.token)
        .query(&OrgMemberListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        members
            .items
            .iter()
            .find(|m| m.user.id == guest.id())
            .unwrap()
            .terms_hash,
        Some(shown.hash)
    );
}

#[tokio::test]
async fn a_suborganization_joins_ancestor_text_in_order_and_records_its_actual_domain() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let guest = person(&world, "guest").await;
    let root = make_org(&world, &owner, "Office", "office").await.base.id;
    let child = make_department(&world, &owner, root, "Research", "research")
        .await
        .base
        .id;
    world
        .set_at(
            Target::Node(root),
            "org.terms",
            json!("# Whole organization\nKeep it private."),
        )
        .await;
    world
        .set_at(
            Target::Node(child),
            "org.terms",
            json!("# Research\nKeep experiment notes."),
        )
        .await;
    let invitation = invite(&world, &owner, &offer(InvitationKind::Org, child)).await;
    let shown = terms(&world, &invitation.token).await.unwrap();
    assert_eq!(
        shown.text,
        "# Whole organization\nKeep it private.\n\n# Research\nKeep experiment notes."
    );
    assert_eq!(
        shown.hash,
        hex::encode(Sha256::digest(shown.text.as_bytes()))
    );
    let accepted = accept(&world, &guest, &invitation.token, Some(&shown.hash))
        .await
        .ok();
    assert_eq!(accepted.org.unwrap().base.id, child);
    let saved: (Id, Option<String>) = sqlx::query_as(
        "SELECT domain_id,terms_hash FROM org_members WHERE org_id=$1 AND user_id=$2",
    )
    .bind(root)
    .bind(guest.id())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(saved, (child, Some(shown.hash.clone())));
    audit_acceptance(
        &world,
        invitation.invitation.id,
        child,
        guest.id(),
        &guest.person.username,
        &shown.hash,
    )
    .await;
}

#[tokio::test]
async fn place_roles_permission_entries_and_chat_guests_keep_their_own_acknowledgement() {
    let room = chat_world::Room::new().await;
    room.world
        .set_at(
            Target::Node(room.root),
            "org.terms",
            json!("Use the shared work responsibly."),
        )
        .await;
    let group = room
        .group(&room.ada, &[&room.bea], Some("Release"))
        .await
        .base
        .id;
    for (index, kind, target) in [
        (0, InvitationKind::Node, room.project),
        (1, InvitationKind::Node, room.project),
        (2, InvitationKind::Chat, group),
    ] {
        let guest = person(&room.world, &format!("outside{index}")).await;
        let mut body = offer(kind, target);
        if index == 0 {
            body.role = Some(Role::Viewer);
        } else if index == 1 {
            body.permissions = Some(vec!["project.view".into()]);
        }
        let invitation = invite(&room.world, &room.ada, &body).await;
        let shown = terms(&room.world, &invitation.token).await.unwrap();
        assert_eq!(shown.text, "Use the shared work responsibly.");
        assert_eq!(
            accept(&room.world, &guest, &invitation.token, None)
                .await
                .problem()
                .code,
            ErrorCode::InvitationTermsNotAccepted
        );
        untouched(&room.world, invitation.invitation.id, room.root, guest.id()).await;
        let grants: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM node_roles WHERE node_id=$1 AND subject_id=$2)
                  + (SELECT count(*) FROM permission_entries WHERE object_id=$1 AND holder_id=$2)
                  + (SELECT count(*) FROM chat_members WHERE chat_id=$1 AND user_id=$2)",
        )
        .bind(target)
        .bind(guest.id())
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
        assert_eq!(
            grants, 0,
            "refusal creates no guest grant of either native form"
        );
        let reply = accept(&room.world, &guest, &invitation.token, Some(&shown.hash))
            .await
            .ok();
        assert!(reply.org.is_none());
        let query = match index {
            0 => {
                "SELECT terms_hash,terms_accepted_at FROM node_roles WHERE node_id=$1 AND subject_id=$2 AND removed_at IS NULL"
            }
            1 => {
                "SELECT terms_hash,terms_accepted_at FROM permission_entries WHERE object_id=$1 AND holder_id=$2 AND permission='project.view' AND removed_at IS NULL"
            }
            _ => {
                "SELECT terms_hash,terms_accepted_at FROM chat_members WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL"
            }
        };
        let saved: (Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(query)
            .bind(target)
            .bind(guest.id())
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
        assert_eq!(saved, (Some(shown.hash.clone()), Some(room.world.now())));
        let membership: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM org_members WHERE org_id=$1 AND user_id=$2)",
        )
        .bind(room.root)
        .bind(guest.id())
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
        assert!(
            !membership,
            "acknowledgement never turns a guest into a tree member"
        );
        audit_acceptance(
            &room.world,
            invitation.invitation.id,
            room.root,
            guest.id(),
            &guest.person.username,
            &shown.hash,
        )
        .await;
    }
}

#[tokio::test]
async fn no_terms_and_system_invitations_keep_the_existing_admission_without_fabricating_consent() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let guest = person(&world, "guest").await;
    let root = make_org(&world, &owner, "Office", "office").await.base.id;
    let invitation = invite(&world, &owner, &offer(InvitationKind::Org, root)).await;
    assert!(terms(&world, &invitation.token).await.is_none());
    accept(&world, &guest, &invitation.token, None).await.ok();
    let record: (Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT terms_hash,terms_accepted_at FROM org_members WHERE org_id=$1 AND user_id=$2",
    )
    .bind(root)
    .bind(guest.id())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(record, (None, None));
    let admin = places::admin(&world, "admin").await;
    world
        .set_at(
            Target::Node(root),
            "org.terms",
            json!("Unrelated organization terms"),
        )
        .await;
    let system = invite(&world, &admin, &offer(InvitationKind::System, Id::nil())).await;
    assert!(terms(&world, &system.token).await.is_none());
    let outsider = person(&world, "outsider").await;
    accept(&world, &outsider, &system.token, None).await.ok();
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='member.terms_accepted'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(audits, 0);
}

#[tokio::test]
async fn registration_with_a_missing_or_stale_hash_rolls_back_the_account_home_session_and_join() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let root = make_org(&world, &owner, "Office", "office").await.base.id;
    world.set("registration.mode", json!("invite")).await;
    world
        .set_at(
            Target::Node(root),
            "org.terms",
            json!("Review these before joining."),
        )
        .await;
    let invitation = invite(&world, &owner, &offer(InvitationKind::Org, root)).await;
    let shown = terms(&world, &invitation.token).await.unwrap();
    let before: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM users),(SELECT count(*) FROM orgs),(SELECT count(*) FROM sessions)")
        .fetch_one(&world.db.pool).await.unwrap();
    for hash in [None, Some("outdated")] {
        let reply = world
            .app
            .call(AuthRegister, &[])
            .body(&AuthRegisterBody {
                username: "newperson".into(),
                display_name: "New Person".into(),
                password: PASSWORD.into(),
                email: None,
                invitation: Some(invitation.token.clone()),
                accept_invitation: true,
                accepted_terms_hash: hash.map(str::to_owned),
                timezone: None,
            })
            .send()
            .await;
        assert_eq!(reply.status, 409);
        let problem = reply.problem();
        assert_eq!(problem.code, ErrorCode::InvitationTermsNotAccepted);
        assert_eq!(problem.extra["terms"], json!(shown));
        let after: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM users),(SELECT count(*) FROM orgs),(SELECT count(*) FROM sessions)")
            .fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(after, before);
        let uses: i32 = sqlx::query_scalar("SELECT uses FROM invitations WHERE id=$1")
            .bind(invitation.invitation.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(uses, 0);
    }
    let reply = world
        .app
        .call(AuthRegister, &[])
        .body(&AuthRegisterBody {
            username: "newperson".into(),
            display_name: "New Person".into(),
            password: PASSWORD.into(),
            email: None,
            invitation: Some(invitation.token.clone()),
            accept_invitation: true,
            accepted_terms_hash: Some(shown.hash.clone()),
            timezone: None,
        })
        .send()
        .await
        .ok();
    assert!(matches!(reply, AuthRegisterReply::Registered(_)));
    let user: Id = sqlx::query_scalar("SELECT id FROM users WHERE username='newperson'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let saved: Option<String> =
        sqlx::query_scalar("SELECT terms_hash FROM org_members WHERE org_id=$1 AND user_id=$2")
            .bind(root)
            .bind(user)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(saved.as_deref(), Some(shown.hash.as_str()));
    audit_acceptance(
        &world,
        invitation.invitation.id,
        root,
        user,
        "New Person",
        &shown.hash,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_accept_waiting_for_a_terms_write_checks_the_new_text_including_an_absent_source() {
    for initial in [None, Some("Earlier terms")] {
        let world = World::new().await;
        let owner = person(&world, "owner").await;
        let guest = person(&world, "guest").await;
        let root = make_org(&world, &owner, "Office", "office").await.base.id;
        // A project also proves that the text source is its actual organization, not its target.
        let project = make_project(&world, &owner, root, "Work").await.base.id;
        if let Some(text) = initial {
            world
                .set_at(Target::Node(root), "org.terms", json!(text))
                .await;
        }
        let mut body = offer(InvitationKind::Node, project);
        body.role = Some(Role::Viewer);
        let invitation = invite(&world, &owner, &body).await;
        let old = terms(&world, &invitation.token).await;
        let mut writer = world.db.pool.begin().await.unwrap();
        orch_db::queries::settings::lock_org_terms(&mut writer, Some(root), true)
            .await
            .unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *writer)
            .await
            .unwrap();
        let running = accept(
            &world,
            &guest,
            &invitation.token,
            old.as_ref().map(|terms| terms.hash.as_str()),
        );
        let release = async {
            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                loop {
                    let waits: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                        .bind(blocker).fetch_one(&world.db.pool).await.unwrap();
                    if waits {break;}
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }).await.expect("acceptance reached the real organization text lock");
            // Use the native settings writer in the same transaction; its lock is reentrant.
            orch_settings::write::write(
                &mut writer,
                &orch_settings::write::Write {
                    actor: orch_db::audit::Actor::System,
                    target: Target::Node(root),
                    key: "org.terms",
                    value: json!("Current terms after the wait"),
                    locked: None,
                    expected_revision: None,
                    may_lock: false,
                    origin: &orch_db::audit::Origin::default(),
                },
                world.now(),
            )
            .await
            .unwrap();
            writer.commit().await.unwrap();
        };
        let (reply, ()) = tokio::join!(running, release);
        assert_eq!(reply.status, 409);
        let problem = reply.problem();
        assert_eq!(problem.code, ErrorCode::InvitationTermsNotAccepted);
        assert_eq!(
            problem.extra["terms"]["text"],
            "Current terms after the wait"
        );
        untouched(&world, invitation.invitation.id, root, guest.id()).await;
        let grants: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM node_roles WHERE node_id=$1 AND subject_id=$2",
        )
        .bind(project)
        .bind(guest.id())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(grants, 0);
    }
}
