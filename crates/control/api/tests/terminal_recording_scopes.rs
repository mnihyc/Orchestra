//! Recording reads bind stored sources before the once-only supplier and recheck them afterwards.

mod places;
mod rig;
mod support;
mod terminals_support;
mod thread_history_scope;

use orch_api_contract::terminals::TerminalRecordingGet;
use orch_api_core::testing::RawReply;
use orch_types::{Hash, Id, new_id};
use serde_json::json;
use terminals_support::Floor;
use thread_history_scope::{Proof, key, narrow, rule};

async fn recorded(floor: &Floor) -> Proof<(Id, Hash, Vec<u8>)> {
    let id = floor.shell_of(&floor.scene.dan).await.session.id;
    let bytes = format!("A native recorded output {}\n", new_id()).into_bytes();
    *floor
        .fake
        .kept
        .lock()
        .map_err(|_| "recording observer is poisoned")? = Some(floor.store.uploaded(&bytes));
    assert!(floor.close(&floor.scene.dan, id).await.ok().recorded);
    let hash = floor
        .row(id)
        .await
        .recording()
        .ok_or("closed native recording is absent")?;
    Ok((id, hash, bytes))
}

async fn read(floor: &Floor, id: Id, secret: &str) -> RawReply {
    floor
        .scene
        .rig
        .world
        .app
        .call(TerminalRecordingGet, &[&id])
        .token(secret)
        .send_bytes()
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_requires_its_own_read_on_the_actual_session_environment_and_native_door() -> Proof
{
    let floor = Floor::new().await;
    let (id, hash, bytes) = recorded(&floor).await?;
    for who in [&floor.scene.dan, &floor.scene.eve] {
        let selected = key(
            &floor.scene.rig.world,
            who.id(),
            vec![rule(
                "TerminalRecordingGet",
                "environment",
                floor.environment,
                "self",
                "read",
            )],
        )
        .await?;
        let before = floor.store.reads_of(hash);
        let actual = read(&floor, id, &selected.secret).await;
        assert_eq!(actual.status.as_u16(), 200);
        assert_eq!(actual.raw.as_ref(), bytes.as_slice());
        assert_eq!(actual.headers["content-type"], "application/octet-stream");
        assert_eq!(floor.store.reads_of(hash), before + 1);
    }
    let after = floor.store.reads_of(hash);
    for (user, grants) in [
        (
            floor.scene.dan.id(),
            vec![rule(
                "TerminalRecordingGet",
                "environment",
                new_id(),
                "self",
                "read",
            )],
        ),
        (
            floor.scene.dan.id(),
            vec![rule(
                "TerminalRecordingGet",
                "environment",
                floor.environment,
                "self",
                "manage",
            )],
        ),
        (
            floor.scene.dan.id(),
            vec![rule(
                "EnvironmentTerminalList",
                "environment",
                floor.environment,
                "self",
                "read",
            )],
        ),
        (
            floor.scene.cai.id(),
            vec![rule(
                "TerminalRecordingGet",
                "environment",
                floor.environment,
                "self",
                "read",
            )],
        ),
    ] {
        let selected = key(&floor.scene.rig.world, user, grants).await?;
        let actual = read(&floor, id, &selected.secret).await;
        assert!(!actual.status.is_success());
        assert!(
            !actual
                .raw
                .windows(bytes.len())
                .any(|part| part == bytes.as_slice())
        );
    }
    assert_eq!(
        floor.store.reads_of(hash),
        after,
        "a refused source never starts storage IO"
    );
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='terminal.recording_read' AND object_id=$1",
    )
    .bind(id)
    .fetch_one(floor.pool())
    .await?;
    assert_eq!(audited, 1);
    let ordinary = read(&floor, id, &floor.scene.dan.token).await;
    assert_eq!(ordinary.raw.as_ref(), bytes.as_slice());
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_thread_audit_binds_only_its_actual_alternative_and_cannot_borrow_another_operations_read()
-> Proof {
    let floor = Floor::new().await;
    let (id, _, bytes) = recorded(&floor).await?;
    floor
        .names_on_the_thread(&floor.scene.cai, "audit.view")
        .await;
    let root = rule(
        "TerminalRecordingGet",
        "environment",
        floor.environment,
        "self",
        "read",
    );
    for thread_grant in [
        None,
        Some(rule(
            "ThreadSyncGet.read",
            "thread",
            floor.thread,
            "self",
            "read",
        )),
        Some(rule(
            "TerminalRecordingGet",
            "thread",
            new_id(),
            "self",
            "read",
        )),
        Some(rule(
            "TerminalRecordingGet",
            "thread",
            floor.thread,
            "self",
            "read",
        )),
    ] {
        let allowed = thread_grant.as_ref().is_some_and(|value| {
            value["allow"][0]["operation"] == "TerminalRecordingGet"
                && value["target"]["id"] == json!(floor.thread)
        });
        let mut grants = vec![root.clone()];
        grants.extend(thread_grant);
        let selected = key(&floor.scene.rig.world, floor.scene.cai.id(), grants).await?;
        let actual = read(&floor, id, &selected.secret).await;
        if allowed {
            assert_eq!(actual.status.as_u16(), 200);
            assert_eq!(actual.raw.as_ref(), bytes.as_slice());
        } else {
            assert!(!actual.status.is_success());
        }
    }
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_opener_keeps_the_native_recording_after_its_environment_is_retained() -> Proof {
    let floor = Floor::new().await;
    let (id, _, bytes) = recorded(&floor).await?;
    sqlx::query("UPDATE environments SET deleted_at=$2 WHERE id=$1")
        .bind(floor.environment)
        .bind(floor.scene.rig.world.now())
        .execute(floor.pool())
        .await?;
    let selected = key(
        &floor.scene.rig.world,
        floor.scene.dan.id(),
        vec![rule(
            "TerminalRecordingGet",
            "environment",
            floor.environment,
            "self",
            "read",
        )],
    )
    .await?;
    for token in [&selected.secret, &floor.scene.dan.token] {
        let actual = read(&floor, id, token).await;
        assert_eq!(actual.status.as_u16(), 200);
        assert_eq!(actual.raw.as_ref(), bytes.as_slice());
    }
    floor.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_supplier_is_once_only_and_rechecks_original_key_raw_session_and_source_after_await()
-> Proof {
    for mode in [
        "unchanged",
        "narrow",
        "revoke",
        "expire",
        "recording",
        "association",
        "session",
    ] {
        let floor = Floor::new().await;
        let (id, hash, bytes) = recorded(&floor).await?;
        let selected = key(
            &floor.scene.rig.world,
            floor.scene.dan.id(),
            vec![rule(
                "TerminalRecordingGet",
                "environment",
                floor.environment,
                "self",
                "read",
            )],
        )
        .await?;
        let secret = if mode == "session" {
            floor.scene.dan.token.clone()
        } else {
            selected.secret.clone()
        };
        // Fail an unbound baseline before installing a supplier barrier it cannot reach.
        assert_eq!(read(&floor, id, &secret).await.status.as_u16(), 200);
        let before = floor.store.reads_of(hash);
        let (entered, release) = floor.store.pause_read(hash);
        let app = floor.scene.rig.world.app.clone();
        let pending = tokio::spawn(async move {
            let request = app.call(TerminalRecordingGet, &[&id]).token(&secret);
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), entered).await??;
        match mode {
            "narrow" => narrow(&floor.scene.rig.world, &selected, Vec::new()).await?,
            "revoke" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(selected.id)
                    .bind(floor.scene.rig.world.now())
                    .execute(floor.pool())
                    .await?;
            }
            "expire" => floor.scene.rig.world.advance(chrono::Duration::hours(2)),
            "recording" => {
                sqlx::query("UPDATE terminal_sessions SET recording_blob=NULL WHERE id=$1")
                    .bind(id)
                    .execute(floor.pool())
                    .await?;
            }
            "association" => {
                sqlx::query("UPDATE terminal_sessions SET thread_id=NULL WHERE id=$1")
                    .bind(id)
                    .execute(floor.pool())
                    .await?;
            }
            "session" => {
                sqlx::query("DELETE FROM sessions WHERE user_id=$1")
                    .bind(floor.scene.dan.id())
                    .execute(floor.pool())
                    .await?;
            }
            _ => {}
        }
        release
            .send(())
            .map_err(|_| "native supplier ended before release")?;
        let actual = tokio::time::timeout(std::time::Duration::from_secs(30), pending).await??;
        assert_eq!(
            floor.store.reads_of(hash),
            before + 1,
            "publication never reruns the storage supplier"
        );
        if mode == "unchanged" {
            assert_eq!(actual.status.as_u16(), 200);
            assert_eq!(actual.raw.as_ref(), bytes.as_slice());
        } else {
            assert!(
                !actual.status.is_success(),
                "{mode} cannot publish retained recording bytes"
            );
            assert!(
                !actual
                    .raw
                    .windows(bytes.len())
                    .any(|part| part == bytes.as_slice())
            );
        }
        floor.stop().await;
    }
    Ok(())
}
