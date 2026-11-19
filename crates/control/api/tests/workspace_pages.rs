//! Directory continuations preserve sorted discoverability and current environment authority.

mod places;
mod support;
mod workspace;

use async_trait::async_trait;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::workspace::{
    DirListing, EnvironmentFileList, EnvironmentFileListQuery, FileListSource,
};
use orch_api_core::testing::{Machine, Reply};
use orch_db::queries::{access, storage};
use orch_link_contract::commands::{
    Cmd, CommandError, ErrorCode as MachineCode, FsList, FsListPageResult, FsListResult,
};
use orch_types::{Clock, Hash, Time, new_id};
use std::sync::Arc;
use tokio::sync::Notify;
use workspace::{Disk, Site};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn page(
    site: &Site,
    token: &str,
    path: &str,
    source: Option<FileListSource>,
    cursor: Option<String>,
    limit: u32,
) -> Reply<DirListing> {
    site.app()
        .call(EnvironmentFileList, &[&site.environment])
        .token(token)
        .query(&EnvironmentFileListQuery {
            thread: None,
            path: path.to_owned(),
            source,
            mount: None,
            cursor,
            limit: Some(limit),
        })
        .send()
        .await
}

#[tokio::test]
async fn a_cached_manifest_pages_every_sibling_in_order_and_keeps_its_original_as_of() -> Proof {
    let site = Site::new().await;
    for n in (0..5_007).rev() {
        site.disk.put(&format!("wide/file-{n:05}"), b"x");
    }
    site.publish_manifest().await;
    site.power_off().await;
    let mut cursor = None;
    let mut names = Vec::new();
    let mut revision = None;
    let mut as_of = None;
    loop {
        let listed = page(
            &site,
            &site.ada.token,
            "wide",
            Some(FileListSource::Cached),
            cursor,
            731,
        )
        .await
        .ok();
        assert!(!listed.entries.is_empty());
        assert!(listed.entries.len() <= 731);
        assert!(serde_json::to_vec(&listed)?.len() <= 2 * 1024 * 1024);
        if let Some(revision) = &revision {
            assert_eq!(&listed.revision, revision);
        } else {
            revision = Some(listed.revision.clone());
            as_of = Some(listed.as_of);
        }
        assert_eq!(Some(listed.as_of), as_of);
        assert_eq!(listed.truncated, listed.next_cursor.is_some());
        names.extend(listed.entries.into_iter().map(|entry| entry.name));
        cursor = listed.next_cursor;
        if cursor.is_none() {
            break;
        }
        assert!(names.len() < 5_007);
        site.world.advance(chrono::Duration::seconds(10));
    }
    assert_eq!(
        names,
        (0..5_007)
            .map(|n| format!("file-{n:05}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        site.sent(orch_link_contract::commands::CommandKind::FsList),
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_cached_cursor_cannot_change_path_source_generation_or_outlive_its_idle_expiry() -> Proof
{
    let site = Site::new().await;
    site.disk.put("wide/a", b"a");
    site.disk.put("wide/b", b"b");
    site.publish_manifest().await;
    site.power_off().await;
    let first = page(&site, &site.ada.token, "wide", None, None, 1)
        .await
        .ok();
    let cursor = first.next_cursor.ok_or("continuation")?;
    for (path, source) in [("other", None), ("wide", Some(FileListSource::Live))] {
        let failure = page(
            &site,
            &site.ada.token,
            path,
            source,
            Some(cursor.clone()),
            1,
        )
        .await
        .problem();
        assert_eq!(failure.code, ErrorCode::ConflictStaleRevision);
        assert_eq!(failure.extra.get("restart"), Some(&serde_json::json!(true)));
    }
    site.world.advance(chrono::Duration::minutes(5));
    assert_eq!(
        page(&site, &site.ada.token, "wide", None, Some(cursor), 1)
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision
    );
    let fresh = page(&site, &site.ada.token, "wide", None, None, 1)
        .await
        .ok();
    sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await?;
    assert_eq!(
        page(&site, &site.ada.token, "wide", None, fresh.next_cursor, 1)
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision
    );
    assert_eq!(
        page(&site, &site.ada.token, "wide", None, None, 0)
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    Ok(())
}

#[tokio::test]
async fn offline_opened_versions_are_bounded_deduplicated_and_change_invalidates_the_cursor()
-> Proof {
    let site = Site::new().await;
    sqlx::query("UPDATE environments SET facts=jsonb_set(facts,'{index}','\"live\"') WHERE id=$1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await?;
    for (path, size) in [
        ("wide/a", 1),
        ("wide/a", 2),
        ("wide/b", 3),
        ("wide/sub/old", 4),
    ] {
        let bytes = vec![b'x'; usize::try_from(size)?];
        // A cached version references content actually uploaded and registered, not an invented hash.
        // Finish the store write before acquiring its metadata connection.
        let blob = site.app().app.blobs.put_bytes(bytes.into()).await?;
        site.world.advance(chrono::Duration::seconds(1));
        let mut conn = site.world.conn().await;
        site.app()
            .app
            .blobs
            .register(&mut conn, blob, site.world.now())
            .await?;
        storage::record_cached(
            &mut conn,
            site.environment,
            &storage::CachedFile {
                path: path.to_owned(),
                size,
                mtime_ns: size,
                hash: blob.hash,
                cached_at: site.world.now(),
            },
        )
        .await?;
    }
    site.power_off().await;
    let first = page(&site, &site.ada.token, "wide", None, None, 1)
        .await
        .ok();
    assert_eq!(first.entries[0].name, "sub");
    assert!(!first.entries[0].cached);
    let second = page(
        &site,
        &site.ada.token,
        "wide",
        None,
        first.next_cursor.clone(),
        1,
    )
    .await
    .ok();
    assert_eq!(second.entries[0].name, "a");
    assert_eq!(
        second.entries[0].size, 2,
        "the newest cached version is shown once"
    );
    assert!(second.entries[0].cached);
    let last = page(&site, &site.ada.token, "wide", None, second.next_cursor, 1)
        .await
        .ok();
    assert_eq!(last.entries[0].name, "b");
    assert!(last.next_cursor.is_none());
    let blob = site
        .app()
        .app
        .blobs
        .put_bytes(bytes::Bytes::from_static(b"new!!"))
        .await?;
    let mut conn = site.world.conn().await;
    site.app()
        .app
        .blobs
        .register(&mut conn, blob, site.world.now())
        .await?;
    storage::record_cached(
        &mut conn,
        site.environment,
        &storage::CachedFile {
            path: "wide/new".to_owned(),
            size: 5,
            mtime_ns: 5,
            hash: blob.hash,
            cached_at: site.world.now(),
        },
    )
    .await?;
    drop(conn);
    assert_eq!(
        page(&site, &site.ada.token, "wide", None, first.next_cursor, 1)
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision
    );
    Ok(())
}

struct Gate {
    entered: Notify,
    resume: Notify,
}
struct PagedMachine {
    disk: Arc<Disk>,
    clock: Arc<dyn Clock>,
    gate: Option<Arc<Gate>>,
}

#[async_trait]
impl Machine for PagedMachine {
    async fn run(&self, command: &Cmd) -> Result<serde_json::Value, CommandError> {
        let answer = self.disk.run(command).await?;
        if command.kind != "fs.list" {
            return Ok(answer);
        }
        if let Some(gate) = &self.gate {
            gate.entered.notify_one();
            gate.resume.notified().await;
        }
        let bad =
            |error: serde_json::Error| CommandError::new(MachineCode::Internal, error.to_string());
        let body: FsList = serde_json::from_value(command.body.clone()).map_err(bad)?;
        let mut result: FsListResult = serde_json::from_value(answer).map_err(bad)?;
        let revision = Hash::of(&serde_json::to_vec(&result.entries).map_err(bad)?);
        let start = body
            .page
            .as_ref()
            .and_then(|page| page.cursor.as_deref())
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| CommandError::new(MachineCode::Internal, "bad synthetic cursor"))?;
        let limit = body
            .page
            .as_ref()
            .and_then(|page| page.limit)
            .unwrap_or(5000) as usize;
        let end = start.saturating_add(limit).min(result.entries.len());
        if start > end {
            return Err(CommandError::new(
                MachineCode::Internal,
                "synthetic cursor beyond end",
            ));
        }
        let next_cursor = (end < result.entries.len()).then(|| end.to_string());
        result.entries = result.entries[start..end].to_vec();
        result.truncated = next_cursor.is_some();
        result.watch_instance = body.watch_instance;
        result.page = Some(FsListPageResult {
            revision,
            next_cursor,
            as_of: self.clock.time(),
            observed_from: None,
            expires_at: Time::new(self.clock.now() + chrono::Duration::minutes(5)),
        });
        serde_json::to_value(result).map_err(bad)
    }
}

#[tokio::test]
async fn live_continuation_is_wrapped_and_the_last_page_is_the_actual_end() -> Proof {
    let mut site = Site::new().await;
    site.disk.put("wide/a", b"a");
    site.disk.put("wide/b", b"b");
    site.power_on_with(Arc::new(PagedMachine {
        disk: Arc::clone(&site.disk),
        clock: Arc::clone(&site.app().app.clock),
        gate: None,
    }));
    let first = page(&site, &site.ada.token, "wide", None, None, 1)
        .await
        .ok();
    assert_eq!(first.entries[0].name, "a");
    assert!(first.truncated);
    assert_ne!(
        first.next_cursor.as_deref(),
        Some("1"),
        "the API must bind the native selector"
    );
    let last = page(&site, &site.ada.token, "wide", None, first.next_cursor, 1)
        .await
        .ok();
    assert_eq!(last.entries[0].name, "b");
    assert_eq!(last.revision, first.revision);
    assert!(!last.truncated);
    assert!(last.next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn a_legacy_connector_partial_window_never_fabricates_a_cursor_or_an_end() -> Proof {
    let mut site = Site::new().await;
    site.disk.put("wide/a", b"a");
    site.disk.put("wide/b", b"b");
    site.power_on();
    let listed = page(&site, &site.ada.token, "wide", None, None, 1)
        .await
        .ok();
    assert_eq!(listed.entries.len(), 1);
    assert!(listed.truncated);
    assert!(listed.next_cursor.is_none());
    assert!(listed.revision.is_none());
    assert!(listed.expires_at.is_none());
    Ok(())
}

#[tokio::test]
async fn authority_revoked_while_the_native_page_waits_never_returns_its_entries() -> Proof {
    let mut site = Site::new().await;
    site.disk.put("wide/private", b"private");
    let gate = Arc::new(Gate {
        entered: Notify::new(),
        resume: Notify::new(),
    });
    site.power_on_with(Arc::new(PagedMachine {
        disk: Arc::clone(&site.disk),
        clock: Arc::clone(&site.app().app.clock),
        gate: Some(Arc::clone(&gate)),
    }));
    let id = new_id();
    let mut conn = site.world.conn().await;
    access::insert_entry(
        &mut conn,
        &access::NewEntry {
            id,
            page: &access::PageKey::new("environment", site.environment, "live"),
            profile: "any",
            permission: "files.view",
            holder_kind: "user",
            holder_id: Some(site.eve.id()),
            holder_role: None,
            options: &serde_json::json!({}),
            expires_at: None,
            note: "",
            source: "page",
            created_by: Some(site.ada.id()),
            at: site.world.now(),
        },
    )
    .await?;
    access::bump_epoch(&mut conn).await?;
    drop(conn);
    let response = page(&site, &site.eve.token, "wide", None, None, 1);
    tokio::pin!(response);
    let waiting = tokio::time::timeout(workspace::PATIENCE, async {
        tokio::select! {
            () = gate.entered.notified() => true,
            _ = &mut response => false,
        }
    })
    .await?;
    assert!(waiting, "the page must wait for the held native result");
    let mut conn = site.world.conn().await;
    assert!(access::remove_entry(&mut conn, id, site.ada.id(), site.world.now()).await?);
    access::bump_epoch(&mut conn).await?;
    drop(conn);
    gate.resume.notify_one();
    assert_eq!(
        tokio::time::timeout(workspace::PATIENCE, response)
            .await?
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    Ok(())
}
