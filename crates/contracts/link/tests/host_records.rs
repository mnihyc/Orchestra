//! A host's observed peers and current record queries retain their concrete ephemeral link shapes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_link_contract::backplane::{DownFrame, UpFrame};
use orch_link_contract::frames::Frame;
use orch_link_contract::tunnels::Open;
use serde_json::json;

#[test]
fn a_raw_host_open_carries_only_the_observed_socket_and_an_ordinary_open_needs_no_extra_member() {
    let open:Frame=serde_json::from_value(json!({"t":"open","c":1,"ticket":"signed-template",
        "host_peer":{"use_id":"0199c1a0-0000-7000-8000-000000000001","peer":{"address":"192.0.2.9","port":41002}}})).unwrap();
    let Frame::Open(Open::Request(request)) = open else {
        panic!("a host accept is an opening request");
    };
    assert_eq!(request.host_peer.unwrap().peer.port, 41002);
    let ordinary: Frame =
        serde_json::from_value(json!({"t":"open","c":3,"ticket":"single-use"})).unwrap();
    let Frame::Open(Open::Request(request)) = ordinary else {
        panic!("an ordinary opening keeps its shape");
    };
    assert!(request.host_peer.is_none());
}

#[test]
fn live_queries_and_answers_cross_the_session_without_a_command_id_deadline_or_retained_result() {
    let query = json!({"t":"port_connections_query","request_ref":"0199c1a0-0000-7000-8000-000000000001",
        "environment_id":"0199c1a0-0000-7000-8000-000000000002","generation":4,"query":{"port":3000,"limit":50}});
    let down: DownFrame = serde_json::from_value(query).unwrap();
    let session: Frame = down.into();
    assert!(matches!(&session, Frame::PortConnectionsQuery(_)));
    assert!(
        !serde_json::to_value(session)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("deadline")
    );
    let reply = json!({"t":"port_connections_reply","request_ref":"0199c1a0-0000-7000-8000-000000000001",
        "environment_id":"0199c1a0-0000-7000-8000-000000000002","connections":{
            "as_of":"2026-10-14T20:00:00.000Z","owner":"node","counts":{"active":0,"recent":0,"http_requests":null,
                "bytes_in":0,"bytes_out":0,"tracked_since":"2026-10-14T20:00:00.000Z","reset_at":"2026-10-14T20:00:00.000Z"},
            "connections":[],"totals":[],"cursor":null,"truncated":false}});
    let session: Frame = serde_json::from_value(reply.clone()).unwrap();
    let up = UpFrame::try_from(session).unwrap();
    assert_eq!(serde_json::to_value(up).unwrap(), reply);
}
