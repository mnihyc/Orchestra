//! Stack and edge checks, each saying what it tried and what came back.
//!
//! `orchctl doctor` is what an operator runs when something is wrong, and what
//! `update.sh` waits on before it says that an update worked. Run where the
//! stack runs (`docker compose exec api orchctl doctor`), it reads the
//! configuration and the database address that the API has and tries, in turn,
//! the configuration, the database, the API itself, the gateway's link to the
//! API, the worker's heartbeat and the node. With `--url` it also tries the
//! edge in front of the stack from wherever it is run: a page, an API call, a
//! WebSocket held idle, a body of 8 MiB, the machines' upgrade and one
//! connection to each endpoint the system advertises, under the trust it is
//! advertised with.
//!
//! Every check prints one line: `ok`, or `FAIL` with what was tried and what
//! came back. `wait` is a state that is no fault (the node cannot enroll before
//! the first administrator exists) and `part` a check that could show only a
//! part of what it is for, and says which. The exit status is 1 when any line
//! failed.

mod edge;
mod net;

use clap::Args;
use net::{Address, Resolve};
use orch_config::ServerConfig;
use orch_db::Db;
use orch_db::heartbeat::{self, Heartbeat};
use orch_pki::certificates::{Certificate, ServiceName};
use orch_pki::verification::{ServerTrust, pinned_client_config};
use orch_types::{Clock, SystemClock};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A heartbeat older than this is a service that is gone: the worker itself
/// takes a worker for dead after as long, which is three of the fifteen
/// seconds each service beats at.
const HEARTBEAT_FRESH_SECS: i64 = 45;
/// The longest any one check of the stack may take.
const CHECK_SECS: u64 = 20;
/// The one connection the checks make to the database.
const DATABASE_CONNECTIONS: u32 = 1;
/// The file of the authority's certificate in the secrets directory.
const CA_FILE: &str = "orch_ca_cert.pem";

/// What `orchctl doctor` is asked.
#[derive(Args, Debug)]
pub struct Options {
    /// The public address of the system, to try the edge in front of it as well
    /// (a page, an API call, a WebSocket held idle for 60 s, a body of 8 MiB,
    /// the machines' upgrade and each advertised endpoint). With it the command
    /// can run anywhere; without a configuration to read it tries only the edge.
    #[arg(long, value_name = "ADDRESS")]
    url: Option<String>,
    /// The deployment's configuration file.
    #[arg(long, env = "ORCH_CONFIG", default_value = "/etc/orch/orch.toml")]
    config: PathBuf,
    /// The directory of the deployment's secrets, for the authority's
    /// certificate the API is checked against.
    #[arg(long, env = "ORCH_SECRETS_DIR", default_value = orch_config::DEFAULT_SECRETS_DIR)]
    secrets_dir: PathBuf,
    /// The local password file to inspect, when present. It is not mounted into
    /// the API container; run this check beside the deployment to inspect it.
    #[arg(long, default_value = ".env", value_name = "FILE")]
    env_file: PathBuf,
    /// Try only these checks (`config`, `database`, `api`, `gateway`, `worker`,
    /// `node`, `secrets`, and with `--url` `edge-page`, `edge-api`, `edge-body`, `edge-ws`,
    /// `edge-link`, `edge-endpoints`, `edge-ports`, or `edge` for all of them).
    #[arg(long, value_delimiter = ',', value_name = "CHECKS")]
    only: Vec<String>,
    /// Leave these checks out: `node`, for a stack that has none of its own.
    #[arg(long, value_delimiter = ',', value_name = "CHECKS")]
    skip: Vec<String>,
    /// The release the stack must be running; a service on another fails.
    #[arg(long, value_name = "VERSION")]
    expect_version: Option<String>,
    /// Reach NAME at ADDRESS in the edge checks: the advertised endpoints
    /// may resolve only inside the stack's own network.
    #[arg(long, value_name = "NAME=ADDRESS")]
    resolve: Vec<String>,
    /// An access token of a signed-in browser session, to hold the realtime
    /// socket idle as a person's is: an unsigned one is ended by the API after
    /// ten seconds, whatever the edge does.
    #[arg(
        long,
        env = "ORCH_ACCESS_TOKEN",
        hide_env_values = true,
        value_name = "TOKEN"
    )]
    access_token: Option<String>,
    /// How long the realtime socket is held idle.
    #[arg(long, default_value_t = edge::IDLE_SECS, value_name = "SECONDS", hide = true)]
    idle_secs: u64,
}

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// What was tried worked.
    Ok,
    /// Only a part of what the check is for could be shown.
    Part,
    /// Not yet, and not a fault.
    Wait,
    /// What was tried did not work.
    Fail,
    /// Not tried, and why.
    Skip,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Part => "part",
            Self::Wait => "wait",
            Self::Fail => "FAIL",
            Self::Skip => "skip",
        }
    }
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The check.
    pub id: String,
    /// How it came out.
    pub status: Status,
    /// What was tried and what came back.
    pub said: String,
}

fn line(id: &str, status: Status, said: impl Into<String>) -> Line {
    Line {
        id: id.to_owned(),
        status,
        said: said.into(),
    }
}

/// The checks there are, by the name `--only` and `--skip` take.
const CHECKS: [&str; 14] = [
    "config",
    "secrets",
    "database",
    "api",
    "gateway",
    "worker",
    "node",
    "edge",
    "edge-page",
    "edge-api",
    "edge-body",
    "edge-ws",
    "edge-link",
    "edge-ports",
];

impl Options {
    /// Refuses a name that is no check and a choice that leaves nothing to run:
    /// a typo must not read as a pass.
    fn validate(&self) -> Result<(), String> {
        for name in self.only.iter().chain(&self.skip) {
            if !CHECKS.contains(&name.as_str()) && name != "edge-endpoints" {
                return Err(format!(
                    "{name:?} is no check; they are {}, and edge-endpoints",
                    CHECKS.join(", ")
                ));
            }
        }
        let host = [
            "config", "secrets", "database", "api", "gateway", "worker", "node",
        ]
        .iter()
        .any(|id| self.wanted(id));
        let edge = self.url.is_some()
            && [
                "edge-page",
                "edge-api",
                "edge-body",
                "edge-ws",
                "edge-link",
                "edge-endpoints",
                "edge-ports",
            ]
            .iter()
            .any(|id| self.wanted(id));
        if host || edge {
            Ok(())
        } else {
            Err("no check is left to run (the checks of the edge need --url)".to_owned())
        }
    }

    fn matches(list: &[String], id: &str) -> bool {
        list.iter().any(|wanted| {
            wanted == id
                || (wanted == "edge" && id.starts_with("edge"))
                || (wanted == "edge-endpoints" && id.starts_with("edge-endpoint-"))
        })
    }

    fn wanted(&self, id: &str) -> bool {
        (self.only.is_empty() || Self::matches(&self.only, id)) && !Self::matches(&self.skip, id)
    }
}

/// Runs the checks, prints the report, and fails when a line did.
pub fn run(options: &Options) -> Result<(), String> {
    options.validate()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start: {error}"))?;
    runtime.block_on(checks(options))
}

async fn checks(options: &Options) -> Result<(), String> {
    let resolve = Resolve::parse(&options.resolve)?;
    let mut lines = Vec::new();
    let show = |batch: Vec<Line>, lines: &mut Vec<Line>| {
        for line in &batch {
            println!("{:<4}  {:<16} {}", line.status.label(), line.id, line.said);
        }
        lines.extend(batch);
    };

    // The stack itself, where there is a stack to read.
    let host = options.config.is_file() || options.url.is_none();
    if host {
        show(host_checks(options).await, &mut lines);
    } else {
        show(
            vec![line(
                "host",
                Status::Skip,
                format!(
                    "no configuration at {}: only the edge is tried",
                    options.config.display()
                ),
            )],
            &mut lines,
        );
    }

    // The edge in front of it, which waits a minute for the socket.
    if let Some(url) = &options.url {
        let address = Address::parse(url)?;
        let wanted = |id: &str| options.wanted(id);
        let edge = edge::Edge {
            address,
            resolve: &resolve,
            secrets_dir: &options.secrets_dir,
            access_token: options.access_token.as_deref(),
            idle: Duration::from_secs(options.idle_secs),
            wanted: &wanted,
            ports_domain: orch_config::load::<ServerConfig>(&options.config)
                .ok()
                .and_then(|config| config.edge.ports_domain),
        };
        show(edge::checks(&edge).await, &mut lines);
    }

    let failed = lines
        .iter()
        .filter(|line| line.status == Status::Fail)
        .count();
    if failed == 0 {
        println!("all {} checks passed", lines.len());
        Ok(())
    } else {
        Err(format!("{failed} of {} checks failed", lines.len()))
    }
}

/// The checks of the stack, in the order that a fault is found.
async fn host_checks(options: &Options) -> Vec<Line> {
    let mut lines = Vec::new();
    if options.wanted("secrets") {
        lines.push(secret_permissions(options));
    }
    let config = match orch_config::load::<ServerConfig>(&options.config) {
        Ok(config) => config,
        Err(error) => {
            if options.wanted("config") {
                lines.push(line("config", Status::Fail, error.to_string()));
            }
            return lines;
        }
    };
    if options.wanted("config") {
        lines.push(line(
            "config",
            Status::Ok,
            format!(
                "read {}: {} at {}. {}",
                options.config.display(),
                config.system.name,
                config.system.public_url,
                if config.api.trusted_proxy_cidrs.is_empty() {
                    "trusted_proxy_cidrs is empty: forwarded headers are ignored; callers behind an unnamed private proxy are counted as the proxy".to_owned()
                } else {
                    format!("trusted_proxy_cidrs: {}; forwarded chains are read only from these ranges; callers behind an unnamed private proxy outside them are counted as the proxy", config.api.trusted_proxy_cidrs.join(", "))
                }
            ),
        ));
    }

    let needs_database = ["database", "gateway", "worker", "node"]
        .iter()
        .any(|id| options.wanted(id));
    let (database, database_line) = if needs_database {
        let (db, said) = connect(&config).await;
        (db, Some(said))
    } else {
        (None, None)
    };
    if options.wanted("database")
        && let Some(said) = database_line
    {
        lines.push(said);
    }

    if options.wanted("api") {
        lines.push(within("api", api(options, &config)).await);
    }

    for id in ["gateway", "worker", "node"] {
        if !options.wanted(id) {
            continue;
        }
        let done = match &database {
            Some(db) => within(id, services(id, db, options.expect_version.as_deref())).await,
            None => line(
                id,
                Status::Skip,
                "needs the database, which does not answer",
            ),
        };
        lines.push(done);
    }
    lines
}

/// Inspect permissions without reading or printing any credential contents.
#[cfg(unix)]
fn secret_permissions(options: &Options) -> Line {
    use super::bootstrap::{Protection, expected_files};
    use std::os::unix::fs::PermissionsExt;

    // A credential belongs to its owner alone, including group access.
    const OTHER_ACCESS: u32 = 0o077;
    // The bootstrap manifest declares certificates public; others may read them, never write or execute them.
    const PUBLIC_OTHER_ACCESS: u32 = 0o033;
    let expected = expected_files();
    let mut files = Vec::new();
    if options.env_file.exists() {
        files.push((options.env_file.clone(), Protection::Private, false));
    }
    match std::fs::read_dir(&options.secrets_dir) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        let name = entry.file_name();
                        let protection = expected
                            .iter()
                            .find(|(known, _)| name.to_str() == Some(known.as_str()))
                            .map_or(Protection::Private, |(_, protection)| *protection);
                        files.push((entry.path(), protection, name == ".gitkeep"));
                    }
                    Err(error) => {
                        return line(
                            "secrets",
                            Status::Fail,
                            format!("cannot inspect {}: {error}", options.secrets_dir.display()),
                        );
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return line(
                "secrets",
                Status::Fail,
                format!("cannot inspect {}: {error}", options.secrets_dir.display()),
            );
        }
    }
    files.sort_by(|(left, _, _), (right, _, _)| left.cmp(right));
    let mut exposed = Vec::new();
    let mut checked = 0;
    for (file, protection, placeholder) in files {
        match std::fs::metadata(&file) {
            Ok(metadata) if metadata.is_file() => {
                // The shipped empty directory marker contains no credential; a nonempty one is private.
                if placeholder && metadata.len() == 0 {
                    continue;
                }
                checked += 1;
                let mode = metadata.permissions().mode();
                let forbidden = match protection {
                    Protection::Private => OTHER_ACCESS,
                    Protection::Public => PUBLIC_OTHER_ACCESS,
                };
                if mode & forbidden != 0 {
                    exposed.push(format!("{} (mode {:04o})", file.display(), mode & 0o777));
                }
            }
            Ok(_) => {}
            Err(error) => {
                return line(
                    "secrets",
                    Status::Fail,
                    format!("cannot inspect {}: {error}", file.display()),
                );
            }
        }
    }
    if !exposed.is_empty() {
        line(
            "secrets",
            Status::Fail,
            format!(
                "other accounts can access {}; set these files to mode 0600 as their owner",
                exposed.join(", ")
            ),
        )
    } else if checked == 0 {
        line(
            "secrets",
            Status::Skip,
            "no local password or secret files to inspect",
        )
    } else {
        line(
            "secrets",
            Status::Ok,
            format!("{checked} local files have their declared access protections"),
        )
    }
}

#[cfg(not(unix))]
fn secret_permissions(_options: &Options) -> Line {
    line(
        "secrets",
        Status::Part,
        "check local credential access with this host's file permissions",
    )
}

/// Runs one check, and gives it up when it takes too long.
async fn within(id: &str, check: impl std::future::Future<Output = Line>) -> Line {
    match tokio::time::timeout(Duration::from_secs(CHECK_SECS), check).await {
        Ok(line) => line,
        Err(_) => line(id, Status::Fail, format!("no answer in {CHECK_SECS} s")),
    }
}

/// Where the database is, for a sentence: the host, the port and the name,
/// and never the password.
fn place_of(url: &str) -> String {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let after_credentials = after_scheme
        .rsplit_once('@')
        .map_or(after_scheme, |(_, rest)| rest);
    after_credentials
        .split(['?', '#'])
        .next()
        .unwrap_or(after_credentials)
        .to_owned()
}

/// Connects to the database and says how far its schema has come.
async fn connect(config: &ServerConfig) -> (Option<Db>, Line) {
    const ID: &str = "database";
    let url = &config.database.url;
    if url.is_empty() {
        return (
            None,
            line(
                ID,
                Status::Fail,
                "no database address: ORCH_DATABASE_URL is not set where this runs",
            ),
        );
    }
    let place = place_of(url);
    let tried = format!("connecting to {place}");
    let connecting = tokio::time::timeout(
        Duration::from_secs(CHECK_SECS),
        Db::connect(url, DATABASE_CONNECTIONS),
    )
    .await;
    let db = match connecting {
        Err(_) => {
            return (
                None,
                line(
                    ID,
                    Status::Fail,
                    format!("{tried}: no answer in {CHECK_SECS} s"),
                ),
            );
        }
        Ok(Err(error)) => return (None, line(ID, Status::Fail, format!("{tried}: {error}"))),
        Ok(Ok(db)) => db,
    };
    let facts = async {
        let mut conn = db
            .acquire()
            .await
            .map_err(|error| format!("{tried}: {error}"))?;
        let version = heartbeat::server_version(&mut conn)
            .await
            .map_err(|error| format!("asking {place} for its version: {error}"))?;
        let schema = heartbeat::schema_state(&mut conn)
            .await
            .map_err(|error| format!("reading the migrations at {place}: {error}"))?;
        Ok::<_, String>((version, schema))
    }
    .await;
    let (version, schema) = match facts {
        Ok(facts) => facts,
        Err(error) => return (Some(db), line(ID, Status::Fail, error)),
    };
    let shipped = shipped_migrations(Path::new(&config.database.migrations_dir));
    let name = version
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let newest = schema
        .newest
        .map_or_else(|| "none".to_owned(), |n| n.to_string());
    let said = match shipped {
        Some(shipped) if schema.newest.unwrap_or(0) < shipped => line(
            ID,
            Status::Fail,
            format!(
                "connected to {place} ({name}); the schema is at migration {newest} and the release ships up to {shipped}: the API applies the rest when it starts"
            ),
        ),
        Some(shipped) => line(
            ID,
            Status::Ok,
            format!(
                "connected to {place} ({name}); {} migrations applied, the newest {newest} (the release ships up to {shipped})",
                schema.applied
            ),
        ),
        None => line(
            ID,
            Status::Ok,
            format!(
                "connected to {place} ({name}); {} migrations applied, the newest {newest}",
                schema.applied
            ),
        ),
    };
    (Some(db), said)
}

/// The number of the newest migration the release ships, when its directory is
/// here to be read.
fn shipped_migrations(dir: &Path) -> Option<i64> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".sql")?
                .split('_')
                .next()?
                .parse::<i64>()
                .ok()
        })
        .max()
}

/// The API on its own listener: its health, and what it says it is.
async fn api(options: &Options, config: &ServerConfig) -> Line {
    const ID: &str = "api";
    let bind: SocketAddr = match config.api.bind.parse() {
        Ok(bind) => bind,
        Err(_) => {
            return line(
                ID,
                Status::Fail,
                format!("[api] bind {:?} is not an address", config.api.bind),
            );
        }
    };
    // The listener is reached on this machine, by the name its certificate is
    // good for.
    let host = if bind.ip().is_unspecified() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        bind.ip()
    };
    let authority = match authority(&options.secrets_dir) {
        Ok(authority) => authority,
        Err(error) => return line(ID, Status::Fail, error),
    };
    let tls = match pinned_client_config(
        ServerTrust::Authority(Box::new(authority)),
        Some(ServiceName::Api),
        None,
        Arc::new(SystemClock),
    ) {
        Ok(tls) => tls,
        Err(error) => return line(ID, Status::Fail, error.to_string()),
    };
    let address = Address {
        secure: true,
        host: host.to_string(),
        port: bind.port(),
        base: String::new(),
    };
    let place = format!("{host}:{}", bind.port());
    let ask = |path: &'static str| {
        let (address, tls) = (address.clone(), Arc::clone(&tls));
        async move {
            let stream =
                net::connect(&address, &Resolve::default(), Some(&tls), Some("api")).await?;
            net::request(
                stream,
                http::Method::GET,
                "api:8443",
                path,
                &[],
                bytes::Bytes::new(),
            )
            .await
        }
    };
    let health = match ask("/health").await {
        Ok(reply) => reply,
        Err(error) => {
            return line(
                ID,
                Status::Fail,
                format!("GET https://{place}/health as api: {error}"),
            );
        }
    };
    if health.status != 200 {
        return line(
            ID,
            Status::Fail,
            format!(
                "GET https://{place}/health -> {}: the API is not ready; check database connectivity and the worker heartbeat",
                health.status
            ),
        );
    }
    let meta = match ask("/api/v1/meta").await {
        Ok(reply) if reply.status == 200 => reply,
        Ok(reply) => {
            return line(
                ID,
                Status::Fail,
                format!("GET https://{place}/api/v1/meta -> {}", reply.status),
            );
        }
        Err(error) => {
            return line(
                ID,
                Status::Fail,
                format!("GET https://{place}/api/v1/meta: {error}"),
            );
        }
    };
    let meta: serde_json::Value = match serde_json::from_slice(&meta.body) {
        Ok(meta) => meta,
        Err(error) => {
            return line(
                ID,
                Status::Fail,
                format!("GET https://{place}/api/v1/meta -> 200, and not JSON: {error}"),
            );
        }
    };
    let version = meta
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let build = meta
        .get("build")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    if let Some(expected) = &options.expect_version
        && expected != version
    {
        return line(
            ID,
            Status::Fail,
            format!(
                "GET https://{place}/api/v1/meta -> release {version}, and release {expected} was expected"
            ),
        );
    }
    line(
        ID,
        Status::Ok,
        format!(
            "GET https://{place}/health -> 200; /api/v1/meta -> release {version} (build {build})"
        ),
    )
}

/// The authority's certificate from the secrets directory.
fn authority(dir: &Path) -> Result<Certificate, String> {
    let path = dir.join(CA_FILE);
    let pem = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "cannot read the authority's certificate {}: {error}",
            path.display()
        )
    })?;
    Certificate::from_pem(&pem)
        .map_err(|error| format!("{} is not a certificate: {error}", path.display()))
}

/// The services that report through the database: the gateway, the worker and
/// the node.
async fn services(id: &str, db: &Db, expect: Option<&str>) -> Line {
    let mut conn = match db.acquire().await {
        Ok(conn) => conn,
        Err(error) => {
            return line(
                id,
                Status::Fail,
                format!("the database does not answer: {error}"),
            );
        }
    };
    let now = SystemClock.now();
    match id {
        "node" => node(&mut conn).await,
        component => {
            let beats = match heartbeat::all(&mut conn).await {
                Ok(beats) => beats,
                Err(error) => {
                    return line(id, Status::Fail, format!("reading the heartbeats: {error}"));
                }
            };
            let beat = beats.iter().find(|beat| beat.component == component);
            beat_line(component, beat, now, expect)
        }
    }
}

fn beat_line(
    component: &str,
    beat: Option<&Heartbeat>,
    now: chrono::DateTime<chrono::Utc>,
    expect: Option<&str>,
) -> Line {
    let reach = match component {
        "gateway" => {
            "it reports through the API's backplane, so the link between them is down or the gateway is"
        }
        _ => "it reports through the database",
    };
    let Some(beat) = beat else {
        return line(
            component,
            Status::Fail,
            format!(
                "the heartbeats hold no row for the {component}: it has not reported yet, or never will; {reach}"
            ),
        );
    };
    let age = (now - beat.seen_at).num_seconds();
    if age > HEARTBEAT_FRESH_SECS {
        return line(
            component,
            Status::Fail,
            format!(
                "last heard {age} s ago (release {}), and a live one reports every few seconds: {reach}",
                beat.version
            ),
        );
    }
    if let Some(expected) = expect
        && expected != beat.version
    {
        return line(
            component,
            Status::Fail,
            format!(
                "heartbeat {age} s ago, from release {}; release {expected} was expected",
                beat.version
            ),
        );
    }
    let how = if component == "gateway" {
        "the backplane to the API is connected"
    } else {
        "alive"
    };
    line(
        component,
        Status::Ok,
        format!("heartbeat {age} s ago, release {}: {how}", beat.version),
    )
}

/// The node: a host of the system that has a session.
async fn node(conn: &mut orch_db::Conn) -> Line {
    const ID: &str = "node";
    let hosts = match heartbeat::hosts_seen(conn).await {
        Ok(hosts) => hosts,
        Err(error) => return line(ID, Status::Fail, format!("reading the hosts: {error}")),
    };
    if hosts.is_empty() {
        return match orch_db::queries::identity::no_users_yet(conn).await {
            Ok(true) => line(
                ID,
                Status::Wait,
                "no host yet, and no one has signed up: the bundled node enrolls itself as soon as the first administrator exists",
            ),
            Ok(false) => line(
                ID,
                Status::Fail,
                "no host has enrolled, though the first administrator exists: the bundled node should have; see its log",
            ),
            Err(error) => line(ID, Status::Fail, format!("reading the users: {error}")),
        };
    }
    let online: Vec<_> = hosts.iter().filter(|host| host.online).collect();
    if let Some(host) = online.first() {
        return line(
            ID,
            Status::Ok,
            format!(
                "host {} is online (connector {}); {} of {} hosts have a session",
                host.name,
                host.connector_version,
                online.len(),
                hosts.len()
            ),
        );
    }
    let seen = hosts
        .iter()
        .map(|host| {
            let when = host
                .last_seen_at
                .map_or_else(|| "never seen".to_owned(), |at| format!("last seen {at}"));
            format!("{} ({when})", host.name)
        })
        .collect::<Vec<_>>()
        .join(", ");
    line(ID, Status::Fail, format!("no host has a session: {seen}"))
}
