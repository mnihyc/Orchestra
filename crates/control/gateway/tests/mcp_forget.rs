//! Local Forget crosses authenticated links and the actual deletion/custody writer before unlinking a private copy.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Machine, PATIENCE, World, eventually};
use orch_api_contract::environment::McpScope;
use orch_connector_core::local_socket::{self, Activity, LocalState, Reply, Request};
use orch_db::queries::{access, hierarchy, resources};
use orch_envhost::{
    blobs::MemoryBlobs,
    environment::{EnvHost, Gates, HostSettings, Reports},
    mcp::McpGates,
};
use orch_link_contract::{
    commands::{Command, CommandKind, CommandResult, Empty, EnvAttach, EnvDetach, IndexMode},
    enrollment::Trust,
    mcp_import::{McpForgetAsk, McpForgetSelection, McpImportAsk},
    session::Role,
};
use orch_types::{Id, new_id};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

struct Fixture {
    world: World,
    machine: Machine,
    environment: Id,
    org: Id,
    host: Arc<EnvHost>,
    data: tempfile::TempDir,
    work: tempfile::TempDir,
    source: PathBuf,
    stop: CancellationToken,
    socket: tokio::task::JoinHandle<()>,
    _reports: tokio::sync::mpsc::Receiver<orch_envhost::environment::Reported>,
}
impl Fixture {
    async fn new() -> Self {
        let world = World::start().await;
        let enrolled = world.enroll("native-copy-machine").await;
        let machine = world.start_machine_as(
            &enrolled,
            enrolled.endpoints(Trust::Pinned),
            vec![Role::Environment],
        );
        tokio::time::timeout(PATIENCE, machine.link.connected())
            .await
            .unwrap();
        let data = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let org = new_id();
        let environment = {
            let mut conn = world.db.acquire().await.unwrap();
            hierarchy::testing::insert_node(
                &mut conn,
                org,
                "org",
                None,
                "Native-copy team",
                world.user,
            )
            .await
            .unwrap();
            access::testing::insert_member(&mut conn, org, world.user, org)
                .await
                .unwrap();
            access::testing::add_entry(
                &mut conn,
                &access::PageKey::new("node", org, "live"),
                "any",
                "access.own",
                ("user", Some(world.user), None),
                None,
                world.clock.now(),
            )
            .await
            .unwrap();
            let environment = resources::access_fixtures::insert_environment(
                &mut conn,
                world.user,
                Some(org),
                "Native-copy work",
                "connected",
            )
            .await
            .unwrap();
            access::testing::add_entry(
                &mut conn,
                &access::PageKey::new("environment", environment, "live"),
                "any",
                "access.own",
                ("user", Some(world.user), None),
                None,
                world.clock.now(),
            )
            .await
            .unwrap();
            // Named discovery double; adapters' real isolated configuration/digest controls are separate.
            // Here the private file, copy writer, links, admission, journal, audit and unlink are real.
            let found: Vec<_> = [("native:name", "fake"), ("native:name", "second-fake"), ("notes", "fake"), ("blocked", "fake")]
                .into_iter().map(|(name, source)| json!({"source":source,"scope":"user","name":name,"directory":null,"transport":"stdio","digest":"scripted-discovery"})).collect();
            sqlx::query(
                "UPDATE environments SET machine_id=$2,workspace_root=$3,facts=$4 WHERE id=$1",
            )
            .bind(environment)
            .bind(machine.id)
            .bind(work.path().to_str().unwrap())
            .bind(json!({"mcp_found":found}))
            .execute(&mut *conn)
            .await
            .unwrap();
            environment
        };
        let mut settings =
            HostSettings::connected(data.path().to_owned(), vec![work.path().to_owned()]);
        settings.gates = Gates::OPEN;
        let (reports, receiver) = Reports::channel(256);
        let host = Arc::new(EnvHost::new(
            settings,
            world.clock.clone(),
            reports,
            Arc::new(MemoryBlobs::new()),
            Vec::new(),
        ));
        host.env_attach(
            &EnvAttach {
                resources: None,
                data_generation: None,
                storage: None,
                data_sources: None,
                environment_id: environment,
                workspace_root: Some(work.path().display().to_string()),
                ignore: Vec::new(),
                index: IndexMode::Full,
                mounts: None,
                variables: BTreeMap::new(),
                secrets: BTreeMap::new(),
            },
            &BTreeMap::new(),
        )
        .await
        .unwrap();
        let source = data.path().join("scripted-native-definition.json");
        std::fs::write(&source, serde_json::to_vec(&json!({
            "spec":{"transport":"stdio","command":["scripted-native-tool"],"secret_env":["SERVICE_TOKEN"]},
            "secrets":{"SERVICE_TOKEN":"private-copy-value"}
        })).unwrap()).unwrap();
        let native = host.clone();
        let config = source.clone();
        machine.handler.on(CommandKind::McpImport, move |command| {
            let Command::McpImport(body) = command else {
                panic!("expected the actual import command")
            };
            let definition = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
            native
                .mcp_import(body, &definition, Some(2))
                .map(CommandResult::McpImport)
                .map_err(Into::into)
        });
        let native = host.clone();
        machine.handler.on(CommandKind::McpForget, move |command| {
            let Command::McpForget(body) = command else {
                panic!("expected the recorded cleanup command")
            };
            native
                .mcp_forget(body.server_id)
                .map(|()| CommandResult::McpForget(Empty {}))
                .map_err(Into::into)
        });
        let stop = CancellationToken::new();
        let socket = tokio::spawn(local_socket::serve(
            Arc::new(LocalState {
                gateway_control: Arc::new(orch_connector_core::gateway_control::Requests::new(
                    data.path().to_owned(),
                    Arc::new(
                        orch_connector_core::policy::PolicyStore::connected(
                            orch_config::ConnectorConfig::default(),
                            Some(
                                orch_connector_core::enrollment::DataDir::open(data.path())
                                    .unwrap()
                                    .config_path(),
                            ),
                        )
                        .unwrap(),
                    ),
                    {
                        let link = machine.link.clone();
                        Arc::new(move |choice| {
                            if link.gateway_status().choice != choice {
                                link.select_gateway(choice);
                            }
                        })
                    },
                )),
                identity_reload: None,
                identity_fingerprint: String::new(),
                link: machine.link.clone(),
                version: "0.1.0".into(),
                data_dir: data.path().to_owned(),
                environments: None,
                hosting: None,
                activity: Arc::new(Activity::default()),
            }),
            stop.clone(),
        ));
        eventually("the actual local console listener", || async {
            data.path().join(local_socket::SOCKET_FILE).exists()
        })
        .await;
        Self {
            world,
            machine,
            environment,
            org,
            host,
            data,
            work,
            source,
            stop,
            socket,
            _reports: receiver,
        }
    }
    async fn import(&self, name: &str, source: &str, alias: &str) -> Id {
        let server = new_id();
        let answer = tokio::time::timeout(
            PATIENCE,
            self.machine.link.import_mcp(McpImportAsk {
                request_id: server,
                environment_id: self.environment,
                source: source.into(),
                scope: McpScope::User,
                directory: None,
                name: name.into(),
                as_name: Some(alias.into()),
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(answer.code, None, "{}", answer.detail);
        assert_eq!(answer.server_id, Some(server));
        let row = resources::mcp::servers(&mut self.world.db.acquire().await.unwrap(), &[server])
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(row.name, alias);
        assert_eq!(row.owner_id, Some(self.world.user));
        assert!(!row.spec.to_string().contains("private-copy-value"));
        let enabled: bool = sqlx::query_scalar("SELECT COALESCE((config->'mcp.enabled'->>$2)::boolean,false) FROM environments WHERE id=$1")
            .bind(self.environment).bind(server.to_string()).fetch_one(self.world.db.pool()).await.unwrap();
        assert!(enabled);
        let copy = self.copy(server);
        assert!(copy.is_file());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            std::fs::read_to_string(copy)
                .unwrap()
                .contains("private-copy-value")
        );
        server
    }
    fn copy(&self, server: Id) -> PathBuf {
        self.data.path().join("mcp").join(format!("{server}.json"))
    }
    async fn forget(&self, name: &str, from: Option<&str>) -> Reply {
        tokio::time::timeout(
            PATIENCE,
            local_socket::ask(
                self.data.path(),
                &Request::McpForget {
                    name: name.into(),
                    from: from.map(str::to_owned),
                },
            ),
        )
        .await
        .unwrap()
        .unwrap()
    }
    async fn live(&self, server: Id) -> bool {
        sqlx::query_scalar("SELECT deleted_at IS NULL FROM mcp_servers WHERE id=$1")
            .bind(server)
            .fetch_one(self.world.db.pool())
            .await
            .unwrap()
    }
    async fn command_count(&self, server: Id) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
        )
        .bind(server.to_string())
        .fetch_one(self.world.db.pool())
        .await
        .unwrap()
    }
    async fn intact(&self, server: Id) {
        assert!(self.live(server).await);
        assert!(self.copy(server).is_file());
        assert_eq!(self.command_count(server).await, 0);
    }
    async fn stop(mut self) {
        self.stop.cancel();
        self.socket.await.unwrap();
        self.host
            .env_detach(&EnvDetach {
                environment_id: self.environment,
            })
            .await
            .unwrap();
        self.machine.link.shutdown().await;
        self.world.stop().await;
    }
}

#[tokio::test]
async fn a_local_forget_removes_the_recorded_resource_and_its_real_private_copy_after_discovery_disappears()
 {
    let s = Fixture::new().await;
    let first = s.import("native:name", "fake", "file-tools").await;
    let second = s.import("native:name", "second-fake", "other-tools").await;
    let third = s.import("notes", "fake", "note-tools").await;
    let refused = s.forget("native:name", None).await;
    let Reply::Error { message } = refused else {
        panic!("an ambiguous original name must be refused")
    };
    assert!(message.contains("several imports match"), "{message}");
    s.intact(first).await;
    s.intact(second).await;

    // Same machine certificate after transfer: fresh ordinary Manage is required from its new owner.
    let later_owner = {
        let mut conn = s.world.db.acquire().await.unwrap();
        let user = resources::testing::insert_user(&mut conn, "later-native-owner")
            .await
            .unwrap();
        access::testing::insert_member(&mut conn, s.org, user, s.org)
            .await
            .unwrap();
        user
    };
    sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
        .bind(s.machine.id)
        .bind(later_owner)
        .execute(s.world.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        s.forget("file-tools", None).await,
        Reply::Error { .. }
    ));
    s.intact(first).await;
    sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
        .bind(s.machine.id)
        .bind(s.world.user)
        .execute(s.world.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
        .bind(s.world.user)
        .execute(s.world.db.pool())
        .await
        .unwrap();
    let Reply::Error { message } = s.forget("file-tools", None).await else {
        panic!("a suspended owner cannot forget")
    };
    assert!(message.contains("auth.suspended"), "{message}");
    s.intact(first).await;
    sqlx::query("UPDATE users SET status='active' WHERE id=$1")
        .bind(s.world.user)
        .execute(s.world.db.pool())
        .await
        .unwrap();

    let enrolled = s.world.enroll("another-native-machine").await;
    let foreign = s.world.start_machine_as(
        &enrolled,
        enrolled.endpoints(Trust::Pinned),
        vec![Role::Environment],
    );
    tokio::time::timeout(PATIENCE, foreign.link.connected())
        .await
        .unwrap();
    let answer = tokio::time::timeout(
        PATIENCE,
        foreign.link.import_mcp(McpForgetAsk {
            request_id: new_id(),
            forget: McpForgetSelection {
                name: first.to_string(),
                from: None,
            },
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(answer.code.is_some());
    assert_eq!(answer.server_id, None);
    s.intact(first).await;
    foreign.link.shutdown().await;

    // The original file and discoveries cease to exist; Forget must use the recorded origin/id.
    std::fs::remove_file(&s.source).unwrap();
    sqlx::query("UPDATE environments SET facts='{}' WHERE id=$1")
        .bind(s.environment)
        .execute(s.world.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
        .bind(s.machine.id)
        .bind(
            serde_json::to_value(orch_link_contract::policy::LocalPolicy {
                mcp_import: false,
                ..Default::default()
            })
            .unwrap(),
        )
        .execute(s.world.db.pool())
        .await
        .unwrap();
    s.host.set_policy(
        Gates::OPEN,
        Some(vec![s.work.path().to_owned()]),
        None,
        McpGates {
            import: false,
            add: false,
        },
    );
    assert!(
        !s.host
            .environment(s.environment)
            .unwrap()
            .mcp()
            .import_allowed()
    );
    for (server, selector, source) in [
        (first, "native:name".to_owned(), Some("fake")),
        (second, second.to_string(), None),
        (third, "note-tools".to_owned(), None),
    ] {
        let Reply::Done { said } = s.forget(&selector, source).await else {
            panic!("the recorded import must remain forgettable")
        };
        assert!(said.contains(&server.to_string()), "{said}");
        assert!(said.contains("copy was removed"), "{said}");
        assert!(!said.contains("private-copy-value"));
        assert!(!s.live(server).await);
        assert!(!s.copy(server).exists());
        assert_eq!(s.command_count(server).await, 1);
        let actors: Vec<Option<Id>> = sqlx::query_scalar(
            "SELECT actor_id FROM audit_log WHERE action='mcp.deleted' AND object_id=$1",
        )
        .bind(server)
        .fetch_all(s.world.db.pool())
        .await
        .unwrap();
        assert_eq!(actors, vec![Some(s.world.user)]);
        let command_id: Id = sqlx::query_scalar(
            "SELECT id FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
        )
        .bind(server.to_string())
        .fetch_one(s.world.db.pool())
        .await
        .unwrap();
        let saved = orch_db::queries::link::command_with_authority(
            &mut s.world.db.acquire().await.unwrap(),
            command_id,
        )
        .await
        .unwrap()
        .unwrap()
        .authority
        .unwrap();
        assert_eq!(saved["kind"], "mcp_forget");
        assert_eq!(saved["authority"]["custody"]["kind"], "recorded_deletion");
        assert!(!saved.to_string().contains("private-copy-value"));
    }
    s.stop().await;
}

#[tokio::test]
async fn a_local_forget_reports_actual_copy_cleanup_failure_without_claiming_the_unlink_succeeded()
{
    let s = Fixture::new().await;
    let server = s.import("blocked", "fake", "blocked-tools").await;
    let copy = s.copy(server);
    // A real filesystem failure, independent of uid/permission-bit bypass when tests run as root.
    std::fs::rename(&copy, s.data.path().join("protected-copy-backup.json")).unwrap();
    std::fs::create_dir(&copy).unwrap();
    let Reply::Done { said } = s.forget("blocked-tools", None).await else {
        panic!("the committed deletion is acknowledged separately from its failed cleanup")
    };
    assert!(said.contains("in Orch"), "{said}");
    assert!(said.contains("not confirmed"), "{said}");
    assert!(!said.contains("copy was removed"), "{said}");
    assert!(!s.live(server).await);
    assert!(copy.is_dir());
    assert_eq!(s.command_count(server).await, 1);
    assert_eq!(
        s.machine
            .handler
            .executions()
            .iter()
            .filter(|run| run.kind == CommandKind::McpForget)
            .count(),
        1,
        "the real filesystem operation failed after native dispatch, rather than an earlier admission"
    );
    s.stop().await;
}
