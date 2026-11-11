//! The actual front's client address, rather than a caller's first header, bounds sign-in tries.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_api_contract::auth::{AuthLogin, AuthLoginBody, AuthResult};
use orch_api_contract::common::ErrorCode;
use orch_api_core::testing::Reply;
use orch_config::ServerConfig;
use std::net::SocketAddr;
use support::World;

async fn attempt(
    world: &World,
    peer: &str,
    name: &str,
    header: &'static str,
    chain: &str,
) -> Reply<AuthResult> {
    world
        .app
        .call(AuthLogin, &[])
        .peer(SocketAddr::new(peer.parse().unwrap(), 0))
        .header(header, chain)
        .body(&AuthLoginBody {
            login: name.to_owned(),
            password: Some("an incorrect password".to_owned()),
            email_code: None,
            totp: None,
            recovery_code: None,
        })
        .send()
        .await
}

fn proxy_config() -> ServerConfig {
    let mut config = ServerConfig::default();
    config.api.trusted_proxy_cidrs = vec!["127.0.0.0/8".to_owned(), "10.0.0.0/8".to_owned()];
    config
}

async fn address_buckets(world: &World) -> Vec<String> {
    world
        .buckets()
        .await
        .into_iter()
        .filter(|bucket| bucket.starts_with("address|"))
        .collect()
}

#[tokio::test]
async fn a_caller_supplied_first_address_does_not_choose_its_failed_try_bucket() {
    let world = World::with(proxy_config()).await;
    let failures = world.app.app.config.auth.address_failure_limit;
    for index in 0..failures {
        let chain = format!("198.51.100.{}, 203.0.113.12, 127.0.0.1", 41 + index);
        let reply = attempt(
            &world,
            "127.0.0.2",
            &format!("unknown-{index}"),
            "x-forwarded-for",
            &chain,
        )
        .await;
        assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
    }
    let reply = attempt(
        &world,
        "127.0.0.2",
        "another-name",
        "x-forwarded-for",
        "198.51.100.99, 203.0.113.12, 127.0.0.1",
    )
    .await;
    assert_eq!(reply.status, 429);
    assert_eq!(reply.problem().code, ErrorCode::RateLimited);
    assert_eq!(address_buckets(&world).await, ["address|203.0.113.12"]);
    let audit_addresses: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT host(ip) FROM audit_log WHERE action = 'auth.login_failed'",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audit_addresses, ["203.0.113.12"]);
    assert_eq!(
        attempt(
            &world,
            "127.0.0.2",
            "another-caller",
            "x-forwarded-for",
            "203.0.113.13, 127.0.0.1"
        )
        .await
        .status,
        401
    );
}

#[tokio::test]
async fn direct_calls_with_rotating_headers_are_counted_at_their_connection() {
    let mut config = proxy_config();
    // A smaller configured cap makes this independent direct-connection control quick.
    config.auth.address_failure_limit = 2;
    let world = World::with(config).await;
    for (index, header) in ["x-forwarded-for", "forwarded"].into_iter().enumerate() {
        let chain = if header == "forwarded" {
            "for=198.51.100.42"
        } else {
            "198.51.100.41"
        };
        assert_eq!(
            attempt(
                &world,
                "203.0.113.12",
                &format!("ghost-{index}"),
                header,
                chain
            )
            .await
            .status,
            401
        );
    }
    assert_eq!(
        attempt(
            &world,
            "203.0.113.12",
            "ghost-final",
            "x-forwarded-for",
            "198.51.100.43"
        )
        .await
        .status,
        429
    );
    assert_eq!(address_buckets(&world).await, ["address|203.0.113.12"]);
}

#[tokio::test]
async fn two_trusted_hops_mapped_addresses_and_standard_headers_share_one_address() {
    let mut config = proxy_config();
    config.auth.address_failure_limit = 2;
    let world = World::with(config).await;
    assert_eq!(
        attempt(
            &world,
            "::ffff:127.0.0.1",
            "ghost-one",
            "x-forwarded-for",
            "198.51.100.41, ::ffff:203.0.113.12, 10.0.0.1"
        )
        .await
        .status,
        401
    );
    assert_eq!(
        attempt(
            &world,
            "127.0.0.1",
            "ghost-two",
            "forwarded",
            "for=198.51.100.99, for=203.0.113.12, for=10.0.0.1"
        )
        .await
        .status,
        401
    );
    assert_eq!(
        attempt(
            &world,
            "127.0.0.1",
            "ghost-three",
            "x-forwarded-for",
            "203.0.113.12, 10.0.0.1"
        )
        .await
        .status,
        429
    );
    assert_eq!(address_buckets(&world).await, ["address|203.0.113.12"]);
}

#[tokio::test]
async fn a_malformed_chain_and_an_empty_trust_list_use_the_connection() {
    for (config, chain) in [
        (proxy_config(), "bad, 203.0.113.12"),
        (ServerConfig::default(), "203.0.113.12"),
    ] {
        let world = World::with(config).await;
        assert_eq!(
            attempt(&world, "127.0.0.1", "ghost", "x-forwarded-for", chain)
                .await
                .status,
            401
        );
        assert_eq!(address_buckets(&world).await, ["address|127.0.0.1"]);
    }
}

#[tokio::test]
async fn a_header_without_socket_metadata_cannot_claim_an_address() {
    let world = World::with(proxy_config()).await;
    let reply = world
        .app
        .call(AuthLogin, &[])
        .header("x-forwarded-for", "198.51.100.41")
        .body(&AuthLoginBody {
            login: "ghost".to_owned(),
            password: Some("wrong password".to_owned()),
            email_code: None,
            totp: None,
            recovery_code: None,
        })
        .send()
        .await;
    assert_eq!(reply.status, 401);
    assert!(address_buckets(&world).await.is_empty());
}
