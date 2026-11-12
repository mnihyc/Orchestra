//! The archive of a native export crosses the line for as long as it takes: the person sees how many
//! bytes have reached the system while the machine still works, the plan keeps where it was, and a stop
//! says what arrived and that it is kept.

use super::*;
use orch_api_contract::workspace::JobProgress;

/// What the system's content endpoint holds of an upload that is under way: the record a piece makes.
async fn upload_under_way(rig: &Rig, command: Id, hash: &Hash, size: i64, received: i64) {
    sqlx::query(
        "INSERT INTO blob_uploads (hash, size, received, machine_id, started_at, updated_at, command_id)
         VALUES ($1, $2, $3, $4, $5, $5, $6)
         ON CONFLICT (hash) DO UPDATE SET received = $3, updated_at = $5, command_id = $6",
    )
    .bind(hash.as_bytes().as_slice())
    .bind(size)
    .bind(received)
    .bind(rig.world.machine_id)
    .bind(rig.world.clock.now())
    .bind(command)
    .execute(rig.world.pool.pool())
    .await
    .unwrap();
}

fn export_command(rig: &Rig, thread: Id) -> Id {
    let commands = rig
        .world
        .commands
        .issued_commands("session.export")
        .into_iter()
        .filter(|(machine, command)| {
            *machine == rig.world.machine_id && command.body["thread_id"] == json!(thread)
        })
        .collect::<Vec<_>>();
    assert_eq!(commands.len(), 1, "one exact export supplies the upload");
    commands[0].1.id
}

/// The progress of the job once `enough` holds of it.
async fn progress_where(
    rig: &Rig,
    job: Id,
    what: &str,
    enough: impl Fn(&JobProgress) -> bool,
) -> JobProgress {
    let mut last = None;
    for _ in 0..600 {
        let (progress, _) = orch_engine::jobs::runner::read_progress(&rig.job(job).await);
        if enough(&progress) {
            return progress;
        }
        last = Some(progress);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the job's progress never showed {what}: it shows {last:?}");
}

/// Waits for the upload watcher to reach a table lock held by this test.
async fn wait_for_locked_upload_look(rig: &Rig) {
    eventually("the upload watcher to read the locked upload", || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid
                WHERE l.relation = 'blob_uploads'::regclass AND NOT l.granted
                  AND a.datname = current_database()
                  AND a.query LIKE '%FROM blob_uploads WHERE hash = $1 AND machine_id = $2 AND command_id = $3%'
            )",
        )
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap()
    })
    .await;
}

/// Reads progress after the watcher has inspected an upload and finished that observation.
async fn progress_after_upload_look(rig: &Rig, job: Id) -> JobProgress {
    let mut first = rig.world.pool.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE blob_uploads IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *first)
        .await
        .unwrap();
    wait_for_locked_upload_look(rig).await;
    first.commit().await.unwrap();

    // The blocked read precedes this lock in PostgreSQL's lock queue. When the next read reaches
    // the lock, the previous observation and any progress update it caused have both finished.
    let mut next = rig.world.pool.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE blob_uploads IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *next)
        .await
        .unwrap();
    wait_for_locked_upload_look(rig).await;
    let (progress, _) = orch_engine::jobs::runner::read_progress(&rig.job(job).await);
    next.commit().await.unwrap();
    progress
}

/// Has the database note every commit that leaves bytes of an upload shown for a job while the job's
/// plan does not name the upload: what a reader of the job can see between two commits is then told
/// by the database, not by the moment a poll happens to fall in.
async fn note_bytes_shown_without_the_upload_they_belong_to(rig: &Rig) {
    sqlx::raw_sql(
        r#"CREATE TABLE bytes_shown_without_their_upload (job uuid NOT NULL, bytes_done bigint NOT NULL);
        CREATE FUNCTION note_bytes_shown_without_their_upload() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            INSERT INTO bytes_shown_without_their_upload
            SELECT id, (progress->>'bytes_done')::bigint FROM jobs
             WHERE id = NEW.id AND (progress->>'bytes_done')::bigint > 0
               AND progress->>'message' = 'Uploading the session''s files'
               AND NOT (plan ? 'native_upload');
            RETURN NULL;
        END $$;
        CREATE CONSTRAINT TRIGGER note_bytes_shown_without_their_upload AFTER INSERT OR UPDATE ON jobs
            DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION note_bytes_shown_without_their_upload();"#,
    )
    .execute(rig.world.pool.pool())
    .await
    .unwrap();
}

/// The commits that were noted: the job and the bytes it showed.
async fn bytes_shown_without_the_upload_they_belong_to(rig: &Rig) -> Vec<(String, i64)> {
    sqlx::query_as::<_, (String, i64)>(
        "SELECT job::text, bytes_done FROM bytes_shown_without_their_upload ORDER BY bytes_done",
    )
    .fetch_all(rig.world.pool.pool())
    .await
    .unwrap()
}

fn native() -> Can {
    Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bytes_that_reach_the_system_are_shown_while_the_machine_puts_the_archive_up() {
    let rig = Rig::new(native(), "fake", native()).await;
    let thread = rig.thread_with_history("Crossing").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    rig.world.machine.handler.hold(CommandKind::SessionExport);
    let job = rig.confirm(thread, &plan).await;
    rig.world.wait_began(CommandKind::SessionExport, 1).await;
    let command = export_command(&rig, thread);
    // The machine puts the archive up: the system holds what has come of it, as the content endpoint does.
    let hash = Hash::of(b"the archive that is being put up");
    let size = 8_000_000_i64;
    upload_under_way(&rig, command, &hash, size, 2_000_000).await;
    let first = progress_where(&rig, job.id, "the first bytes", |progress| {
        progress.bytes_done == 2_000_000
    })
    .await;
    assert_eq!(first.bytes_total, Some(size));
    assert_eq!(first.phase, "history");
    assert_eq!(
        first.message.as_deref(),
        Some("Uploading the session's files")
    );
    upload_under_way(&rig, command, &hash, size, 6_000_000).await;
    let later = progress_where(&rig, job.id, "more bytes", |progress| {
        progress.bytes_done == 6_000_000
    })
    .await;
    assert!(
        later.percent > first.percent,
        "the bar moves with the bytes: {} then {}",
        first.percent,
        later.percent
    );
    // The plan keeps where it was, for a runner that takes the job over: it is in the commit that shows
    // the bytes, so the row that shows 6,000,000 bytes holds it already.
    let kept = rig.job(job.id).await.plan["native_upload"].clone();
    assert_eq!(kept["received"], json!(6_000_000));
    assert_eq!(kept["size"], json!(size));
    assert_eq!(kept["hash"], serde_json::to_value(hash).unwrap());
    assert_eq!(kept["command"], json!(command));
    // The hash stays the same, but another command now continues it. A locked watcher still verifies command identity.
    upload_under_way(&rig, new_id(), &hash, size, 7_000_000).await;
    let unchanged = progress_after_upload_look(&rig, job.id).await;
    assert_eq!(unchanged.bytes_done, 6_000_000, "{unchanged:?}");
    // The machine answers, and the switch goes on: the bytes of an upload that is over are not left in the progress.
    rig.world.machine.handler.release();
    let done = rig.wait_job(job.id, "succeeded").await;
    let (progress, _) = orch_engine::jobs::runner::read_progress(&done);
    assert_eq!((progress.bytes_done, progress.bytes_total), (0, None));
    assert_ne!(
        progress.message.as_deref(),
        Some("Uploading the session's files")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bytes_of_an_upload_are_never_shown_in_a_commit_that_does_not_name_the_upload() {
    let rig = Rig::new(native(), "fake", native()).await;
    note_bytes_shown_without_the_upload_they_belong_to(&rig).await;
    let thread = rig.thread_with_history("Shown with their upload").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    rig.world.machine.handler.hold(CommandKind::SessionExport);
    let job = rig.confirm(thread, &plan).await;
    rig.world.wait_began(CommandKind::SessionExport, 1).await;
    let command = export_command(&rig, thread);
    let hash = Hash::of(b"the archive whose bytes are shown with its name");
    let size = 8_000_000_i64;
    // The first bytes, a small step on (under a twentieth of the whole: only the progress moves), and a
    // long step on (the plan moves with the progress).
    for received in [2_000_000, 2_100_000, 6_000_000] {
        upload_under_way(&rig, command, &hash, size, received).await;
        progress_where(&rig, job.id, "the bytes that came", |progress| {
            progress.bytes_done == received
        })
        .await;
        assert_eq!(
            bytes_shown_without_the_upload_they_belong_to(&rig).await,
            Vec::<(String, i64)>::new(),
            "bytes were shown for the job in a commit whose plan did not name their upload"
        );
        let kept = rig.job(job.id).await.plan["native_upload"].clone();
        assert_eq!(kept["hash"], json!(hash), "{kept}");
        assert_eq!(kept["command"], json!(command), "{kept}");
        assert_eq!(kept["size"], json!(size), "{kept}");
    }
    let kept = rig.job(job.id).await.plan["native_upload"].clone();
    assert_eq!(
        kept["received"],
        json!(6_000_000),
        "the plan holds where the bytes shown had come: {kept}"
    );
    rig.world.machine.handler.release();
    rig.wait_job(job.id, "succeeded").await;
    assert_eq!(
        bytes_shown_without_the_upload_they_belong_to(&rig).await,
        Vec::<(String, i64)>::new(),
        "bytes were shown for the job in a commit whose plan did not name their upload"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_shown_while_two_uploads_of_the_command_cannot_be_told_apart_and_the_one_left_is_shown()
 {
    let rig = Rig::new(native(), "fake", native()).await;
    let thread = rig.thread_with_history("Two at once").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    rig.world.machine.handler.hold(CommandKind::SessionExport);
    let job = rig.confirm(thread, &plan).await;
    rig.world.wait_began(CommandKind::SessionExport, 1).await;
    let command = export_command(&rig, thread);
    let (ours, other) = (Hash::of(b"the archive"), Hash::of(b"something else"));
    upload_under_way(&rig, command, &ours, 4_000_000, 1_000_000).await;
    upload_under_way(&rig, command, &other, 9_000_000, 5_000_000).await;
    // Several looks later, the bytes of neither are put to the job's account.
    meanwhile(Duration::from_millis(3500)).await;
    let (progress, _) = orch_engine::jobs::runner::read_progress(&rig.job(job.id).await);
    assert_eq!(progress.bytes_done, 0, "{progress:?}");
    assert_eq!(progress.bytes_total, None);
    // The other work of the machine ends; what is left is the archive.
    sqlx::query("DELETE FROM blob_uploads WHERE hash = $1")
        .bind(other.as_bytes().as_slice())
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    let shown = progress_where(&rig, job.id, "the one that is left", |progress| {
        progress.bytes_done == 1_000_000
    })
    .await;
    assert_eq!(shown.bytes_total, Some(4_000_000));
    rig.world.machine.handler.release();
    rig.wait_job(job.id, "succeeded").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sole_upload_of_another_command_never_supplies_this_jobs_bytes() {
    let rig = Rig::new(native(), "fake", native()).await;
    let thread = rig.thread_with_history("One unrelated upload").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    rig.world.machine.handler.hold(CommandKind::SessionExport);
    let job = rig.confirm(thread, &plan).await;
    rig.world.wait_began(CommandKind::SessionExport, 1).await;
    let command = export_command(&rig, thread);
    upload_under_way(
        &rig,
        new_id(),
        &Hash::of(b"unrelated"),
        9_000_000,
        5_000_000,
    )
    .await;
    meanwhile(Duration::from_millis(3500)).await;
    let (progress, _) = orch_engine::jobs::runner::read_progress(&rig.job(job.id).await);
    assert_eq!(progress.bytes_done, 0, "{progress:?}");
    assert_eq!(progress.bytes_total, None);
    assert!(rig.job(job.id).await.plan.get("native_upload").is_none());

    let ours = Hash::of(b"the actual archive");
    upload_under_way(&rig, command, &ours, 4_000_000, 1_000_000).await;
    let progress = progress_where(
        &rig,
        job.id,
        "our exact command despite other work",
        |progress| progress.bytes_done == 1_000_000,
    )
    .await;
    assert_eq!(progress.bytes_total, Some(4_000_000));
    assert_eq!(
        rig.job(job.id).await.plan["native_upload"]["hash"],
        json!(ours)
    );
    rig.world.machine.handler.release();
    rig.wait_job(job.id, "succeeded").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_after_part_of_the_archive_arrived_says_so_and_that_it_is_kept() {
    let rig = Rig::new(native(), "fake", native()).await;
    let thread = rig.thread_with_history("Stopped").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    // The line stands still before the end.
    rig.on_export(|_| {
        Err(CommandError::new(ErrorCode::Timeout, "the transfer stood still").retryable())
    });
    rig.world.machine.handler.hold(CommandKind::SessionExport);
    let job = rig.confirm(thread, &plan).await;
    rig.world.wait_began(CommandKind::SessionExport, 1).await;
    let command = export_command(&rig, thread);
    let hash = Hash::of(b"half an archive");
    upload_under_way(&rig, command, &hash, 8_000_000, 3_000_000).await;
    progress_where(&rig, job.id, "the bytes that came", |progress| {
        progress.bytes_done == 3_000_000
    })
    .await;
    rig.world.machine.handler.release();
    let row = rig.wait_job(job.id, "failed").await;
    let error = row.error.expect("the job says why");
    let told = error["message"].as_str().unwrap_or_default();
    assert!(
        told.contains("the transfer stood still")
            && told.contains("2.9 MiB of the 7.6 MiB")
            && told.contains("goes on from there"),
        "{error}"
    );
    assert_eq!(error["code"], json!("timeout"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_discarded_partial_upload_is_neither_complete_nor_promised_as_kept() {
    let rig = Rig::new(native(), "fake", native()).await;
    let thread = rig.thread_with_history("Rejected upload").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    rig.on_export(|_| {
        Err(CommandError::new(
            ErrorCode::Internal,
            "the content hash was rejected",
        ))
    });
    rig.world.machine.handler.hold(CommandKind::SessionExport);
    let job = rig.confirm(thread, &plan).await;
    rig.world.wait_began(CommandKind::SessionExport, 1).await;
    let command = export_command(&rig, thread);
    let hash = Hash::of(b"a discarded upload");
    upload_under_way(&rig, command, &hash, 8_000_000, 2_000_000).await;
    progress_where(
        &rig,
        job.id,
        "the partial bytes before rejection",
        |progress| progress.bytes_done == 2_000_000,
    )
    .await;
    sqlx::query("DELETE FROM blob_uploads WHERE hash = $1")
        .bind(hash.as_bytes().as_slice())
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    meanwhile(Duration::from_millis(3500)).await;
    let (progress, _) = orch_engine::jobs::runner::read_progress(&rig.job(job.id).await);
    assert_eq!(
        progress.bytes_done, 2_000_000,
        "a missing row is not completion: {progress:?}"
    );
    assert_eq!(progress.bytes_total, Some(8_000_000));
    rig.world.machine.handler.release();
    let failed = rig.wait_job(job.id, "failed").await;
    let error = failed.error.expect("the job says why");
    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains("the content hash was rejected"), "{error}");
    assert!(
        !message.contains("goes on from there"),
        "a discarded partial file is not kept: {error}"
    );
}
