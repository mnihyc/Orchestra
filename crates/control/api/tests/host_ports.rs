//! A host's declaration of host ports: saved only once its node accepted it under its own ceiling, never while
//! the node is away, and a host opening reviewed from the saved declaration.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::ports::{HostPorts, PortRange};
use orch_api_contract::resources::{HostGet, HostUpdate, HostUpdateBody};
use orch_db::queries::resources::access_fixtures as fixtures;
use places::{make_org, person};
use support::World;

fn declaration() -> HostPorts {
    HostPorts {
        incoming_address: "node.example.net".into(),
        listen: "all".into(),
        ranges: vec![PortRange {
            from: 50_000,
            to: 50_015,
        }],
    }
}

/// A host of Ada's at home in `org`, its page started as enrolling it starts it: she owns it.
async fn adas_host(world: &World, ada: &places::Who, org: orch_types::Id) -> orch_types::Id {
    let mut conn = world.conn().await;
    let host = fixtures::insert_resource(&mut conn, "host", Some(ada.id()), Some(org), "Node")
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
    host
}

async fn report_ports(world: &World, host: orch_types::Id) {
    let mut policy = orch_link_contract::policy::LocalPolicy::installed(
        orch_link_contract::policy::PolicyDefaults::Managed,
    );
    policy.policy_delegated = vec!["*".into()];
    policy.ports.publish = Some(declaration());
    sqlx::query("UPDATE machines m SET facts=jsonb_set(m.facts,'{policy}',$2) FROM hosts h WHERE h.id=$1 AND m.id=h.machine_id")
        .bind(host).bind(policy.reported()).execute(&world.db.pool).await.unwrap();
}

#[tokio::test]
async fn a_declaration_is_not_saved_while_the_hosts_node_is_away_and_the_host_shows_none() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Office", "office").await.base.id;
    let host = adas_host(&world, &ada, org).await;
    let read = world
        .app
        .call(HostGet, &[&host])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(read.ports.is_none());
    assert_eq!(read.ports_revision, 0);
    assert!(
        read.ports_proposal.is_none(),
        "an away node proposes nothing"
    );

    let refused = world
        .app
        .call(HostUpdate, &[&host])
        .token(&ada.token)
        .body(&HostUpdateBody {
            name: None,
            labels: None,
            schedulable: None,
            when_full: None,
            ports: Some(Some(declaration())),
            expected_revision: read.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::SourceOffline);
    let after = world
        .app
        .call(HostGet, &[&host])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(after.ports.is_none(), "nothing was saved");
    assert_eq!(after.ports_revision, 0);
    assert_eq!(after.revision, read.revision, "nothing moved");

    // Clearing what is not there changes nothing either, and needs no node.
    let cleared = world
        .app
        .call(HostUpdate, &[&host])
        .token(&ada.token)
        .body(&HostUpdateBody {
            name: None,
            labels: None,
            schedulable: None,
            when_full: None,
            ports: Some(None),
            expected_revision: read.revision,
        })
        .send()
        .await
        .ok();
    assert!(cleared.ports.is_none());
    assert_eq!(cleared.revision, read.revision);
}

#[tokio::test]
async fn a_saved_declaration_is_shown_with_its_acknowledged_revision() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Office", "office").await.base.id;
    let host = adas_host(&world, &ada, org).await;
    // As a node's acceptance leaves it.
    sqlx::query("UPDATE hosts SET ports = $2, ports_revision = 3 WHERE id = $1")
        .bind(host)
        .bind(serde_json::to_value(declaration()).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    report_ports(&world, host).await;
    let read = world
        .app
        .call(HostGet, &[&host])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.ports, Some(declaration()));
    assert_eq!(read.ports_revision, 3);
}

#[tokio::test]
async fn a_stopped_environments_host_opening_gives_its_number_back_and_keeps_it_as_the_one_it_prefers()
 {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Office", "office").await.base.id;
    let host = adas_host(&world, &ada, org).await;
    let mut conn = world.conn().await;
    sqlx::query("UPDATE hosts SET ports = $2, ports_revision = 1 WHERE id = $1")
        .bind(host)
        .bind(serde_json::to_value(declaration()).unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    report_ports(&world, host).await;
    let mut conn = world.conn().await;
    let env = fixtures::insert_environment(&mut conn, ada.id(), Some(org), "Web", "managed")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO runtimes (id, kind, host_id, unit_name, environment_id, driver, template_id, spec_hash, state)
         SELECT $1, 'environment', $2, 'orch-environment-hostports', e.id, 'test', e.template_id, '\\x00'::bytea, 'stopped'
           FROM environments e WHERE e.id = $3",
    )
    .bind(orch_types::new_id())
    .bind(host)
    .bind(env)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET status = 'stopped' WHERE id = $1")
        .bind(env)
        .execute(&mut *conn)
        .await
        .unwrap();
    let session_id: orch_types::Id =
        sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
            .bind(orch_api_core::middleware::authentication::token_hash(
                &ada.token,
            ))
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let opening = orch_db::queries::ports::Opening {
        id: orch_types::new_id(),
        environment_id: env,
        port: 8080,
        kind: "host".into(),
        transports: vec!["tcp".into(), "udp".into()],
        entry_methods: vec!["host".into()],
        place_id: None,
        label: "Anyone who reaches the host".into(),
        requested_by: Some(ada.id()),
        requested_by_thread: None,
        allowed_by: Some(ada.id()),
        decider_credential_kind: Some("session".into()),
        decider_credential_id: Some(session_id),
        decider_as_admin: false,
        admission: None,
        opened_at: world.now(),
        until_at: None,
        state: "stopped".into(),
        reason: None,
        secret_hash: None,
        secret_sealed: None,
        link_recipient: None,
        link_recipient_user: None,
        approval_id: None,
        host_id: Some(host),
        publish_machine_id: None,
        host_port: Some(50_003),
        fixed_host_port: false,
        host_allocated: true,
        revision: 1,
        closed_at: None,
    };
    orch_db::queries::ports::insert(&mut conn, &opening)
        .await
        .unwrap();
    drop(conn);
    // Its node is away: it holds nothing once it starts again, so the number is given back at once.
    orch_api_core::host_ports::follow_environments(&world.app.app, false).await;
    let (held, state, number): (bool, String, Option<i32>) =
        sqlx::query_as("SELECT host_allocated, state, host_port FROM port_openings WHERE id = $1")
            .bind(opening.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(!held, "a stopped environment holds no host number");
    assert_eq!(state, "stopped");
    assert_eq!(
        number,
        Some(50_003),
        "the number stays the one it prefers when it starts again"
    );
    let free: Vec<i32> = orch_db::queries::ports::host_ports_held(&mut *world.conn().await, host)
        .await
        .unwrap();
    assert!(
        free.is_empty(),
        "another opening may take the number meanwhile"
    );
}
