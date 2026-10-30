//! Template imports return a driver's proposal and notes only after checking creation, host and upload authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    TemplateImport, TemplateImportBody, TemplateImportNoteLevel, UnitKind,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_types::{Field, FieldType, Id, Time};
use resources::{Company, answer, company};
use serde_json::json;
use support::World;

async fn host(world: &World, c: &Company) -> Id {
    let mut conn = world.conn().await;
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(c.admin.id()),
        Some(c.acme),
        "import-host",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id: host,
        },
        c.admin.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let fields = [Field::new("file", "File text", FieldType::SecretText)];
    let archive = [Field::new("blob", "Archive", FieldType::Text)];
    let descriptor = json!({"name":"import-driver","version":"1.0.0","title":"Import driver", "spec_schemas":{"environment":{"type":"object","properties":{"image":{"type":"string"}},"required":["image"],"additionalProperties":false},"provider":{"type":"object","properties":{"image":{"type":"string"}},"required":["image"],"additionalProperties":false}}, "imports":[{"format":"file","title":"File","fields":fields},{"format":"archive","title":"Archive","fields":archive}]});
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_build_object('import-driver',$2::jsonb) WHERE id=$1",
    )
    .bind(host)
    .bind(descriptor)
    .execute(&mut *conn)
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
    machine
}

fn body(home: Id, kind: UnitKind) -> TemplateImportBody {
    TemplateImportBody {
        home_id: Some(home),
        driver: "import-driver".into(),
        kind: Some(kind),
        format: "file".into(),
        values: [("file".into(), "An existing file".into())].into(),
    }
}

#[tokio::test]
async fn both_pools_receive_exact_driver_notes_and_no_template_or_unit_is_created() {
    let world = World::new().await;
    let c = company(&world).await;
    let machine = host(&world, &c).await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM env_templates")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    for kind in [UnitKind::Environment, UnitKind::Provider] {
        let replied = answer(
            &world,
            machine,
            json!({"spec":{"image":"source-image"},"notes":[{"level":"info","text":"Read the source image"},{"level":"warning","text":"The image's entrypoint will not run"},{"level":"blocker","text":"No target host offers this architecture"}]}),
        );
        let proposal = world
            .app
            .call(TemplateImport, &[])
            .token(&c.admin.token)
            .body(&body(c.acme, kind))
            .send()
            .await
            .ok();
        replied.await.unwrap();
        assert_eq!(proposal.spec, json!({"image":"source-image"}));
        assert_eq!(proposal.notes[1].level, TemplateImportNoteLevel::Warning);
        assert_eq!(proposal.notes[2].level, TemplateImportNoteLevel::Blocker);
        assert_eq!(
            proposal.notes[1].text,
            "The image's entrypoint will not run"
        );
    }
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM env_templates")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, before, "the proposal only fills a form");
    let commands: Vec<String> = sqlx::query_scalar("SELECT body->>'kind' FROM commands WHERE kind='node.template.import' ORDER BY created_at,id").fetch_all(&world.db.pool).await.unwrap();
    assert_eq!(commands.len(), 2);
    assert!(
        commands.contains(&"environment".to_owned()) && commands.contains(&"provider".to_owned())
    );
    let created: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.runtime.create'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(created, 0);
}

#[tokio::test]
async fn foreign_creation_unknown_fields_and_unowned_archives_never_reach_the_node() {
    let world = World::new().await;
    let c = company(&world).await;
    host(&world, &c).await;
    assert_eq!(
        world
            .app
            .call(TemplateImport, &[])
            .token(&c.stranger.token)
            .body(&body(c.acme, UnitKind::Environment))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let mut unknown = body(c.acme, UnitKind::Environment);
    unknown.values.insert("privileged".into(), "true".into());
    let refused = world
        .app
        .call(TemplateImport, &[])
        .token(&c.admin.token)
        .body(&unknown)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(
        refused.extra["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fault| fault["path"] == "values.privileged")
    );
    let mut archive = body(c.acme, UnitKind::Environment);
    archive.format = "archive".into();
    archive.values = [(
        "blob".into(),
        orch_types::Hash::of(b"somebody else's bytes").to_string(),
    )]
    .into();
    assert_eq!(
        world
            .app
            .call(TemplateImport, &[])
            .token(&c.admin.token)
            .body(&archive)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    world
        .set("templates.restricted_drivers", json!(["import-driver"]))
        .await;
    world
        .app
        .call(TemplateImport, &[])
        .token(&c.admin.token)
        .body(&body(c.acme, UnitKind::Environment))
        .send()
        .await
        .problem();
    let called: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.template.import'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        called, 0,
        "refusals are made before the host sees the source text"
    );
}

#[tokio::test]
async fn an_existing_archive_is_still_private_until_the_caller_has_a_completed_upload_reference() {
    let world = World::new().await;
    let c = company(&world).await;
    let machine = host(&world, &c).await;
    let content = bytes::Bytes::from_static(b"archive bytes");
    let length = content.len() as i64;
    let meta = world
        .app
        .app
        .blobs
        .store_stream(
            futures::stream::iter([Ok::<_, &'static str>(content)]),
            length,
        )
        .await
        .unwrap();
    let hash = meta.hash;
    let mut conn = world.conn().await;
    world
        .app
        .app
        .blobs
        .register(&mut conn, meta, world.now())
        .await
        .unwrap();
    orch_db::queries::uploads::remember(&mut conn, c.viewer.id(), &hash, world.now())
        .await
        .unwrap();
    drop(conn);
    let mut archive = body(c.acme, UnitKind::Environment);
    archive.format = "archive".into();
    archive.values = [("blob".into(), hash.to_string())].into();
    assert_eq!(
        world
            .app
            .call(TemplateImport, &[])
            .token(&c.admin.token)
            .body(&archive)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let mut conn = world.conn().await;
    orch_db::queries::uploads::remember(&mut conn, c.admin.id(), &hash, world.now())
        .await
        .unwrap();
    drop(conn);
    let replied = answer(
        &world,
        machine,
        json!({"spec":{"image":"from-archive"},"notes":[{"level":"info","text":"Read the completed archive"}]}),
    );
    let proposal = world
        .app
        .call(TemplateImport, &[])
        .token(&c.admin.token)
        .body(&archive)
        .send()
        .await
        .ok();
    replied.await.unwrap();
    assert_eq!(proposal.spec["image"], "from-archive");
}
