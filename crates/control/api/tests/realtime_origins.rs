//! Inspection on a real socket keeps the same caller address as an ordinary request.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod realtime_support;
mod support;

use chat_world::Room;
use orch_config::ServerConfig;
use orch_db::Db;
use orch_types::Clock;
use realtime_support::{Client, Server, quick};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_inspection_records_its_resolved_peer_and_cannot_be_given_a_forged_origin() {
    for trusted in [false, true] {
        let room = Room::new().await;
        let direct = room.direct(&room.bea, &room.cai).await.base.id;
        let mut config = ServerConfig::default();
        if trusted {
            config.api.trusted_proxy_cidrs =
                vec!["127.0.0.0/8".to_owned(), "10.0.0.0/8".to_owned()];
        }
        let clock: Arc<dyn Clock> = Arc::new(room.world.app.clock.clone());
        let server = Server::over_config(
            &Db::from_pool(room.world.db.pool.clone()),
            &clock,
            quick(&config),
            config,
        )
        .await;
        let mut socket = Client::connect_headers(
            server.addr,
            &[
                ("x-forwarded-for", "198.51.100.41, 203.0.113.12, 10.1.0.2"),
                ("user-agent", "socket-origin-proof"),
            ],
        )
        .await;
        socket.auth(&room.dee.token).await;
        socket.sub(1, &format!("chat:{direct}"), None).await;
        socket.expect("subscribed").await;
        let (ip, agent): (String, Option<String>) = sqlx::query_as(
            "SELECT host(ip), user_agent FROM audit_log WHERE action = 'chat.inspected' AND object_id = $1 AND actor_id = $2")
            .bind(direct).bind(room.dee.id()).fetch_one(&room.world.db.pool).await.unwrap();
        assert_eq!(ip, if trusted { "203.0.113.12" } else { "127.0.0.1" });
        assert_eq!(agent.as_deref(), Some("socket-origin-proof"));
        server.stop().await;
    }
}
