//! Physical registration moves reset governance atomically and never duplicate enrollment.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod places;
mod resources;
mod support;
mod thread_history_scope;
use orch_api_contract::{
    duplicates::{
        self as api, DuplicateBody, DuplicateDestination, MoveOutBody, MoveOutConfirmation,
        MoveOutDeletePlan,
    },
    resource_groups::ResourceHomeSetReceipt,
};
use orch_types::{Id, Json, Time};
use serde_json::json;
use support::World;
use thread_history_scope::{Proof, key, ok, rule};

struct Scene {
    world: World,
    company: resources::Company,
    kind: &'static str,
    source: Id,
    machine: Id,
    target: Id,
}
impl Scene {
    async fn new(kind: &'static str) -> Proof<Self> {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let source = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut *world.conn().await,
            kind,
            Some(company.boss.id()),
            Some(company.acme),
            "Physical source",
        )
        .await?;
        orch_authz::ownership::start_resource(
            &mut *world.conn().await,
            orch_access_contract::decisions::ObjectRef {
                kind: kind.parse::<orch_access_contract::permissions::ObjectKind>()?,
                id: source,
            },
            company.boss.id(),
            orch_authz::ownership::NewAccess::OnlyMe,
            Time::new(world.now()),
        )
        .await?;
        let machine = if kind == "host" {
            sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
                .bind(source)
                .fetch_one(&world.db.pool)
                .await?
        } else {
            source
        };
        let target = places::make_org(
            &world,
            &company.boss,
            "Registration recipient",
            "registration-recipient",
        )
        .await
        .base
        .id;
        Ok(Self {
            world,
            company,
            kind,
            source,
            machine,
            target,
        })
    }
    fn body(&self) -> DuplicateBody {
        DuplicateBody {
            destination: Some(DuplicateDestination::Node { id: self.target }),
            ..Default::default()
        }
    }
    async fn preview(&self, token: &str) -> orch_api_core::testing::RawReply {
        if self.kind == "host" {
            self.world
                .app
                .call(api::HostMoveOutPreview, &[&self.source])
                .token(token)
                .body(&self.body())
                .send_bytes()
                .await
        } else {
            self.world
                .app
                .call(api::MachineMoveOutPreview, &[&self.source])
                .token(token)
                .body(&self.body())
                .send_bytes()
                .await
        }
    }
    async fn apply(
        &self,
        token: &str,
        plan: MoveOutDeletePlan,
    ) -> orch_api_core::testing::RawReply {
        let body = MoveOutBody {
            copy: self.body(),
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        };
        if self.kind == "host" {
            self.world
                .app
                .call(api::HostMoveOut, &[&self.source])
                .token(token)
                .body(&body)
                .send_bytes()
                .await
        } else {
            self.world
                .app
                .call(api::MachineMoveOut, &[&self.source])
                .token(token)
                .body(&body)
                .send_bytes()
                .await
        }
    }
    async fn registration(&self) -> Proof<Json> {
        Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',id,'serial',cert_serial,'fingerprint',encode(cert_fingerprint,'hex'),'public_key',cert_public_key,'session',current_session_id,'online',online,'roles',roles,'facts',facts,'placement',placement,'runtime',runtime_id,'host',host_id,'status',status) FROM machines WHERE id=$1")
            .bind(self.machine).fetch_one(&self.world.db.pool).await?)
    }
    async fn home(&self) -> Proof<Option<Id>> {
        if self.kind == "host" {
            Ok(
                sqlx::query_scalar("SELECT home_id FROM hosts WHERE id=$1")
                    .bind(self.source)
                    .fetch_one(&self.world.db.pool)
                    .await?,
            )
        } else {
            Ok(
                sqlx::query_scalar("SELECT home_id FROM machines WHERE id=$1")
                    .bind(self.source)
                    .fetch_one(&self.world.db.pool)
                    .await?,
            )
        }
    }
}
#[tokio::test]
async fn host_and_machine_moves_keep_exact_enrollment_and_reset_access_and_tags_at_the_new_root()
-> Proof {
    for kind in ["host", "machine"] {
        let s = Scene::new(kind).await?;
        resources::name_person(
            &s.world,
            kind,
            s.source,
            &format!("{kind}.manage"),
            s.company.stranger.id(),
        )
        .await;
        let tag = orch_types::new_id();
        sqlx::query(
            "INSERT INTO tags(id,node_id,name,color) VALUES($1,$2,'Original registration tag',1)",
        )
        .bind(tag)
        .bind(s.company.acme)
        .execute(&s.world.db.pool)
        .await?;
        sqlx::query("INSERT INTO resource_tags(resource_kind,resource_id,tag_id) VALUES($1,$2,$3)")
            .bind(kind)
            .bind(s.source)
            .bind(tag)
            .execute(&s.world.db.pool)
            .await?;
        let before = s.registration().await?;
        let plan: MoveOutDeletePlan =
            serde_json::from_value(ok(&s.preview(&s.company.boss.token).await)?)?;
        assert_eq!(
            plan.effects,
            vec![api::MoveOutDeleteEffect::KeepRegistration]
        );
        assert_eq!(s.home().await?, Some(s.company.acme));
        let operation = if kind == "host" {
            "HostMoveOut"
        } else {
            "MachineMoveOut"
        };
        let actor = key(
            &s.world,
            s.company.boss.id(),
            vec![
                rule(operation, kind, s.source, "self", "manage"),
                rule(operation, "org", s.target, "self", "write"),
            ],
        )
        .await?;
        let reply = s.apply(&actor.secret, plan).await;
        let value = ok(&reply)?;
        let receipt: ResourceHomeSetReceipt = serde_json::from_value(value.clone())?;
        assert_eq!(
            value,
            json!({"projection":"receipt","object_id":s.source,"kind":kind,"home_id":s.target,"changed":true})
        );
        assert_eq!(receipt.object_id, s.source);
        assert_eq!(
            s.registration().await?,
            before,
            "{kind}: no replacement, revocation, session or credential mutation"
        );
        assert_eq!(s.home().await?, Some(s.target));
        let active:Vec<(String,Option<Id>)>=sqlx::query_as("SELECT permission,holder_id FROM permission_entries WHERE object_kind=$1 AND object_id=$2 AND removed_at IS NULL ORDER BY permission").bind(kind).bind(s.source).fetch_all(&s.world.db.pool).await?;
        assert_eq!(
            active,
            vec![("access.own".into(), Some(s.company.boss.id()))]
        );
        let tags: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM resource_tags WHERE resource_kind=$1 AND resource_id=$2",
        )
        .bind(kind)
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
        assert_eq!(tags, 0);
        let audits:Vec<(String,Option<Id>)>=sqlx::query_as("SELECT action,org_id FROM audit_log WHERE object_id=$1 AND action IN ('content.copied_out','resource.home_changed') ORDER BY action").bind(s.source).fetch_all(&s.world.db.pool).await?;
        assert_eq!(
            audits,
            vec![
                ("content.copied_out".into(), Some(s.company.acme)),
                ("resource.home_changed".into(), Some(s.target))
            ]
        );
    }
    Ok(())
}
#[tokio::test]
async fn attached_environments_and_current_source_policy_refuse_the_whole_registration_move()
-> Proof {
    for kind in ["host", "machine"] {
        let s = Scene::new(kind).await?;
        let plan: MoveOutDeletePlan =
            serde_json::from_value(ok(&s.preview(&s.company.boss.token).await)?)?;
        s.world
            .set_at(
                orch_settings::Target::Node(s.company.acme),
                "sharing.allow_copy_out",
                json!(false),
            )
            .await;
        let policy = s.apply(&s.company.boss.token, plan.clone()).await;
        assert_eq!(policy.status.as_u16(), 403, "{:?}", policy);
        s.world
            .set_at(
                orch_settings::Target::Node(s.company.acme),
                "sharing.allow_copy_out",
                json!(true),
            )
            .await;
        let env = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut *s.world.conn().await,
            "environment",
            Some(s.company.boss.id()),
            Some(s.company.acme),
            "Native attached environment",
        )
        .await?;
        sqlx::query("UPDATE environments SET machine_id=$2 WHERE id=$1")
            .bind(env)
            .bind(s.machine)
            .execute(&s.world.db.pool)
            .await?;
        let busy = s.apply(&s.company.boss.token, plan).await;
        assert_eq!(
            busy.problem().code,
            orch_api_contract::common::ErrorCode::ResourceInUse
        );
        assert_eq!(s.home().await?, Some(s.company.acme));
        let audits:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action IN ('content.copied_out','resource.home_changed')").bind(s.source).fetch_one(&s.world.db.pool).await?;
        assert_eq!(audits, 0);
    }
    Ok(())
}
#[tokio::test]
async fn registration_move_never_promotes_managers_or_inspecting_admins_to_owners() -> Proof {
    for kind in ["host", "machine"] {
        let s = Scene::new(kind).await?;
        for actor in [&s.company.viewer, &s.company.admin] {
            resources::name_person(
                &s.world,
                kind,
                s.source,
                &format!("{kind}.manage"),
                actor.id(),
            )
            .await;
            places::join(
                &s.world,
                s.target,
                s.target,
                actor,
                "admin",
                &s.company.boss,
            )
            .await;
            if actor.id() == s.company.admin.id() {
                places::admin_mode(&s.world, actor, true).await;
            }
            let denied = s.preview(&actor.token).await;
            assert_eq!(denied.status.as_u16(), 403, "{kind}: {:?}", denied);
        }
        assert_eq!(s.home().await?, Some(s.company.acme));
    }
    Ok(())
}
#[tokio::test]
async fn a_late_native_page_reset_failure_rolls_back_registration_home_and_source_audit() -> Proof {
    let s = Scene::new("host").await?;
    let plan: MoveOutDeletePlan =
        serde_json::from_value(ok(&s.preview(&s.company.boss.token).await)?)?;
    let before = s.registration().await?;
    sqlx::query("CREATE FUNCTION hold_registration_page_reset() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.removed_at IS NOT NULL THEN RAISE EXCEPTION 'held native page reset'; END IF; RETURN NEW; END $$").execute(&s.world.db.pool).await?;
    sqlx::query("CREATE TRIGGER hold_registration_page_reset BEFORE UPDATE OF removed_at ON permission_entries FOR EACH ROW EXECUTE FUNCTION hold_registration_page_reset()").execute(&s.world.db.pool).await?;
    let denied = s.apply(&s.company.boss.token, plan).await;
    assert_eq!(denied.status.as_u16(), 500);
    assert_eq!(s.home().await?, Some(s.company.acme));
    assert_eq!(s.registration().await?, before);
    let audits:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action IN ('content.copied_out','resource.home_changed')").bind(s.source).fetch_one(&s.world.db.pool).await?;
    assert_eq!(audits, 0);
    Ok(())
}
