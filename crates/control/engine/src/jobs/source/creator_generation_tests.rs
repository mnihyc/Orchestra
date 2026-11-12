//! First activation hands the accepted generation to its exact owned candidate, never to a replacement.
use super::*;
use crate::{EngineParts, test_common::World};
use orch_access_contract::key_scope::{KeyAction, KeyFacet, KeyTargetKind};
use orch_db::queries::environments::hosting as env_rows;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    world: World,
    engine: Engine,
    job: Job,
    runtime: Id,
    thread: Id,
    plan: Plan,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        let world = World::observed().await;
        let thread = world.thread("Owned initial environment activation").await;
        world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: crate::test_common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let mut tx = world.pool.begin().await?;
        sqlx::query("UPDATE environments SET generation=0,status='provisioning',machine_id=NULL WHERE id=$1")
            .bind(world.environment).execute(&mut *tx).await?;
        let parent = crate::operation_scope::targets::node(&mut tx, world.topic).await?;
        let caller = Caller::session(world.ada)
            .admit_facets(&[KeyFacet {
                operation: "ThreadCreate".into(),
                action: KeyAction::Write,
                resource_kind: KeyTargetKind::Object(ObjectKind::Thread),
                target: parent,
            }])?
            .into_caller();
        let mut unit = Unit::new(&mut tx, world.clock.now());
        freeze(
            &mut unit,
            &caller,
            thread,
            world.environment,
            "web",
            Some(true),
            None,
        )
        .await?;
        unit.finish().await?;
        let (host, runtime) = (new_id(), new_id());
        sqlx::query("INSERT INTO hosts(id,machine_id,name) VALUES($1,$2,'Initial candidate host')")
            .bind(host)
            .bind(world.machine_id)
            .execute(&mut *tx)
            .await?;
        let template: Id = sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
            .bind(world.environment)
            .fetch_one(&mut *tx)
            .await?;
        env_rows::insert_runtime(
            &mut tx,
            &env_rows::NewRuntime {
                id: runtime,
                host_id: host,
                unit_name: "owned-initial-candidate",
                environment_id: world.environment,
                driver: "test",
                template_id: template,
                spec_hash: &[0; 32],
                generation: 1,
                volumes: &json!({}),
            },
            world.clock.now(),
        )
        .await?;
        sqlx::query("UPDATE runtimes SET machine_id=$2 WHERE id=$1")
            .bind(runtime)
            .bind(world.machine_id)
            .execute(&mut *tx)
            .await?;
        let row = rows::insert(
            &mut tx,
            &NewJob {
                id: new_id(),
                kind: kind::ENVIRONMENT_PROVISION,
                node_id: None,
                environment_id: Some(world.environment),
                provider_id: None,
                plan: &json!({"mode":"make","generation":0,"runtime":runtime}),
                progress: &json!(initial_progress(&PHASES)),
                created_by: Some(world.ada),
            },
            world.clock.now(),
        )
        .await?;
        let runner = new_id();
        assert!(
            rows::claim(
                &mut tx,
                row.id,
                runner,
                world.clock.now() + chrono::Duration::hours(1),
                world.clock.now()
            )
            .await?
        );
        let owner = rows::job(&mut tx, row.id).await?.ok_or("job absent")?;
        let mut current = threads::thread(&mut tx, thread)
            .await?
            .ok_or("thread absent")?;
        let value = current.source.as_mut().ok_or("source absent")?;
        let origin = creator_origin(value)?.ok_or("creator origin absent")?;
        let repository: RepositorySource = serde_json::from_value(value["source"].clone())?;
        let plan = Plan {
            environment: world.environment,
            generation: 1,
            machine: world.machine_id,
            thread: Some(thread),
            caller: world.ada,
            token: None,
            creator_origin: Some(origin),
            scheduled_input: None,
            action: Action::Prepare {
                directory: "web".into(),
                source: repository,
                credential: None,
                destination: SourceDestination::MissingDirectory,
                effect_id: Some(crate::ids::derived(
                    "source.provision.files",
                    &[owner.id.as_bytes(), thread.as_bytes()],
                )),
            },
        };
        value["native_plan"] = json!(plan);
        value["info"]["state"] = json!("ready");
        value["info"]["job"] = json!(reference(&owner));
        source::set_source(&mut tx, thread, value, current.seq_state).await?;
        rows::merge_plan(
            &mut tx,
            row.id,
            &json!({"file_publications":{(thread.to_string()):true}}),
            world.clock.now(),
        )
        .await?;
        tx.commit().await?;
        let job = Job::claimed(engine.clone(), row.id, runner);
        Ok(Self {
            world,
            engine,
            job,
            runtime,
            thread,
            plan,
        })
    }

    async fn generation(&self) -> TestResult<i32> {
        let mut conn = self.world.pool.acquire().await?;
        let row = threads::thread(&mut conn, self.thread)
            .await?
            .ok_or("thread absent")?;
        Ok(
            accepted_creator_generation(row.source.as_ref().ok_or("source absent")?)?
                .ok_or("generation absent")?,
        )
    }

    async fn publish(&self, conn: &mut Conn) -> TestResult {
        assert!(
            env_rows::make_running(
                conn,
                self.world.environment,
                &["provisioning"],
                &env_rows::Up {
                    machine: self.world.machine_id,
                    workspace_root: None,
                    facts: None,
                    new_unit: true
                },
                self.world.clock.now()
            )
            .await?
        );
        assert!(
            env_rows::set_runtime_state(
                conn,
                self.runtime,
                &["creating"],
                "running",
                None,
                self.world.clock.now()
            )
            .await?
        );
        Ok(())
    }
}

#[tokio::test]
async fn only_the_owned_first_candidate_may_publish_the_creator_generation() -> TestResult {
    let s = Fixture::new().await?;
    assert_eq!(s.generation().await?, 0);
    let mut tx = s.world.pool.begin().await?;
    let owner = s.job.lock_owned(&mut tx).await?.ok_or("claim absent")?;
    assert!(
        matching_target(&mut tx, &s.plan, Some((&owner, s.runtime)))
            .await?
            .is_some()
    );
    check_at(
        &mut tx,
        s.job.id(),
        Time::new(s.world.clock.now()),
        &s.plan,
        false,
        Some(s.runtime),
    )
    .await
    .map_err(|error| format!("first candidate should retain accepted binding: {error:?}"))?;
    s.publish(&mut tx).await?;
    assert_eq!(
        publish_creator_generation(
            &mut tx,
            &s.job,
            s.world.environment,
            s.runtime,
            s.world.machine_id,
            0
        )
        .await?,
        1
    );
    tx.commit().await?;
    assert_eq!(s.generation().await?, 1);
    assert_eq!(s.world.commands.issued_of("fs.stat"), 0);
    assert_eq!(s.world.commands.issued_of("git.prepare"), 0);
    let mut tx = s.world.pool.begin().await?;
    // The same commit cannot advance a creator that has already accepted its actual activation.
    assert_eq!(
        publish_creator_generation(
            &mut tx,
            &s.job,
            s.world.environment,
            s.runtime,
            s.world.machine_id,
            0
        )
        .await?,
        0
    );
    tx.rollback().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn another_runtime_claim_or_later_generation_cannot_advance_the_accepted_source() -> TestResult
{
    let s = Fixture::new().await?;
    let mut tx = s.world.pool.begin().await?;
    s.publish(&mut tx).await?;
    assert!(
        publish_creator_generation(
            &mut tx,
            &s.job,
            s.world.environment,
            new_id(),
            s.world.machine_id,
            0
        )
        .await
        .is_err()
    );
    tx.rollback().await?;
    assert_eq!(s.generation().await?, 0);
    let mut tx = s.world.pool.begin().await?;
    s.publish(&mut tx).await?;
    let wrong = Job::claimed(s.engine.clone(), s.job.id(), new_id());
    assert!(
        publish_creator_generation(
            &mut tx,
            &wrong,
            s.world.environment,
            s.runtime,
            s.world.machine_id,
            0
        )
        .await
        .is_err()
    );
    tx.rollback().await?;
    assert_eq!(s.generation().await?, 0);
    let mut tx = s.world.pool.begin().await?;
    s.publish(&mut tx).await?;
    assert_eq!(
        publish_creator_generation(
            &mut tx,
            &s.job,
            s.world.environment,
            s.runtime,
            s.world.machine_id,
            1
        )
        .await?,
        0
    );
    tx.rollback().await?;
    assert_eq!(s.generation().await?, 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn an_unready_or_foreign_source_is_left_unchanged_without_blocking_activation() -> TestResult
{
    for mismatch in ["unready", "different_source_attempt", "scheduled"] {
        let s = Fixture::new().await?;
        let mut tx = s.world.pool.begin().await?;
        s.publish(&mut tx).await?;
        let mut current = threads::thread(&mut tx, s.thread)
            .await?
            .ok_or("thread absent")?;
        let value = current.source.as_mut().ok_or("source absent")?;
        match mismatch {
            "unready" => value["info"]["state"] = json!("failed"),
            "different_source_attempt" => {
                value["native_plan"]["action"]["effect_id"] = json!(new_id())
            }
            "scheduled" => value["scheduled_input"] = json!(new_id()),
            _ => return Err("unknown mismatch".into()),
        }
        source::set_source(&mut tx, s.thread, value, current.seq_state).await?;
        assert_eq!(
            publish_creator_generation(
                &mut tx,
                &s.job,
                s.world.environment,
                s.runtime,
                s.world.machine_id,
                0
            )
            .await?,
            0
        );
        let published = environments::workspace::environment(&mut tx, s.world.environment)
            .await?
            .ok_or("environment absent")?;
        assert_eq!(published.status, "running");
        assert_eq!(published.generation, 1);
        tx.commit().await?;
        assert_eq!(
            s.generation().await?,
            0,
            "{mismatch} never receives a new accepted generation"
        );
        s.engine.stop();
    }
    Ok(())
}
