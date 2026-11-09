//! Actual HTTP import producers through the production entry/wire cuts and file-backed native journal.
//! This named configuration peer copies a disposable definition; it never supplies admission or a synthetic caller.
use super::*;
use crate::{
    middleware::authentication,
    registry::Registry,
    testing::{RawReply, TestApp},
};
use orch_access_contract::decisions::ObjectRef;
use orch_agent_contract::discovery::McpDefinition;
use orch_api_contract::{
    environment::{EnvironmentMcpImportCreate, EnvironmentMcpImportCreateBody, McpScope},
    resources::{McpOfferingUse, McpOfferingUseBody, McpServerReimport},
};
use orch_authz::ownership;
use orch_db::queries::{access, identity};
use orch_link_contract::commands::{McpImport, McpImportResult};
use orch_types::Json;

fn copy_path(native: &Native, server: Id) -> PathBuf {
    native
        ._directory
        .path()
        .join("mcp")
        .join(format!("{server}.json"))
}

pub(super) async fn reply(
    native: &Native,
    context: CommandContext,
    command: Command,
) -> Result<CommandResult, CommandError> {
    match command {
        Command::McpImport(body) => {
            let environment = body.environment_id.ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Conflict,
                    "the offering import has no actual environment",
                )
            })?;
            if context.key != SerialKey::Environment(environment).wire() {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "not the selected environment's native import queue",
                ));
            }
            if environment != native.environment
                || body.source != "fixture"
                || body.name != "search"
                || body.scope != "user"
                || body.directory.is_some()
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "not the actual selected native configuration",
                ));
            }
            let bytes = tokio::fs::read(native._directory.path().join("native-definition.json"))
                .await
                .map_err(|_| {
                    CommandError::new(
                        ErrorCode::NotFound,
                        "the isolated native definition is absent",
                    )
                })?;
            let definition: McpDefinition = serde_json::from_slice(&bytes).map_err(|_| {
                CommandError::new(
                    ErrorCode::Conflict,
                    "the isolated native definition is invalid",
                )
            })?;
            if body
                .expected_fingerprint
                .is_some_and(|expected| expected != definition.definition_fingerprint())
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the reviewed native definition changed",
                ));
            }
            native.reads.fetch_add(1, Ordering::SeqCst);
            let path = copy_path(native, body.server_id);
            std::fs::create_dir_all(path.parent().ok_or_else(|| {
                CommandError::new(ErrorCode::Internal, "the copy directory is absent")
            })?)
            .map_err(|_| {
                CommandError::new(ErrorCode::Internal, "the copy directory cannot be made")
            })?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            options
                .open(path)
                .and_then(|mut file| file.write_all(&bytes))
                .map_err(|_| {
                    CommandError::new(ErrorCode::Internal, "the actual private copy failed")
                })?;
            Ok(CommandResult::McpImport(McpImportResult {
                snapshot: None,
                definition_fingerprint: Some(definition.definition_fingerprint()),
                transport: "stdio".into(),
                digest: definition.digest().to_string(),
                tools: None,
            }))
        }
        Command::McpForget(body) => {
            if context.key != SerialKey::Machine.wire() {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "not the exact machine's native inverse queue",
                ));
            }
            match std::fs::remove_file(copy_path(native, body.server_id)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(CommandError::new(
                        ErrorCode::Internal,
                        "the actual inverse unlink failed",
                    ));
                }
            }
            native.writes.fetch_add(1, Ordering::SeqCst);
            Ok(CommandResult::McpForget(Empty {}))
        }
        _ => Err(CommandError::new(
            ErrorCode::Unsupported,
            "not an import or its owned inverse",
        )),
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Key,
    Session,
}
#[derive(Clone, Copy)]
enum Act {
    Create,
    Reimport(Id),
    Offering(Id, i64, Hash),
}
impl Act {
    fn operation(self) -> &'static str {
        match self {
            Self::Create => "EnvironmentMcpImportCreate",
            Self::Reimport(_) => "McpServerReimport",
            Self::Offering(..) => "McpOfferingUse",
        }
    }
}
struct Scene {
    transport: Fixture,
    app: TestApp,
    user: Id,
    credential: Id,
    bearer: String,
    digest: Vec<u8>,
    mode: Mode,
}
impl Scene {
    async fn new(mode: Mode) -> Proof<Self> {
        let mut transport = Fixture::new(false).await?;
        let mut registry = Registry::new();
        crate::routes::environments::mount(&mut registry);
        crate::routes::resources::mcp::mount(&mut registry);
        let app = TestApp::new(
            transport.link.inner.db.clone(),
            transport.clock.clone(),
            registry,
        );
        // The actual endpoint and the existing native transport share one production Link/dispatcher.
        transport.link = app.app.link.clone();
        transport.dispatcher = Dispatcher::new(transport.link.clone());
        let mut conn = transport.test.pool.acquire().await?;
        let (user, org): (Id, Id) =
            sqlx::query_as("SELECT owner_id,org_id FROM environments WHERE id=$1")
                .bind(transport.environment)
                .fetch_one(&mut *conn)
                .await?;
        access::testing::insert_member(&mut conn, org, user, org).await?;
        access::testing::name_owner(&mut conn, org, ("user", user), transport.clock.now()).await?;
        sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
            .bind(transport.machine)
            .bind(user)
            .execute(&mut *conn)
            .await?;
        for object in [
            ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Environment,
                id: transport.environment,
            },
            ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Machine,
                id: transport.machine,
            },
        ] {
            ownership::start_resource(
                &mut conn,
                object,
                user,
                ownership::NewAccess::OnlyMe,
                transport.clock.time(),
            )
            .await?;
        }
        let definition: McpDefinition = serde_json::from_value(json!({
            "spec":{"transport":"stdio","command":["isolated-native-mcp"],"secret_env":["FIXTURE_VALUE"]},
            "secrets":{"FIXTURE_VALUE":"raw-origin-private-value"}
        }))?;
        let fingerprint = definition.definition_fingerprint();
        tokio::fs::write(
            transport._directory.path().join("native-definition.json"),
            serde_json::to_vec(&definition)?,
        )
        .await?;
        let found = json!([{"source":"fixture","scope":"user","name":"search","directory":null,"transport":"stdio","digest":definition.digest().to_string(),"definition_fingerprint":fingerprint,"tools":null}]);
        sqlx::query(
            "UPDATE environments SET facts=jsonb_set(facts,'{mcp_found}',$2::jsonb) WHERE id=$1",
        )
        .bind(transport.environment)
        .bind(found)
        .execute(&mut *conn)
        .await?;
        sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1")
            .bind(transport.machine)
            .bind(json!(orch_link_contract::policy::LocalPolicy {
                mcp_import: true,
                ..Default::default()
            }))
            .execute(&mut *conn)
            .await?;
        let credential = new_id();
        let bearer = authentication::new_token(match mode {
            Mode::Key => authentication::API_TOKEN_PREFIX,
            Mode::Session => authentication::ACCESS_TOKEN_PREFIX,
        });
        let digest = authentication::token_hash(&bearer);
        let until = transport.clock.now() + chrono::Duration::hours(1);
        match mode {
            Mode::Key => {
                let policy: orch_access_contract::key_scope::KeyScope =
                    serde_json::from_value(json!({"rules":[]}))?;
                identity::create_api_token_scoped(
                    &mut conn,
                    &identity::NewApiToken {
                        id: credential,
                        user_id: user,
                        name: "Original MCP raw bearer",
                        token_hash: &digest,
                        scopes: &[],
                        node_id: None,
                        at: transport.clock.now(),
                        expires_at: Some(until),
                    },
                    Some(&json!(policy)),
                )
                .await?;
            }
            Mode::Session => {
                identity::testing::insert_session(&mut conn, credential, user, &digest, until)
                    .await?;
                sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
                    .bind(&digest)
                    .bind(transport.clock.now() + chrono::Duration::seconds(5))
                    .execute(&mut *conn)
                    .await?;
            }
        }
        drop(conn);
        let result = Self {
            transport,
            app,
            user,
            credential,
            bearer,
            digest,
            mode,
        };
        result.scope(Act::Create).await?;
        Ok(result)
    }
    async fn scope(&self, act: Act) -> Proof {
        if !matches!(self.mode, Mode::Key) {
            return Ok(());
        }
        let op = act.operation();
        let rule = |kind: &str, id: Id, resources: &[&str], actions: &[&str]| json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":resources,"allow":[{"operation":op,"actions":actions}]});
        let mut rules = vec![rule(
            "environment",
            self.transport.environment,
            &["environment", "mcp_server"],
            &["manage"],
        )];
        if let Act::Reimport(server) | Act::Offering(server, ..) = act {
            rules.push(rule("mcp_server", server, &["mcp_server"], &["manage"]));
            rules.push(rule(
                "machine",
                self.transport.machine,
                &["machine"],
                &["manage"],
            ));
        }
        let scope: orch_access_contract::key_scope::KeyScope =
            serde_json::from_value(json!({"rules":rules}))?;
        assert!(scope.validate().is_empty());
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(self.credential)
            .bind(json!(scope))
            .execute(&self.transport.test.pool)
            .await?;
        Ok(())
    }
    fn start(&self, act: Act) -> JoinHandle<RawReply> {
        let app = self.app.clone();
        let bearer = self.bearer.clone();
        let environment = self.transport.environment;
        let machine = self.transport.machine;
        tokio::spawn(async move {
            match act {
                Act::Create => {
                    let body = EnvironmentMcpImportCreateBody {
                        source: "fixture".into(),
                        scope: McpScope::User,
                        name: "search".into(),
                        directory: None,
                        as_name: Some("raw-origin-import".into()),
                        enable: Some(true),
                    };
                    let request = app
                        .call(EnvironmentMcpImportCreate, &[&environment])
                        .token(&bearer)
                        .body(&body);
                    request.send_bytes().await
                }
                Act::Reimport(server) => {
                    let request = app.call(McpServerReimport, &[&server]).token(&bearer);
                    request.send_bytes().await
                }
                Act::Offering(server, revision, fp) => {
                    let body = McpOfferingUseBody {
                        expected_revision: revision,
                        expected_fingerprint: Some(fp),
                    };
                    let request = app
                        .call(McpOfferingUse, &[&server, &machine])
                        .token(&bearer)
                        .body(&body);
                    request.send_bytes().await
                }
            }
        })
    }
    async fn command(&self, request: &JoinHandle<RawReply>) -> Proof<Id> {
        within(async {
            loop {
                if let Some(id)=sqlx::query_scalar::<_,Id>("SELECT id FROM commands WHERE machine_id=$1 AND kind='mcp.import' AND state IN('queued','sent','accepted') ORDER BY created_at,id DESC LIMIT 1")
                    .bind(self.transport.machine).fetch_optional(&self.transport.test.pool).await? {return Ok(id);}
                if request.is_finished() {return Err("the actual HTTP import ended before native dispatch".into());}
                tokio::task::yield_now().await;
            }
        }).await?
    }
    async fn rotate(&self) -> Proof {
        match self.mode {
            Mode::Key => {
                sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                    .bind(self.credential)
                    .bind(authentication::token_hash("a different fixture key bearer"))
                    .execute(&self.transport.test.pool)
                    .await?;
            }
            Mode::Session => {
                sqlx::query("DELETE FROM access_tokens WHERE token_hash=$1")
                    .bind(&self.digest)
                    .execute(&self.transport.test.pool)
                    .await?;
            }
        }
        Ok(())
    }
    async fn resource_count(&self) -> Proof<i64> {
        let rows: Vec<Option<Id>> = sqlx::query_scalar(
            "SELECT owner_id FROM mcp_servers WHERE machine_id=$1 AND deleted_at IS NULL",
        )
        .bind(self.transport.machine)
        .fetch_all(&self.transport.test.pool)
        .await?;
        assert!(rows.iter().all(|owner| *owner == Some(self.user)));
        Ok(i64::try_from(rows.len())?)
    }
    async fn seed(&mut self, offering: bool) -> Proof<Act> {
        within(self.transport.connect(false)).await??;
        let request = self.start(Act::Create);
        let result = within(request).await??;
        assert_eq!(
            result.status,
            http::StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&result.raw)
        );
        let server: Id = sqlx::query_scalar(
            "SELECT id FROM mcp_servers WHERE machine_id=$1 AND deleted_at IS NULL",
        )
        .bind(self.transport.machine)
        .fetch_one(&self.transport.test.pool)
        .await?;
        let command = if offering {
            json!(["isolated-native-mcp", "--updated"])
        } else {
            json!(["isolated-native-mcp"])
        };
        let definition: McpDefinition = serde_json::from_value(json!({
            "spec":{"transport":"stdio","command":command,"secret_env":["FIXTURE_VALUE"]},
            "secrets":{"FIXTURE_VALUE":"raw-origin-updated-private-value"}
        }))?;
        let fp = definition.definition_fingerprint();
        tokio::fs::write(
            self.transport
                ._directory
                .path()
                .join("native-definition.json"),
            serde_json::to_vec(&definition)?,
        )
        .await?;
        let source = json!({"source":"fixture","scope":"user","name":"search","directory":null,
            "transport":"stdio","digest":definition.digest().to_string(),"definition_fingerprint":fp,"tools":null});
        sqlx::query(
            "UPDATE environments SET facts=jsonb_set(facts,'{mcp_found}',$2::jsonb) WHERE id=$1",
        )
        .bind(self.transport.environment)
        .bind(json!([source.clone()]))
        .execute(&self.transport.test.pool)
        .await?;
        if offering {
            sqlx::query("UPDATE mcp_server_offerings SET state='found',reason='different_definition',proposal=$3 WHERE server_id=$1 AND machine_id=$2")
                .bind(server).bind(self.transport.machine)
                .bind(json!({"reason":"different_definition","environment":self.transport.environment,"source":source}))
                .execute(&self.transport.test.pool).await?;
        }
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM mcp_servers WHERE id=$1")
            .bind(server)
            .fetch_one(&self.transport.test.pool)
            .await?;
        let act = if offering {
            Act::Offering(server, revision, fp)
        } else {
            Act::Reimport(server)
        };
        self.scope(act).await?;
        self.transport.native.reads.store(0, Ordering::SeqCst);
        self.transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        Ok(act)
    }
    fn private_wire(&self) -> Proof {
        for wire in self
            .transport
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            let encoded = serde_json::to_string(wire)?;
            assert!(!encoded.contains(&self.bearer));
            assert!(!encoded.contains("raw-origin-private-value"));
            assert!(!encoded.contains("raw-origin-updated-private-value"));
            assert!(wire.body.get("authority").is_none());
        }
        Ok(())
    }
}

#[tokio::test]
async fn imported_mcp_keeps_its_raw_bearer_at_actual_entry_and_wire() -> Proof {
    // Exact access-token/hash continuity, not merely this still-live session's identity.
    for mode in [Mode::Key, Mode::Session] {
        for changed in [false, true] {
            let mut s = Scene::new(mode).await?;
            within(s.transport.connect(false)).await??;
            let (reached, release) = gate(s.transport.machine, Phase::Entry);
            let request = s.start(Act::Create);
            within(reached).await??;
            if changed {
                s.rotate().await?;
            }
            release
                .send(())
                .map_err(|_| "the actual entry gate ended")?;
            let result = within(request).await??;
            assert_eq!(
                result.status == http::StatusCode::CREATED,
                !changed,
                "{}",
                String::from_utf8_lossy(&result.raw)
            );
            if changed {
                assert!(result.status.is_client_error());
                assert!(
                    s.transport
                        .native
                        .wire
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .is_empty()
                );
            }
            assert_eq!(
                s.transport.native.reads.load(Ordering::SeqCst),
                usize::from(!changed)
            );
            assert_eq!(s.resource_count().await?, i64::from(!changed));
            s.private_wire()?;
            s.transport.close().await?;
        }
    }
    // The current row is extended only after the real web producer captured five seconds.
    // Extend at entry, before wire locks; then expire the original cut at the real wire gate.
    for expire in [false, true] {
        let mut s = Scene::new(Mode::Session).await?;
        within(s.transport.connect(false)).await??;
        let (entry, open_entry) = gate(s.transport.machine, Phase::Entry);
        let (wire, open_wire) = gate(s.transport.machine, Phase::MachineWire);
        let request = s.start(Act::Create);
        within(entry).await??;
        sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
            .bind(&s.digest)
            .bind(s.transport.clock.now() + chrono::Duration::hours(1))
            .execute(&s.transport.test.pool)
            .await?;
        open_entry
            .send(())
            .map_err(|_| "the actual entry gate ended")?;
        within(wire).await??;
        let id = s.command(&request).await?;
        if expire {
            s.transport.clock.advance(chrono::Duration::seconds(6));
        }
        open_wire
            .send(())
            .map_err(|_| "the actual wire gate ended")?;
        let result = within(request).await??;
        assert_eq!(
            result.status == http::StatusCode::CREATED,
            !expire,
            "{}",
            String::from_utf8_lossy(&result.raw)
        );
        if expire {
            assert!(result.status.is_client_error());
            assert!(s.transport.server.entry(id).is_none());
            assert!(
                !s.transport
                    .native
                    .wire
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter()
                    .any(|command| command.id == id)
            );
        }
        assert_eq!(
            s.transport.native.reads.load(Ordering::SeqCst),
            usize::from(!expire)
        );
        assert_eq!(s.resource_count().await?, i64::from(!expire));
        s.private_wire()?;
        s.transport.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn stored_mcp_reimport_and_offering_use_keep_the_same_raw_bearer_at_actual_entry() -> Proof {
    for offering in [false, true] {
        for changed in [false, true] {
            let mut s = Scene::new(Mode::Key).await?;
            let act = s.seed(offering).await?;
            let (entry, release) = gate(s.transport.machine, Phase::Entry);
            let request = s.start(act);
            within(entry).await??;
            if changed {
                s.rotate().await?;
            }
            release
                .send(())
                .map_err(|_| "the stored-copy entry gate ended")?;
            let result = within(request).await??;
            assert_eq!(
                result.status.is_success(),
                !changed,
                "{}",
                String::from_utf8_lossy(&result.raw)
            );
            assert_eq!(
                s.transport.native.reads.load(Ordering::SeqCst),
                usize::from(!changed)
            );
            assert_eq!(s.resource_count().await?, 1);
            s.private_wire()?;
            s.transport.close().await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_import_inverse_owns_only_the_successful_unfiled_copy_after_the_original_key_ends()
-> Proof {
    let mut s = Scene::new(Mode::Key).await?;
    within(s.transport.connect(false)).await??;
    let (wire, release_wire) = gate(s.transport.machine, Phase::MachineWire);
    let request = s.start(Act::Create);
    within(wire).await??;
    let id = s.command(&request).await?;
    let body: Json = sqlx::query_scalar("SELECT body FROM commands WHERE id=$1")
        .bind(id)
        .fetch_one(&s.transport.test.pool)
        .await?;
    let body: McpImport = serde_json::from_value(body)?;
    let (accepted, release_accepted) = gate(id, Phase::MirrorAccepted);
    release_wire
        .send(())
        .map_err(|_| "the native wire gate ended")?;
    within(accepted).await??;
    until(|| {
        s.transport.native.reads.load(Ordering::SeqCst) == 1
            && copy_path(&s.transport.native, body.server_id).is_file()
    })
    .await?;
    assert!(copy_path(&s.transport.native, body.server_id).is_file());
    s.rotate().await?;
    release_accepted
        .send(())
        .map_err(|_| "the actual native acknowledgement gate ended")?;
    let result = within(request).await??;
    assert!(
        result.status.is_client_error(),
        "{}",
        String::from_utf8_lossy(&result.raw)
    );
    until(|| s.transport.native.writes.load(Ordering::SeqCst) == 1).await?;
    assert!(!copy_path(&s.transport.native, body.server_id).exists());
    assert_eq!(s.resource_count().await?, 0);
    assert_eq!(s.transport.native.reads.load(Ordering::SeqCst), 1);
    let mut conn = s.transport.test.pool.acquire().await?;
    let authority = rows::command_with_authority(&mut conn, id)
        .await?
        .and_then(|row| row.authority)
        .ok_or("the raw forward authority is absent")?;
    assert_eq!(authority["kind"], "http_origin");
    let inverse_id: Id = sqlx::query_scalar(
        "SELECT id FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
    )
    .bind(body.server_id.to_string())
    .fetch_one(&mut *conn)
    .await?;
    let inverse = rows::command_with_authority(&mut conn, inverse_id)
        .await?
        .and_then(|row| row.authority)
        .ok_or("the owned inverse authority is absent")?;
    assert_eq!(inverse["kind"], "mcp_forget");
    drop(conn);
    assert_eq!(s.transport.native.writes.load(Ordering::SeqCst), 1);
    s.private_wire()?;
    s.transport.close().await?;
    Ok(())
}
