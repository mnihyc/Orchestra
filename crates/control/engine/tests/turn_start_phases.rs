//! How long the start of a turn takes, phase by phase, and what each phase asks of the database, the store of
//! content and the machine: the first turn of a thread, which opens its session, and the next turn of that
//! session, whose files are as they were. The machine answers each recording of the files before a turn with the
//! same manifest, as a machine does whose directory did not change. Run with `--nocapture` to see the table.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    reason = "a test fails by panicking and shows what it measured"
)]

mod common;

use async_trait::async_trait;
use common::{World, queued, turn_ended};
use orch_authz::Caller;
use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobError, BlobRead, BlobStore, ByteStream, Stored, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::queries::{source as source_rows, threads};
use orch_engine::manifest::{self, Header};
use orch_link_contract::commands::{CommandKind, CommandResult};
use orch_link_contract::events::TurnOutcome;
use orch_link_contract::source::{FsRecordResult, RecordState};
use orch_types::{Hash, Id, Time};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

/// One statement the database was sent: when, whether the engine's work on a thread sent it, by which task, and
/// whether it takes the thread's row lock.
#[derive(Clone)]
struct Statement {
    at: Instant,
    work: bool,
    task: Option<tokio::task::Id>,
    locks_thread: bool,
    /// Not a statement: a connection taken from the pool (which the pool first asks whether it is alive).
    acquired: bool,
    summary: String,
}

static STATEMENTS: OnceLock<Mutex<Vec<Statement>>> = OnceLock::new();
static INSTALLED: Once = Once::new();

fn statements() -> std::sync::MutexGuard<'static, Vec<Statement>> {
    STATEMENTS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct Sql {
    summary: String,
    statement: String,
    message: String,
}

impl Visit for Sql {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "summary" => value.clone_into(&mut self.summary),
            "db.statement" => value.clone_into(&mut self.statement),
            "message" => value.clone_into(&mut self.message),
            _ => {}
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if matches!(field.name(), "summary" | "db.statement" | "message") {
            self.record_str(field, &format!("{value:?}"));
        }
    }
}

/// Keeps every statement SQLx reports, with the work it was sent for.
struct Recorder;

impl<S: tracing::Subscriber> Layer<S> for Recorder {
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        let target = event.metadata().target();
        if target != "sqlx::query" && target != "orch_db::pool" {
            return;
        }
        let mut sql = Sql::default();
        event.record(&mut sql);
        if target == "orch_db::pool" {
            if sql.message.contains("database connection acquired") {
                statements().push(Statement {
                    at: Instant::now(),
                    work: orch_db::pool::serving_route() == Some("thread work"),
                    task: tokio::task::try_id(),
                    locks_thread: false,
                    acquired: true,
                    summary: String::new(),
                });
            }
            return;
        }
        let whole = if sql.statement.is_empty() {
            &sql.summary
        } else {
            &sql.statement
        };
        let summary: String = sql
            .summary
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(72)
            .collect();
        let statement = Statement {
            at: Instant::now(),
            work: orch_db::pool::serving_route() == Some("thread work"),
            task: tokio::task::try_id(),
            locks_thread: whole.contains("FOR UPDATE OF t"),
            acquired: false,
            summary,
        };
        statements().push(statement);
    }
}

fn record_statements() {
    INSTALLED.call_once(|| {
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Recorder))
            .expect("the statement recorder is installed once for this test program");
    });
}

/// The store of content, counting how often a blob is read whole or in part.
struct CountedStore {
    local: Arc<LocalBlobStore>,
    reads: AtomicUsize,
}

#[async_trait]
impl BlobStore for CountedStore {
    fn lifetimes(&self) -> &orch_blob_contract::lifetime::Lifetimes {
        self.local.lifetimes()
    }
    async fn put_held(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
        publication: &orch_blob_contract::lifetime::Publication,
    ) -> Result<Stored, BlobError> {
        publication.check(self.lifetimes(), hash)?;
        self.local.put_held(hash, size, body, publication).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.local.get(hash, range).await
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.local.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        self.local.size(hash).await
    }
    async fn delete_held(
        &self,
        hash: &Hash,
        retirement: &orch_blob_contract::lifetime::Retirement,
    ) -> Result<bool, BlobError> {
        self.local.delete_held(hash, retirement).await
    }
}

/// When each command reached the machine, by kind, in order.
type Arrivals = Arc<Mutex<Vec<(Instant, CommandKind)>>>;

fn follow_arrivals(world: &World) -> (Arrivals, tokio::task::JoinHandle<()>) {
    let machine = Arc::clone(&world.machine);
    let arrivals: Arrivals = Arc::default();
    let kept = Arc::clone(&arrivals);
    let task = tokio::spawn(async move {
        let mut seen = 0;
        loop {
            let runs = machine.handler.executions();
            if runs.len() > seen {
                let now = Instant::now();
                kept.lock()
                    .unwrap()
                    .extend(runs[seen..].iter().map(|run| (now, run.kind)));
                seen = runs.len();
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    (arrivals, task)
}

/// One stretch of a start, between two moments: the statements of the engine's work on the thread in it.
struct Stretch {
    name: String,
    took: Duration,
    statements: usize,
    locks: usize,
    acquired: usize,
}

/// What one start of a turn took, from the person's send to the arrival of `turn.start` at the machine.
struct Start {
    label: &'static str,
    total: Duration,
    send: Duration,
    send_statements: usize,
    send_locks: usize,
    work_statements: usize,
    work_locks: usize,
    work_acquired: usize,
    manifest_reads: usize,
    commands: Vec<(Duration, CommandKind)>,
    stretches: Vec<Stretch>,
    most: Vec<(usize, String)>,
}

impl Start {
    fn kinds(&self) -> Vec<CommandKind> {
        self.commands.iter().map(|(_, kind)| *kind).collect()
    }

    fn report(&self) {
        let millis = |took: Duration| took.as_secs_f64() * 1000.0;
        eprintln!(
            "== {}: {:.0} ms from send to turn.start; send {:.0} ms ({} statements, {} thread locks); \
             engine work {} statements, {} thread locks, {} connections taken; manifest reads {}",
            self.label,
            millis(self.total),
            millis(self.send),
            self.send_statements,
            self.send_locks,
            self.work_statements,
            self.work_locks,
            self.work_acquired,
            self.manifest_reads
        );
        for (at, kind) in &self.commands {
            eprintln!(
                "   command {kind} reached the machine at +{:.0} ms",
                millis(*at)
            );
        }
        for stretch in &self.stretches {
            eprintln!(
                "   {:<40} {:>7.0} ms {:>5} statements {:>3} thread locks {:>4} connections",
                stretch.name,
                millis(stretch.took),
                stretch.statements,
                stretch.locks,
                stretch.acquired
            );
        }
        for (count, summary) in &self.most {
            eprintln!("   {count:>5} × {summary}");
        }
    }
}

/// Waits until nothing of the turns before is left to do: the thread is idle, the end of its last turn is accounted
/// for and sealed, and its publication has had time to end.
async fn settle(world: &World, thread: Id) {
    world.wait_state(thread, "idle").await;
    common::eventually("the last turn's end is done", || async {
        let mut conn = world.pool.acquire().await.unwrap();
        !threads::turn_end_work_pending(&mut conn, thread)
            .await
            .unwrap()
            && source_rows::unsealed_end_turn(&mut conn, thread)
                .await
                .unwrap()
                .is_none()
            && source_rows::end_due_turn(&mut conn, thread)
                .await
                .unwrap()
                .is_none()
    })
    .await;
    common::meanwhile(Duration::from_millis(300)).await;
}

/// Sends `text` to `thread`, measures its start up to the arrival of its `turn.start` (the `nth` of the thread), and
/// ends the turn.
async fn measured_start(
    world: &World,
    thread: Id,
    nth: usize,
    label: &'static str,
    arrivals: &Arrivals,
    store: &CountedStore,
) -> Start {
    settle(world, thread).await;
    let first_statement = statements().len();
    let first_arrival = arrivals.lock().unwrap().len();
    let reads = store.reads.load(Ordering::SeqCst);
    let me = tokio::task::try_id();
    let began = Instant::now();
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued(label))
        .await
        .unwrap();
    let sent = Instant::now();
    let start = world.turn_start(thread, nth).await;
    let reached = arrivals.lock().unwrap()[first_arrival..]
        .iter()
        .find(|(_, kind)| matches!(kind, CommandKind::TurnStart | CommandKind::TurnStartFenced))
        .map_or_else(Instant::now, |(at, _)| *at);
    let manifest_reads = store.reads.load(Ordering::SeqCst) - reads;
    let seen: Vec<Statement> = statements()[first_statement..]
        .iter()
        .filter(|statement| statement.at <= reached)
        .cloned()
        .collect();
    let commands: Vec<(Instant, CommandKind)> = arrivals.lock().unwrap()[first_arrival..]
        .iter()
        .filter(|(at, _)| *at <= reached)
        .copied()
        .collect();
    let send: Vec<&Statement> = seen
        .iter()
        .filter(|statement| !statement.acquired && statement.task == me && statement.at <= sent)
        .collect();
    let work: Vec<&Statement> = seen
        .iter()
        .filter(|statement| statement.work && !statement.acquired)
        .collect();
    let taken: Vec<&Statement> = seen
        .iter()
        .filter(|statement| statement.work && statement.acquired)
        .collect();
    // The stretches between the send and each command's arrival at the machine.
    let mut marks: Vec<(Instant, String)> = vec![(sent, "until the first command".to_owned())];
    for (at, kind) in &commands {
        marks.push((*at, format!("after {kind}")));
    }
    let mut stretches = Vec::new();
    for (index, (from, name)) in marks.iter().enumerate() {
        let to = marks.get(index + 1).map_or(reached, |(at, _)| *at);
        let inside: Vec<&&Statement> = work
            .iter()
            .filter(|statement| statement.at > *from && statement.at <= to)
            .collect();
        stretches.push(Stretch {
            name: name.clone(),
            took: to.saturating_duration_since(*from),
            statements: inside.len(),
            locks: inside
                .iter()
                .filter(|statement| statement.locks_thread)
                .count(),
            acquired: taken
                .iter()
                .filter(|statement| statement.at > *from && statement.at <= to)
                .count(),
        });
    }
    let mut counted: BTreeMap<String, usize> = BTreeMap::new();
    for statement in &work {
        *counted.entry(statement.summary.clone()).or_default() += 1;
    }
    let mut most: Vec<(usize, String)> = counted
        .into_iter()
        .map(|(summary, count)| (count, summary))
        .collect();
    most.sort_by(|one, two| two.0.cmp(&one.0).then_with(|| one.1.cmp(&two.1)));
    most.truncate(12);
    let measured = Start {
        label,
        total: reached.saturating_duration_since(began),
        send: sent.saturating_duration_since(began),
        send_statements: send.len(),
        send_locks: send
            .iter()
            .filter(|statement| statement.locks_thread)
            .count(),
        work_statements: work.len(),
        work_locks: work
            .iter()
            .filter(|statement| statement.locks_thread)
            .count(),
        work_acquired: taken.len(),
        manifest_reads,
        commands: commands
            .iter()
            .map(|(at, kind)| (at.saturating_duration_since(began), *kind))
            .collect(),
        stretches,
        most,
    };
    world
        .answer(thread, &start, &format!("m{nth}"), "Done.")
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    measured
}

/// How many files the recorded directory holds: a large repository's.
const FILES: i32 = 50_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_next_turn_of_an_open_session_starts_without_harness_session_or_manifest_work() {
    record_statements();
    // The deployed pace of the supervisor: a sweep a second, not the small worlds' every forty milliseconds.
    let world = World::tuned(|config| {
        let deployed = orch_engine::config::EngineConfig::default();
        config.sweep_every = deployed.sweep_every;
        config.recheck_every = deployed.recheck_every;
        config.post_commit_grace = deployed.post_commit_grace;
    })
    .await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(CountedStore {
        local: Arc::new(LocalBlobStore::open(directory.path()).await.unwrap()),
        reads: AtomicUsize::new(0),
    });
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Phases of a start").await;
    // The directory's files as the machine records them before each turn: a large directory, unchanged between
    // turns, so that each recording answers with the same manifest.
    let (root, relative, generation): (String, String, i32) = sqlx::query_as(
        "SELECT e.workspace_root,t.workspace_dir,e.generation FROM threads t JOIN environments e ON e.id=t.environment_id WHERE t.node_id=$1")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    let before = manifest::build(
        &Header {
            v: manifest::MANIFEST_VERSION,
            root: orch_engine::sessions::absolute_directory(&root, &relative),
            generated_at: Time::new(world.clock.now()),
            generation: i64::from(generation),
            case_sensitive: true,
        },
        (0..FILES)
            .map(|index| manifest::Entry {
                path: format!("src/file-{index:06}.rs"),
                kind: manifest::EntryKind::File,
                size: 64,
                mtime_ns: 0,
                mode: 0o644,
                hash: Some(Hash::of(format!("file {index}").as_bytes())),
                link: None,
            })
            .collect(),
    )
    .unwrap();
    store
        .local
        .put(
            &before.hash,
            Some(i64::try_from(before.bytes.len()).unwrap()),
            stream_of(vec![bytes::Bytes::copy_from_slice(&before.bytes)]),
        )
        .await
        .unwrap();
    let hash = before.hash;
    let expires = Time::new(world.clock.now() + chrono::Duration::days(7));
    world.machine.handler.on(CommandKind::FsRecord, move |_| {
        Ok(CommandResult::FsRecord(FsRecordResult {
            indexing: None,
            state: RecordState::Complete,
            tree_ref: Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904".into()),
            repository_head: None,
            manifest_blob: Some(hash),
            files: i64::from(FILES),
            bytes: i64::from(FILES) * 64,
            expires_at: Some(expires),
            reason: None,
        }))
    });
    let (arrivals, follower) = follow_arrivals(&world);

    let first = measured_start(&world, thread, 0, "First turn", &arrivals, &store).await;
    let second = measured_start(&world, thread, 1, "Next turn", &arrivals, &store).await;
    follower.abort();
    first.report();
    second.report();

    // Both turns recorded their files and bound the recording, whatever was read to make sure of it.
    let mut conn = world.pool.acquire().await.unwrap();
    let recorded: Vec<(String, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT s.state, s.manifest_blob FROM turns u JOIN snapshots s ON s.id=u.before_snapshot_id
          WHERE u.thread_id=$1 ORDER BY u.ordinal",
    )
    .bind(thread)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        recorded,
        vec![
            ("complete".to_owned(), Some(hash.as_bytes().to_vec())),
            ("complete".to_owned(), Some(hash.as_bytes().to_vec())),
        ],
        "each turn has its restore point, made of the machine's recording"
    );
    drop(conn);
    assert_eq!(
        first
            .kinds()
            .iter()
            .filter(|kind| **kind == CommandKind::FsRecord)
            .count(),
        1
    );
    assert_eq!(
        second
            .kinds()
            .iter()
            .filter(|kind| **kind == CommandKind::FsRecord)
            .count(),
        1,
        "the next turn records its files once"
    );
    assert!(
        !second
            .kinds()
            .iter()
            .any(|kind| matches!(kind, CommandKind::AgentStart | CommandKind::SessionOpen)),
        "the next turn uses the harness and the session that are open: {:?}",
        second.kinds()
    );
    assert_eq!(
        first.manifest_reads, 1,
        "the first recording's manifest is read once, to make sure it covers the directory"
    );
    assert_eq!(
        second.manifest_reads, 0,
        "a recording whose manifest was made sure of already is not read again"
    );
    assert!(
        second.work_statements < first.work_statements,
        "the next turn's start does less than the first: {} statements against {}",
        second.work_statements,
        first.work_statements
    );
}
