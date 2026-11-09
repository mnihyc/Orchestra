//! Signed creation replay under the existing genuinely connected native backplane cut.
#[path = "../../../tests/ports_core/fixtures.rs"]
mod fixtures;
use fixtures::token;
use http::StatusCode;
use orch_api_contract::ports::*;
use orch_types::{Id, Time, new_id};
use serde_json::json;

#[tokio::test]
async fn signed_port_use_headers_keep_tcp_and_udp_admissions_without_a_second_proof_or_native_observation()
 {
    for transport in [PortTransport::Tcp, PortTransport::Udp] {
        let (world, manager, _, env, _, machine) = fixtures::setup().await;
        sqlx::query("UPDATE environments SET status='running',facts=COALESCE(facts,'{}'::jsonb)||jsonb_build_object('ports',jsonb_build_object('transports',jsonb_build_array('tcp','udp'))) WHERE id=$1")
            .bind(env).execute(&world.db.pool).await.unwrap();
        let generation: i64 =
            sqlx::query_scalar("SELECT generation::bigint FROM environments WHERE id=$1")
                .bind(env)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        let now = Time::new(world.now());
        world.app.app.cache.ports().observe(PortSnapshot {
            environment_id: env,
            generation,
            as_of: now,
            activity_at: None,
            discovery: PortDiscovery::Native,
            reason: None,
            listeners: vec![PortObservation {
                port: 8080,
                transport,
                local_address: "127.0.0.1:8080".into(),
                generation,
                observation_id: "native admission fixture".into(),
                first_seen_at: now,
                binding: if transport == PortTransport::Tcp {
                    PortBinding::Listening
                } else {
                    PortBinding::BoundUnknown
                },
                process: None,
                internal: false,
                unavailable_reason: None,
            }],
            udp_dropped: Default::default(),
        });
        let (issuer, writer) = token(
            &world,
            manager.id(),
            env,
            "EnvironmentPortUseCreate",
            &["read", "write"],
        )
        .await;
        let act = new_id();
        let input = EnvironmentPortUseCreateBody {
            transport,
            opening_id: None,
        };
        let offline = world
            .app
            .call(EnvironmentPortUseCreate, &[&env, &"8080"])
            .token(&writer)
            .body(&input);
        assert_eq!(
            offline.send_bytes().await.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "a cached listener does not replace an actual current connector"
        );
        let _wire = super::Wire::new(&world.app.app.link, machine).await;
        assert!(
            world.app.app.link.is_online(machine),
            "the actual native connector cut is connected"
        );
        let call = world
            .app
            .call(EnvironmentPortUseCreate, &[&env, &"8080"])
            .token(&writer)
            .header("Idempotency-Key", &act.to_string())
            .body(&input);
        let first = call.send_bytes().await;
        assert_eq!(
            first.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&first.raw)
        );
        let first: serde_json::Value = serde_json::from_slice(&first.raw).unwrap();
        let id: Id = serde_json::from_value(first["id"].clone()).unwrap();
        assert!(!first["ticket"].as_str().unwrap().is_empty());
        assert_eq!(
            world.app.app.cache.ports().pending_transport(id),
            Some(transport)
        );
        let before: (serde_json::Value, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(manager.id())
        .bind(act.to_string())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(before.0["outcome"]["id"], json!(id));
        assert_eq!(
            before.0["outcome"]["data"],
            json!({"ticket_expires_at":first["ticket_expires_at"]})
        );
        assert!(
            !before
                .0
                .to_string()
                .contains(first["ticket"].as_str().unwrap())
        );
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(issuer)
            .bind(world.now())
            .execute(&world.db.pool)
            .await
            .unwrap();
        world.app.app.cache.ports().clear();
        assert_eq!(world.app.app.cache.ports().pending_transport(id), None);
        let (_, reader) = token(&world, manager.id(), env, "EnvironmentPortGet", &["read"]).await;
        let call = world
            .app
            .call(EnvironmentPortUseCreate, &[&env, &"8080"])
            .token(&reader)
            .header("Idempotency-Key", &act.to_string())
            .body(&input);
        let replay = call.send_bytes().await;
        assert_eq!(replay.status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&replay.raw).unwrap(),
            json!({
                "projection":"receipt","ticket_id":id,"ticket_expires_at":first["ticket_expires_at"]
            })
        );
        assert_eq!(world.app.app.cache.ports().pending_transport(id), None);
        let after: (serde_json::Value, String) = sqlx::query_as(
            "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
        )
        .bind(manager.id())
        .bind(act.to_string())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(after, before);
    }
}
