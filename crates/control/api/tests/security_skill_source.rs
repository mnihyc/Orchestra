//! Reading a skill must not reveal the credential used to import its archive.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use std::io::Write;

use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::skills::{
    SkillCreate, SkillCreateBody, SkillGet, SkillReimport, SkillSource, SkillVersionCreate,
    SkillVersionCreateBody,
};
use orch_api_contract::threads::{
    UploadSessionComplete, UploadSessionCreate, UploadSessionCreateBody, UploadSessionPut,
    UploadSessionPutQuery,
};
use orch_api_core::routes::resources::sealing::{binding, install_master_key};
use orch_db::secrets::MasterKey;
use orch_types::{Hash, Id, Json};
use places::Who;
use resources::{Company, company};
use support::World;

fn archive() -> Vec<u8> {
    let skill = b"---\nname: from-url\ndescription: Reads a sample archive.\n---\n# Sample\n";
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(skill.len() as u64);
    header.set_mode(0o644);
    builder
        .append_data(&mut header, "from-url/SKILL.md", skill.as_slice())
        .unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&builder.into_inner().unwrap()).unwrap();
    encoder.finish().unwrap()
}

async fn upload(world: &World, by: &Who, bytes: &[u8]) -> Hash {
    let session = world
        .app
        .call(UploadSessionCreate, &[])
        .token(&by.token)
        .body(&UploadSessionCreateBody {
            size: i64::try_from(bytes.len()).unwrap(),
            name: "source.tar.gz".into(),
            media_type: None,
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(UploadSessionPut, &[&session.upload_id])
        .token(&by.token)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(bytes)
        .send()
        .await
        .ok();
    world
        .app
        .call(UploadSessionComplete, &[&session.upload_id])
        .token(&by.token)
        .send()
        .await
        .ok()
        .blob
}

#[tokio::test]
async fn a_skill_reader_is_not_given_the_secret_query_used_to_import_its_archive() {
    install_master_key(MasterKey::new([7; 32]));
    let world = World::new().await;
    world
        .set(
            "outbound.allowed_private_hosts",
            serde_json::json!(["skill.internal"]),
        )
        .await;
    let Company {
        viewer,
        admin,
        boss,
        acme,
        ..
    } = company(&world).await;
    let mut conn = world.conn().await;
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(boss.id()),
        Some(acme),
        "builder",
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    resources::online(&world, machine).await;
    resources::name_person(&world, "host", host, "host.use", viewer.id()).await;
    let bytes = archive();
    let blob = upload(&world, &viewer, &bytes).await;
    let answering = resources::answer(
        &world,
        machine,
        serde_json::json!({ "archive": { "hash": blob, "size": bytes.len() }, "commit": null }),
    );
    let secret = "archive-download-test-secret";
    let skill = world
        .app
        .call(SkillCreate, &[])
        .token(&viewer.token)
        .body(&SkillCreateBody {
            home_id: acme,
            from: SkillSource::Url {
                host_id: host,
                url: format!("https://archive.example.invalid/skill.tar.gz?token={secret}"),
                folder: None,
            },
            access: Some(NewResourceAccess::OnlyMe),
        })
        .send()
        .await
        .ok();
    answering.await.unwrap();
    resources::name_person(&world, "skill", skill.id, "skill.view", admin.id()).await;
    let reply = world
        .app
        .call(SkillGet, &[&skill.id])
        .token(&admin.token)
        .send()
        .await;
    assert!(
        reply.status.is_success(),
        "the named skill reader may open it"
    );
    assert!(
        !String::from_utf8_lossy(&reply.raw).contains(secret),
        "reading a skill must not disclose the credential used to fetch its source"
    );
    assert!(String::from_utf8_lossy(&reply.raw).contains("?…"));
    let mut conn = world.conn().await;
    let (origin, pointer): (Json, Option<Id>) =
        sqlx::query_as("SELECT origin,source_url_secret_id FROM skills WHERE id=$1")
            .bind(skill.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let pointer = pointer.expect("a signed address has a sealed source");
    assert!(!origin.to_string().contains(secret));
    let versions: Vec<Json> =
        sqlx::query_scalar("SELECT origin FROM skill_versions WHERE skill_id=$1")
            .bind(skill.id)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert!(
        versions
            .iter()
            .all(|origin| !origin.to_string().contains(secret))
    );
    let (owner_kind, owner_id, name, sealed): (String, Id, String, Vec<u8>) =
        sqlx::query_as("SELECT owner_kind,owner_id,name,sealed FROM secrets WHERE id=$1")
            .bind(pointer)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!((owner_kind.as_str(), owner_id), ("skill", skill.id));
    assert!(!String::from_utf8_lossy(&sealed).contains(secret));
    let original = format!("https://archive.example.invalid/skill.tar.gz?token={secret}");
    assert_eq!(
        MasterKey::new([7; 32])
            .open(&sealed, &binding("skill", skill.id, &name))
            .unwrap(),
        original.as_bytes()
    );
    let commands: Vec<Json> = sqlx::query_scalar(
        "SELECT body FROM commands WHERE kind='node.skill.fetch' AND machine_id=$1",
    )
    .bind(machine)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|body| {
        !body.to_string().contains(secret)
            && body["source"]["url"]["$secret"] == pointer.to_string()
            && body["allowed_private_hosts"] == serde_json::json!(["skill.internal"])
    }));
    let audit: Vec<Json> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE object_kind='skill' AND object_id=$1",
    )
    .bind(skill.id)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert!(
        audit
            .iter()
            .all(|details| !details.to_string().contains(secret))
    );
    let plans: Vec<Json> = sqlx::query_scalar("SELECT plan FROM jobs WHERE plan IS NOT NULL")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert!(plans.iter().all(|plan| !plan.to_string().contains(secret)));
    drop(conn);

    let again = resources::answer(
        &world,
        machine,
        serde_json::json!({ "archive": { "hash": blob, "size": bytes.len() }, "commit": null }),
    );
    let reimported = world
        .app
        .call(SkillReimport, &[&skill.id])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    again.await.unwrap();
    assert!(!reimported.made);
    let newer = resources::answer(
        &world,
        machine,
        serde_json::json!({ "archive": { "hash": blob, "size": bytes.len() }, "commit": null }),
    );
    world
        .app
        .call(SkillVersionCreate, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillVersionCreateBody {
            from: SkillSource::Url {
                host_id: host,
                url: original.clone(),
                folder: None,
            },
        })
        .send()
        .await
        .ok();
    newer.await.unwrap();
    let mut conn = world.conn().await;
    let body_and_audit: Vec<String> = sqlx::query_scalar(
        "SELECT body::text FROM commands WHERE kind='node.skill.fetch' AND machine_id=$1",
    )
    .bind(machine)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert!(body_and_audit.iter().all(|body| !body.contains(secret)));
    let details: Vec<String> = sqlx::query_scalar(
        "SELECT details::text FROM audit_log WHERE object_kind='skill' AND object_id=$1",
    )
    .bind(skill.id)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert!(details.iter().all(|detail| !detail.contains(secret)));

    // A row left by an older release is sealed and redacted on its next read.
    sqlx::query(
        "UPDATE skills SET origin=jsonb_set(origin,'{url}',to_jsonb($2::text)),
                           source_url_secret_id=NULL WHERE id=$1",
    )
    .bind(skill.id)
    .bind(&original)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE skill_versions SET origin=jsonb_set(origin,'{url}',to_jsonb($2::text))
          WHERE skill_id=$1 AND origin->>'kind'='url'",
    )
    .bind(skill.id)
    .bind(&original)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    let migrated = world
        .app
        .call(SkillGet, &[&skill.id])
        .token(&admin.token)
        .send()
        .await
        .ok();
    assert!(!serde_json::to_string(&migrated).unwrap().contains(secret));
    let mut conn = world.conn().await;
    let (origin, pointer): (Json, Option<Id>) =
        sqlx::query_as("SELECT origin,source_url_secret_id FROM skills WHERE id=$1")
            .bind(skill.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(!origin.to_string().contains(secret));
    assert!(pointer.is_some());
    let versions: Vec<String> =
        sqlx::query_scalar("SELECT origin::text FROM skill_versions WHERE skill_id=$1")
            .bind(skill.id)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert!(versions.iter().all(|origin| !origin.contains(secret)));
}
