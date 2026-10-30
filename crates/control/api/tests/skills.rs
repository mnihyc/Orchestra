//! Skills over HTTP: made from an uploaded archive that is checked and kept as a version, read with their versions and files, refused with the reason when broken, given further versions only by whoever manages them, switched off and deleted; a project may be a skill's home for those who work in it.
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
use orch_api_contract::common::{ErrorCode, Listed};
use orch_api_contract::skills::*;
use orch_api_contract::threads::{
    UploadSessionComplete, UploadSessionCreate, UploadSessionCreateBody, UploadSessionPut,
    UploadSessionPutQuery,
};
use orch_types::{Hash, Id};
use places::{Who, make_project};
use resources::{Company, company};
use support::World;

/// A tar.gz of a skill named `name` in a folder of its own, as a person would pack one.
fn archive(name: &str, body: &str) -> Vec<u8> {
    let skill = format!(
        "---\nname: {name}\ndescription: Turns notes into reports. Use it when asked for one.\n---\n{body}\n"
    );
    let mut builder = tar::Builder::new(Vec::new());
    for (path, data, mode) in [
        (format!("{name}/SKILL.md"), skill.as_bytes(), 0o644),
        (
            format!("{name}/scripts/report.sh"),
            &b"#!/bin/sh\necho report\n"[..],
            0o755,
        ),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(mode);
        builder.append_data(&mut header, path, data).unwrap();
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&builder.into_inner().unwrap()).unwrap();
    encoder.finish().unwrap()
}

async fn upload(world: &World, by: &Who, name: &str, bytes: &[u8]) -> Hash {
    let app = &world.app;
    let session = app
        .call(UploadSessionCreate, &[])
        .token(&by.token)
        .body(&UploadSessionCreateBody {
            size: i64::try_from(bytes.len()).unwrap(),
            name: name.to_owned(),
            media_type: None,
        })
        .send()
        .await
        .ok();
    app.call(UploadSessionPut, &[&session.upload_id])
        .token(&by.token)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(bytes)
        .send()
        .await
        .ok();
    app.call(UploadSessionComplete, &[&session.upload_id])
        .token(&by.token)
        .send()
        .await
        .ok()
        .blob
}

fn uploaded(blob: Hash, name: &str) -> SkillSource {
    SkillSource::Upload {
        blob,
        name: name.to_owned(),
        folder: None,
    }
}

async fn make(world: &World, by: &Who, home: Id, name: &str) -> Skill {
    let blob = upload(
        world,
        by,
        &format!("{name}.tar.gz"),
        &archive(name, "Steps."),
    )
    .await;
    world
        .app
        .call(SkillCreate, &[])
        .token(&by.token)
        .body(&SkillCreateBody {
            home_id: home,
            from: uploaded(blob, &format!("{name}.tar.gz")),
            access: Some(NewResourceAccess::OnlyMe),
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_member_makes_a_skill_from_an_uploaded_archive_and_reads_its_version_and_files() {
    let world = World::new().await;
    let Company { viewer, acme, .. } = company(&world).await;
    let skill = make(&world, &viewer, acme, "report-writer").await;
    assert_eq!(skill.name, "report-writer");
    assert_eq!(skill.versions, 1);
    assert!(skill.enabled);
    assert_eq!(skill.enabled_places, Some(0));
    assert!(
        skill
            .access
            .permissions
            .contains(&"skill.manage".to_owned())
    );
    let latest = skill.latest.clone().unwrap();
    assert_eq!(latest.number, 1);
    assert_eq!(latest.file_count, 2);
    assert!(
        matches!(latest.origin, SkillOrigin::Upload { ref name, .. } if name == "report-writer.tar.gz")
    );
    assert!(
        skill.origin.is_none(),
        "an upload is not taken again by an update"
    );

    let read = world
        .app
        .call(SkillGet, &[&skill.id])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(read.id, skill.id);
    let listed = world
        .app
        .call(SkillList, &[])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    assert!(
        listed
            .iter()
            .any(|row| matches!(row, Listed::Visible(row) if row.content.id == skill.id))
    );

    let page = world
        .app
        .call(SkillVersionList, &[&skill.id])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    let version = world
        .app
        .call(SkillVersionGet, &[&skill.id, &latest.id])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    let paths: Vec<&str> = version
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, ["SKILL.md", "scripts/report.sh"]);
    assert!(version.files[1].executable);
    let file = world
        .app
        .call(SkillVersionFileGet, &[&skill.id, &latest.id])
        .token(&viewer.token)
        .query(&SkillFileQuery {
            path: "scripts/report.sh".to_owned(),
        })
        .send_bytes()
        .await;
    assert!(file.status.is_success());
    assert_eq!(&file.raw[..], b"#!/bin/sh\necho report\n");
}

#[tokio::test]
async fn a_skill_card_counts_explicit_enabled_places_without_counting_off_entries() {
    let world = World::new().await;
    let c = company(&world).await;
    let skill = make(&world, &c.viewer, c.acme, "counted-skill").await;
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "agents.skills",
            serde_json::json!({(skill.id.to_string()): "latest"}),
        )
        .await;
    let enabled = world
        .app
        .call(SkillGet, &[&skill.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(enabled.enabled_places, Some(1));
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "agents.skills",
            serde_json::json!({(skill.id.to_string()): "off"}),
        )
        .await;
    let disabled = world
        .app
        .call(SkillGet, &[&skill.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(disabled.enabled_places, Some(0));
    world
        .set_at(
            orch_settings::Target::Node(c.stranger.person.personal_org),
            "agents.skills",
            serde_json::json!({(skill.id.to_string()): "latest"}),
        )
        .await;
    let elsewhere = world
        .app
        .call(SkillGet, &[&skill.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(elsewhere.enabled_places, Some(1));
    sqlx::query("UPDATE nodes SET deleted_at=created_at WHERE id=$1")
        .bind(c.stranger.person.personal_org)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let removed = world
        .app
        .call(SkillGet, &[&skill.id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(
        removed.enabled_places,
        Some(0),
        "removed places no longer enable a skill"
    );
}

#[tokio::test]
async fn a_broken_skill_is_refused_with_the_reason_and_nothing_is_kept() {
    let world = World::new().await;
    let Company { viewer, acme, .. } = company(&world).await;
    let mut builder = tar::Builder::new(Vec::new());
    let text = b"---\ndescription: No name here.\n---\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(text.len() as u64);
    header.set_mode(0o644);
    builder
        .append_data(&mut header, "SKILL.md", &text[..])
        .unwrap();
    let blob = upload(
        &world,
        &viewer,
        "broken.tar",
        &builder.into_inner().unwrap(),
    )
    .await;
    let problem = world
        .app
        .call(SkillCreate, &[])
        .token(&viewer.token)
        .body(&SkillCreateBody {
            home_id: acme,
            from: uploaded(blob, "broken.tar"),
            access: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::RequestValueRefused);
    // The reason stands at its place in the body; the sentence of the code stays general.
    let why = problem.extra["errors"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(why.contains("gives no name"), "{why}");
    assert_eq!(problem.extra["errors"][0]["path"], "from");
    assert_eq!(problem.extra["about"], serde_json::json!(["SKILL.md"]));
    let listed = world
        .app
        .call(SkillList, &[])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    assert!(listed.is_empty());
}

#[tokio::test]
async fn an_archive_somebody_else_uploaded_cannot_be_named() {
    let world = World::new().await;
    let Company {
        viewer,
        admin,
        acme,
        ..
    } = company(&world).await;
    let blob = upload(&world, &viewer, "mine.tar.gz", &archive("mine", "Mine.")).await;
    let problem = world
        .app
        .call(SkillCreate, &[])
        .token(&admin.token)
        .body(&SkillCreateBody {
            home_id: acme,
            from: uploaded(blob, "mine.tar.gz"),
            access: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn somebody_from_outside_makes_no_skill_in_an_org() {
    let world = World::new().await;
    let Company { stranger, acme, .. } = company(&world).await;
    let blob = upload(&world, &stranger, "x.tar.gz", &archive("outside", "x")).await;
    let reply = world
        .app
        .call(SkillCreate, &[])
        .token(&stranger.token)
        .body(&SkillCreateBody {
            home_id: acme,
            from: uploaded(blob, "x.tar.gz"),
            access: None,
        })
        .send()
        .await;
    assert!(!reply.status.is_success(), "{:?}", reply.status);
}

#[tokio::test]
async fn whoever_manages_a_skill_adds_versions_that_keep_its_name() {
    let world = World::new().await;
    let Company {
        viewer, boss, acme, ..
    } = company(&world).await;
    let skill = make(&world, &viewer, acme, "keeper").await;
    let next = upload(
        &world,
        &viewer,
        "keeper.tar.gz",
        &archive("keeper", "Better."),
    )
    .await;
    let version = world
        .app
        .call(SkillVersionCreate, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillVersionCreateBody {
            from: uploaded(next, "keeper.tar.gz"),
        })
        .send()
        .await
        .ok();
    assert_eq!(version.version.number, 2);
    let renamed = upload(&world, &viewer, "other.tar.gz", &archive("other", "x")).await;
    let problem = world
        .app
        .call(SkillVersionCreate, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillVersionCreateBody {
            from: uploaded(renamed, "other.tar.gz"),
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::RequestValueRefused);
    let why = problem.extra["errors"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(why.contains("keeps its name"), "{why}");

    // The page names its maker alone: another member, the org's first owner too, adds nothing.
    let theirs = upload(&world, &boss, "keeper.tar.gz", &archive("keeper", "Mine.")).await;
    let refused = world
        .app
        .call(SkillVersionCreate, &[&skill.id])
        .token(&boss.token)
        .body(&SkillVersionCreateBody {
            from: uploaded(theirs, "keeper.tar.gz"),
        })
        .send()
        .await;
    assert!(!refused.status.is_success(), "{:?}", refused.status);
}

#[tokio::test]
async fn a_skill_is_switched_off_and_deleted_by_whoever_manages_it() {
    let world = World::new().await;
    let Company { viewer, acme, .. } = company(&world).await;
    let skill = make(&world, &viewer, acme, "short-lived").await;
    let off = world
        .app
        .call(SkillUpdate, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillUpdateBody {
            enabled: Some(false),
            expected_revision: skill.revision,
        })
        .send()
        .await
        .ok();
    assert!(!off.enabled);
    let stale = world
        .app
        .call(SkillUpdate, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillUpdateBody {
            enabled: Some(true),
            expected_revision: skill.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    world
        .app
        .call(SkillDelete, &[&skill.id])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    let gone = world
        .app
        .call(SkillGet, &[&skill.id])
        .token(&viewer.token)
        .send()
        .await;
    assert!(!gone.status.is_success());
    let audit = world.audit().await;
    for action in ["skill.created", "skill.disabled", "skill.deleted"] {
        assert!(
            audit.iter().any(|line| line.contains(action)),
            "{action}: {audit:?}"
        );
    }
}

#[tokio::test]
async fn a_project_is_a_skills_home_for_those_who_work_in_it() {
    let world = World::new().await;
    let Company {
        viewer, boss, acme, ..
    } = company(&world).await;
    let project = make_project(&world, &boss, acme, "Reports").await.base.id;
    let blob = upload(&world, &viewer, "p.tar.gz", &archive("project-skill", "x")).await;
    let refused = world
        .app
        .call(SkillCreate, &[])
        .token(&viewer.token)
        .body(&SkillCreateBody {
            home_id: project,
            from: uploaded(blob, "p.tar.gz"),
            access: None,
        })
        .send()
        .await;
    assert!(
        !refused.status.is_success(),
        "a viewer of the org does not work in its project: {:?}",
        refused.status
    );
    let made = make(&world, &boss, project, "project-skill").await;
    assert_eq!(made.home.id, project);
}

async fn draft(world: &World, by: &Who, skill: Id) -> SkillDraft {
    world
        .app
        .call(SkillDraftGet, &[&skill])
        .token(&by.token)
        .send()
        .await
        .ok()
}

async fn put_text(
    world: &World,
    by: &Who,
    skill: Id,
    path: &str,
    text: &str,
    executable: Option<bool>,
) -> SkillDraft {
    let revision = draft(world, by, skill).await.info.revision;
    world
        .app
        .call(SkillDraftFilePut, &[&skill])
        .token(&by.token)
        .body(&SkillDraftFilePutBody {
            path: path.to_owned(),
            text: Some(text.to_owned()),
            blob: None,
            executable,
            expected_revision: revision,
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_skill_begun_in_orch_is_a_draft_until_its_managers_publish_it() {
    let world = World::new().await;
    let Company { viewer, acme, .. } = company(&world).await;
    let skill = world
        .app
        .call(SkillCreate, &[])
        .token(&viewer.token)
        .body(&SkillCreateBody {
            home_id: acme,
            from: SkillSource::New {
                name: "fresh-start".to_owned(),
                description: "Says \"hello\" when asked: a test.".to_owned(),
            },
            access: Some(NewResourceAccess::OnlyMe),
        })
        .send()
        .await
        .ok();
    assert!(skill.latest.is_none());
    assert_eq!(skill.versions, 0);
    assert!(skill.draft.is_some());
    let begun = draft(&world, &viewer, skill.id).await;
    assert_eq!(begun.files.len(), 1);
    assert_eq!(begun.files[0].path, "SKILL.md");
    let edited = put_text(
        &world,
        &viewer,
        skill.id,
        "scripts/hello.sh",
        "#!/bin/sh\necho hello\n",
        Some(true),
    )
    .await;
    assert_eq!(edited.files.len(), 2);
    assert!(edited.files[1].executable);
    let text = world
        .app
        .call(SkillDraftFileGet, &[&skill.id])
        .token(&viewer.token)
        .query(&SkillFileQuery {
            path: "scripts/hello.sh".to_owned(),
        })
        .send_bytes()
        .await;
    assert_eq!(&text.raw[..], b"#!/bin/sh\necho hello\n");
    let published = world
        .app
        .call(SkillDraftPublish, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillDraftPublishBody {
            expected_revision: edited.info.revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(published.version.number, 1);
    assert_eq!(
        published.version.description,
        "Says \"hello\" when asked: a test."
    );
    assert!(matches!(
        published.version.origin,
        SkillOrigin::Draft { based_on: None }
    ));
    let paths: Vec<&str> = published
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, ["SKILL.md", "scripts/hello.sh"]);
    let after = world
        .app
        .call(SkillGet, &[&skill.id])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    assert!(after.draft.is_none(), "publishing takes the draft's place");
    assert_eq!(after.latest.unwrap().number, 1);
}

#[tokio::test]
async fn a_draft_begins_from_a_version_and_a_broken_one_is_kept_and_refused_with_the_reason() {
    let world = World::new().await;
    let Company { viewer, acme, .. } = company(&world).await;
    let skill = make(&world, &viewer, acme, "drafted").await;
    let begun = world
        .app
        .call(SkillDraftCreate, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillDraftCreateBody::default())
        .send()
        .await
        .ok();
    assert_eq!(begun.info.based_on, Some(skill.latest.as_ref().unwrap().id));
    assert_eq!(begun.files.len(), 2);
    let again = world
        .app
        .call(SkillDraftCreate, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillDraftCreateBody::default())
        .send()
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::ConflictExists);

    let broken = put_text(
        &world,
        &viewer,
        skill.id,
        "SKILL.md",
        "---\nname: drafted\n---\n",
        None,
    )
    .await;
    let refused = world
        .app
        .call(SkillDraftPublish, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillDraftPublishBody {
            expected_revision: broken.info.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestValueRefused);
    let why = refused.extra["errors"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(why.contains("description"), "{why}");
    assert_eq!(
        draft(&world, &viewer, skill.id).await.info.revision,
        broken.info.revision,
        "a refused publish keeps the draft as it was"
    );

    let fixed = put_text(
        &world,
        &viewer,
        skill.id,
        "SKILL.md",
        "---\nname: drafted\ndescription: Better now.\n---\n",
        None,
    )
    .await;
    let stale = world
        .app
        .call(SkillDraftFilePut, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillDraftFilePutBody {
            path: "notes.md".to_owned(),
            text: Some("late".to_owned()),
            blob: None,
            executable: None,
            expected_revision: broken.info.revision,
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let published = world
        .app
        .call(SkillDraftPublish, &[&skill.id])
        .token(&viewer.token)
        .body(&SkillDraftPublishBody {
            expected_revision: fixed.info.revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(published.version.number, 2);
    assert_eq!(published.version.description, "Better now.");
}

#[tokio::test]
async fn a_new_skill_follows_the_format_of_names() {
    let world = World::new().await;
    let Company { viewer, acme, .. } = company(&world).await;
    let problem = world
        .app
        .call(SkillCreate, &[])
        .token(&viewer.token)
        .body(&SkillCreateBody {
            home_id: acme,
            from: SkillSource::New {
                name: "Not--Valid".to_owned(),
                description: "x".to_owned(),
            },
            access: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn an_uploaded_skill_has_no_origin_to_be_taken_from_again() {
    let world = World::new().await;
    let Company { viewer, acme, .. } = company(&world).await;
    let skill = make(&world, &viewer, acme, "uploaded-once").await;
    let problem = world
        .app
        .call(SkillReimport, &[&skill.id])
        .token(&viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::RequestValueRefused);
}

/// A host in `org`, first owned by `owner`, whose node has a session, with its machine.
async fn host_in(world: &World, owner: &Who, org: Id) -> (Id, Id) {
    let mut conn = world.conn().await;
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(owner.id()),
        Some(org),
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
    resources::online(world, machine).await;
    (host, machine)
}

#[tokio::test]
async fn a_repository_is_fetched_by_a_host_the_caller_may_use_and_taken_again_from_there() {
    let world = World::new().await;
    let Company {
        viewer, boss, acme, ..
    } = company(&world).await;
    let (host, machine) = host_in(&world, &boss, acme).await;
    let source = SkillSource::Git {
        host_id: host,
        repository: "https://example.org/skills.git".to_owned(),
        reference: "main".to_owned(),
        folder: None,
    };
    let refused = world
        .app
        .call(SkillCreate, &[])
        .token(&viewer.token)
        .body(&SkillCreateBody {
            home_id: acme,
            from: source.clone(),
            access: None,
        })
        .send()
        .await;
    assert!(
        !refused.status.is_success(),
        "a host the caller may not use fetches nothing for them: {:?}",
        refused.status
    );

    resources::name_person(&world, "host", host, "host.use", viewer.id()).await;
    let bytes = archive("from-git", "Fetched.");
    // What the host would put up: the archive is in the store under its hash.
    let blob = upload(&world, &viewer, "fetched.tar.gz", &bytes).await;
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let fetched =
        serde_json::json!({ "archive": { "hash": blob, "size": bytes.len() }, "commit": commit });
    let answering = resources::answer(&world, machine, fetched.clone());
    let skill = world
        .app
        .call(SkillCreate, &[])
        .token(&viewer.token)
        .body(&SkillCreateBody {
            home_id: acme,
            from: source,
            access: Some(NewResourceAccess::OnlyMe),
        })
        .send()
        .await
        .ok();
    answering.await.unwrap();
    assert_eq!(skill.name, "from-git");
    assert!(matches!(
        skill.origin,
        Some(SkillOrigin::Git { commit: ref found, ref reference, .. }) if *found == commit && reference == "main"
    ));

    let answering = resources::answer(&world, machine, fetched);
    let again = world
        .app
        .call(SkillReimport, &[&skill.id])
        .token(&viewer.token)
        .send()
        .await
        .ok();
    answering.await.unwrap();
    assert!(!again.made, "the same files are no new version");
    assert_eq!(again.version.number, 1);
}
