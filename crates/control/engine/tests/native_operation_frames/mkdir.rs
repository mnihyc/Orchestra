//! Genuine mkdir source and raw key checks at the common command boundary.
use super::*;
use orch_access_contract::decisions::ObjectRef;
use orch_api_contract::{workspace::EnvironmentFileMkdir, workspace_sources};
use orch_db::queries::{environments::workspace, link::NewCommand};
use orch_engine::{
    commands::{CommandAuthority, FileMkdirCommand, FileMkdirInput, Outgoing},
    operation_scope::{
        http_command_source::CommandEvidence, http_origin::HttpOrigin, request_origin::Credential,
    },
};
use orch_link_contract::commands::{CommandKind, SerialKey};

async fn setup(world: &World) -> (Id, Caller, HttpOrigin, FileMkdirInput, Outgoing) {
    let token = new_id();
    let hash = Hash::of(token.as_bytes());
    let expires = Time::new(world.clock.now() + chrono::Duration::hours(1));
    let policy: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"environment","id":world.environment,"reach":"self"},
        "allow":[{"operation":"EnvironmentFileMkdir","actions":["write"]}]
    }]}))
    .unwrap();
    assert!(policy.validate().is_empty());
    let mut tx = world.pool.begin().await.unwrap();
    identity::create_api_token_scoped(
        &mut tx,
        &identity::NewApiToken {
            id: token,
            user_id: world.ada,
            name: "Native mkdir source",
            token_hash: hash.as_bytes(),
            scopes: &[],
            node_id: None,
            at: world.clock.now(),
            expires_at: Some(expires.get()),
        },
        Some(&json!(policy)),
    )
    .await
    .unwrap();
    let caller = Caller::token(
        world.ada,
        TokenGrant::from_stored_scope(&[], None, Some(policy), Some(expires))
            .unwrap()
            .for_token(token),
    );
    let target = targets::resource_identity(
        &mut tx,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: world.environment,
        },
    )
    .await
    .unwrap();
    let caller = sources::bind_required(
        EnvironmentFileMkdir::ROUTE,
        &workspace_sources::ENVIRONMENT_FILE_MKDIR,
        &[Facts::one(
            workspace_sources::Source::MkdirEnvironment,
            target.object.kind,
            target,
        )],
    )
    .unwrap()
    .admit(&caller, false)
    .unwrap();
    let principal = Principal::load(&mut tx, caller.clone(), world.clock.time())
        .await
        .unwrap();
    let http = HttpOrigin::capture(
        CapturedOperation::capture(&caller).unwrap(),
        Credential::Token {
            id: token,
            hash: hash.as_bytes().to_vec(),
        },
        false,
        principal.has_system_admin_standing(),
        Some(expires),
    )
    .unwrap();
    let env = workspace::environment(&mut tx, world.environment)
        .await
        .unwrap()
        .unwrap();
    let input = FileMkdirInput::new(&env, world.machine_id, None, "native-created".into());
    let outgoing = Outgoing {
        id: new_id(),
        machine: world.machine_id,
        key: input.serial(),
        kind: CommandKind::FsMkdir,
        body: json!(input.command()),
        deadline: Some(Time::new(world.clock.now() + chrono::Duration::minutes(2))),
        authority: None,
    };
    tx.commit().await.unwrap();
    (token, caller, http, input, outgoing)
}

async fn check(
    world: &World,
    authority: &CommandAuthority,
    command: &Outgoing,
) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
    let mut tx = world.pool.begin().await.unwrap();
    let key = command.key.wire();
    authority
        .current_issue_callers(
            &mut tx,
            &NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: key.as_deref(),
                deadline_at: command.deadline.unwrap().get(),
            },
            world.clock.as_ref(),
            world.access.as_ref(),
        )
        .await
}

#[tokio::test]
async fn mkdir_keeps_its_actual_write_only_key_and_exact_native_frame_without_copy_read() {
    let clock = ManualClock::new(chrono::Utc::now());
    let world = World::with_clock(Arc::new(clock.clone())).await;
    world.engine().stop();
    let (_, caller, http, input, command) = setup(&world).await;
    let native = FileMkdirCommand::capture(&caller, input.clone(), &command).unwrap();
    let carried: CommandEvidence =
        serde_json::from_value(json!(CommandEvidence::FileMkdir(native))).unwrap();
    let authority = http.command_source(&command, carried).unwrap();
    let current = check(&world, &authority, &command).await.unwrap().unwrap();
    let actual = current[0].0.as_ref().unwrap();
    assert_eq!(actual.user(), world.ada);
    assert!(
        actual.grant().is_some(),
        "a raw API key never becomes a Session"
    );
    assert_eq!(actual.admitted_operation(), Some("EnvironmentFileMkdir"));
    assert!(
        actual
            .admitted_facets()
            .unwrap()
            .iter()
            .all(|facet| facet.action == orch_access_contract::key_scope::KeyAction::Write)
    );
    assert_eq!(input.path(), "native-created");
    let mut changed = command.clone();
    changed.id = new_id();
    assert!(check(&world, &authority, &changed).await.unwrap().is_none());
    let mut wrong = command.clone();
    wrong.body["path"] = json!("another-directory");
    assert!(check(&world, &authority, &wrong).await.unwrap().is_none());
    let mut wrong = command.clone();
    wrong.key = SerialKey::Runtime(world.environment);
    assert!(FileMkdirCommand::capture(&caller, input.clone(), &wrong).is_err());
    let mut wrong = command.clone();
    wrong.deadline = Some(Time::new(
        command.deadline.unwrap().get() + chrono::Duration::seconds(1),
    ));
    assert!(check(&world, &authority, &wrong).await.unwrap().is_none());
    clock.advance(chrono::Duration::minutes(3));
    let closed = check(&world, &authority, &command).await;
    assert!(
        matches!(closed, Ok(None) | Err(EngineError::BeforeEffect(_))),
        "{closed:?}"
    );
}

#[tokio::test]
async fn mkdir_rechecks_current_key_physical_workspace_and_native_remote_write() {
    for change in [
        "scope",
        "revoke",
        "hash",
        "generation",
        "root",
        "remote_write",
        "thread",
    ] {
        let world = World::new().await;
        world.engine().stop();
        let (token, caller, http, mut input, command) = setup(&world).await;
        if change == "thread" {
            let env =
                workspace::environment(&mut world.pool.acquire().await.unwrap(), world.environment)
                    .await
                    .unwrap()
                    .unwrap();
            input =
                FileMkdirInput::new(&env, world.machine_id, Some(new_id()), input.path().into());
        }
        let native = FileMkdirCommand::capture(&caller, input, &command).unwrap();
        let authority = http
            .command_source(&command, CommandEvidence::FileMkdir(native))
            .unwrap();
        match change {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(token).bind(json!({"rules":[{"target":{"kind":"environment","id":world.environment,"reach":"self"},"allow":[{"operation":"EnvironmentFileCopy","actions":["read","write"]}]}]})).execute(world.pool.pool()).await.unwrap();
            }
            "revoke" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(token)
                    .bind(world.clock.now())
                    .execute(world.pool.pool())
                    .await
                    .unwrap();
            }
            "hash" => {
                sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                    .bind(token)
                    .bind(Hash::of(b"replaced raw token").as_bytes().to_vec())
                    .execute(world.pool.pool())
                    .await
                    .unwrap();
            }
            "generation" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(world.environment)
                    .execute(world.pool.pool())
                    .await
                    .unwrap();
            }
            "root" => {
                sqlx::query("UPDATE environments SET workspace_root='changed-root' WHERE id=$1")
                    .bind(world.environment)
                    .execute(world.pool.pool())
                    .await
                    .unwrap();
            }
            "remote_write" => {
                sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
                    .bind(world.environment)
                    .execute(world.pool.pool())
                    .await
                    .unwrap();
            }
            "thread" => {}
            _ => unreachable!(),
        }
        let result = check(&world, &authority, &command).await;
        assert!(
            matches!(result, Ok(None) | Err(EngineError::BeforeEffect(_))),
            "{change}: {result:?}"
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='fs.mkdir'")
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "source checks confer no directory effect or command insert"
        );
    }
}
