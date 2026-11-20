//! A machine declaration is visible through a live personal port reach entry.

use super::*;
use orch_db::queries::access;

#[tokio::test]
async fn the_port_list_follows_the_current_readers_reach_and_the_machine_declaration() {
    let (world, manager, viewer, environment, _, machine) = setup().await;
    sqlx::query("UPDATE environments SET status='running',facts=facts || jsonb_build_object('ports',jsonb_build_object('transports',jsonb_build_array('tcp'))) WHERE id=$1")
        .bind(environment).execute(&world.db.pool).await.unwrap();
    let port = 5901;
    let entry = new_id();
    let mut conn = world.conn().await;
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,ports,loopback_tcp}',$2::jsonb) WHERE id=$1")
        .bind(machine).bind(json!([port])).execute(&mut *conn).await.unwrap();
    access::insert_entry(
        &mut conn,
        &access::NewEntry {
            id: entry,
            page: &access::PageKey::new("environment", environment, "live"),
            profile: "any",
            permission: "ports.view",
            holder_kind: "user",
            holder_id: Some(viewer.id()),
            holder_role: None,
            options: &json!({}),
            expires_at: None,
            note: "",
            source: "page",
            created_by: Some(manager.id()),
            at: world.now(),
        },
    )
    .await
    .unwrap();
    drop(conn);
    for (reach, expected) in [("roots", false), ("machine", true), ("roots", false)] {
        let mut conn = world.conn().await;
        assert!(
            access::rewrite_entry(&mut conn, entry, &json!({"reach":reach}), None, "", None)
                .await
                .unwrap()
        );
        access::bump_epoch(&mut conn).await.unwrap();
        drop(conn);
        let listing = world
            .app
            .call(EnvironmentPortList, &[&environment])
            .token(&viewer.token)
            .query(&EnvironmentPortListQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(
            listing
                .targets
                .iter()
                .any(|target| target.port == port && target.named_loopback),
            expected
        );
        let admission = world
            .app
            .call(EnvironmentPortUseCreate, &[&environment, &port.to_string()])
            .token(&viewer.token)
            .body(&EnvironmentPortUseCreateBody {
                transport: PortTransport::Tcp,
                opening_id: None,
            })
            .send()
            .await;
        assert_eq!(
            admission.status.is_success(),
            expected,
            "personal admission with {reach} reach"
        );
        assert!(
            listing.snapshot.listeners.is_empty(),
            "a declaration does not invent a live socket observation"
        );
    }
}
