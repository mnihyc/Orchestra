//! Agent duplicates copy readable configuration, retain physical guards and recreate native pages.
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
    duplicates::{AgentDuplicate, DuplicateBody, DuplicateDestination},
    resources::{OrgAgentCreate, OrgAgentCreateBody},
};
use orch_types::{Id, Json};
use serde_json::json;
use support::World;
use thread_history_scope::{Proof, key, ok, rule};
fn body() -> OrgAgentCreateBody {
    OrgAgentCreateBody {
        name: "Native profile".into(),
        harness_kind: "fake".into(),
        profile: json!({"config":{"mode":"native"},"env":{"MODE":"one"}}),
    }
}
struct Scene {
    world: World,
    company: resources::Company,
    source: Id,
}
impl Scene {
    async fn new() -> Proof<Self> {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let source = world
            .app
            .call(OrgAgentCreate, &[&company.acme])
            .token(&company.boss.token)
            .body(&body())
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "actual source profile missing")?
            .base
            .id;
        Ok(Self {
            world,
            company,
            source,
        })
    }
    async fn copy(&self, token: &str, body: &DuplicateBody) -> orch_api_core::testing::RawReply {
        self.world
            .app
            .call(AgentDuplicate, &[&self.source])
            .token(token)
            .body(body)
            .send_bytes()
            .await
    }
    async fn count(&self) -> Proof<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM agents WHERE home_id=$1")
                .bind(self.company.acme)
                .fetch_one(&self.world.db.pool)
                .await?,
        )
    }
}
fn id(reply: &orch_api_core::testing::RawReply) -> Proof<Id> {
    let value = ok(reply)?;
    let id = serde_json::from_value(value["id"].clone())?;
    assert_eq!(
        value,
        json!({"projection":"receipt","kind":"agent","id":id})
    );
    Ok(id)
}
#[tokio::test]
async fn native_profile_duplicates_copy_configuration_without_descriptors_history_or_source_permissions()
-> Proof {
    let s = Scene::new().await?;
    let department = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "New profile home",
        "new-profile-home",
    )
    .await
    .base
    .id;

    for place in [s.company.acme, department] {
        resources::name_person(
            &s.world,
            "node",
            place,
            "settings.manage",
            s.company.viewer.id(),
        )
        .await;
    }
    resources::name_person(
        &s.world,
        "agent",
        s.source,
        "agent.view",
        s.company.viewer.id(),
    )
    .await;
    resources::name_person(
        &s.world,
        "agent",
        s.source,
        "agent.manage",
        s.company.stranger.id(),
    )
    .await;
    sqlx::query(
        "UPDATE agents SET descriptor='{}',descriptor_overrides='{}',disabled=true WHERE id=$1",
    )
    .bind(s.source)
    .execute(&s.world.db.pool)
    .await?;
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
        type CopiedProfile = (
            String,
            Option<Id>,
            Option<Id>,
            Option<Id>,
            Option<Id>,
            Json,
            Json,
            bool,
        );
        let row: CopiedProfile = sqlx::query_as("SELECT name,home_id,org_id,owner_id,machine_id,profile,descriptor,disabled FROM agents WHERE id=$1").bind(made).fetch_one(&s.world.db.pool).await?;
        assert_eq!(
            (row.0.as_str(), row.1, row.2, row.3, row.4),
            (
                "Native profile copy",
                Some(home.unwrap_or(s.company.acme)),
                Some(s.company.acme),
                Some(s.company.viewer.id()),
                None
            )
        );
        assert_eq!(row.5["config"], json!({"mode":"native"}));
        assert_eq!(row.5["env"], json!({"MODE":"one"}));
        assert_eq!(row.6, json!({}));
        assert!(!row.7);
        let old:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='agent' AND object_id=$1 AND holder_id=$2 AND removed_at IS NULL").bind(made).bind(s.company.stranger.id()).fetch_one(&s.world.db.pool).await?;
        assert_eq!(old, 0);
        let owners:Vec<Id>=sqlx::query_scalar("SELECT holder_id FROM permission_entries WHERE object_kind='agent' AND object_id=$1 AND permission='access.own' AND removed_at IS NULL").bind(made).fetch_all(&s.world.db.pool).await?;
        assert_eq!(owners, vec![s.company.viewer.id()]);
    }
    Ok(())
}
#[tokio::test]
async fn profile_duplicate_requires_own_read_destination_write_and_current_native_creation_policy()
-> Proof {
    let s = Scene::new().await?;
    let read = || rule("AgentDuplicate", "agent", s.source, "self", "read");
    let write = || rule("AgentDuplicate", "org", s.company.acme, "self", "write");
    for rules in [
        vec![read()],
        vec![write()],
        vec![rule("AgentGet", "agent", s.source, "self", "read"), write()],
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
    let actor = key(&s.world, s.company.boss.id(), vec![read(), write()]).await?;
    id(&s.copy(&actor.secret, &DuplicateBody::default()).await)?;
    assert_eq!(s.count().await?, 2);
    resources::name_person(
        &s.world,
        "agent",
        s.source,
        "agent.view",
        s.company.viewer.id(),
    )
    .await;
    s.world
        .set_at(
            orch_settings::Target::Node(s.company.acme),
            "resources.created_by_members",
            json!([]),
        )
        .await;
    assert_eq!(
        s.copy(&s.company.viewer.token, &DuplicateBody::default())
            .await
            .status
            .as_u16(),
        403
    );
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
    assert_eq!(s.count().await?, 2);
    Ok(())
}
#[tokio::test]
async fn system_profile_duplicates_require_current_system_authority_and_give_the_new_copy_an_owner()
-> Proof {
    let world = World::new().await;
    let maker = places::admin(&world, "system-maker").await;
    let source = world
        .app
        .call(orch_api_contract::admin::AdminAgentCreate, &[])
        .token(&maker.token)
        .body(&body())
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "actual global source profile missing")?
        .base
        .id;
    resources::name_person(&world, "agent", source, "agent.view", maker.id()).await;
    let read = rule("AgentDuplicate", "agent", source, "self", "read");
    let system = json!({"target":{"kind":"system","reach":"self"},"resources":["agent"],"allow":[{"operation":"AgentDuplicate","actions":["write"]}]});
    let token = key(&world, maker.id(), vec![read, system]).await?;
    let reply = world
        .app
        .call(AgentDuplicate, &[&source])
        .token(&token.secret)
        .body(&DuplicateBody::default())
        .send_bytes()
        .await;
    let made = id(&reply)?;
    let home: Option<Id> = sqlx::query_scalar("SELECT home_id FROM agents WHERE id=$1")
        .bind(made)
        .fetch_one(&world.db.pool)
        .await?;
    assert_eq!(home, None);
    let owners:Vec<Id>=sqlx::query_scalar("SELECT holder_id FROM permission_entries WHERE object_kind='agent' AND object_id=$1 AND permission='access.own' AND removed_at IS NULL").bind(made).fetch_all(&world.db.pool).await?;
    assert_eq!(owners, vec![maker.id()]);
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(maker.id())
        .execute(&world.db.pool)
        .await?;
    let refused = world
        .app
        .call(AgentDuplicate, &[&source])
        .token(&token.secret)
        .body(&DuplicateBody::default())
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    Ok(())
}

#[tokio::test]
async fn machine_profile_duplicate_retains_the_actual_physical_registration_and_current_machine_use()
-> Proof {
    let mut s = Scene::new().await?;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut *s.world.conn().await,
        s.company.boss.id(),
        "Physical harness machine",
    )
    .await?;
    let facts = json!({"adapters":[{"kind":"fake","adapter_version":1,"builtin":true}],"harnesses":[{"kind":"fake","path":"/test/harness","version":"1.0.0","daemon":false}],"policy":{"agent.harnesses":[{"kind":"fake","path":"/test/harness"}]}});
    sqlx::query("UPDATE machines SET roles=ARRAY['agent'],facts=$2 WHERE id=$1")
        .bind(machine)
        .bind(facts)
        .execute(&s.world.db.pool)
        .await?;
    s.source = s
        .world
        .app
        .call(
            orch_api_contract::resources::MachineAgentCreate,
            &[&machine],
        )
        .token(&s.company.boss.token)
        .body(&orch_api_contract::resources::MachineAgentCreateBody {
            home_id: s.company.acme,
            harness_kind: "fake".into(),
            name: "Physical native profile".into(),
            profile: Some(body().profile),
        })
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "physical native source missing")?
        .base
        .id;
    let read = || rule("AgentDuplicate", "agent", s.source, "self", "read");
    let write = || rule("AgentDuplicate", "org", s.company.acme, "self", "write");
    let physical = || rule("AgentDuplicate", "machine", machine, "self", "write");
    let partial = key(&s.world, s.company.boss.id(), vec![read(), write()]).await?;
    assert_eq!(
        s.copy(&partial.secret, &DuplicateBody::default())
            .await
            .status
            .as_u16(),
        403
    );
    let full = key(
        &s.world,
        s.company.boss.id(),
        vec![read(), write(), physical()],
    )
    .await?;
    let made = id(&s.copy(&full.secret, &DuplicateBody::default()).await)?;
    let actual: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM agents WHERE id=$1")
        .bind(made)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(actual, Some(machine));
    assert_eq!(s.count().await?, 3);
    resources::name_person(
        &s.world,
        "agent",
        s.source,
        "agent.view",
        s.company.viewer.id(),
    )
    .await;
    resources::name_person(
        &s.world,
        "node",
        s.company.acme,
        "settings.manage",
        s.company.viewer.id(),
    )
    .await;
    let viewer = key(
        &s.world,
        s.company.viewer.id(),
        vec![read(), write(), physical()],
    )
    .await?;
    assert!(
        !s.copy(&viewer.secret, &DuplicateBody::default())
            .await
            .status
            .is_success(),
        "complete key facts never supply ordinary use of another person's physical machine"
    );
    sqlx::query("UPDATE machines SET roles='{}' WHERE id=$1")
        .bind(machine)
        .execute(&s.world.db.pool)
        .await?;
    assert!(
        !s.copy(&full.secret, &DuplicateBody::default())
            .await
            .status
            .is_success()
    );
    assert_eq!(s.count().await?, 3);
    Ok(())
}

#[tokio::test]
async fn cross_org_agent_copy_uses_native_creation_and_records_the_original_source_namespace()
-> Proof {
    let s = Scene::new().await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Cross-root recipient",
        "cross-root-recipient",
    )
    .await
    .base
    .id;
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("AgentDuplicate", "agent", s.source, "self", "read"),
            rule("AgentDuplicate", "org", destination, "self", "write"),
        ],
    )
    .await?;
    let made = id(&s
        .copy(
            &token.secret,
            &DuplicateBody {
                destination: Some(DuplicateDestination::Node { id: destination }),
                ..Default::default()
            },
        )
        .await)?;
    assert_ne!(made, s.source);
    let row: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('home',home_id,'org',org_id,'owner',owner_id) FROM agents WHERE id=$1")
        .bind(made).fetch_one(&s.world.db.pool).await?;
    assert_eq!(
        row,
        json!({"home":destination,"org":destination,"owner":s.company.boss.id()})
    );
    let audit: (Option<Id>, serde_json::Value) = sqlx::query_as(
        "SELECT org_id,details FROM audit_log WHERE object_id=$1 AND action='content.copied_out'",
    )
    .bind(s.source)
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(audit.0, Some(s.company.acme));
    assert_eq!(audit.1["created_id"], json!(made));
    assert_eq!(audit.1["destination_org_id"], json!(destination));
    Ok(())
}

#[tokio::test]
async fn an_owner_moves_a_profile_with_fresh_access_and_the_native_durable_source_stop() -> Proof {
    use orch_api_contract::duplicates::{
        AgentMoveOut, AgentMoveOutPreview, MoveOutBody, MoveOutConfirmation, MoveOutDeleteEffect,
    };
    let s = Scene::new().await?;
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
        "agent",
        s.source,
        "agent.manage",
        s.company.stranger.id(),
    )
    .await;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(AgentMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    assert_eq!(plan.subject.id, s.source);
    assert_eq!(
        plan.effects,
        vec![
            MoveOutDeleteEffect::StopBoundWork,
            MoveOutDeleteEffect::RemoveSource
        ]
    );
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='resource_quiesce'")
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(s.count().await?, 1, "the preview does not create a profile");
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("AgentMoveOut", "agent", s.source, "self", "manage"),
            rule("AgentMoveOut", "org", destination, "self", "write"),
        ],
    )
    .await?;
    let moved = s
        .world
        .app
        .call(AgentMoveOut, &[&s.source])
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
    let made = id(&moved)?;
    let ended: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM agents WHERE id=$1")
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert!(ended);
    let fresh: (Option<Id>, Option<Id>, Option<Id>, Json, bool) =
        sqlx::query_as("SELECT home_id,org_id,owner_id,profile,disabled FROM agents WHERE id=$1")
            .bind(made)
            .fetch_one(&s.world.db.pool)
            .await?;
    assert_eq!(
        (fresh.0, fresh.1, fresh.2),
        (
            Some(destination),
            Some(destination),
            Some(s.company.boss.id())
        )
    );
    assert_eq!(fresh.3["config"], json!({"mode":"native"}));
    assert!(!fresh.4);
    let copied_access: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_id=$1 AND holder_id=$2 AND removed_at IS NULL").bind(made).bind(s.company.stranger.id()).fetch_one(&s.world.db.pool).await?;
    assert_eq!(copied_access, 0);
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_log WHERE (object_id=$1 AND action IN ('content.copied_out','agent.deleted')) OR (object_id=$2 AND action='agent.created') ORDER BY action").bind(s.source).bind(made).fetch_all(&s.world.db.pool).await?;
    assert_eq!(
        actions,
        vec!["agent.created", "agent.deleted", "content.copied_out"]
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='resource_quiesce'")
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(
        after,
        before + 1,
        "the same native durable stop is accepted with the deletion"
    );
    let accepted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE kind='resource_quiesce' AND plan#>>'{quiesce,target,object,id}'=$1 AND plan#>>'{quiesce,deleted}'='true')").bind(s.source.to_string()).fetch_one(&s.world.db.pool).await?;
    assert!(accepted);
    Ok(())
}

#[tokio::test]
async fn profile_move_refuses_managers_nonowning_admins_and_a_changed_native_plan() -> Proof {
    use orch_api_contract::duplicates::{
        AgentMoveOut, AgentMoveOutPreview, MoveOutBody, MoveOutConfirmation,
    };
    let s = Scene::new().await?;
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
    for actor in [&s.company.viewer, &s.company.admin] {
        resources::name_person(&s.world, "agent", s.source, "agent.manage", actor.id()).await;
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
        let denied = s
            .world
            .app
            .call(AgentMoveOutPreview, &[&s.source])
            .token(&actor.token)
            .body(&copy)
            .send_bytes()
            .await;
        assert_eq!(denied.status.as_u16(), 403, "{:?}", denied);
    }
    let plan = s
        .world
        .app
        .call(AgentMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    sqlx::query("UPDATE agents SET revision=revision+1,profile=jsonb_set(profile,'{config,mode}','\"changed\"') WHERE id=$1").bind(s.source).execute(&s.world.db.pool).await?;
    let changed = s
        .world
        .app
        .call(AgentMoveOut, &[&s.source])
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
        changed.problem().code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    let copies: i64 = sqlx::query_scalar("SELECT count(*) FROM agents WHERE home_id=$1")
        .bind(destination)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(copies, 0);
    let live: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM agents WHERE id=$1")
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert!(live);
    Ok(())
}

#[tokio::test]
async fn a_late_profile_delete_failure_rolls_back_the_copy_stop_job_and_both_audits() -> Proof {
    use orch_api_contract::duplicates::{
        AgentMoveOut, AgentMoveOutPreview, MoveOutBody, MoveOutConfirmation,
    };
    let s = Scene::new().await?;
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
        .call(AgentMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send()
        .await
        .ok();
    sqlx::query("CREATE FUNCTION refuse_profile_move_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.deleted_at IS NOT NULL THEN RAISE EXCEPTION 'held native delete'; END IF; RETURN NEW; END $$").execute(&s.world.db.pool).await?;
    sqlx::query("CREATE TRIGGER refuse_profile_move_delete BEFORE UPDATE OF deleted_at ON agents FOR EACH ROW EXECUTE FUNCTION refuse_profile_move_delete()").execute(&s.world.db.pool).await?;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='resource_quiesce'")
        .fetch_one(&s.world.db.pool)
        .await?;
    let denied = s
        .world
        .app
        .call(AgentMoveOut, &[&s.source])
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
    assert_eq!(denied.status.as_u16(), 500);
    let copies: i64 = sqlx::query_scalar("SELECT count(*) FROM agents WHERE home_id=$1")
        .bind(destination)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(copies, 0);
    let live: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM agents WHERE id=$1")
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert!(live);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='resource_quiesce'")
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(after, before);
    let exports: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action IN ('content.copied_out','agent.deleted')").bind(s.source).fetch_one(&s.world.db.pool).await?;
    assert_eq!(exports, 0);
    Ok(())
}
