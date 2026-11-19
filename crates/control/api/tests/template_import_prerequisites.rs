//! Defaulted archive inputs are actual native sources, before a proposal asks the node for IO.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod resources;
mod support;
use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_api_contract::resources::{TemplateImport, TemplateImportBody, UnitKind};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures;
use orch_types::{Hash, Id, Time};
use serde_json::json;
use support::World;

#[tokio::test]
async fn a_driver_default_cannot_smuggle_an_archive_past_the_native_upload_custody_check() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let hash = Hash::of(b"an archive the caller never uploaded");
    let mut conn = world.conn().await;
    let host = access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(company.boss.id()),
        Some(company.acme),
        "Import host",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id: host,
        },
        company.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=true WHERE id=$1")
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1").bind(host).bind(json!({"memory":{
        "name":"memory","title":"Import host","version":"1.0.0","spec_schemas":{"environment":{},"provider":{}},
        "imports":[{"format":"archive","title":"An archive","fields":[{"name":"blob","label":"Archive","type":"text","required":true,"default":hash}]},
            {"format":"plain","title":"An inline recipe","fields":[]}]
    }})).execute(&mut *conn).await.unwrap();
    drop(conn);
    let answer = resources::answer(&world, machine, json!({"spec":{},"notes":[]}));
    let ordinary = world
        .app
        .call(TemplateImport, &[])
        .token(&company.boss.token)
        .body(&TemplateImportBody {
            driver: "memory".into(),
            kind: Some(UnitKind::Environment),
            format: "plain".into(),
            home_id: Some(company.acme),
            values: Default::default(),
        })
        .send()
        .await
        .ok();
    assert_eq!(
        ordinary.spec,
        json!({}),
        "the same ordinary creation/Host selection and native proposal work"
    );
    answer.await.unwrap();
    let app = world.app.clone();
    let token = company.boss.token.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(TemplateImport, &[])
            .token(&token)
            .body(&TemplateImportBody {
                driver: "memory".into(),
                kind: Some(UnitKind::Environment),
                format: "archive".into(),
                home_id: Some(company.acme),
                values: Default::default(),
            });
        call.send_bytes().await
    });
    // A scripted native reply makes the old unchecked path finish too. It is not
    // an authorization callback or a replacement for the source/custody gate.
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        while !request.is_finished() {
            sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE machine_id=$1 AND kind='node.template.import' AND state IN ('queued','sent','accepted')")
                .bind(machine).bind(json!({"spec":{},"notes":[]})).execute(&world.db.pool).await.unwrap();
            tokio::task::yield_now().await;
        }
    }).await.expect("the native proposal or actual custody refusal completes");
    let reply = request.await.unwrap();
    assert_eq!(
        reply.status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='node.template.import' AND body->>'format'='archive'",
    )
    .bind(machine)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "the actual default archive is checked before the first native queue effect"
    );
}
