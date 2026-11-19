//! Stored-key, native-offering and copy/deletion custody checkpoints.

use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{environment_sources::MCP_IMPORT, routes::Endpoint};
use orch_authz::{Caller, CapturedOperation, TokenGrant};
use orch_db::queries::identity::{self, NewApiToken};
use orch_engine::operation_scope::continuing::Source;
use orch_engine::{
    environment_mcp::{
        importing::{Input, Offering},
        select_import,
    },
    operation_scope::{continuing, sources},
};
use orch_types::{Time, new_id};

const OPERATION: &str = "EnvironmentMcpImportCreate";

async fn stored_caller(
    world: &World,
    actor: Id,
    env: Id,
    operation: &str,
    resources: &[&str],
) -> Caller {
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"environment","id":env,"reach":"self"},
        "resources":resources,
        "allow":[{"operation":operation,"actions":["manage"]}],
    }]}))
    .unwrap();
    kept_caller(world, actor, scope).await
}

async fn kept_caller(world: &World, actor: Id, scope: KeyScope) -> Caller {
    assert!(scope.validate().is_empty());
    let id = new_id();
    let expires = world.now() + chrono::Duration::hours(1);
    let secret = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::API_TOKEN_PREFIX,
    );
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: actor,
            name: "Native MCP import source fixture",
            token_hash: &orch_api_core::middleware::authentication::token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(expires),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Caller::token(
        actor,
        TokenGrant::from_stored_scope(&[], None, Some(scope), Some(Time::new(expires)))
            .unwrap()
            .for_token(id),
    )
}

async fn offering(world: &World, env: Id) -> Input {
    let mut conn = world.conn().await;
    let selected = select_import(&mut conn, env, "codex", json!("user"), None, "search")
        .await
        .unwrap();
    Input::capture(&mut conn, &selected, new_id())
        .await
        .unwrap()
}

async fn origin(
    world: &World,
    caller: &Caller,
    input: &Input,
) -> Result<CapturedOperation, String> {
    CapturedOperation::capture(&admitted(world, caller, input).await?).map_err(|e| e.to_string())
}

async fn admitted(world: &World, caller: &Caller, input: &Input) -> Result<Caller, String> {
    let mut conn = world.conn().await;
    let (_, facts) = Offering::prepare(&mut conn, caller, input, Time::new(world.now()))
        .await
        .map_err(|e| e.to_string())?;
    let plan = sources::bind_required(EnvironmentMcpImportCreate::ROUTE, &MCP_IMPORT, &facts)
        .map_err(|e| e.to_string())?;
    plan.preparation_caller(caller).map_err(|e| e.to_string())
}

#[tokio::test]
async fn native_import_command_pins_the_actual_queue_frame_and_checks_current_sources_before_insert()
 {
    use orch_api_core::{dispatch::StoredCommand, thread_access::AuthzAccess};
    use orch_engine::{
        commands::{CommandAuthority, Outgoing},
        environment_mcp::CopyCommand,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    for change in ["unchanged", "offering", "scope", "expired"] {
        let world = World::new().await;
        let c = company(&world).await;
        let (env, machine) = environment(&world, &c, true).await;
        found(&world, env, "one").await;
        let input = offering(&world, env).await;
        let caller = stored_caller(
            &world,
            c.admin.id(),
            env,
            OPERATION,
            &["environment", "mcp_server"],
        )
        .await;
        let caller = admitted(&world, &caller, &input).await.unwrap();
        let command = Outgoing::new(new_id(), machine, input.serial(), &input.command()).unwrap();
        let evidence = CopyCommand::capture(&caller, None, input.clone(), &command).unwrap();
        assert_eq!(evidence.origin().operation(), OPERATION);
        assert_eq!(evidence.input().server(), input.server());
        let authority = CommandAuthority::mcp_import(evidence);
        let encoded = json!(authority);
        assert_eq!(encoded["kind"], "mcp_import");
        let authority: CommandAuthority = serde_json::from_value(encoded).unwrap();
        assert!(authority.requires_issue_checkpoint());
        assert!(!command.body.to_string().contains("authority"));
        let mut wrong = command.clone();
        wrong.body["name"] = json!("another-tool");
        assert!(CopyCommand::capture(&caller, None, input.clone(), &wrong).is_err());

        match change {
            "offering" => found(&world, env, "two").await,
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(caller.grant().unwrap().token().unwrap())
                    .bind(json!({"rules":[]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => world.advance(chrono::Duration::hours(2)),
            _ => (),
        }
        let cuts = Arc::new(AtomicUsize::new(0));
        let called = cuts.clone();
        let issued = world
            .app
            .app
            .link
            .dispatcher()
            .issue_stored_before(
                StoredCommand {
                    id: Some(command.id),
                    machine,
                    kind: "mcp.import".to_owned(),
                    key: command.key.wire(),
                    deadline: command.deadline,
                    body: command.body.clone(),
                    authority: Some(authority.clone()),
                },
                Box::new(move || {
                    called.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            )
            .await;
        assert_eq!(issued.is_ok(), change == "unchanged", "{change}");
        assert_eq!(
            cuts.load(Ordering::SeqCst),
            usize::from(change == "unchanged"),
            "{change}"
        );
        let mut conn = world.conn().await;
        let stored = orch_db::queries::link::command_with_authority(&mut conn, command.id)
            .await
            .unwrap();
        if change != "unchanged" {
            assert!(
                stored.is_none(),
                "the native queue never inserts a closed original frame"
            );
            continue;
        }
        let stored = stored.unwrap();
        assert_eq!(stored.command.body, command.body);
        assert_eq!(stored.authority, Some(json!(authority)));
        let access = AuthzAccess::new(world.app.app.clock.clone());
        drop(conn);
        let mut tx = world.db.pool.begin().await.unwrap();
        assert!(
            authority
                .current_callers(
                    &mut tx,
                    &stored.command,
                    world.app.app.clock.as_ref(),
                    &access
                )
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            !authority
                .permits_command(&mut tx, &stored.command, Time::new(world.now()))
                .await
                .unwrap(),
            "the sampled-time verifier cannot replace the native clock/source checkpoint"
        );
        for field in ["id", "body", "recipient", "lane"] {
            let mut replacement = stored.command.clone();
            match field {
                "id" => replacement.id = new_id(),
                "body" => replacement.body["name"] = json!("another-tool"),
                "recipient" => replacement.machine_id = new_id(),
                _ => replacement.serial_key = Some("machine".to_owned()),
            }
            assert!(
                authority
                    .current_callers(&mut tx, &replacement, world.app.app.clock.as_ref(), &access)
                    .await
                    .unwrap()
                    .is_none(),
                "{field}"
            );
        }
        tx.rollback().await.unwrap();
        found(&world, env, "two").await;
        let mut tx = world.db.pool.begin().await.unwrap();
        assert!(
            authority
                .current_callers(
                    &mut tx,
                    &stored.command,
                    world.app.app.clock.as_ref(),
                    &access
                )
                .await
                .unwrap()
                .is_none(),
            "the queued body cannot borrow a replacement native offering at send"
        );
        tx.rollback().await.unwrap();
    }
}

async fn found(world: &World, env: Id, digest: &str) {
    sqlx::query("UPDATE environments SET facts=jsonb_set(facts,'{mcp_found}',$2::jsonb) WHERE id=$1")
        .bind(env)
        .bind(json!([{"source":"codex","scope":"user","name":"search","directory":null,"transport":"stdio","digest":digest,"tools":2}]))
        .execute(&world.db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn native_import_checkpoint_requires_both_actual_parent_families_and_the_native_owner_gate() {
    let world = World::new().await;
    let c = company(&world).await;
    let (env, machine) = environment(&world, &c, true).await;
    found(&world, env, "one").await;
    let input = offering(&world, env).await;
    assert_eq!(input.environment(), env);
    assert_eq!(input.machine(), machine);
    assert_eq!(input.command().expected_fingerprint, None);
    assert_eq!(input.command().scope, "user");

    for (operation, resources) in [
        (OPERATION, vec!["environment"]),
        (OPERATION, vec!["mcp_server"]),
        (
            "EnvironmentMcpServerCreate",
            vec!["environment", "mcp_server"],
        ),
    ] {
        let caller = stored_caller(&world, c.admin.id(), env, operation, &resources).await;
        assert!(origin(&world, &caller, &input).await.is_err());
    }
    let caller = stored_caller(
        &world,
        c.admin.id(),
        env,
        OPERATION,
        &["environment", "mcp_server"],
    )
    .await;
    assert_eq!(input.owner(&caller), c.admin.id());
    let accepted = origin(&world, &caller, &input).await.unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    let frame = continuing::prepare::<Offering>(
        &mut tx,
        world.app.app.clock.as_ref(),
        &accepted,
        input.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        frame.principal().caller().admitted_operation(),
        Some(OPERATION)
    );
    assert_eq!(frame.value().command(input.server()), input.command());
    frame
        .into_parts()
        .first_effect
        .check(world.app.app.clock.as_ref())
        .unwrap();
    tx.rollback().await.unwrap();

    let stranger = stored_caller(
        &world,
        c.stranger.id(),
        env,
        OPERATION,
        &["environment", "mcp_server"],
    )
    .await;
    let stranger = origin(&world, &stranger, &input).await.unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        continuing::prepare::<Offering>(&mut tx, world.app.app.clock.as_ref(), &stranger, input)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_servers WHERE machine_id=$1")
        .bind(machine)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let commands: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='mcp.import'",
    )
    .bind(machine)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        (rows, commands),
        (0, 0),
        "native preparation performs no copy or filing"
    );
}

#[tokio::test]
async fn native_import_keeps_the_original_offering_and_current_key_without_reusing_a_new_selection()
{
    for change in ["unchanged", "offering", "generation", "scope", "expired"] {
        let world = World::new().await;
        let c = company(&world).await;
        let (env, _) = environment(&world, &c, true).await;
        found(&world, env, "one").await;
        let input = offering(&world, env).await;
        let caller = stored_caller(
            &world,
            c.admin.id(),
            env,
            OPERATION,
            &["environment", "mcp_server"],
        )
        .await;
        let accepted = origin(&world, &caller, &input).await.unwrap();
        let mut tx = world.db.pool.begin().await.unwrap();
        let opened = continuing::prepare::<Offering>(
            &mut tx,
            world.app.app.clock.as_ref(),
            &accepted,
            input,
        )
        .await
        .unwrap()
        .into_parts()
        .opened;
        tx.commit().await.unwrap();
        match change {
            "offering" => found(&world, env, "two").await,
            "generation" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(env)
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(caller.grant().unwrap().token().unwrap())
                    .bind(json!({"rules":[]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => world.advance(chrono::Duration::hours(2)),
            _ => (),
        }
        let mut tx = world.db.pool.begin().await.unwrap();
        let fresh = opened.refresh(&mut tx, world.app.app.clock.as_ref()).await;
        assert_eq!(fresh.is_ok(), change == "unchanged", "{change}");
        tx.rollback().await.unwrap();
        if matches!(change, "offering" | "generation") {
            let replacement = offering(&world, env).await;
            let mut tx = world.db.pool.begin().await.unwrap();
            assert!(
                continuing::prepare::<Offering>(
                    &mut tx,
                    world.app.app.clock.as_ref(),
                    &accepted,
                    replacement
                )
                .await
                .is_ok(),
                "a new real selection remains available to a fresh native act"
            );
            tx.rollback().await.unwrap();
        }
    }
}

#[tokio::test]
async fn native_import_inverse_owns_only_the_successful_unfiled_copy_after_the_original_key_ends() {
    use orch_api_core::{dispatch::StoredCommand, thread_access::AuthzAccess};
    use orch_db::queries::resources::mcp::{self, McpRow};
    use orch_engine::{
        commands::{CommandAuthority, Outgoing},
        environment_mcp::{CopyCommand, ForgetCommand},
    };
    use orch_link_contract::commands::{CommandBody, McpForget, McpImport, SerialKey};

    for state in [
        "unfiled",
        "queued",
        "missing_private",
        "changed_body",
        "filed",
        "retained",
        "expired_budget",
    ] {
        let world = World::new().await;
        let c = company(&world).await;
        let (env, machine) = environment(&world, &c, true).await;
        found(&world, env, "one").await;
        let input = offering(&world, env).await;
        let caller = stored_caller(
            &world,
            c.admin.id(),
            env,
            OPERATION,
            &["environment", "mcp_server"],
        )
        .await;
        let caller = admitted(&world, &caller, &input).await.unwrap();
        let command = Outgoing::new(new_id(), machine, input.serial(), &input.command()).unwrap();
        let copy = CopyCommand::capture(&caller, None, input.clone(), &command).unwrap();
        world
            .app
            .app
            .link
            .dispatcher()
            .issue_stored(StoredCommand {
                id: Some(command.id),
                machine,
                kind: "mcp.import".to_owned(),
                key: command.key.wire(),
                deadline: command.deadline,
                body: command.body.clone(),
                authority: Some(CommandAuthority::mcp_import(copy.clone())),
            })
            .await
            .unwrap();
        let cleanup = Outgoing::new(
            new_id(),
            machine,
            SerialKey::Machine,
            &McpForget {
                server_id: input.server(),
            },
        )
        .unwrap();
        assert!(
            ForgetCommand::capture(
                &mut *world.conn().await,
                world.app.app.clock.as_ref(),
                copy.clone(),
                &cleanup
            )
            .await
            .is_err(),
            "a queued command is never a successful copy receipt"
        );
        if state != "queued" {
            let replied = answer(
                &world,
                machine,
                json!({"transport":"stdio","digest":"one","tools":2}),
            );
            let result = world
                .app
                .app
                .link
                .dispatcher()
                .wait_for::<McpImport>(command.id)
                .await
                .unwrap();
            replied.await.unwrap();
            assert_eq!(result.digest, "one");
        }
        world.advance(chrono::Duration::hours(2));
        match state {
            "missing_private" => {
                sqlx::query("UPDATE commands SET login_authority=NULL WHERE id=$1")
                    .bind(command.id)
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "changed_body" => {
                sqlx::query("UPDATE commands SET body=jsonb_set(body,'{name}','\"another-tool\"') WHERE id=$1")
                .bind(command.id).execute(&world.db.pool).await.unwrap();
            }
            _ => (),
        }
        let captured = ForgetCommand::capture(
            &mut *world.conn().await,
            world.app.app.clock.as_ref(),
            copy.clone(),
            &cleanup,
        )
        .await;
        if matches!(state, "queued" | "missing_private" | "changed_body") {
            assert!(captured.is_err(), "{state}");
            continue;
        }
        let captured = captured.unwrap();
        let mut wrong = cleanup.clone();
        wrong.body["server_id"] = json!(new_id());
        assert!(
            ForgetCommand::capture(
                &mut *world.conn().await,
                world.app.app.clock.as_ref(),
                copy,
                &wrong
            )
            .await
            .is_err()
        );
        let until = captured.deadline().unwrap();
        let legacy = json!(captured);
        assert!(legacy.get("custody").is_none());
        assert!(legacy.get("copy").is_some());
        let restored: ForgetCommand = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(
            json!(restored),
            legacy,
            "existing unfiled-copy private records stay exact"
        );
        let authority = CommandAuthority::mcp_forget(captured);
        assert_eq!(authority.command_kind(), "mcp.forget");
        let authority: CommandAuthority = serde_json::from_value(json!(authority)).unwrap();
        if matches!(state, "filed" | "retained") {
            mcp::insert(
                &mut *world.conn().await,
                &McpRow {
                    imported: true,
                    id: input.server(),
                    name: "filed-tool".to_owned(),
                    description: String::new(),
                    org_id: Some(c.acme),
                    home_id: Some(c.acme),
                    owner_id: Some(c.admin.id()),
                    machine_id: Some(machine),
                    environment_id: None,
                    spec: json!({"transport":"stdio"}),
                    enabled: true,
                    revision: 1,
                },
                world.now(),
            )
            .await
            .unwrap();
            if state == "retained" {
                mcp::delete(&mut *world.conn().await, input.server(), world.now())
                    .await
                    .unwrap();
            }
        }
        if state == "expired_budget" {
            world.advance(chrono::Duration::seconds(
                i64::from(McpForget::KIND.deadline_secs()) + 1,
            ));
        }
        let issued = world
            .app
            .app
            .link
            .dispatcher()
            .issue_stored(StoredCommand {
                id: Some(cleanup.id),
                machine,
                kind: "mcp.forget".to_owned(),
                key: cleanup.key.wire(),
                deadline: Some(until),
                body: cleanup.body.clone(),
                authority: Some(authority.clone()),
            })
            .await;
        assert_eq!(issued.is_ok(), state == "unfiled", "{state}");
        let mut conn = world.conn().await;
        let stored = orch_db::queries::link::command(&mut conn, cleanup.id)
            .await
            .unwrap();
        if state == "unfiled" {
            let stored = stored.unwrap();
            let access = AuthzAccess::new(world.app.app.clock.clone());
            drop(conn);
            let mut tx = world.db.pool.begin().await.unwrap();
            let callers = authority
                .current_callers(&mut tx, &stored, world.app.app.clock.as_ref(), &access)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(callers.len(), 1);
            assert!(
                callers[0].0.is_none(),
                "owned cleanup cannot create or renew any caller"
            );
            assert_eq!(callers[0].1, Some(until));
            tx.rollback().await.unwrap();
        } else {
            assert!(stored.is_none(), "{state}");
        }
    }
}

#[tokio::test]
async fn recorded_deletion_custody_requires_its_actual_journal_and_never_admits_fresh_forget() {
    use orch_access_contract::{
        decisions::ObjectRef,
        key_scope::KeyTargetKind,
        permissions::{ObjectKind, Permission, ResourceKind},
    };
    use orch_api_contract::{
        mcp_actions::{DELETE, Source as DeleteSource},
        resources::McpServerDelete,
    };
    use orch_api_core::{dispatch::StoredCommand, thread_access::AuthzAccess};
    use orch_authz::{
        Policy, Principal,
        ownership::{NewAccess, start_resource},
    };
    use orch_db::queries::{
        link,
        resources::mcp::{self as rows, McpRow},
    };
    use orch_engine::{
        commands::{CommandAuthority, Outgoing},
        environment_mcp::{ForgetCommand, ForgetCustody},
        operation_scope::targets,
    };
    use orch_link_contract::commands::{CommandBody, McpForget, SerialKey};

    let world = World::new().await;
    let c = company(&world).await;
    let (_, machine) = environment(&world, &c, true).await;
    let server = new_id();
    let source = json!({"source":"fixture","scope":"user","name":"recorded-tool","digest":"one"});
    rows::insert(
        &mut *world.conn().await,
        &McpRow {
            imported: true,
            id: server,
            name: "recorded-tool".into(),
            description: String::new(),
            org_id: Some(c.acme),
            home_id: Some(c.acme),
            owner_id: Some(c.admin.id()),
            machine_id: Some(machine),
            environment_id: None,
            spec: json!({"transport":"stdio","imported":source}),
            enabled: true,
            revision: 1,
        },
        world.now(),
    )
    .await
    .unwrap();
    start_resource(
        &mut *world.conn().await,
        ObjectRef {
            kind: ResourceKind::McpServer.into(),
            id: server,
        },
        c.admin.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let scope: KeyScope = serde_json::from_value(json!({"rules":[
        {"target":{"kind":"mcp_server","id":server,"reach":"self"},"allow":[{"operation":"McpServerDelete","actions":["manage"]}]},
        {"target":{"kind":"machine","id":machine,"reach":"self"},"allow":[{"operation":"McpServerDelete","actions":["manage"]}]}
    ]})).unwrap();
    let caller = kept_caller(&world, c.admin.id(), scope).await;
    let key = caller.grant().unwrap().token().unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    let offering = rows::offerings(&mut tx, &[server]).await.unwrap().remove(0);
    let facts = [
        sources::Facts::one(
            DeleteSource::ManagedServer,
            KeyTargetKind::Object(ObjectKind::McpServer),
            targets::page_object(
                &mut tx,
                ObjectRef {
                    kind: ObjectKind::McpServer,
                    id: server,
                },
            )
            .await
            .unwrap(),
        ),
        sources::Facts::collection(
            DeleteSource::ForgottenCopies,
            vec![(
                KeyTargetKind::Object(ObjectKind::Machine),
                vec![targets::machine_subject(&mut tx, machine).await.unwrap()],
            )],
        ),
    ];
    let admitted = sources::bind_required(McpServerDelete::ROUTE, &DELETE, &facts)
        .unwrap()
        .admit(&caller, false)
        .unwrap();
    let principal = Principal::load(&mut tx, admitted, Time::new(world.now()))
        .await
        .unwrap();
    assert!(
        orch_authz::resources::check(
            &mut tx,
            &principal,
            ObjectRef {
                kind: ObjectKind::McpServer,
                id: server
            },
            Permission::McpManage,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    let command = Outgoing::new(
        new_id(),
        machine,
        SerialKey::Machine,
        &McpForget { server_id: server },
    )
    .unwrap();
    let proof = ForgetCommand::capture_recorded_deletion(
        &mut tx,
        principal.caller(),
        &offering,
        Time::new(world.now()),
        &command,
    )
    .await
    .unwrap();
    assert!(matches!(
        proof.custody(),
        ForgetCustody::RecordedDeletion(_)
    ));
    assert_eq!(
        proof.deadline(),
        None,
        "the deletion journal supplies its own native command deadline"
    );
    let encoded = json!(proof);
    assert_eq!(encoded["custody"]["kind"], "recorded_deletion");
    let proof: ForgetCommand = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(json!(proof), encoded);
    let authority = CommandAuthority::mcp_forget(proof);
    let until = world.now() + chrono::Duration::seconds(i64::from(McpForget::KIND.deadline_secs()));
    let access = AuthzAccess::new(world.app.app.clock.clone());
    let lane = command.key.wire();
    let proposed = link::NewCommand {
        id: command.id,
        machine_id: machine,
        kind: "mcp.forget",
        body: &command.body,
        serial_key: lane.as_deref(),
        deadline_at: until,
    };
    assert!(
        authority
            .current_issue_callers(&mut tx, &proposed, world.app.app.clock.as_ref(), &access)
            .await
            .unwrap()
            .is_none(),
        "deletion custody can never grant an insertion"
    );
    rows::delete(&mut tx, server, world.now()).await.unwrap();
    assert!(
        link::insert_command_with_authority(
            &mut tx,
            &proposed,
            world.now(),
            Some(&json!(authority))
        )
        .await
        .unwrap()
    );
    tx.commit().await.unwrap();

    let saved = link::command_with_authority(&mut *world.conn().await, command.id)
        .await
        .unwrap()
        .unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    let callers = authority
        .current_callers(
            &mut tx,
            &saved.command,
            world.app.app.clock.as_ref(),
            &access,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(callers.len(), 1);
    assert!(callers[0].0.is_none());
    assert_eq!(callers[0].1, Some(Time::new(until)));
    tx.rollback().await.unwrap();
    world.advance(chrono::Duration::seconds(2));
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &saved.command,
                world.app.app.clock.as_ref(),
                &access
            )
            .await
            .unwrap()
            .is_some(),
        "accepted reducing cleanup does not renew its ended original key"
    );
    for field in ["id", "body", "machine", "lane", "deadline"] {
        let mut different = saved.command.clone();
        match field {
            "id" => different.id = new_id(),
            "body" => different.body["server_id"] = json!(new_id()),
            "machine" => different.machine_id = new_id(),
            "lane" => different.serial_key = Some("another-lane".into()),
            _ => different.deadline_at += chrono::Duration::seconds(1),
        }
        assert!(
            authority
                .current_callers(&mut tx, &different, world.app.app.clock.as_ref(), &access)
                .await
                .unwrap()
                .is_none(),
            "{field}"
        );
    }
    tx.rollback().await.unwrap();
    let fresh = new_id();
    assert!(
        world
            .app
            .app
            .link
            .dispatcher()
            .issue_stored(StoredCommand {
                id: Some(fresh),
                machine,
                kind: "mcp.forget".into(),
                key: command.key.wire(),
                deadline: Some(Time::new(until)),
                body: command.body.clone(),
                authority: Some(authority.clone()),
            })
            .await
            .is_err()
    );
    assert!(
        link::command(&mut *world.conn().await, fresh)
            .await
            .unwrap()
            .is_none()
    );
    for damage in ["journal", "private", "source", "active"] {
        let mut tx = world.db.pool.begin().await.unwrap();
        match damage {
            "journal" => {
                sqlx::query("DELETE FROM commands WHERE id=$1")
                    .bind(command.id)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            "private" => {
                sqlx::query("UPDATE commands SET login_authority=NULL WHERE id=$1")
                    .bind(command.id)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            "source" => {
                sqlx::query("UPDATE mcp_server_offerings SET source=$2 WHERE server_id=$1")
                    .bind(server)
                    .bind(json!({"source":"another-copy"}))
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("UPDATE mcp_servers SET deleted_at=NULL WHERE id=$1")
                    .bind(server)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
        }
        assert!(
            authority
                .current_callers(
                    &mut tx,
                    &saved.command,
                    world.app.app.clock.as_ref(),
                    &access
                )
                .await
                .unwrap()
                .is_none(),
            "{damage}"
        );
        tx.rollback().await.unwrap();
    }
    let mut malformed = json!(authority);
    malformed["authority"]["custody"]["kind"] = json!("unknown-proof");
    assert!(serde_json::from_value::<CommandAuthority>(malformed).is_err());
    world.advance(chrono::Duration::seconds(i64::from(
        McpForget::KIND.deadline_secs(),
    )));
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        authority
            .current_callers(
                &mut tx,
                &saved.command,
                world.app.app.clock.as_ref(),
                &access
            )
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
}
