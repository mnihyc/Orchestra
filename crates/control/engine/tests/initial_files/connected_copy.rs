//! Runtime-free native provision uses real source bytes, raw credentials and held materialization.
use super::*;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyScope, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    duplicates::EnvironmentDuplicate,
    environment_sources::{CreateSource as Role, duplicating::CONNECTED},
    routes::Endpoint,
};
use orch_authz::{
    CapturedOperation, TokenGrant,
    ownership::{self, NewAccess},
};
use orch_db::queries::{
    environments::{hosting as hosting_rows, routes},
    identity,
};
use orch_engine::operation_scope::{
    http_origin::HttpOrigin,
    request_origin::Credential,
    sources::{self, Facts},
    targets,
};
use orch_types::new_id;

const OPERATION: &str = "EnvironmentDuplicate.connected";

struct Copy {
    world: World,
    prepared: Prepared,
    made: environments::Made,
    token: Id,
}
impl Copy {
    async fn new() -> Self {
        Self::with_move(false).await
    }
    async fn with_move(moving: bool) -> Self {
        let world = World::new().await;
        let prepared = source_with_mode(&world, false, true).await;
        let target = new_id();
        let mut tx = world.pool.begin().await.unwrap();
        let now = world.clock.now();
        let home = if moving {
            let root = new_id();
            let project = new_id();
            let topic = new_id();
            for (id, kind, parent, name) in [
                (root, "org", None, "Copy destination"),
                (project, "project", Some(root), "Destination project"),
                (topic, "topic", Some(project), "Destination topic"),
            ] {
                orch_db::queries::hierarchy::testing::insert_node(
                    &mut tx, id, kind, parent, name, world.ada,
                )
                .await
                .unwrap();
                orch_db::queries::access::testing::name_owner(
                    &mut tx,
                    id,
                    ("user", world.ada),
                    now,
                )
                .await
                .unwrap();
            }
            orch_db::queries::access::testing::insert_member(&mut tx, root, world.ada, root)
                .await
                .unwrap();
            topic
        } else {
            world.topic
        };
        let operation = if moving {
            orch_api_contract::environment_sources::duplicating::MOVE_CONNECTED.operation
        } else {
            OPERATION
        };

        sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
            .bind(world.machine_id)
            .bind(world.ada)
            .execute(&mut *tx)
            .await
            .unwrap();
        orch_db::queries::access::testing::add_entry(
            &mut tx,
            &orch_db::queries::access::PageKey::new("machine", world.machine_id, "live"),
            "any",
            "access.own",
            ("user", Some(world.ada), None),
            None,
            now,
        )
        .await
        .unwrap();
        let governor =
            orch_db::queries::resources::hosting::host_of_machine(&mut tx, world.machine_id)
                .await
                .unwrap();
        let (kind, subject, permission) = governor
            .map_or(("machine", world.machine_id, "machine.use"), |host| {
                ("host", host, "host.use")
            });
        orch_db::queries::access::testing::add_entry(
            &mut tx,
            &orch_db::queries::access::PageKey::new(kind, subject, "live"),
            "any",
            permission,
            ("user", Some(world.ada), None),
            None,
            now,
        )
        .await
        .unwrap();
        let scope:KeyScope=serde_json::from_value(json!({"rules":[
            {"target":{"kind":"topic","id":home,"reach":"self"},"resources":["environment"],"allow":[{"operation":operation,"actions":["write"]}]},
            {"target":{"kind":"environment","id":prepared.source,"reach":"self"},"allow":[{"operation":operation,"actions":if moving {vec!["read","manage"]} else {vec!["read"]}}]},
            {"target":{"kind":"machine","id":world.machine_id,"reach":"self"},"allow":[{"operation":operation,"actions":["write"]}]}
        ]})).unwrap();
        let token = new_id();
        let hash = Hash::of(token.as_bytes());
        let expires = Time::new(now + chrono::Duration::hours(1));
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: token,
                user_id: world.ada,
                name: "Connected copy origin",
                token_hash: hash.as_bytes(),
                scopes: &[],
                node_id: None,
                at: now,
                expires_at: Some(expires.get()),
            },
            Some(&json!(scope)),
        )
        .await
        .unwrap();
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(scope), Some(expires))
                .unwrap()
                .for_token(token),
        );
        let env = KeyTargetKind::Object(ObjectKind::Environment);
        let machine = KeyTargetKind::Object(ObjectKind::Machine);
        let mut facts = vec![
            Facts::one(Role::Home, env, targets::node(&mut tx, home).await.unwrap()),
            Facts::one(
                Role::Machine,
                machine,
                targets::machine_subject(&mut tx, world.machine_id)
                    .await
                    .unwrap(),
            ),
            Facts::collection(Role::Mounts, Vec::new()),
            Facts::collection(Role::InitialThreads, Vec::new()),
            Facts::collection(
                Role::InitialEnvironments,
                vec![(
                    env,
                    vec![
                        targets::resource_identity(
                            &mut tx,
                            ObjectRef {
                                kind: ObjectKind::Environment,
                                id: prepared.source,
                            },
                        )
                        .await
                        .unwrap(),
                    ],
                )],
            ),
        ];
        if moving {
            facts.push(Facts::one(
                Role::MoveSubject,
                env,
                targets::resource_identity(
                    &mut tx,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: prepared.source,
                    },
                )
                .await
                .unwrap(),
            ));
        }
        let bound = sources::bind_required(
            if moving {
                orch_api_contract::duplicates::EnvironmentMoveOut::ROUTE
            } else {
                EnvironmentDuplicate::ROUTE
            },
            if moving {
                &orch_api_contract::environment_sources::duplicating::MOVE_CONNECTED
            } else {
                &CONNECTED
            },
            &facts,
        )
        .unwrap();
        let caller = bound.admit(&caller, false).unwrap();
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now))
            .await
            .unwrap();
        let files = initial_files::plan(
            &mut tx,
            &principal,
            &NewEnvironmentFiles::FromEnvironment {
                from_environment: prepared.source,
                selection: None,
            },
        )
        .await
        .unwrap();
        let policies = orch_engine::access::resource_policies(&mut tx, world.machine_id)
            .await
            .unwrap();
        let actual_creation = orch_authz::environments::check_creation(
            &mut tx,
            &principal,
            &orch_authz::environments::NewEnvironment {
                home,
                place: orch_authz::environments::Place::Machine {
                    machine: world.machine_id,
                },
                mounts: Vec::new(),
            },
            &policies,
        )
        .await
        .unwrap();
        assert!(
            matches!(
                &actual_creation,
                orch_authz::environments::Creating::Allowed(_)
            ),
            "the actual native physical Use/creation rule refused the fixture: {actual_creation:?}"
        );
        assert!(
            routes::insert_connected(
                &mut tx,
                &routes::NewConnected {
                    id: target,
                    name: "Connected copied workspace",
                    owner_id: world.ada,
                    machine_id: world.machine_id,
                    home_node_id: home,
                    workspace_root: "",
                    remote_write: Some(true),
                    terminal: Some(true),
                },
                now
            )
            .await
            .unwrap()
        );
        sqlx::query("UPDATE environments SET config=$2 WHERE id=$1").bind(target)
            .bind(json!({"env.data_sources":orch_driver_contract::runtime::EnvironmentDataSources::default()})).execute(&mut *tx).await.unwrap();
        ownership::start_resource(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: target,
            },
            world.ada,
            NewAccess::Defaults,
            Time::new(now),
        )
        .await
        .unwrap();
        let state = hosting_rows::environment(&mut tx, target)
            .await
            .unwrap()
            .unwrap();
        let job = orch_engine::jobs::provision::queue(
            &mut tx,
            now,
            &state,
            orch_engine::jobs::provision::Mode::Copy,
            Some(world.ada),
        )
        .await
        .unwrap();
        jobs::merge_plan(&mut tx, job.id, &json!({"initial_files":files}), now)
            .await
            .unwrap();
        if moving {
            let source = routes::environments(&mut tx, &[prepared.source])
                .await
                .unwrap()
                .pop()
                .unwrap();
            let review = orch_engine::environments::ending::review(&mut tx, &source)
                .await
                .unwrap();
            jobs::merge_plan(
                &mut tx,
                job.id,
                &json!({"environment_move_out":review}),
                now,
            )
            .await
            .unwrap();
        }
        let origin = CapturedOperation::capture(&caller).unwrap();
        let raw = HttpOrigin::capture(
            origin.clone(),
            Credential::Token {
                id: token,
                hash: hash.as_bytes().to_vec(),
            },
            false,
            false,
            Some(expires),
        )
        .unwrap();
        let made = environments::Made { id: target, job };
        orch_engine::environment_creation::retain_duplicate(
            &mut tx,
            &origin,
            &raw,
            &made,
            home,
            Time::new(now),
        )
        .await
        .unwrap();
        let mut files = files;
        files
            .capture_environment_creator(&caller, made.job.id, target, 0)
            .unwrap();
        jobs::merge_plan(&mut tx, made.job.id, &json!({"initial_files":files}), now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let root = format!("/owned/{target}/workspace");
        world
            .machine
            .handler
            .on(CommandKind::EnvAttach, move |command| {
                let Command::EnvAttach(body) = command else {
                    panic!("native attach");
                };
                assert_eq!(body.environment_id, target);
                assert_eq!(body.workspace_root, Some(String::new()));
                assert_eq!(body.data_sources, Some(Default::default()));
                let mut facts = hosting::facts();
                facts["workspace_root"] = json!(root);
                facts["generation"] = json!(0);
                Ok(CommandResult::EnvAttach(
                    serde_json::from_value(facts).unwrap(),
                ))
            });
        world.machine.handler.on(CommandKind::ExecStart, |_| {
            Ok(CommandResult::ExecStart(
                serde_json::from_value(
                    json!({"service":"mcp/exec","version":"0.1.0","tool_sets":["plain"]}),
                )
                .unwrap(),
            ))
        });
        let copy_job = made.job.id;
        world
            .machine
            .handler
            .on(CommandKind::FsMaterialize, move |command| {
                let Command::FsMaterialize(body) = command else {
                    panic!("native materialization");
                };
                assert_eq!(body.environment_id, target);
                assert_eq!(body.job_id, Some(copy_job));
                Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                    files: 3,
                    bytes: 3 * i64::try_from(SOURCE_BYTES.len()).unwrap(),
                    skipped: 0,
                }))
            });
        world
            .machine
            .handler
            .on(CommandKind::EnvDataRemove, move |command| {
                let Command::EnvDataRemove(body) = command else {
                    panic!("native owned-data inverse");
                };
                assert_eq!(body.environment_id, target);
                assert_eq!(body.data_generation, 0);
                Ok(CommandResult::EnvDataRemove(
                    orch_link_contract::commands::Empty {},
                ))
            });
        world.machine.handler.hold(CommandKind::FsMaterialize);
        world.engine().kick();
        Self {
            world,
            prepared,
            made,
            token,
        }
    }
    async fn held(&self) {
        common::eventually("connected copy reaches native materialization", || async {
            let row = jobs::job(
                &mut self.world.pool.acquire().await.unwrap(),
                self.made.job.id,
            )
            .await
            .unwrap()
            .unwrap();
            assert!(row.is_open(), "connected copy ended: {:?}", row.error);
            self.world
                .machine
                .handler
                .runs_of(orch_engine::ids::derived(
                    "initial-files.materialize",
                    &[self.made.job.id.as_bytes()],
                ))
                > 0
        })
        .await;
    }
}

#[tokio::test]
async fn a_connected_copy_has_no_runtime_and_keeps_readiness_after_its_actual_byte_materialization()
{
    let s = Copy::new().await;
    s.held().await;
    assert_eq!(hosting::status(&s.world, s.made.id).await.0, "provisioning");
    let mut conn = s.world.pool.acquire().await.unwrap();
    let row = jobs::job(&mut conn, s.made.job.id).await.unwrap().unwrap();
    assert!(row.plan["runtime"].is_null());
    assert!(row.plan["creator_provision"]["template"].is_null());
    let hash: Hash = serde_json::from_value(row.plan["initial_files_manifest"].clone()).unwrap();
    let manifest = s.world.engine().read_manifest(&hash).await.unwrap();
    assert_eq!(
        manifest
            .entries()
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .count(),
        3
    );
    for entry in manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        assert_eq!(entry.hash, Some(s.prepared.hash));
        let bytes = read_all(
            s.prepared
                .store
                .get(&s.prepared.hash, None)
                .await
                .unwrap()
                .body,
        )
        .await
        .unwrap();
        assert_eq!(bytes.as_ref(), SOURCE_BYTES);
    }
    drop(conn);
    s.world.machine.handler.release();
    assert_eq!(
        hosting::wait_over(&s.world, s.made.job.id).await.0,
        "succeeded"
    );
    assert_eq!(hosting::status(&s.world, s.made.id).await.0, "running");
    assert_eq!(s.world.commands.issued_of("node.runtime.create"), 0);
    s.world.engine().stop();
}

#[tokio::test]
async fn withdrawing_the_original_key_after_accepted_connected_io_finishes_only_owned_cleanup() {
    let s = Copy::new().await;
    s.held().await;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.token)
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    s.world.machine.handler.release();
    assert_eq!(
        hosting::wait_over(&s.world, s.made.job.id).await.0,
        "failed"
    );
    assert_eq!(hosting::status(&s.world, s.made.id).await.0, "failed");
    assert_eq!(s.world.commands.issued_of("env.data.remove"), 1);
    assert_eq!(s.world.commands.issued_of("node.runtime.remove"), 0);
    assert_eq!(
        hosting::status(&s.world, s.prepared.source).await.0,
        "running"
    );
    s.world.engine().stop();
}

#[tokio::test]
async fn an_actual_connected_move_delivers_original_bytes_before_ending_the_reviewed_source() {
    let s = Copy::with_move(true).await;
    s.held().await;
    assert_eq!(
        hosting::status(&s.world, s.prepared.source).await.0,
        "running"
    );
    assert_eq!(hosting::status(&s.world, s.made.id).await.0, "provisioning");
    let mut conn = s.world.pool.acquire().await.unwrap();
    let row = jobs::job(&mut conn, s.made.job.id).await.unwrap().unwrap();
    assert_eq!(
        row.plan["creator_provision"]["origin"]["operation"],
        "EnvironmentMoveOut.connected"
    );
    let hash: Hash = serde_json::from_value(row.plan["initial_files_manifest"].clone()).unwrap();
    let manifest = s.world.engine().read_manifest(&hash).await.unwrap();
    assert_eq!(
        manifest
            .entries()
            .iter()
            .filter(|item| item.kind == EntryKind::File)
            .count(),
        3
    );
    for item in manifest
        .entries()
        .iter()
        .filter(|item| item.kind == EntryKind::File)
    {
        assert_eq!(item.hash, Some(s.prepared.hash));
    }
    drop(conn);
    s.world.machine.handler.release();
    let completed = hosting::wait_over(&s.world, s.made.job.id).await;
    assert_eq!(
        completed.0, "succeeded",
        "native copy progress: {}; terminal error: {:?}",
        completed.1, completed.2
    );
    assert_eq!(hosting::status(&s.world, s.made.id).await.0, "running");
    let mut conn = s.world.pool.acquire().await.unwrap();
    let old: (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as("SELECT deleted_at,remove_at FROM environments WHERE id=$1")
        .bind(s.prepared.source)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(
        old.0.is_some() || old.1.is_some(),
        "the original Delete was accepted in the successful copy commit"
    );
    let actions:Vec<String>=sqlx::query_scalar("SELECT action FROM audit_log WHERE object_id=$1 AND action IN ('content.copied_out','environment.deleted','environment.removal_scheduled') ORDER BY action").bind(s.prepared.source).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(actions.len(), 2);
    assert!(actions.iter().any(|action| action == "content.copied_out"));
    assert_eq!(s.world.commands.issued_of("node.runtime.create"), 0);
    s.world.engine().stop();
}
#[tokio::test]
async fn a_connected_move_raw_or_review_withdrawal_keeps_the_original_and_collects_only_its_own_new_data()
 {
    for change in ["raw", "review", "scope"] {
        let s = Copy::with_move(true).await;
        s.held().await;
        if change == "raw" {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(s.token)
                .bind(s.world.clock.now())
                .execute(s.world.pool.pool())
                .await
                .unwrap();
        } else if change == "scope" {
            let mut policy: serde_json::Value =
                sqlx::query_scalar("SELECT operation_scope FROM api_tokens WHERE id=$1")
                    .bind(s.token)
                    .fetch_one(s.world.pool.pool())
                    .await
                    .unwrap();
            for rule in policy["rules"].as_array_mut().unwrap() {
                if rule["target"]["id"] == json!(s.prepared.source) {
                    for action in rule["allow"].as_array_mut().unwrap() {
                        action["actions"] = json!(["read"]);
                    }
                }
            }
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(s.token)
                .bind(policy)
                .execute(s.world.pool.pool())
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE environments SET revision=revision+1 WHERE id=$1")
                .bind(s.prepared.source)
                .execute(s.world.pool.pool())
                .await
                .unwrap();
        }
        s.world.machine.handler.release();
        assert_eq!(
            hosting::wait_over(&s.world, s.made.job.id).await.0,
            "failed",
            "{change}"
        );
        assert_eq!(
            hosting::status(&s.world, s.prepared.source).await.0,
            "running",
            "{change}"
        );
        assert_eq!(s.world.commands.issued_of("env.data.remove"), 1, "{change}");
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='content.copied_out'",
        )
        .bind(s.prepared.source)
        .fetch_one(s.world.pool.pool())
        .await
        .unwrap();
        assert_eq!(audits, 0);
        s.world.engine().stop();
    }
}
#[tokio::test]
async fn a_connected_move_late_original_delete_failure_rolls_back_ready_and_source_audits() {
    let s = Copy::with_move(true).await;
    s.held().await;
    let mut conn = s.world.pool.acquire().await.unwrap();
    let ddl = format!(
        "CREATE FUNCTION refuse_environment_move() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id='{}'::uuid AND NEW.remove_at IS NOT NULL THEN RAISE EXCEPTION 'synthetic late removal failure'; END IF; RETURN NEW; END $$",
        s.prepared.source
    );
    sqlx::query(sqlx::AssertSqlSafe(ddl))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER refuse_environment_move BEFORE UPDATE ON environments FOR EACH ROW EXECUTE FUNCTION refuse_environment_move()").execute(&mut *conn).await.unwrap();
    drop(conn);
    s.world.machine.handler.release();
    assert_eq!(
        hosting::wait_over(&s.world, s.made.job.id).await.0,
        "failed"
    );
    assert_eq!(
        hosting::status(&s.world, s.prepared.source).await.0,
        "running"
    );
    assert_eq!(s.world.commands.issued_of("env.data.remove"), 1);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='content.copied_out'",
    )
    .bind(s.prepared.source)
    .fetch_one(s.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(audits, 0);
    s.world.engine().stop();
}
