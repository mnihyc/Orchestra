//! A native fork maker owns its pending child before any Thread binds that Environment.
use super::*;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ObjectKind};
use orch_api_contract::{
    fork_sources,
    forks::{FileSelection, ForkEnvironment, ForkNewEnvironment},
    routes::Endpoint,
};
use orch_authz::TokenGrant;
use orch_engine::{
    Engine, EngineParts,
    fork_plan::operation::{self as source, Confirm},
};
use orch_types::Time;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const OP: &str = "ThreadForkCreate.fork";
struct Child {
    rig: Rig,
    _host: hosting::ScriptedHost,
    engine: Engine,
    parent: orch_engine::jobs::fork::operation::Preparation,
    made: orch_engine::environments::Made,
    token: Id,
    _unit: Option<hosting::Unit>,
}
impl Child {
    async fn new() -> Proof<Self> {
        Self::with_existing(false).await
    }
    async fn with_existing(existing: bool) -> Proof<Self> {
        let rig = Rig::new(Can::default(), "fake", Can::default()).await;
        hosting::serve(&rig.world);
        let host = hosting::ScriptedHost::on(&rig.world, "native-child-host", 16_384).await;
        let template = hosting::template(&rig.world).await;
        let existing_target = if existing {
            Some(hosting::running(&rig.world, &host, template, "Existing fork target").await)
        } else {
            None
        };
        let thread = rig.world.thread("Parent source").await;
        let mut request = rig.fork();
        request.environment = Some(ForkEnvironment::New {
            new: ForkNewEnvironment {
                template_id: Some(template),
                host_id: Some(Some(host.id)),
                files: Some(FileSelection::Nothing),
                ..ForkNewEnvironment::default()
            },
        });
        if let Some((target, _, _)) = &existing_target {
            request.environment = Some(ForkEnvironment::Existing {
                environment_id: *target,
                files: Some(FileSelection::Nothing),
            });
        }
        let plan = rig.plan(thread, &request).await;
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        rig.world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: rig.world.pool.clone(),
            clock: rig.world.clock.clone(),
            config: common::config(),
            commands: rig.world.commands.as_engine(),
            access: rig.world.access.clone(),
            live: rig.world.live.clone(),
        });
        let token = new_id();
        let hash = Hash::of(token.as_bytes());
        let expires = Time::new(rig.world.clock.now() + chrono::Duration::hours(1));
        let policy: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"thread","id":thread,"reach":"self"},
            "allow":[{"operation":OP,"actions":["manage"]}]
        }]}))?;
        assert!(policy.validate().is_empty());
        let mut tx = rig.world.pool.begin().await?;
        orch_db::queries::identity::create_api_token_scoped(
            &mut tx,
            &orch_db::queries::identity::NewApiToken {
                id: token,
                user_id: rig.world.ada,
                name: "Native child source",
                token_hash: hash.as_bytes(),
                scopes: &[],
                node_id: None,
                at: rig.world.clock.now(),
                expires_at: Some(expires.get()),
            },
            Some(&json!(policy)),
        )
        .await?;
        let caller = Caller::token(
            rig.world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(policy), Some(expires))?.for_token(token),
        );
        let target = orch_engine::operation_scope::targets::page_object(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Thread,
                id: thread,
            },
        )
        .await?;
        let known = orch_engine::operation_scope::sources::bind_known(
            orch_api_contract::forks::ThreadForkCreate::ROUTE,
            &fork_sources::CONFIRM_FORK,
            &[orch_engine::operation_scope::sources::Facts::one(
                fork_sources::PlanSource::Thread,
                target.object.kind,
                target,
            )],
        )?;
        let caller = known.preparation_caller(&caller)?;
        let body = ForkCreateBody {
            preflight_id: plan.preflight_id,
            notify: None,
            options: None,
        };
        let selected = engine
            .select_fork_confirmation(&mut tx, &caller, thread, &body)
            .await?;
        let input = source::Input::selected(&mut tx, selected.selected()).await?;
        let facts = source::facts::<Confirm>(&mut tx, &input).await?;
        let bound = orch_engine::operation_scope::sources::bind_required(
            orch_api_contract::forks::ThreadForkCreate::ROUTE,
            &fork_sources::CONFIRM_FORK,
            &facts,
        )?;
        let caller = bound.admit(&caller, false)?;
        let origin = source::Origin::capture::<Confirm>(
            &caller,
            input,
            orch_engine::operation_scope::request_origin::Credential::Token {
                id: token,
                hash: hash.as_bytes().to_vec(),
            },
            Some(expires),
        )?;
        tx.commit().await?;
        let preparing = source::Preparation::new(origin);
        let draft = engine
            .assemble_prepared_fork_with_source(
                &mut *rig.world.pool.acquire().await?,
                selected.selected(),
                Some(&preparing),
            )
            .await?;
        let origin = preparing.retained().await;
        let mut tx = rig.world.pool.begin().await?;
        let (principal, _) = origin
            .current::<Confirm>(&mut tx, rig.world.clock.as_ref(), rig.world.access.as_ref())
            .await?;
        let confirmation = engine
            .prepare_fork_confirmation(&mut tx, principal.caller(), thread, &body)
            .await?;
        let parent_row = engine
            .commit_fork_confirmation_with_source(
                tx,
                principal.caller(),
                &confirmation,
                draft,
                &origin,
                || Ok(()),
                || Ok(()),
            )
            .await?;
        let parent = orch_engine::jobs::fork::operation::Preparation::from_job(&parent_row)?
            .ok_or("native parent source missing")?;
        if let Some((target, runtime, unit)) = existing_target {
            sqlx::query("UPDATE environments SET status='stopped' WHERE id=$1")
                .bind(target)
                .execute(rig.world.pool.pool())
                .await?;
            sqlx::query("UPDATE runtimes SET state='stopped' WHERE id=$1")
                .bind(runtime)
                .execute(rig.world.pool.pool())
                .await?;
            let started = engine.start_environment_from_fork(target, &parent).await?;
            let orch_engine::environments::Starting::Job(job) = started else {
                return Err("the existing unit did not queue its native wake".into());
            };
            assert_eq!(job.plan.get("mode"), Some(&json!("start")));
            let made = orch_engine::environments::Made {
                id: target,
                job: *job,
            };
            return Ok(Self {
                rig,
                _host: host,
                engine,
                parent,
                made,
                token,
                _unit: Some(unit),
            });
        }
        let mut tx = rig.world.pool.begin().await?;
        let checkpoint = parent
            .current(&mut tx, rig.world.clock.as_ref(), rig.world.access.as_ref())
            .await?;
        let made = orch_engine::environments::create(
            &mut tx,
            rig.world.clock.now(),
            checkpoint.principal(),
            &orch_engine::environments::Request {
                owner: rig.world.ada,
                name: Some("Native child"),
                home: rig.world.topic,
                template: Some(template),
                host: Some(Some(host.id)),
                config: &json!({}),
            },
        )
        .await?;
        jobs::merge_plan(
            &mut tx,
            parent_row.id,
            &json!({"new_environment":made.id}),
            rig.world.clock.now(),
        )
        .await?;
        orch_engine::jobs::provision::retain_fork_provision_origin(
            &mut tx,
            &parent,
            &made,
            rig.world.topic,
            rig.world.clock.as_ref(),
            rig.world.access.as_ref(),
        )
        .await?;
        let bound_threads: i64 =
            sqlx::query_scalar("SELECT count(*) FROM threads WHERE environment_id=$1")
                .bind(made.id)
                .fetch_one(&mut *tx)
                .await?;
        assert_eq!(
            bound_threads, 0,
            "the native child is retained before any target Thread is inserted/bound"
        );
        tx.commit().await?;
        Ok(Self {
            rig,
            _host: host,
            engine,
            parent,
            made,
            token,
            _unit: None,
        })
    }
    async fn current(&self) -> Proof<bool> {
        let mut tx = self.rig.world.pool.begin().await?;
        let row = jobs::job(&mut tx, self.made.job.id)
            .await?
            .ok_or("native provision missing")?;
        let result = orch_engine::jobs::provision::creator_checkpoint(
            &mut tx,
            &row,
            self.rig.world.clock.as_ref(),
            self.rig.world.access.as_ref(),
        )
        .await;
        let permitted = match result {
            Ok(Some(check)) => {
                assert_eq!(check.principal().caller().admitted_operation(), Some(OP));
                check.check(self.rig.world.clock.as_ref())?;
                assert!(check.until().is_some());
                true
            }
            Ok(None) => return Err("known parent source became ordinary work".into()),
            Err(
                orch_engine::EngineError::BeforeEffect(_) | orch_engine::EngineError::Unreadable(_),
            ) => false,
            Err(error) => return Err(Box::new(error)),
        };
        tx.commit().await?;
        Ok(permitted)
    }
}

#[tokio::test]
async fn fork_child_without_a_bound_thread_uses_the_original_parent_scope_and_raw_credential()
-> Proof {
    for change in [
        "live", "scope", "expired", "parent", "null", "missing", "version", "config", "cancel",
    ] {
        let s = Child::new().await?;
        assert!(s.current().await?);
        let mut tx = s.rig.world.pool.begin().await?;
        match change {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(json!({"rules":[]}))
                    .execute(&mut *tx)
                    .await?;
            }
            "expired" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(s.rig.world.clock.now() - chrono::Duration::seconds(1))
                    .execute(&mut *tx)
                    .await?;
            }
            "parent" => {
                jobs::merge_plan(
                    &mut tx,
                    s.parent.job(),
                    &json!({"new_environment":new_id()}),
                    s.rig.world.clock.now(),
                )
                .await?;
            }
            "null" => {
                jobs::merge_plan(
                    &mut tx,
                    s.made.job.id,
                    &json!({"creator_provision":null}),
                    s.rig.world.clock.now(),
                )
                .await?;
            }
            "missing" => {
                sqlx::query("UPDATE jobs SET plan=plan-'creator_provision' WHERE id=$1")
                    .bind(s.made.job.id)
                    .execute(&mut *tx)
                    .await?;
            }
            "version" => {
                jobs::merge_plan(
                    &mut tx,
                    s.made.job.id,
                    &json!({"creator_provision_version":2}),
                    s.rig.world.clock.now(),
                )
                .await?;
            }
            "config" => {
                sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
                    .bind(s.made.id)
                    .bind(json!({"changed_after_creation":true}))
                    .execute(&mut *tx)
                    .await?;
            }
            "cancel" => {
                jobs::request_cancel_by(
                    &mut tx,
                    s.parent.job(),
                    Some(s.rig.world.ada),
                    s.rig.world.clock.now(),
                )
                .await?;
            }
            _ => {}
        }
        tx.commit().await?;
        assert_eq!(
            s.current().await?,
            change == "live",
            "native parent/provision control {change}"
        );
        s.engine.stop();
    }
    Ok(())
}

#[tokio::test]
async fn an_existing_provision_cannot_be_adopted_by_replaying_the_fork_child_retainer() -> Proof {
    let s = Child::new().await?;
    let mut tx = s.rig.world.pool.begin().await?;
    assert!(
        orch_engine::jobs::provision::retain_fork_provision_origin(
            &mut tx,
            &s.parent,
            &s.made,
            s.rig.world.topic,
            s.rig.world.clock.as_ref(),
            s.rig.world.access.as_ref()
        )
        .await
        .is_err()
    );
    tx.rollback().await?;
    assert!(
        s.current().await?,
        "the failed replay leaves the original child authority intact"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn an_existing_target_wake_retains_the_real_fork_source_and_original_unit() -> Proof {
    for change in [
        "live",
        "scope",
        "expired",
        "unit",
        "generation",
        "home",
        "cancel",
        "null",
    ] {
        let s = Child::with_existing(true).await?;
        assert!(s.current().await?);
        let mut tx = s.rig.world.pool.begin().await?;
        let row = jobs::job(&mut tx, s.made.job.id)
            .await?
            .ok_or("wake child missing")?;
        assert_eq!(row.plan["creator_provision_version"], json!(2));
        match change {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(json!({"rules":[]}))
                    .execute(&mut *tx)
                    .await?;
            }
            "expired" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(s.rig.world.clock.now() - chrono::Duration::seconds(1))
                    .execute(&mut *tx)
                    .await?;
            }
            "unit" => {
                sqlx::query("UPDATE runtimes SET state='retired' WHERE environment_id=$1")
                    .bind(s.made.id)
                    .execute(&mut *tx)
                    .await?;
            }
            "generation" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(s.made.id)
                    .execute(&mut *tx)
                    .await?;
            }
            "home" => {
                sqlx::query("UPDATE environments SET home_node_id=$2 WHERE id=$1")
                    .bind(s.made.id)
                    .bind(s.rig.world.root)
                    .execute(&mut *tx)
                    .await?;
            }
            "cancel" => {
                jobs::request_cancel_by(
                    &mut tx,
                    s.parent.job(),
                    Some(s.rig.world.ada),
                    s.rig.world.clock.now(),
                )
                .await?;
            }
            "null" => {
                jobs::merge_plan(
                    &mut tx,
                    s.made.job.id,
                    &json!({"creator_provision":null}),
                    s.rig.world.clock.now(),
                )
                .await?;
            }
            _ => {}
        }
        tx.commit().await?;
        assert_eq!(
            s.current().await?,
            change == "live",
            "existing native target {change}"
        );
        s.engine.stop();
    }
    Ok(())
}

#[tokio::test]
async fn a_committed_wake_cannot_be_adopted_by_the_existing_target_retainer() -> Proof {
    let s = Child::with_existing(true).await?;
    let mut tx = s.rig.world.pool.begin().await?;
    assert!(
        orch_engine::jobs::provision::retain_fork_wake_origin(
            &mut tx,
            &s.parent,
            s.made.id,
            s.made.job.id,
            s.rig.world.clock.as_ref(),
            s.rig.world.access.as_ref()
        )
        .await
        .is_err()
    );
    tx.rollback().await?;
    assert!(s.current().await?);
    s.engine.stop();
    Ok(())
}
