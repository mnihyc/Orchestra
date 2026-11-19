//! Scoped admission and revocation on the existing real connector port walk.

use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;

pub(super) async fn scoped_native_uses(
    world: &ports_support::World,
    client: &TestApp,
    app: &orch_api_core::state::App,
    user: Id,
    environment: Id,
    port: u16,
    opening: Id,
) {
    for (operation, actions, target, allowed) in [
        ("EnvironmentPortUseCreate", vec!["read"], environment, false),
        (
            "EnvironmentPortUseCreate",
            vec!["write"],
            environment,
            false,
        ),
        (
            "EnvironmentPortEnter",
            vec!["read", "write"],
            environment,
            false,
        ),
        (
            "EnvironmentPortUseCreate",
            vec!["read", "write"],
            new_id(),
            false,
        ),
        (
            "EnvironmentPortUseCreate",
            vec!["read", "write"],
            environment,
            true,
        ),
    ] {
        let scope:KeyScope=serde_json::from_value(serde_json::json!({"rules":[{"target":{"kind":"environment","id":target,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]}]})).unwrap();
        assert!(scope.validate().is_empty());
        let id = new_id();
        let secret = new_token(API_TOKEN_PREFIX);
        let now = world.clock.now();
        identity::create_api_token_scoped(
            &mut *world.db.acquire().await.unwrap(),
            &identity::NewApiToken {
                id,
                user_id: user,
                name: "Real connector scoped port use",
                token_hash: &token_hash(&secret),
                scopes: &[],
                node_id: None,
                at: now,
                expires_at: Some(now + chrono::Duration::hours(1)),
            },
            Some(&serde_json::to_value(scope).unwrap()),
        )
        .await
        .unwrap();
        let call = client
            .call(EnvironmentPortUseCreate, &[&environment, &port.to_string()])
            .token(&secret)
            .body(&EnvironmentPortUseCreateBody {
                transport: PortTransport::Tcp,
                opening_id: Some(opening),
            });
        let response = call.send().await;
        if !allowed {
            assert_eq!(response.status, http::StatusCode::FORBIDDEN);
            continue;
        }
        let usage = response.ok();
        assert_eq!(usage.environment_id, environment);
        let ingress = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let _outside = tokio::net::TcpStream::connect(ingress.local_addr().unwrap())
            .await
            .unwrap();
        let (_accepted, peer) = ingress.accept().await.unwrap();
        assert!(
            app.cache
                .ports()
                .open_channel(usage.id, &usage.ticket, peer, HttpSurface::Browser)
                .await
                .is_err(),
            "another surface cannot spend the key's native use"
        );
        let mut channel = app
            .cache
            .ports()
            .open_channel(usage.id, &usage.ticket, peer, HttpSurface::Key)
            .await
            .unwrap();
        channel.write_all(b"scope-bound").await.unwrap();
        let mut bytes = [0; 11];
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            channel.read_exact(&mut bytes),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            &bytes, b"scope-bound",
            "the actual owned inside program must exchange bytes"
        );
        let closer = channel.closer();
        assert!(
            identity::revoke_api_token(
                &mut *world.db.acquire().await.unwrap(),
                id,
                user,
                world.clock.now()
            )
            .await
            .unwrap()
        );
        app.cache.ports().recheck().await;
        assert!(
            closer.is_closed(),
            "native traffic must retain the original key rather than its person id"
        );
    }
}
