//! Real enrollment and resource routes keep a person's new machine visible to its owner and private from other members.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{MachineEnvironmentCreate, MachineEnvironmentCreateBody};
use orch_api_contract::resources::{
    MachineEnrollmentTokenCreate, MachineEnrollmentTokenCreateBody, MachineGet, MachineList,
};
use orch_link_contract::enrollment::{EnrollReply, EnrollRequest, EnrollmentToken};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_types::{Id, new_id};
use places::{Who, admin, join, make_department, make_org, person};
use support::World;
use tower::ServiceExt as _;

/// Runs the actual internal enrollment route. Only its already authenticated gateway transport is a fixture.
async fn enroll(world: &World, token: &str, name: &str) -> Id {
    let pending = orch_pki::enrollment::PendingEnrollment::start().unwrap();
    let request = EnrollRequest {
        machine_id: None,
        token: token.into(),
        csr_pem: pending.csr_pem().into(),
        name: name.into(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".into(),
        protocol_versions: vec![1],
        roles: vec![Role::Agent, Role::Environment],
    };
    let router = orch_api_core::certificates::internal_routes()
        .with_state(world.app.app.clone())
        .layer(axum::Extension(orch_api_core::server::Peer {
            service: Some(orch_pki::certificates::ServiceName::Gateway),
            certificate: None,
            remote: "127.0.0.1:41001".parse().unwrap(),
        }));
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(u64::from(
            orch_link_contract::enrollment::REQUEST_TIMEOUT_SECS,
        )),
        router.oneshot(
            http::Request::post("/internal/link/enroll")
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&request).unwrap(),
                ))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), 201);
    // A leaf and CA chain are small; a bounded read also catches an accidental unbounded reply.
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let reply: EnrollReply = serde_json::from_slice(&bytes).unwrap();
    let parsed: EnrollmentToken = token.parse().unwrap();
    let identity = pending.complete(&parsed, &reply, &world.app.clock).unwrap();
    assert_eq!(identity.machine_id, reply.machine_id);
    reply.machine_id
}

fn enrollment_request(token: &str) -> EnrollRequest {
    let pending = orch_pki::enrollment::PendingEnrollment::start().unwrap();
    EnrollRequest {
        machine_id: None,
        token: token.to_owned(),
        csr_pem: pending.csr_pem().to_owned(),
        name: "test machine".to_owned(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".to_owned(),
        protocol_versions: vec![1],
        roles: vec![Role::Agent, Role::Environment],
    }
}

#[tokio::test]
async fn an_enrollment_token_stops_when_the_issuing_session_or_team_authority_ends() {
    use orch_api_contract::directory::{OrgMemberSuspend, OrgMemberSuspendBody};
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let maker = person(&world, "maker").await;
    let team = make_org(&world, &owner, "Team", "enroll-issuer")
        .await
        .base
        .id;
    join(&world, team, team, &maker, "developer", &owner).await;
    // Machine creation at a team home needs settings management unless its policy lists machines.
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", team, "live"),
        "any",
        "settings.manage",
        ("user", Some(maker.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let issue = |home| MachineEnrollmentTokenCreateBody {
        home_id: Some(home),
        ..Default::default()
    };
    let team_token = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&maker.token)
        .body(&issue(team))
        .send()
        .await
        .ok()
        .token;
    let own_token = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&maker.token)
        .body(&issue(maker.person.personal_org))
        .send()
        .await
        .ok()
        .token;
    let bearer = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::API_TOKEN_PREFIX,
    );
    let mut conn = world.conn().await;
    let api_id = orch_db::queries::identity::testing::insert_api_token(
        &mut conn,
        maker.id(),
        &orch_api_core::middleware::authentication::token_hash(&bearer),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE api_tokens SET scopes=ARRAY['*'] WHERE id=$1")
        .bind(api_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let token_issued_by_api = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&bearer)
        .body(&issue(maker.person.personal_org))
        .send()
        .await
        .ok()
        .token;
    let control = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&owner.token)
        .body(&issue(owner.person.personal_org))
        .send()
        .await
        .ok()
        .token;
    let created = world
        .app
        .app
        .link
        .certificates()
        .enroll(
            &enrollment_request(&control),
            &orch_db::audit::Origin::default(),
        )
        .await
        .expect("an unchanged issuer can enroll");
    let again = world
        .app
        .call(
            orch_api_contract::resources::MachineEnrollmentAgainTokenCreate,
            &[&created.machine_id],
        )
        .token(&owner.token)
        .send()
        .await
        .ok()
        .token;
    world
        .app
        .call(OrgMemberSuspend, &[&team, &maker.id()])
        .token(&owner.token)
        .body(&OrgMemberSuspendBody::default())
        .send()
        .await
        .ok();
    let denied = world
        .app
        .app
        .link
        .certificates()
        .enroll(
            &enrollment_request(&team_token),
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.status, 410);
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(api_id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let denied = world
        .app
        .app
        .link
        .certificates()
        .enroll(
            &enrollment_request(&token_issued_by_api),
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.status, 410, "the issuing API token was revoked");
    sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE user_id=$1")
        .bind(maker.id())
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let denied = world
        .app
        .app
        .link
        .certificates()
        .enroll(
            &enrollment_request(&own_token),
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.status, 410);
    sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE user_id=$1")
        .bind(owner.id())
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let denied = world
        .app
        .app
        .link
        .certificates()
        .enroll(
            &enrollment_request(&again),
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        denied.status, 410,
        "a replacement identity requires its issuing session"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM machines WHERE name='test machine'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "only the unchanged issuer's machine was created");
}

async fn visible_and_private(
    world: &World,
    owner: &Who,
    other: &Who,
    administrator: &Who,
    machine: Id,
    home: Id,
    root: Id,
) {
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    orch_api_core::routes::settings::install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    let (actual_home, actual_root, actual_owner): (Option<Id>, Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT home_id,org_id,owner_id FROM machines WHERE id=$1")
            .bind(machine)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        (actual_home, actual_root, actual_owner),
        (Some(home), Some(root), Some(owner.id()))
    );
    let mine = world
        .app
        .call(MachineList, &[])
        .token(&owner.token)
        .send()
        .await
        .ok();
    let row = mine
        .iter()
        .find(|row| row.base.id == machine)
        .expect("the person sees the machine they enrolled");
    assert!(
        row.access
            .as_ref()
            .unwrap()
            .permissions
            .iter()
            .any(|permission| permission == "machine.use")
    );
    let opened = world
        .app
        .call(MachineGet, &[&machine])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert_eq!(opened.owner.as_ref().unwrap().id, owner.id());
    assert!(
        opened.access.unwrap().admin_access.is_none(),
        "ordinary ownership needs no admin mode"
    );
    assert!(
        world
            .app
            .call(MachineList, &[])
            .token(&other.token)
            .send()
            .await
            .ok()
            .iter()
            .all(|row| row.base.id != machine)
    );
    assert_eq!(
        world
            .app
            .call(MachineGet, &[&machine])
            .token(&other.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let inspected = world
        .app
        .call(MachineGet, &[&machine])
        .token(&administrator.token)
        .header("x-orch-opening", &new_id().to_string())
        .send()
        .await
        .ok();
    assert_eq!(inspected.owner.as_ref().unwrap().id, owner.id());
    assert!(
        inspected.access.unwrap().admin_access.is_some(),
        "an As admin inspection states its source"
    );

    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv/enrollment".into()],
        ..Default::default()
    };
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(machine)
        .bind(serde_json::to_value(policy).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let registered = world
        .app
        .call(MachineEnvironmentCreate, &[&machine])
        .token(&owner.token)
        .body(&MachineEnvironmentCreateBody {
            data_sources: None,
            home_id: home,
            name: format!("Work {machine}"),
            root: "/srv/enrollment/work".into(),
            home_node_id: None,
            remote_write: None,
            terminal: None,
            access: Some(orch_api_contract::binding::NewEnvironmentAccess::OnlyMe),
        })
        .send()
        .await
        .ok();
    assert_eq!(registered.base.name, format!("Work {machine}"));
    assert_eq!(registered.machine.as_ref().unwrap().id, machine);
}

#[tokio::test]
async fn a_name_only_token_enrolls_at_its_owners_personal_home_and_its_real_resource_routes_work() {
    let world = World::new().await;
    let alice = person(&world, "alice").await;
    let bob = person(&world, "bob").await;
    let inspector = admin(&world, "inspector").await;
    let team = make_org(&world, &alice, "Team", "enrollment-team")
        .await
        .base
        .id;
    join(&world, team, team, &bob, "viewer", &alice).await;
    let token = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&alice.token)
        .body(&MachineEnrollmentTokenCreateBody {
            name_hint: Some("alice-laptop".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let parsed: EnrollmentToken = token.token.parse().unwrap();
    let home: Option<Id> =
        sqlx::query_scalar("SELECT org_id FROM enrollment_tokens WHERE token_hash=$1")
            .bind(hex::decode(orch_pki::enrollment::token_digest(parsed.secret())).unwrap())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(home, Some(alice.person.personal_org));
    let machine = enroll(&world, &token.token, "alice-laptop").await;
    visible_and_private(
        &world,
        &alice,
        &bob,
        &inspector,
        machine,
        alice.person.personal_org,
        alice.person.personal_org,
    )
    .await;
}

#[tokio::test]
async fn an_explicit_department_is_bound_to_the_token_and_other_members_still_have_no_machine_entry()
 {
    let world = World::new().await;
    let alice = person(&world, "alice").await;
    let bob = person(&world, "bob").await;
    let inspector = admin(&world, "inspector").await;
    let team = make_org(&world, &alice, "Team", "enrollment-home")
        .await
        .base
        .id;
    let home = make_department(&world, &alice, team, "Development", "development")
        .await
        .base
        .id;
    join(&world, team, team, &bob, "viewer", &alice).await;
    let token = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&alice.token)
        .body(&MachineEnrollmentTokenCreateBody {
            home_id: Some(home),
            name_hint: Some("team-laptop".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let machine = enroll(&world, &token.token, "team-laptop").await;
    visible_and_private(&world, &alice, &bob, &inspector, machine, home, team).await;
    let refused = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&bob.token)
        .body(&MachineEnrollmentTokenCreateBody {
            home_id: Some(alice.person.personal_org),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "a home choice is not authority to create a machine there"
    );
}

#[tokio::test]
async fn an_older_unspent_token_without_a_home_still_enrolls_at_the_persons_home() {
    let world = World::new().await;
    let alice = person(&world, "alice").await;
    let token = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&alice.token)
        .body(&MachineEnrollmentTokenCreateBody::default())
        .send()
        .await
        .ok();
    let parsed: EnrollmentToken = token.token.parse().unwrap();
    sqlx::query("UPDATE enrollment_tokens SET org_id=NULL WHERE token_hash=$1")
        .bind(hex::decode(orch_pki::enrollment::token_digest(parsed.secret())).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let machine = enroll(&world, &token.token, "older-command").await;
    let home: (Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT home_id,org_id FROM machines WHERE id=$1")
            .bind(machine)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        home,
        (
            Some(alice.person.personal_org),
            Some(alice.person.personal_org)
        )
    );
    assert!(
        world
            .app
            .call(MachineList, &[])
            .token(&alice.token)
            .send()
            .await
            .ok()
            .iter()
            .any(|row| row.base.id == machine)
    );
    assert_eq!(
        world
            .app
            .call(MachineGet, &[&machine])
            .token(&alice.token)
            .send()
            .await
            .ok()
            .owner
            .unwrap()
            .id,
        alice.id()
    );
}
#[tokio::test]
async fn a_later_admin_mode_cannot_increase_an_enrollment_tokens_issuer_authority() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let maker = person(&world, "maker").await;
    let team = make_org(&world, &owner, "Team", "enrollment-ceiling")
        .await
        .base
        .id;
    join(&world, team, team, &maker, "viewer", &owner).await;
    let mut conn = world.conn().await;
    let entry = orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", team, "live"),
        "any",
        "settings.manage",
        ("user", Some(maker.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let token = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&maker.token)
        .body(&MachineEnrollmentTokenCreateBody {
            home_id: Some(team),
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .token;
    let mut conn = world.conn().await;
    orch_db::queries::access::remove_entry(&mut conn, entry, owner.id(), world.now())
        .await
        .unwrap();
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(maker.id())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    places::admin_mode(&world, &maker, true).await;
    let denied = world
        .app
        .app
        .link
        .certificates()
        .enroll(
            &enrollment_request(&token),
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        denied.status, 410,
        "authority enabled after issuance cannot rescue the token"
    );
}
