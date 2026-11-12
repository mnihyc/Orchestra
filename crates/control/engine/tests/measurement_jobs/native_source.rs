//! Real native fork-source measurement claims, workers and completed observations.
use super::*;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyAction, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    fork_sources,
    forks::{ForkRequest, ThreadForkPreflight},
    routes::Endpoint,
};
use orch_authz::TokenGrant;
use orch_engine::{
    Engine, EngineParts,
    fork_plan::{
        Reach,
        operation::{self as source, Preview},
    },
    jobs::measurement::{Preparation, Request},
};
use orch_link_contract::commands::{NodeRuntimeVolumesMeasure, SelectedVolume};
use orch_types::{Clock as _, Hash, Id, ManualClock, Time, new_id};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const OP: &str = "ThreadForkPreflight.fork";
struct Fixture {
    world: World,
    clock: std::sync::Arc<ManualClock>,
    host: ScriptedHost,
    _unit: hosting::Unit,
    engine: Engine,
    source: Preparation,
    token: Id,
    thread: Id,
    body: serde_json::Value,
    fork: ForkRequest,
}
impl Fixture {
    async fn new() -> Proof<Self> {
        let clock = std::sync::Arc::new(ManualClock::new(orch_testkit::clock::start()));
        let world = World::with_clock(clock.clone()).await;
        serve(&world);
        let host = ScriptedHost::on(&world, "Measurement source", 16_384).await;
        let template = template(&world).await;
        let (environment, runtime, unit) =
            running(&world, &host, template, "Measured source").await;
        let thread = world.thread("Native measurement origin").await;
        bind(&world, thread, environment).await;
        world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: clock.clone(),
            config: common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let token = new_id();
        let hash = Hash::of(token.as_bytes());
        let expires = Time::new(clock.now() + chrono::Duration::hours(1));
        let policy = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Org),
                    id: Some(world.root),
                    reach: KeyReach::Subtree,
                },
                resources: None,
                allow: vec![KeyOperation {
                    operation: OP.into(),
                    actions: vec![KeyAction::Read, KeyAction::Manage],
                }],
            }],
            ..KeyScope::default()
        };
        assert!(policy.validate().is_empty());
        let mut tx = world.pool.begin().await?;
        orch_db::queries::identity::create_api_token_scoped(
            &mut tx,
            &orch_db::queries::identity::NewApiToken {
                id: token,
                user_id: world.ada,
                name: "Native count source",
                token_hash: hash.as_bytes(),
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(expires.get()),
            },
            Some(&json!(policy)),
        )
        .await?;
        let caller = Caller::token(
            world.ada,
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
            ThreadForkPreflight::ROUTE,
            &fork_sources::PREFLIGHT_FORK,
            &[orch_engine::operation_scope::sources::Facts::one(
                fork_sources::PlanSource::Thread,
                target.object.kind,
                target,
            )],
        )?;
        let caller = known.preparation_caller(&caller)?;
        let request: ForkRequest = serde_json::from_value(
            json!({"result":"new","history":{"mode":"none"},"environment":{"new":{"template_id":template,"host_id":host.id,"files":{"mode":"none"},"home":true,"private_views":false}}}),
        )?;
        let selected = engine
            .prepare_fork_in(&mut tx, &caller, thread, &request, Reach::Seen, None, None)
            .await?;
        let input = source::Input::selected(&mut tx, &selected).await?;
        let facts = source::facts::<Preview>(&mut tx, &input).await?;
        let bound = orch_engine::operation_scope::sources::bind_required(
            ThreadForkPreflight::ROUTE,
            &fork_sources::PREFLIGHT_FORK,
            &facts,
        )?;
        let caller = bound.admit(&caller, false)?;
        let origin = source::Origin::capture::<Preview>(
            &caller,
            input,
            orch_engine::operation_scope::request_origin::Credential::Token {
                id: token,
                hash: hash.as_bytes().to_vec(),
            },
            Some(expires),
        )?;
        let runtime_row = orch_db::queries::environments::hosting::runtime(&mut tx, runtime)
            .await?
            .ok_or("runtime missing")?;
        let body = serde_json::to_value(NodeRuntimeVolumesMeasure {
            work: None,
            runtime_id: runtime,
            volumes: vec![SelectedVolume {
                selector: orch_driver_contract::volumes::OwnedVolumeSelector::Home,
                volume: serde_json::from_value(runtime_row.volumes["home"].clone())?,
            }],
        })?;
        tx.commit().await?;
        let source = Preparation::from_fork(&source::Preparation::new(origin))?;
        Ok(Self {
            world,
            clock,
            host,
            _unit: unit,
            engine,
            source,
            token,
            thread,
            body,
            fork: request,
        })
    }
    fn request(&self) -> Request {
        Request {
            machine: self.host.machine,
            kind: CommandKind::NodeRuntimeVolumesMeasure,
            body: self.body.clone(),
            subject: json!({"name":"Native home"}),
            by: self.world.ada,
            reuse_unsupported: false,
        }
    }
    async fn claim(&self) -> Proof<orch_engine::jobs::measurement::Count> {
        Ok(self
            .engine
            .measure_with_preparation(self.request(), &self.source)
            .await?)
    }

    async fn ordinary(&self) -> Proof<(Request, orch_api_contract::forks::Measurement)> {
        let draft = self
            .engine
            .draft_fork(&Caller::session(self.world.ada), self.thread, &self.fork)
            .await?;
        let reference = draft
            .plan
            .files
            .ok_or("native files missing")?
            .home
            .ok_or("native home missing")?
            .measurement
            .ok_or("native reference missing")?;
        let row =
            orch_db::queries::jobs::job(&mut *self.world.pool.acquire().await?, reference.job_id)
                .await?
                .ok_or("ordinary count missing")?;
        assert!(row.plan.get("measurement_source").is_none());
        let request = Request {
            machine: serde_json::from_value(row.plan["machine"].clone())?,
            kind: row.plan["command_kind"]
                .as_str()
                .ok_or("kind missing")?
                .parse()?,
            body: row.plan["body"].clone(),
            subject: row.plan["subject"].clone(),
            by: self.world.ada,
            reuse_unsupported: row.plan["measurement_optional_home"] == json!(true),
        };
        Ok((request, reference))
    }
    async fn optional_home(&self) -> Proof<(Request, orch_api_contract::forks::Measurement)> {
        let target = ScriptedHost::on(&self.world, "Optional home destination", 16_384).await;
        let mut conn = self.world.pool.acquire().await?;
        let runtime: Id = serde_json::from_value(self.body["runtime_id"].clone())?;
        let environment: Id = sqlx::query_scalar("SELECT environment_id FROM runtimes WHERE id=$1")
            .bind(runtime)
            .fetch_one(&mut *conn)
            .await?;
        let root: String =
            sqlx::query_scalar("SELECT workspace_root FROM environments WHERE id=$1")
                .bind(environment)
                .fetch_one(&mut *conn)
                .await?;
        let store = std::sync::Arc::new(
            orch_blob_local::LocalBlobStore::open(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../.tmp/measurement-observation-blobs")
                    .join(new_id().to_string()),
            )
            .await?,
        );
        self.engine.attach_blobs(store);
        let header = json!({"v":1,"root":root,"generated_at":self.clock.time(),"generation":1,"case_sensitive":true});
        let manifest = self
            .engine
            .put_manifest(
                &mut conn,
                zstd::stream::encode_all(format!("{header}\n").as_bytes(), 0)?,
            )
            .await?;
        self._unit
            .added
            .machine
            .handler
            .on(CommandKind::FsManifest, move |command| {
                let body = command.body().unwrap();
                assert_eq!(body["environment_id"], json!(environment));
                Ok(orch_link_contract::commands::CommandResult::FsManifest(
                    orch_link_contract::commands::FsManifestResult {
                        manifest_blob: manifest.hash,
                        file_count: 0,
                        total_bytes: 0,
                    },
                ))
            });
        let principal = orch_authz::Principal::load(
            &mut conn,
            Caller::session(self.world.ada),
            self.clock.time(),
        )
        .await?;
        let request =
            serde_json::from_value(json!({"host_id":target.id,"home":true,"private_views":false}))?;
        let draft = self
            .engine
            .draft_move(&mut conn, &principal, environment, &request, None)
            .await?;
        let reference = draft
            .recipe
            .files
            .home
            .and_then(|home| home.measurement)
            .ok_or("optional home count missing")?;
        let row = orch_db::queries::jobs::job(&mut conn, reference.job_id)
            .await?
            .ok_or("optional count missing")?;
        let request = Request {
            machine: serde_json::from_value(row.plan["machine"].clone())?,
            kind: row.plan["command_kind"]
                .as_str()
                .ok_or("kind missing")?
                .parse()?,
            body: row.plan["body"].clone(),
            subject: row.plan["subject"].clone(),
            by: self.world.ada,
            reuse_unsupported: row.plan["measurement_optional_home"] == json!(true),
        };
        Ok((request, reference))
    }

    async fn consumer(&self, seconds: i64) -> Proof<(Preparation, Id)> {
        use orch_api_contract::forks::ThreadForkCreate;
        let operation = "ThreadForkCreate.fork";
        let token = new_id();
        let digest = Hash::of(token.as_bytes());
        let expires = Time::new(self.clock.now() + chrono::Duration::seconds(seconds));
        let policy = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Org),
                    id: Some(self.world.root),
                    reach: KeyReach::Subtree,
                },
                resources: None,
                allow: vec![KeyOperation {
                    operation: operation.into(),
                    actions: vec![KeyAction::Read, KeyAction::Write, KeyAction::Manage],
                }],
            }],
            ..KeyScope::default()
        };
        let mut tx = self.world.pool.begin().await?;
        orch_db::queries::identity::create_api_token_scoped(
            &mut tx,
            &orch_db::queries::identity::NewApiToken {
                id: token,
                user_id: self.world.ada,
                name: "Fresh observation consumer",
                token_hash: digest.as_bytes(),
                scopes: &[],
                node_id: None,
                at: self.clock.now(),
                expires_at: Some(expires.get()),
            },
            Some(&json!(policy)),
        )
        .await?;
        let caller = Caller::token(
            self.world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(policy), Some(expires))?.for_token(token),
        );
        let target = orch_engine::operation_scope::targets::page_object(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Thread,
                id: self.thread,
            },
        )
        .await?;
        let known = orch_engine::operation_scope::sources::bind_known(
            ThreadForkCreate::ROUTE,
            &fork_sources::CONFIRM_FORK,
            &[orch_engine::operation_scope::sources::Facts::one(
                fork_sources::PlanSource::Thread,
                target.object.kind,
                target,
            )],
        )?;
        let caller = known.preparation_caller(&caller)?;
        let selected = self
            .engine
            .prepare_fork_in(
                &mut tx,
                &caller,
                self.thread,
                &self.fork,
                Reach::Seen,
                None,
                None,
            )
            .await?;
        let input = source::Input::selected(&mut tx, &selected).await?;
        let facts = source::facts::<source::Confirm>(&mut tx, &input).await?;
        let plan = orch_engine::operation_scope::sources::bind_required(
            ThreadForkCreate::ROUTE,
            &fork_sources::CONFIRM_FORK,
            &facts,
        )?;
        let accepted = plan.admit(&caller, false)?;
        let origin = source::Origin::capture::<source::Confirm>(
            &accepted,
            input,
            orch_engine::operation_scope::request_origin::Credential::Token {
                id: token,
                hash: digest.as_bytes().to_vec(),
            },
            Some(expires),
        )?;
        tx.commit().await?;
        Ok((
            Preparation::from_fork(&source::Preparation::new(origin))?,
            token,
        ))
    }
}

#[tokio::test]
async fn a_source_claim_reuses_its_pending_owner_but_never_a_terminal_marker_without_a_native_answer()
-> Proof {
    let s = Fixture::new().await?;
    let first = s.claim().await?;
    assert!(first.result.is_none());
    assert_eq!(
        first.source().map(|(origin, _)| origin.operation()),
        Some(OP)
    );
    let same = s.claim().await?;
    assert_eq!(first.reference.job_id, same.reference.job_id);
    let mut reordered = s.request();
    reordered.body.sort_all_objects();
    reordered.subject.sort_all_objects();
    let same = s
        .engine
        .measure_with_preparation(reordered, &s.source)
        .await?;
    assert_eq!(
        first.reference.job_id, same.reference.job_id,
        "JSON object order cannot create another native count"
    );
    let row =
        orch_db::queries::jobs::job(&mut *s.world.pool.acquire().await?, first.reference.job_id)
            .await?
            .ok_or("count missing")?;
    assert_eq!(
        row.plan["measurement_source"]["origin"]["operation"],
        json!(OP)
    );
    assert!(
        row.plan["measurement_source"]["observations"]
            .as_object()
            .is_some_and(|o| o.is_empty())
    );
    // A terminal marker alone is deliberately forged as the negative control, never an accepted observation.
    sqlx::query("UPDATE jobs SET state='succeeded',finished_at=$2,result=$3,plan=plan || jsonb_build_object('count',$3::jsonb) WHERE id=$1").bind(first.reference.job_id).bind(s.world.clock.now()).bind(json!({"volumes":[]})).execute(s.world.pool.pool()).await?;
    let fresh = s.claim().await?;
    assert_ne!(fresh.reference.job_id, first.reference.job_id);
    assert!(fresh.result.is_none());
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn the_restarted_native_worker_rechecks_the_original_key_before_issuing_a_count() -> Proof {
    for change in [
        "live", "scope", "expired", "source", "null", "missing", "version",
    ] {
        let s = Fixture::new().await?;
        let count = s.claim().await?;
        match change {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(json!({"rules":[]}))
                    .execute(s.world.pool.pool())
                    .await?;
            }
            "expired" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(s.world.clock.now() - chrono::Duration::seconds(1))
                    .execute(s.world.pool.pool())
                    .await?;
            }
            "source" => {
                sqlx::query("UPDATE runtimes SET volumes=jsonb_set(volumes,'{home,name}','\"changed-after-claim\"') WHERE id=($1::jsonb->>'runtime_id')::uuid").bind(s.body.clone()).execute(s.world.pool.pool()).await?;
            }
            "null" => {
                sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{measurement_source}','null') WHERE id=$1").bind(count.reference.job_id).execute(s.world.pool.pool()).await?;
            }
            "missing" => {
                sqlx::query("UPDATE jobs SET plan=plan-'measurement_source' WHERE id=$1")
                    .bind(count.reference.job_id)
                    .execute(s.world.pool.pool())
                    .await?;
            }
            "version" => {
                sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{measurement_source_version}','99') WHERE id=$1").bind(count.reference.job_id).execute(s.world.pool.pool()).await?;
            }
            _ => {}
        }
        s.engine.start();
        let (state, _, error) = hosting::wait_over(&s.world, count.reference.job_id).await;
        if change == "live" {
            assert_eq!(state, "succeeded", "{error:?}");
            let complete = s.claim().await?;
            assert_eq!(complete.reference.job_id, count.reference.job_id);
            assert!(complete.reference.measured_at.is_some());
            assert!(complete.result.is_some());
            let completed = orch_db::queries::jobs::job(
                &mut *s.world.pool.acquire().await?,
                count.reference.job_id,
            )
            .await?
            .ok_or("completed count missing")?;
            assert!(completed.plan["active_command"].is_null());
            assert!(completed.plan["measurement_observation"]["completed"].is_object());
            let observed = s.engine.measure_observation(s.request(), &s.source).await?;
            for _ in 0..3 {
                assert_eq!(
                    observed.replay(&s.request(), &s.source)?.reference.job_id,
                    count.reference.job_id
                );
            }
            let mut substituted = s.request();
            substituted.body["runtime_id"] = json!(new_id());
            assert!(observed.replay(&substituted, &s.source).is_err());
            assert_eq!(s.host.began(CommandKind::NodeRuntimeVolumesMeasure), 1);
        } else {
            assert_eq!(state, "failed", "{change}: {error:?}");
            assert_eq!(
                s.host.began(CommandKind::NodeRuntimeVolumesMeasure),
                0,
                "{change}"
            );
        }
        s.engine.stop();
    }
    Ok(())
}

#[tokio::test]
async fn expiry_during_the_real_measurement_claim_wait_refuses_pending_reuse() -> Proof {
    let s = std::sync::Arc::new(Fixture::new().await?);
    let first = s.claim().await?;
    let row =
        orch_db::queries::jobs::job(&mut *s.world.pool.acquire().await?, first.reference.job_id)
            .await?
            .ok_or("count missing")?;
    let key = row.plan["measurement_key"]
        .as_str()
        .ok_or("identity missing")?;
    let mut held = s.world.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(key)
        .execute(&mut *held)
        .await?;
    let task = {
        let s = s.clone();
        tokio::spawn(async move { s.claim().await })
    };
    common::eventually("the actual source-aware count claim waits on its identity",||async {
        let mut conn=s.world.pool.acquire().await.unwrap();
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE '%pg_advisory_xact_lock(hashtextextended%')").fetch_one(&mut *conn).await.unwrap()
    }).await;
    s.clock.advance(chrono::Duration::hours(2));
    held.commit().await?;
    assert!(task.await?.is_err());
    assert_eq!(s.host.began(CommandKind::NodeRuntimeVolumesMeasure), 0);
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='measurement'")
        .fetch_one(s.world.pool.pool())
        .await?;
    assert_eq!(
        pending, 1,
        "the refused waiter creates no second native claim"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn pending_reuse_retains_an_accepted_narrower_policy_when_the_key_is_later_broadened() -> Proof
{
    let s = Fixture::new().await?;
    let first = s.claim().await?;
    let broad: serde_json::Value =
        sqlx::query_scalar("SELECT operation_scope FROM api_tokens WHERE id=$1")
            .bind(s.token)
            .fetch_one(s.world.pool.pool())
            .await?;
    let narrow = KeyScope {
        rules: vec![KeyRule {
            target: KeyTarget {
                kind: KeyTargetKind::Object(ObjectKind::Thread),
                id: Some(s.thread),
                reach: KeyReach::SelfOnly,
            },
            resources: None,
            allow: vec![KeyOperation {
                operation: OP.into(),
                actions: vec![KeyAction::Manage],
            }],
        }],
        ..KeyScope::default()
    };
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(narrow))
        .execute(s.world.pool.pool())
        .await?;
    assert_eq!(s.claim().await?.reference.job_id, first.reference.job_id);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(broad)
        .execute(s.world.pool.pool())
        .await?;
    let retained = s.claim().await?;
    assert_eq!(retained.reference.job_id, first.reference.job_id);
    let other = s
        .world
        .thread("Outside the retained source restriction")
        .await;
    let mut conn = s.world.pool.acquire().await?;
    let caller = retained
        .source()
        .ok_or("source minimum missing")?
        .0
        .restore_current(&mut conn, s.clock.as_ref())
        .await?
        .ok_or("live original key missing")?;
    let target = orch_engine::operation_scope::targets::page_object(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Thread,
            id: other,
        },
    )
    .await?;
    assert!(
        !caller.permits_facets(&[orch_access_contract::key_scope::KeyFacet {
            operation: OP.into(),
            action: KeyAction::Manage,
            resource_kind: target.object.kind,
            target
        }])
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn completed_ordinary_observation_is_data_for_a_new_current_operation_without_adopting_the_job()
-> Proof {
    let s = Fixture::new().await?;
    let (request, pending) = s.ordinary().await?;
    s.engine.start();
    let (state, _, error) = hosting::wait_over(&s.world, pending.job_id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    s.engine.stop();
    let (consumer, key) = s.consumer(3600).await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.token)
        .bind(s.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    let before = orch_db::queries::jobs::job(&mut *s.world.pool.acquire().await?, pending.job_id)
        .await?
        .ok_or("count missing")?;
    // Persist the real issued frame and actual native result, then consume its read answer.
    // The worker produced the witness from this actual frame and returned native result.
    let receipt: Id = serde_json::from_value(
        before.plan["measurement_observation"]["answers"]["measure"]["command"].clone(),
    )?;
    let (machine, wire) = s
        .world
        .commands
        .issued_commands("node.runtime.volumes.measure")
        .into_iter()
        .find(|(_, wire)| wire.id == receipt)
        .ok_or("actual native frame missing")?;
    let mut conn = s.world.pool.acquire().await?;
    orch_db::queries::link::insert_command(
        &mut conn,
        &orch_db::queries::link::NewCommand {
            id: wire.id,
            machine_id: machine,
            kind: &wire.kind,
            body: &wire.body,
            serial_key: wire.key.as_deref(),
            deadline_at: wire.deadline.get(),
        },
        s.clock.now(),
    )
    .await?;
    let native_result = before.result.as_ref().ok_or("native result missing")?;
    assert!(
        orch_db::queries::link::finish_command(
            &mut conn,
            receipt,
            &orch_db::queries::link::Ending::Succeeded(native_result),
            s.clock.now()
        )
        .await?
    );
    assert!(orch_db::queries::link::forget_result(&mut conn, receipt).await?);
    drop(conn);
    let issued = s.world.commands.issued_of("node.runtime.volumes.measure");
    let observed = s
        .engine
        .completed_measurement_observation(&request, &pending, &consumer)
        .await?
        .ok_or("genuine completed observation was not readable")?;
    let count = observed.replay(&request, &consumer)?;
    assert!(observed.witness().is_some());
    assert_eq!(count.reference.job_id, pending.job_id);
    assert_eq!(
        count.reference.measured_at.map(|at| at.get()),
        before.finished_at.map(Time::new).map(Time::get)
    );
    assert_eq!(count.result, before.result);
    assert_eq!(
        count.source().map(|(origin, _)| origin.operation()),
        Some("ThreadForkCreate.fork")
    );
    let mut conn = s.world.pool.acquire().await?;
    let restored = count
        .source()
        .ok_or("fresh source missing")?
        .0
        .restore_current(&mut conn, s.clock.as_ref())
        .await?
        .ok_or("fresh key ended")?;
    let ids = restored
        .grant()
        .ok_or("consumer became a session")?
        .token_ids()
        .collect::<Vec<_>>();
    assert!(ids.contains(&key));
    assert!(!ids.contains(&s.token));
    let after = orch_db::queries::jobs::job(&mut conn, pending.job_id)
        .await?
        .ok_or("count missing")?;
    assert_eq!(
        after.plan, before.plan,
        "a factual reader changed the old source or keepalive"
    );
    assert!(after.plan.get("measurement_source").is_none());
    assert_eq!(after.created_by, before.created_by);
    assert_eq!(
        s.world.commands.issued_of("node.runtime.volumes.measure"),
        issued
    );
    assert!(
        observed.replay(&s.request(), &consumer).is_err(),
        "different native request reused the observation"
    );
    Ok(())
}

#[tokio::test]
async fn completed_observation_reader_refuses_pending_forged_changed_and_expired_facts_without_new_io()
-> Proof {
    for change in [
        "pending",
        "marker",
        "body",
        "result",
        "time",
        "witness",
        "scope",
        "placement",
        "expired_data",
    ] {
        let s = Fixture::new().await?;
        let (request, reference) = s.ordinary().await?;
        if change != "pending" && change != "marker" {
            s.engine.start();
            assert_eq!(
                hosting::wait_over(&s.world, reference.job_id).await.0,
                "succeeded"
            );
            s.engine.stop();
        }
        let (consumer, key) = s.consumer(3600).await?;
        match change {
            "marker" => {
                sqlx::query("UPDATE jobs SET state='succeeded',finished_at=$2,result=$3,plan=plan||jsonb_build_object('count',$3::jsonb) WHERE id=$1").bind(reference.job_id).bind(s.clock.now()).bind(json!({"volumes":[]})).execute(s.world.pool.pool()).await?;
            }
            "body" => {
                sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{body,runtime_id}',$2::jsonb) WHERE id=$1").bind(reference.job_id).bind(json!(new_id())).execute(s.world.pool.pool()).await?;
            }
            "result" => {
                sqlx::query("UPDATE jobs SET result=$2,plan=plan||jsonb_build_object('count',$2::jsonb) WHERE id=$1").bind(reference.job_id).bind(json!({"volumes":[]})).execute(s.world.pool.pool()).await?;
            }
            "time" => {
                sqlx::query(
                    "UPDATE jobs SET finished_at=finished_at-interval '1 second' WHERE id=$1",
                )
                .bind(reference.job_id)
                .execute(s.world.pool.pool())
                .await?;
            }
            "witness" => {
                sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{measurement_observation}','null'::jsonb) WHERE id=$1").bind(reference.job_id).execute(s.world.pool.pool()).await?;
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key)
                    .bind(json!({"rules":[]}))
                    .execute(s.world.pool.pool())
                    .await?;
            }
            "placement" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=(SELECT environment_id FROM runtimes WHERE id=$1)").bind(serde_json::from_value::<Id>(s.body["runtime_id"].clone())?).execute(s.world.pool.pool()).await?;
            }
            "expired_data" => s.clock.advance(chrono::Duration::minutes(30)),
            _ => {}
        }
        let issued = s.world.commands.issued_of("node.runtime.volumes.measure");
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='measurement'")
            .fetch_one(s.world.pool.pool())
            .await?;
        let result = s
            .engine
            .completed_measurement_observation(&request, &reference, &consumer)
            .await;
        assert!(!matches!(result, Ok(Some(_))), "{change}");
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='measurement'")
            .fetch_one(s.world.pool.pool())
            .await?;
        assert_eq!(after, before, "{change} created a replacement count");
        assert_eq!(
            s.world.commands.issued_of("node.runtime.volumes.measure"),
            issued,
            "{change}"
        );
        s.engine.stop();
    }
    Ok(())
}

#[tokio::test]
async fn completed_observation_rechecks_the_new_source_after_the_real_measurement_row_wait() -> Proof
{
    for expire in [false, true] {
        let s = std::sync::Arc::new(Fixture::new().await?);
        let (request, reference) = s.ordinary().await?;
        s.engine.start();
        assert_eq!(
            hosting::wait_over(&s.world, reference.job_id).await.0,
            "succeeded"
        );
        s.engine.stop();
        let (consumer, _) = s.consumer(5).await?;
        let mut held = s.world.pool.begin().await?;
        sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR UPDATE")
            .bind(reference.job_id)
            .fetch_one(&mut *held)
            .await?;
        let (pid_send, pid_recv) = tokio::sync::oneshot::channel();
        let future = {
            let s = s.clone();
            tokio::spawn(async move {
                let mut tx = s.world.pool.begin().await?;
                let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(&mut *tx)
                    .await?;
                let _ = pid_send.send(pid);
                let result = s
                    .engine
                    .completed_measurement_observation_on(&mut tx, &request, &reference, &consumer)
                    .await;
                tx.commit().await?;
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(result)
            })
        };
        let pid = pid_recv.await?;
        common::eventually("the exact completed-observation reader waits on its real row",||async {
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock' AND query LIKE '%FROM jobs WHERE id=$1 FOR SHARE%')").bind(pid).fetch_one(s.world.pool.pool()).await.unwrap()
        }).await;
        if expire {
            s.clock.advance(chrono::Duration::seconds(6));
        }
        held.commit().await?;
        let result = future.await??;
        assert_eq!(matches!(result, Ok(Some(_))), !expire);
    }
    Ok(())
}

#[tokio::test]
async fn completed_optional_home_unsupported_keeps_its_native_refusal_and_never_becomes_a_zero_count()
-> Proof {
    let s = Fixture::new().await?;
    let (request, reference) = s.optional_home().await?;
    assert!(request.reuse_unsupported);
    // Planning may already have an ordinary home count queued. This node lacks the
    // capability, rather than refusing whichever different job happens to execute first.
    s.host.answer(CommandKind::NodeRuntimeVolumesMeasure, |_| {
        Err(orch_link_contract::commands::CommandError::new(
            orch_link_contract::commands::ErrorCode::Unsupported,
            "the native home source cannot be counted",
        ))
    });
    s.engine.start();
    let (state, _, error) = hosting::wait_over(&s.world, reference.job_id).await;
    assert_eq!(state, "failed", "{error:?}");
    s.engine.stop();
    let (consumer, _) = s.consumer(3600).await?;
    let observed = s
        .engine
        .completed_measurement_observation(&request, &reference, &consumer)
        .await?
        .ok_or("native unsupported witness missing")?;
    let count = observed.replay(&request, &consumer)?;
    assert!(count.unsupported);
    assert!(count.result.is_none());
    assert!(count.reference.measured_at.is_none());
    assert_eq!(count.reference.job_id, reference.job_id);
    let mut required = request.clone();
    required.reuse_unsupported = false;
    assert!(
        s.engine
            .completed_measurement_observation(&required, &reference, &consumer)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn completed_fingerprint_keeps_both_native_frames_and_its_original_comparison_identity_and_time()
-> Proof {
    use orch_db::queries::resources::mounts::{self as mounts, LocationRow, MountRow};
    use orch_link_contract::commands::{NodeMountFingerprint, NodeMountFingerprintResult};
    let s = Fixture::new().await?;
    let mount = new_id();
    let runtime: Id = serde_json::from_value(s.body["runtime_id"].clone())?;
    let home: Id = sqlx::query_scalar("SELECT e.home_node_id FROM runtimes r JOIN environments e ON e.id=r.environment_id WHERE r.id=$1").bind(runtime).fetch_one(s.world.pool.pool()).await?;
    let mut conn = s.world.pool.acquire().await?;
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,mounts}','\"placed\"'::jsonb) WHERE id=$1").bind(s.host.id).execute(&mut *conn).await?;
    mounts::insert(
        &mut conn,
        &MountRow {
            id: mount,
            name: "observed-fingerprint".into(),
            description: String::new(),
            org_id: Some(s.world.root),
            home_id: Some(s.world.root),
            owner_id: Some(s.world.ada),
            target: "/data/observed".into(),
            read_only: true,
            enabled: true,
            default_fate: "as_is".into(),
            revision: 1,
        },
        s.clock.now(),
    )
    .await?;
    orch_authz::ownership::start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Mount,
            id: mount,
        },
        s.world.ada,
        orch_authz::ownership::NewAccess::OnlyMe,
        Time::new(s.clock.now()),
    )
    .await?;
    mounts::set_location(
        &mut conn,
        &LocationRow {
            mount_id: mount,
            machine_id: s.host.machine,
            host_id: Some(s.host.id),
            host_name: Some("Measurement source".into()),
            kind: "path".into(),
            source: "/srv/observed-fingerprint".into(),
            storage: None,
            data_kind: None,
            created_by: s.world.ada,
            created_at: s.clock.now(),
        },
    )
    .await?;
    drop(conn);
    s.world
        .set(home, "env.mounts", json!({(mount.to_string()):true}))
        .await;
    s.host
        .added
        .machine
        .handler
        .on(CommandKind::NodeMountFingerprint, |cmd| {
            let request: NodeMountFingerprint =
                serde_json::from_value(cmd.body().unwrap()).unwrap();
            Ok(
                orch_link_contract::commands::CommandResult::NodeMountFingerprint(
                    NodeMountFingerprintResult {
                        fingerprint: Hash::of(&serde_json::to_vec(&request.data).unwrap()),
                    },
                ),
            )
        });
    let (consumer, _) = s.consumer(3600).await?;
    let body = NodeMountFingerprint {
        work: None,
        driver: "docker".into(),
        mount_id: mount,
        data: orch_engine::mount_copies::DataRef {
            kind: "path".into(),
            source: "/srv/observed-fingerprint".into(),
            storage: None,
        }
        .link(),
    };
    let request = Request {
        machine: s.host.machine,
        kind: CommandKind::NodeMountFingerprint,
        body: json!({"request_id":new_id(),"source":body,"target_machine":s.host.machine,"target":body}),
        subject: json!({"mount":mount}),
        by: s.world.ada,
        reuse_unsupported: false,
    };
    let pending = s
        .engine
        .measure_with_preparation(request.clone(), &consumer)
        .await?;
    s.engine.start();
    assert_eq!(
        hosting::wait_over(&s.world, pending.reference.job_id)
            .await
            .0,
        "succeeded"
    );
    s.engine.stop();
    let observed = s
        .engine
        .completed_measurement_observation(&request, &pending.reference, &consumer)
        .await?
        .ok_or("fingerprint witness missing")?;
    let original = observed.replay(&request, &consumer)?;
    s.clock.advance(chrono::Duration::minutes(1));
    let again = s
        .engine
        .completed_measurement_observation(&request, &pending.reference, &consumer)
        .await?
        .ok_or("completed comparison was restarted or retimed")?;
    assert_eq!(again.witness(), observed.witness());
    assert_eq!(again.replay(&request, &consumer)?.result, original.result);
    assert_eq!(
        again.replay(&request, &consumer)?.reference.measured_at,
        original.reference.measured_at
    );
    let mut changed = request.clone();
    changed.body["request_id"] = json!(new_id());
    assert!(
        s.engine
            .completed_measurement_observation(&changed, &pending.reference, &consumer)
            .await?
            .is_none()
    );
    assert_eq!(s.host.began(CommandKind::NodeMountFingerprint), 2);
    Ok(())
}

#[tokio::test]
async fn a_pending_native_reference_stays_the_same_under_a_new_create_source_without_claim_or_io()
-> Proof {
    use orch_engine::jobs::measurement::ReferenceObservation;
    let s = Fixture::new().await?;
    let original = s.claim().await?;
    let request = s.request();
    let (consumer, _) = s.consumer(3600).await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.token)
        .bind(s.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    let before = orch_db::queries::jobs::job(
        &mut *s.world.pool.acquire().await?,
        original.reference.job_id,
    )
    .await?
    .ok_or("pending job missing")?;
    let read = s
        .engine
        .measurement_reference_observation(&request, &original.reference, &consumer)
        .await?
        .ok_or("matching pending native reference missing")?;
    assert_eq!(read.reference().job_id, original.reference.job_id);
    let ReferenceObservation::Pending(pending) = read else {
        return Err("pending marker became a completed observation".into());
    };
    let count = pending.replay(&request, &consumer)?;
    assert_eq!(count.reference, original.reference);
    assert!(count.result.is_none());
    assert!(!count.unsupported);
    assert_eq!(
        count.source().map(|(origin, _)| origin.operation()),
        Some("ThreadForkCreate.fork")
    );
    let after = orch_db::queries::jobs::job(
        &mut *s.world.pool.acquire().await?,
        original.reference.job_id,
    )
    .await?
    .ok_or("pending job missing")?;
    assert_eq!(before.plan, after.plan);
    assert_eq!(before.created_by, after.created_by);
    assert_eq!(s.host.began(CommandKind::NodeRuntimeVolumesMeasure), 0);
    assert!(
        s.engine
            .completed_measurement_observation(&request, &original.reference, &consumer)
            .await?
            .is_none()
    );
    Ok(())
}
#[tokio::test]
async fn pending_reference_reader_distinguishes_unrelated_claims_and_never_promotes_an_unproven_marker()
-> Proof {
    let s = Fixture::new().await?;
    let (request, reference) = s.ordinary().await?;
    let (consumer, token) = s.consumer(3600).await?;
    let mut different = request.clone();
    different.body["runtime_id"] = json!(new_id());
    assert!(
        s.engine
            .measurement_reference_observation(&different, &reference, &consumer)
            .await?
            .is_none()
    );
    let mut wrong = reference;
    wrong.job_id = new_id();
    assert!(
        s.engine
            .measurement_reference_observation(&request, &wrong, &consumer)
            .await?
            .is_none()
    );
    wrong = reference;
    wrong.measured_at = Some(Time::new(s.clock.now()));
    assert!(
        s.engine
            .measurement_reference_observation(&request, &wrong, &consumer)
            .await?
            .is_none()
    );
    sqlx::query(
        "UPDATE jobs SET plan=plan || jsonb_build_object('measurement_key','forged') WHERE id=$1",
    )
    .bind(reference.job_id)
    .execute(s.world.pool.pool())
    .await?;
    assert!(
        s.engine
            .measurement_reference_observation(&request, &reference, &consumer)
            .await?
            .is_none()
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token)
        .bind(json!({"rules":[]}))
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        s.engine
            .measurement_reference_observation(&request, &reference, &consumer)
            .await
            .is_err()
    );
    assert_eq!(s.host.began(CommandKind::NodeRuntimeVolumesMeasure), 0);
    Ok(())
}
#[tokio::test]
async fn pending_reference_becomes_complete_only_after_the_same_native_job_answers() -> Proof {
    use orch_engine::jobs::measurement::ReferenceObservation;
    let s = Fixture::new().await?;
    let (request, reference) = s.ordinary().await?;
    let (consumer, _) = s.consumer(3600).await?;
    let pending = s
        .engine
        .measurement_reference_observation(&request, &reference, &consumer)
        .await?
        .ok_or("pending claim missing")?;
    assert!(matches!(pending, ReferenceObservation::Pending(_)));
    s.engine.start();
    let (state, _, error) = hosting::wait_over(&s.world, reference.job_id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    s.engine.stop();
    let complete = s
        .engine
        .measurement_reference_observation(&request, &reference, &consumer)
        .await?
        .ok_or("native completed observation missing")?;
    let ReferenceObservation::Complete(observation) = complete else {
        return Err("native answer stayed pending".into());
    };
    let count = observation.replay(&request, &consumer)?;
    assert_eq!(count.reference.job_id, reference.job_id);
    assert!(count.reference.measured_at.is_some());
    assert!(count.result.is_some());
    assert!(observation.witness().is_some());
    assert_eq!(s.host.began(CommandKind::NodeRuntimeVolumesMeasure), 1);
    Ok(())
}

#[tokio::test]
async fn pending_reference_rechecks_the_new_source_after_the_actual_saved_job_lock() -> Proof {
    for expire in [false, true] {
        let s = Fixture::new().await?;
        let (request, reference) = s.ordinary().await?;
        let (consumer, _) = s.consumer(5).await?;
        let mut held = s.world.pool.begin().await?;
        sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR UPDATE")
            .bind(reference.job_id)
            .execute(&mut *held)
            .await?;
        let (engine, clock, pool) = (s.engine.clone(), s.clock.clone(), s.world.pool.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut conn = pool.begin().await?;
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *conn)
                .await?;
            tx.send(pid).map_err(|_| "PID observer ended")?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                engine
                    .measurement_reference_observation_on(
                        &mut conn, &request, &reference, &consumer,
                    )
                    .await,
            )
        });
        let pid = rx.await?;
        common::eventually("the pending-reference reader waits on the saved native job",||async {
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock' AND query LIKE '%FROM jobs WHERE id=$1 FOR SHARE%')")
                .bind(pid).fetch_one(s.world.pool.pool()).await.unwrap()
        }).await;
        if expire {
            clock.advance(chrono::Duration::seconds(6));
        }
        held.commit().await?;
        let result = task.await??;
        assert_eq!(
            matches!(
                result,
                Ok(Some(
                    orch_engine::jobs::measurement::ReferenceObservation::Pending(_)
                ))
            ),
            !expire
        );
        assert_eq!(s.host.began(CommandKind::NodeRuntimeVolumesMeasure), 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_genuine_measurement_keeps_an_exact_database_completion_seal_while_public_references_use_milliseconds()
-> Proof {
    use chrono::Timelike;
    let s = Fixture::new().await?;
    let (request, reference) = s.ordinary().await?;
    s.engine.start();
    assert_eq!(
        hosting::wait_over(&s.world, reference.job_id).await.0,
        "succeeded"
    );
    s.engine.stop();
    let (consumer, _) = s.consumer(3600).await?;
    let observed = s
        .engine
        .completed_measurement_observation(&request, &reference, &consumer)
        .await?
        .ok_or("genuine completion seal missing")?;
    let row = orch_db::queries::jobs::job(&mut *s.world.pool.acquire().await?, reference.job_id)
        .await?
        .ok_or("completed row missing")?;
    let at = row.finished_at.ok_or("completion time missing")?;
    let exact: chrono::DateTime<chrono::Utc> =
        serde_json::from_value(row.plan["measurement_observation"]["completed"]["at"].clone())?;
    assert_eq!(
        exact, at,
        "the private seal retains every persisted database microsecond"
    );
    let count = observed.replay(&request, &consumer)?;
    assert_eq!(count.reference.measured_at, Some(Time::new(at)));
    assert!(
        s.engine
            .completed_measurement_observation(&request, &count.reference, &consumer)
            .await?
            .is_some(),
        "a native public measured_at remains a valid reference to that exact sealed job"
    );
    // Change one database microsecond inside the SAME public millisecond. A rounded private seal would accept this forgery.
    let delta = if at.nanosecond() / 1_000 % 1_000 == 999 {
        -1
    } else {
        1
    };
    let changed = at + chrono::Duration::microseconds(delta);
    assert_eq!(Time::new(changed), Time::new(at));
    sqlx::query("UPDATE jobs SET finished_at=$2 WHERE id=$1")
        .bind(reference.job_id)
        .bind(changed)
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        s.engine
            .completed_measurement_observation(&request, &count.reference, &consumer)
            .await?
            .is_none()
    );
    Ok(())
}
