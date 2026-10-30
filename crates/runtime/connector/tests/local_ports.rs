//! An attached environment gets only its own read bearer, which dies on detach; its HTTP reader never writes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod rig_environment;

use orch_connector_core::local_ports::{LocalReadError, read_local};
use orch_link_contract::commands::EnvDetach;
use orch_plugin_contract::Secret;
use orch_types::new_id;
use rig_environment::EnvRig;

async fn raw(base: &str, method: hyper::Method, host: &str) -> hyper::StatusCode {
    let parsed = url::Url::parse(base).unwrap();
    let socket = tokio::net::TcpStream::connect(("127.0.0.1", parsed.port().unwrap()))
        .await
        .unwrap();
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(socket))
            .await
            .unwrap();
    let connection = tokio::spawn(async move {
        let _ = connection.await;
    });
    let request = hyper::Request::builder()
        .method(method)
        .uri("/ports")
        .header(hyper::header::HOST, host)
        .body(http_body_util::Full::new(bytes::Bytes::new()))
        .unwrap();
    let status = tokio::time::timeout(rig_environment::PATIENCE, sender.send_request(request))
        .await
        .unwrap()
        .unwrap()
        .status();
    connection.abort();
    status
}

fn variables(rig: &EnvRig) -> std::collections::BTreeMap<String, String> {
    rig.connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap()
        .variables()
        .clone()
}

#[tokio::test]
async fn connected_environments_have_distinct_bearers_and_reattaching_rotates_the_one_retired_by_detach()
 {
    let mut rig = EnvRig::new();
    rig.start().await;
    let first = rig.environment;
    let root = rig.directory("one");
    let reported = rig.attach(&root).await;
    let one = variables(&rig);
    let token = Secret::new(one["ORCH_PORTS_TOKEN"].clone());
    assert!(
        !reported.to_string().contains(token.expose()),
        "a local process bearer is not a link fact or command answer"
    );
    assert_eq!(
        raw(&one["ORCH_PORTS_URL"], hyper::Method::GET, "localhost").await,
        hyper::StatusCode::MISDIRECTED_REQUEST
    );
    let host = format!(
        "127.0.0.1:{}",
        url::Url::parse(&one["ORCH_PORTS_URL"])
            .unwrap()
            .port()
            .unwrap()
    );
    assert_eq!(
        raw(&one["ORCH_PORTS_URL"], hyper::Method::POST, &host).await,
        hyper::StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(token.expose().len(), 64); // 32 random bytes, hex encoded; not a user's or harness's token.
    let who = "/whois?peer=127.0.0.1%3A43123&listener_port=8080&transport=tcp";
    assert!(
        read_local(&one["ORCH_PORTS_URL"], Some(&token), who)
            .await
            .unwrap()
            .is_null()
    );
    assert!(matches!(
        read_local(&one["ORCH_PORTS_URL"], None, who).await,
        Err(LocalReadError::Refused(401))
    ));
    rig.environment = new_id();
    let second_root = rig.directory("two");
    rig.attach(&second_root).await;
    let two = variables(&rig);
    assert_ne!(one["ORCH_PORTS_TOKEN"], two["ORCH_PORTS_TOKEN"]);
    assert!(matches!(
        read_local(&two["ORCH_PORTS_URL"], Some(&token), who).await,
        Err(LocalReadError::Refused(401))
    ));
    let second_token = Secret::new(two["ORCH_PORTS_TOKEN"].clone());
    assert!(
        read_local(&two["ORCH_PORTS_URL"], Some(&second_token), who)
            .await
            .unwrap()
            .is_null()
    );
    rig.environment = first;
    rig.succeed(&EnvDetach {
        environment_id: first,
    })
    .await;
    assert!(
        read_local(&one["ORCH_PORTS_URL"], Some(&token), who)
            .await
            .is_err()
    );
    rig.attach(&root).await;
    let fresh = variables(&rig);
    assert_ne!(fresh["ORCH_PORTS_TOKEN"], one["ORCH_PORTS_TOKEN"]);
    assert!(matches!(
        read_local(&fresh["ORCH_PORTS_URL"], Some(&token), who).await,
        Err(LocalReadError::Refused(401))
    ));
    rig.stop().await;
}
