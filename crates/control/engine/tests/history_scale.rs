//! A full-size history runs in its own program so its database work cannot hold up unit fixtures.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, eventually, queued};
use orch_api_contract::exports::{ExportChosen, ExportParts, ExportRequest, ExportWhat};
use orch_authz::Caller;
use orch_blob_contract::store::{BlobStore, read_all};
use orch_blob_local::LocalBlobStore;
use orch_db::audit::Origin;
use orch_db::queries::{exports, jobs, threads};
use orch_engine::{fork_history, unit::Unit, written};
use orch_types::Hash;
use std::io::{BufRead, BufReader, Cursor};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The complete scale requested for long-thread capture, copy and streamed export.
const TURNS: i64 = 200_000;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn two_hundred_thousand_turns_keep_a_bounded_carry_and_copy_and_export_every_page() -> Proof {
    let began = Instant::now();
    tracing_subscriber::fmt()
        .with_env_filter("sqlx::query=warn,orch_engine::fork_history=debug,orch_db::queries::thread_history_cuts=debug")
        .with_test_writer()
        .try_init()?;
    // This scale program measures production history work rather than the
    // accelerated reapers used by small behavioral worlds.
    let world = World::tuned(|config| {
        let deployed = orch_engine::config::EngineConfig::default();
        config.sweep_every = deployed.sweep_every;
        config.recheck_every = deployed.recheck_every;
        config.post_commit_grace = deployed.post_commit_grace;
        config.hosting.reaper_every = deployed.hosting.reaper_every;
        config.cap_check = deployed.cap_check;
    })
    .await;
    let directory = tempfile::tempdir()?;
    let store = Arc::new(LocalBlobStore::open(directory.path().to_owned()).await?);
    world.engine().attach_blobs(store.clone());
    let source = world.thread("A full history scale proof").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), source, queued("Ask"))
        .await?;
    world.run_turn(source, 0, "Answer").await;
    eventually("the seed turn is accounted", || async {
        let mut conn = world.pool.acquire().await.unwrap();
        !threads::turn_end_work_pending(&mut conn, source)
            .await
            .unwrap()
    })
    .await;
    world.wait_followed_up(source).await;
    eprintln!(
        "history-scale phase=fixture elapsed_ms={}",
        began.elapsed().as_millis()
    );
    world.grow_history(source, TURNS).await;
    eprintln!(
        "history-scale phase=grown turns={TURNS} elapsed_ms={}",
        began.elapsed().as_millis()
    );
    let mut conn = world.pool.acquire().await?;
    let carry = written::read_conversation(
        &mut conn,
        source,
        (None, None),
        written::Detail::Normal,
        Some(64 * 1024),
    )
    .await?
    .ok_or("the source is missing")?;
    assert_eq!(carry.last, TURNS);
    assert_eq!(carry.to, TURNS);
    assert!(carry.from > 1 && carry.text.len() <= 64 * 1024);
    drop(conn);
    let target = world.thread("A full history copy").await;
    let segment = world.segment_of(target).await;
    let defaults = orch_config::ServerConfig::default().database;
    let requests = orch_db::Db::connect_with_limits(
        &world.db.url,
        1,
        Duration::from_millis(defaults.slow_statement_ms),
        Duration::from_millis(defaults.slow_acquire_ms),
        orch_db::pool::Limits::REQUESTS,
    )
    .await?;
    let mut tx = requests.begin().await?;
    let profile: (String, String) =
        sqlx::query_as("SELECT current_setting('jit'),current_setting('statement_timeout')")
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(profile, ("off".to_owned(), "15s".to_owned()));
    let copying = Instant::now();
    fork_history::copy_conversation(
        &mut Unit::new(&mut tx, world.clock.now()),
        source,
        target,
        segment,
        Some((1, i32::try_from(TURNS)?)),
        None,
    )
    .await?;
    tx.commit().await?;
    eprintln!(
        "history-scale phase=copied copy_ms={} elapsed_ms={}",
        copying.elapsed().as_millis(),
        began.elapsed().as_millis()
    );
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM turns WHERE thread_id=$1),
        (SELECT count(*) FROM items WHERE thread_id=$1 AND kind<>'marker')",
    )
    .bind(target)
    .fetch_one(world.pool.pool())
    .await?;
    assert_eq!(counts, (TURNS, 2 * TURNS));
    requests.pool().close().await;

    // Use the real streamed Markdown archive writer, rather than the intentionally
    // bounded string transcript API. Only the compressed archive is held here.
    let request = ExportRequest {
        what: ExportWhat::Chosen(ExportChosen {
            nodes: Some(vec![source]),
            ..ExportChosen::default()
        }),
        parts: Some(ExportParts {
            conversations: Some(true),
            chats: Some(false),
            documents: Some(false),
        }),
        format: None,
        detail: None,
        files: Some(false),
        versions: None,
        comments: None,
        from: None,
        to: None,
        package: Some(false),
    };
    let made = world
        .engine()
        .make_export(
            &Caller::session(world.ada),
            &Origin::default(),
            world.root,
            &request,
        )
        .await?;
    // The hosted job bounds this isolated program. Completion follows the
    // archive's terminal state, rather than a second race with runner speed.
    let mut previous = None;
    loop {
        let mut conn = world.pool.acquire().await.unwrap();
        let job = jobs::job(&mut conn, made.job.id).await.unwrap().unwrap();
        if previous.as_ref() != Some(&job.progress) {
            eprintln!(
                "history-scale phase=export-progress state={} progress={} elapsed_ms={}",
                job.state,
                job.progress,
                began.elapsed().as_millis()
            );
            previous = Some(job.progress.clone());
        }
        if !job.is_open() {
            break;
        }
        drop(conn);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut conn = world.pool.acquire().await?;
    let export = exports::get(&mut conn, made.export.id)
        .await?
        .ok_or("the export is missing")?;
    assert_eq!(export.state, "ready", "{:?}", export.left_out);
    let hash: Hash = export
        .blob
        .as_deref()
        .ok_or("the archive has no blob")?
        .parse()?;
    drop(conn);
    let bytes = read_all(store.get(&hash, None).await?.body).await?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut conversations = 0;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        if !file.name()?.ends_with(".md") {
            continue;
        }
        let mut turns = 0;
        for line in BufReader::new(file).lines() {
            if line?.starts_with("## Turn ") {
                turns += 1;
            }
        }
        if turns != 0 {
            assert_eq!(turns, TURNS);
            conversations += 1;
        }
    }
    assert_eq!(conversations, 1);
    eprintln!(
        "history-scale phase=exported elapsed_ms={}",
        began.elapsed().as_millis()
    );
    world.shutdown().await;
    Ok(())
}
