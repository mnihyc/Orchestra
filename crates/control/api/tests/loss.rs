//! Permanent-loss endpoints require management, a fresh plan and the exact typed name.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::{common::ErrorCode, loss::*};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures;
use orch_types::Time;
use support::World;

#[tokio::test]
async fn only_managers_can_plan_and_confirm_a_host_loss() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let mut conn = world.conn().await;
    let host = access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(c.boss.id()),
        Some(c.acme),
        "Lost host",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        c.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    let body = LossPreflightBody {
        target: LossTarget::Host { id: host },
    };
    assert_eq!(
        world
            .app
            .call(LossPreflight, &[])
            .token(&c.stranger.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let plan = world
        .app
        .call(LossPreflight, &[])
        .token(&c.boss.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(plan.name, "Lost host");
    let wrong = LossDeclareBody {
        notify: true,
        preflight_id: plan.preflight_id,
        confirm: "wrong".into(),
    };
    assert_eq!(
        world
            .app
            .call(LossDeclare, &[])
            .token(&c.boss.token)
            .body(&wrong)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let correct = LossDeclareBody {
        notify: true,
        preflight_id: plan.preflight_id,
        confirm: plan.name,
    };
    assert_eq!(
        world
            .app
            .call(LossDeclare, &[])
            .token(&c.stranger.token)
            .body(&correct)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    world
        .app
        .call(LossDeclare, &[])
        .token(&c.boss.token)
        .body(&correct)
        .send()
        .await
        .ok();
    let host = world
        .app
        .call(orch_api_contract::resources::HostGet, &[&host])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert!(host.lost_at.is_some());
    assert!(!host.schedulable);
    assert_eq!(
        world
            .app
            .call(
                orch_api_contract::resources::MachineEnrollmentAgainTokenCreate,
                &[&host.machine.id]
            )
            .token(&c.boss.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::PlanBlocked
    );
    world
        .app
        .call(orch_api_contract::resources::HostDelete, &[&host.base.id])
        .token(&c.boss.token)
        .query(&orch_api_contract::resources::HostDeleteQuery {
            confirm: "Lost host".into(),
        })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn a_persons_lost_machine_cannot_renew_or_use_a_previously_issued_reenrollment_token() {
    use orch_api_core::certificates::{RenewCaller, TokenFor, TokenRequest};
    use orch_db::audit::Origin;
    use orch_link_contract::{
        enrollment::{EnrollRequest, RenewRequest},
        session::{Arch, Os, Platform, Role},
    };
    use orch_pki::{certificates::Certificate, enrollment::PendingEnrollment};
    let world = World::new().await;
    let c = resources::company(&world).await;
    let service = world.app.app.link.certificates();
    let issued = service
        .issue_token(&TokenRequest {
            created_by: c.boss.id(),
            purpose: TokenFor::Machine,
            name_hint: None,
        })
        .await
        .unwrap();
    let request = |token: String| EnrollRequest {
        token,
        machine_id: None,
        csr_pem: PendingEnrollment::start().unwrap().csr_pem().into(),
        name: "Lost laptop".into(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".into(),
        protocol_versions: vec![1],
        roles: vec![Role::Agent, Role::Environment],
    };
    let enrolled = service
        .enroll(&request(issued.token), &Origin::default())
        .await
        .unwrap();
    let replacement = service
        .issue_token(&TokenRequest {
            created_by: c.boss.id(),
            purpose: TokenFor::Again {
                machine: enrolled.machine_id,
            },
            name_hint: None,
        })
        .await
        .unwrap();
    let body = LossPreflightBody {
        target: LossTarget::Machine {
            id: enrolled.machine_id,
        },
    };
    assert_eq!(
        world
            .app
            .call(LossPreflight, &[])
            .token(&c.viewer.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let plan = world
        .app
        .call(LossPreflight, &[])
        .token(&c.boss.token)
        .body(&body)
        .send()
        .await
        .ok();
    world
        .app
        .call(LossDeclare, &[])
        .token(&c.boss.token)
        .body(&LossDeclareBody {
            notify: true,
            preflight_id: plan.preflight_id,
            confirm: plan.name,
        })
        .send()
        .await
        .ok();
    let certificate = Certificate::from_pem(&enrolled.certificate_pem).unwrap();
    let refused = service
        .renew_machine(
            &RenewCaller::Certificate(certificate),
            &RenewRequest {
                csr_pem: PendingEnrollment::start().unwrap().csr_pem().into(),
            },
            &Origin::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.status, 403, "the old identity remains revoked");
    let refused = service
        .enroll(&request(replacement.token), &Origin::default())
        .await
        .unwrap_err();
    assert_eq!(
        refused.status, 410,
        "an older replacement token cannot resurrect a lost identity"
    );
}

async fn mixed_host(
    world: &World,
    c: &resources::Company,
) -> (orch_types::Id, orch_types::Id, orch_types::Id) {
    let mut conn = world.conn().await;
    let host = access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(c.boss.id()),
        Some(c.acme),
        "Mixed host",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        c.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let mut ids = Vec::new();
    for (owner, name) in [
        (c.viewer.id(), "Own environment"),
        (c.stranger.id(), "Private environment"),
    ] {
        let id =
            access_fixtures::insert_environment(&mut conn, owner, Some(c.acme), name, "managed")
                .await
                .unwrap();
        let template: orch_types::Id =
            sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
                .bind(id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        orch_db::queries::environments::hosting::insert_runtime(
            &mut conn,
            &orch_db::queries::environments::hosting::NewRuntime {
                id: orch_types::new_id(),
                host_id: host,
                unit_name: name,
                environment_id: id,
                driver: "test",
                template_id: template,
                spec_hash: &[0; 32],
                generation: 1,
                volumes: &serde_json::json!({}),
            },
            world.now(),
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE environments SET machine_id=NULL,generation=1,status='offline' WHERE id=$1",
        )
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id,
            },
            owner,
            NewAccess::OnlyMe,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        ids.push(id);
    }
    drop(conn);
    resources::name_person(world, "host", host, "host.manage", c.viewer.id()).await;
    (host, ids[0], ids[1])
}

#[tokio::test]
async fn a_host_manager_recovers_only_their_managed_rows_and_owners_are_notified_only_by_explicit_choice()
 {
    use orch_api_contract::forks::ForkCreateBody;
    for notify in [false, true] {
        let world = World::new().await;
        let c = resources::company(&world).await;
        let (host, own, other) = mixed_host(&world, &c).await;
        let plan = world
            .app
            .call(LossPreflight, &[])
            .token(&c.viewer.token)
            .body(&LossPreflightBody {
                target: LossTarget::Host { id: host },
            })
            .send()
            .await
            .ok();
        world
            .app
            .call(LossDeclare, &[])
            .token(&c.viewer.token)
            .body(&LossDeclareBody {
                notify,
                preflight_id: plan.preflight_id,
                confirm: plan.name,
            })
            .send()
            .await
            .ok();
        let notices:i64=sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=$1 AND data->>'operation_kind'='lost'").bind(c.stranger.id()).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(
            notices,
            i64::from(notify),
            "the owner of the other environment receives only the operation's explicit notification"
        );
        let sources = world
            .app
            .call(LossRecoveryList, &[])
            .token(&c.viewer.token)
            .body(&LossPreflightBody {
                target: LossTarget::Host { id: host },
            })
            .send()
            .await
            .ok();
        assert_eq!(sources.others, 1);
        assert_eq!(sources.environments.len(), 1);
        assert_eq!(sources.environments[0].id, own);
        let recovery = world
            .app
            .call(EnvironmentRecoverPreflight, &[])
            .token(&c.viewer.token)
            .body(&EnvironmentRecoverPreflightBody {
                environment_ids: vec![own, other],
                host_id: None,
                choices: vec![],
                environment: None,
            })
            .send()
            .await
            .ok();
        assert_eq!(recovery.others, 1);
        assert_eq!(recovery.rows.len(), 1);
        assert_eq!(recovery.rows[0].environment.id, own);
        assert_eq!(
            recovery.executable, 0,
            "an environment with no available destination stays blocked"
        );
        assert_eq!(
            world
                .app
                .call(EnvironmentRecoverCreate, &[])
                .token(&c.viewer.token)
                .body(&ForkCreateBody {
                    notify: Some(false),
                    preflight_id: recovery.preflight_id,
                    options: None
                })
                .send()
                .await
                .problem()
                .code,
            ErrorCode::PlanBlocked
        );
        let state: String = sqlx::query_scalar("SELECT status FROM environments WHERE id=$1")
            .bind(other)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(state, "lost");
        let jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_provision'",
        )
        .bind(other)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(jobs, 0);
    }
}

#[tokio::test]
async fn declaring_loss_as_administrator_names_the_actor_on_the_page_and_marks_the_act() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let (host, _, _) = mixed_host(&world, &c).await;
    let admin = places::admin(&world, "responder").await;
    let plan = world
        .app
        .call(LossPreflight, &[])
        .token(&admin.token)
        .body(&LossPreflightBody {
            target: LossTarget::Host { id: host },
        })
        .send()
        .await
        .ok();
    assert_eq!(plan.admin_grants.len(), 1);
    assert_eq!(
        plan.admin_grants[0].permission,
        orch_access_contract::permissions::Permission::HostManage
    );
    let before:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='host' AND object_id=$1 AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL").bind(host).bind(admin.id()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(before, 0, "a preview makes no page change");
    world
        .app
        .call(LossDeclare, &[])
        .token(&admin.token)
        .body(&LossDeclareBody {
            notify: false,
            preflight_id: plan.preflight_id,
            confirm: plan.name,
        })
        .send()
        .await
        .ok();
    let after:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='host' AND object_id=$1 AND permission='host.manage' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL").bind(host).bind(admin.id()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(after, 1);
    let marked: bool = sqlx::query_scalar(
        "SELECT \"override\" FROM audit_log WHERE action='host.lost' AND object_id=$1",
    )
    .bind(host)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert!(marked);
}

#[tokio::test]
async fn a_host_deletion_plan_lists_cascades_and_never_removes_others_live_environments() {
    let world = World::new().await;
    let c = resources::company(&world).await;
    let (host, own, other) = mixed_host(&world, &c).await;
    let mut conn = world.conn().await;
    let machine: orch_types::Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=true WHERE id=$1")
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let plan = world
        .app
        .call(HostDeletePreflight, &[&host])
        .token(&c.viewer.token)
        .body(&HostDeletePreflightBody {
            environments: HostEnvironmentCascade::Remove,
            erase: Some(false),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(plan.rows.len(), 4);
    assert_eq!(plan.others, 1);
    assert!(
        plan.executable,
        "others do not prevent the caller from starting their reviewed work: {:?}",
        plan.rows
    );
    assert!(
        !plan
            .rows
            .iter()
            .find(|row| row.kind == "environments")
            .unwrap()
            .blockers
            .iter()
            .any(|row| row.code == "environments_of_others")
    );
    let accepted = world
        .app
        .call(HostDeleteCreate, &[&host])
        .token(&c.viewer.token)
        .body(&HostDeleteCreateBody {
            preflight_id: plan.preflight_id,
            confirm: plan.name,
            notify: false,
            volumes: vec![],
            data_confirmations: vec![],
        })
        .send()
        .await
        .ok();
    assert_eq!(accepted.kind, "host_delete");
    let paused: bool = sqlx::query_scalar("SELECT NOT schedulable FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(
        paused,
        "no new scheduled work enters the host while managers finish its remaining environments"
    );
    let own_started:bool=sqlx::query_scalar("SELECT remove_at IS NOT NULL OR removal_phase IS NOT NULL OR deleted_at IS NOT NULL FROM environments WHERE id=$1").bind(own).fetch_one(&world.db.pool).await.unwrap();
    assert!(
        own_started,
        "the caller's removal is admitted before waiting for somebody else's work"
    );
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM environments WHERE id=$1")
            .bind(other)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(!deleted);
    assert_eq!(
        world
            .app
            .call(HostDeletePreflight, &[&host])
            .token(&c.stranger.token)
            .body(&HostDeletePreflightBody::default())
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn host_data_stays_by_default_and_exact_consent_is_required_before_its_removal_is_admitted() {
    use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind, ResourceRef};
    use std::collections::BTreeMap;
    let world = World::new().await;
    let c = resources::company(&world).await;
    let (host, _, _) = mixed_host(&world, &c).await;
    let mut conn = world.conn().await;
    let machine: orch_types::Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=true WHERE id=$1")
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    let inventory = ResourceInventory {
        observed_at: Time::new(world.now()),
        resources: vec![Resource {
            resource: ResourceRef {
                kind: ResourceKind::Volume,
                id: "unclaimed-data".into(),
            },
            name: "Unclaimed data".into(),
            size_bytes: Some(8192),
            labels: BTreeMap::new(),
            depends_on: vec![],
            aliases: vec![],
            last_used_at: None,
            busy: false,
        }],
        unavailable: vec![],
    };
    sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1")
        .bind(host)
        .bind(serde_json::to_value(&inventory).unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let mut request = HostDeletePreflightBody {
        environments: HostEnvironmentCascade::Remove,
        erase: Some(false),
        ..Default::default()
    };
    let plan = world
        .app
        .call(HostDeletePreflight, &[&host])
        .token(&c.viewer.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert!(!plan.erase);
    assert!(plan.executable, "{:?}", plan.rows);
    let data = plan
        .data
        .iter()
        .find(|row| row.id == "native:unclaimed-data")
        .unwrap();
    assert_eq!(data.selected, HostDataFate::Leave);
    assert_eq!(data.bytes, Some(8192));
    assert_eq!(data.label.as_deref(), Some("Unclaimed data"));
    request.data.push(HostDataChoice {
        id: data.id.clone(),
        fate: HostDataFate::Delete,
        host_id: None,
    });
    let counting = world
        .app
        .call(HostDeletePreflight, &[&host])
        .token(&c.viewer.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert!(!counting.executable);
    assert!(
        counting
            .erase_blockers
            .iter()
            .any(|row| row.code == "measurement_pending")
    );
    let count = counting.inventory_measurement.unwrap();
    assert_eq!(
        world
            .app
            .call(orch_api_contract::workspace::JobGet, &[&count.job_id])
            .token(&c.stranger.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied,
        "inventory details keep Host Manage authority"
    );
    sqlx::query("UPDATE jobs SET state='succeeded',result=$2,finished_at=$3 WHERE id=$1")
        .bind(count.job_id)
        .bind(serde_json::to_value(&inventory).unwrap())
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let plan = world
        .app
        .call(HostDeletePreflight, &[&host])
        .token(&c.viewer.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert!(plan.executable);
    assert_eq!(plan.data[0].selected, HostDataFate::Delete);
    let wrong = HostDeleteCreateBody {
        preflight_id: plan.preflight_id,
        confirm: plan.name.clone(),
        notify: false,
        volumes: vec![],
        data_confirmations: vec![HostDataConfirmation {
            id: data.id.clone(),
            confirm: "unclaimed data".into(),
        }],
    };
    assert_eq!(
        world
            .app
            .call(HostDeleteCreate, &[&host])
            .token(&c.viewer.token)
            .body(&wrong)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let admitted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='host_delete' AND plan#>>'{cascade,host}'=$1",
    )
    .bind(host.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        admitted, 0,
        "wrong typed consent cannot admit cascades or erasure"
    );
    let correct = HostDeleteCreateBody {
        data_confirmations: vec![HostDataConfirmation {
            id: data.id.clone(),
            confirm: "Unclaimed data".into(),
        }],
        ..wrong
    };
    let accepted = world
        .app
        .call(HostDeleteCreate, &[&host])
        .token(&c.viewer.token)
        .body(&correct)
        .send()
        .await
        .ok();
    assert_eq!(accepted.kind, "host_delete");
    let persisted: serde_json::Value = sqlx::query_scalar("SELECT plan FROM jobs WHERE id=$1")
        .bind(accepted.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(persisted["cascade"]["custody"]["erase"], false);
    assert_eq!(
        persisted["cascade"]["custody"]["native_data"][0]["row"]["selected"],
        "delete"
    );
    let session: orch_types::Id = persisted["identity"]["session_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let user: orch_types::Id = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        user,
        c.viewer.id(),
        "every newly accepted destructive parent retains its actual browser requester"
    );
}

#[tokio::test]
async fn notification_previews_are_caller_bound_and_filter_private_names_and_personal_kind_choices()
{
    let world = World::new().await;
    let c = resources::company(&world).await;
    let (host, _, _) = mixed_host(&world, &c).await;
    sqlx::query("INSERT INTO user_profiles(user_id,audience) VALUES($1,'private')")
        .bind(c.stranger.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO notification_prefs(user_id,disabled_kinds) VALUES($1,'[\"environment.attention\"]')").bind(c.boss.id()).execute(&world.db.pool).await.unwrap();
    let plan = world
        .app
        .call(LossPreflight, &[])
        .token(&c.viewer.token)
        .body(&LossPreflightBody {
            target: LossTarget::Host { id: host },
        })
        .send()
        .await
        .ok();
    let people = world
        .app
        .call(OperationRecipientsGet, &[&plan.preflight_id])
        .token(&c.viewer.token)
        .query(&orch_api_contract::common::PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(people.total, plan.notification.people);
    assert_eq!(people.hidden, 1);
    assert!(
        !people
            .people
            .items
            .iter()
            .any(|person| person.id == c.stranger.id() || person.id == c.boss.id())
    );
    assert_eq!(
        world
            .app
            .call(OperationRecipientsGet, &[&plan.preflight_id])
            .token(&c.stranger.token)
            .query(&orch_api_contract::common::PageQuery::default())
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}
