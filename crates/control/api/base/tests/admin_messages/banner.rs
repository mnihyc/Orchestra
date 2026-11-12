//! The optional banner is the existing setting write, atomic with the one accepted message.
use super::*;
use orch_settings::Place;
use orch_settings_contract::registry::keys;

async fn banner(world: &World) -> Json {
    orch_settings::get::<Json>(&mut *world.conn().await, Place::System, keys::SYSTEM_BANNER)
        .await
        .unwrap()
        .unwrap_or(Json::Null)
}

#[tokio::test]
async fn a_system_message_sets_the_native_banner_once_and_replay_does_not_replace_a_later_banner() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let mut body = body();
    body.banner_until = Some(orch_types::Time::new(world.now() + Duration::hours(3)));
    let act = new_id();
    let call = world
        .app
        .call(AdminMessageCreate, &[])
        .token(&c.root.token)
        .header("Idempotency-Key", &act.to_string())
        .body(&body);
    let first = call.send_bytes().await;
    assert_eq!(
        first.status.as_u16(),
        201,
        "{}",
        String::from_utf8_lossy(&first.raw)
    );
    let first: Json = serde_json::from_slice(&first.raw).unwrap();
    assert_eq!(
        banner(&world).await,
        json!({"text":body.title,"level":"info","until":body.banner_until})
    );
    let later = json!({"text":"A later native banner","level":"warning","until":null});
    world.set("system.banner", later.clone()).await;
    let before: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(c.root.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    let call = world
        .app
        .call(AdminMessageCreate, &[])
        .token(&c.root.token)
        .header("Idempotency-Key", &act.to_string())
        .body(&body);
    let replay = call.send_bytes().await;
    assert_eq!(replay.status.as_u16(), 201);
    assert_eq!(
        serde_json::from_slice::<Json>(&replay.raw).unwrap()["id"],
        first["id"]
    );
    assert_eq!(banner(&world).await, later);
    assert_eq!(count(&world).await, 1);
    assert_eq!(
        admin_support::lines_of(&world, "message.sent").await.len(),
        1
    );
    let after: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(c.root.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn an_organization_message_cannot_write_a_system_banner_or_send_any_partial_notice() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let mut body = body();
    body.banner_until = Some(orch_types::Time::new(world.now() + Duration::hours(3)));
    let refused = org_send(&world, &c.boss.token, c.acme, &body).await;
    assert_eq!(refused.status.as_u16(), 400);
    assert_eq!(count(&world).await, 0);
    assert!(banner(&world).await.is_null());
    assert!(
        admin_support::lines_of(&world, "message.sent")
            .await
            .is_empty()
    );
}
