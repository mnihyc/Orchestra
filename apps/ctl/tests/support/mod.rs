//! A deployment on a database of its own, in a directory of its own, and the built program run against it as a person runs it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

pub mod smtp;

use orch_api_core::registry::Registry;
use orch_api_core::testing::TestApp;
use orch_db::Db;
use orch_db::queries::identity::{self, NewUser, testing as rows};
use orch_testkit::clock;
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};
use sqlx::PgPool;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::net::TcpListener;
use tokio::process::Command;

/// The master key of these deployments: 32 bytes written in hexadecimal.
pub const MASTER_KEY_HEX: &str = "0707070707070707070707070707070707070707070707070707070707070707";

/// What the program did.
#[derive(Debug)]
pub struct Run {
    /// The status it left; -1 where a signal ended it.
    pub code: i32,
    /// What it wrote on standard output.
    pub out: String,
    /// What it wrote on standard error.
    pub err: String,
}

impl Run {
    /// Asserts that it succeeded, and returns what it printed.
    pub fn succeeded(&self) -> &str {
        assert_eq!(self.code, 0, "it failed: {}", self.err);
        &self.out
    }

    /// Asserts that it refused as a person is refused: a non-zero status, one
    /// line that begins `orchctl:` and contains `words`, and no text of a
    /// panic.
    pub fn refused_with(&self, words: &str) -> &str {
        assert_ne!(
            self.code, 0,
            "it was expected to refuse, and printed: {}",
            self.out
        );
        assert!(
            !self.err.contains("panicked") && !self.err.contains("RUST_BACKTRACE"),
            "a refusal is no panic: {}",
            self.err
        );
        let lines: Vec<&str> = self.err.lines().collect();
        assert_eq!(lines.len(), 1, "one line says what to do: {:?}", self.err);
        assert!(lines[0].starts_with("orchctl: "), "{}", lines[0]);
        assert!(
            lines[0].contains(words),
            "{:?} should say {words:?}",
            lines[0]
        );
        lines[0]
    }
}

/// A deployment: a database with the schema and nothing else, and the files a
/// host command reads.
pub struct Deployment {
    /// The database, gone when this is dropped.
    pub test: TestDb,
    files: tempfile::TempDir,
}

impl Deployment {
    /// A deployment whose system is called `Test Orch` and is reached at
    /// `https://orch.test`.
    pub async fn new() -> Self {
        let test = TestDb::fresh().await;
        let files = tempfile::tempdir().unwrap();
        std::fs::write(
            files.path().join("orch.toml"),
            "[system]\nname = \"Test Orch\"\npublic_url = \"https://orch.test\"\n",
        )
        .unwrap();
        let secrets = files.path().join("secrets");
        std::fs::create_dir(&secrets).unwrap();
        std::fs::write(
            secrets.join("orch_master_key"),
            format!("{MASTER_KEY_HEX}\n"),
        )
        .unwrap();
        Self { test, files }
    }

    /// The directory the deployment's files are in.
    pub fn dir(&self) -> PathBuf {
        self.files.path().to_owned()
    }

    /// The configuration the program is told to read.
    pub fn config(&self) -> PathBuf {
        self.files.path().join("orch.toml")
    }

    /// The directory of the secrets the program is told to read.
    pub fn secrets_dir(&self) -> PathBuf {
        self.files.path().join("secrets")
    }

    /// The database, as the libraries take it.
    pub fn db(&self) -> Db {
        Db::from_pool(self.test.pool.clone())
    }

    /// The pool, to look at rows.
    pub fn pool(&self) -> &PgPool {
        &self.test.pool
    }

    /// The program, with nothing of this machine's environment but what a
    /// deployment gives it.
    pub fn command(&self) -> Command {
        self.command_of(Path::new(env!("CARGO_BIN_EXE_orchctl")))
    }

    /// The program as a release has it, `bin/orchctl` under a directory of
    /// this deployment's own, so that what happens to lie beside the build
    /// (a checkout's files) does not lie beside the program.
    pub fn installed(&self) -> Command {
        let bin = self.dir().join("release").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let program = bin.join("orchctl");
        if !program.exists() {
            std::fs::copy(env!("CARGO_BIN_EXE_orchctl"), &program).unwrap();
        }
        self.command_of(&program)
    }

    fn command_of(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("ORCH_CONFIG", self.config())
            .env("ORCH_SECRETS_DIR", self.secrets_dir())
            .env("ORCH_DATABASE_URL", &self.test.url)
            .env("RUST_BACKTRACE", "0")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    /// Runs the program with `args` and waits for it.
    pub async fn run(&self, args: &[&str]) -> Run {
        run(self.command(), args).await
    }
}

/// Runs `command` with `args` and collects what it did.
pub async fn run(mut command: Command, args: &[&str]) -> Run {
    let done = command.args(args).output().await.unwrap();
    Run {
        code: done.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&done.stdout).into_owned(),
        err: String::from_utf8_lossy(&done.stderr).into_owned(),
    }
}

/// Makes a person with the org that is theirs, with a confirmed address.
pub async fn person(db: &Db, username: &str) -> Id {
    let id = new_id();
    let mut tx = db.begin().await.unwrap();
    identity::create_user(
        &mut tx,
        &NewUser {
            id,
            username,
            display_name: username,
            email: Some(&format!("{username}@example.org")),
            email_confirmed: true,
            password_hash: None,
            locale: "en",
            is_system_admin: false,
            created_by: None,
            personal_org: Some(identity::PersonalOrg {
                closed: true,
                flat: true,
            }),
        },
        clock::start(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    id
}

/// Makes a person who administers the system.
pub async fn admin(db: &Db, username: &str) -> Id {
    let id = person(db, username).await;
    let mut conn = db.acquire().await.unwrap();
    rows::make_admin(&mut conn, id).await.unwrap();
    id
}

/// The API on a loopback port over a database, as `orch-api` serves it
/// without TLS; gone when this is dropped.
pub struct Api {
    /// Its address, as `ORCH_URL` holds it.
    pub url: String,
    /// The application, to call it in memory as well.
    pub app: TestApp,
    task: tokio::task::JoinHandle<()>,
}

impl Api {
    /// Serves the API over `db`.
    pub async fn serve(db: &Db) -> Self {
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let app = TestApp::new(db.clone(), clock::manual(), registry);
        let mut served = Registry::new();
        orch_api_core::routes::mount(&mut served);
        let router = orch_api_core::server::router(app.app.clone(), served);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let _ =
                orch_api_core::server::serve_plain(listener, router, std::future::pending()).await;
        });
        Self {
            url: format!("http://{address}"),
            app,
            task,
        }
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// An API token of `user`, which never expires; the token as it is given in
/// `ORCH_TOKEN`.
pub async fn api_token(db: &Db, user: Id) -> String {
    let token = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::API_TOKEN_PREFIX,
    );
    let hash = orch_api_core::middleware::authentication::token_hash(&token);
    let mut conn = db.acquire().await.unwrap();
    rows::insert_api_token(&mut conn, user, &hash)
        .await
        .unwrap();
    token
}
