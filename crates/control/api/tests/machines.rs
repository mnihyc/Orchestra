//! A person's machines over HTTP: the token a machine enrolls with, listing and reading with presence and what was registered from each, renaming, revoking, and the local policy with its keys.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    MachineEnrollmentTokenCreate, MachineEnrollmentTokenCreateBody, MachineGet, MachineList,
    MachinePolicyGet, MachinePolicyUpdate, MachineProbe, MachineRevoke, MachineStatus,
    MachineUpdate, MachineUpdateBody, PolicyBounds,
};
use orch_db::queries::resources::{access_fixtures as fixtures, set_presence};
use orch_link_contract::policy::LocalPolicy;
use orch_types::{Id, Json};
use places::{Who, admin, make_org, person};
use serde_json::json;
use std::time::Duration;
use support::{Statements, World};

/// A machine of `owner`'s that has reported `policy` and handed over the keys in `delegated`.
async fn machine_with_policy(world: &World, owner: &Who, delegated: &[&str]) -> Id {
    let mut conn = world.conn().await;
    let machine = fixtures::insert_persons_machine(&mut conn, owner.id(), "laptop")
        .await
        .unwrap();
    let policy = LocalPolicy {
        policy_delegated: delegated.iter().map(|key| (*key).to_owned()).collect(),
        ..LocalPolicy::default()
    };
    sqlx::query(
        "UPDATE machines SET roles=ARRAY['environment','agent','provider'],facts = jsonb_build_object('policy', $2::jsonb) WHERE id = $1",
    )
    .bind(machine)
    .bind(serde_json::to_value(&policy).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    machine
}

/// Plays the machine: when it is sent a command it answers with `result`. The commands wait in
/// the database; the answer is written in their place.
fn answer(world: &World, machine: Id, result: Json) -> tokio::task::JoinHandle<()> {
    let pool = world.db.pool.clone();
    tokio::spawn(async move {
        for _ in 0..400 {
            let answered = sqlx::query(
                "UPDATE commands SET state = 'succeeded', result = $2
                  WHERE machine_id = $1 AND state IN ('queued', 'sent', 'accepted')",
            )
            .bind(machine)
            .bind(&result)
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected();
            if answered > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
}

async fn online(world: &World, machine: Id) {
    let mut conn = world.conn().await;
    set_presence(&mut conn, machine, true, None, world.now())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_person_lists_the_machines_they_own_and_nobody_elses() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let mut conn = world.conn().await;
    let laptop = fixtures::insert_persons_machine(&mut conn, ada.id(), "laptop")
        .await
        .unwrap();
    fixtures::insert_persons_machine(&mut conn, bea.id(), "desktop")
        .await
        .unwrap();
    drop(conn);
    let listed = world
        .app
        .call(MachineList, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].base.id, laptop);
    assert_eq!(listed[0].base.name, "laptop");
    assert_eq!(listed[0].owner.as_ref().unwrap().username, "ada");
    assert_eq!(listed[0].status, MachineStatus::Active);
    let none = world.app.call(MachineList, &[]).send().await.problem();
    assert_eq!(none.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_list_of_machines_costs_the_same_statements_for_one_and_for_many() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let mut conn = world.conn().await;
    fixtures::insert_persons_machine(&mut conn, ada.id(), "one")
        .await
        .unwrap();
    drop(conn);
    async fn measure(world: &World, who: &Who) -> (usize, usize) {
        let call = world.app.call(MachineList, &[]).token(&who.token);
        let before = Statements::now();
        let listed = call.send().await.ok();
        (Statements::now() - before, listed.len())
    }
    let (few, few_count) = measure(&world, &ada).await;
    let mut conn = world.conn().await;
    for index in 0..6 {
        fixtures::insert_persons_machine(&mut conn, ada.id(), &format!("m{index}"))
            .await
            .unwrap();
    }
    drop(conn);
    let (many, many_count) = measure(&world, &ada).await;
    assert_eq!((few_count, many_count), (1, 7));
    assert_eq!(few, many, "{few} statements for 1 machine, {many} for 7");
}

#[tokio::test]
async fn upgrade_repairs_an_owned_machine_home_without_opening_it_to_other_members_or_bypassing_membership()
 {
    let world = World::new().await;
    let owner = person(&world, "legacy-owner").await;
    let member = person(&world, "same-org-member").await;
    let operator = admin(&world, "operator").await;
    let machine = machine_with_policy(&world, &owner, &[]).await;
    // A member of the owner's org with its lowest role: a role is viewer, interactor, developer or admin.
    places::join(
        &world,
        owner.person.personal_org,
        owner.person.personal_org,
        &member,
        "viewer",
        &owner,
    )
    .await;
    let mut conn = world.conn().await;
    sqlx::query("UPDATE machines SET home_id=NULL,org_id=NULL WHERE id=$1")
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        orch_db::migrations::repair_machine_homes(&mut conn)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        orch_db::migrations::repair_machine_homes(&mut conn)
            .await
            .unwrap(),
        0,
        "the upgrade repair is idempotent"
    );
    let home: (Id, Id) = sqlx::query_as("SELECT home_id,org_id FROM machines WHERE id=$1")
        .bind(machine)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(home, (owner.person.personal_org, owner.person.personal_org));
    drop(conn);
    let listed = world
        .app
        .call(MachineList, &[])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert!(listed.iter().any(|row| row.base.id == machine));
    let read = world
        .app
        .call(MachineGet, &[&machine])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert!(
        read.access
            .unwrap()
            .permissions
            .iter()
            .any(|key| key == "machine.manage")
    );
    assert!(
        !world
            .app
            .call(MachineList, &[])
            .token(&member.token)
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
            .token(&member.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let administrative = world
        .app
        .call(MachineGet, &[&machine])
        .token(&operator.token)
        .send()
        .await
        .ok()
        .access
        .unwrap();
    assert!(
        administrative
            .admin_only
            .iter()
            .any(|key| key == "machine.view")
    );
    assert!(!administrative.participating);
    // Stored ownership remains bounded by root membership after repair.
    sqlx::query("DELETE FROM org_members WHERE org_id=$1 AND user_id=$2")
        .bind(owner.person.personal_org)
        .bind(owner.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        world
            .app
            .call(MachineGet, &[&machine])
            .token(&owner.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn an_enrollment_token_is_the_callers_and_comes_with_the_commands_that_use_it() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let token = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&ada.token)
        .body(&MachineEnrollmentTokenCreateBody {
            name_hint: Some("  ada's laptop ".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(token.token.starts_with("oenr_"), "{}", token.token);
    assert!(token.expires_at.get() > world.now());
    for command in [
        &token.commands.linux,
        &token.commands.macos,
        &token.commands.windows,
    ] {
        assert!(command.contains(&token.token), "{command}");
        assert!(command.contains("https://orch.test"), "{command}");
    }
    assert!(token.commands.windows.contains("install.ps1"));
    // The token is kept as a hash, for the person who asked, to make a machine of theirs.
    let rows: Vec<(Id, String, Option<String>, Option<Id>)> =
        sqlx::query_as("SELECT created_by, purpose, name_hint, machine_id FROM enrollment_tokens")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![(
            ada.id(),
            "connected".to_owned(),
            Some("ada's laptop".to_owned()),
            None
        )]
    );
    let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT token_hash FROM enrollment_tokens")
        .fetch_all(&world.db.pool)
        .await
        .unwrap();
    assert!(
        stored
            .iter()
            .all(|hash| !String::from_utf8_lossy(hash).contains(&token.token)),
        "the token itself is not stored"
    );
    let blank = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&ada.token)
        .body(&MachineEnrollmentTokenCreateBody {
            name_hint: Some("   ".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(blank.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn enrollment_roles_sections_and_dedication_are_bound_to_the_token_and_each_platforms_command()
 {
    use orch_api_contract::resources::MachineEnrollmentRole;
    use orch_link_contract::{
        enrollment::{EnrollRequest, EnrollmentToken},
        session::{Arch, Os, Platform, Role},
    };
    let world = World::new().await;
    let owner = person(&world, "role-owner").await;
    let made = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&owner.token)
        .body(&MachineEnrollmentTokenCreateBody {
            home_id: None,
            name_hint: Some("workstation".into()),
            roles: Some(vec![
                MachineEnrollmentRole::Environment,
                MachineEnrollmentRole::Provider,
            ]),
            delegate: Some(vec!["environment".into()]),
            dedicated: Some(true),
        })
        .send()
        .await
        .ok();
    let parsed: EnrollmentToken = made.token.parse().unwrap();
    let choices = parsed.choices().unwrap().unwrap();
    assert_eq!(choices.roles, vec![Role::Environment, Role::Provider]);
    assert_eq!(choices.delegate, ["environment.*"]);
    assert!(choices.dedicated);
    for command in [&made.commands.linux, &made.commands.macos] {
        assert!(
            command.contains("--roles 'environment,provider'"),
            "{command}"
        );
        assert!(command.contains("--delegate 'environment'"));
        assert!(command.ends_with("--dedicated"));
        assert!(!command.contains("--delegate-policy"));
        assert!(!command.contains("install-node"));
    }
    assert!(
        made.commands
            .windows
            .contains("-Roles 'environment,provider'")
    );
    assert!(made.commands.windows.contains("-Delegate 'environment'"));
    assert!(made.commands.windows.ends_with("-Dedicated"));
    let digest = orch_pki::enrollment::token_digest(parsed.secret());
    let raw = hex::decode(digest).unwrap();
    let (purpose, roles): (String, Vec<String>) =
        sqlx::query_as("SELECT purpose,roles FROM enrollment_tokens WHERE token_hash=$1")
            .bind(&raw)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(purpose, "machine");
    assert_eq!(roles, ["environment", "provider"]);
    let pending = orch_pki::enrollment::PendingEnrollment::start().unwrap();
    let mut request = EnrollRequest {
        machine_id: None,
        token: made.token.clone(),
        csr_pem: pending.csr_pem().to_owned(),
        name: "workstation".into(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".into(),
        protocol_versions: vec![1],
        roles: vec![Role::Agent],
    };
    assert_eq!(
        world
            .app
            .app
            .link
            .certificates()
            .enroll(&request, &Default::default())
            .await
            .unwrap_err()
            .status,
        400
    );
    let used: bool =
        sqlx::query_scalar("SELECT used_at IS NOT NULL FROM enrollment_tokens WHERE token_hash=$1")
            .bind(&raw)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(!used, "an unaccepted role cannot spend the valid token");
    request.roles = choices.roles;
    let enrolled = world
        .app
        .app
        .link
        .certificates()
        .enroll(&request, &Default::default())
        .await
        .unwrap();
    let actual: Vec<String> = sqlx::query_scalar("SELECT roles FROM machines WHERE id=$1")
        .bind(enrolled.machine_id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(actual, ["environment", "provider"]);
    let all = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&owner.token)
        .body(&MachineEnrollmentTokenCreateBody {
            roles: Some(vec![MachineEnrollmentRole::Agent]),
            delegate: Some(vec!["*".into()]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(all.commands.linux.ends_with("--delegate-policy"));
    assert!(all.commands.windows.ends_with("-DelegatePolicy"));
}

#[tokio::test]
async fn enrollment_refuses_no_roles_and_handover_of_an_unselected_role_without_making_a_token() {
    use orch_api_contract::resources::MachineEnrollmentRole;
    let world = World::new().await;
    let owner = person(&world, "role-owner").await;
    for body in [
        MachineEnrollmentTokenCreateBody {
            roles: Some(Vec::new()),
            ..Default::default()
        },
        MachineEnrollmentTokenCreateBody {
            roles: Some(vec![MachineEnrollmentRole::Environment]),
            delegate: Some(vec!["provider".into()]),
            ..Default::default()
        },
    ] {
        let failure = world
            .app
            .call(MachineEnrollmentTokenCreate, &[])
            .token(&owner.token)
            .body(&body)
            .send()
            .await
            .problem();
        assert_eq!(failure.code, ErrorCode::RequestInvalid);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(
        serde_json::from_value::<MachineEnrollmentTokenCreateBody>(json!({"roles":["node"]}))
            .is_err(),
        "Node has its own host enrollment"
    );
}

#[tokio::test]
async fn a_machine_is_read_by_its_owner_and_a_stranger_is_told_it_is_not_there() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let machine = machine_with_policy(&world, &ada, &[]).await;
    online(&world, machine).await;
    let read = world
        .app
        .call(MachineGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.base.id, machine);
    assert!(read.base.online, "the row says it had a session");
    assert!(
        read.facts.get("$link").is_none(),
        "what the link keeps of its certificate is not shown"
    );
    assert!(read.facts.get("policy").is_some());
    let stranger = world
        .app
        .call(MachineGet, &[&machine])
        .token(&bea.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::NotFound);
    let nobody = world
        .app
        .call(MachineGet, &[&machine])
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    let missing = world
        .app
        .call(MachineGet, &[&orch_types::new_id()])
        .token(&ada.token)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_machine_that_is_not_connected_says_so() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let machine = machine_with_policy(&world, &ada, &[]).await;
    let read = world
        .app
        .call(MachineGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(!read.base.online);
    online(&world, machine).await;
    let read = world
        .app
        .call(MachineGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(read.base.online);
}

#[tokio::test]
async fn a_machine_lists_what_was_registered_from_it() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await;
    let machine = machine_with_policy(&world, &ada, &[]).await;
    let mut conn = world.conn().await;
    let provider = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(ada.id()),
        Some(acme.base.id),
        "Home key",
    )
    .await
    .unwrap();
    let agent = fixtures::insert_resource(
        &mut conn,
        "agent",
        Some(ada.id()),
        Some(acme.base.id),
        "Codex",
    )
    .await
    .unwrap();
    let environment = fixtures::insert_resource(
        &mut conn,
        "environment",
        Some(ada.id()),
        Some(acme.base.id),
        "work",
    )
    .await
    .unwrap();
    for (kind, id) in [
        ("provider", provider),
        ("agent", agent),
        ("environment", environment),
    ] {
        fixtures::register_on(&mut conn, kind, id, machine)
            .await
            .unwrap();
    }
    // Something of ada's that is on another machine is not listed here.
    let other = fixtures::insert_persons_machine(&mut conn, ada.id(), "desk")
        .await
        .unwrap();
    let elsewhere = fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(ada.id()),
        Some(acme.base.id),
        "Other",
    )
    .await
    .unwrap();
    fixtures::register_on(&mut conn, "provider", elsewhere, other)
        .await
        .unwrap();
    drop(conn);
    let read = world
        .app
        .call(MachineGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        read.providers
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["Home key"]
    );
    assert_eq!(read.providers[0].address, "acme.ada.Home key");
    assert_eq!(
        read.agents
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["Codex"]
    );
    assert_eq!(read.agents[0].machine.as_ref().unwrap().id, machine);
    assert_eq!(
        read.environments
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["work"]
    );
    assert_eq!(read.environments[0].owner.username, "ada");
}

#[tokio::test]
async fn a_machine_is_renamed_by_its_owner_and_the_change_is_audited() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let machine = machine_with_policy(&world, &ada, &[]).await;
    let renamed = world
        .app
        .call(MachineUpdate, &[&machine])
        .token(&ada.token)
        .body(&MachineUpdateBody {
            name: "  Ada's laptop ".to_owned(),
        })
        .send()
        .await
        .ok();
    assert_eq!(renamed.base.name, "Ada's laptop");
    let audited: Vec<(String, String, Option<Id>)> = sqlx::query_as(
        "SELECT action, object_kind, object_id FROM audit_log WHERE action = 'machine.renamed'",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        audited,
        vec![(
            "machine.renamed".to_owned(),
            "machine".to_owned(),
            Some(machine)
        )]
    );
    for name in ["", "   ", &"x".repeat(81)] {
        let refused = world
            .app
            .call(MachineUpdate, &[&machine])
            .token(&ada.token)
            .body(&MachineUpdateBody {
                name: name.to_owned(),
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{name:?}");
    }
    let stranger = world
        .app
        .call(MachineUpdate, &[&machine])
        .token(&bea.token)
        .body(&MachineUpdateBody {
            name: "mine now".to_owned(),
        })
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::NotFound);
    let still = world
        .app
        .call(MachineGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(still.base.name, "Ada's laptop");
}

#[tokio::test]
async fn a_machine_is_revoked_by_its_owner_or_a_system_admin_and_by_nobody_else() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let root = admin(&world, "root").await;
    let first = machine_with_policy(&world, &ada, &[]).await;
    let second = machine_with_policy(&world, &ada, &[]).await;
    let stranger = world
        .app
        .call(MachineRevoke, &[&first])
        .token(&bea.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::NotFound);
    let by_owner = world
        .app
        .call(MachineRevoke, &[&first])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(by_owner.status, MachineStatus::Revoked);
    assert!(!by_owner.base.online);
    let by_admin = world
        .app
        .call(MachineRevoke, &[&second])
        .token(&root.token)
        .send()
        .await
        .ok();
    assert_eq!(by_admin.status, MachineStatus::Revoked);
    // Revoking twice says the same and writes nothing more.
    let again = world
        .app
        .call(MachineRevoke, &[&first])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(again.status, MachineStatus::Revoked);
    let audited: Vec<Option<Id>> = sqlx::query_scalar(
        "SELECT object_id FROM audit_log WHERE action = 'machine.revoked' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audited.len(), 2);
    assert!(audited.contains(&Some(first)) && audited.contains(&Some(second)));
}

#[tokio::test]
async fn a_machine_asked_for_its_facts_again_answers_with_them_and_a_machine_that_is_away_is_not_waited_for()
 {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let machine = machine_with_policy(&world, &ada, &[]).await;
    let away = world
        .app
        .call(MachineProbe, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .problem();
    assert_eq!(away.code, ErrorCode::SourceOffline);
    online(&world, machine).await;
    let facts = json!({
        "hostname": "ada-laptop", "os_version": "Debian 13", "cpus": 8, "memory_bytes": 17_179_869_184_i64,
        "harnesses": [], "adapters": [], "plugins": [], "policy": null,
        "rates": { "up_bps": 1_000_000, "down_bps": 4_000_000, "round_trip_ms": 30 }, "clock_offset_ms": 0
    });
    let machine_says = answer(&world, machine, facts);
    let probed = world
        .app
        .call(MachineProbe, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    machine_says.await.unwrap();
    assert_eq!(probed.facts["hostname"], "ada-laptop");
    let stranger = world
        .app
        .call(MachineProbe, &[&machine])
        .token(&bea.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_policy_of_a_machine_says_which_keys_were_handed_over_and_what_the_reader_may_change() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let machine = machine_with_policy(&world, &ada, &["mcp.add", "provider.upstreams"]).await;
    let policy = world
        .app
        .call(MachinePolicyGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(policy.machine.id, machine);
    assert!(policy.reported_at.is_some());
    let key = |name: &str| policy.keys.iter().find(|key| key.key == name).unwrap();
    assert!(key("mcp.add").delegated && key("mcp.add").writable);
    assert_eq!(key("mcp.add").bounds, PolicyBounds::Environment);
    assert!(key("provider.upstreams").delegated);
    assert_eq!(key("provider.upstreams").bounds, PolicyBounds::Provider);
    assert!(
        !key("policy.delegated").writable,
        "which keys are handed over is changed on the machine and nowhere else"
    );
    assert_eq!(key("environment.browser").value, json!(false));
    assert_eq!(key("environment.browser").bounds, PolicyBounds::Environment);
    assert!(!key("environment.browser").delegated && !key("environment.browser").writable);
    assert_eq!(
        key("environment.terminal").definition.sections,
        [orch_api_contract::resources::MachinePolicySection::Environment]
    );
    assert_eq!(key("environment.terminal").definition.managed, json!(true));
    assert_eq!(
        key("environment.terminal").definition.cautious,
        json!(false)
    );
    assert!(
        policy
            .keys
            .iter()
            .any(|key| key.key == "environment.after_up")
    );
    assert!(policy.keys.iter().any(|key| key.key == "ports"
        && key.definition.sections
            == [orch_api_contract::resources::MachinePolicySection::Environment]));
    assert!(!policy.keys.iter().any(|key| key.key == "hooks"));
    assert!(!policy.keys.iter().any(|key| key.key == "tools.fetch"
        || key.key == "agent.allow_auto_approval"
        || key.key == "environment.ports"));
    let stranger = world
        .app
        .call(MachinePolicyGet, &[&machine])
        .token(&bea.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn machine_readiness_explains_an_open_only_executor_before_any_thread_exists() {
    use orch_agent_contract::descriptor::SandboxMode;
    use orch_api_contract::environment::SandboxLimitSource;
    use orch_db::queries::settings::{Stored, put};
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let machine = machine_with_policy(&world, &ada, &["environment.*"]).await;
    let mut conn = world.conn().await;
    let environment = fixtures::insert_resource(
        &mut conn,
        "environment",
        Some(ada.id()),
        Some(ada.person.personal_org),
        "Acme workspace",
    )
    .await
    .unwrap();
    fixtures::register_on(&mut conn, "environment", environment, machine)
        .await
        .unwrap();
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: None,
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET name='world-box',facts=jsonb_set(facts || $2,'{policy}',COALESCE(facts->'policy','{}') || $3) WHERE id=$1")
        .bind(machine).bind(json!({"environment_descriptor":descriptor}))
        .bind(json!({"agent.max_sandbox":"workspace"})).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE environments SET placement='connected',template_id=NULL,config=$2,facts=$3 WHERE id=$1")
        .bind(environment).bind(json!({"exec.offers":["plain"]}))
        .bind(json!({"executors":[{"kind":"orch-exec","available":true,"sandbox_modes":["open"],"tool_sets":["plain"]}]}))
        .execute(&mut *conn).await.unwrap();
    put(
        &mut conn,
        Some(ada.person.personal_org),
        "sandbox.mode",
        &Stored {
            value: &json!("open"),
            locked: false,
            by: Some(ada.id()),
            at: world.now(),
        },
        None,
    )
    .await
    .unwrap();
    drop(conn);
    let read = world
        .app
        .call(MachineGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    let before = read
        .environment_sandboxes
        .iter()
        .find(|row| row.environment.id == environment)
        .unwrap();
    assert!(!before.sandbox.ready);
    assert!(
        before.policy_manage,
        "the owner can follow the machine-policy act"
    );
    assert_eq!(before.machine.as_ref().unwrap().name, "world-box");
    assert_eq!(before.sandbox.source, SandboxLimitSource::MachinePolicy);
    assert_eq!(before.sandbox.widest, SandboxMode::Workspace);
    assert_eq!(before.sandbox.enforceable, vec![SandboxMode::Open]);
    assert_eq!(before.sandbox.template_limit, None);
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,agent.max_sandbox}','\"open\"') WHERE id=$1")
        .bind(machine).execute(&world.db.pool).await.unwrap();
    let read = world
        .app
        .call(MachineGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(
        read.environment_sandboxes
            .iter()
            .find(|row| row.environment.id == environment)
            .unwrap()
            .sandbox
            .ready
    );
    let threads: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        threads, 0,
        "the machine's page reports readiness before a thread exists"
    );
}

#[tokio::test]
async fn a_shared_machine_manager_can_read_rename_and_change_delegated_keys() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let machine = machine_with_policy(&world, &ada, &["mcp.add"]).await;
    let before = world
        .app
        .call(MachinePolicyGet, &[&machine])
        .token(&bea.token)
        .send()
        .await
        .problem();
    assert_eq!(before.code, ErrorCode::NotFound);
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("machine", machine, "live"),
        "any",
        "machine.manage",
        ("user", Some(bea.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let read = world
        .app
        .call(MachinePolicyGet, &[&machine])
        .token(&bea.token)
        .send()
        .await
        .ok();
    assert!(
        read.keys
            .iter()
            .find(|key| key.key == "mcp.add")
            .unwrap()
            .writable
    );
    let renamed = world
        .app
        .call(MachineUpdate, &[&machine])
        .token(&bea.token)
        .body(&MachineUpdateBody {
            name: "Team workstation".into(),
        })
        .send()
        .await
        .ok();
    assert_eq!(renamed.base.name, "Team workstation");
    assert!(
        renamed
            .access
            .unwrap()
            .permissions
            .iter()
            .any(|p| p == "machine.manage")
    );
    let still_owned = world
        .app
        .call(MachineGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(still_owned.base.name, "Team workstation");
}

#[tokio::test]
async fn an_environment_manager_reads_its_machine_ceiling_without_gaining_machine_management() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Office", "office").await.base.id;
    let machine = machine_with_policy(&world, &ada, &["mcp.add"]).await;
    let mut conn = world.conn().await;
    let environment =
        fixtures::insert_resource(&mut conn, "environment", Some(ada.id()), Some(org), "Work")
            .await
            .unwrap();
    sqlx::query(
        "UPDATE environments SET placement='connected',template_id=NULL,machine_id=$2 WHERE id=$1",
    )
    .bind(environment)
    .bind(machine)
    .execute(&mut *conn)
    .await
    .unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("environment", environment, "live"),
        "any",
        "environment.manage",
        ("user", Some(bea.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let read = world
        .app
        .call(MachinePolicyGet, &[&machine])
        .token(&bea.token)
        .send()
        .await
        .ok();
    assert!(
        read.keys
            .iter()
            .any(|key| key.key == "environment.terminal")
    );
    assert!(read.keys.iter().all(|key| !key.writable));
    assert!(
        !read
            .keys
            .iter()
            .any(|key| key.key == "policy.delegated" || key.key == "provider.upstreams")
    );
    assert_eq!(
        world
            .app
            .call(MachineGet, &[&machine])
            .token(&bea.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let change = std::collections::BTreeMap::from([("mcp.add".into(), json!(true))]);
    assert_eq!(
        world
            .app
            .call(MachinePolicyUpdate, &[&machine])
            .token(&bea.token)
            .body(&change)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn a_key_is_changed_from_here_when_its_owner_handed_it_over_and_the_machine_confirms() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let machine = machine_with_policy(&world, &ada, &["mcp.add"]).await;
    online(&world, machine).await;
    let mut now_in_force = LocalPolicy {
        policy_delegated: vec!["mcp.add".to_owned()],
        ..LocalPolicy::default()
    };
    now_in_force.mcp_add = true;
    let machine_says = answer(
        &world,
        machine,
        json!({
            "applied": ["mcp.add"], "refused": [],
            "policy": serde_json::to_value(&now_in_force).unwrap()
        }),
    );
    let mut body = std::collections::BTreeMap::new();
    body.insert("mcp.add".to_owned(), json!(true));
    let changed = world
        .app
        .call(MachinePolicyUpdate, &[&machine])
        .token(&ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    machine_says.await.unwrap();
    let key = changed
        .keys
        .iter()
        .find(|key| key.key == "mcp.add")
        .unwrap();
    assert_eq!(key.value, json!(true));
    // The policy in force is what the machine reported back, and the change is audited with the
    // values before and after.
    let audited: Vec<Json> =
        sqlx::query_scalar("SELECT details FROM audit_log WHERE action = 'machine.policy_changed'")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0]["keys"], json!(["mcp.add"]));
    assert_eq!(audited[0]["before"]["mcp.add"], json!(false));
    assert_eq!(audited[0]["after"]["mcp.add"], json!(true));
}

#[tokio::test]
async fn invalid_driver_connection_fields_are_refused_before_the_machine_is_told() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let machine = machine_with_policy(&world, &ada, &["drivers"]).await;
    online(&world, machine).await;
    let descriptors = json!({"cluster": {"configuration_schema": {
        "type": "object", "required": ["namespace", "storage_class"],
        "properties": {"namespace": {"type": "string"},
            "storage_class": {"type": "string", "minLength": 1}},
        "additionalProperties": false
    }}});
    let mut conn = world.conn().await;
    let host = orch_types::new_id();
    let home: Id = sqlx::query_scalar("SELECT home_id FROM machines WHERE id = $1")
        .bind(machine)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    orch_db::queries::resources::insert_host(
        &mut conn,
        &orch_db::queries::resources::NewHost {
            id: host,
            machine_id: machine,
            owner_id: Some(ada.id()),
            home_id: Some(home),
            name: "cluster",
        },
        world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET drivers = $2 WHERE id = $1")
        .bind(host)
        .bind(descriptors)
        .execute(&mut *conn)
        .await
        .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ResourceKind::Host.into(),
            id: host,
        },
        ada.id(),
        orch_authz::ownership::NewAccess::Defaults,
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE machines SET roles = ARRAY['node'] WHERE id = $1")
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let reported = world
        .app
        .call(MachinePolicyGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(
        reported
            .keys
            .iter()
            .any(|key| key.key == "drivers" && key.writable)
    );
    let body = std::collections::BTreeMap::from([(
        "drivers".to_owned(),
        json!({
            "allowed": ["cluster"], "configurations": {"cluster": {"namespace": 7, "storage_class": ""}}
        }),
    )]);
    assert_eq!(
        world
            .app
            .call(MachinePolicyUpdate, &[&machine])
            .token(&bea.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let problem = world
        .app
        .call(MachinePolicyUpdate, &[&machine])
        .token(&ada.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
    let faults = problem.extra["errors"].as_array().unwrap();
    assert!(faults.iter().any(|fault| {
        fault["path"]
            .as_str()
            .unwrap_or_default()
            .ends_with("/namespace")
    }));
    assert!(faults.iter().any(|fault| {
        fault["path"]
            .as_str()
            .unwrap_or_default()
            .ends_with("/storage_class")
    }));
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE machine_id=$1")
        .bind(machine)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(commands, 0);

    // An unavailable driver is absent from the schedulable list but still reports its setup schema.
    let schemas = json!({"cluster": {"type": "object", "required": ["namespace", "storage_class"],
        "properties": {"namespace": {"type": "string"}, "storage_class": {"type": "string"}}}});
    sqlx::query("UPDATE hosts SET drivers = '{}'::jsonb, driver_configuration_schemas = $2 WHERE machine_id = $1")
        .bind(machine)
        .bind(schemas)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let missing = std::collections::BTreeMap::from([(
        "drivers".to_owned(),
        json!({"allowed": ["cluster"], "configurations": {"cluster": {"storage_class": "local"}}}),
    )]);
    let problem = world
        .app
        .call(MachinePolicyUpdate, &[&machine])
        .token(&ada.token)
        .body(&missing)
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
    assert!(
        problem.extra["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fault| fault["path"] == "drivers.configurations.cluster/namespace")
    );
    let malformed = std::collections::BTreeMap::from([(
        "drivers".to_owned(),
        json!({"allowed":"synthetic-private-value"}),
    )]);
    let problem = world
        .app
        .call(MachinePolicyUpdate, &[&machine])
        .token(&ada.token)
        .body(&malformed)
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
    assert!(
        !serde_json::to_string(&problem)
            .unwrap()
            .contains("synthetic-private-value")
    );
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE machine_id=$1")
        .bind(machine)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(commands, 0);
}

#[tokio::test]
async fn a_key_that_was_not_handed_over_is_refused_whole_with_the_keys_listed() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let machine = machine_with_policy(&world, &ada, &["mcp.add"]).await;
    online(&world, machine).await;
    let mut body = std::collections::BTreeMap::new();
    body.insert("mcp.add".to_owned(), json!(true));
    body.insert("tools.fetch".to_owned(), json!(true));
    body.insert("policy.delegated".to_owned(), json!(["*"]));
    let refused = world
        .app
        .call(MachinePolicyUpdate, &[&machine])
        .token(&ada.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::PolicyNotDelegated);
    assert_eq!(
        refused.extra["keys"],
        json!(["policy.delegated", "tools.fetch"])
    );
    let sent: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        sent, 0,
        "nothing was sent, not even the key that could have been"
    );
    // A key that is none, and a value that does not fit, are told apart from a refusal.
    let mut unknown = std::collections::BTreeMap::new();
    unknown.insert("no.such.key".to_owned(), json!(true));
    let invalid = world
        .app
        .call(MachinePolicyUpdate, &[&machine])
        .token(&ada.token)
        .body(&unknown)
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
    let mut wrong = std::collections::BTreeMap::new();
    wrong.insert("mcp.add".to_owned(), json!("yes"));
    let invalid = world
        .app
        .call(MachinePolicyUpdate, &[&machine])
        .token(&ada.token)
        .body(&wrong)
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_key_the_machine_no_longer_has_handed_over_is_not_taken_and_a_machine_away_is_not_waited_for()
 {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let machine = machine_with_policy(&world, &ada, &["mcp.add"]).await;
    let mut body = std::collections::BTreeMap::new();
    body.insert("mcp.add".to_owned(), json!(true));
    let away = world
        .app
        .call(MachinePolicyUpdate, &[&machine])
        .token(&ada.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(away.code, ErrorCode::SourceOffline);
    online(&world, machine).await;
    let unchanged = LocalPolicy::default();
    let machine_says = answer(
        &world,
        machine,
        json!({
            "applied": [], "refused": ["mcp.add"],
            "policy": serde_json::to_value(&unchanged).unwrap()
        }),
    );
    let refused = world
        .app
        .call(MachinePolicyUpdate, &[&machine])
        .token(&ada.token)
        .body(&body)
        .send()
        .await
        .problem();
    machine_says.await.unwrap();
    assert_eq!(refused.code, ErrorCode::PolicyNotDelegated);
    assert_eq!(refused.extra["keys"], json!(["mcp.add"]));
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'machine.policy_changed'",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audited, 0, "nothing changed, nothing is audited");
}

#[tokio::test]
async fn enrollment_information_is_read_only_and_a_nonloopback_http_system_never_mints_a_useless_command()
 {
    use orch_api_contract::resources::MachineEnrollmentInfoGet;
    let mut config = orch_config::ServerConfig::default();
    config.system.public_url = "http://orch.test:3000".into();
    let world = World::with(config).await;
    let ada = person(&world, "ada").await;
    let info = world
        .app
        .call(MachineEnrollmentInfoGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(!info.secure);
    assert_eq!(info.public_url, "http://orch.test:3000");
    let refused = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&ada.token)
        .body(&MachineEnrollmentTokenCreateBody::default())
        .send()
        .await
        .problem();
    // The detail is the code's sentence; the fault names the system's address and why.
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"][0]["path"], "public_url");
    assert!(
        refused.extra["errors"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("HTTPS")),
        "{refused:?}"
    );
    let machine = machine_with_policy(&world, &ada, &[]).await;
    // This test uses a canonical owned resource, as actual enrollment creates it.
    let mut conn = world.conn().await;
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ResourceKind::Machine.into(),
            id: machine,
        },
        ada.id(),
        orch_authz::ownership::NewAccess::Defaults,
        orch_types::Time::new(world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    let again = world
        .app
        .call(
            orch_api_contract::resources::MachineEnrollmentAgainTokenCreate,
            &[&machine],
        )
        .token(&ada.token)
        .send()
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::RequestInvalid);
    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        tokens, 0,
        "opening enrollment and rejecting initial/replacement invalid commands mint no secrets"
    );
}

#[tokio::test]
async fn programs_reads_are_stored_and_the_native_acknowledgement_updates_the_registry_and_adapters_before_returning()
 {
    use orch_api_contract::machine_programs::{
        MachineProgramKind, MachineProgramRegistration, MachineProgramsCommand,
    };
    use orch_api_contract::resources::{MachineProgramsChange, MachineProgramsGet};
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let machine = machine_with_policy(&world, &ada, &["*"]).await;
    online(&world, machine).await;
    let empty = world
        .app
        .call(MachineProgramsGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(empty.entries.is_empty());
    assert!(empty.can_manage);
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(commands, 0);
    assert_eq!(
        world
            .app
            .call(MachineProgramsGet, &[&machine])
            .token(&bea.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let request = MachineProgramsCommand::Register(MachineProgramRegistration {
        kind: MachineProgramKind::Agent,
        name: "my-agent".into(),
        command: vec!["/opt/my-agent".into()],
    });
    assert_eq!(
        world
            .app
            .call(MachineProgramsChange, &[&machine])
            .token(&bea.token)
            .body(&request)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let declaration = json!({"kind":"my-agent","adapter_version":1,"harness":{"name":"My agent","version":"1.0.0","supported_range":">=1.0.0","status":"supported"},"arrangements":[{"type":"together"}],"approval_modes":["on_request"],"sandbox_modes":["open"]});
    let facts = json!({"hostname":"laptop","os_version":"Linux","cpus":4,"memory_bytes":8192,
        "harnesses":[],"adapters":[{"kind":"my-agent","adapter_version":1,"builtin":false,"needs_binary":false}],
        "plugins":[{"kind":"adapter","name":"my-agent","version":"1.0.0","state":"running","descriptor":null}],
        "policy":serde_json::to_value(LocalPolicy::default()).unwrap(),"rates":{"up_bps":0,"down_bps":0,"round_trip_ms":1},"clock_offset_ms":0,
        "programs":{"entries":[{"kind":"agent","name":"my-agent","command":["/opt/my-agent"],"origin":"orch","ready":true,"version":"1.0.0","descriptor":declaration,"models":null,"detail":null}],
            "capabilities":[{"kind":"agent","supported":true,"remote":true,"policy_key":"agent.programs","reason":null}],"config_file":"/data/config.toml","can_manage":false}});
    let native = answer(&world, machine, facts);
    let registered = world
        .app
        .call(MachineProgramsChange, &[&machine])
        .token(&ada.token)
        .body(&request)
        .send()
        .await
        .ok();
    native.await.unwrap();
    assert!(
        registered.can_manage,
        "the server projects the current caller, not a native authority flag"
    );
    assert_eq!(
        registered.entries[0].descriptor["harness"]["name"],
        "My agent"
    );
    let shown = world
        .app
        .call(MachineGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(shown.facts["adapters"][0]["kind"], "my-agent");
    assert_eq!(shown.facts["adapters"][0]["needs_binary"], false);
    assert_eq!(
        registered.machine_view.as_ref().unwrap().facts["adapters"][0]["kind"],
        "my-agent"
    );
    let saved = world
        .app
        .call(MachineProgramsGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(saved.entries, registered.entries);
    assert_eq!(saved.can_manage, registered.can_manage);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='machine.programs_changed'",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        commands, 1,
        "GETs and acknowledgements never ask the connector a second time"
    );
    let profile = world
        .app
        .call(
            orch_api_contract::resources::MachineAgentCreate,
            &[&machine],
        )
        .token(&ada.token)
        .body(&orch_api_contract::resources::MachineAgentCreateBody {
            home_id: ada.person.personal_org,
            harness_kind: "my-agent".into(),
            name: "Research profile".into(),
            profile: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(profile.base.harness_kind, "my-agent");
    assert_eq!(profile.base.machine.as_ref().unwrap().id, machine);

    let mut source = orch_provider_contract::SourceDescriptor::new(
        "gpu-source",
        "GPU service",
        "Models and limits from a local service.",
    );
    source.facts = Some(Default::default());
    source.models = Some(Default::default());
    let mut source_facts = shown.facts.clone();
    source_facts["plugins"] = json!([{"kind":"provider","name":"gpu-source","version":"1","state":"running","descriptor":source}]);
    source_facts["programs"] = json!({"entries":[{"kind":"provider_source","name":"gpu-source","command":["/opt/source"],"origin":"orch","ready":true,"version":"1","descriptor":source,"models":null,"detail":null}],"capabilities":[],"config_file":"/data/config.toml","can_manage":false});
    let source_request = MachineProgramsCommand::Register(MachineProgramRegistration {
        kind: MachineProgramKind::ProviderSource,
        name: "gpu-source".into(),
        command: vec!["/opt/source".into()],
    });
    let native = answer(&world, machine, source_facts);
    world
        .app
        .call(MachineProgramsChange, &[&machine])
        .token(&ada.token)
        .body(&source_request)
        .send()
        .await
        .ok();
    native.await.unwrap();
    let setup = world
        .app
        .call(orch_api_contract::resources::ProviderSetupGet, &[])
        .token(&ada.token)
        .query(&orch_api_contract::resources::ProviderSetupGetQuery {
            org_id: ada.person.personal_org,
            provider_id: None,
        })
        .send()
        .await
        .ok();
    let offer = setup
        .sources
        .iter()
        .find(|offer| {
            offer.name == "gpu-source"
                && offer
                    .machine
                    .as_ref()
                    .is_some_and(|item| item.id == machine)
        })
        .unwrap();
    assert_eq!(offer.descriptor.as_ref().unwrap().title, "GPU service");
    assert!(offer.descriptor.as_ref().unwrap().facts.is_some());
    assert!(offer.descriptor.as_ref().unwrap().models.is_some());
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        commands, 2,
        "profile creation and provider choices use the acknowledged declarations without another native read"
    );
}

#[tokio::test]
async fn delegated_roles_use_machine_management_and_acknowledge_without_inventing_active_roles() {
    use orch_api_contract::resources::{MachineRolesSet, MachineRolesSetBody};
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let machine = machine_with_policy(&world, &ada, &["machine.roles"]).await;
    online(&world, machine).await;
    let policy = LocalPolicy::resolve(
        &["machine.roles".into()],
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    sqlx::query("UPDATE machines SET facts=facts || $2 WHERE id=$1").bind(machine)
        .bind(json!({"policy":policy,"role_management":{"supported":true,"delegated":false,"enabled":false}})).execute(&world.db.pool).await.unwrap();
    let request = MachineRolesSetBody {
        roles: vec!["agent".into(), "provider".into()],
    };
    assert_eq!(
        world
            .app
            .call(MachineRolesSet, &[&machine])
            .token(&bea.token)
            .body(&request)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let mut grant = world.conn().await;
    orch_db::queries::access::testing::add_entry(
        &mut grant,
        &orch_db::queries::access::PageKey::new("machine", machine, "live"),
        "any",
        "machine.manage",
        ("user", Some(bea.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(grant);
    let native = answer(
        &world,
        machine,
        json!({"roles":request.roles,"reconnecting":true}),
    );
    let acknowledged = world
        .app
        .call(MachineRolesSet, &[&machine])
        .token(&bea.token)
        .body(&request)
        .send()
        .await
        .ok();
    native.await.unwrap();
    assert!(acknowledged.reconnecting);
    assert_eq!(acknowledged.requested_roles, request.roles);
    assert_eq!(
        acknowledged.machine_view.roles,
        ["environment", "agent", "provider"],
        "only a new native hello can change the actual roles"
    );
    assert!(
        acknowledged
            .machine_view
            .role_management
            .as_ref()
            .unwrap()
            .enabled,
        "the latest effective policy wins over a stale capability report"
    );
    assert!(
        acknowledged
            .machine_view
            .facts
            .get("role_management")
            .is_none(),
        "the API serves role controls once as typed facts"
    );
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(commands, 1);
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='machine.roles_changed'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn kept_roles_node_roles_and_removing_a_used_role_refuse_before_any_native_command() {
    use orch_api_contract::resources::{MachineRolesSet, MachineRolesSetBody};
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let machine = machine_with_policy(&world, &ada, &[]).await;
    online(&world, machine).await;
    let request = MachineRolesSetBody {
        roles: vec!["agent".into()],
    };
    let refusal = world
        .app
        .call(MachineRolesSet, &[&machine])
        .token(&ada.token)
        .body(&request)
        .send()
        .await
        .problem();
    assert_eq!(refusal.code, ErrorCode::RequestInvalid);
    // The refusal names the member; an explanation that names a policy key is the catalogue's sentence.
    assert_eq!(refusal.extra["errors"][0]["path"], "roles", "{refusal:?}");
    let local = std::collections::BTreeMap::from([("machine.roles".into(), json!(false))]);
    let policy = LocalPolicy::resolve(&["*".into()], &local, &Default::default()).unwrap();
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',$2) WHERE id=$1")
        .bind(machine)
        .bind(json!(policy))
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        world
            .app
            .call(MachineRolesSet, &[&machine])
            .token(&ada.token)
            .body(&request)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let policy = LocalPolicy::resolve(
        &["machine.roles".into()],
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',$2) WHERE id=$1")
        .bind(machine)
        .bind(json!(policy))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut conn = world.conn().await;
    let environment = fixtures::insert_resource(
        &mut conn,
        "environment",
        Some(ada.id()),
        Some(ada.person.personal_org),
        "Work",
    )
    .await
    .unwrap();
    fixtures::register_on(&mut conn, "environment", environment, machine)
        .await
        .unwrap();
    drop(conn);
    let in_use = world
        .app
        .call(MachineRolesSet, &[&machine])
        .token(&ada.token)
        .body(&request)
        .send()
        .await
        .problem();
    assert!(
        serde_json::to_string(&in_use)
            .unwrap()
            .contains("registered environments")
    );
    let node = MachineRolesSetBody {
        roles: vec!["node".into()],
    };
    assert!(
        serde_json::to_string(
            &world
                .app
                .call(MachineRolesSet, &[&machine])
                .token(&ada.token)
                .body(&node)
                .send()
                .await
                .problem()
        )
        .unwrap()
        .contains("orch-node")
    );
    let commands: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(commands, 0);
}

#[tokio::test]
async fn every_policy_row_uses_the_reported_physical_program_and_its_named_data_directory() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let machine = machine_with_policy(&world, &ada, &[]).await;
    let command = "'/opt/orch-connector' --data-dir '/data/another instance' policy";
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{installation}',jsonb_build_object('commands',jsonb_build_object('policy',$2::text))) WHERE id=$1")
        .bind(machine).bind(command).execute(&world.db.pool).await.unwrap();
    let policy = world
        .app
        .call(MachinePolicyGet, &[&machine])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(policy.command.as_deref(), Some(command));
    assert!(policy.keys.iter().any(|entry| {
        entry.key == "environment.roots"
            && entry
                .definition
                .sections
                .contains(&orch_api_contract::resources::MachinePolicySection::Node)
    }));
}

#[tokio::test]
async fn a_loopback_http_system_only_mints_a_token_when_its_local_command_is_requested() {
    use orch_api_contract::resources::MachineEnrollmentInfoGet;
    let mut config = orch_config::ServerConfig::default();
    config.system.public_url = "http://127.0.0.1:3000".into();
    config.gateway.endpoints[0].url = "wss://gateway.test:9443".into();
    let world = World::with(config).await;
    let ada = person(&world, "ada").await;
    let info = world
        .app
        .call(MachineEnrollmentInfoGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(info.secure);
    assert_eq!(info.public_url, "http://127.0.0.1:3000");
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(before, 0);
    let command = world
        .app
        .call(MachineEnrollmentTokenCreate, &[])
        .token(&ada.token)
        .body(&MachineEnrollmentTokenCreateBody::default())
        .send()
        .await
        .ok();
    assert!(
        command
            .commands
            .linux
            .contains("--url 'http://127.0.0.1:3000'")
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, 1);
}
