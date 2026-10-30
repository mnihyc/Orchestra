//! Isolated PostgreSQL fixtures.
//!
//! Every test gets a database of its own, so tests run side by side and
//! none sees another's rows. The schema is applied once, to a template
//! named after the content of the migrations; each test's database is a
//! copy of that template. Admission reserves connections across cooperating
//! test processes before fixture connections are opened; the reservation remains
//! held until cleanup ends. Exact ownership and cleanup results are kept in
//! the lease directory, so a stopped run can clean up only its own databases.
//!
//! The server is the one `ORCH_TEST_DATABASE_URL` names; without the
//! variable, a PostgreSQL 17 on `127.0.0.1:55432` with user and password
//! `orch`, which is what the repository's development container is.

use serde::{Deserialize, Serialize};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgConnectOptions, postgres::PgPoolOptions};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

/// The variable that names the server tests use: a URL of its `postgres`
/// database, for a user that may create databases.
pub const URL_VARIABLE: &str = "ORCH_TEST_DATABASE_URL";
const DEFAULT_URL: &str = "postgres://orch:orch@127.0.0.1:55432/postgres";

/// The directory shared by test processes using the same database server.
pub const LEASE_DIRECTORY_VARIABLE: &str = "ORCH_TEST_DATABASE_LEASE_DIR";
/// Connections reserved for all fixtures together on a server.
pub const CONNECTION_BUDGET_VARIABLE: &str = "ORCH_TEST_DATABASE_CONNECTION_BUDGET";
/// Connections reserved for one fixture, including pools opened through its URL.
pub const CONNECTION_RESERVATION_VARIABLE: &str = "ORCH_TEST_DATABASE_CONNECTIONS_PER_FIXTURE";
/// The identifier whose exact database records a test job checks at its end.
pub const RUN_ID_VARIABLE: &str = "ORCH_TEST_DATABASE_RUN_ID";
/// Seconds admission may wait for another fixture to finish.
pub const ADMISSION_WAIT_VARIABLE: &str = "ORCH_TEST_DATABASE_WAIT_SECS";

/// Four fixture connections suffice when several transactions run together.
const POOL_SIZE: u32 = 4;
/// The largest existing secondary test pool uses 24 connections. Reserve
/// those, the fixture's four, and four for administration, listeners and headroom.
const MINIMUM_RESERVATION: u32 = POOL_SIZE + 24 + 4;
/// Preserve the original four-fixture allowance when the server can fit it.
const DEFAULT_CONNECTION_BUDGET: u32 = MINIMUM_RESERVATION * 4;
/// Leave one fifth of total connections for non-fixture traffic, matching the
/// dedicated test server's 960-connection allowance out of 1,200 connections.
const SERVER_HEADROOM_DIVISOR: u32 = 5;
/// Admission allows the deployed enrollment and attach waits (120 + 150 seconds)
/// and one job lease (30 seconds) for a fixture ahead of it to finish.
const DEFAULT_ADMISSION_WAIT: u64 = 300;
/// An admission poll touches only local locks; twenty looks a second avoid busy waiting.
const ADMISSION_POLL: Duration = Duration::from_millis(50);
/// Cleanup has the same administrative bound as admission, including a slow
/// database drop on a busy server; a failure remains recorded for the job runner.
const CLEANUP_WAIT: Duration = Duration::from_secs(DEFAULT_ADMISSION_WAIT);
/// Leave two minutes for assertions and cleanup within the twelve-minute test-program guard.
const STATEMENT_TIMEOUT: &str = "10min";
/// Notice an abandoned test client even while PostgreSQL is executing its statement.
const CLIENT_CONNECTION_CHECK_INTERVAL: &str = "5s";

/// A key for the advisory lock under which the template is made, so that
/// tests starting together make it once. Any constant will do.
const TEMPLATE_LOCK: i64 = 0x6f72_6368_5f74_706c;

/// A database with the whole schema and no rows, gone when this is dropped.
#[derive(Debug)]
pub struct TestDb {
    /// A pool on the database.
    pub pool: PgPool,
    /// Its address, for code that connects by itself.
    pub url: String,
    lease: Option<Lease>,
}

/// Why a fixture's database could not be removed or its result recorded.
#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    /// The database server refused cleanup.
    #[error("{0}")]
    Database(#[from] sqlx::Error),
    /// The ownership record could not be written.
    #[error("{0}")]
    Record(#[from] io::Error),
    /// The database server did not finish cleanup within its administrative bound.
    #[error("database cleanup did not finish within {CLEANUP_WAIT:?}")]
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Budget {
    connections: u32,
    per_fixture: u32,
}

impl Budget {
    async fn configured(admin: &PgConnectOptions, directory: &Path) -> io::Result<Self> {
        let connections = if std::env::var_os(CONNECTION_BUDGET_VARIABLE).is_some() {
            configured_number(CONNECTION_BUDGET_VARIABLE, DEFAULT_CONNECTION_BUDGET)?
        } else {
            // A full small server still has to answer capacity probes. Serialize
            // them across processes so waiting fixtures add at most one probe
            // connection, rather than a connection each before admission.
            let discovery = lock_file(&directory.join("capacity.lock"))?;
            while !try_lock(&discovery)? {
                tokio::time::sleep(ADMISSION_POLL).await;
            }
            let mut connection = PgConnection::connect_with(admin)
                .await
                .map_err(io::Error::other)?;
            let (total, usable) = server_capacity(&mut connection)
                .await
                .map_err(io::Error::other)?;
            connection.close().await.map_err(io::Error::other)?;
            total
                .saturating_sub(total / SERVER_HEADROOM_DIVISOR)
                .max(DEFAULT_CONNECTION_BUDGET)
                .min(usable)
        };
        let budget = Self {
            connections,
            per_fixture: configured_number(CONNECTION_RESERVATION_VARIABLE, MINIMUM_RESERVATION)?,
        };
        if budget.per_fixture < MINIMUM_RESERVATION || budget.connections < budget.per_fixture {
            return Err(io::Error::other(format!(
                "database admission needs at least {MINIMUM_RESERVATION} connections per fixture and a budget large enough for one fixture"
            )));
        }
        Ok(budget)
    }

    fn slots(self) -> u32 {
        self.connections / self.per_fixture
    }
}

async fn server_capacity(connection: &mut PgConnection) -> Result<(u32, u32), sqlx::Error> {
    sqlx::query_as(
        "SELECT current_setting('max_connections')::int,
                current_setting('max_connections')::int
                  - current_setting('superuser_reserved_connections')::int
                  - current_setting('reserved_connections')::int",
    )
    .fetch_one(connection)
    .await
    .and_then(|(total, usable): (i32, i32)| {
        Ok((
            u32::try_from(total).map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            u32::try_from(usable).map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        ))
    })
}

fn configured_number<T>(name: &str, default: T) -> io::Result<T>
where
    T: FromStr + PartialEq + Default,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse::<T>()
            .ok()
            .filter(|value| *value != T::default())
            .ok_or_else(|| io::Error::other(format!("{name} must be a positive integer"))),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(io::Error::other(error)),
    }
}

#[derive(Debug, Serialize)]
struct LeaseRecord {
    database: String,
    run_id: String,
    pid: u32,
    server_host: String,
    server_port: u16,
    slot: u32,
    budget: Budget,
    state: &'static str,
    error: Option<String>,
}

#[derive(Debug)]
struct Lease {
    // Keeping the file open keeps its kernel lock held, including during cleanup.
    _slot: File,
    record_path: PathBuf,
    record: LeaseRecord,
    admin: PgConnectOptions,
}

fn lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn try_lock(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    // An interrupted writer leaves the previous complete ownership record
    // intact. Creation finishes this record before creating the database.
    let pending = path.with_extension("pending");
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let mut file = File::create(&pending)?;
    file.write_all(&bytes)?;
    file.sync_data()?;
    std::fs::rename(pending, path)
}

/// The budget file serializes admission and configuration changes. A new
/// budget is accepted only when every old slot is free, so different jobs
/// cannot silently disagree about the capacity while a fixture is live.
fn try_slot(directory: &Path, requested: Budget) -> io::Result<Option<(File, u32)>> {
    let gate = lock_file(&directory.join("budget.lock"))?;
    if !try_lock(&gate)? {
        return Ok(None);
    }
    let policy = directory.join("budget.json");
    let previous = match std::fs::read(&policy) {
        Ok(bytes) => Some(serde_json::from_slice::<Budget>(&bytes).map_err(io::Error::other)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if previous != Some(requested) {
        let mut old_slots = Vec::new();
        if let Some(previous) = previous {
            for index in 0..previous.slots() {
                let file = lock_file(&directory.join(format!("slot-{index}.lock")))?;
                if !try_lock(&file)? {
                    return Err(io::Error::other(
                        "database connection budget differs from an active fixture's budget",
                    ));
                }
                old_slots.push(file);
            }
        }
        write_json(&policy, &requested)?;
        drop(old_slots);
    }
    for index in 0..requested.slots() {
        let file = lock_file(&directory.join(format!("slot-{index}.lock")))?;
        if try_lock(&file)? {
            return Ok(Some((file, index)));
        }
    }
    Ok(None)
}

impl Lease {
    async fn acquire(admin: PgConnectOptions) -> io::Result<Self> {
        let allowed = Duration::from_secs(configured_number(
            ADMISSION_WAIT_VARIABLE,
            DEFAULT_ADMISSION_WAIT,
        )?);
        let base = std::env::var_os(LEASE_DIRECTORY_VARIABLE)
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("orch-test-databases"));
        // Credentials and the admin database do not change the server's capacity.
        let identity = format!("{}:{}", admin.get_host(), admin.get_port());
        let directory = base.join(orch_types::Hash::of(identity.as_bytes()).to_string());
        std::fs::create_dir_all(directory.join("records"))?;
        // Capacity discovery uses one short administrative connection. It is
        // closed before waiting for a slot, so queued fixtures hold none.
        let budget = Budget::configured(&admin, &directory).await?;
        let began = tokio::time::Instant::now();
        let (file, slot) = loop {
            if let Some(slot) = try_slot(&directory, budget)? {
                break slot;
            }
            if began.elapsed() >= allowed {
                return Err(io::Error::other(format!(
                    "waiting for a test database lease exceeded {allowed:?}: all {} slots reserve {} of {} connections; inspect the owned lease records in {}",
                    budget.slots(),
                    budget.slots() * budget.per_fixture,
                    budget.connections,
                    directory.display()
                )));
            }
            tokio::time::sleep(ADMISSION_POLL).await;
        };
        let id = orch_types::new_id();
        let run_id =
            std::env::var(RUN_ID_VARIABLE).unwrap_or_else(|_| orch_types::new_id().to_string());
        orch_types::Id::parse_str(&run_id).map_err(io::Error::other)?;
        let lease = Self {
            _slot: file,
            record_path: directory.join("records").join(format!("{id}.json")),
            record: LeaseRecord {
                database: format!("orch_test_{}", id.simple()),
                run_id,
                pid: std::process::id(),
                server_host: admin.get_host().to_owned(),
                server_port: admin.get_port(),
                slot,
                budget,
                state: "preparing",
                error: None,
            },
            admin,
        };
        lease.write_record()?;
        Ok(lease)
    }

    fn write_record(&self) -> io::Result<()> {
        write_json(&self.record_path, &self.record)
    }

    async fn cleanup(&mut self) -> Result<(), CleanupError> {
        let result = tokio::time::timeout(CLEANUP_WAIT, async {
            let mut admin = PgConnection::connect_with(&self.admin).await?;
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.record.database
            )))
            .execute(&mut admin)
            .await?;
            admin.close().await
        })
        .await
        .map_err(|_| CleanupError::Timeout)
        .and_then(|result| result.map_err(CleanupError::from));
        self.record.state = if result.is_ok() {
            "dropped"
        } else {
            "cleanup_failed"
        };
        self.record.error = result.as_ref().err().map(ToString::to_string);
        self.write_record()?;
        result
    }
}

fn migrations_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../migrations")
}

#[expect(
    clippy::expect_used,
    reason = "a fixture: a test without its database has failed, and says why"
)]
impl TestDb {
    /// A fresh database. Panics, with the reason, when the server cannot
    /// be reached: a test without its database has failed.
    pub async fn fresh() -> Self {
        let admin_url = std::env::var(URL_VARIABLE).unwrap_or_else(|_| DEFAULT_URL.to_owned());
        // Identify this process's administrative connections so admission can
        // be observed without listing unrelated test databases or connections.
        let options = PgConnectOptions::from_str(&admin_url)
            .expect("the test database URL")
            .application_name(&format!("orch_testkit_{}", std::process::id()));
        let lease = Lease::acquire(options.clone())
            .await
            .expect("a test database lease");
        let name = lease.record.database.clone();
        let budget = lease.record.budget;
        let url = with_database(&admin_url, &name);
        // The guard exists before CREATE DATABASE: cancellation or a pool failure
        // after creation still drops exactly this database and records the result.
        let mut db = Self {
            pool: PgPoolOptions::new()
                .max_connections(POOL_SIZE)
                .connect_lazy(&url)
                .expect("the test database pool"),
            url,
            lease: Some(lease),
        };
        let mut admin = PgConnection::connect_with(&options)
            .await
            .unwrap_or_else(|error| {
                panic!("the test database server at {URL_VARIABLE} cannot be reached: {error}")
            });
        let (_, usable) = server_capacity(&mut admin)
            .await
            .expect("the test server's connection capacity");
        assert!(
            i64::from(budget.connections) <= i64::from(usable),
            "the configured test connection budget {} exceeds the server's usable capacity {usable}",
            budget.connections
        );
        let template = template_name();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(TEMPLATE_LOCK)
            .execute(&mut admin)
            .await
            .expect("lock");
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
                .bind(&template)
                .fetch_one(&mut admin)
                .await
                .expect("look up the template");
        if !exists {
            let building = format!("{template}_building");
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS {building}"
            )))
            .execute(&mut admin)
            .await
            .expect("clear a half-made template");
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {building}")))
                .execute(&mut admin)
                .await
                .expect("create the template");
            let mut conn = PgConnection::connect(&with_database(&admin_url, &building))
                .await
                .expect("connect to the template");
            let migrator = sqlx::migrate::Migrator::new(migrations_dir())
                .await
                .expect("read the migrations");
            migrator.run(&mut conn).await.expect("apply the migrations");
            conn.close().await.expect("close the template");
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "ALTER DATABASE {building} RENAME TO {template}"
            )))
            .execute(&mut admin)
            .await
            .expect("name the template");
        }
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(TEMPLATE_LOCK)
            .execute(&mut admin)
            .await
            .expect("unlock");

        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {name} TEMPLATE {template}"
        )))
        .execute(&mut admin)
        .await
        .expect("copy the template");
        // Database defaults also reach secondary pools opened through db.url.
        // Set them before the lazy fixture pool opens its first connection.
        for (setting, value) in [
            ("statement_timeout", STATEMENT_TIMEOUT),
            (
                "client_connection_check_interval",
                CLIENT_CONNECTION_CHECK_INTERVAL,
            ),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "ALTER DATABASE {name} SET {setting} = '{value}'"
            )))
            .execute(&mut admin)
            .await
            .expect("bound statements and abandoned clients in the test database");
        }
        admin.close().await.expect("close the setup connection");
        drop(
            db.pool
                .acquire()
                .await
                .expect("connect to the test database"),
        );
        // Pending additive schema belongs to this fresh copy, never the shared migration template.
        // Integration workers use this while the schema owner generates the actual migration.
        if let Some(path) = std::env::var_os("ORCH_TEST_DATABASE_EXTRA_SCHEMA") {
            let ddl = std::fs::read_to_string(path).expect("read the pending test schema");
            // Only the test operator supplies this DDL file; no product input reaches it.
            sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
                .execute(&db.pool)
                .await
                .expect("apply the pending schema to the owned test database");
        }
        if let Some(lease) = db.lease.as_mut() {
            lease.record.state = "ready";
            lease
                .write_record()
                .expect("record the owned test database");
        }
        db
    }

    /// Closes the fixture's pool and removes its exact database, returning and
    /// recording any failure instead of leaving cleanup to the destructor.
    pub async fn close(mut self) -> Result<(), CleanupError> {
        self.pool.close().await;
        if let Some(lease) = self.lease.as_mut() {
            lease.cleanup().await?;
        }
        self.lease.take();
        Ok(())
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        // Dropping the database needs a connection and a runtime of its own,
        // so it runs on a thread, which is waited for: the test's process may
        // end right after. The pool is not closed first: closing waits for
        // connections that the test's own runtime, which this call is
        // holding up, would have to give back. The database is dropped with
        // its connections instead.
        let Some(mut lease) = self.lease.take() else {
            return;
        };
        let cleanup = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    lease.record.state = "cleanup_failed";
                    lease.record.error = Some(error.to_string());
                    let recorded = lease.write_record();
                    eprintln!(
                        "test database {} cleanup runtime failed: {error}; record: {recorded:?}",
                        lease.record.database
                    );
                    return;
                }
            };
            if let Err(error) = runtime.block_on(lease.cleanup()) {
                eprintln!(
                    "test database {} cleanup failed: {error}",
                    lease.record.database
                );
            }
        });
        if cleanup.join().is_err() {
            eprintln!("test database cleanup thread panicked; inspect the owned lease record");
        }
    }
}

fn with_database(url: &str, database: &str) -> String {
    let (base, query) = url
        .split_once('?')
        .map_or((url, None), |(base, query)| (base, Some(query)));
    let server = base.rsplit_once('/').map_or(base, |(server, _)| server);
    query.map_or_else(
        || format!("{server}/{database}"),
        |query| format!("{server}/{database}?{query}"),
    )
}

/// A name for the template that changes whenever a migration does, so
/// that a changed schema is never mistaken for the old one.
fn template_name() -> String {
    let mut files: Vec<PathBuf> = std::fs::read_dir(migrations_dir())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    files.retain(|path| path.extension().is_some_and(|extension| extension == "sql"));
    files.sort();
    let mut hasher = orch_types::Hash::hasher();
    for file in files {
        hasher.update(
            file.file_name()
                .map(|name| name.as_encoded_bytes())
                .unwrap_or_default(),
        );
        hasher.update(&std::fs::read(&file).unwrap_or_default());
    }
    let digest = orch_types::Hash::finish(&hasher).to_string();
    format!("orch_template_{}", &digest[..16])
}
