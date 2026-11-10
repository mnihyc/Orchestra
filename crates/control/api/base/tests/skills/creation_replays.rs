//! Completed native versions need their own current Read, never another source preparation.
use super::*;
use crate::thread_history_scope::{key, rule};
use http::StatusCode;
use orch_api_core::testing::RawReply;
use orch_types::new_id;
use serde_json::{Value, json};

fn checked(reply: RawReply, status: StatusCode) -> Value {
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn versions(world: &World, skill: Id) -> Vec<(Id, String)> {
    sqlx::query_as("SELECT id,xmin::text FROM skill_versions WHERE skill_id=$1 ORDER BY number")
        .bind(skill)
        .fetch_all(&world.db.pool)
        .await
        .unwrap()
}

async fn state(world: &World, skill: Id) -> (i64, Option<Id>, Option<Id>, Value) {
    sqlx::query_as(
        "SELECT revision,latest_version_id,source_url_secret_id,COALESCE(origin,'null'::jsonb) FROM skills WHERE id=$1",
    )
    .bind(skill)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn an_uploaded_version_replays_one_record_without_reopening_its_archive_or_borrowing_read() {
    let world = World::new().await;
    let c = company(&world).await;
    let skill = make(&world, &c.viewer, c.acme, "replayed-version").await;
    let original = versions(&world, skill.id).await;
    let uploaded = upload(
        &world,
        &c.viewer,
        "replayed-version.tar.gz",
        &archive("replayed-version", "The next version."),
    )
    .await;
    let body = SkillVersionCreateBody {
        from: super::uploaded(uploaded, "replayed-version.tar.gz"),
    };
    let writer = key(
        &world,
        c.viewer.id(),
        vec![rule(
            "SkillVersionCreate",
            "skill",
            skill.id,
            "self",
            "manage",
        )],
    )
    .await
    .unwrap();
    let header = new_id().to_string();
    let first = checked(
        world
            .app
            .call(SkillVersionCreate, &[&skill.id])
            .token(&writer.secret)
            .header("Idempotency-Key", &header)
            .body(&body)
            .send_bytes()
            .await,
        StatusCode::CREATED,
    );
    let version: Id = serde_json::from_value(first["version_id"].clone()).unwrap();
    assert_eq!(
        first,
        json!({"projection":"receipt","skill_id":skill.id,"version_id":version})
    );
    let accepted = versions(&world, skill.id).await;
    assert_eq!(accepted.len(), original.len() + 1);
    assert_eq!(accepted.last().unwrap().0, version);
    assert_eq!(&accepted[..original.len()], &original);
    let accepted_state = state(&world, skill.id).await;

    // Removing this isolated fixture's supplier makes a repeated source preparation fail.
    world.app.app.blobs.store().delete(&uploaded).await.unwrap();
    assert!(!world.app.app.blobs.store().exists(&uploaded).await.unwrap());
    for own in [true, false] {
        let reader = key(
            &world,
            c.viewer.id(),
            vec![rule(
                if own {
                    "SkillVersionCreate"
                } else {
                    "SkillVersionGet"
                },
                "skill",
                skill.id,
                "self",
                "read",
            )],
        )
        .await
        .unwrap();
        let replay = checked(
            world
                .app
                .call(SkillVersionCreate, &[&skill.id])
                .token(&reader.secret)
                .header("Idempotency-Key", &header)
                .body(&body)
                .send_bytes()
                .await,
            StatusCode::CREATED,
        );
        if own {
            let full: SkillVersion = serde_json::from_value(replay).unwrap();
            assert_eq!(full.version.id, version);
            assert_eq!(full.version.number, 2);
            assert_eq!(full.version.name, "replayed-version");
            assert_eq!(full.files.len(), 2);
            let fresh = world
                .app
                .call(SkillVersionCreate, &[&skill.id])
                .token(&reader.secret)
                .header("Idempotency-Key", &new_id().to_string())
                .body(&body)
                .send_bytes()
                .await;
            assert_eq!(fresh.status, StatusCode::FORBIDDEN);
        } else {
            assert_eq!(replay, first);
        }
        assert_eq!(versions(&world, skill.id).await, accepted);
        assert_eq!(state(&world, skill.id).await, accepted_state);
    }
}

#[tokio::test]
async fn a_published_version_replays_after_its_draft_is_consumed_without_publishing_again() {
    let world = World::new().await;
    let c = company(&world).await;
    let skill = make(&world, &c.viewer, c.acme, "replayed-publish").await;
    world
        .app
        .call(SkillDraftCreate, &[&skill.id])
        .token(&c.viewer.token)
        .body(&SkillDraftCreateBody::default())
        .send()
        .await
        .ok();
    let edited = put_text(
        &world,
        &c.viewer,
        skill.id,
        "scripts/published.sh",
        "#!/bin/sh\necho published\n",
        Some(true),
    )
    .await;
    let body = SkillDraftPublishBody {
        expected_revision: edited.info.revision,
    };
    let original = versions(&world, skill.id).await;
    let writer = key(
        &world,
        c.viewer.id(),
        vec![rule(
            "SkillDraftPublish",
            "skill",
            skill.id,
            "self",
            "manage",
        )],
    )
    .await
    .unwrap();
    let header = new_id().to_string();
    let first = checked(
        world
            .app
            .call(SkillDraftPublish, &[&skill.id])
            .token(&writer.secret)
            .header("Idempotency-Key", &header)
            .body(&body)
            .send_bytes()
            .await,
        StatusCode::CREATED,
    );
    let version: Id = serde_json::from_value(first["version_id"].clone()).unwrap();
    assert_eq!(
        first,
        json!({"projection":"receipt","skill_id":skill.id,"version_id":version})
    );
    let accepted = versions(&world, skill.id).await;
    assert_eq!(accepted.len(), original.len() + 1);
    assert_eq!(&accepted[..original.len()], &original);
    let accepted_state = state(&world, skill.id).await;
    for own in [true, false] {
        let reader = key(
            &world,
            c.viewer.id(),
            vec![rule(
                if own {
                    "SkillDraftPublish"
                } else {
                    "SkillVersionGet"
                },
                "skill",
                skill.id,
                "self",
                "read",
            )],
        )
        .await
        .unwrap();
        let replay = checked(
            world
                .app
                .call(SkillDraftPublish, &[&skill.id])
                .token(&reader.secret)
                .header("Idempotency-Key", &header)
                .body(&body)
                .send_bytes()
                .await,
            StatusCode::CREATED,
        );
        if own {
            let full: SkillVersion = serde_json::from_value(replay).unwrap();
            assert_eq!(full.version.id, version);
            assert_eq!(full.version.number, 2);
            assert!(
                matches!(full.version.origin, SkillOrigin::Draft { based_on: Some(id) } if id == original[0].0)
            );
            assert!(
                full.files
                    .iter()
                    .any(|file| file.path == "scripts/published.sh" && file.executable)
            );
            let fresh = world
                .app
                .call(SkillDraftPublish, &[&skill.id])
                .token(&reader.secret)
                .header("Idempotency-Key", &new_id().to_string())
                .body(&body)
                .send_bytes()
                .await;
            assert_eq!(fresh.status, StatusCode::FORBIDDEN);
        } else {
            assert_eq!(replay, first);
        }
        assert_eq!(versions(&world, skill.id).await, accepted);
        assert_eq!(state(&world, skill.id).await, accepted_state);
        let drafts: i64 = sqlx::query_scalar("SELECT count(*) FROM skill_drafts WHERE skill_id=$1")
            .bind(skill.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(drafts, 0);
    }
}

#[tokio::test]
async fn a_draft_creation_key_reads_its_current_native_slot_and_never_recreates_a_discarded_draft()
{
    let world = World::new().await;
    let c = company(&world).await;
    let skill = make(&world, &c.viewer, c.acme, "replayed-draft").await;
    let body = SkillDraftCreateBody::default();
    let writer = key(
        &world,
        c.viewer.id(),
        vec![rule(
            "SkillDraftCreate",
            "skill",
            skill.id,
            "self",
            "manage",
        )],
    )
    .await
    .unwrap();
    let header = new_id().to_string();
    let first = checked(
        world
            .app
            .call(SkillDraftCreate, &[&skill.id])
            .token(&writer.secret)
            .header("Idempotency-Key", &header)
            .body(&body)
            .send_bytes()
            .await,
        StatusCode::CREATED,
    );
    assert_eq!(first, json!({"projection":"receipt","skill_id":skill.id}));
    let edited = put_text(
        &world,
        &c.viewer,
        skill.id,
        "notes.txt",
        "The current native draft.",
        None,
    )
    .await;
    let accepted: String =
        sqlx::query_scalar("SELECT xmin::text FROM skill_drafts WHERE skill_id=$1")
            .bind(skill.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    // The original version's supplier is not needed by the metadata reader of a completed draft.
    let source = skill.latest.as_ref().unwrap().archive.blob;
    world.app.app.blobs.store().delete(&source).await.unwrap();
    for own in [true, false] {
        let reader = key(
            &world,
            c.viewer.id(),
            vec![rule(
                if own {
                    "SkillDraftCreate"
                } else {
                    "SkillVersionGet"
                },
                "skill",
                skill.id,
                "self",
                "read",
            )],
        )
        .await
        .unwrap();
        let replay = checked(
            world
                .app
                .call(SkillDraftCreate, &[&skill.id])
                .token(&reader.secret)
                .header("Idempotency-Key", &header)
                .body(&body)
                .send_bytes()
                .await,
            StatusCode::CREATED,
        );
        if own {
            assert_eq!(replay, serde_json::to_value(&edited).unwrap());
            let fresh = world
                .app
                .call(SkillDraftCreate, &[&skill.id])
                .token(&reader.secret)
                .header("Idempotency-Key", &new_id().to_string())
                .body(&body)
                .send_bytes()
                .await;
            assert_eq!(fresh.status, StatusCode::FORBIDDEN);
        } else {
            assert_eq!(replay, first);
        }
        let current: String =
            sqlx::query_scalar("SELECT xmin::text FROM skill_drafts WHERE skill_id=$1")
                .bind(skill.id)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(current, accepted);
    }
    let discarded = world
        .app
        .call(SkillDraftDelete, &[&skill.id])
        .token(&c.viewer.token)
        .send_bytes()
        .await;
    assert_eq!(discarded.status, StatusCode::NO_CONTENT);
    let reader = key(
        &world,
        c.viewer.id(),
        vec![rule("SkillDraftCreate", "skill", skill.id, "self", "read")],
    )
    .await
    .unwrap();
    assert_eq!(
        checked(
            world
                .app
                .call(SkillDraftCreate, &[&skill.id])
                .token(&reader.secret)
                .header("Idempotency-Key", &header)
                .body(&body)
                .send_bytes()
                .await,
            StatusCode::CREATED,
        ),
        first
    );
    let drafts: i64 = sqlx::query_scalar("SELECT count(*) FROM skill_drafts WHERE skill_id=$1")
        .bind(skill.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(drafts, 0);
    assert_eq!(versions(&world, skill.id).await.len(), 1);
}

#[tokio::test]
async fn native_skill_creation_keys_read_new_and_uploaded_resources_without_repeating_suppliers() {
    for archived in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let operation = if archived {
            "SkillCreate.upload"
        } else {
            "SkillCreate.new"
        };
        let blob = if archived {
            Some(
                upload(
                    &world,
                    &c.boss,
                    "header-skill.tar.gz",
                    &archive("header-skill", "Initial native bytes."),
                )
                .await,
            )
        } else {
            None
        };
        let body = SkillCreateBody {
            home_id: c.acme,
            from: match blob {
                Some(blob) => uploaded(blob, "header-skill.tar.gz"),
                None => SkillSource::New {
                    name: "header-skill".into(),
                    description: "A native supplied draft.".into(),
                },
            },
            access: Some(NewResourceAccess::OnlyMe),
        };
        let mut grants = vec![rule(operation, "org", c.acme, "subtree", "manage")];
        if archived {
            grants.push(serde_json::json!({"target":{"kind":"account","id":c.boss.id(),"reach":"self"},"resources":["upload"],"allow":[{"operation":operation,"actions":["read"]}]}));
        }
        let writer = key(&world, c.boss.id(), grants).await.unwrap();
        let header = new_id().to_string();
        let first = checked(
            world
                .app
                .call(SkillCreate, &[])
                .token(&writer.secret)
                .header("Idempotency-Key", &header)
                .body(&body)
                .send_bytes()
                .await,
            StatusCode::CREATED,
        );
        let id: Id = serde_json::from_value(first["skill_id"].clone()).unwrap();
        assert_eq!(first["projection"], "receipt");
        let original_version: Option<Id> =
            sqlx::query_scalar("SELECT latest_version_id FROM skills WHERE id=$1")
                .bind(id)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        if archived {
            assert_eq!(first["version_id"], json!(original_version.unwrap()));
        } else {
            assert!(first.get("version_id").is_none());
        }
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM skills WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        checked(
            world
                .app
                .call(SkillUpdate, &[&id])
                .token(&c.boss.token)
                .raw_body(&json!({"enabled":false,"expected_revision":revision}))
                .send_bytes()
                .await,
            StatusCode::OK,
        );
        let accepted = state(&world, id).await;
        if let Some(blob) = blob {
            world.app.app.blobs.store().delete(&blob).await.unwrap();
        }
        for own in [true, false] {
            let reader = key(
                &world,
                c.boss.id(),
                vec![rule(
                    if own { operation } else { "SkillGet" },
                    "skill",
                    id,
                    "self",
                    "read",
                )],
            )
            .await
            .unwrap();
            let replay = checked(
                world
                    .app
                    .call(SkillCreate, &[])
                    .token(&reader.secret)
                    .header("Idempotency-Key", &header)
                    .body(&body)
                    .send_bytes()
                    .await,
                StatusCode::CREATED,
            );
            if own {
                let full: Skill = serde_json::from_value(replay).unwrap();
                assert_eq!(full.id, id);
                assert_eq!(full.name, "header-skill");
                assert!(!full.enabled, "replay selects current native metadata");
                assert_eq!(
                    full.latest.as_ref().map(|version| version.id),
                    original_version
                );
                assert_eq!(full.versions, i64::from(archived));
                let fresh = world
                    .app
                    .call(SkillCreate, &[])
                    .token(&reader.secret)
                    .header("Idempotency-Key", &new_id().to_string())
                    .body(&body)
                    .send_bytes()
                    .await;
                assert_eq!(fresh.status, StatusCode::FORBIDDEN);
            } else {
                assert_eq!(replay, first);
            }
            assert_eq!(state(&world, id).await, accepted);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM skills WHERE name='header-skill'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        assert_eq!(versions(&world, id).await.len(), usize::from(archived));
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='skill.created'",
        )
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(audits, 1);
    }
}
