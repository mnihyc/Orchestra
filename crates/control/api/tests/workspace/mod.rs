//! A world for the tests of an environment's workspace over HTTP: people with different rights on an environment and on the thread that runs in it, an environment on a machine that a test plays, with a disk of files in memory, and the routes of the internal listener that machines upload to and download from.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::{Who, join, make_org, make_project, make_topic, person};
use crate::support::World;
use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::extract::Extension;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http::{HeaderMap, Method, Request, Response, StatusCode};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileMatch, FileStat};
use orch_api_core::server::{self, Peer};
use orch_api_core::state::App;
use orch_api_core::testing::{Machine, Played};
use orch_db::queries::access::{NewEntry, PageKey, insert_entry};
use orch_db::queries::link as command_rows;
use orch_db::queries::resources::testing::insert_machine;
use orch_link_contract::commands::{
    Cmd, CommandError, CommandKind, EnvAttach, ErrorCode, ExpectedHash, FsArchiveList,
    FsArchiveRead, FsCopy, FsFetch, FsFetchResult, FsList, FsMkdir, FsRemove, FsRename, FsSearch,
    FsSnapshot, FsSnapshotResult, FsStat, FsWrite, GitDiff, ProcList, ProcSignal, Signal,
};
use orch_pki::certificates::ServiceName;
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tower::ServiceExt;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The model the tests' agent runs.
pub const MODEL: &str = "fake-1";

/// How long a test waits for something that has to happen.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// Waits until `check` holds, looking again every few milliseconds.
pub async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let started = std::time::Instant::now();
    loop {
        if check().await {
            return;
        }
        assert!(started.elapsed() < PATIENCE, "gave up waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------- the routes machines upload to

/// What the internal listener answered.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Answer {
    pub fn json(&self) -> Json {
        serde_json::from_slice(&self.body).unwrap_or(Json::Null)
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }

    /// How many bytes the answer says have arrived, from its headers.
    pub fn received(&self) -> Option<i64> {
        self.header("x-orch-received")?.parse().ok()
    }

    pub fn complete(&self) -> Option<bool> {
        Some(self.header("x-orch-complete")? == "1")
    }
}

/// The routes of the internal listener that serve content, as a gateway reaches them, over the
/// application in memory.
#[derive(Clone)]
pub struct Content {
    router: axum::Router,
}

impl Content {
    /// The routes as a gateway reaches them.
    pub fn over(app: &App) -> Self {
        Self::as_service(app, Some(ServiceName::Gateway))
    }

    /// The routes as reached by a connection that showed the certificate of `service`, or none.
    pub fn as_service(app: &App, service: Option<ServiceName>) -> Self {
        let router = server::internal_router(Arc::clone(app), orch_api_core::content::routes())
            .layer(Extension(Peer {
                service,
                certificate: None,
                remote: "127.0.0.1:41001".parse().unwrap(),
            }));
        Self { router }
    }

    /// A request, as the gateway hands it over for `machine` under `command`.
    pub fn request(
        &self,
        method: Method,
        path: &str,
        machine: Option<Id>,
        command: Option<Id>,
        extra: &[(&str, String)],
        body: Bytes,
    ) -> Request<Body> {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(machine) = machine {
            request = request.header("x-orch-machine", machine.to_string());
        }
        if let Some(command) = command {
            request = request.header("x-orch-command", command.to_string());
        }
        for (name, value) in extra {
            request = request.header(*name, value);
        }
        request.body(Body::from(body)).unwrap()
    }

    /// Sends a request and gives the answer whole.
    pub async fn send(&self, request: Request<Body>) -> Answer {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status,
            headers: parts.headers,
            body: to_bytes(body, usize::MAX).await.unwrap_or_default(),
        }
    }

    /// Sends a request and gives the answer as it is, its body not yet read.
    pub async fn send_open(&self, request: Request<Body>) -> Response<Body> {
        self.router.clone().oneshot(request).await.unwrap()
    }

    pub async fn head(&self, machine: Id, command: Id, hash: &Hash) -> Answer {
        let request = self.request(
            Method::HEAD,
            &format!("/internal/link/blobs/{hash}"),
            Some(machine),
            Some(command),
            &[],
            Bytes::new(),
        );
        self.send(request).await
    }

    pub async fn put(
        &self,
        machine: Id,
        command: Id,
        hash: &Hash,
        offset: i64,
        size: i64,
        piece: &[u8],
    ) -> Answer {
        let request = self.request(
            Method::PUT,
            &format!("/internal/link/blobs/{hash}?offset={offset}&size={size}"),
            Some(machine),
            Some(command),
            &[],
            Bytes::copy_from_slice(piece),
        );
        self.send(request).await
    }

    pub async fn get(&self, machine: Id, command: Id, hash: &Hash, range: Option<&str>) -> Answer {
        let extra: Vec<(&str, String)> = range
            .map(|range| ("range", range.to_owned()))
            .into_iter()
            .collect();
        let request = self.request(
            Method::GET,
            &format!("/internal/link/blobs/{hash}"),
            Some(machine),
            Some(command),
            &extra,
            Bytes::new(),
        );
        self.send(request).await
    }

    pub async fn missing(&self, machine: Id, command: Id, hashes: &[Hash]) -> Answer {
        let request = self.request(
            Method::POST,
            "/internal/link/blobs/missing",
            Some(machine),
            Some(command),
            &[],
            Bytes::from(serde_json::to_vec(&json!({ "hashes": hashes })).unwrap()),
        );
        self.send(request).await
    }

    /// Uploads `bytes` whole, in pieces of `piece` bytes, as a machine does for a command.
    pub async fn upload(&self, machine: Id, command: Id, bytes: &[u8], piece: usize) -> Hash {
        let hash = Hash::of(bytes);
        let size = i64::try_from(bytes.len()).unwrap();
        let mut offset = 0_usize;
        loop {
            let end = (offset + piece).min(bytes.len());
            let answer = self
                .put(
                    machine,
                    command,
                    &hash,
                    i64::try_from(offset).unwrap(),
                    size,
                    &bytes[offset..end],
                )
                .await;
            assert_eq!(answer.status, StatusCode::OK, "{answer:?}");
            offset = end;
            if offset >= bytes.len() {
                return hash;
            }
        }
    }
}

// ---------------------------------------------------------------- commands in the database

/// A command for `machine`, as the system would have stored it: the identifier it is named by.
pub async fn issue(app: &App, machine: Id, kind: CommandKind, body: &Json) -> Id {
    let mut conn = app.db.acquire().await.unwrap();
    let id = new_id();
    command_rows::insert_command(
        &mut conn,
        &command_rows::NewCommand {
            id,
            machine_id: machine,
            kind: kind.as_str(),
            body,
            serial_key: None,
            deadline_at: app.clock.now() + chrono::Duration::hours(1),
        },
        app.clock.now(),
    )
    .await
    .unwrap();
    id
}

/// Ends a command, as the machine's answer would have.
pub async fn finish(app: &App, command: Id) {
    let mut conn = app.db.acquire().await.unwrap();
    command_rows::finish_command(
        &mut conn,
        command,
        &command_rows::Ending::Succeeded(&json!({})),
        app.clock.now(),
    )
    .await
    .unwrap();
}

// ---------------------------------------------------------------- a bare world with machines

/// The application with a machine that is active and has the role of an environment, and nothing
/// else: for the tests of what moves content.
pub struct Depot {
    pub world: World,
    pub machine: Id,
    pub content: Content,
}

impl Depot {
    pub async fn new() -> Self {
        let world = World::new().await;
        let machine = Self::add_machine(&world, &["environment"]).await;
        let content = Content::over(&world.app.app);
        Self {
            world,
            machine,
            content,
        }
    }

    /// Another machine, active, with the roles given.
    pub async fn add_machine(world: &World, roles: &[&str]) -> Id {
        let mut conn = world.conn().await;
        let id = insert_machine(&mut conn, &format!("machine-{}", new_id()), "connected")
            .await
            .unwrap();
        let roles: Vec<String> = roles.iter().map(|role| (*role).to_owned()).collect();
        sqlx::query("UPDATE machines SET roles = $2, online = true WHERE id = $1")
            .bind(id)
            .bind(&roles)
            .execute(&mut *conn)
            .await
            .unwrap();
        id
    }

    pub fn app(&self) -> &App {
        &self.world.app.app
    }

    /// A command that is open for `machine`.
    pub async fn command(&self, machine: Id, kind: CommandKind, body: &Json) -> Id {
        issue(self.app(), machine, kind, body).await
    }

    /// A command of a kind that uploads, open for the depot's machine.
    pub async fn uploading(&self) -> Id {
        self.command(
            self.machine,
            CommandKind::FsFetch,
            &json!({ "environment_id": new_id(), "path": "a.txt" }),
        )
        .await
    }

    /// Puts content in the store and records it, as an upload that was complete would have.
    pub async fn store(&self, bytes: &[u8]) -> Hash {
        let meta = self
            .app()
            .blobs
            .put_bytes(Bytes::copy_from_slice(bytes))
            .await
            .unwrap();
        let mut conn = self.world.conn().await;
        self.app()
            .blobs
            .register(&mut conn, meta, self.app().clock.now())
            .await
            .unwrap();
        meta.hash
    }
}

// ---------------------------------------------------------------- the disk of a machine

/// A file on the disk.
#[derive(Debug, Clone)]
pub struct Stored {
    pub bytes: Vec<u8>,
    pub mtime_ns: i64,
    pub mode: u32,
}

/// What an environment host would do on a machine, with its files in memory: it lists, describes,
/// uploads, writes, renames, copies and removes, takes snapshots, and answers what is left with
/// canned words. It moves content through the routes of the internal listener, as a machine does.
pub struct Disk {
    pub machine: Id,
    pub environment: Id,
    content: Content,
    files: Mutex<BTreeMap<String, Stored>>,
    dirs: Mutex<BTreeSet<String>>,
    clock: AtomicI64,
    refusals: Mutex<HashMap<CommandKind, VecDeque<CommandError>>>,
    attached: Mutex<Vec<EnvAttach>>,
    signalled: Mutex<Vec<(i64, Signal)>>,
    /// Files that cannot be read, which a snapshot lists without content.
    unreadable: Mutex<BTreeSet<String>>,
    /// The processes it lists.
    pub processes: Mutex<Vec<Json>>,
    /// How long a fetch takes before it answers.
    pub fetch_delay: Mutex<Duration>,
    /// The difference of the repository, and whether more was cut; the canned one when none is set.
    pub diff: Mutex<Option<(String, bool)>>,
    /// While one is set, a snapshot waits for it to be let go before it uploads anything.
    pub gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    cancelled: Mutex<BTreeSet<Id>>,
}

/// The first modification time on a disk: a moment of October 2026, in nanoseconds.
const EPOCH_NS: i64 = 1_791_484_800_000_000_000;

/// Whether `path`, from the workspace root, is taken by `pattern` as the selection grammar says:
/// `*` and `?` match within a component, a whole `**` component any number of directories, and
/// whatever is under a matching directory is taken with it.
fn selected(pattern: &str, path: &str) -> bool {
    fn within(pattern: &[u8], name: &[u8]) -> bool {
        match (pattern.split_first(), name.split_first()) {
            (None, None) => true,
            (Some((&b'*', rest)), _) => {
                within(rest, name)
                    || name
                        .split_first()
                        .is_some_and(|(_, after)| within(pattern, after))
            }
            (Some((&b'?', rest)), Some((_, after))) => within(rest, after),
            (Some((wanted, rest)), Some((found, after))) => wanted == found && within(rest, after),
            _ => false,
        }
    }
    fn from(pattern: &[&str], path: &[&str]) -> bool {
        match (pattern.split_first(), path.split_first()) {
            (None, _) => true,
            (Some((&"**", rest)), _) => (0..=path.len()).any(|skip| from(rest, &path[skip..])),
            (Some((wanted, rest)), Some((found, after))) => {
                within(wanted.as_bytes(), found.as_bytes()) && from(rest, after)
            }
            (Some(_), None) => false,
        }
    }
    let pattern: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    let path: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    from(&pattern, &path)
}

/// How many bytes the disk moves in one request, in either direction: small, so that a file of a few
/// hundred kilobytes is several pieces, as a file of many megabytes is for a connector.
pub const PIECE: usize = 64 * 1024;

impl Disk {
    /// Whether the simulated machine has received a stop for this exact command.
    pub fn saw_stop(&self, command: Id) -> bool {
        lock(&self.cancelled).contains(&command)
    }

    pub fn new(machine: Id, environment: Id, content: Content) -> Arc<Self> {
        Arc::new(Self {
            machine,
            environment,
            cancelled: Mutex::new(BTreeSet::new()),
            content,
            files: Mutex::new(BTreeMap::new()),
            dirs: Mutex::new(BTreeSet::new()),
            clock: AtomicI64::new(EPOCH_NS),
            refusals: Mutex::new(HashMap::new()),
            attached: Mutex::new(Vec::new()),
            signalled: Mutex::new(Vec::new()),
            unreadable: Mutex::new(BTreeSet::new()),
            processes: Mutex::new(Vec::new()),
            fetch_delay: Mutex::new(Duration::ZERO),
            diff: Mutex::new(None),
            gate: Mutex::new(None),
        })
    }

    fn tick(&self) -> i64 {
        self.clock.fetch_add(1_234_567, Ordering::SeqCst)
    }

    /// Puts a file on the disk, as somebody working there would have.
    pub fn put(&self, path: &str, bytes: &[u8]) {
        let mtime_ns = self.tick();
        let mut dirs = lock(&self.dirs);
        let mut at = String::new();
        for part in path
            .split('/')
            .take(path.split('/').count().saturating_sub(1))
        {
            if !at.is_empty() {
                at.push('/');
            }
            at.push_str(part);
            dirs.insert(at.clone());
        }
        lock(&self.files).insert(
            path.to_owned(),
            Stored {
                bytes: bytes.to_vec(),
                mtime_ns,
                mode: 0o644,
            },
        );
    }

    /// Changes a file that is there.
    pub fn change(&self, path: &str, bytes: &[u8]) {
        self.put(path, bytes);
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        lock(&self.files).get(path).map(|file| file.bytes.clone())
    }

    pub fn exists(&self, path: &str) -> bool {
        lock(&self.files).contains_key(path) || lock(&self.dirs).contains(path)
    }

    pub fn paths(&self) -> Vec<String> {
        lock(&self.files).keys().cloned().collect()
    }

    pub fn make_unreadable(&self, path: &str) {
        lock(&self.unreadable).insert(path.to_owned());
    }

    /// The machine refuses the next command of a kind.
    pub fn refuse_next(&self, kind: CommandKind, error: CommandError) {
        lock(&self.refusals)
            .entry(kind)
            .or_default()
            .push_back(error);
    }

    /// What it was told when its environment was attached, each time.
    pub fn attaches(&self) -> Vec<EnvAttach> {
        lock(&self.attached).clone()
    }

    pub fn signals(&self) -> Vec<(i64, Signal)> {
        lock(&self.signalled).clone()
    }

    fn entry_of(&self, path: &str) -> Option<DirEntry> {
        if let Some(file) = lock(&self.files).get(path) {
            return Some(DirEntry {
                name: path.rsplit('/').next().unwrap_or(path).to_owned(),
                path: path.to_owned(),
                kind: DirEntryKind::File,
                size: i64::try_from(file.bytes.len()).unwrap(),
                mtime: Time::new(DateTime::<Utc>::from_timestamp_nanos(file.mtime_ns)),
                mode: file.mode,
                link_target: None,
                cached: false,
            });
        }
        if path.is_empty() || lock(&self.dirs).contains(path) {
            return Some(DirEntry {
                name: path.rsplit('/').next().unwrap_or(path).to_owned(),
                path: path.to_owned(),
                kind: DirEntryKind::Dir,
                size: 0,
                mtime: Time::new(DateTime::<Utc>::from_timestamp_nanos(EPOCH_NS)),
                mode: 0o755,
                link_target: None,
                cached: false,
            });
        }
        None
    }

    fn stat_of(&self, path: &str) -> Option<FileStat> {
        let entry = self.entry_of(path)?;
        let hash = lock(&self.files)
            .get(path)
            .map(|file| Hash::of(&file.bytes));
        let is_file = entry.kind == DirEntryKind::File;
        let media = orch_api_core::media::media_type(&entry.name, &[]).to_owned();
        let is_text = is_file && orch_api_core::media::is_text_by_name(&entry.name);
        Some(FileStat {
            entry,
            hash,
            media_type: if is_file {
                media
            } else {
                orch_api_core::media::DIRECTORY.to_owned()
            },
            is_text,
        })
    }

    fn children(&self, directory: &str) -> Option<Vec<DirEntry>> {
        self.entry_of(directory)?;
        let prefix = if directory.is_empty() {
            String::new()
        } else {
            format!("{directory}/")
        };
        let mut names: BTreeSet<String> = BTreeSet::new();
        for path in lock(&self.files).keys().chain(lock(&self.dirs).iter()) {
            if let Some(rest) = path.strip_prefix(&prefix)
                && !rest.is_empty()
            {
                names.insert(format!(
                    "{prefix}{}",
                    rest.split('/').next().unwrap_or(rest)
                ));
            }
        }
        let mut entries: Vec<DirEntry> = names
            .iter()
            .filter_map(|path| self.entry_of(path))
            .collect();
        entries.sort_by(|a, b| {
            (b.kind == DirEntryKind::Dir)
                .cmp(&(a.kind == DirEntryKind::Dir))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Some(entries)
    }

    fn not_found(what: &str) -> CommandError {
        CommandError::new(ErrorCode::NotFound, format!("{what} is not there"))
    }

    fn json<T: serde::Serialize>(value: &T) -> Result<Json, CommandError> {
        serde_json::to_value(value)
            .map_err(|e| CommandError::new(ErrorCode::Internal, e.to_string()))
    }

    fn facts(&self) -> Json {
        json!({
            "os": "linux", "arch": "x86_64", "shell": { "name": "sh", "path": "/bin/sh" },
            "user": "agent", "home": "/home/agent", "workspace_root": "/workspace",
            "path_style": "posix", "case_sensitive": true, "executors": [], "mcp_found": [],
            "index": "full", "mounts": [], "private_views": false, "suspend": true, "git": true,
            "generation": 1
        })
    }

    async fn fetch(&self, id: Id, body: FsFetch) -> Result<Json, CommandError> {
        let delay = *lock(&self.fetch_delay);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let file = lock(&self.files)
            .get(&body.path)
            .cloned()
            .ok_or_else(|| Self::not_found(&body.path))?;
        let hash = self
            .content
            .upload(self.machine, id, &file.bytes, PIECE)
            .await;
        Self::json(&FsFetchResult {
            hash,
            size: i64::try_from(file.bytes.len()).unwrap(),
            mtime_ns: file.mtime_ns,
        })
    }

    async fn write(&self, id: Id, body: FsWrite) -> Result<Json, CommandError> {
        let present = lock(&self.files)
            .get(&body.path)
            .map(|file| Hash::of(&file.bytes));
        let stale = match (body.expected_hash, present) {
            (Some(ExpectedHash::Absent), Some(_)) => true,
            (Some(ExpectedHash::Is(expected)), found) => found != Some(expected),
            _ => false,
        };
        if stale {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the file is not the version that was expected",
            )
            .with_details(json!({"current": self.stat_of(&body.path)})));
        }
        let mut bytes = Vec::new();
        let mut total: Option<usize> = None;
        while total.is_none_or(|total| bytes.len() < total) {
            let offset = bytes.len();
            let range = format!("bytes={offset}-{}", offset + PIECE - 1);
            let answer = self
                .content
                .get(self.machine, id, &body.blob, Some(&range))
                .await;
            if !answer.status.is_success() {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    format!("the content could not be fetched: {}", answer.status),
                ));
            }
            // A range that holds the whole is answered whole; otherwise the size stands after the slash.
            total = Some(match answer.header("content-range") {
                Some(text) => text
                    .rsplit('/')
                    .next()
                    .and_then(|size| size.parse().ok())
                    .unwrap_or(offset + answer.body.len()),
                None => offset + answer.body.len(),
            });
            bytes.extend_from_slice(&answer.body);
        }
        assert_eq!(
            Hash::of(&bytes),
            body.blob,
            "the machine got the content it was named"
        );
        self.put(&body.path, &bytes);
        Self::json(&self.stat_of(&body.path).unwrap())
    }

    fn matches_in(&self, body: &FsSearch) -> Vec<FileMatch> {
        let needle = if body.case {
            body.query.clone()
        } else {
            body.query.to_lowercase()
        };
        let prefix = if body.path.is_empty() {
            String::new()
        } else {
            format!("{}/", body.path)
        };
        let mut found = Vec::new();
        for (path, file) in lock(&self.files).iter() {
            if !path.starts_with(&prefix) {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&file.bytes) else {
                continue;
            };
            for (number, line) in text.lines().enumerate() {
                let hay = if body.case {
                    line.to_owned()
                } else {
                    line.to_lowercase()
                };
                if let Some(at) = hay.find(&needle) {
                    found.push(FileMatch {
                        path: path.clone(),
                        line: i64::try_from(number + 1).unwrap(),
                        text: line.to_owned(),
                        ranges: vec![(
                            u32::try_from(at).unwrap(),
                            u32::try_from(at + needle.len()).unwrap(),
                        )],
                    });
                }
            }
        }
        found
    }

    async fn snapshot(&self, id: Id, body: FsSnapshot) -> Result<Json, CommandError> {
        use orch_engine::manifest::{Entry, EntryKind, Header, build};
        let gate = lock(&self.gate).clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        if self.saw_stop(id) {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the snapshot was stopped before uploading",
            ));
        }
        // As a machine reads a selection: one literal directory is the root its paths are written
        // from; patterns (an export's `**`) are matched from the workspace root, which is the root.
        let (root, patterns) = match body.selection.include.as_slice() {
            [only] if only != "." && !only.contains(['*', '?']) => (only.clone(), Vec::new()),
            many => (String::new(), many.to_vec()),
        };
        let taken = |path: &str| {
            patterns.is_empty() || patterns.iter().any(|pattern| selected(pattern, path))
        };
        let prefix = if root.is_empty() {
            String::new()
        } else {
            format!("{root}/")
        };
        let mut entries = Vec::new();
        let mut missing = 0_i64;
        let mut uploaded = 0_i64;
        let files: Vec<(String, Stored)> = lock(&self.files)
            .iter()
            .filter(|(path, _)| path.starts_with(&prefix) && taken(path))
            .map(|(path, file)| (path.clone(), file.clone()))
            .collect();
        for (path, file) in files {
            if body
                .selection
                .exclude
                .iter()
                .any(|skip| path == *skip || path.starts_with(&format!("{skip}/")))
            {
                continue;
            }
            let relative = path[prefix.len()..].to_owned();
            let unreadable = lock(&self.unreadable).contains(&path);
            let hash = if unreadable {
                missing += 1;
                None
            } else {
                let hash = self
                    .content
                    .upload(self.machine, id, &file.bytes, PIECE)
                    .await;
                uploaded += i64::try_from(file.bytes.len()).unwrap();
                Some(hash)
            };
            entries.push(Entry {
                path: relative,
                kind: EntryKind::File,
                size: i64::try_from(file.bytes.len()).unwrap(),
                mtime_ns: file.mtime_ns,
                mode: file.mode,
                hash,
                link: None,
            });
        }
        let dirs: Vec<String> = lock(&self.dirs)
            .iter()
            .filter(|dir| dir.starts_with(&prefix) && dir.len() > prefix.len() && taken(dir))
            .cloned()
            .collect();
        for dir in dirs {
            if body
                .selection
                .exclude
                .iter()
                .any(|skip| dir == *skip || dir.starts_with(&format!("{skip}/")))
            {
                continue;
            }
            entries.push(Entry {
                path: dir[prefix.len()..].to_owned(),
                kind: EntryKind::Dir,
                size: 0,
                mtime_ns: EPOCH_NS,
                mode: 0o755,
                hash: None,
                link: None,
            });
        }
        let written = build(
            &Header {
                v: 1,
                root: format!("/workspace/{root}"),
                generated_at: Time::new(Utc::now()),
                generation: 1,
                case_sensitive: true,
            },
            entries,
        )
        .unwrap();
        let manifest = self
            .content
            .upload(self.machine, id, &written.bytes, PIECE)
            .await;
        assert_eq!(manifest, written.hash);
        Self::json(&FsSnapshotResult {
            manifest_blob: written.hash,
            file_count: written.file_count,
            total_bytes: written.total_bytes,
            uploaded_bytes: uploaded,
            missing,
        })
    }
}

#[async_trait]
impl Machine for Disk {
    fn cancel(&self, command: Id) {
        lock(&self.cancelled).insert(command);
    }
    async fn run(&self, command: &Cmd) -> Result<Json, CommandError> {
        let (id, body) = (command.id, command.body.clone());
        let kind: CommandKind = command
            .kind
            .parse()
            .map_err(|_| CommandError::new(ErrorCode::Unsupported, "that is no command"))?;
        if let Some(error) = lock(&self.refusals)
            .get_mut(&kind)
            .and_then(VecDeque::pop_front)
        {
            return Err(error);
        }
        let bad = |e: serde_json::Error| CommandError::new(ErrorCode::Internal, e.to_string());
        match kind {
            CommandKind::EnvAttach => {
                let attach: EnvAttach = serde_json::from_value(body).map_err(bad)?;
                lock(&self.attached).push(attach);
                Ok(self.facts())
            }
            CommandKind::WatchSet => Ok(json!({})),
            CommandKind::FsList => {
                let list: FsList = serde_json::from_value(body).map_err(bad)?;
                let entries = self
                    .children(&list.path)
                    .ok_or_else(|| Self::not_found(&list.path))?;
                Ok(json!({ "entries": entries, "truncated": false }))
            }
            CommandKind::FsStat => {
                let stat: FsStat = serde_json::from_value(body).map_err(bad)?;
                let found = self
                    .stat_of(&stat.path)
                    .ok_or_else(|| Self::not_found(&stat.path))?;
                Self::json(&found)
            }
            CommandKind::FsFetch => {
                self.fetch(id, serde_json::from_value(body).map_err(bad)?)
                    .await
            }
            CommandKind::FsWrite => {
                self.write(id, serde_json::from_value(body).map_err(bad)?)
                    .await
            }
            CommandKind::FsMkdir => {
                let mkdir: FsMkdir = serde_json::from_value(body).map_err(bad)?;
                lock(&self.dirs).insert(mkdir.path.clone());
                Self::json(&self.stat_of(&mkdir.path).unwrap())
            }
            CommandKind::FsRename => {
                let rename: FsRename = serde_json::from_value(body).map_err(bad)?;
                if !self.exists(&rename.from) {
                    return Err(Self::not_found(&rename.from));
                }
                if self.exists(&rename.to) {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "the destination is there",
                    ));
                }
                let moved: Vec<(String, Stored)> = lock(&self.files)
                    .iter()
                    .filter(|(path, _)| {
                        **path == rename.from || path.starts_with(&format!("{}/", rename.from))
                    })
                    .map(|(path, file)| (path.clone(), file.clone()))
                    .collect();
                for (path, file) in moved {
                    lock(&self.files).remove(&path);
                    let target = format!("{}{}", rename.to, &path[rename.from.len()..]);
                    lock(&self.files).insert(target, file);
                }
                lock(&self.dirs).remove(&rename.from);
                lock(&self.dirs).insert(rename.to.clone());
                Self::json(
                    &self
                        .stat_of(&rename.to)
                        .unwrap_or_else(|| self.stat_of("").unwrap()),
                )
            }
            CommandKind::FsCopy => {
                let copy: FsCopy = serde_json::from_value(body).map_err(bad)?;
                if !self.exists(&copy.from) {
                    return Err(Self::not_found(&copy.from));
                }
                if self.exists(&copy.to) {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "the destination is there",
                    ));
                }
                let copied: Vec<(String, Vec<u8>)> = lock(&self.files)
                    .iter()
                    .filter(|(path, _)| {
                        **path == copy.from || path.starts_with(&format!("{}/", copy.from))
                    })
                    .map(|(path, file)| (path.clone(), file.bytes.clone()))
                    .collect();
                for (path, bytes) in copied {
                    self.put(&format!("{}{}", copy.to, &path[copy.from.len()..]), &bytes);
                }
                Self::json(
                    &self
                        .stat_of(&copy.to)
                        .unwrap_or_else(|| self.stat_of("").unwrap()),
                )
            }
            CommandKind::FsRemove => {
                let remove: FsRemove = serde_json::from_value(body).map_err(bad)?;
                if !self.exists(&remove.path) {
                    return Err(Self::not_found(&remove.path));
                }
                let below = format!("{}/", remove.path);
                let has_children = lock(&self.files)
                    .keys()
                    .any(|path| path.starts_with(&below));
                if has_children && !remove.recursive {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "the directory is not empty",
                    ));
                }
                lock(&self.files)
                    .retain(|path, _| *path != remove.path && !path.starts_with(&below));
                lock(&self.dirs).retain(|path| *path != remove.path && !path.starts_with(&below));
                Ok(json!({}))
            }
            CommandKind::FsSearch => {
                let search: FsSearch = serde_json::from_value(body).map_err(bad)?;
                let matches = self.matches_in(&search);
                let limit = usize::try_from(search.limit).unwrap();
                let truncated = matches.len() > limit;
                Ok(json!({
                    "matches": matches.into_iter().take(limit).collect::<Vec<_>>(),
                    "truncated": truncated,
                    "searched": lock(&self.files).len()
                }))
            }
            CommandKind::FsArchiveList => {
                let list: FsArchiveList = serde_json::from_value(body).map_err(bad)?;
                if !self.exists(&list.path) {
                    return Err(Self::not_found(&list.path));
                }
                Ok(json!({
                    "format": "zip", "compression": null, "truncated": false, "encrypted": false,
                    "entries": [{ "path": "docs/", "kind": "dir", "size": null, "modified_at": null },
                                { "path": "readme.txt", "kind": "file", "size": 11, "modified_at": null }]
                }))
            }
            CommandKind::FsArchiveRead => {
                let read: FsArchiveRead = serde_json::from_value(body).map_err(bad)?;
                let bytes = format!("inside {}", read.entry).into_bytes();
                let hash = self.content.upload(self.machine, id, &bytes, PIECE).await;
                Ok(json!({ "blob": { "hash": hash, "size": bytes.len() } }))
            }
            CommandKind::FsSnapshot => {
                self.snapshot(id, serde_json::from_value(body).map_err(bad)?)
                    .await
            }
            CommandKind::FsMeasure => {
                let asked: orch_link_contract::commands::FsMeasure =
                    serde_json::from_value(body).map_err(bad)?;
                let selected = orch_engine::selection::Matcher::new(&asked.selection)
                    .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
                let files = lock(&self.files);
                let mut directories: BTreeMap<String, (i64, i64)> = BTreeMap::new();
                let mut count = 0i64;
                let mut bytes = 0i64;
                for (path, file) in files.iter().filter(|(path, _)| selected.matches(path)) {
                    count += 1;
                    let size = i64::try_from(file.bytes.len()).unwrap_or(i64::MAX);
                    bytes = bytes.saturating_add(size);
                    let mut parent = path.rsplit_once('/').map(|(parent, _)| parent);
                    while let Some(path) = parent {
                        let totals = directories.entry(path.to_owned()).or_default();
                        totals.0 += 1;
                        totals.1 = totals.1.saturating_add(size);
                        parent = path.rsplit_once('/').map(|(parent, _)| parent);
                    }
                }
                let entry_count =
                    count.saturating_add(i64::try_from(directories.len()).unwrap_or(i64::MAX));
                let mut largest: Vec<_> = directories
                    .iter()
                    .map(|(path, (file_count, total_bytes))| {
                        orch_link_contract::commands::DirectoryMeasure {
                            path: path.clone(),
                            file_count: *file_count,
                            total_bytes: *total_bytes,
                        }
                    })
                    .collect();
                largest.sort_by(|a, b| {
                    b.total_bytes
                        .cmp(&a.total_bytes)
                        .then_with(|| a.path.cmp(&b.path))
                });
                largest.truncate(10);
                let refusal = (entry_count > 250_000)
                    .then_some(orch_link_contract::commands::SelectionRefusal::EntryLimit);
                Self::json(&orch_link_contract::commands::FsMeasureResult {
                    entry_count,
                    file_count: count,
                    directory_count: i64::try_from(directories.len()).unwrap_or(i64::MAX),
                    total_bytes: bytes,
                    largest_directories: largest,
                    complete: refusal.is_none(),
                    refusal,
                })
            }
            CommandKind::GitStatus => {
                let status: orch_link_contract::commands::GitStatus =
                    serde_json::from_value(body).map_err(bad)?;
                Ok(json!({
                    "available": true, "reason": null,
                    "repository_dir": status.path.as_deref().unwrap_or(""), "status_hash": Hash::of(b"fixture status"),
                    "remotes": [], "upstream": null,
                    "branch": status.path.as_deref().filter(|path| !path.is_empty()).unwrap_or("main"),
                    "head": "abc1234", "ahead": 1, "behind": 0,
                    "changes": [{ "path": "web/a.txt", "index": " ", "worktree": "M" }]
                }))
            }
            CommandKind::GitDiff => {
                let diff: GitDiff = serde_json::from_value(body).map_err(bad)?;
                if let Some((text, truncated)) = lock(&self.diff).clone() {
                    return Ok(json!({ "text": text, "truncated": truncated }));
                }
                Ok(json!({
                    "text": format!("diff --git a/x b/x\n{}", if diff.staged == Some(true) { "staged" } else { "unstaged" }),
                    "truncated": false
                }))
            }
            CommandKind::ProcList => {
                let _: ProcList = serde_json::from_value(body).map_err(bad)?;
                Ok(
                    json!({ "as_of": "2026-10-08T17:04:05.123Z", "processes": lock(&self.processes).clone() }),
                )
            }
            CommandKind::ProcSignal => {
                let signal: ProcSignal = serde_json::from_value(body).map_err(bad)?;
                let known = lock(&self.processes).iter().any(|process| {
                    serde_json::from_value::<orch_api_contract::processes::ProcessInfo>(
                        process.clone(),
                    )
                    .is_ok_and(|each| {
                        i64::from(each.pid) == signal.pid && each.started_at == signal.started_at
                    })
                });
                if !known {
                    return Err(Self::not_found("that process"));
                }
                lock(&self.signalled).push((signal.pid, signal.signal));
                Ok(json!({}))
            }
            other => Err(CommandError::new(
                ErrorCode::Unsupported,
                format!("the disk has no script for {other}"),
            )),
        }
    }
}

// ---------------------------------------------------------------- the site

/// A directory of a machine that the world knows already, which a site's environment is.
pub struct Placed {
    /// The machine.
    pub machine: Id,
    /// The directory, absolute: the root of the environment's workspace.
    pub root: String,
}

/// An environment on a machine that a test plays, with the thread that runs in it and the people who
/// stand in different places around them.
pub struct Site {
    pub world: World,
    pub ada: Who,
    pub vic: Who,
    pub dev: Who,
    pub eve: Who,
    pub org: Id,
    pub topic: Id,
    pub machine: Id,
    pub environment: Id,
    pub agent: Id,
    pub provider: Id,
    pub thread: Id,
    pub content: Content,
    pub disk: Arc<Disk>,
    pub played: Option<Played>,
}

/// What an agent declares of itself.
fn descriptor() -> Json {
    json!({
        "kind": "fake", "adapter_version": 1,
        "harness": { "name": "Fake", "version": "1.0.0", "supported_range": ">=1.0.0", "status": "supported" },
        "arrangements": [
            { "type": "together" },
            { "type": "separated", "executor": "orch-exec", "executor_version": "*", "tool_sets": ["plain"] }
        ],
        "native_provider": { "harness_kind": "fake", "title": "Fake subscription", "arrangements": ["separated"] },
        "forwarded_protocols": ["openai-responses"],
        "approval_modes": ["ask", "on_request", "bypass"],
        "sandbox_modes": ["read_only", "workspace", "open"],
        "capabilities": {
            "steer": true, "interrupt": true, "approvals": ["command", "file_change", "user_input"],
            "input_images": true, "usage_tokens": true, "memory": "none"
        },
        "parameters": []
    })
}

impl Site {
    /// Ada owns an org with a topic, an environment on a machine, and a thread in it, whose
    /// directory is `web`. Vic only reads it, Dev is a developer of the org and writes the files of
    /// threads, and Eve belongs to no org.
    pub async fn new() -> Self {
        Self::with_config(orch_config::ServerConfig::default()).await
    }

    /// As [`Site::new`], on a system configured as `config` says.
    pub async fn with_config(config: orch_config::ServerConfig) -> Self {
        Self::in_world(World::with(config).await, None).await
    }

    /// As [`Site::new`], in a world that was made already. With `on`, the environment is a directory
    /// of that person's machine, which the world knows already; without, it is made from a template
    /// on a machine that the site makes.
    pub async fn in_world(world: World, on: Option<Placed>) -> Self {
        let ada = person(&world, "ada").await;
        let vic = person(&world, "vic").await;
        let dev = person(&world, "dev").await;
        let eve = person(&world, "eve").await;
        let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
        join(&world, org, org, &vic, "viewer", &ada).await;
        join(&world, org, org, &dev, "developer", &ada).await;
        let project = make_project(&world, &ada, org, "Web").await.base.id;
        let topic = make_topic(&world, &ada, project, "Login").await.base.id;
        let (agent, provider, environment, template) = (new_id(), new_id(), new_id(), new_id());
        let mut conn = world.conn().await;
        let machine = match &on {
            Some(placed) => placed.machine,
            None => insert_machine(&mut conn, "workspace-machine", "connected")
                .await
                .unwrap(),
        };
        sqlx::query(
            "UPDATE machines SET roles = ARRAY['agent','environment','provider'], online = true,
             facts = jsonb_set(facts || $2, '{policy}', COALESCE(facts -> 'policy', '{}') || $3)
             WHERE id = $1",
        )
        .bind(machine)
        .bind(json!({
            "adapters": [{ "kind": "fake", "adapter_version": 1, "builtin": true }],
            "harnesses": [{ "kind": "fake", "path": "/test/harness", "version": "1.0.0", "daemon": false }]
        }))
        .bind(json!({ "agent.harnesses": [{ "kind": "fake", "path": "/test/harness" }] }))
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
             VALUES ($1, 'Fake agent', $2, $3, $3, 'fake', '{}', $4)",
        )
        .bind(agent)
        .bind(ada.id())
        .bind(org)
        .bind(descriptor())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                    machine_id, arrangements, status)
             VALUES ($1, 'Fake subscription', $2, $3, $3, 'subscription', 'fake', $4, $5,
                     ARRAY['separated'], 'ready')",
        )
        .bind(provider)
        .bind(ada.id())
        .bind(org)
        .bind(agent)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
             VALUES ($1, $2, $3, 'Fake One', $4, true, 'manual', $5)",
        )
        .bind(new_id())
        .bind(provider)
        .bind(MODEL)
        .bind(json!({ "efforts": ["low", "high"], "default_effort": "low", "input_images": true }))
        .bind(ada.id())
        .execute(&mut *conn)
        .await
        .unwrap();
        if let Some(placed) = &on {
            // A person's machine says that it attaches the directories registered on it, as its
            // connector reports: an attachment stands for the session it was made in.
            let attaches =
                orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
                    "test",
                    orch_driver_contract::descriptor::EnvironmentDataStorage {
                        made_by_orch: None,
                        existing_directory: true,
                        reason: None,
                    },
                );
            sqlx::query("UPDATE machines SET facts = facts || $2 WHERE id = $1")
                .bind(machine)
                .bind(json!({ "environment_descriptor": attaches }))
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root,
                                           machine_id, generation, status, remote_write, terminal)
                 VALUES ($1, 'Directory', $2, $3, 'connected', $4, $5, $6, 1, 'running', true, true)",
            )
            .bind(environment)
            .bind(ada.id())
            .bind(org)
            .bind(topic)
            .bind(&placed.root)
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        } else {
            sqlx::query(
                "INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 'template', 'test', '{}')",
            )
            .bind(template)
            .execute(&mut *conn)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root,
                                           machine_id, template_id, generation, status, remote_write, terminal)
                 VALUES ($1, 'Workspace', $2, $3, 'managed', $4, '/workspace', $5, $6, 1, 'running', true, true)",
            )
            .bind(environment)
            .bind(ada.id())
            .bind(org)
            .bind(topic)
            .bind(machine)
            .bind(template)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        for (kind, id) in [
            ("agent", agent),
            ("provider", provider),
            ("environment", environment),
        ] {
            insert_entry(
                &mut conn,
                &NewEntry {
                    id: new_id(),
                    page: &PageKey::new(kind, id, "live"),
                    profile: "any",
                    permission: "access.own",
                    holder_kind: "user",
                    holder_id: Some(ada.id()),
                    holder_role: None,
                    options: &json!({}),
                    expires_at: None,
                    note: "",
                    source: "creation",
                    created_by: Some(ada.id()),
                    at: world.now(),
                },
            )
            .await
            .unwrap();
        }
        drop(conn);
        let thread = world
            .app
            .call(ThreadCreate, &[&topic])
            .token(&ada.token)
            .body(&CreateThread {
                name: Some("Fix login".to_owned()),
                binding: Some(CreateThreadBinding {
                    agent_id: agent,
                    provider_id: provider,
                    arrangement: None,
                }),
                model: Some(MODEL.to_owned()),
                environment: Some(EnvironmentChoice::Use(environment)),
                directory: Some("web".to_owned()),
                ..CreateThread::default()
            })
            .send()
            .await
            .ok()
            .base
            .id;
        let content = Content::over(&world.app.app);
        let disk = Disk::new(machine, environment, content.clone());
        Self {
            world,
            ada,
            vic,
            dev,
            eve,
            org,
            topic,
            machine,
            environment,
            agent,
            provider,
            thread,
            content,
            disk,
            played: None,
        }
    }

    /// Another thread of the same topic, in the same environment, made by `by`.
    pub async fn another_thread(&self, by: &Who, directory: &str) -> Id {
        self.world
            .app
            .call(ThreadCreate, &[&self.topic])
            .token(&by.token)
            .body(&CreateThread {
                name: Some("Another".to_owned()),
                binding: Some(CreateThreadBinding {
                    agent_id: self.agent,
                    provider_id: self.provider,
                    arrangement: None,
                }),
                model: Some(MODEL.to_owned()),
                environment: Some(EnvironmentChoice::Use(self.environment)),
                directory: Some(directory.to_owned()),
                ..CreateThread::default()
            })
            .send()
            .await
            .ok()
            .base
            .id
    }

    /// An item of a thread that reports the changes of files, as the engine writes one when the
    /// harness's tool call ends.
    pub async fn file_change_item(&self, thread: Id, changes: &[Json]) -> Id {
        let mut conn = self.world.conn().await;
        let seq = orch_db::sequences::next(&mut conn, thread)
            .await
            .unwrap()
            .unwrap();
        let segment: Id =
            sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id = $1")
                .bind(thread)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        let id = new_id();
        sqlx::query(
            "INSERT INTO items (id, thread_id, segment_id, stream, kind, status, payload, seq_created, seq_updated, updated_at)
             VALUES ($1, $2, $3, 'main', 'tool_call', 'completed', $4, $5, $5, $6)",
        )
        .bind(id)
        .bind(thread)
        .bind(segment)
        .bind(json!({
            "tool": { "kind": "file_change", "changes": changes },
            "title": "Edit files", "started_at": "2026-10-08T14:00:00.000Z", "duration_ms": 5,
            "raw": { "name": "Edit", "input": { "inline": "{}", "total_len": 2, "truncated": false, "blob": null }, "output": null, "is_error": false }
        }))
        .bind(seq)
        .bind(self.world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
        id
    }

    /// The machine reports how far a job has come, as it does while it works, and the system takes
    /// the report in.
    pub async fn machine_reports(&self, job: Id, percent: u32, phase: &str) {
        self.machine_reports_phases(
            job,
            percent,
            phase,
            ["done", "running", "pending", "pending", "pending"],
        )
        .await;
    }

    /// The same, with the state of each of the five phases of a snapshot as the machine says it.
    pub async fn machine_reports_phases(
        &self,
        job: Id,
        percent: u32,
        phase: &str,
        states: [&str; 5],
    ) {
        let progress = json!({
            "phase": phase,
            "phases": [
                { "name": "scan", "weight": 5, "state": states[0] },
                { "name": "hash", "weight": 30, "state": states[1] },
                { "name": "upload", "weight": 55, "state": states[2] },
                { "name": "manifest", "weight": 5, "state": states[3] },
                { "name": "finalize", "weight": 5, "state": states[4] }
            ],
            "percent": percent, "files_done": 3, "files_total": 10, "bytes_done": 300,
            "bytes_total": 1000, "rate_bps": null, "eta_seconds": null, "message": "reading"
        });
        let event = orch_link::streams::DeliveredEvent {
            seq: 1,
            at: Time::new(self.world.now()),
            kind: "job.progress".to_owned(),
            body: json!({ "job_id": job, "progress": progress }),
        };
        let mut tx = self.world.app.app.db.begin().await.unwrap();
        self.world
            .app
            .app
            .engine
            .apply_stream_events(
                &mut tx,
                self.machine,
                &orch_link_contract::streams::Stream::Env {
                    environment_id: self.environment,
                },
                &[event],
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// A site whose machine answers.
    pub async fn live() -> Self {
        let mut site = Self::new().await;
        site.power_on();
        site
    }

    /// A site whose environment is a directory of a person's machine, which the engine attaches in
    /// each session of the machine (it is not running here until a test starts it); the machine
    /// answers.
    pub async fn on_a_directory() -> Self {
        let world = World::new().await;
        let machine = {
            let mut conn = world.conn().await;
            insert_machine(&mut conn, "laptop", "connected")
                .await
                .unwrap()
        };
        let mut site = Self::in_world(
            world,
            Some(Placed {
                machine,
                root: "/workspace".to_owned(),
            }),
        )
        .await;
        site.power_on();
        site
    }

    /// The machine starts to answer: from now on it does what its disk does.
    pub fn power_on(&mut self) {
        self.played = Some(self.world.app.play(self.machine, self.disk.clone()));
    }

    /// A machine of another kind answers in its place.
    pub fn power_on_with(&mut self, machine: Arc<dyn Machine>) {
        self.played = Some(self.world.app.play(self.machine, machine));
    }

    /// The machine's session is gone.
    pub async fn power_off(&self) {
        sqlx::query("UPDATE machines SET online = false WHERE id = $1")
            .bind(self.machine)
            .execute(&self.world.db.pool)
            .await
            .unwrap();
    }

    pub async fn power_back(&self) {
        sqlx::query("UPDATE machines SET online = true WHERE id = $1")
            .bind(self.machine)
            .execute(&self.world.db.pool)
            .await
            .unwrap();
    }

    pub fn app(&self) -> &orch_api_core::testing::TestApp {
        &self.world.app
    }

    /// The commands of a kind that the machine was sent.
    pub fn sent(&self, kind: CommandKind) -> usize {
        self.played
            .as_ref()
            .map_or(0, |played| played.seen_of(kind).len())
    }

    /// Switches the environment's own switch for writes.
    pub async fn set_remote_write(&self, on: bool) {
        sqlx::query("UPDATE environments SET remote_write = $2 WHERE id = $1")
            .bind(self.environment)
            .bind(on)
            .execute(&self.world.db.pool)
            .await
            .unwrap();
    }

    /// Makes the machine upload a list of its tree, as it does when the tree changed, and the system
    /// takes it in.
    pub async fn publish_manifest(&self) -> Hash {
        use orch_engine::manifest::{Entry, EntryKind, Header, build};
        let mut entries = Vec::new();
        for (path, file) in lock(&self.disk.files).iter() {
            entries.push(Entry {
                path: path.clone(),
                kind: EntryKind::File,
                size: i64::try_from(file.bytes.len()).unwrap(),
                mtime_ns: file.mtime_ns,
                mode: file.mode,
                hash: Some(Hash::of(&file.bytes)),
                link: None,
            });
        }
        for dir in lock(&self.disk.dirs).iter() {
            entries.push(Entry {
                path: dir.clone(),
                kind: EntryKind::Dir,
                size: 0,
                mtime_ns: EPOCH_NS,
                mode: 0o755,
                hash: None,
                link: None,
            });
        }
        let written = build(
            &Header {
                v: 1,
                root: "/workspace".to_owned(),
                generated_at: Time::new(self.world.now()),
                generation: 1,
                case_sensitive: true,
            },
            entries,
        )
        .unwrap();
        // The machine uploads the list on its own account (the nil command) and says so.
        let nil = Id::nil();
        self.content
            .upload(self.machine, nil, &written.bytes, PIECE)
            .await;
        let event = orch_link::streams::DeliveredEvent {
            seq: 1,
            at: Time::new(self.world.now()),
            kind: "env.manifest".to_owned(),
            body: json!({
                "manifest_blob": written.hash, "file_count": written.file_count, "total_bytes": written.total_bytes
            }),
        };
        let mut tx = self.world.app.app.db.begin().await.unwrap();
        self.world
            .app
            .app
            .engine
            .apply_stream_events(
                &mut tx,
                self.machine,
                &orch_link_contract::streams::Stream::Env {
                    environment_id: self.environment,
                },
                &[event],
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        written.hash
    }
}

// ---------------------------------------------------------------- the requests, as a person makes them

/// Requests for an environment's workspace, made as `by` through the thread of the site unless a
/// test says otherwise.
pub mod ask {
    use super::Site;
    use crate::places::Who;
    use orch_api_contract::environment::ThroughThreadQuery;
    use orch_api_contract::workspace::{
        DirListing, EnvironmentFileContentGet, EnvironmentFileContentGetQuery,
        EnvironmentFileContentPut, EnvironmentFileContentPutQuery, EnvironmentFileCopy,
        EnvironmentFileDelete, EnvironmentFileDeleteQuery, EnvironmentFileFetch,
        EnvironmentFileFetchQuery, EnvironmentFileList, EnvironmentFileListQuery,
        EnvironmentFileMkdir, EnvironmentFileRename, EnvironmentFileStat, EnvironmentFileStatQuery,
        FetchState, FileFromToBody, FileListSource, FilePathBody, FileStat,
    };
    use orch_api_core::testing::{RawReply, Reply};
    use orch_types::{Hash, Id};

    /// A directory.
    pub async fn list(
        site: &Site,
        by: &Who,
        path: &str,
        source: Option<FileListSource>,
    ) -> Reply<DirListing> {
        site.app()
            .call(EnvironmentFileList, &[&site.environment])
            .token(&by.token)
            .query(&EnvironmentFileListQuery {
                thread: Some(site.thread),
                path: path.to_owned(),
                source,
                mount: None,
                cursor: None,
                limit: None,
            })
            .send()
            .await
    }

    /// A file or a directory, described.
    pub async fn stat(site: &Site, by: &Who, path: &str) -> Reply<FileStat> {
        site.app()
            .call(EnvironmentFileStat, &[&site.environment])
            .token(&by.token)
            .query(&EnvironmentFileStatQuery {
                thread: Some(site.thread),
                path: path.to_owned(),
                mount: None,
            })
            .send()
            .await
    }

    /// A file asked for to be in the store, waiting `wait` seconds for it.
    pub async fn fetch(site: &Site, by: &Who, path: &str, wait: Option<u32>) -> Reply<FetchState> {
        site.app()
            .call(EnvironmentFileFetch, &[&site.environment])
            .token(&by.token)
            .query(&EnvironmentFileFetchQuery {
                thread: Some(site.thread),
                mount: None,
                wait,
            })
            .body(&FilePathBody {
                path: path.to_owned(),
            })
            .send()
            .await
    }

    /// The bytes of a version of a file, with a range if one is given.
    pub async fn content(
        site: &Site,
        by: &Who,
        path: &str,
        hash: &Hash,
        range: Option<&str>,
        download: bool,
    ) -> RawReply {
        let mut call = site
            .app()
            .call(EnvironmentFileContentGet, &[&site.environment])
            .token(&by.token)
            .query(&EnvironmentFileContentGetQuery {
                thread: Some(site.thread),
                mount: None,
                path: path.to_owned(),
                hash: *hash,
                download: download.then_some(true),
                preview: None,
            });
        if let Some(range) = range {
            call = call.header("range", range);
        }
        call.send_bytes().await
    }

    /// A file written with `bytes`, on the version `expected` names (a hash, or `none`).
    pub async fn write(
        site: &Site,
        by: &Who,
        path: &str,
        expected: Option<&str>,
        bytes: &[u8],
    ) -> Reply<FileStat> {
        site.app()
            .call(EnvironmentFileContentPut, &[&site.environment])
            .token(&by.token)
            .query(&EnvironmentFileContentPutQuery {
                thread: Some(site.thread),
                path: path.to_owned(),
                expected_hash: expected.map(str::to_owned),
                blob: None,
                upload_batch: None,
            })
            .bytes(bytes)
            .send()
            .await
    }

    /// A directory made.
    pub async fn mkdir(site: &Site, by: &Who, path: &str) -> Reply<FileStat> {
        site.app()
            .call(EnvironmentFileMkdir, &[&site.environment])
            .token(&by.token)
            .query(&ThroughThreadQuery {
                thread: Some(site.thread),
            })
            .body(&FilePathBody {
                path: path.to_owned(),
            })
            .send()
            .await
    }

    /// A path renamed.
    pub async fn rename(site: &Site, by: &Who, from: &str, to: &str) -> Reply<FileStat> {
        site.app()
            .call(EnvironmentFileRename, &[&site.environment])
            .token(&by.token)
            .query(&ThroughThreadQuery {
                thread: Some(site.thread),
            })
            .body(&FileFromToBody {
                from: from.to_owned(),
                to: to.to_owned(),
            })
            .send()
            .await
    }

    /// A path copied.
    pub async fn copy(site: &Site, by: &Who, from: &str, to: &str) -> Reply<FileStat> {
        site.app()
            .call(EnvironmentFileCopy, &[&site.environment])
            .token(&by.token)
            .query(&ThroughThreadQuery {
                thread: Some(site.thread),
            })
            .body(&FileFromToBody {
                from: from.to_owned(),
                to: to.to_owned(),
            })
            .send()
            .await
    }

    /// A path deleted.
    pub async fn delete(site: &Site, by: &Who, path: &str, recursive: bool) -> Reply<()> {
        site.app()
            .call(EnvironmentFileDelete, &[&site.environment])
            .token(&by.token)
            .query(&EnvironmentFileDeleteQuery {
                thread: Some(site.thread),
                path: path.to_owned(),
                recursive: recursive.then_some(true),
            })
            .send()
            .await
    }

    /// The same, for the thread id given: a caller who names no thread has `None`.
    pub fn via(thread: Option<Id>) -> ThroughThreadQuery {
        ThroughThreadQuery { thread }
    }
}
