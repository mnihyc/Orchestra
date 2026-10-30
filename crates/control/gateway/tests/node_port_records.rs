//! Host records are queried live through authenticated node sessions and never through command history.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World};
use orch_api_contract::ports::{
    EnvironmentPortConnectionListQuery, PortCaller, PortConnectionInside, PortConnectionOutside,
    PortConnectionOwner, PortTransport,
};
use orch_link::ports::PortRecords;
use orch_link_contract::enrollment::Trust;
use orch_link_contract::ports::{HostPortTemplate, PortTicketClaims};
use orch_link_contract::session::Role;
use orch_types::new_id;
use std::sync::Arc;

#[tokio::test]
async fn current_node_memory_reaches_the_api_with_its_owner_peers_and_reset_without_a_durable_command()
 {
    let mut world = World::start().await;
    let enrolled = world.enroll("record-node").await;
    let node = world.start_machine_as(
        &enrolled,
        enrolled.endpoints(Trust::Pinned),
        vec![Role::Node],
    );
    tokio::time::timeout(PATIENCE, node.link.connected())
        .await
        .unwrap();
    let records = Arc::new(PortRecords::new(PortConnectionOwner::Node));
    node.link.serve_port_records(records.clone());
    let (environment, connection, opening) = (new_id(), new_id(), new_id());
    let now = world.clock.time();
    // The observed socket fixture stands only for the node's in-memory producer. The node
    // host_publish suite supplies real TCP/UDP sockets and distinct actual inside peers.
    records.opened(
        connection,
        &PortTicketClaims {
            environment_id: environment,
            generation: 1,
            port: 3000,
            transport: PortTransport::Tcp,
            opening_id: Some(opening),
            audience: Some(orch_api_contract::ports::PortOpeningKind::Host),
            authority_revision: 1,
            allocation_revision: Some(1),
            host_template: Some(HostPortTemplate {
                host_id: new_id(),
                proxy_protocol: false,
            }),
            source: PortConnectionOutside {
                address: "192.0.2.7".into(),
                port: 41001,
                caller: PortCaller::Address,
            },
        },
        PortConnectionInside {
            address: "127.0.0.1".into(),
            port: 41002,
        },
        now,
    );
    records.traffic(environment, connection, true, 17, false, now);
    let query = EnvironmentPortConnectionListQuery {
        inside_port: Some(41002),
        ..Default::default()
    };
    let list = tokio::time::timeout(
        PATIENCE,
        world
            .api
            .link
            .node_port_connections(node.id, environment, 1, query.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(list.owner, PortConnectionOwner::Node);
    assert_eq!(list.counts.active, 1);
    assert_eq!(list.counts.bytes_in, 17);
    assert_eq!(list.counts.reset_at, now);
    let row = &list.connections.unwrap()[0];
    assert_eq!(row.id, connection);
    assert_eq!(row.outside.address, "192.0.2.7");
    assert_eq!(row.inside.port, 41002);
    records.ended(environment, connection, world.clock.time());
    let ended = world
        .api
        .link
        .node_port_connections(node.id, environment, 1, query)
        .await
        .unwrap();
    assert_eq!(ended.counts.active, 0);
    assert!(ended.connections.unwrap()[0].ended_at.is_some());
    let durable: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE machine_id=$1")
        .bind(node.id)
        .fetch_one(world.db.pool())
        .await
        .unwrap();
    assert_eq!(
        durable, 0,
        "the live query is neither journalled nor retained"
    );
    node.link.shutdown().await;
    world.stop().await;
}
