//! A harness whose key is kept on another machine: its model call goes to that machine through an `llm` tunnel of the link, which runs the same steps there; the harness's machine never has the key, and the call is held to the lease on the machine that has it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::model::{MODEL, ProviderRig, RESPONSES, files_holding, responses_request};
use orch_connector_core::loopback::Tickets;
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_pki::tickets::{TicketRequest, TicketSigner};
use orch_testkit::doubles::model::Verdict;
use orch_types::{Id, Time, new_id};
use serde_json::json;

fn ticket(signer: &TicketSigner, p: &ProviderRig, from: Id, lease: Id, thread: Id) -> String {
    signer
        .issue(&TicketRequest {
            src: TicketSource::Machine(from),
            dst: p.rig.machine_id,
            service: Service::Llm { lease_id: lease },
            thread_id: Some(thread),
            tool_set: None,
            expires_at: Time::new(p.rig.clock.now() + chrono::Duration::hours(1)),
        })
        .unwrap()
}

#[tokio::test]
async fn a_call_of_a_harness_on_another_machine_goes_through_a_tunnel_to_the_machine_that_has_the_key()
 {
    let p = ProviderRig::new().await;
    let signer = TicketSigner::generate();
    p.rig.hub.set_ticket_verifier(signer.verifier());
    p.start_responses_provider().await;
    let lease = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let agent = p.add_agent_machine().await;

    let loopback = agent.connector.loopback();
    let grant = loopback.grant();
    let binding = loopback.bind(
        grant.id,
        p.thread,
        p.segment,
        p.rig.provider,
        p.rig.agent,
        Tickets {
            llm: Some(ticket(&signer, &p, agent.machine_id, lease, p.thread)),
            ..Tickets::default()
        },
    );
    let client = orch_testkit::doubles::model::ModelClient::new(&format!(
        "http://127.0.0.1:{}",
        loopback.port()
    ));
    let call = |model: &'static str| {
        let request = orch_testkit::doubles::model::ClientRequest::post_json(
            &format!("/llm/{}/v1/responses", binding.session_key),
            &responses_request(model, "hello"),
        )
        .bearer(grant.token.expose());
        client.send(request)
    };

    let reply = call(MODEL).await.unwrap();
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        reply.json()["output"][0]["content"][0]["text"],
        "You said: hello"
    );
    let seen = p.model_requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].credential.verdict, Verdict::Accepted);
    assert!(
        !serde_json::to_string(&seen[0].headers)
            .unwrap()
            .contains(grant.token.expose()),
        "the token of the harness's listener goes no further than the listener"
    );
    // It was counted under the lease, where the key is.
    let usage = p.wait_provider_event("lease.usage", 1).await;
    assert_eq!(usage[0].body["lease_id"], json!(lease));

    // The lease on the other machine holds the call, in the protocol's shape.
    let outside = call("a-model-the-lease-does-not-name").await.unwrap();
    assert_eq!(outside.status, 403);
    assert_eq!(outside.json()["error"]["code"], "model_not_allowed");

    // The machine of the harness never had the key.
    assert!(files_holding(agent.data.path(), &p.key).is_empty());

    // After the lease is closed the call is refused there.
    p.close_lease(lease).await;
    let closed = call(MODEL).await.unwrap();
    assert_eq!(closed.status, 403);
    assert_eq!(closed.json()["error"]["code"], "lease_closed");
    assert_eq!(p.model_requests().len(), 1);
}

#[tokio::test]
async fn a_ticket_names_the_lease_but_the_next_turn_of_the_thread_is_served_under_its_own() {
    let p = ProviderRig::new().await;
    let signer = TicketSigner::generate();
    p.rig.hub.set_ticket_verifier(signer.verifier());
    p.start_responses_provider().await;
    let first = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let agent = p.add_agent_machine().await;
    let loopback = agent.connector.loopback();
    let grant = loopback.grant();
    let binding = loopback.bind(
        grant.id,
        p.thread,
        p.segment,
        p.rig.provider,
        p.rig.agent,
        Tickets {
            llm: Some(ticket(&signer, &p, agent.machine_id, first, p.thread)),
            ..Tickets::default()
        },
    );
    let client = orch_testkit::doubles::model::ModelClient::new(&format!(
        "http://127.0.0.1:{}",
        loopback.port()
    ));
    let call = || {
        let request = orch_testkit::doubles::model::ClientRequest::post_json(
            &format!("/llm/{}/v1/responses", binding.session_key),
            &responses_request(MODEL, "hello"),
        )
        .bearer(grant.token.expose());
        client.send(request)
    };
    assert_eq!(call().await.unwrap().status, 200);
    p.close_lease(first).await;
    assert_eq!(
        call().await.unwrap().status,
        403,
        "no lease is open for the thread"
    );
    // The turn after: a lease of its own. The ticket of the session still opens the tunnel.
    let second = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    assert_ne!(first, second);
    assert_eq!(call().await.unwrap().status, 200);
    // But it is for the thread it names: another thread's lease is not its leave.
    p.close_lease(second).await;
    p.open_lease(new_id(), &[MODEL], Some(RESPONSES), None)
        .await;
    assert_eq!(call().await.unwrap().status, 403);
}

#[tokio::test]
async fn an_agent_machine_that_cannot_reach_the_machine_with_the_key_tells_the_harness_to_wait() {
    let p = ProviderRig::new().await;
    let signer = TicketSigner::generate();
    p.rig.hub.set_ticket_verifier(signer.verifier());
    let agent = p.add_agent_machine().await;
    let loopback = agent.connector.loopback();
    let grant = loopback.grant();
    // A ticket to a machine that is nowhere.
    let nowhere = signer
        .issue(&TicketRequest {
            src: TicketSource::Machine(agent.machine_id),
            dst: new_id(),
            service: Service::Llm { lease_id: new_id() },
            thread_id: Some(p.thread),
            tool_set: None,
            expires_at: Time::new(p.rig.clock.now() + chrono::Duration::hours(1)),
        })
        .unwrap();
    let binding = loopback.bind(
        grant.id,
        p.thread,
        p.segment,
        p.rig.provider,
        p.rig.agent,
        Tickets {
            llm: Some(nowhere),
            ..Tickets::default()
        },
    );
    let client = orch_testkit::doubles::model::ModelClient::new(&format!(
        "http://127.0.0.1:{}",
        loopback.port()
    ));
    let request = orch_testkit::doubles::model::ClientRequest::post_json(
        &format!("/llm/{}/v1/responses", binding.session_key),
        &responses_request(MODEL, "hello"),
    )
    .bearer(grant.token.expose());
    let reply = tokio::time::timeout(Duration::from_secs(40), client.send(request))
        .await
        .expect("the harness is answered")
        .unwrap();
    assert_eq!(reply.status, 503);
    assert_eq!(reply.json()["error"]["code"], "provider_unavailable");
    assert!(reply.header("retry-after").is_some());
}
