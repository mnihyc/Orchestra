//! A pending provider birth keeps its original key and its exact native command identities.
use super::*;
use crate::commands::Outgoing;
use crate::jobs::provider_provision::{Mode, ProviderProvision};
use crate::jobs::runner::{Failure, Job, Stop, Undone};
use crate::operation_scope::targets;
use crate::{
    Engine, EngineParts,
    test_common::{Added, World},
};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyAction, KeyFacet, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind,
};
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_authz::ownership::{self, NewAccess};
use orch_authz::{Caller, CapturedOperation, TokenGrant};
use orch_db::queries::resources::provider_runtime as provider_rows;
use orch_db::queries::{identity, jobs};
use orch_link_contract::commands::{
    CommandKind, CommandResult, Empty, NodeImageEnsure, NodeImageEnsureResult, SerialKey,
};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    world: World,
    engine: Engine,
    job: Job,
    token: Id,
    origin: CapturedOperation,
    policy: KeyScope,
    runtime: Id,
    host_machine: Added,
    placement_home: Id,
    source: Option<Id>,
    copy_keeper: Option<Added>,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        Self::with_legacy(false).await
    }

    async fn legacy_unplaced() -> TestResult<Self> {
        Self::with_legacy(true).await
    }

    async fn with_legacy(legacy: bool) -> TestResult<Self> {
        Self::with_kind(legacy, false).await
    }
    async fn with_kind(legacy: bool, duplicate: bool) -> TestResult<Self> {
        Self::with_transfer(legacy, duplicate, false).await
    }
    async fn with_transfer(legacy: bool, duplicate: bool, physical: bool) -> TestResult<Self> {
        Self::with_intent(legacy, duplicate, physical, false).await
    }
    async fn moving(physical: bool) -> TestResult<Self> {
        Self::with_intent(false, true, physical, true).await
    }
    async fn with_intent(
        legacy: bool,
        duplicate: bool,
        physical: bool,
        moving: bool,
    ) -> TestResult<Self> {
        let world = World::on_a_key("synthetic-provider-creator-fixture").await;
        let host_machine = world
            .add_machine("Provider creator physical host", &["node"])
            .await;
        let copy_keeper = if physical {
            Some(
                world
                    .add_machine("Actual owner copy keeper", &["provider"])
                    .await,
            )
        } else {
            None
        };
        world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: crate::test_common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let (token, host, template, runtime) = (new_id(), new_id(), new_id(), new_id());
        let expires = world.clock.now() + chrono::Duration::hours(1);
        let source = duplicate.then(new_id);
        let mut policy = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Org),
                    id: Some(world.root),
                    reach: KeyReach::SelfOnly,
                },
                resources: Some(vec![KeyTargetKind::Object(ObjectKind::Provider)]),
                allow: vec![KeyOperation {
                    operation: "ProviderCreate".into(),
                    actions: vec![KeyAction::Manage],
                }],
            }],
            ..KeyScope::default()
        };
        if let Some(source) = source {
            policy.rules[0].allow[0].operation = "ProviderDuplicate".into();
            policy.rules[0].allow[0].actions = vec![KeyAction::Write];
            policy.rules.push(KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Provider),
                    id: Some(source),
                    reach: KeyReach::SelfOnly,
                },
                resources: Some(vec![KeyTargetKind::Object(ObjectKind::Provider)]),
                allow: vec![KeyOperation {
                    operation: "ProviderDuplicate".into(),
                    actions: vec![KeyAction::Read],
                }],
            });
        }
        let scopes = if legacy {
            vec!["provider.manage".to_owned()]
        } else {
            vec![]
        };
        let subtree = legacy.then_some(world.root);
        let mut tx = world.pool.begin().await?;
        if let Some(keeper) = &copy_keeper {
            sqlx::query("UPDATE machines SET owner_id=$2,home_id=$3,org_id=$3 WHERE id=$1")
                .bind(keeper.id)
                .bind(world.ada)
                .bind(world.root)
                .execute(&mut *tx)
                .await?;
            ownership::start_resource(
                &mut tx,
                ObjectRef {
                    kind: ResourceKind::Machine.into(),
                    id: keeper.id,
                },
                world.ada,
                NewAccess::OnlyMe,
                world.clock.time(),
            )
            .await?;
        }
        if let Some(source) = source {
            orch_db::queries::resources::routes::insert_provider(
                &mut tx,
                &orch_db::queries::resources::routes::NewProvider {
                    id: source,
                    name: "Actual copied source",
                    owner_id: world.ada,
                    home_id: world.root,
                    upstreams: &json!([{"protocol":"openai-responses","base_url":"https://api.example.test/v1","options":{}}]),
                    protocols: &["openai-responses".into()],
                    machine_id: copy_keeper.as_ref().map(|keeper|keeper.id),
                    key_machine_id: copy_keeper.as_ref().map(|keeper|keeper.id),
                    arrangements: &["separated".into()],
                    runtime_config: &json!({}),
                    status: "needs_setup",
                },
                world.clock.now(),
            )
            .await?;
            ownership::start_resource(
                &mut tx,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: source,
                },
                world.ada,
                NewAccess::OnlyMe,
                world.clock.time(),
            )
            .await?;
        }
        if moving {
            let outside = new_id();
            orch_db::queries::hierarchy::testing::insert_node(
                &mut tx,
                outside,
                "org",
                None,
                "Move source",
                world.ada,
            )
            .await?;
            orch_db::queries::access::testing::insert_member(&mut tx, outside, world.ada, outside)
                .await?;
            orch_db::queries::access::testing::name_owner(
                &mut tx,
                outside,
                ("user", world.ada),
                world.clock.now(),
            )
            .await?;
            sqlx::query("UPDATE providers SET home_id=$2,org_id=$2 WHERE id=$1")
                .bind(source.ok_or("move source absent")?)
                .bind(outside)
                .execute(&mut *tx)
                .await?;
            for (index, rule) in policy.rules.iter_mut().enumerate() {
                rule.allow[0].operation = "ProviderMoveOut".into();
                rule.allow[0].actions = vec![if index == 0 {
                    KeyAction::Write
                } else {
                    KeyAction::Manage
                }];
            }
        }
        let operation_scope = (!legacy).then(|| policy.clone());
        let stored_policy = operation_scope.as_ref().map(|scope| json!(scope));
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: token,
                user_id: world.ada,
                name: "Pending provider birth",
                token_hash: Hash::of(token.as_bytes()).as_bytes(),
                scopes: &scopes,
                node_id: subtree,
                at: world.clock.now(),
                expires_at: Some(expires),
            },
            stored_policy.as_ref(),
        )
        .await?;
        let target = targets::node(&mut tx, world.root).await?;
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(
                &scopes,
                subtree,
                operation_scope,
                Some(Time::new(expires)),
            )?
            .for_token(token),
        );
        let caller = if let Some(source) = source {
            use orch_api_contract::{
                duplicate_sources::{PROVIDER, Source},
                duplicates::ProviderDuplicate,
                routes::Endpoint,
            };
            let subject = targets::page_object(
                &mut tx,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: source,
                },
            )
            .await?;
            crate::operation_scope::sources::bind_required(
                if moving {
                    orch_api_contract::duplicates::ProviderMoveOut::ROUTE
                } else {
                    ProviderDuplicate::ROUTE
                },
                if moving {
                    &orch_api_contract::duplicate_sources::PROVIDER_MOVE
                } else {
                    &PROVIDER
                },
                &[
                    crate::operation_scope::sources::Facts::one(
                        Source::Subject,
                        KeyTargetKind::Object(ObjectKind::Provider),
                        subject,
                    ),
                    crate::operation_scope::sources::Facts::one(
                        Source::Destination,
                        KeyTargetKind::Object(ObjectKind::Provider),
                        target,
                    ),
                ],
            )?
            .admit(&caller, false)?
        } else {
            caller
                .admit_facets(&[KeyFacet {
                    operation: "ProviderCreate".into(),
                    action: KeyAction::Manage,
                    resource_kind: KeyTargetKind::Object(ObjectKind::Provider),
                    target,
                }])?
                .into_caller()
        };
        let origin = CapturedOperation::capture(&caller)?;
        let placement_home = if legacy {
            let outside = new_id();
            orch_db::queries::hierarchy::testing::insert_node(
                &mut tx,
                outside,
                "org",
                None,
                "Placement",
                world.ada,
            )
            .await?;
            orch_db::queries::access::testing::insert_member(&mut tx, outside, world.ada, outside)
                .await?;
            orch_db::queries::access::testing::name_owner(
                &mut tx,
                outside,
                ("user", world.ada),
                world.clock.now(),
            )
            .await?;
            outside
        } else {
            world.root
        };
        sqlx::query("INSERT INTO hosts(id,machine_id,name,owner_id,org_id,home_id,drivers,capacity) VALUES($1,$2,'Creator provider host',$3,$4,$4,$5,$6)")
            .bind(host).bind(host_machine.id).bind(world.ada).bind(placement_home)
            .bind(json!({"test":{"spec_schemas":{"provider":{}},"sources":["image"]}}))
            .bind(json!({"memory_mb":4096})).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO env_templates(id,name,owner_id,org_id,home_id,kind,driver,spec) VALUES($1,'Creator provider template',$2,$3,$3,'provider','test',$4)")
            .bind(template).bind(world.ada).bind(placement_home)
            .bind(json!({"image":"synthetic-provider-image"})).execute(&mut *tx).await?;
        for (kind, id) in [
            (ResourceKind::Host, host),
            (ResourceKind::Template, template),
        ] {
            ownership::start_resource(
                &mut tx,
                ObjectRef {
                    kind: kind.into(),
                    id,
                },
                world.ada,
                NewAccess::OnlyMe,
                Time::new(world.clock.now()),
            )
            .await?;
        }
        sqlx::query("UPDATE providers SET machine_id=NULL,key_machine_id=NULL,status='needs_setup',runtime_config=$2 WHERE id=$1")
            .bind(world.provider)
            .bind(json!({"runtime.template_id":template,"runtime.host_id":host}))
            .execute(&mut *tx).await?;
        if physical {
            orch_db::queries::settings::delete_secret(
                &mut tx,
                "provider",
                world.provider,
                "api_key",
            )
            .await?;
        }
        let row = match super::super::queue(
            &mut tx,
            world.clock.now(),
            world.provider,
            Mode::Make,
            Some(world.ada),
        )
        .await?
        {
            jobs::provider_jobs::Queued::Made(row) => row,
            _ => return Err("fresh provider Make was not queued".into()),
        };
        if let Some(source) = source {
            let raw = crate::operation_scope::http_origin::HttpOrigin::capture(
                origin.clone(),
                crate::operation_scope::request_origin::Credential::Token {
                    id: token,
                    hash: Hash::of(token.as_bytes()).as_bytes().to_vec(),
                },
                false,
                false,
                Some(Time::new(expires)),
            )?;
            if moving {
                let source_row = orch_db::queries::resources::routes::providers(&mut tx, &[source])
                    .await?
                    .pop()
                    .ok_or("actual source gone")?;
                let fingerprint =
                    crate::providers::deleting::fingerprint(&mut tx, &source_row).await?;
                retain_move(
                    &mut tx,
                    &raw,
                    (source, fingerprint),
                    world.provider,
                    world.root,
                    row.id,
                    world.clock.as_ref(),
                )
                .await?;
            } else {
                retain_duplicate(
                    &mut tx,
                    &raw,
                    source,
                    world.provider,
                    world.root,
                    row.id,
                    world.clock.as_ref(),
                )
                .await?;
            }
        } else {
            retain(
                &mut tx,
                &origin,
                world.provider,
                world.root,
                row.id,
                Time::new(world.clock.now()),
            )
            .await?;
        }
        // The retained birth comes first. Its exact ordinary scheduler candidate is then recorded;
        // no old or unrelated runtime is attached to establish the accepted creator origin.
        // The legacy case leaves that step to the actual schedule method under test.
        if !legacy {
            provider_rows::insert_runtime(
                &mut tx,
                &provider_rows::NewRuntime {
                    id: runtime,
                    host_id: host,
                    unit_name: "provider-creator-candidate",
                    provider_id: world.provider,
                    driver: "test",
                    template_id: template,
                    spec_hash: &[0; 32],
                    generation: 1,
                    volumes: &json!({}),
                },
                world.clock.now(),
            )
            .await?;
            assert!(
                orch_db::queries::resources::hosting::adopt_machine(
                    &mut tx,
                    world.machine_id,
                    runtime,
                    world.clock.now()
                )
                .await?
            );
            jobs::merge_plan(
                &mut tx,
                row.id,
                &json!({"runtime":runtime,"host":host,"fresh":true}),
                world.clock.now(),
            )
            .await?;
        }
        let runner = new_id();
        assert!(
            jobs::claim(
                &mut tx,
                row.id,
                runner,
                world.clock.now() + chrono::Duration::hours(1),
                world.clock.now()
            )
            .await?
        );
        tx.commit().await?;
        let job = Job::claimed(engine.clone(), row.id, runner);
        Ok(Self {
            world,
            engine,
            job,
            token,
            origin,
            policy,
            runtime,
            host_machine,
            placement_home,
            source,
            copy_keeper,
        })
    }

    async fn subscription() -> TestResult<Self> {
        let s = Self::moving(true).await?;
        let source = s.source.ok_or("moving source absent")?;
        let mut tx = s.world.pool.begin().await?;
        // The subscription's real profile needs its own current Use authority. The transport
        // world's owner_id attributes that profile without creating an Owners entry.
        ownership::start_resource(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Agent,
                id: s.world.agent,
            },
            s.world.ada,
            NewAccess::OnlyMe,
            s.world.clock.time(),
        )
        .await?;
        sqlx::query("UPDATE providers SET kind='subscription',harness_kind='fake',agent_id=$2,upstreams='[]',protocols=ARRAY[]::text[] WHERE id=ANY($1)").bind(vec![source,s.world.provider]).bind(s.world.agent).execute(&mut *tx).await?;
        let mut descriptor = crate::test_common::descriptor();
        descriptor["credentials"] = json!({"issues":[{"form":"fake_login","custody":"transferred","lifetime_secs":null}],"accepts":["fake_login"]});
        sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
            .bind(s.world.agent)
            .bind(descriptor)
            .execute(&mut *tx)
            .await?;
        for key in [orch_settings_contract::registry::keys::RUNTIME_HOME] {
            let mut config: Json =
                sqlx::query_scalar("SELECT runtime_config FROM providers WHERE id=$1")
                    .bind(s.world.provider)
                    .fetch_one(&mut *tx)
                    .await?;
            config[key] = json!({"kind":"made_by_orch"});
            sqlx::query("UPDATE providers SET runtime_config=$2 WHERE id=$1")
                .bind(s.world.provider)
                .bind(config)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("UPDATE providers SET credential_home=$2 WHERE id=$1")
            .bind(s.world.provider)
            .bind(json!({"kind":"made_by_orch"}))
            .execute(&mut *tx)
            .await?;
        let owner = jobs::job(&mut tx, s.job.id()).await?.ok_or("job absent")?;
        let mut authority = read(&owner)?.ok_or("creator absent")?;
        authority.kind = "subscription".into();
        authority.harness = Some("fake".into());
        authority.agent = Some(s.world.agent);
        let target = provider_rows::state(&mut tx, s.world.provider)
            .await?
            .ok_or("target absent")?;
        authority.runtime_config_hash = body_hash(&target.runtime_config)?;
        let source = orch_db::queries::resources::routes::providers(&mut tx, &[source])
            .await?
            .pop()
            .ok_or("source absent")?;
        authority
            .duplicate
            .as_mut()
            .ok_or("source proof absent")?
            .moving = Some(crate::providers::deleting::fingerprint(&mut tx, &source).await?);
        jobs::merge_plan(
            &mut tx,
            s.job.id(),
            &json!({FIELD:authority}),
            s.world.clock.now(),
        )
        .await?;
        tx.commit().await?;
        Ok(s)
    }

    async fn schedule(&mut self) -> TestResult {
        let provision = ProviderProvision::of(self.engine.clone(), &self.job.row().await?);
        provision
            .schedule(&self.job)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let mut conn = self.world.pool.acquire().await?;
        self.runtime = provider_rows::live_runtime(&mut conn, self.world.provider)
            .await?
            .ok_or("scheduled provider candidate absent")?
            .id;
        Ok(())
    }

    fn image(&self) -> TestResult<Outgoing> {
        Ok(Outgoing::new(
            new_id(),
            self.host_machine.id,
            SerialKey::Image("synthetic-provider-image".into()),
            &NodeImageEnsure {
                driver: "test".into(),
                spec: json!({"image":"synthetic-provider-image"}),
                job_id: Some(self.job.id()),
            },
        )?)
    }

    async fn reserve(&self, command: &Outgoing) -> TestResult {
        self.job.remember(&json!({"active_command":{
            "id":command.id,"machine":command.machine,"kind":command.kind.as_str(),"phase":"image"
        }})).await?;
        remember_command(&self.job, command)
            .await
            .map_err(|error| format!("{error:?}"))?;
        Ok(())
    }

    async fn queue_copy(&self, mut command: Outgoing) -> TestResult<Outgoing> {
        // The transport double completes stored rows but does not create them. Keep the real
        // queued frame before dispatch, so only the native answer can become a copy receipt.
        let deadline = command.deadline.unwrap_or_else(|| {
            Time::new(
                self.world.clock.now()
                    + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            )
        });
        command.deadline = Some(deadline);
        self.reserve(&command).await?;
        let mut tx = self.world.pool.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let key = command.key.wire();
        assert!(
            command_authorized(
                &mut tx,
                command.id,
                command.machine,
                key.as_deref(),
                command.kind.as_str(),
                &command.body,
                self.world.clock.as_ref(),
            )
            .await?,
            "the actual owner copy must pass its first queue boundary"
        );
        orch_db::queries::link::insert_command(
            &mut tx,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: key.as_deref(),
                deadline_at: deadline.get(),
            },
            self.world.clock.now(),
        )
        .await?;
        tx.commit().await?;
        Ok(command)
    }

    async fn allows(&self, command: &Outgoing) -> TestResult<bool> {
        let mut tx = self.world.pool.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        // Production first-send reads the retained command through JSONB, including key order.
        let body: Json = sqlx::query_scalar("SELECT $1::jsonb")
            .bind(&command.body)
            .fetch_one(&mut *tx)
            .await?;
        let allowed = command_authorized(
            &mut tx,
            command.id,
            command.machine,
            command.key.wire().as_deref(),
            command.kind.as_str(),
            &body,
            self.world.clock.as_ref(),
        )
        .await?;
        tx.rollback().await?;
        Ok(allowed)
    }
}

#[tokio::test]
async fn queued_provider_birth_intersects_its_snapshot_with_current_scope_and_revocation()
-> TestResult {
    let s = Fixture::new().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let before = s.job.row().await?.plan;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(KeyScope::default()))
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        !s.allows(&command).await?,
        "the captured creator cannot override narrower current scope"
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(s.policy))
        .execute(s.world.pool.pool())
        .await?;
    assert!(s.allows(&command).await?);
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(
        !s.allows(&command).await?,
        "queued work cannot replace the original key with the owner's session"
    );
    assert_eq!(
        s.job.row().await?.plan,
        before,
        "refusal does not rewrite retained origin or recipe"
    );
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn provider_journal_keeps_old_ids_and_refuses_changed_or_aborted_forward_frames() -> TestResult
{
    let s = Fixture::new().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    for mismatch in ["body", "machine", "key", "kind"] {
        let mut changed = command.clone();
        match mismatch {
            "body" => changed.body["spec"]["image"] = json!("different-provider-image"),
            "machine" => changed.machine = new_id(),
            "key" => changed.key = SerialKey::Image("another-image".into()),
            "kind" => changed.kind = CommandKind::NodeRuntimeStart,
            _ => unreachable!(),
        }
        assert!(!s.allows(&changed).await?, "{mismatch}");
    }
    let mut rewritten = command.clone();
    rewritten.body["spec"]["image"] = json!("rewritten");
    assert!(remember_command(&s.job, &rewritten).await.is_err());
    assert!(
        s.allows(&command).await?,
        "failed same-ID rewrite preserves original evidence"
    );
    let mut tx = s.world.pool.begin().await?;
    sqlx::query("UPDATE jobs SET plan=plan-'provider_creator' WHERE id=$1")
        .bind(s.job.id())
        .execute(&mut *tx)
        .await?;
    assert!(
        !command_authorized(
            &mut tx,
            command.id,
            command.machine,
            command.key.wire().as_deref(),
            command.kind.as_str(),
            &command.body,
            s.world.clock.as_ref()
        )
        .await?,
        "retained command evidence without its creator origin must not become ordinary work"
    );
    tx.rollback().await?;
    assert!(s.allows(&command).await?);
    let next = s.image()?;
    s.reserve(&next).await?;
    let mut tx = s.world.pool.begin().await?;
    jobs::merge_plan(
        &mut tx,
        s.job.id(),
        &json!({"cancel":true}),
        s.world.clock.now(),
    )
    .await?;
    assert!(
        !command_authorized(
            &mut tx,
            next.id,
            next.machine,
            next.key.wire().as_deref(),
            next.kind.as_str(),
            &next.body,
            s.world.clock.as_ref()
        )
        .await?,
        "an owned cancellation independently refuses the next queued frame"
    );
    tx.rollback().await?;
    assert!(
        s.allows(&next).await?,
        "rollback leaves the same unmodified accepted command permitted"
    );
    assert!(
        !s.allows(&command).await?,
        "an older queued ID is found and fails its active fence"
    );
    assert!(s.allows(&next).await?);
    s.job.remember(&json!({"active_command":null})).await?;
    assert!(
        !s.allows(&next).await?,
        "clearing the pointer does not launder the retained ID"
    );
    s.reserve(&next).await?;
    let why = Stop::Failed(Failure::new(
        "creator_authority",
        "The pending provider birth was stopped.",
    ));
    s.job.begin_undo(&why).await?.ok_or("undo claim absent")?;
    assert!(
        !s.allows(&next).await?,
        "failed undo also stops old forward frames without requiring cancel"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn provider_birth_marker_cannot_be_moved_to_another_job_or_existing_runtime() -> TestResult {
    let s = Fixture::new().await?;
    let mut tx = s.world.pool.begin().await?;
    assert!(
        retain(
            &mut tx,
            &s.origin,
            new_id(),
            s.world.root,
            s.job.id(),
            Time::new(s.world.clock.now())
        )
        .await
        .is_err()
    );
    assert!(
        retain(
            &mut tx,
            &s.origin,
            s.world.provider,
            s.world.root,
            new_id(),
            Time::new(s.world.clock.now())
        )
        .await
        .is_err()
    );
    tx.rollback().await?;
    sqlx::query("UPDATE jobs SET state='failed',finished_at=$2 WHERE id=$1")
        .bind(s.job.id())
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    let mut tx = s.world.pool.begin().await?;
    let next = match super::super::queue(
        &mut tx,
        s.world.clock.now(),
        s.world.provider,
        Mode::Make,
        Some(s.world.ada),
    )
    .await?
    {
        jobs::provider_jobs::Queued::Made(row) => row,
        _ => return Err("replacement test job not queued".into()),
    };
    assert!(
        retain(
            &mut tx,
            &s.origin,
            s.world.provider,
            s.world.root,
            next.id,
            Time::new(s.world.clock.now())
        )
        .await
        .is_err(),
        "a new Make for an already existing unit is not this creator's first birth"
    );
    tx.rollback().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn revoked_provider_creator_refuses_new_work_while_owned_make_undo_removes_its_unit()
-> TestResult {
    let s = Fixture::new().await?;
    let forward = s.image()?;
    s.reserve(&forward).await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(!s.allows(&forward).await?);
    assert!(s.job.command(s.image()?).await.is_err());
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    let why = Stop::Failed(Failure::new(
        "creator_authority",
        "The creator credential was revoked.",
    ));
    s.job.begin_undo(&why).await?.ok_or("undo claim absent")?;
    s.host_machine
        .machine
        .handler
        .on(CommandKind::NodeRuntimeRemove, |_| {
            Ok(CommandResult::NodeRuntimeRemove(Empty {}))
        });
    let provision = ProviderProvision::of(s.engine.clone(), &s.job.row().await?);
    assert!(matches!(
        provision.undo_make(&s.job, &why).await,
        Undone::Everything
    ));
    let issued = s.world.commands.issued_commands("node.runtime.remove");
    assert_eq!(issued.len(), 1);
    let (machine, actual) = &issued[0];
    assert_eq!(
        actual.id,
        crate::ids::derived("node.runtime.remove", &[s.job.id().as_bytes(), b"undo"])
    );
    assert_eq!(actual.body["runtime_id"], json!(s.runtime));
    let mut tx = s.world.pool.begin().await?;
    assert!(
        command_authorized(
            &mut tx,
            actual.id,
            *machine,
            actual.key.as_deref(),
            &actual.kind,
            &actual.body,
            s.world.clock.as_ref()
        )
        .await?,
        "existing owned compensation has a disjoint command identity"
    );
    tx.rollback().await?;
    assert!(
        !s.allows(&forward).await?,
        "cleanup does not reauthorize the old forward operation"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn accepted_and_terminal_provider_commands_keep_their_result_after_creator_revocation()
-> TestResult {
    let s = Fixture::new().await?;
    s.host_machine
        .machine
        .handler
        .hold(CommandKind::NodeImageEnsure);
    s.host_machine
        .machine
        .handler
        .on(CommandKind::NodeImageEnsure, |_| {
            Ok(CommandResult::NodeImageEnsure(NodeImageEnsureResult {
                digest: Some("synthetic-retained-digest".into()),
                built: None,
                programs: std::collections::BTreeMap::new(),
            }))
        });
    let command = s.image()?;
    let replay = command.clone();
    let id = command.id;
    let job = s.job.clone();
    let running = tokio::spawn(async move { job.command(command).await });
    s.host_machine.machine.handler.started(id).await;
    assert!(
        s.allows(&replay).await?,
        "the runner retained its actual outgoing native identity"
    );
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(
        !s.allows(&replay).await?,
        "future first-send work would now be refused"
    );
    // Its real native journal already accepted this identity. Reconcile that receipt, then the
    // same terminal receipt on phase replay; revocation must not fabricate a different outcome.
    s.host_machine.machine.handler.release();
    let result = running.await?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(result["digest"], json!("synthetic-retained-digest"));
    assert_eq!(
        s.job
            .command(replay)
            .await
            .map_err(|error| format!("{error:?}"))?,
        result
    );
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 1);
    assert_eq!(s.host_machine.machine.handler.runs_of(id), 1);
    assert!(
        s.job.command(s.image()?).await.is_err(),
        "a new native identity still needs current creator authority"
    );
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 1);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn unchanged_provider_command_needs_its_current_first_runtime_and_physical_host_machine()
-> TestResult {
    let s = Fixture::new().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let before = s.job.row().await?.plan;
    sqlx::query("UPDATE runtimes SET generation=2 WHERE id=$1")
        .bind(s.runtime)
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        !s.allows(&command).await?,
        "the same journal cannot authorize a later provider runtime generation"
    );
    sqlx::query("UPDATE runtimes SET generation=1 WHERE id=$1")
        .bind(s.runtime)
        .execute(s.world.pool.pool())
        .await?;
    assert!(s.allows(&command).await?);
    let mut conn = s.world.pool.acquire().await?;
    let runtime = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("provider candidate absent")?;
    drop(conn);
    sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
        .bind(runtime.host_id)
        .bind(s.world.machine_id)
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        !s.allows(&command).await?,
        "a current host remap cannot send the original queued command to its old machine"
    );
    sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
        .bind(runtime.host_id)
        .bind(s.host_machine.id)
        .execute(s.world.pool.pool())
        .await?;
    assert!(s.allows(&command).await?);
    assert_eq!(
        s.job.row().await?.plan,
        before,
        "current-binding refusals preserve the accepted origin and immutable native journal"
    );
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn revoked_provider_creator_cannot_publish_or_pass_commit() -> TestResult {
    let s = Fixture::new().await?;
    let before = s.job.row().await?;
    assert!(before.plan.get("committed").is_none());
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    let provision = ProviderProvision::of(s.engine.clone(), &before);
    let result = provision.commit(&s.job).await;
    assert!(
        matches!(&result,Err(Stop::Failed(failure)) if failure.code=="access.denied"),
        "the original creator must be checked within publication before pass_commit: {result:?}"
    );
    let after = s.job.row().await?;
    assert_eq!(after.plan, before.plan);
    assert!(after.plan.get("committed").is_none());
    assert!(after.plan.get("provider_creator_committed").is_none());
    let mut conn = s.world.pool.acquire().await?;
    let provider = provider_rows::state(&mut conn, s.world.provider)
        .await?
        .ok_or("provider absent")?;
    assert_eq!(provider.machine_id, None);
    let runtime = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("provider candidate absent")?;
    assert_eq!(runtime.id, s.runtime);
    assert_eq!(runtime.state, "creating");
    assert_eq!(s.world.commands.issued_of("node.runtime.start"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn inline_creator_login_keeps_both_credential_boundaries_and_its_original_attempt()
-> TestResult {
    use crate::provider_setup::{LoginAuthority, Pending};
    let s = Fixture::new().await?;
    let second_token = new_id();
    let expires = s.world.clock.now() + chrono::Duration::hours(1);
    let now = Time::new(s.world.clock.now());
    let mut tx = s.world.pool.begin().await?;
    identity::create_api_token_scoped(
        &mut tx,
        &identity::NewApiToken {
            id: second_token,
            user_id: s.world.ada,
            name: "Second inline creator boundary",
            token_hash: Hash::of(second_token.as_bytes()).as_bytes(),
            scopes: &[],
            node_id: None,
            at: s.world.clock.now(),
            expires_at: Some(expires),
        },
        Some(&json!(s.policy)),
    )
    .await?;
    let original = s
        .origin
        .restore(&mut tx, now)
        .await?
        .ok_or("original creator credential absent")?;
    let second = Caller::token(
        s.world.ada,
        TokenGrant::from_stored_scope(&[], None, Some(s.policy.clone()), Some(Time::new(expires)))?
            .for_token(second_token),
    );
    let target = targets::node(&mut tx, s.world.root).await?;
    let admitted = original
        .intersect_restrictions(&second)?
        .admit_facets(&[KeyFacet {
            operation: "ProviderCreate".into(),
            action: KeyAction::Manage,
            resource_kind: KeyTargetKind::Object(ObjectKind::Provider),
            target,
        }])?
        .into_caller();
    // Exercise the pending authority carrier; ordinary subscription/method validation is
    // performed by the creator producer before this continuation is retained.
    let pending = Pending::new(
        "synthetic-inline-method",
        &std::collections::BTreeMap::new(),
        s.world.ada,
    )
    .with_authority(&admitted)?;
    assert_eq!(
        pending.authority.as_ref().map(CapturedOperation::operation),
        Some("ProviderCreate")
    );
    let bytes = pending.to_bytes()?;
    let authority = pending.authority_state()?;
    provider_rows::keep_pending(&mut tx, s.world.provider, &bytes, s.world.clock.now()).await?;
    provider_rows::keep_login_authority(&mut tx, s.world.provider, authority.as_ref()).await?;
    tx.commit().await?;
    let mut conn = s.world.pool.acquire().await?;
    let active = LoginAuthority::load(&mut conn, s.world.provider)
        .await?
        .ok_or("active inline creator authority absent")?;
    assert!(active.authorized(&mut conn, s.world.provider, now).await?);
    assert!(
        identity::revoke_api_token(&mut conn, second_token, s.world.ada, s.world.clock.now())
            .await?
    );
    assert!(s.origin.restore(&mut conn, now).await?.is_some());
    assert!(
        !active.authorized(&mut conn, s.world.provider, now).await?,
        "the still-live first token cannot replace its revoked second boundary"
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=NULL WHERE id=$1")
        .bind(second_token)
        .execute(&mut *conn)
        .await?;
    assert!(active.authorized(&mut conn, s.world.provider, now).await?);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(KeyScope::default()))
        .execute(&mut *conn)
        .await?;
    assert!(
        !active.authorized(&mut conn, s.world.provider, now).await?,
        "the second live token cannot replace narrower current scope on the first boundary"
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(s.policy))
        .execute(&mut *conn)
        .await?;
    assert!(active.authorized(&mut conn, s.world.provider, now).await?);
    drop(conn);
    let replacement = Pending::new(
        "synthetic-inline-method",
        &std::collections::BTreeMap::new(),
        s.world.ada,
    )
    .with_authority(&admitted)?;
    assert_ne!(pending.attempt, replacement.attempt);
    assert_eq!(
        replacement
            .authority
            .as_ref()
            .map(CapturedOperation::operation),
        Some("ProviderCreate")
    );
    let bytes = replacement.to_bytes()?;
    let authority = replacement.authority_state()?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::keep_pending(&mut tx, s.world.provider, &bytes, s.world.clock.now()).await?;
    provider_rows::keep_login_authority(&mut tx, s.world.provider, authority.as_ref()).await?;
    tx.commit().await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        !active.authorized(&mut conn, s.world.provider, now).await?,
        "an old attempt cannot borrow the replacement's otherwise valid creator origin"
    );
    let current = LoginAuthority::load(&mut conn, s.world.provider)
        .await?
        .ok_or("replacement inline creator authority absent")?;
    assert!(current.authorized(&mut conn, s.world.provider, now).await?);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn legacy_provider_manage_keeps_its_home_boundary_while_scheduling_account_placement()
-> TestResult {
    let mut s = Fixture::legacy_unplaced().await?;
    let mut conn = s.world.pool.acquire().await?;
    let restored = s
        .origin
        .restore(&mut conn, Time::new(s.world.clock.now()))
        .await?
        .ok_or("legacy creator credential absent")?;
    assert!(restored.permits(Permission::ProviderManage));
    assert!(!restored.permits(Permission::HostUse));
    assert!(!restored.permits(Permission::TemplateUse));
    assert!(restored.reaches(Some(&[s.world.root])));
    assert!(!restored.reaches(Some(&[s.placement_home])));
    assert!(
        provider_rows::live_runtime(&mut conn, s.world.provider)
            .await?
            .is_none()
    );
    drop(conn);
    // Run the real initial scheduler: internal host/template use is the account's placement
    // decision, while the original provider.manage credential remains scoped to this home.
    s.schedule().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let before = s.job.row().await?.plan;
    let mut tx = s.world.pool.begin().await?;
    let owner = jobs::job(&mut tx, s.job.id())
        .await?
        .ok_or("legacy provider job absent")?;
    let current = principal(&mut tx, &owner, s.world.clock.as_ref())
        .await?
        .ok_or("legacy provider principal absent")?;
    assert!(
        current.caller().grant().is_some(),
        "placement must not reconstruct a browser session"
    );
    assert!(!current.caller().permits(Permission::HostUse));
    assert!(!current.caller().permits(Permission::TemplateUse));
    tx.rollback().await?;
    for boundary in ["permission", "subtree", "exclusion"] {
        let mut tx = s.world.pool.begin().await?;
        match boundary {
            "permission" => {
                sqlx::query("UPDATE api_tokens SET scopes=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(vec!["provider.view".to_owned()])
                    .execute(&mut *tx)
                    .await?;
            }
            "subtree" => {
                sqlx::query("UPDATE api_tokens SET node_id=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(s.placement_home)
                    .execute(&mut *tx)
                    .await?;
            }
            "exclusion" => {
                assert!(
                    orch_db::queries::tokens::refuse(
                        &mut tx,
                        s.token,
                        s.world.root,
                        s.world.ada,
                        s.world.clock.now()
                    )
                    .await?
                );
            }
            _ => unreachable!(),
        }
        assert!(
            !command_authorized(
                &mut tx,
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
                s.world.clock.as_ref()
            )
            .await?,
            "{boundary}"
        );
        tx.rollback().await?;
        assert!(
            s.allows(&command).await?,
            "restored {boundary} keeps the original unchanged command usable"
        );
    }
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(!s.allows(&command).await?);
    assert_eq!(s.job.row().await?.plan, before);
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn legacy_provider_placement_still_requires_current_account_provider_host_and_template_rights()
-> TestResult {
    let mut s = Fixture::legacy_unplaced().await?;
    s.schedule().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let mut conn = s.world.pool.acquire().await?;
    let runtime = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("legacy provider candidate absent")?;
    drop(conn);
    let before = s.job.row().await?.plan;
    for (kind, id, permission) in [
        (
            ResourceKind::Provider,
            s.world.provider,
            Permission::ProviderManage,
        ),
        (ResourceKind::Host, runtime.host_id, Permission::HostUse),
        (
            ResourceKind::Template,
            runtime.template_id,
            Permission::TemplateUse,
        ),
    ] {
        let mut tx = s.world.pool.begin().await?;
        let page = orch_db::queries::access::PageKey::new(kind.as_str(), id, "live");
        let entries = orch_db::queries::access::entries_of(&mut tx, &page).await?;
        assert!(
            !entries.is_empty(),
            "the fixture grants ordinary account rights on {}",
            kind.as_str()
        );
        for entry in entries {
            assert!(
                orch_db::queries::access::remove_entry(
                    &mut tx,
                    entry.id,
                    s.world.ada,
                    s.world.clock.now()
                )
                .await?
            );
        }
        orch_db::queries::access::testing::set_row(
            &mut tx,
            &page,
            "any",
            permission.as_str(),
            false,
            false,
        )
        .await?;
        assert!(
            !command_authorized(
                &mut tx,
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
                s.world.clock.as_ref()
            )
            .await?,
            "the legacy provider.manage boundary must not replace withdrawn account {}",
            permission.as_str()
        );
        tx.rollback().await?;
        assert!(s.allows(&command).await?);
    }
    assert_eq!(s.job.row().await?.plan, before);
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn copied_provider_make_keeps_its_actual_namespace_source_destination_and_raw_bearer_at_first_send()
-> TestResult {
    for change in [
        "live",
        "revoked",
        "bearer",
        "source_read",
        "destination_write",
        "namespace",
    ] {
        let s = Fixture::with_kind(false, true).await?;
        let command = s.image()?;
        s.reserve(&command).await?;
        assert!(
            s.allows(&command).await?,
            "positive native duplicate Make must be usable before each isolated withdrawal"
        );
        assert!(s.source.is_some());
        assert_eq!(s.origin.operation(), "ProviderDuplicate");
        let mut scope = s.policy.clone();
        match change {
            "live" => {}
            "revoked" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(s.world.clock.now())
                    .execute(s.world.pool.pool())
                    .await?;
            }
            "bearer" => {
                sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(
                        Hash::of(b"another synthetic raw bearer")
                            .as_bytes()
                            .to_vec(),
                    )
                    .execute(s.world.pool.pool())
                    .await?;
            }
            "source_read" => {
                scope.rules.pop();
            }
            "destination_write" => {
                scope.rules.remove(0);
            }
            "namespace" => {
                for rule in &mut scope.rules {
                    rule.allow[0].operation = "ProviderGet".into();
                    rule.allow[0].actions = vec![KeyAction::Read];
                }
            }
            _ => unreachable!(),
        }
        if matches!(change, "source_read" | "destination_write" | "namespace") {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(s.token)
                .bind(json!(scope))
                .execute(s.world.pool.pool())
                .await?;
        }
        assert_eq!(s.allows(&command).await?, change == "live", "{change}");
        assert_eq!(
            s.world.commands.issued_of("node.image.ensure"),
            0,
            "the first-send checker cannot issue or replace its native command"
        );
        s.engine.stop();
    }
    Ok(())
}

#[tokio::test]
async fn only_the_real_owners_duplicate_job_can_issue_its_exact_static_copy_to_its_actual_enrolled_recipient()
-> TestResult {
    use orch_link_contract::commands::{
        MachineKey, PROVIDER_COPY_FORM, ProviderCredentialCopy, ProviderCredentialIssue,
    };
    for change in [
        "live",
        "source",
        "target",
        "job",
        "recipient",
        "lease",
        "form",
        "scope",
        "bearer",
        "keeper_home",
    ] {
        let s = Fixture::with_transfer(false, true, true).await?;
        let owner = s.job.row().await?;
        let authority = read(&owner)?.ok_or("actual native copy authority missing")?;
        let source = s.source.ok_or("actual source missing")?;
        let keeper = s.copy_keeper.as_ref().ok_or("actual keeper missing")?.id;
        assert_eq!(
            authority
                .duplicate
                .as_ref()
                .and_then(|copy| copy.copy_keeper),
            Some(keeper)
        );
        let mut conn = s.world.pool.acquire().await?;
        let target = orch_db::queries::resources::machine(&mut conn, s.world.machine_id)
            .await?
            .ok_or("actual enrolled recipient missing")?;
        drop(conn);
        let mut body = ProviderCredentialIssue {
            return_credential: None,
            lease_id: None,
            copy: Some(ProviderCredentialCopy {
                handover: None,
                job_id: owner.id,
                source_provider_id: source,
                target_provider_id: s.world.provider,
                headers: vec![],
            }),
            provider_id: source,
            form: PROVIDER_COPY_FORM.into(),
            recipient: MachineKey {
                machine_id: target.id,
                public_key: target.link().public_key,
            },
        };
        match change {
            "source" => {
                body.copy
                    .as_mut()
                    .ok_or("copy purpose missing")?
                    .source_provider_id = new_id();
            }
            "target" => {
                body.copy
                    .as_mut()
                    .ok_or("copy purpose missing")?
                    .target_provider_id = new_id();
            }
            "job" => {
                body.copy.as_mut().ok_or("copy purpose missing")?.job_id = new_id();
            }
            "recipient" => {
                body.recipient.machine_id = new_id();
            }
            "lease" => {
                body.lease_id = Some(new_id());
            }
            "form" => {
                body.form = "api_key".into();
            }
            _ => {}
        }
        let command = Outgoing::new(new_id(), keeper, SerialKey::Provider(source), &body)?;
        if matches!(
            change,
            "source" | "target" | "job" | "recipient" | "lease" | "form"
        ) {
            assert!(s.reserve(&command).await.is_err(), "{change}");
            continue;
        }
        s.reserve(&command).await?;
        assert!(
            s.allows(&command).await?,
            "the exact actual native owner copy must be usable"
        );
        if change == "scope" {
            let mut scope = s.policy.clone();
            scope.rules.pop();
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(s.token)
                .bind(json!(scope))
                .execute(s.world.pool.pool())
                .await?;
        }
        if change == "bearer" {
            sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                .bind(s.token)
                .bind(Hash::of(b"another copy bearer").as_bytes().to_vec())
                .execute(s.world.pool.pool())
                .await?;
        }
        if change == "keeper_home" {
            sqlx::query("UPDATE machines SET home_id=NULL,org_id=NULL WHERE id=$1")
                .bind(keeper)
                .execute(s.world.pool.pool())
                .await?;
        }
        assert_eq!(s.allows(&command).await?, change == "live", "{change}");
        assert_eq!(
            s.world.commands.issued_of("provider.credential.issue"),
            0,
            "source verification cannot issue or replace native work"
        );
    }
    Ok(())
}
#[tokio::test]
async fn an_unclaimed_owner_copy_purpose_is_never_ordinary_or_autonomous_credential_work()
-> TestResult {
    use orch_link_contract::commands::{
        MachineKey, PROVIDER_COPY_FORM, ProviderCredentialCopy, ProviderCredentialIssue,
    };
    let s = Fixture::new().await?;
    let body = serde_json::to_value(ProviderCredentialIssue {
        return_credential: None,
        lease_id: None,
        copy: Some(ProviderCredentialCopy {
            handover: None,
            job_id: new_id(),
            source_provider_id: s.world.provider,
            target_provider_id: new_id(),
            headers: vec![],
        }),
        provider_id: s.world.provider,
        form: PROVIDER_COPY_FORM.into(),
        recipient: MachineKey {
            machine_id: s.world.machine_id,
            public_key: "not an enrolled recipient key".into(),
        },
    })?;
    let mut tx = s.world.pool.begin().await?;
    assert!(
        !command_authorized(
            &mut tx,
            new_id(),
            s.world.machine_id,
            None,
            "provider.credential.issue",
            &body,
            s.world.clock.as_ref()
        )
        .await?
    );
    Ok(())
}

#[tokio::test]
async fn an_actual_owner_copy_needs_its_issued_receipt_and_audits_accepted_native_effects_after_revocation()
-> TestResult {
    use orch_link_contract::commands::{
        MachineKey, PROVIDER_COPY_FORM, ProviderCredentialAccept, ProviderCredentialCopy,
        ProviderCredentialIssue, Sealed, SealedCredential,
    };
    let s = Fixture::with_transfer(false, true, true).await?;
    let source = s.source.ok_or("actual source absent")?;
    let keeper = s.copy_keeper.as_ref().ok_or("actual keeper absent")?;
    let mut conn = s.world.pool.acquire().await?;
    let recipient = orch_db::queries::resources::machine(&mut conn, s.world.machine_id)
        .await?
        .ok_or("actual enrolled recipient absent")?;
    drop(conn);
    let purpose = ProviderCredentialCopy {
        handover: None,
        job_id: s.job.id(),
        source_provider_id: source,
        target_provider_id: s.world.provider,
        headers: vec![],
    };
    // The scripted physical peer supplies an opaque native result. The connector conformance
    // case independently drives the real seal/open/store implementation with two identities.
    let sealed = SealedCredential {
        form: PROVIDER_COPY_FORM.into(),
        credential: Sealed {
            value: "synthetic-opaque-peer-result".into(),
        },
        expires_at: None,
    };
    let result = sealed.clone();
    keeper
        .machine
        .handler
        .on(CommandKind::ProviderCredentialIssue, move |_| {
            Ok(CommandResult::ProviderCredentialIssue(result.clone()))
        });
    let issue = Outgoing::new(
        new_id(),
        keeper.id,
        SerialKey::Provider(source),
        &ProviderCredentialIssue {
            return_credential: None,
            lease_id: None,
            copy: Some(purpose.clone()),
            provider_id: source,
            form: PROVIDER_COPY_FORM.into(),
            recipient: MachineKey {
                machine_id: recipient.id,
                public_key: recipient.link().public_key,
            },
        },
    )?;
    let body = ProviderCredentialAccept {
        provider_id: s.world.provider,
        copy: Some(purpose),
        form: sealed.form,
        credential: sealed.credential,
        expires_at: None,
    };
    let accept = Outgoing::new(
        new_id(),
        s.world.machine_id,
        SerialKey::Provider(s.world.provider),
        &body,
    )?;
    assert!(
        s.reserve(&accept).await.is_err(),
        "accept cannot invent a source issue receipt"
    );
    let issue = s.queue_copy(issue).await?;
    tokio::time::timeout(crate::test_common::PATIENCE, s.job.command(issue.clone()))
        .await
        .map_err(|_| "owner copy timed out waiting for the source issue receipt")?
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(s.world.commands.issued_of("provider.credential.issue"), 1);
    let mut replaced = body.clone();
    replaced.credential.value = "another opaque result".into();
    let changed = Outgoing::new(
        new_id(),
        s.world.machine_id,
        SerialKey::Provider(s.world.provider),
        &replaced,
    )?;
    assert!(
        s.reserve(&changed).await.is_err(),
        "accept must carry precisely the real issued result"
    );
    s.world
        .machine
        .handler
        .hold(CommandKind::ProviderCredentialAccept);
    s.world
        .machine
        .handler
        .on(CommandKind::ProviderCredentialAccept, |_| {
            Ok(CommandResult::ProviderCredentialAccept(Empty {}))
        });
    let accept = s.queue_copy(accept).await?;
    let replay = accept.clone();
    let job = s.job.clone();
    let mut waiting = tokio::spawn(async move { job.command(accept).await });
    tokio::select! {
        started = tokio::time::timeout(
            crate::test_common::PATIENCE,
            s.world.machine.handler.started(replay.id),
        ) => {
            if started.is_err() {
                waiting.abort();
                return Err("owner copy timed out waiting for the target accept to start".into());
            }
        }
        ended = &mut waiting => {
            return Err(format!("owner copy target accept ended before reaching its held handler: {ended:?}").into());
        }
    }
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(
        !s.allows(&replay).await?,
        "new first-send work is now refused"
    );
    s.world.machine.handler.release();
    tokio::time::timeout(crate::test_common::PATIENCE, &mut waiting)
        .await
        .map_err(|_| {
            waiting.abort();
            "owner copy timed out waiting for the accepted target receipt after revocation"
        })??
        .map_err(|error| format!("{error:?}"))?;
    // Drive the same opaque data-only ACK facade after native completion. It grants no
    // user restoration or fresh queue/send permission, and repeated ACKs audit once.
    for command in [issue.id, replay.id, issue.id, replay.id] {
        let mut tx = s.world.pool.begin().await?;
        let ack = recorded_copy(&mut tx, command)
            .await?
            .ok_or("exact native copy ACK absent")?;
        ack.lock_source(&mut tx).await?;
        ack.reconcile(&mut tx, s.world.clock.as_ref()).await?;
        tx.commit().await?;
    }
    let mut conn = s.world.pool.acquire().await?;
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE details->>'purpose'='provider_duplicate' AND details->>'job'=$1")
        .bind(s.job.id().to_string()).fetch_one(&mut *conn).await?;
    assert_eq!(
        audits, 2,
        "one source issue and one target accept, despite repeat ACK and raw revocation"
    );
    drop(conn);
    let before = s.job.row().await?.plan;
    let mut tx = s.world.pool.begin().await?;
    sqlx::query(
        "UPDATE commands SET body=jsonb_set(body,'{provider_id}',to_jsonb($2::uuid)) WHERE id=$1",
    )
    .bind(replay.id)
    .bind(new_id())
    .execute(&mut *tx)
    .await?;
    assert!(
        recorded_copy(&mut tx, replay.id).await.is_err(),
        "a changed native frame is not an ACK witness"
    );
    tx.rollback().await?;
    assert_eq!(s.job.row().await?.plan, before);
    assert!(!s.allows(&replay).await?);
    assert_eq!(s.world.commands.issued_of("provider.credential.issue"), 1);
    assert_eq!(s.world.commands.issued_of("provider.credential.accept"), 1);
    assert_eq!(keeper.machine.handler.runs_of(issue.id), 1);
    assert_eq!(s.world.machine.handler.runs_of(replay.id), 1);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn an_owner_provider_move_keeps_its_actual_source_until_the_successful_native_commit()
-> TestResult {
    let s = Fixture::moving(false).await?;
    let source = s.source.ok_or("move source absent")?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(
        s.allows(&command).await?,
        "the actual Manage/Write moving declaration admits its native Make"
    );
    assert!(
        orch_db::queries::resources::routes::providers(
            &mut *s.world.pool.acquire().await?,
            &[source]
        )
        .await?
        .len()
            == 1
    );
    let provision = ProviderProvision::of(s.engine.clone(), &s.job.row().await?);
    provision
        .commit(&s.job)
        .await
        .map_err(|error| format!("{error:?}"))?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        orch_db::queries::resources::routes::providers(&mut conn, &[source])
            .await?
            .is_empty()
    );
    assert_eq!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("copy disappeared")?
            .machine_id,
        Some(s.world.machine_id)
    );
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action=ANY($2)")
            .bind(source)
            .bind(vec!["content.copied_out", "provider.deleted"])
            .fetch_one(&mut *conn)
            .await?;
    assert_eq!(audits, 2);
    drop(conn);
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.token)
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    provision
        .commit(&s.job)
        .await
        .map_err(|error| format!("{error:?}"))?;
    s.engine.stop();
    Ok(())
}
#[tokio::test]
async fn a_provider_move_cannot_end_its_source_after_raw_closure_or_changed_reviewed_work()
-> TestResult {
    for change in ["raw", "source", "scope", "owner"] {
        let s = Fixture::moving(false).await?;
        let source = s.source.ok_or("source absent")?;
        match change {
            "raw" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(s.world.clock.now())
                    .execute(s.world.pool.pool())
                    .await?;
            }
            "source" => {
                sqlx::query("UPDATE providers SET revision=revision+1 WHERE id=$1")
                    .bind(source)
                    .execute(s.world.pool.pool())
                    .await?;
            }
            "scope" => {
                let mut scope = s.policy.clone();
                scope.rules.pop();
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(json!(scope))
                    .execute(s.world.pool.pool())
                    .await?;
            }
            "owner" => {
                sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE object_kind='provider' AND object_id=$1 AND permission='access.own' AND removed_at IS NULL").bind(source).bind(s.world.clock.now()).execute(s.world.pool.pool()).await?;
            }
            _ => unreachable!(),
        }
        let provision = ProviderProvision::of(s.engine.clone(), &s.job.row().await?);
        assert!(provision.commit(&s.job).await.is_err(), "{change}");
        let mut conn = s.world.pool.acquire().await?;
        assert_eq!(
            orch_db::queries::resources::routes::providers(&mut conn, &[source])
                .await?
                .len(),
            1,
            "{change}"
        );
        assert!(
            provider_rows::state(&mut conn, s.world.provider)
                .await?
                .ok_or("target absent")?
                .machine_id
                .is_none(),
            "{change}"
        );
        s.engine.stop();
    }
    Ok(())
}
#[tokio::test]
async fn a_late_provider_source_delete_failure_rolls_back_the_copy_runtime_commit_and_audits()
-> TestResult {
    let s = Fixture::moving(false).await?;
    let source = s.source.ok_or("source absent")?;
    let mut conn = s.world.pool.acquire().await?;
    let sql = format!(
        "CREATE FUNCTION refuse_moving_source() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id='{}'::uuid AND NEW.deleted_at IS NOT NULL THEN RAISE EXCEPTION 'synthetic late source failure'; END IF; RETURN NEW; END $$",
        source
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(&mut *conn)
        .await?;
    sqlx::query("CREATE TRIGGER refuse_moving_source BEFORE UPDATE ON providers FOR EACH ROW EXECUTE FUNCTION refuse_moving_source()").execute(&mut *conn).await?;
    drop(conn);
    let provision = ProviderProvision::of(s.engine.clone(), &s.job.row().await?);
    assert!(provision.commit(&s.job).await.is_err());
    let mut conn = s.world.pool.acquire().await?;
    assert_eq!(
        orch_db::queries::resources::routes::providers(&mut conn, &[source])
            .await?
            .len(),
        1
    );
    assert!(
        provider_rows::state(&mut conn, s.world.provider)
            .await?
            .ok_or("copy absent")?
            .machine_id
            .is_none()
    );
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action=ANY($2)")
            .bind(source)
            .bind(vec!["content.copied_out", "provider.deleted"])
            .fetch_one(&mut *conn)
            .await?;
    assert_eq!(audits, 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_subscription_move_startup_keeps_the_actual_move_and_rechecks_its_raw_origin_and_physical_target()
-> TestResult {
    use crate::harnesses::preparation::{Source as _, Target};
    let s = Fixture::subscription().await?;
    let owner = s.job.row().await?;
    let authority = read(&owner)?.ok_or("creator absent")?;
    let source = harness::Source {
        job: owner.id,
        origin: authority.origin.clone(),
        until: authority
            .duplicate
            .as_ref()
            .ok_or("source absent")?
            .raw
            .until(),
    };
    let target = Target::Harness(crate::harnesses::HarnessKey {
        provider: s.world.provider,
        agent: s.world.agent,
        machine: s.world.machine_id,
    });
    let mut tx = s.world.pool.begin().await?;
    let ready = source
        .checkpoint(&mut tx, s.world.clock.as_ref(), target)
        .await?;
    assert_eq!(ready.caller().admitted_operation(), Some("ProviderMoveOut"));
    assert!(ready.caller().admitted_required_roles().is_some());
    ready.check(s.world.clock.as_ref())?;
    let command = Outgoing::new(
        new_id(),
        s.world.machine_id,
        SerialKey::Harness {
            provider_id: s.world.provider,
            agent_id: s.world.agent,
        },
        &orch_link_contract::commands::AgentStop {
            provider_id: s.world.provider,
            agent_id: s.world.agent,
            logout: None,
        },
    )?;
    let native = source
        .command(&mut tx, s.world.clock.as_ref(), target, &command)
        .await?;
    let serial = command.key.wire();
    let queued = orch_db::queries::link::NewCommand {
        id: command.id,
        machine_id: command.machine,
        kind: command.kind.as_str(),
        serial_key: serial.as_deref(),
        body: &command.body,
        deadline_at: s.world.clock.now()
            + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
    };
    let callers = native
        .current_issue_callers(
            &mut tx,
            &queued,
            s.world.clock.as_ref(),
            s.world.access.as_ref(),
        )
        .await?
        .ok_or("the genuine native startup command lost its current source proof")?;
    assert!(!callers.is_empty());
    assert!(callers.iter().all(|(caller, _)| {
        caller.as_ref().is_some_and(|caller| {
            caller.admitted_operation() == Some("ProviderMoveOut")
                && caller.admitted_required_roles().is_some()
        })
    }));
    let changed = orch_db::queries::link::NewCommand {
        machine_id: s.copy_keeper.as_ref().ok_or("keeper absent")?.id,
        ..queued
    };
    assert!(
        native
            .current_issue_callers(
                &mut tx,
                &changed,
                s.world.clock.as_ref(),
                s.world.access.as_ref(),
            )
            .await?
            .is_none(),
        "the target's private proof cannot be sent to the equally live source keeper"
    );
    tx.rollback().await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.token)
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    let mut tx = s.world.pool.begin().await?;
    assert!(
        source
            .checkpoint(&mut tx, s.world.clock.as_ref(), target)
            .await
            .is_err()
    );
    assert!(
        native
            .current_issue_callers(
                &mut tx,
                &queued,
                s.world.clock.as_ref(),
                s.world.access.as_ref(),
            )
            .await?
            .is_none(),
        "native dispatch keeps the same revoked raw origin as startup"
    );
    tx.rollback().await?;
    assert_eq!(s.world.machine.handler.runs_of(command.id), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn owned_sign_in_return_needs_the_genuine_forward_receipt_and_cannot_revive_the_old_actor()
-> TestResult {
    use orch_link_contract::commands::{
        MachineKey, ProviderCredentialAccept, ProviderCredentialIssue, Sealed, SealedCredential,
    };
    let s = Fixture::subscription().await?;
    let owner = s.job.row().await?;
    let authority = read(&owner)?.ok_or("creator absent")?;
    let mut conn = s.world.pool.acquire().await?;
    let (keeper, recipient, purpose) = transfer::context(&mut conn, &owner, &authority)
        .await?
        .ok_or("native handover absent")?;
    let issued = Outgoing::new(
        new_id(),
        keeper,
        SerialKey::Provider(purpose.source_provider_id),
        &ProviderCredentialIssue {
            lease_id: None,
            copy: Some(purpose.clone()),
            provider_id: purpose.source_provider_id,
            form: purpose
                .handover
                .as_ref()
                .ok_or("native form absent")?
                .form
                .clone(),
            recipient: recipient.clone(),
            return_credential: None,
        },
    )?;
    drop(conn);
    s.reserve(&issued).await?;
    remember_command(&s.job, &issued)
        .await
        .map_err(|error| format!("{error:?}"))?;
    let sealed = SealedCredential {
        form: "fake_login".into(),
        credential: Sealed {
            value: "synthetic-native-sealed-login".into(),
        },
        expires_at: None,
    };
    let mut tx = s.world.pool.begin().await?;
    orch_db::queries::link::insert_command(
        &mut tx,
        &orch_db::queries::link::NewCommand {
            id: issued.id,
            machine_id: issued.machine,
            kind: issued.kind.as_str(),
            body: &issued.body,
            serial_key: issued.key.wire().as_deref(),
            deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
        },
        s.world.clock.now(),
    )
    .await?;
    sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE id=$1")
        .bind(issued.id)
        .bind(json!(sealed))
        .execute(&mut *tx)
        .await?;
    jobs::merge_plan(
        &mut tx,
        s.job.id(),
        &json!({"abort":{"kind":"cancelled"}}),
        s.world.clock.now(),
    )
    .await?;
    tx.commit().await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.token)
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    let mut returning = purpose;
    returning
        .handover
        .as_mut()
        .ok_or("handover absent")?
        .returning = true;
    let original: MachineKey = returning
        .handover
        .as_ref()
        .ok_or("handover absent")?
        .original
        .clone();
    let inverse = Outgoing::new(
        new_id(),
        recipient.machine_id,
        SerialKey::Provider(s.world.provider),
        &ProviderCredentialIssue {
            lease_id: None,
            copy: Some(returning.clone()),
            provider_id: s.world.provider,
            form: "fake_login".into(),
            recipient: original,
            return_credential: Some(sealed),
        },
    )?;
    let owner = s.job.row().await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        transfer::inverse_binding(
            &mut conn,
            &owner,
            &authority,
            inverse.machine,
            inverse.key.wire().as_deref(),
            inverse.kind.as_str(),
            &inverse.body
        )
        .await?
    );
    let mut borrowed = inverse.body.clone();
    borrowed["copy"]["job_id"] = json!(new_id());
    assert!(
        !transfer::inverse_binding(
            &mut conn,
            &owner,
            &authority,
            inverse.machine,
            inverse.key.wire().as_deref(),
            inverse.kind.as_str(),
            &borrowed
        )
        .await?
    );
    assert!(
        principal(&mut conn, &owner, s.world.clock.as_ref())
            .await
            .is_err(),
        "the original revoked actor stays refused independently of its exact owned return"
    );
    let unproven = serde_json::to_value(ProviderCredentialAccept {
        copy: Some(returning),
        provider_id: s.source.ok_or("source absent")?,
        form: "fake_login".into(),
        credential: Sealed {
            value: "a return with no actual issued receipt".into(),
        },
        expires_at: None,
    })?;
    assert!(
        !transfer::inverse_binding(
            &mut conn,
            &owner,
            &authority,
            keeper,
            SerialKey::Provider(s.source.ok_or("source absent")?)
                .wire()
                .as_deref(),
            "provider.credential.accept",
            &unproven
        )
        .await?
    );
    s.engine.stop();
    Ok(())
}
