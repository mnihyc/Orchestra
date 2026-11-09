//! Native template copies retain schema/restricted-driver checks and recreate ownership at their destination.
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
    duplicates::{DuplicateBody, DuplicateDestination, TemplateDuplicate},
    resources::{TemplateCreate, TemplateCreateBody, UnitKind},
};
use orch_types::{Id, Json};
use serde_json::json;
use support::World;
use thread_history_scope::{Proof, key, ok, rule};

struct Scene {
    world: World,
    company: resources::Company,
    source: Id,
    host: Id,
}
impl Scene {
    async fn new(kind: UnitKind) -> Proof<Self> {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let host = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut *world.conn().await,
            "host",
            Some(company.boss.id()),
            Some(company.acme),
            "Actual reported template schema",
        )
        .await?;
        let schema =
            json!({"type":"object","required":["image"],"properties":{"image":{"type":"string"}}});
        sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1").bind(host).bind(json!({"native":{"name":"native","version":"1.0.0","spec_schemas":{"environment":schema,"provider":schema}}})).execute(&world.db.pool).await?;
        let source = world
            .app
            .call(TemplateCreate, &[])
            .token(&company.boss.token)
            .body(&TemplateCreateBody {
                name: "Actual recipe".into(),
                description: Some("Source configuration".into()),
                home_id: Some(company.acme),
                kind: Some(kind),
                driver: "native".into(),
                spec: json!({"image":"fixture:v1"}),
                access: Some(NewResourceAccess::OnlyMe),
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "source ordinary result missing")?
            .id;
        Ok(Self {
            world,
            company,
            source,
            host,
        })
    }
    async fn copy(&self, token: &str, body: &DuplicateBody) -> orch_api_core::testing::RawReply {
        self.world
            .app
            .call(TemplateDuplicate, &[&self.source])
            .token(token)
            .body(body)
            .send_bytes()
            .await
    }
    async fn row(
        &self,
        id: Id,
    ) -> Proof<(
        String,
        Option<Id>,
        Option<Id>,
        Option<Id>,
        String,
        Json,
        i64,
    )> {
        Ok(sqlx::query_as("SELECT name,home_id,org_id,owner_id,kind,spec,(SELECT count(*) FROM runtimes WHERE template_id=$1) FROM env_templates WHERE id=$1").bind(id).fetch_one(&self.world.db.pool).await?)
    }
    async fn copy_policy(&self, value: bool, locked: bool) -> Proof {
        let mut tx = self.world.db.pool.begin().await?;
        orch_settings::write::write(
            &mut tx,
            &orch_settings::write::Write {
                actor: orch_db::audit::Actor::User(self.company.boss.id()),
                target: orch_settings::Target::Node(self.company.acme),
                key: orch_settings_contract::registry::keys::SHARING_ALLOW_COPY_OUT,
                value: json!(value),
                locked: Some(locked),
                expected_revision: None,
                may_lock: true,
                origin: &orch_db::audit::Origin::default(),
            },
            self.world.now(),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn count(&self) -> Proof<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM env_templates")
            .fetch_one(&self.world.db.pool)
            .await?)
    }
}
fn id(reply: &orch_api_core::testing::RawReply) -> Proof<Id> {
    let value = ok(reply)?;
    let id: Id = serde_json::from_value(value["id"].clone())?;
    assert_eq!(
        value,
        json!({"projection":"receipt","kind":"template","id":id})
    );
    Ok(id)
}
#[tokio::test]
async fn both_template_pools_duplicate_native_configuration_with_fresh_pages_in_the_same_org()
-> Proof {
    for kind in [UnitKind::Environment, UnitKind::Provider] {
        let s = Scene::new(kind).await?;
        let department = places::make_department(
            &s.world,
            &s.company.boss,
            s.company.acme,
            "Recipe home",
            "recipe-home",
        )
        .await
        .base
        .id;

        resources::name_person(
            &s.world,
            "template",
            s.source,
            "template.view",
            s.company.viewer.id(),
        )
        .await;
        resources::name_person(
            &s.world,
            "template",
            s.source,
            "template.manage",
            s.company.stranger.id(),
        )
        .await;
        for home in [None, Some(department)] {
            let body = DuplicateBody {
                destination: home.map(|id| DuplicateDestination::Node { id }),
                ..Default::default()
            };
            let made = id(&s.copy(&s.company.viewer.token, &body).await)?;
            let row = s.row(made).await?;
            assert_ne!(made, s.source);
            assert_eq!(
                (row.0.as_str(), row.1, row.2, row.3),
                (
                    "Actual recipe copy",
                    Some(home.unwrap_or(s.company.acme)),
                    Some(s.company.acme),
                    Some(s.company.viewer.id())
                )
            );
            assert_eq!(
                row.4,
                if kind == UnitKind::Environment {
                    "environment"
                } else {
                    "provider"
                }
            );
            assert_eq!(row.5, json!({"image":"fixture:v1"}));
            assert_eq!(row.6, 0);
            let managers:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='template' AND object_id=$1 AND holder_id=$2 AND removed_at IS NULL").bind(made).bind(s.company.stranger.id()).fetch_one(&s.world.db.pool).await?;
            assert_eq!(managers, 0);
            let audits: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='template.created'",
            )
            .bind(made)
            .fetch_one(&s.world.db.pool)
            .await?;
            assert_eq!(audits, 1);
        }
        assert_eq!(s.count().await?, 3);
    }
    Ok(())
}
#[tokio::test]
async fn template_duplicate_needs_its_own_complete_scope_and_current_native_driver_administration()
-> Proof {
    let s = Scene::new(UnitKind::Environment).await?;
    let read = || rule("TemplateDuplicate", "template", s.source, "self", "read");
    let write = || rule("TemplateDuplicate", "org", s.company.acme, "self", "write");
    for rules in [
        vec![read()],
        vec![write()],
        vec![
            rule("TemplateGet", "template", s.source, "self", "read"),
            write(),
        ],
    ] {
        let token = key(&s.world, s.company.boss.id(), rules).await?;
        assert_eq!(
            s.copy(&token.secret, &DuplicateBody::default())
                .await
                .status
                .as_u16(),
            403
        );
    }
    let token = key(&s.world, s.company.boss.id(), vec![read(), write()]).await?;
    id(&s.copy(&token.secret, &DuplicateBody::default()).await)?;
    s.world
        .set_at(
            orch_settings::Target::System,
            "templates.restricted_drivers",
            json!(["native"]),
        )
        .await;
    assert_eq!(
        s.copy(&token.secret, &DuplicateBody::default())
            .await
            .status
            .as_u16(),
        403
    );
    s.world.make_admin(&s.company.boss.person).await;
    assert_eq!(
        s.copy(&token.secret, &DuplicateBody::default())
            .await
            .status
            .as_u16(),
        403,
        "system standing cannot widen the key's own admitted family"
    );
    let system = json!({"target":{"kind":"system","reach":"self"},"resources":["template"],"allow":[{"operation":"TemplateDuplicate","actions":["manage"]}]});
    let full = key(&s.world, s.company.boss.id(), vec![read(), write(), system]).await?;
    id(&s.copy(&full.secret, &DuplicateBody::default()).await)?;
    assert_eq!(s.count().await?, 3);
    Ok(())
}
#[tokio::test]
async fn template_duplicate_uses_the_current_saved_schema_and_optional_source_revision() -> Proof {
    let s = Scene::new(UnitKind::Environment).await?;
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
    let schema =
        json!({"type":"object","required":["image"],"properties":{"image":{"type":"boolean"}}});
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1").bind(s.host).bind(json!({"native":{"name":"native","version":"1.0.0","spec_schemas":{"environment":schema,"provider":schema}}})).execute(&s.world.db.pool).await?;
    let refused = s
        .copy(&s.company.boss.token, &DuplicateBody::default())
        .await;
    assert_eq!(
        refused.problem().code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
    assert_eq!(s.count().await?, 1);
    assert_eq!(s.row(s.source).await?.5, json!({"image":"fixture:v1"}));
    Ok(())
}

#[tokio::test]
async fn cross_org_template_copy_checks_the_source_policy_keeps_fresh_pages_and_audits_once()
-> Proof {
    let s = Scene::new(UnitKind::Environment).await?;
    let destination = places::make_org(&s.world, &s.company.boss, "Recipient", "recipient")
        .await
        .base
        .id;
    let tag = orch_types::new_id();
    sqlx::query("INSERT INTO tags(id,node_id,name,color) VALUES($1,$2,'Source-only tag',1)")
        .bind(tag)
        .bind(s.company.acme)
        .execute(&s.world.db.pool)
        .await?;
    sqlx::query(
        "INSERT INTO resource_tags(resource_kind,resource_id,tag_id) VALUES('template',$1,$2)",
    )
    .bind(s.source)
    .bind(tag)
    .execute(&s.world.db.pool)
    .await?;
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("TemplateDuplicate", "template", s.source, "self", "read"),
            rule("TemplateDuplicate", "org", destination, "self", "write"),
        ],
    )
    .await?;
    let body = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let made = id(&s.copy(&token.secret, &body).await)?;
    let row = s.row(made).await?;
    assert_eq!(
        (row.1, row.2, row.3),
        (
            Some(destination),
            Some(destination),
            Some(s.company.boss.id())
        )
    );
    assert_eq!(row.5, json!({"image":"fixture:v1"}));
    let tags: i64 = sqlx::query_scalar("SELECT count(*) FROM resource_tags WHERE resource_id=$1")
        .bind(made)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(tags, 0);
    let audit: (String, Option<Id>, Json) = sqlx::query_as(
        "SELECT object_kind,org_id,details FROM audit_log WHERE action='content.copied_out' AND object_id=$1")
        .bind(s.source).fetch_one(&s.world.db.pool).await?;
    assert_eq!(audit.0, "template");
    assert_eq!(audit.1, Some(s.company.acme));
    assert_eq!(audit.2["destination_org_id"], json!(destination));
    assert_eq!(audit.2["created_id"], json!(made));
    let before = s.count().await?;
    s.copy_policy(false, true).await?;
    let denied = s.copy(&token.secret, &body).await;
    assert_eq!(denied.status.as_u16(), 403, "{:?}", denied);
    assert_eq!(s.count().await?, before);
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='content.copied_out' AND object_id=$1",
    )
    .bind(s.source)
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(retained, 1);
    let local = id(&s
        .copy(&s.company.boss.token, &DuplicateBody::default())
        .await)?;
    assert_eq!(s.row(local).await?.2, Some(s.company.acme));
    let local_tags: i64 =
        sqlx::query_scalar("SELECT count(*) FROM resource_tags WHERE resource_id=$1 AND tag_id=$2")
            .bind(local)
            .bind(tag)
            .fetch_one(&s.world.db.pool)
            .await?;
    assert_eq!(
        local_tags, 1,
        "a source copy-out lock does not change same-root duplication"
    );
    Ok(())
}

#[tokio::test]
async fn destination_judgements_are_read_only_keep_refused_candidates_and_retain_native_ownership()
-> Proof {
    use orch_api_contract::duplicates::{
        CopyDestinationCause, CopyDestinationChoice, CopyDestinationOutcome, CopyDestinationsJudge,
        CopyDestinationsRequest, CopyIntent, CopySubject, CopySubjectKind,
    };
    let s = Scene::new(UnitKind::Environment).await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Picker recipient",
        "picker-recipient",
    )
    .await
    .base
    .id;
    let subject = CopySubject {
        kind: CopySubjectKind::Template,
        id: s.source,
    };
    let request = CopyDestinationsRequest {
        subject,
        intent: CopyIntent::Duplicate,
        destinations: vec![
            DuplicateDestination::Node { id: s.company.acme },
            DuplicateDestination::Node { id: destination },
        ],
    };
    let before = s.count().await?;
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&s.world.db.pool)
        .await?;
    let sources: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='content.copied_out'")
            .fetch_one(&s.world.db.pool)
            .await?;
    let reply = s
        .world
        .app
        .call(CopyDestinationsJudge, &[])
        .token(&s.company.boss.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(reply.subject, subject);
    assert_eq!(reply.intent, CopyIntent::Duplicate);
    assert_eq!(reply.destinations.len(), 2);
    assert!(!reply.destinations[0].cross_root);
    assert!(reply.destinations[1].cross_root);
    assert!(
        reply
            .destinations
            .iter()
            .all(|row| matches!(row.outcome, CopyDestinationOutcome::Allowed))
    );
    resources::name_person(
        &s.world,
        "template",
        s.source,
        "template.view",
        s.company.admin.id(),
    )
    .await;
    resources::name_person(
        &s.world,
        "template",
        s.source,
        "template.manage",
        s.company.admin.id(),
    )
    .await;
    places::join(
        &s.world,
        destination,
        destination,
        &s.company.admin,
        "admin",
        &s.company.boss,
    )
    .await;
    let moving = CopyDestinationsRequest {
        intent: CopyIntent::Move,
        ..request.clone()
    };
    let manager = s
        .world
        .app
        .call(CopyDestinationsJudge, &[])
        .token(&s.company.admin.token)
        .body(&moving)
        .send()
        .await
        .ok();
    assert!(matches!(
        manager.destinations[1].outcome,
        CopyDestinationOutcome::Blocked {
            cause: CopyDestinationCause::OwnersOnly,
            ..
        }
    ));
    let owner = s
        .world
        .app
        .call(CopyDestinationsJudge, &[])
        .token(&s.company.boss.token)
        .body(&moving)
        .send()
        .await
        .ok();
    assert!(matches!(
        owner.destinations[1].outcome,
        CopyDestinationOutcome::NeedsChoice {
            choice: CopyDestinationChoice::DeletePreview
        }
    ));
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule(
                "CopyDestinationsJudge",
                "template",
                s.source,
                "self",
                "read",
            ),
            rule("CopyDestinationsJudge", "org", destination, "self", "read"),
        ],
    )
    .await?;
    let scoped = CopyDestinationsRequest {
        destinations: vec![DuplicateDestination::Node { id: destination }],
        ..request
    };
    let reply = s
        .world
        .app
        .call(CopyDestinationsJudge, &[])
        .token(&token.secret)
        .body(&scoped)
        .send()
        .await
        .ok();
    assert!(matches!(
        reply.destinations[0].outcome,
        CopyDestinationOutcome::NeedsChoice {
            choice: CopyDestinationChoice::NativeOperationScope
        }
    ));
    s.copy_policy(false, true).await?;
    let policy = s
        .world
        .app
        .call(CopyDestinationsJudge, &[])
        .token(&s.company.boss.token)
        .body(&scoped)
        .send()
        .await
        .ok();
    assert!(matches!(
        policy.destinations[0].outcome,
        CopyDestinationOutcome::Blocked {
            cause: CopyDestinationCause::SourcePolicy,
            ..
        }
    ));
    let missing = key(
        &s.world,
        s.company.boss.id(),
        vec![rule(
            "CopyDestinationsJudge",
            "template",
            s.source,
            "self",
            "read",
        )],
    )
    .await?;
    let refused = s
        .world
        .app
        .call(CopyDestinationsJudge, &[])
        .token(&missing.secret)
        .body(&scoped)
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    assert_eq!(s.count().await?, before);
    let after_jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&s.world.db.pool)
        .await?;
    let after_sources: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='content.copied_out'")
            .fetch_one(&s.world.db.pool)
            .await?;
    assert_eq!(after_jobs, jobs);
    assert_eq!(after_sources, sources);
    Ok(())
}

#[tokio::test]
async fn an_owner_moves_a_template_atomically_under_its_current_native_delete_plan() -> Proof {
    use orch_api_contract::duplicates::{
        MoveOutBody, MoveOutConfirmation, MoveOutDeleteEffect, TemplateMoveOut,
        TemplateMoveOutPreview,
    };
    let s = Scene::new(UnitKind::Environment).await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Move recipient",
        "move-recipient",
    )
    .await
    .base
    .id;
    resources::name_person(
        &s.world,
        "template",
        s.source,
        "template.manage",
        s.company.stranger.id(),
    )
    .await;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let reviewer = key(&s.world, s.company.boss.id(), vec![
        serde_json::json!({"target":{"kind":"template","id":s.source,"reach":"self"},"resources":["template"],"allow":[{"operation":"TemplateMoveOutPreview","actions":["manage","read"]}]}),
        rule("TemplateMoveOutPreview","org",destination,"self","write"),
    ]).await?;
    let plan = s
        .world
        .app
        .call(TemplateMoveOutPreview, &[&s.source])
        .token(&reviewer.secret)
        .body(&copy)
        .send()
        .await
        .ok();
    assert_eq!(plan.subject.id, s.source);
    assert_eq!(plan.effects, vec![MoveOutDeleteEffect::RemoveSource]);
    assert_eq!(s.count().await?, 1, "the preview does not copy or delete");
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("TemplateMoveOut", "template", s.source, "self", "manage"),
            rule("TemplateMoveOut", "org", destination, "self", "write"),
        ],
    )
    .await?;
    let reply = s
        .world
        .app
        .call(TemplateMoveOut, &[&s.source])
        .token(&token.secret)
        .body(&MoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    let made = id(&reply)?;
    let source_deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM env_templates WHERE id=$1")
            .bind(s.source)
            .fetch_one(&s.world.db.pool)
            .await?;
    assert!(source_deleted);
    let row = s.row(made).await?;
    assert_eq!(
        (row.1, row.2, row.3),
        (
            Some(destination),
            Some(destination),
            Some(s.company.boss.id())
        )
    );
    assert_eq!(row.5, json!({"image":"fixture:v1"}));
    let inherited: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_id=$1 AND holder_id=$2 AND removed_at IS NULL")
        .bind(made).bind(s.company.stranger.id()).fetch_one(&s.world.db.pool).await?;
    assert_eq!(inherited, 0);
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_log WHERE (object_id=$1 AND action IN ('content.copied_out','template.deleted')) OR (object_id=$2 AND action='template.created') ORDER BY action")
        .bind(s.source).bind(made).fetch_all(&s.world.db.pool).await?;
    assert_eq!(
        actions,
        vec!["content.copied_out", "template.created", "template.deleted"]
    );
    Ok(())
}

#[tokio::test]
async fn template_move_refuses_a_nonowner_and_a_changed_delete_plan_before_either_change() -> Proof
{
    use orch_api_contract::duplicates::{
        MoveOutBody, MoveOutConfirmation, TemplateMoveOut, TemplateMoveOutPreview,
    };
    let s = Scene::new(UnitKind::Environment).await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Move recipient",
        "move-recipient",
    )
    .await
    .base
    .id;
    for actor in [&s.company.viewer, &s.company.admin] {
        resources::name_person(
            &s.world,
            "template",
            s.source,
            "template.manage",
            actor.id(),
        )
        .await;
        places::join(
            &s.world,
            destination,
            destination,
            actor,
            "admin",
            &s.company.boss,
        )
        .await;
        if actor.id() == s.company.admin.id() {
            places::admin_mode(&s.world, actor, true).await;
        }
        let refused = s
            .world
            .app
            .call(TemplateMoveOutPreview, &[&s.source])
            .token(&actor.token)
            .body(&DuplicateBody {
                destination: Some(DuplicateDestination::Node { id: destination }),
                ..Default::default()
            })
            .send_bytes()
            .await;
        assert_eq!(refused.status.as_u16(), 403, "{:?}", refused);
    }
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(TemplateMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    sqlx::query("UPDATE env_templates SET revision=revision+1 WHERE id=$1")
        .bind(s.source)
        .execute(&s.world.db.pool)
        .await?;
    let refused = s
        .world
        .app
        .call(TemplateMoveOut, &[&s.source])
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
    let source_live: bool =
        sqlx::query_scalar("SELECT deleted_at IS NULL FROM env_templates WHERE id=$1")
            .bind(s.source)
            .fetch_one(&s.world.db.pool)
            .await?;
    assert!(source_live);
    Ok(())
}

#[tokio::test]
async fn a_native_delete_failure_rolls_back_the_template_copy_and_every_move_audit() -> Proof {
    use orch_api_contract::duplicates::{
        MoveOutBody, MoveOutConfirmation, TemplateMoveOut, TemplateMoveOutPreview,
    };
    let s = Scene::new(UnitKind::Environment).await?;
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
        .call(TemplateMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    sqlx::query("CREATE FUNCTION reject_template_ending() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL THEN RAISE EXCEPTION 'controlled native deletion failure' USING ERRCODE='23514'; END IF; RETURN NEW; END $$")
        .execute(&s.world.db.pool).await?;
    sqlx::query("CREATE TRIGGER reject_template_ending BEFORE UPDATE ON env_templates FOR EACH ROW EXECUTE FUNCTION reject_template_ending()")
        .execute(&s.world.db.pool).await?;
    let refused = s
        .world
        .app
        .call(TemplateMoveOut, &[&s.source])
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
    assert!(!refused.status.is_success());
    assert_eq!(s.count().await?, 1);
    let copied: i64 = sqlx::query_scalar("SELECT count(*) FROM env_templates WHERE home_id=$1")
        .bind(destination)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(copied, 0);
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action IN ('content.copied_out','template.deleted') AND object_id=$1")
        .bind(s.source).fetch_one(&s.world.db.pool).await?;
    assert_eq!(audit, 0);
    Ok(())
}

#[tokio::test]
async fn a_template_delete_refusal_refuses_the_whole_move_with_the_same_native_reason() -> Proof {
    use orch_api_contract::duplicates::{
        MoveOutBody, MoveOutConfirmation, TemplateMoveOut, TemplateMoveOutPreview,
    };
    let s = Scene::new(UnitKind::Environment).await?;
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
        .call(TemplateMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    sqlx::query("INSERT INTO environments(id,name,owner_id,org_id,home_node_id,placement,template_id) VALUES($1,'Actual dependent environment',$2,$3,$3,'managed',$4)")
        .bind(orch_types::new_id()).bind(s.company.boss.id()).bind(s.company.acme).bind(s.source).execute(&s.world.db.pool).await?;
    let refused = s
        .world
        .app
        .call(TemplateMoveOut, &[&s.source])
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
    assert_eq!(
        refused.problem().code,
        orch_api_contract::common::ErrorCode::ResourceInUse
    );
    assert_eq!(s.count().await?, 1);
    let source_live: bool =
        sqlx::query_scalar("SELECT deleted_at IS NULL FROM env_templates WHERE id=$1")
            .bind(s.source)
            .fetch_one(&s.world.db.pool)
            .await?;
    assert!(source_live);
    Ok(())
}
