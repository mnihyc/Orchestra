//! One real waiting/automatic request and its current readback, without another card or effect.
use super::*;
use crate::thread_history_scope::{Proof, key, rule};
use http::StatusCode;

async fn header(kind: PortOpeningKind, automatic: bool) -> Proof {
    let (world, manager, _, env, project, machine) = setup().await;
    let capability = match kind {
        PortOpeningKind::Place => "port_place",
        PortOpeningKind::Link => "port_link",
        PortOpeningKind::Host => "port_host",
    };
    port_policy(&world, capability, if automatic { "allow" } else { "ask" }).await;
    if kind == PortOpeningKind::Host {
        let ports = orch_link_contract::policy::MachinePorts {
            allowed: vec![PortOpeningKind::Host],
            publish: Some(HostPorts {
                incoming_address: "dedicated-vm.example.test".into(),
                listen: "127.0.0.1".into(),
                ranges: vec![PortRange {
                    from: 50000,
                    to: 50002,
                }],
            }),
            limits: None,
        };
        sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,ports}',$2) WHERE id=$1")
            .bind(machine)
            .bind(serde_json::to_value(ports)?)
            .execute(&world.db.pool)
            .await?;
    }
    let thread = asking_thread(&world, &manager, project, env).await;
    let mut input = request(8080, project, "Native thread header request");
    input.kind = kind;
    if kind != PortOpeningKind::Place {
        input.place_id = None;
    }
    if kind == PortOpeningKind::Host {
        input.host_port = Some(50000);
        input.transports = Some(vec![PortTransport::Tcp]);
    }
    if kind == PortOpeningKind::Link {
        input.label = Some("Native thread link".into());
        input.until = Some(Time::new(world.now() + chrono::Duration::hours(1)));
    }
    let writer = key(
        &world,
        manager.id(),
        vec![rule(
            "ThreadPortRequestCreate",
            "thread",
            thread,
            "self",
            "write",
        )],
    )
    .await?;
    let act = new_id();
    let call = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&writer.secret)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let first = call.send_bytes().await;
    assert_eq!(
        first.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&first.raw)
    );
    let first: serde_json::Value = serde_json::from_slice(&first.raw)?;
    assert_eq!(first["projection"], "receipt");
    assert_eq!(first["created"], true);
    let opening: Id = serde_json::from_value(first["opening_id"].clone())?;
    assert_eq!(first["request_id"].is_null(), automatic);
    let before: (serde_json::Value, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(manager.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await?;
    assert_eq!(before.0["outcome"]["id"], json!(opening));
    assert!(before.0["outcome"]["data"].get("label").is_none());
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM port_openings WHERE environment_id=$1),(SELECT count(*) FROM approvals WHERE kind='port')")
        .bind(env).fetch_one(&world.db.pool).await?;
    assert_eq!(counts, (1, if automatic { 0 } else { 1 }));
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(writer.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await?;
    port_policy(&world, capability, "deny").await;
    sqlx::query("UPDATE port_openings SET label='Current native request metadata',revision=revision+1 WHERE id=$1")
        .bind(opening).execute(&world.db.pool).await?;
    let mut read = vec![rule(
        "ThreadPortRequestCreate",
        "environment",
        env,
        "self",
        "read",
    )];
    if kind == PortOpeningKind::Host {
        read.push(rule(
            "ThreadPortRequestCreate",
            "machine",
            machine,
            "self",
            "read",
        ));
    }
    let reader = key(&world, manager.id(), read).await?;
    let call = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&reader.secret)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let current = call.send_bytes().await;
    assert_eq!(
        current.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&current.raw)
    );
    let current: serde_json::Value = serde_json::from_slice(&current.raw)?;
    assert_eq!(current["opening"]["id"], json!(opening));
    assert_eq!(
        current["opening"]["label"],
        "Current native request metadata"
    );
    assert!(current["link"].is_null());
    if !automatic {
        assert_eq!(current["request"]["id"], first["request_id"]);
    }
    let neighbour = key(
        &world,
        manager.id(),
        vec![rule("PortOpeningGet", "environment", env, "self", "read")],
    )
    .await?;
    let call = world
        .app
        .call(ThreadPortRequestCreate, &[&thread])
        .token(&neighbour.secret)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let hidden = call.send_bytes().await;
    assert_eq!(hidden.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&hidden.raw)?,
        first
    );
    let after: (serde_json::Value, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(manager.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await?;
    assert_eq!(after, before);
    let final_counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM port_openings WHERE environment_id=$1),(SELECT count(*) FROM approvals WHERE kind='port')")
        .bind(env).fetch_one(&world.db.pool).await?;
    assert_eq!(final_counts, counts);
    Ok(())
}

#[tokio::test]
async fn a_waiting_place_request_header_keeps_one_card_and_its_current_readback() -> Proof {
    header(PortOpeningKind::Place, false).await
}
#[tokio::test]
async fn a_waiting_link_request_header_keeps_one_card_without_another_secret_delivery() -> Proof {
    header(PortOpeningKind::Link, false).await
}
#[tokio::test]
async fn a_waiting_host_request_header_keeps_one_card_without_another_reservation() -> Proof {
    header(PortOpeningKind::Host, false).await
}
#[tokio::test]
async fn an_automatic_place_request_header_keeps_one_opening_and_its_current_readback() -> Proof {
    header(PortOpeningKind::Place, true).await
}
#[tokio::test]
async fn an_automatic_link_request_header_never_delivers_the_first_sealed_link_again() -> Proof {
    header(PortOpeningKind::Link, true).await
}
#[tokio::test]
async fn an_automatic_host_request_header_never_reserves_or_publishes_again() -> Proof {
    header(PortOpeningKind::Host, true).await
}
