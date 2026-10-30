//! Datagram saturation and bounded connection history preserve control, TCP and exact active peers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use bytes::Bytes;
use orch_api_contract::ports::*;
use orch_link::ports::{
    BUFFER_BYTES, CALLERS_PER_OWNER, PortRecords, RECENT_PER_OWNER, RECENT_PER_TARGET, UdpBudget,
};
use orch_link_contract::ports::{PortDatagram, PortTicketClaims};
use orch_types::{Time, new_id};
use std::sync::Arc;

#[test]
fn rule_2_full_udp_storage_and_oversize_drop_and_count_without_waiting_or_replay() {
    let store = Arc::new(UdpBudget::default());
    let mut held = Vec::new();
    while let Some(packet) = store.hold(PortDatagram {
        channel: 1,
        payload: Bytes::from(vec![7; PortDatagram::MAX_BYTES]),
    }) {
        held.push(packet);
    }
    assert!(
        held.len() * (PortDatagram::MAX_BYTES + PortDatagram::HEADER_BYTES)
            <= BUFFER_BYTES as usize
    );
    assert_eq!(store.dropped().full, 1);
    assert!(
        store
            .hold(PortDatagram {
                channel: 1,
                payload: Bytes::from(vec![0; PortDatagram::MAX_BYTES + 1])
            })
            .is_none()
    );
    assert_eq!(store.dropped().oversized, 1);
    let count = held.len();
    drop(held);
    assert_eq!(store.dropped().link_lost, count as i64);
    let empty = store
        .hold(PortDatagram {
            channel: 1,
            payload: Bytes::new(),
        })
        .unwrap()
        .delivered();
    assert!(empty.payload.is_empty());
    assert_eq!(
        store.dropped().link_lost,
        count as i64,
        "emitted packets are not loss or replay"
    );
}

fn claims(environment: orch_types::Id, opening: orch_types::Id, caller: usize) -> PortTicketClaims {
    PortTicketClaims {
        environment_id: environment,
        generation: 0,
        port: 9000,
        transport: PortTransport::Tcp,
        opening_id: Some(opening),
        audience: Some(PortOpeningKind::Link),
        authority_revision: 1,
        allocation_revision: None,
        host_template: None,
        source: PortConnectionOutside {
            address: format!("192.0.2.{}", caller % 255),
            port: 41001,
            caller: PortCaller::Link {
                id: new_id(),
                label: format!("visitor {caller}"),
            },
        },
    }
}
fn now() -> Time {
    "2026-10-13T16:37:00.000Z".parse().unwrap()
}

#[test]
fn a_lower_negotiated_caller_bound_keeps_evicted_active_rows_in_other_even_when_a_new_socket_is_admitted_later()
 {
    let mut limits = orch_link_contract::version::PortLimits::SHIPPED;
    limits.caller_totals = 4;
    let record = PortRecords::with_limits(PortConnectionOwner::Node, &limits);
    let (environment, opening) = (new_id(), new_id());
    let ids = [new_id(), new_id()];
    let mut callers = [
        claims(environment, opening, 1),
        claims(environment, opening, 2),
    ];
    for (index, caller) in callers.iter_mut().enumerate() {
        caller.source.caller = PortCaller::Address;
        record.opened(
            ids[index],
            caller,
            PortConnectionInside {
                address: "127.0.0.1".into(),
                port: 41001 + index as u16,
            },
            now(),
        );
        record.traffic(environment, ids[index], true, 7, false, now());
    }
    limits.caller_totals = 2;
    record.set_limits(&limits);
    let query = EnvironmentPortConnectionListQuery::default();
    let lower = record.list(environment, 0, &query, true, now());
    assert_eq!(lower.counts.active, 2);
    assert_eq!(lower.counts.bytes_in, 14);
    let totals = lower.totals.unwrap();
    assert_eq!(totals.iter().filter(|row| !row.other).count(), 1);
    assert_eq!(totals.iter().find(|row| row.other).unwrap().active, 1);
    let known = totals
        .iter()
        .find(|row| !row.other)
        .unwrap()
        .address
        .as_ref()
        .unwrap();
    let evicted = callers
        .iter()
        .position(|row| &row.source.address != known)
        .unwrap();
    // The larger bound admits a NEW socket from that address. The older socket still belongs to Other.
    limits.caller_totals = 4;
    record.set_limits(&limits);
    record.opened(
        new_id(),
        &callers[evicted],
        PortConnectionInside {
            address: "127.0.0.1".into(),
            port: 41004,
        },
        now(),
    );
    record.traffic(environment, ids[evicted], false, 3, false, now());
    record.ended(environment, ids[evicted], now());
    let after = record.list(environment, 0, &query, true, now());
    assert_eq!(after.counts.active, 2);
    let totals = after.totals.unwrap();
    let new_socket = totals
        .iter()
        .find(|row| row.address.as_ref() == Some(&callers[evicted].source.address))
        .unwrap();
    assert_eq!(new_socket.active, 1);
    assert_eq!(new_socket.bytes_out, 0);
    let other = totals.iter().find(|row| row.other).unwrap();
    assert_eq!(other.active, 0);
    assert_eq!(other.bytes_out, 3);
    assert!(after.truncated);
    assert!(totals.iter().all(|row| row.active >= 0));
}

#[test]
fn rule_6_recent_rows_and_caller_groups_take_one_fixed_owner_half_and_an_exact_peer_is_never_reused()
 {
    let record = PortRecords::new(PortConnectionOwner::Gateway);
    let environment = new_id();
    let opening = new_id();
    for n in 0..RECENT_PER_TARGET + 10 {
        let id = new_id();
        let claims = claims(environment, opening, n);
        record.opened(
            id,
            &claims,
            PortConnectionInside {
                address: "127.0.0.1".into(),
                port: 41001,
            },
            now(),
        );
        record.traffic(environment, id, true, 3, false, now());
        record.ended(environment, id, now());
    }
    let list = record.list(
        environment,
        0,
        &EnvironmentPortConnectionListQuery {
            limit: Some(200),
            ..Default::default()
        },
        true,
        now(),
    );
    assert_eq!(list.counts.recent, RECENT_PER_TARGET as i64);
    assert!(list.truncated);
    assert!(list.totals.as_ref().unwrap().len() <= CALLERS_PER_OWNER + 1);
    assert!(
        list.totals
            .as_ref()
            .unwrap()
            .iter()
            .any(|total| total.other)
    );
    for n in 0..RECENT_PER_OWNER + 10 {
        let id = new_id();
        let claims = claims(environment, new_id(), n);
        record.opened(
            id,
            &claims,
            PortConnectionInside {
                address: "127.0.0.1".into(),
                port: 41001,
            },
            now(),
        );
        record.ended(environment, id, now());
    }
    let list = record.list(
        environment,
        0,
        &EnvironmentPortConnectionListQuery::default(),
        false,
        now(),
    );
    assert_eq!(list.counts.recent, RECENT_PER_OWNER as i64);
    assert!(list.connections.is_none());
    assert!(list.totals.is_none());
    let id = new_id();
    let peer = PortConnectionInside {
        address: "127.0.0.1".into(),
        port: 41001,
    };
    let admitted = claims(environment, opening, 0);
    record.opened(id, &admitted, peer.clone(), now());
    assert_eq!(
        record
            .peer(environment, 0, 9000, PortTransport::Tcp, &peer)
            .unwrap()
            .id,
        id
    );
    record.ended(environment, id, now());
    assert!(
        record
            .peer(environment, 0, 9000, PortTransport::Tcp, &peer)
            .is_none()
    );
    assert!(
        record
            .peer(environment, 1, 9000, PortTransport::Tcp, &peer)
            .is_none()
    );
    let late = Time::new(now().get() + chrono::Duration::minutes(16));
    let list = record.list(
        environment,
        0,
        &EnvironmentPortConnectionListQuery::default(),
        true,
        late,
    );
    assert_eq!(list.counts.recent, 0);
}
