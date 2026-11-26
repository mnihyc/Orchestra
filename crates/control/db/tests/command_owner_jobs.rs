//! Before a command goes to its machine, the jobs that own it are looked up by the command's id: the jobs that
//! name it as their active command or keep it in one of their command journals. The lookup reads those jobs and
//! never every job's plan, so a command is not slower to send on an installation that has kept many jobs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::queries::jobs;
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};
use serde_json::{Value, json};
use std::collections::BTreeSet;

async fn job(conn: &mut orch_db::Conn, kind: &str, plan: Value) -> Id {
    let id = new_id();
    sqlx::query("INSERT INTO jobs (id, kind, state, plan) VALUES ($1, $2, 'running', $3)")
        .bind(id)
        .bind(kind)
        .bind(plan)
        .execute(&mut *conn)
        .await
        .unwrap();
    id
}

fn set(ids: Vec<Id>) -> BTreeSet<Id> {
    ids.into_iter().collect()
}

#[tokio::test]
async fn checking_a_command_reads_only_the_jobs_that_name_it() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    // An installation that has kept a few hundred jobs, each plan holding its own facts and journals.
    for n in 0..300 {
        let filler = "x".repeat(8_000 + n);
        job(
            &mut conn,
            "environment_provision",
            json!({
                "creator_provision": true,
                "facts": filler,
                "active_command": {"id": new_id().to_string()},
                "creator_provision_commands": {new_id().to_string(): {}},
                "provider_update_commands": [new_id().to_string()],
            }),
        )
        .await;
    }
    let command = new_id();
    let owner = job(
        &mut conn,
        "provider_provision",
        json!({"provider_update_origins": {}, "active_command": {"id": command.to_string()}}),
    )
    .await;
    sqlx::query("ANALYZE jobs")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let mut tx = test.pool.begin().await.unwrap();
    assert_eq!(
        jobs::lock_provider_update_command_owners(&mut tx, command)
            .await
            .unwrap(),
        vec![owner]
    );
    assert!(
        jobs::creator_provision_command_owners(&mut tx, command)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        jobs::lock_creator_provision_command_owners(&mut tx, command)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        jobs::lock_provider_creator_command_owners(&mut tx, command)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        jobs::lock_initial_files_command_owners(&mut tx, command)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        jobs::lock_source_command_owners(&mut tx, command)
            .await
            .unwrap(),
        vec![owner]
    );
    assert!(
        jobs::lock_capability_command_owners(&mut tx, command)
            .await
            .unwrap()
            .is_empty()
    );
    let scanned: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(seq_scan), 0)::bigint FROM pg_stat_xact_user_tables WHERE relname = 'jobs'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        scanned, 0,
        "the owners of a command are found without reading every job"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn every_owner_lookup_finds_the_jobs_that_keep_its_command_in_any_journal() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let command = new_id();
    let named = command.to_string();
    let active = json!({"id": named});
    let other = json!({"id": new_id().to_string()});

    let provision_active = job(
        &mut conn,
        "environment_provision",
        json!({"creator_provision": true, "active_command": active}),
    )
    .await;
    let provision_journal = job(
        &mut conn,
        "environment_provision",
        json!({"creator_provision_version": 2, "active_command": other, "creator_provision_commands": {named.clone(): {"phase": "copy"}}}),
    )
    .await;
    let files_active = job(
        &mut conn,
        "workspace_prepare",
        json!({"initial_files": [], "active_command": active}),
    )
    .await;
    let files_journal = job(
        &mut conn,
        "workspace_prepare",
        json!({"initial_files": [], "initial_files_commands": [named.clone()]}),
    )
    .await;
    let creator_active = job(
        &mut conn,
        "provider_provision",
        json!({"provider_creator": {}, "active_command": active}),
    )
    .await;
    let creator_journal = job(
        &mut conn,
        "provider_provision",
        json!({"provider_creator_commands": {named.clone(): true}}),
    )
    .await;
    let update_active = job(
        &mut conn,
        "provider_recreate",
        json!({"active_command": active}),
    )
    .await;
    let update_journals: Vec<Id> = {
        let mut made = Vec::new();
        for journal in [
            "provider_update_commands",
            "provider_update_notice_commands",
            "provider_recreate_commands",
            "provider_recreate_notice_commands",
            "environment_update_commands",
            "environment_update_notice_commands",
            "environment_update_composed_commands",
            "environment_update_batch_commands",
            "environment_update_batch_notice_commands",
        ] {
            made.push(
                job(
                    &mut conn,
                    "environment_recreate",
                    json!({journal: [new_id().to_string(), named.clone()]}),
                )
                .await,
            );
        }
        made
    };
    let creator_named = job(
        &mut conn,
        "workspace_prepare",
        json!({"creator_command": active}),
    )
    .await;
    let publication_named = job(
        &mut conn,
        "workspace_prepare",
        json!({"publication_command": active}),
    )
    .await;
    let capability_active = job(
        &mut conn,
        "capability_change",
        json!({"approval_origin": {}, "active_command": active}),
    )
    .await;
    let capability_journal = job(
        &mut conn,
        "capability_change",
        json!({"capability_commands": {named.clone(): {}}}),
    )
    .await;
    // Jobs that name only other commands, or this command where no lookup looks for it.
    for plan in [
        json!({"creator_provision": true, "active_command": other}),
        json!({"provider_update_commands": [new_id().to_string()]}),
        json!({"unrelated_commands_note": named.clone()}),
        json!({"other_commands": [named.clone()]}),
        json!({"facts": {"active_command": active}}),
    ] {
        job(&mut conn, "environment_provision", plan).await;
    }
    drop(conn);

    let mut tx = test.pool.begin().await.unwrap();
    let provision = set(vec![provision_active, provision_journal]);
    assert_eq!(
        set(jobs::creator_provision_command_owners(&mut tx, command)
            .await
            .unwrap()),
        provision
    );
    assert_eq!(
        set(
            jobs::lock_creator_provision_command_owners(&mut tx, command)
                .await
                .unwrap()
        ),
        provision
    );
    assert_eq!(
        set(jobs::lock_initial_files_command_owners(&mut tx, command)
            .await
            .unwrap()),
        set(vec![files_active, files_journal])
    );
    assert_eq!(
        set(jobs::lock_provider_creator_command_owners(&mut tx, command)
            .await
            .unwrap()),
        set(vec![creator_active, creator_journal])
    );
    let mut updates = set(update_journals);
    updates.insert(update_active);
    assert_eq!(
        set(jobs::lock_provider_update_command_owners(&mut tx, command)
            .await
            .unwrap()),
        updates
    );
    // Every job whose active command it is, and the jobs whose creator or publication command it is.
    let mut sources = set(vec![
        provision_active,
        files_active,
        creator_active,
        update_active,
        capability_active,
    ]);
    sources.extend([creator_named, publication_named]);
    assert_eq!(
        set(jobs::lock_source_command_owners(&mut tx, command)
            .await
            .unwrap()),
        sources
    );
    assert_eq!(
        set(jobs::lock_capability_command_owners(&mut tx, command)
            .await
            .unwrap()),
        set(vec![capability_active, capability_journal])
    );
    tx.rollback().await.unwrap();
}
