//! Current skill bytes copy once through native version/draft writers with fresh authority and destination limits.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod resources;
mod support;
mod thread_history_scope;
use orch_api_contract::{
    access::NewResourceAccess,
    duplicates::{DuplicateBody, DuplicateDestination, SkillDraftDuplicate, SkillDuplicate},
    skills::*,
};
use orch_api_core::testing::RawReply;
use orch_db::queries::resources::skills as rows;
use orch_engine::skills::{archive, front_matter};
use orch_types::Id;
use serde_json::json;
use support::World;
use thread_history_scope::{Proof, key, ok, rule};
struct Scene {
    world: World,
    company: resources::Company,
    source: Id,
}
impl Scene {
    async fn new(publish: bool) -> Proof<Self> {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let source = world
            .app
            .call(SkillCreate, &[])
            .token(&company.boss.token)
            .body(&SkillCreateBody {
                home_id: company.acme,
                from: SkillSource::New {
                    name: "source-skill".into(),
                    description: "Does the actual work".into(),
                },
                access: Some(NewResourceAccess::OnlyMe),
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "native source missing")?
            .id;
        let s = Self {
            world,
            company,
            source,
        };
        s.put("scripts/keep.sh", "#!/bin/sh\necho kept\n", true)
            .await?;
        if publish {
            let draft = s.draft().await;
            s.world
                .app
                .call(SkillDraftPublish, &[&s.source])
                .token(&s.company.boss.token)
                .body(&SkillDraftPublishBody {
                    expected_revision: draft.info.revision,
                })
                .send()
                .await
                .ok();
        }
        Ok(s)
    }
    async fn draft(&self) -> SkillDraft {
        self.world
            .app
            .call(SkillDraftGet, &[&self.source])
            .token(&self.company.boss.token)
            .send()
            .await
            .ok()
    }
    async fn put(&self, path: &str, text: &str, executable: bool) -> Proof {
        let draft = self.draft().await;
        self.world
            .app
            .call(SkillDraftFilePut, &[&self.source])
            .token(&self.company.boss.token)
            .body(&SkillDraftFilePutBody {
                path: path.into(),
                text: Some(text.into()),
                blob: None,
                executable: Some(executable),
                expected_revision: draft.info.revision,
            })
            .send()
            .await
            .ok();
        Ok(())
    }
    async fn copy(&self, token: &str, body: &DuplicateBody) -> RawReply {
        self.world
            .app
            .call(SkillDuplicate, &[&self.source])
            .token(token)
            .body(body)
            .send_bytes()
            .await
    }
    async fn count(&self) -> Proof<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM skills")
            .fetch_one(&self.world.db.pool)
            .await?)
    }
}
fn id(reply: &RawReply) -> Proof<Id> {
    let value = ok(reply)?;
    let id = serde_json::from_value(value["id"].clone())?;
    assert_eq!(
        value,
        json!({"projection":"receipt","kind":"skill","id":id})
    );
    Ok(id)
}
#[tokio::test]
async fn published_skill_duplicates_copy_current_files_to_one_new_version_with_fresh_pages_and_no_source_history()
-> Proof {
    let s = Scene::new(true).await?;
    resources::name_person(
        &s.world,
        "skill",
        s.source,
        "skill.view",
        s.company.viewer.id(),
    )
    .await;
    resources::name_person(
        &s.world,
        "skill",
        s.source,
        "skill.manage",
        s.company.stranger.id(),
    )
    .await;
    let department = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Skill home",
        "skill-home",
    )
    .await
    .base
    .id;

    for home in [None, Some(department)] {
        let made = id(&s
            .copy(
                &s.company.viewer.token,
                &DuplicateBody {
                    destination: home.map(|id| DuplicateDestination::Node { id }),
                    ..Default::default()
                },
            )
            .await)?;
        let row = rows::skills(&mut *s.world.conn().await, &[made])
            .await?
            .pop()
            .ok_or("new skill missing")?;
        assert_eq!(row.owner_id, s.company.viewer.id());
        assert_eq!(row.home_id, home.unwrap_or(s.company.acme));
        assert_eq!(row.name, "source-skill-copy");
        assert!(row.origin.is_none());
        assert!(row.source_url_secret_id.is_none());
        let versions = rows::versions_by_id(
            &mut *s.world.conn().await,
            &[row.latest_version_id.ok_or("version missing")?],
        )
        .await?;
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].number, 1);
        assert_eq!(versions[0].origin, json!({"kind":"copy"}));
        let bytes = s
            .world
            .app
            .app
            .blobs
            .read_whole(&versions[0].archive_hash()?, versions[0].archive_size)
            .await?;
        let limits =
            orch_engine::skills::versions::limits_at(&mut *s.world.conn().await, row.home_id)
                .await?;
        let files = archive::unpack(&bytes, limits)?;
        let skill = files
            .iter()
            .find(|file| file.path == "SKILL.md")
            .ok_or("skill file missing")?;
        assert_eq!(
            front_matter::read(std::str::from_utf8(&skill.data)?)?.name,
            "source-skill-copy"
        );
        let script = files
            .iter()
            .find(|file| file.path == "scripts/keep.sh")
            .ok_or("copied script missing")?;
        assert!(script.executable);
        assert_eq!(script.data, b"#!/bin/sh\necho kept\n");
        let old:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='skill' AND object_id=$1 AND holder_id=$2 AND removed_at IS NULL").bind(made).bind(s.company.stranger.id()).fetch_one(&s.world.db.pool).await?;
        assert_eq!(old, 0);
        let records: i64 =
            sqlx::query_scalar("SELECT count(*) FROM skill_versions WHERE skill_id=$1")
                .bind(made)
                .fetch_one(&s.world.db.pool)
                .await?;
        assert_eq!(records, 1);
    }
    assert_eq!(s.count().await?, 3);
    Ok(())
}
#[tokio::test]
async fn native_draft_duplicate_retains_its_manage_door_and_does_not_publish_even_an_unfinished_draft()
-> Proof {
    let s = Scene::new(false).await?;
    s.put("SKILL.md", "Unfinished draft without front matter\n", false)
        .await?;
    resources::name_person(
        &s.world,
        "skill",
        s.source,
        "skill.view",
        s.company.viewer.id(),
    )
    .await;
    let denied = s
        .world
        .app
        .call(SkillDraftDuplicate, &[&s.source])
        .token(&s.company.viewer.token)
        .body(&DuplicateBody::default())
        .send_bytes()
        .await;
    assert!(!denied.status.is_success());
    let body = DuplicateBody::default();
    let first = s
        .world
        .app
        .call(SkillDraftDuplicate, &[&s.source])
        .token(&s.company.boss.token)
        .body(&body)
        .send_bytes()
        .await;
    let made = id(&first)?;
    let row = rows::skills(&mut *s.world.conn().await, &[made])
        .await?
        .pop()
        .ok_or("new draft skill missing")?;
    assert!(row.latest_version_id.is_none());
    assert_eq!(row.name, "source-skill-copy");
    let files = rows::draft_files(&mut *s.world.conn().await, made).await?;
    assert_eq!(files.len(), 2);
    let skill = files
        .iter()
        .find(|file| file.path == "SKILL.md")
        .ok_or("copied unfinished file missing")?;
    let content = s
        .world
        .app
        .app
        .blobs
        .read_whole(&skill.blob_hash()?, skill.size)
        .await?;
    assert_eq!(&content[..], b"Unfinished draft without front matter\n");
    let readonly = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("SkillDraftDuplicate", "skill", s.source, "self", "read"),
            rule(
                "SkillDraftDuplicate",
                "org",
                s.company.acme,
                "self",
                "write",
            ),
        ],
    )
    .await?;
    assert_eq!(
        s.world
            .app
            .call(SkillDraftDuplicate, &[&s.source])
            .token(&readonly.secret)
            .body(&body)
            .send_bytes()
            .await
            .status
            .as_u16(),
        403
    );
    let full = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("SkillDraftDuplicate", "skill", s.source, "self", "manage"),
            rule(
                "SkillDraftDuplicate",
                "org",
                s.company.acme,
                "self",
                "write",
            ),
        ],
    )
    .await?;
    id(&s
        .world
        .app
        .call(SkillDraftDuplicate, &[&s.source])
        .token(&full.secret)
        .body(&body)
        .send_bytes()
        .await)?;
    Ok(())
}
#[tokio::test]
async fn skill_duplicate_keeps_exact_source_read_destination_write_current_limits_revision_and_retry_identity()
-> Proof {
    let s = Scene::new(true).await?;
    let read = || rule("SkillDuplicate", "skill", s.source, "self", "read");
    let write = || rule("SkillDuplicate", "org", s.company.acme, "self", "write");
    for rules in [
        vec![read()],
        vec![write()],
        vec![rule("SkillGet", "skill", s.source, "self", "read"), write()],
    ] {
        let actor = key(&s.world, s.company.boss.id(), rules).await?;
        assert_eq!(
            s.copy(&actor.secret, &DuplicateBody::default())
                .await
                .status
                .as_u16(),
            403
        );
    }
    let full = key(&s.world, s.company.boss.id(), vec![read(), write()]).await?;
    let retry = orch_types::new_id().to_string();
    let body = DuplicateBody::default();
    let first = s
        .world
        .app
        .call(SkillDuplicate, &[&s.source])
        .token(&full.secret)
        .header("Idempotency-Key", &retry)
        .body(&body)
        .send_bytes()
        .await;
    let made = id(&first)?;
    let reader = key(
        &s.world,
        s.company.boss.id(),
        vec![rule("SkillGet", "skill", s.source, "self", "read")],
    )
    .await?;
    let replay = s
        .world
        .app
        .call(SkillDuplicate, &[&s.source])
        .token(&reader.secret)
        .header("Idempotency-Key", &retry)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(id(&replay)?, made);
    let stale = s
        .copy(
            &s.company.boss.token,
            &DuplicateBody {
                expected_revision: Some(0),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        stale.problem().code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    s.world
        .set_at(
            orch_settings::Target::Node(s.company.acme),
            "skills.max_files",
            json!(1),
        )
        .await;
    assert!(!s.copy(&full.secret, &body).await.status.is_success());
    assert_eq!(s.count().await?, 2);
    Ok(())
}

#[tokio::test]
async fn copied_skill_bytes_keep_the_selected_source_and_current_credential_after_the_real_post_io_source_wait()
-> Proof {
    for change in ["positive", "stale", "expire", "refused"] {
        let s = Scene::new(true).await?;
        let actor = key(
            &s.world,
            s.company.boss.id(),
            vec![
                rule("SkillDuplicate", "skill", s.source, "self", "read"),
                rule("SkillDuplicate", "org", s.company.acme, "self", "write"),
            ],
        )
        .await?;
        let mut blocker = s.world.db.pool.begin().await?;
        rows::lock(&mut blocker, s.source)
            .await?
            .ok_or("source lock missing")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = s.world.app.clone();
        let source = s.source;
        let token = actor.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(SkillDuplicate, &[&source])
                .token(&token)
                .body(&DuplicateBody::default());
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {loop {
            assert!(!request.is_finished(),"the selected byte supplier did not reach its actual guarded native source lock");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM skills WHERE id=$1%FOR UPDATE%')").bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
            if waiting {break;}tokio::task::yield_now().await;
        }}).await?;
        match change {
            "stale" => {
                sqlx::query("UPDATE skills SET revision=revision+1 WHERE id=$1")
                    .bind(s.source)
                    .execute(&mut *blocker)
                    .await?;
            }
            "expire" => s.world.advance(chrono::Duration::hours(2)),
            // A change of the key's own rules is ordered after the request that holds the key's row, so it cannot
            // land while the request waits at the source; the org refusing the key can.
            "refused" => {
                s.world
                    .keep_key_out(actor.id, s.company.acme, s.company.boss.id())
                    .await;
            }
            _ => {}
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request).await??;
        if change == "positive" {
            id(&reply)?;
            assert_eq!(s.count().await?, 2);
        } else {
            assert!(
                !reply.status.is_success(),
                "{change} must close before copying selected bytes into a new skill"
            );
            if change == "stale" {
                assert_eq!(
                    reply.problem().code,
                    orch_api_contract::common::ErrorCode::ConflictStaleRevision
                );
            }
            assert_eq!(s.count().await?, 1);
        }
    }
    Ok(())
}

#[tokio::test]
async fn cross_org_published_and_draft_skills_use_their_own_native_content_writer() -> Proof {
    for published in [true, false] {
        let s = Scene::new(published).await?;
        let destination = places::make_org(
            &s.world,
            &s.company.boss,
            "Skill recipient",
            "skill-recipient",
        )
        .await
        .base
        .id;
        let operation = if published {
            "SkillDuplicate"
        } else {
            "SkillDraftDuplicate"
        };
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![
                rule(
                    operation,
                    "skill",
                    s.source,
                    "self",
                    if published { "read" } else { "manage" },
                ),
                rule(operation, "org", destination, "self", "write"),
            ],
        )
        .await?;
        let body = DuplicateBody {
            destination: Some(DuplicateDestination::Node { id: destination }),
            ..Default::default()
        };
        let reply = if published {
            s.copy(&token.secret, &body).await
        } else {
            s.world
                .app
                .call(SkillDraftDuplicate, &[&s.source])
                .token(&token.secret)
                .body(&body)
                .send_bytes()
                .await
        };
        let made = id(&reply)?;
        let row: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('home',home_id,'org',org_id,'owner',owner_id,'published',latest_version_id IS NOT NULL) FROM skills WHERE id=$1")
            .bind(made).fetch_one(&s.world.db.pool).await?;
        assert_eq!(
            row,
            json!({"home":destination,"org":destination,"owner":s.company.boss.id(),"published":published})
        );
        let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='content.copied_out' AND details->>'created_id'=$2")
            .bind(s.source).bind(made.to_string()).fetch_one(&s.world.db.pool).await?;
        assert_eq!(audits, 1);
    }
    Ok(())
}

#[tokio::test]
async fn published_and_draft_skill_moves_use_the_native_once_only_content_supplier_and_end_the_source()
-> Proof {
    use orch_api_contract::duplicates::{
        MoveOutBody, MoveOutConfirmation, SkillDraftMoveOut, SkillDraftMoveOutPreview,
        SkillMoveOut, SkillMoveOutPreview,
    };
    for published in [true, false] {
        let s = Scene::new(published).await?;
        let destination = places::make_org(
            &s.world,
            &s.company.boss,
            "Move recipient",
            "move-recipient",
        )
        .await
        .base
        .id;
        let copy = DuplicateBody {
            destination: Some(DuplicateDestination::Node { id: destination }),
            ..Default::default()
        };
        let plan = if published {
            s.world
                .app
                .call(SkillMoveOutPreview, &[&s.source])
                .token(&s.company.boss.token)
                .body(&copy)
                .send()
                .await
                .ok()
        } else {
            s.world
                .app
                .call(SkillDraftMoveOutPreview, &[&s.source])
                .token(&s.company.boss.token)
                .body(&copy)
                .send()
                .await
                .ok()
        };
        let operation = if published {
            "SkillMoveOut"
        } else {
            "SkillDraftMoveOut"
        };
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![
                rule(operation, "skill", s.source, "self", "manage"),
                rule(operation, "org", destination, "self", "write"),
            ],
        )
        .await?;
        let body = MoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        };
        let reply = if published {
            s.world
                .app
                .call(SkillMoveOut, &[&s.source])
                .token(&token.secret)
                .body(&body)
                .send_bytes()
                .await
        } else {
            s.world
                .app
                .call(SkillDraftMoveOut, &[&s.source])
                .token(&token.secret)
                .body(&body)
                .send_bytes()
                .await
        };
        let moved = id(&reply)?;
        let row: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('home',home_id,'org',org_id,'owner',owner_id,'published',latest_version_id IS NOT NULL) FROM skills WHERE id=$1")
            .bind(moved).fetch_one(&s.world.db.pool).await?;
        assert_eq!(
            row,
            json!({"home":destination,"org":destination,"owner":s.company.boss.id(),"published":published})
        );
        let ended: bool =
            sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM skills WHERE id=$1")
                .bind(s.source)
                .fetch_one(&s.world.db.pool)
                .await?;
        assert!(ended);
        let copied_files: serde_json::Value = if published {
            sqlx::query_scalar("SELECT files FROM skill_versions WHERE skill_id=$1")
                .bind(moved)
                .fetch_one(&s.world.db.pool)
                .await?
        } else {
            sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_object('path',path,'executable',executable) ORDER BY path) FROM skill_draft_files WHERE skill_id=$1")
                .bind(moved).fetch_one(&s.world.db.pool).await?
        };
        assert!(copied_files.to_string().contains("scripts/keep.sh"));
        let audits: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_log WHERE (object_id=$1 AND action IN ('content.copied_out','skill.deleted')) OR (object_id=$2 AND action='skill.created') ORDER BY action")
            .bind(s.source).bind(moved).fetch_all(&s.world.db.pool).await?;
        assert_eq!(
            audits,
            vec!["content.copied_out", "skill.created", "skill.deleted"]
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_changed_native_draft_refuses_move_without_a_second_skill_or_source_deletion() -> Proof {
    use orch_api_contract::duplicates::{
        MoveOutBody, MoveOutConfirmation, SkillDraftMoveOut, SkillDraftMoveOutPreview,
    };
    let s = Scene::new(false).await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Move recipient",
        "move-recipient",
    )
    .await
    .base
    .id;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(SkillDraftMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    s.put(
        "scripts/keep.sh",
        "#!/bin/sh\necho newer native source\n",
        true,
    )
    .await?;
    let refused = s
        .world
        .app
        .call(SkillDraftMoveOut, &[&s.source])
        .token(&s.company.boss.token)
        .body(&MoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 409, "{:?}", refused);
    assert_eq!(s.count().await?, 1);
    let live: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM skills WHERE id=$1")
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert!(live);
    Ok(())
}
