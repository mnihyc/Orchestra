//! The real stored import frame, original HTTP carrier and ACK decide native snapshot duplication.
use super::*;
#[path = "imported/permission_rules.rs"]
mod permission_rules;
use orch_api_contract::environment::{
    EnvironmentMcpImportCreate, EnvironmentMcpImportCreateBody, McpScope,
};
use orch_api_core::thread_access::AuthzAccess;
use orch_db::queries::link;
use orch_engine::commands::CommandAuthority;
use orch_link_contract::commands::McpImportResult;
use tokio::task::JoinHandle;

async fn queued(
    s: &Scene,
    request: &JoinHandle<RawReply>,
) -> Proof<(link::CommandRow, CommandAuthority)> {
    let id=tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            let id:Option<Id>=sqlx::query_scalar("SELECT id FROM commands WHERE kind='mcp.import' AND state='queued' ORDER BY created_at,id LIMIT 1").fetch_optional(&s.world.db.pool).await.unwrap();
            if let Some(id)=id {break id;}
            assert!(!request.is_finished(),"the actual request must reach its native copy queue");
            tokio::task::yield_now().await;
        }
    }).await?;
    let mut tx = s.world.db.pool.begin().await?;
    let stored = link::command_with_authority(&mut tx, id)
        .await?
        .ok_or("actual native command absent")?;
    let authority: CommandAuthority =
        serde_json::from_value(stored.authority.ok_or("private native source absent")?)?;
    let access = AuthzAccess::new(s.world.app.app.clock.clone());
    assert!(
        authority
            .current_callers(
                &mut tx,
                &stored.command,
                s.world.app.app.clock.as_ref(),
                &access
            )
            .await?
            .is_some()
    );
    assert!(link::mark_accepted(&mut tx, id, s.world.now()).await?);
    tx.commit().await?;
    Ok((stored.command, authority))
}
async fn finish(
    s: &Scene,
    row: &link::CommandRow,
    authority: &CommandAuthority,
    digest: &str,
) -> Proof {
    let result = McpImportResult {
        snapshot: None,
        definition_fingerprint: None,
        transport: "stdio".into(),
        digest: digest.into(),
        tools: None,
    };
    finish_result(s, row, authority, result).await
}
async fn finish_result(
    s: &Scene,
    row: &link::CommandRow,
    authority: &CommandAuthority,
    result: McpImportResult,
) -> Proof {
    let result = serde_json::to_value(result)?;
    let mut tx = s.world.db.pool.begin().await?;
    let ack = authority.mcp_import_ack(row)?;
    if let Some(ack) = &ack {
        if ack.changes_authority() {
            orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        }
        ack.lock_source(&mut tx).await?;
    }
    assert!(
        link::finish_command(
            &mut tx,
            row.id,
            &link::Ending::Succeeded(&result),
            s.world.now()
        )
        .await?
    );
    if let Some(ack) = ack {
        ack.reconcile(&mut tx, s.world.app.app.clock.as_ref())
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
async fn scene() -> Proof<(Scene, Id)> {
    let mut s = Scene::new().await?;
    let (_, env, machine) = own_source(&s).await?;
    // Imports follow the machine's registered home, including for a personal machine.
    // These scopes and disappearance controls deliberately use the company home.
    sqlx::query("UPDATE machines SET roles=ARRAY['environment']::text[],home_id=$2 WHERE id=$1")
        .bind(machine)
        .bind(s.company.acme)
        .execute(&s.world.db.pool)
        .await?;
    sqlx::query("UPDATE environments SET facts=jsonb_set(facts,'{mcp_found}',$2::jsonb) WHERE id=$1")
        .bind(env).bind(json!([{"source":"codex","scope":"user","name":"search","directory":null,"transport":"stdio","digest":"source-snapshot"}])).execute(&s.world.db.pool).await?;
    let app = s.world.app.clone();
    let token = s.company.admin.token.clone();
    let waiting = tokio::spawn(async move {
        let request = app
            .call(EnvironmentMcpImportCreate, &[&env])
            .token(&token)
            .body(&EnvironmentMcpImportCreateBody {
                source: "codex".into(),
                scope: McpScope::User,
                name: "search".into(),
                directory: None,
                as_name: Some("stored-search".into()),
                enable: Some(false),
            });
        request.send_bytes().await
    });
    let (row, authority) = queued(&s, &waiting).await?;
    finish(&s, &row, &authority, "source-snapshot").await?;
    let response = tokio::time::timeout(std::time::Duration::from_secs(30), waiting).await??;
    assert!(
        response.status.is_success(),
        "{}",
        String::from_utf8_lossy(&response.raw)
    );
    s.source = serde_json::from_value(row.body["server_id"].clone())?;
    let stored = rows::servers(&mut *s.world.conn().await, &[s.source])
        .await?
        .pop()
        .ok_or("actual filed native import absent")?;
    assert!(stored.imported);
    assert_eq!(stored.machine_id, Some(machine));
    assert_eq!(stored.home_id, Some(s.company.acme));
    assert_eq!(stored.org_id, Some(s.company.acme));
    Ok((s, machine))
}
fn start(s: &Scene, token: String, key: Option<String>) -> JoinHandle<RawReply> {
    let app = s.world.app.clone();
    let source = s.source;
    tokio::spawn(async move {
        let request = app
            .call(McpServerDuplicate, &[&source])
            .token(&token)
            .body(&McpDuplicateBody::default());
        let request = match &key {
            Some(key) => request.header("idempotency-key", key),
            None => request,
        };
        request.send_bytes().await
    })
}
#[tokio::test]
async fn imported_duplicates_keep_the_exact_snapshot_and_actual_ownership_choice_without_a_fake_environment()
-> Proof {
    let (s, machine) = scene().await?;
    for (actor, owner) in [(&s.company.admin, true), (&s.company.viewer, false)] {
        if !owner {
            resources::name_person(&s.world, "mcp_server", s.source, "mcp.view", actor.id()).await;
            resources::name_person(&s.world, "machine", machine, "machine.use", actor.id()).await;
        }
        let reader = rule("McpServerDuplicate", "mcp_server", s.source, "self", "read");
        let writer = rule("McpServerDuplicate", "org", s.company.acme, "self", "write");
        let scoped = key(
            &s.world,
            actor.id(),
            vec![
                reader,
                writer,
                rule("McpServerDuplicate", "machine", machine, "self", "write"),
            ],
        )
        .await?;
        let waiting = start(&s, scoped.secret, None);
        let (native, authority) = queued(&s, &waiting).await?;
        let copy = &native.body["stored"];
        assert_eq!(copy["server_id"], json!(s.source));
        assert_eq!(copy["expected_digest"], "source-snapshot");
        assert_eq!(copy["include_secrets"], owner);
        assert!(native.body.get("environment_id").is_none());
        assert_eq!(native.serial_key(), Some("machine"));
        assert_eq!(native.machine_id, machine);
        assert_ne!(native.body["server_id"], json!(s.source));
        finish(
            &s,
            &native,
            &authority,
            if owner {
                "owner-snapshot"
            } else {
                "scrubbed-snapshot"
            },
        )
        .await?;
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), waiting).await??;
        let made = id(&response)?;
        let row = rows::servers(&mut *s.world.conn().await, &[made])
            .await?
            .pop()
            .ok_or("native copied server absent")?;
        assert!(row.imported);
        assert_eq!(row.owner_id, Some(actor.id()));
        assert_eq!(row.machine_id, Some(machine));
        assert_eq!(row.name, "stored-search-copy");
        assert!(row.spec.get("command").is_none());
        assert_eq!(
            row.spec["imported"]["digest"],
            if owner {
                "owner-snapshot"
            } else {
                "scrubbed-snapshot"
            }
        );
        let mut tx = s.world.db.pool.begin().await?;
        let ack = authority
            .mcp_import_ack(&native)?
            .ok_or("stored-copy ACK absent")?;
        ack.lock_source(&mut tx).await?;
        ack.reconcile(&mut tx, s.world.app.app.clock.as_ref())
            .await?;
        tx.commit().await?;
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='mcp.imported'",
        )
        .bind(made)
        .fetch_one(&s.world.db.pool)
        .await?;
        assert_eq!(audits, 1);
    }
    let original = rows::servers(&mut *s.world.conn().await, &[s.source])
        .await?
        .pop()
        .ok_or("original absent")?;
    assert_eq!(original.spec["imported"]["digest"], "source-snapshot");
    Ok(())
}
#[tokio::test]
async fn a_native_duplicate_header_retains_its_pending_identity_and_ack_after_the_original_key_ends()
-> Proof {
    let (s, machine) = scene().await?;
    let scoped = key(
        &s.world,
        s.company.admin.id(),
        vec![
            rule("McpServerDuplicate", "mcp_server", s.source, "self", "read"),
            rule("McpServerDuplicate", "org", s.company.acme, "self", "write"),
            rule("McpServerDuplicate", "machine", machine, "self", "write"),
        ],
    )
    .await?;
    let retry = orch_types::new_id().to_string();
    let waiting = start(&s, scoped.secret.clone(), Some(retry.clone()));
    let (native, authority) = queued(&s, &waiting).await?;
    let made: Id = serde_json::from_value(native.body["server_id"].clone())?;
    waiting.abort();
    let _ = waiting.await;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(scoped.id)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await?;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.import'")
        .fetch_one(&s.world.db.pool)
        .await?;
    let response = s
        .world
        .app
        .call(McpServerDuplicate, &[&s.source])
        .token(&s.company.admin.token)
        .header("idempotency-key", &retry)
        .body(&McpDuplicateBody::default())
        .send_bytes()
        .await;
    assert_eq!(id(&response)?, made);
    assert!(
        rows::servers(&mut *s.world.conn().await, &[made])
            .await?
            .is_empty()
    );
    finish(&s, &native, &authority, "retained-owner-copy").await?;
    let actual = rows::servers(&mut *s.world.conn().await, &[made])
        .await?
        .pop()
        .ok_or("completed accepted native copy absent")?;
    assert_eq!(actual.owner_id, Some(s.company.admin.id()));
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.import'")
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(after, before);
    let mut changed = native.clone();
    changed.body["stored"]["server_id"] = json!(orch_types::new_id());
    assert!(authority.mcp_import_ack(&changed).is_err());
    Ok(())
}

#[tokio::test]
async fn a_stored_copy_rechecks_its_own_scoped_source_raw_bearer_owner_and_machine_policy_before_new_send()
-> Proof {
    for change in ["live", "scope", "bearer", "source", "owner", "policy"] {
        let (s, machine) = scene().await?;
        let scoped = key(
            &s.world,
            s.company.admin.id(),
            vec![
                rule("McpServerDuplicate", "mcp_server", s.source, "self", "read"),
                rule("McpServerDuplicate", "org", s.company.acme, "self", "write"),
                rule("McpServerDuplicate", "machine", machine, "self", "write"),
            ],
        )
        .await?;
        let waiting = start(&s, scoped.secret.clone(), None);
        let (row, authority) = queued(&s, &waiting).await?;
        match change {
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(scoped.id)
                    .bind(json!({"rules":[]}))
                    .execute(&s.world.db.pool)
                    .await?;
            }
            "bearer" => {
                sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                    .bind(scoped.id)
                    .bind(
                        orch_types::Hash::of(b"different synthetic raw bearer")
                            .as_bytes()
                            .to_vec(),
                    )
                    .execute(&s.world.db.pool)
                    .await?;
            }
            "source" => {
                sqlx::query("UPDATE mcp_servers SET spec=jsonb_set(spec,'{imported,digest}','\"another-snapshot\"'::jsonb) WHERE id=$1").bind(s.source).execute(&s.world.db.pool).await?;
            }
            "owner" => {
                resources::name_person(
                    &s.world,
                    "mcp_server",
                    s.source,
                    "mcp.view",
                    s.company.admin.id(),
                )
                .await;
                resources::name_person(
                    &s.world,
                    "mcp_server",
                    s.source,
                    "access.own",
                    s.company.viewer.id(),
                )
                .await;
                sqlx::query("UPDATE permission_entries SET removed_at=$3 WHERE object_kind='mcp_server' AND object_id=$1 AND permission='access.own' AND holder_id=$2 AND removed_at IS NULL").bind(s.source).bind(s.company.admin.id()).bind(s.world.now()).execute(&s.world.db.pool).await?;
            }
            "policy" => {
                sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,mcp.import}','false') WHERE id=$1").bind(machine).execute(&s.world.db.pool).await?;
            }
            _ => {}
        }
        let access = AuthzAccess::new(s.world.app.app.clock.clone());
        let mut tx = s.world.db.pool.begin().await?;
        assert_eq!(
            authority
                .current_callers(&mut tx, &row, s.world.app.app.clock.as_ref(), &access)
                .await?
                .is_some(),
            change == "live",
            "{change}"
        );
        tx.rollback().await?;
        waiting.abort();
        let _ = waiting.await;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM commands WHERE kind='mcp.import' AND body ? 'stored'",
        )
        .fetch_one(&s.world.db.pool)
        .await?;
        assert_eq!(count, 1, "the native checker cannot replace the request");
    }
    Ok(())
}

#[tokio::test]
async fn a_completed_snapshot_whose_creation_home_disappeared_owns_only_its_unfiled_file_cleanup()
-> Proof {
    let (s, machine) = scene().await?;
    let waiting = start(&s, s.company.admin.token.clone(), None);
    let (row, authority) = queued(&s, &waiting).await?;
    let made: Id = serde_json::from_value(row.body["server_id"].clone())?;
    waiting.abort();
    let _ = waiting.await;
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(s.company.acme)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await?;
    finish(&s, &row, &authority, "accepted-snapshot").await?;
    assert!(
        rows::servers(&mut *s.world.conn().await, &[made])
            .await?
            .is_empty()
    );
    let mut tx = s.world.db.pool.begin().await?;
    let cleanup = sqlx::query_scalar::<_, Id>(
        "SELECT id FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
    )
    .bind(made.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let native = link::command_with_authority(&mut tx, cleanup)
        .await?
        .ok_or("owned native inverse absent")?;
    assert_eq!(native.command.machine_id, machine);
    let inverse: CommandAuthority =
        serde_json::from_value(native.authority.ok_or("native cleanup proof absent")?)?;
    let access = AuthzAccess::new(s.world.app.app.clock.clone());
    let callers = inverse
        .current_callers(
            &mut tx,
            &native.command,
            s.world.app.app.clock.as_ref(),
            &access,
        )
        .await?
        .ok_or("unfiled exact inverse should remain usable")?;
    assert!(
        callers.iter().all(|(caller, _)| caller.is_none()),
        "an inverse cannot invent or restore a user"
    );
    let mut wrong = native.command.clone();
    wrong.body["server_id"] = json!(s.source);
    assert!(
        inverse
            .current_callers(&mut tx, &wrong, s.world.app.app.clock.as_ref(), &access)
            .await?
            .is_none(),
        "the successful copy does not own its source's file"
    );
    tx.rollback().await?;
    Ok(())
}

async fn target_machine(s: &Scene) -> Proof<Id> {
    let id = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut *s.world.conn().await,
        s.company.admin.id(),
        "Selected snapshot recipient",
    )
    .await?;
    let pki = orch_testkit::doubles::link::TestPki::new(s.world.app.app.clock.clone());
    let identity = pki.machine(id);
    let key = orch_pki::sealing::RecipientKey::from_certificate(&identity.certificate).to_text();
    sqlx::query("UPDATE machines SET roles=ARRAY['environment']::text[],online=true,cert_public_key=$2,facts=$3 WHERE id=$1")
        .bind(id).bind(key).bind(json!({"policy":orch_link_contract::policy::LocalPolicy::default()})).execute(&s.world.db.pool).await?;
    Ok(id)
}
fn start_to(s: &Scene, target: Id, token: String, header: Option<String>) -> JoinHandle<RawReply> {
    let app = s.world.app.clone();
    let source = s.source;
    tokio::spawn(async move {
        let request =
            app.call(McpServerDuplicate, &[&source])
                .token(&token)
                .body(&McpDuplicateBody {
                    machine_id: Some(target),
                    ..Default::default()
                });
        let request = match &header {
            Some(key) => request.header("idempotency-key", key),
            None => request,
        };
        request.send_bytes().await
    })
}
fn sealed_snapshot() -> McpImportResult {
    McpImportResult {
        snapshot: Some(orch_link_contract::commands::Sealed {
            value: "synthetic-native-recipient-ciphertext".into(),
        }),
        definition_fingerprint: None,
        transport: "stdio".into(),
        digest: "cross-machine-snapshot".into(),
        tools: None,
    }
}
#[tokio::test]
async fn a_sealed_preparation_records_no_creation_until_the_actual_destination_command_is_queued()
-> Proof {
    let (s, source_machine) = scene().await?;
    let target = target_machine(&s).await?;
    let header = orch_types::new_id().to_string();
    let waiting = start_to(
        &s,
        target,
        s.company.admin.token.clone(),
        Some(header.clone()),
    );
    let (read, source) = queued(&s, &waiting).await?;
    assert_eq!(read.machine_id, source_machine);
    assert_eq!(read.body["transfer"]["kind"], "issue");
    assert!(source.mcp_import_ack(&read)?.is_none());
    let pending: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE key=$1 AND user_id=$2")
            .bind(&header)
            .bind(s.company.admin.id())
            .fetch_one(&s.world.db.pool)
            .await?;
    assert!(
        orch_db::idempotency::creation::Record::read(pending)?
            .outcome
            .is_none(),
        "reading a sealed snapshot cannot consume creation idempotency"
    );
    finish_result(&s, &read, &source, sealed_snapshot()).await?;
    let (receive, authority) = queued(&s, &waiting).await?;
    assert_eq!(receive.machine_id, target);
    assert_eq!(receive.body["transfer"]["kind"], "receive");
    assert_eq!(receive.body["server_id"], read.body["server_id"]);
    assert_eq!(
        receive.body["transfer"]["snapshot"],
        serde_json::to_value(sealed_snapshot().snapshot.unwrap())?
    );
    let accepted: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE key=$1 AND user_id=$2")
            .bind(&header)
            .bind(s.company.admin.id())
            .fetch_one(&s.world.db.pool)
            .await?;
    let created = orch_db::idempotency::creation::Record::read(accepted)?
        .outcome
        .ok_or("actual destination acceptance must retain its new identity")?;
    assert_eq!(created.data["native_command"]["id"], json!(receive.id));
    assert_ne!(receive.id, read.id);
    sqlx::query("DELETE FROM commands WHERE id=$1")
        .bind(read.id)
        .execute(&s.world.db.pool)
        .await?;
    finish(&s, &receive, &authority, "cross-machine-snapshot").await?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), waiting).await??;
    let id = id(&reply)?;
    assert_eq!(id, created.id);
    let row = rows::servers(&mut *s.world.conn().await, &[id])
        .await?
        .pop()
        .ok_or("actual destination copy missing")?;
    assert_eq!(row.machine_id, Some(target));
    assert!(row.imported);
    let original = rows::servers(&mut *s.world.conn().await, &[s.source])
        .await?
        .pop()
        .ok_or("source missing")?;
    assert_eq!(original.machine_id, Some(source_machine));
    assert_eq!(original.spec["imported"]["digest"], "source-snapshot");
    let copies: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='mcp.import' AND body ? 'stored'",
    )
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(
        copies, 1,
        "only the accepted receiver remains after ordinary source-command retention"
    );
    Ok(())
}
#[tokio::test]
async fn a_native_receive_cannot_outlive_the_original_raw_identity_scope_or_selected_recipient()
-> Proof {
    for change in ["revoked", "scope", "recipient"] {
        let (s, _) = scene().await?;
        let target = target_machine(&s).await?;
        let key = key(
            &s.world,
            s.company.admin.id(),
            vec![
                rule("McpServerDuplicate", "mcp_server", s.source, "self", "read"),
                rule("McpServerDuplicate", "org", s.company.acme, "self", "write"),
                rule("McpServerDuplicate", "machine", target, "self", "write"),
            ],
        )
        .await?;
        let waiting = start_to(&s, target, key.secret, None);
        let (read, source) = queued(&s, &waiting).await?;
        match change {
            "revoked" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(s.world.now())
                    .execute(&s.world.db.pool)
                    .await?;
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":[]}))
                    .execute(&s.world.db.pool)
                    .await?;
            }
            "recipient" => {
                let pki = orch_testkit::doubles::link::TestPki::new(s.world.app.app.clock.clone());
                let identity = pki.machine(target);
                let next = orch_pki::sealing::RecipientKey::from_certificate(&identity.certificate)
                    .to_text();
                sqlx::query("UPDATE machines SET cert_public_key=$2 WHERE id=$1")
                    .bind(target)
                    .bind(next)
                    .execute(&s.world.db.pool)
                    .await?;
            }
            _ => unreachable!(),
        }
        finish_result(&s, &read, &source, sealed_snapshot()).await?;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), waiting).await??;
        assert!(!reply.status.is_success(), "{change}");
        let received:i64=sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='mcp.import' AND body->'transfer'->>'kind'='receive'").fetch_one(&s.world.db.pool).await?;
        assert_eq!(
            received, 0,
            "{change}: original authority cannot be replaced after its native read"
        );
        let made: Id = serde_json::from_value(read.body["server_id"].clone())?;
        assert!(
            rows::servers(&mut *s.world.conn().await, &[made])
                .await?
                .is_empty()
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_native_copy_binds_the_selected_machine_under_its_own_write_namespace_before_any_read_or_creation()
-> Proof {
    let (s, source_machine) = scene().await?;
    let target = target_machine(&s).await?;
    for machine in [None, Some(source_machine)] {
        let mut rules = vec![
            rule("McpServerDuplicate", "mcp_server", s.source, "self", "read"),
            rule("McpServerDuplicate", "org", s.company.acme, "self", "write"),
        ];
        if let Some(machine) = machine {
            rules.push(rule(
                "McpServerDuplicate",
                "machine",
                machine,
                "self",
                "write",
            ));
        }
        let key = key(&s.world, s.company.admin.id(), rules).await?;
        let reply = s
            .world
            .app
            .call(McpServerDuplicate, &[&s.source])
            .token(&key.secret)
            .body(&McpDuplicateBody {
                machine_id: Some(target),
                ..Default::default()
            })
            .send_bytes()
            .await;
        assert_eq!(
            reply.status.as_u16(),
            403,
            "a missing or neighbouring writer cannot name another native target"
        );
    }
    let writes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='mcp.import' AND body ? 'stored'",
    )
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(writes, 0);
    Ok(())
}
#[tokio::test]
async fn a_readable_source_does_not_give_its_copier_use_of_an_unshared_recipient() -> Proof {
    let (s, source_machine) = scene().await?;
    let target = target_machine(&s).await?;
    let actor = &s.company.viewer;
    resources::name_person(&s.world, "mcp_server", s.source, "mcp.view", actor.id()).await;
    resources::name_person(
        &s.world,
        "machine",
        source_machine,
        "machine.use",
        actor.id(),
    )
    .await;
    let token = key(
        &s.world,
        actor.id(),
        vec![
            rule("McpServerDuplicate", "mcp_server", s.source, "self", "read"),
            rule("McpServerDuplicate", "org", s.company.acme, "self", "write"),
            rule("McpServerDuplicate", "machine", target, "self", "write"),
        ],
    )
    .await?;
    let reply = s
        .world
        .app
        .call(McpServerDuplicate, &[&s.source])
        .token(&token.secret)
        .body(&McpDuplicateBody {
            machine_id: Some(target),
            ..Default::default()
        })
        .send_bytes()
        .await;
    assert_eq!(
        reply.status.as_u16(),
        400,
        "the same picker must select a machine the work may use"
    );
    let writes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='mcp.import' AND body ? 'stored'",
    )
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(writes, 0);
    resources::name_person(&s.world, "machine", target, "machine.use", actor.id()).await;
    let waiting = start_to(&s, target, token.secret, None);
    let (read, source) = queued(&s, &waiting).await?;
    assert_eq!(read.body["stored"]["include_secrets"], false);
    finish_result(&s, &read, &source, sealed_snapshot()).await?;
    let (receive, authority) = queued(&s, &waiting).await?;
    assert_eq!(receive.machine_id, target);
    finish(&s, &receive, &authority, "cross-machine-snapshot").await?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), waiting).await??;
    let id = id(&reply)?;
    let actual = rows::servers(&mut *s.world.conn().await, &[id])
        .await?
        .pop()
        .ok_or("the permitted receiver did not get its native copy")?;
    assert_eq!(actual.owner_id, Some(actor.id()));
    Ok(())
}

#[tokio::test]
async fn a_machine_granted_to_source_work_is_not_a_destination_machine_for_a_sibling_org() -> Proof
{
    use orch_api_contract::resource_groups::{ObjectHomeSet, ResourceHomeSetBody};
    let (s, source_machine) = scene().await?;
    let source_home = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Original tools",
        "original-tools",
    )
    .await
    .base
    .id;
    let target_home = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Other tools",
        "other-tools",
    )
    .await
    .base
    .id;
    s.world
        .app
        .call(ObjectHomeSet, &[&"mcp-servers", &s.source])
        .token(&s.company.admin.token)
        .body(&ResourceHomeSetBody {
            home_id: source_home,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let target = target_machine(&s).await?;
    let actor = &s.company.viewer;
    resources::name_person(&s.world, "mcp_server", s.source, "mcp.view", actor.id()).await;
    resources::name_on_page(
        &s.world,
        "machine",
        source_machine,
        "machine.use",
        ("node", Some(source_home), None),
    )
    .await;
    resources::name_on_page(
        &s.world,
        "machine",
        target,
        "machine.use",
        ("node", Some(target_home), None),
    )
    .await;
    let key = key(
        &s.world,
        actor.id(),
        vec![
            rule("McpServerDuplicate", "mcp_server", s.source, "self", "read"),
            rule("McpServerDuplicate", "org", target_home, "self", "write"),
            rule("McpServerDuplicate", "machine", target, "self", "write"),
            rule(
                "McpServerDuplicate",
                "machine",
                source_machine,
                "self",
                "write",
            ),
        ],
    )
    .await?;
    let choice = McpDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target_home }),
        ..Default::default()
    };
    let denied = s
        .world
        .app
        .call(McpServerDuplicate, &[&s.source])
        .token(&key.secret)
        .body(&choice)
        .send_bytes()
        .await;
    assert_eq!(
        denied.status.as_u16(),
        400,
        "the picker must ask for a usable machine instead of carrying the source-only work grant across orgs"
    );
    let app = s.world.app.clone();
    let source = s.source;
    let token = key.secret;
    let waiting = tokio::spawn(async move {
        let request =
            app.call(McpServerDuplicate, &[&source])
                .token(&token)
                .body(&McpDuplicateBody {
                    machine_id: Some(target),
                    ..choice
                });
        request.send_bytes().await
    });
    let (read, source) = queued(&s, &waiting).await?;
    assert_eq!(read.machine_id, source_machine);
    assert_eq!(read.body["stored"]["include_secrets"], false);
    finish_result(&s, &read, &source, sealed_snapshot()).await?;
    let (received, authority) = queued(&s, &waiting).await?;
    assert_eq!(received.machine_id, target);
    finish(&s, &received, &authority, "cross-machine-snapshot").await?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), waiting).await??;
    let copied = id(&reply)?;
    let row = rows::servers(&mut *s.world.conn().await, &[copied])
        .await?
        .pop()
        .ok_or("actual work-admitted copy absent")?;
    assert_eq!(row.home_id, Some(target_home));
    assert_eq!(row.machine_id, Some(target));
    assert_eq!(row.owner_id, Some(actor.id()));
    Ok(())
}

#[tokio::test]
async fn an_accepted_native_move_files_the_copy_and_original_delete_with_owned_cleanup_after_key_revocation()
-> Proof {
    use orch_api_contract::duplicates::{
        McpMoveOutBody, McpServerMoveOut, McpServerMoveOutPreview, MoveOutConfirmation,
    };
    let (s, machine) = scene().await?;
    let target = places::make_org(
        &s.world,
        &s.company.admin,
        "Native move recipient",
        "native-move-recipient",
    )
    .await
    .base
    .id;
    let copy = McpDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(McpServerMoveOutPreview, &[&s.source])
        .token(&s.company.admin.token)
        .body(&copy)
        .send()
        .await
        .ok();
    let actor=key(&s.world,s.company.admin.id(),vec![rule("McpServerMoveOut","mcp_server",s.source,"self","manage"),rule("McpServerMoveOut","org",target,"self","write"),json!({"target":{"kind":"machine","id":machine,"reach":"self"},"resources":["machine"],"allow":[{"operation":"McpServerMoveOut","actions":["manage","write"]}]})]).await?;
    let app = s.world.app.clone();
    let source = s.source;
    let secret = actor.secret.clone();
    let waiting = tokio::spawn(async move {
        let request = app
            .call(McpServerMoveOut, &[&source])
            .token(&secret)
            .body(&McpMoveOutBody {
                copy,
                delete_plan: MoveOutConfirmation {
                    revision: plan.revision,
                    stamp: plan.stamp,
                },
            });
        request.send_bytes().await
    });
    let (command, authority) = queued(&s, &waiting).await?;
    let made: Id = serde_json::from_value(command.body["server_id"].clone())?;
    assert_eq!(command.body["stored"]["server_id"], json!(s.source));
    finish(&s, &command, &authority, "accepted-move-snapshot").await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), waiting).await??;
    assert_eq!(id(&reply)?, made);
    assert!(
        rows::servers(&mut *s.world.conn().await, &[s.source])
            .await?
            .is_empty()
    );
    let row = rows::servers(&mut *s.world.conn().await, &[made])
        .await?
        .pop()
        .ok_or("actual moved native snapshot missing")?;
    assert_eq!(
        (row.machine_id, row.home_id, row.owner_id),
        (Some(machine), Some(target), Some(s.company.admin.id()))
    );
    let mut tx = s.world.db.pool.begin().await?;
    let cleanup: Id = sqlx::query_scalar(
        "SELECT id FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
    )
    .bind(s.source.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let saved = link::command_with_authority(&mut tx, cleanup)
        .await?
        .ok_or("the original native cleanup is not journaled")?;
    let proof: CommandAuthority = serde_json::from_value(
        saved
            .authority
            .ok_or("native deletion custody is missing")?,
    )?;
    let access = AuthzAccess::new(s.world.app.app.clock.clone());
    assert!(
        proof
            .current_callers(
                &mut tx,
                &saved.command,
                s.world.app.app.clock.as_ref(),
                &access
            )
            .await?
            .is_some(),
        "recorded deletion owns only its original source copy after the key ended"
    );
    tx.rollback().await?;
    Ok(())
}
#[tokio::test]
async fn a_native_move_whose_original_definition_changed_owns_only_its_new_unfiled_inverse() -> Proof
{
    use orch_api_contract::duplicates::{
        McpMoveOutBody, McpServerMoveOut, McpServerMoveOutPreview, MoveOutConfirmation,
    };
    let (s, _) = scene().await?;
    let target = places::make_org(
        &s.world,
        &s.company.admin,
        "Native move recipient",
        "native-move-recipient",
    )
    .await
    .base
    .id;
    let copy = McpDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(McpServerMoveOutPreview, &[&s.source])
        .token(&s.company.admin.token)
        .body(&copy)
        .send()
        .await
        .ok();
    let app = s.world.app.clone();
    let source = s.source;
    let token = s.company.admin.token.clone();
    let waiting = tokio::spawn(async move {
        let request = app
            .call(McpServerMoveOut, &[&source])
            .token(&token)
            .body(&McpMoveOutBody {
                copy,
                delete_plan: MoveOutConfirmation {
                    revision: plan.revision,
                    stamp: plan.stamp,
                },
            });
        request.send_bytes().await
    });
    let (command, authority) = queued(&s, &waiting).await?;
    let made: Id = serde_json::from_value(command.body["server_id"].clone())?;
    waiting.abort();
    let _ = waiting.await;
    sqlx::query("UPDATE mcp_servers SET revision=revision+1,description='Changed original definition' WHERE id=$1").bind(s.source).execute(&s.world.db.pool).await?;
    finish(&s, &command, &authority, "already-read-native-copy").await?;
    assert!(
        !rows::servers(&mut *s.world.conn().await, &[s.source])
            .await?
            .is_empty()
    );
    assert!(
        rows::servers(&mut *s.world.conn().await, &[made])
            .await?
            .is_empty()
    );
    let old_cleanup: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
    )
    .bind(s.source.to_string())
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(
        old_cleanup, 0,
        "a changed original definition is never deleted by the old plan"
    );
    let inverse: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
    )
    .bind(made.to_string())
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(inverse, 1);
    Ok(())
}

#[tokio::test]
async fn a_native_move_rechecks_the_same_raw_key_before_ending_the_original_after_copy_io() -> Proof
{
    use orch_api_contract::duplicates::{
        McpMoveOutBody, McpServerMoveOut, McpServerMoveOutPreview, MoveOutConfirmation,
    };
    let (s, machine) = scene().await?;
    let target = places::make_org(
        &s.world,
        &s.company.admin,
        "Checked move recipient",
        "checked-move-recipient",
    )
    .await
    .base
    .id;
    let copy = McpDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    let plan = s
        .world
        .app
        .call(McpServerMoveOutPreview, &[&s.source])
        .token(&s.company.admin.token)
        .body(&copy)
        .send()
        .await
        .ok();
    let actor=key(&s.world,s.company.admin.id(),vec![rule("McpServerMoveOut","mcp_server",s.source,"self","manage"),rule("McpServerMoveOut","org",target,"self","write"),json!({"target":{"kind":"machine","id":machine,"reach":"self"},"resources":["machine"],"allow":[{"operation":"McpServerMoveOut","actions":["manage","write"]}]})]).await?;
    let app = s.world.app.clone();
    let source = s.source;
    let token = actor.secret.clone();
    let waiting = tokio::spawn(async move {
        let request = app
            .call(McpServerMoveOut, &[&source])
            .token(&token)
            .body(&McpMoveOutBody {
                copy,
                delete_plan: MoveOutConfirmation {
                    revision: plan.revision,
                    stamp: plan.stamp,
                },
            });
        request.send_bytes().await
    });
    let (command, authority) = queued(&s, &waiting).await?;
    let made: Id = serde_json::from_value(command.body["server_id"].clone())?;
    waiting.abort();
    let _ = waiting.await;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await?;
    finish(&s, &command, &authority, "native-copy-before-key-close").await?;
    assert!(
        !rows::servers(&mut *s.world.conn().await, &[s.source])
            .await?
            .is_empty()
    );
    assert!(
        rows::servers(&mut *s.world.conn().await, &[made])
            .await?
            .is_empty()
    );
    let inverses: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='mcp.forget' AND body->>'server_id'=$1",
    )
    .bind(made.to_string())
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(
        inverses, 1,
        "the copied physical file is owned cleanup, never authority to end a source after its key closes"
    );
    Ok(())
}
