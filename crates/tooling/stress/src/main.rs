//! orch-stress: seeds a running Orch through its HTTP API, drives it at a team's scale for a set time (threads taking
//! turns through the scripted harness, simulated browsers reading pages and holding live feeds, people searching and
//! using the finder), and reports each route's latency, the errors by code, the live feeds' lag and, with a database
//! URL, the statements that ran long. The latency and error targets are documented in the README.

#![forbid(unsafe_code)]

mod api;
mod drive;
mod feed;
mod metrics;
mod rng;
mod seed;
mod state;
mod statements;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use metrics::{LAG_BAR_MS, Measured, STATEMENT_BAR_MS};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "orch-stress",
    version,
    about = "Seeds a running Orch through its API and drives it at a team's scale"
)]
struct Cli {
    #[command(flatten)]
    conn: Conn,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone)]
struct Conn {
    /// The API's address as a browser reaches it.
    #[arg(
        long,
        env = "ORCH_STRESS_API",
        default_value = "http://127.0.0.1:41280/api/v1",
        global = true
    )]
    api: String,
    /// The realtime socket; by default the API's address with ws(s):// and /ws.
    #[arg(long, env = "ORCH_STRESS_WS", global = true)]
    ws: Option<String>,
    /// A system administrator, who seeds.
    #[arg(long, env = "ORCH_STRESS_LOGIN", default_value = "ada", global = true)]
    login: String,
    /// The administrator's password; the people the seeder makes get it too.
    #[arg(
        long,
        env = "ORCH_STRESS_PASSWORD",
        hide_env_values = true,
        global = true
    )]
    password: Option<String>,
    /// Where what was made is kept between runs.
    #[arg(long, default_value = "orch-stress-state.json", global = true)]
    state: PathBuf,
    /// The first word of every name it makes (lowercase letters and digits).
    #[arg(long, default_value = "stress", global = true)]
    prefix: String,
}

/// Default team-sized scenario counts, multiplied by `--scale`.
#[derive(Args, Clone)]
struct FigureArgs {
    /// Root orgs.
    #[arg(long, default_value_t = 10)]
    orgs: usize,
    /// Projects, spread over the orgs; each has one topic.
    #[arg(long, default_value_t = 100)]
    projects: usize,
    /// Threads, spread over the topics: all of them take turns at once.
    #[arg(long, default_value_t = 100)]
    threads: usize,
    /// Documents, spread over the projects (every fifth with versions, every fourth with a discussion).
    #[arg(long, default_value_t = 1000)]
    docs: usize,
    /// API providers, spread over the orgs.
    #[arg(long, default_value_t = 10)]
    providers: usize,
    /// Agent profiles of the scripted harness, spread over the orgs.
    #[arg(long, default_value_t = 10)]
    agents: usize,
    /// Hosts wanted: counted, never made (a host is a node that enrolled).
    #[arg(long, default_value_t = 10)]
    hosts: usize,
    /// Simulated browsers, one person each (the seeder makes that many people).
    #[arg(long, default_value_t = 20)]
    browsers: usize,
    /// People who search and type into the finder.
    #[arg(long, default_value_t = 5)]
    searchers: usize,
    /// Multiplies every figure above: 1 is the owner's figure, 2 and 10 the next steps.
    #[arg(long, default_value_t = 1)]
    scale: usize,
}

/// The figures after `--scale`.
pub struct Figures {
    pub orgs: usize,
    pub projects: usize,
    pub threads: usize,
    pub docs: usize,
    pub providers: usize,
    pub agents: usize,
    pub hosts: usize,
    pub people: usize,
    pub searchers: usize,
}

impl FigureArgs {
    fn scaled(&self) -> Figures {
        let k = self.scale.max(1);
        Figures {
            orgs: self.orgs * k,
            projects: self.projects * k,
            threads: self.threads * k,
            docs: self.docs * k,
            providers: self.providers * k,
            agents: self.agents * k,
            hosts: self.hosts * k,
            people: self.browsers * k,
            searchers: self.searchers * k,
        }
    }
}

#[derive(Args, Clone)]
struct SeedArgs {
    /// The machine the threads run on (name or id); by default the first online one with the agent, environment
    /// and provider roles.
    #[arg(long)]
    machine: Option<String>,
    /// A running environment on that machine in whose directory the orgs' environments get theirs; found when left out.
    #[arg(long)]
    carrier: Option<String>,
    /// The providers' upstream (the scripted harness makes no model call; a provider needs one all the same).
    #[arg(long, default_value = "http://127.0.0.1:41285/v1")]
    model_upstream: String,
    /// The upstream's protocol.
    #[arg(long, default_value = "openai-responses")]
    model_protocol: String,
    /// The upstream's key (a development stack's mock model server key).
    #[arg(long, env = "ORCH_STRESS_MODEL_KEY", hide_env_values = true)]
    model_key: Option<String>,
    /// The model the threads use.
    #[arg(long, default_value = "mock-large")]
    model: String,
    /// The sandbox mode set at each org. A connected machine's own executor enforces only `open`, and the scripted
    /// harness runs nothing; `none` leaves the setting alone.
    #[arg(long, default_value = "open")]
    sandbox_mode: String,
    /// How a thread's agent is placed (`together`, `key_separated`, `separated`, …); left to the system (its first
    /// permitted arrangement) when not given.
    #[arg(long)]
    arrangement: Option<String>,
    /// How many things are made at once.
    #[arg(long, default_value_t = 12)]
    seed_concurrency: usize,
    /// How many orgs each person is a developer in.
    #[arg(long, default_value_t = 3)]
    orgs_per_person: usize,
}

#[derive(Args, Clone)]
struct DriveArgs {
    /// How long the measured window lasts (`90s`, `10m`, `1h`).
    #[arg(long, default_value = "10m", value_parser = parse_duration)]
    duration: Duration,
    /// How long a turn's script pauses in all, standing for a model's time.
    #[arg(long, default_value_t = 6.0)]
    turn_seconds: f64,
    /// How long a person pauses between a turn's end and their next input, on average.
    #[arg(long, default_value = "3s", value_parser = parse_duration)]
    pause: Duration,
    /// How long a person reads a page, on average.
    #[arg(long, default_value = "3s", value_parser = parse_duration)]
    dwell: Duration,
    /// How long a turn may take before it is counted as not seen to end.
    #[arg(long, default_value = "120s", value_parser = parse_duration)]
    turn_timeout: Duration,
    /// Browsers and searchers only: no turns.
    #[arg(long)]
    no_turns: bool,
    /// When a thread says it is blocked on its provider, the administrator refreshes that provider (at most every
    /// 30 s), as its manager would; the report counts both. Off, a blocked thread stays blocked.
    #[arg(long)]
    refresh_blocked: bool,
    /// A PostgreSQL URL of the system's database: the statements over --slow-ms in the window, from pg_stat_statements.
    #[arg(long, env = "ORCH_STRESS_DATABASE_URL", hide_env_values = true)]
    database_url: Option<String>,
    /// The threshold of the statements reported.
    #[arg(long, default_value_t = 100.0)]
    slow_ms: f64,
    /// Also write the report as JSON here.
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Makes what the drive needs; what the state file says exists is kept.
    Seed {
        #[command(flatten)]
        figures: FigureArgs,
        #[command(flatten)]
        seed: SeedArgs,
    },
    /// Drives what was seeded for --duration, then reports.
    Drive {
        #[command(flatten)]
        figures: FigureArgs,
        #[command(flatten)]
        drive: DriveArgs,
    },
    /// Seeds, then drives.
    Run {
        #[command(flatten)]
        figures: FigureArgs,
        #[command(flatten)]
        seed: SeedArgs,
        #[command(flatten)]
        drive: DriveArgs,
    },
}

fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let n: f64 = number
        .parse()
        .map_err(|_| format!("not a duration: {text}"))?;
    let secs = match unit {
        "ms" => n / 1000.0,
        "" | "s" => n,
        "m" => n * 60.0,
        "h" => n * 3600.0,
        _ => return Err(format!("not a duration: {text} (use ms, s, m or h)")),
    };
    Ok(Duration::from_secs_f64(secs))
}

/// The socket's address from the API's: `http://h/api/v1` -> `ws://h/api/v1/ws`.
fn socket_url(api: &str) -> String {
    let base = api.trim_end_matches('/');
    let base = base
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    format!("{base}/ws")
}

#[derive(Serialize)]
struct Verdict {
    reads_within_bar: bool,
    writes_within_bar: bool,
    http_errors: u64,
    turn_failures: u64,
    socket_faults: u64,
    no_errors: bool,
    statements_within_bar: Option<bool>,
    /// `None` when no change was seen on a feed (nothing to judge).
    lag_within_bar: Option<bool>,
    pass: bool,
}

#[derive(Serialize)]
struct Report {
    api: String,
    at: String,
    window_secs: u64,
    scale: usize,
    made: Made,
    measured: Measured,
    statements: Option<Vec<statements::Slow>>,
    sessions: Option<statements::Activity>,
    verdict: Verdict,
}

#[derive(Serialize)]
struct Made {
    people: usize,
    orgs: usize,
    projects: usize,
    topics: usize,
    threads_driven: usize,
    docs: usize,
    providers: usize,
    agents: usize,
    hosts: Vec<String>,
    hosts_wanted: usize,
    machine: Option<String>,
}

fn verdict(m: &Measured, statements: Option<&Vec<statements::Slow>>, turns: bool) -> Verdict {
    let reads = m
        .routes
        .iter()
        .filter(|r| r.class == "read")
        .all(|r| r.within_bar);
    let writes = m
        .routes
        .iter()
        .filter(|r| r.class == "write")
        .all(|r| r.within_bar);
    let http_errors: u64 = m.routes.iter().flat_map(|r| r.errors.values()).sum();
    let turn_failures: u64 = m.turns.refused.values().sum::<u64>()
        + m.turns.blocked.values().sum::<u64>()
        + m.turns
            .ended
            .iter()
            .filter(|(k, _)| k.as_str() != "completed")
            .map(|(_, v)| v)
            .sum::<u64>()
        + m.turns.timed_out;
    let socket_faults: u64 = m
        .sockets
        .iter()
        .filter(|(k, _)| {
            k.starts_with("error ")
                || k.starts_with("connect failed")
                || k.starts_with("dropped")
                || k.starts_with("closed by")
                || k.starts_with("resync")
                || k.starts_with("feed closed")
        })
        .map(|(_, v)| v)
        .sum();
    let statements_ok = statements.map(|s| {
        s.iter()
            .all(|x| x.max_ms.unwrap_or(x.mean_ms) <= STATEMENT_BAR_MS)
    });
    let lag = (!m.lag.is_empty()).then(|| m.lag.iter().all(|l| l.p95 < LAG_BAR_MS));
    let no_errors = http_errors == 0 && turn_failures == 0 && socket_faults == 0;
    // Turns that ran and were never seen on a feed are a fault, not a pass.
    let lag_ok = lag.unwrap_or(!turns);
    Verdict {
        reads_within_bar: reads,
        writes_within_bar: writes,
        http_errors,
        turn_failures,
        socket_faults,
        no_errors,
        statements_within_bar: statements_ok,
        lag_within_bar: lag,
        pass: reads && writes && no_errors && statements_ok.unwrap_or(true) && lag_ok,
    }
}

fn mark(ok: bool) -> &'static str {
    if ok { "ok" } else { "OVER" }
}

fn print(r: &Report) {
    let m = &r.measured;
    let v = &r.verdict;
    println!(
        "orch-stress report: {} at {} (window {} s, scale {})",
        r.api, r.at, r.window_secs, r.scale
    );
    println!(
        "drove: {} people, {} orgs, {} projects, {} topics, {} threads, {} documents, {} providers, {} agent profiles on {}; hosts {} of {} ({})",
        r.made.people,
        r.made.orgs,
        r.made.projects,
        r.made.topics,
        r.made.threads_driven,
        r.made.docs,
        r.made.providers,
        r.made.agents,
        r.made.machine.as_deref().unwrap_or("?"),
        r.made.hosts.len(),
        r.made.hosts_wanted,
        r.made.hosts.join(", ")
    );
    println!();
    println!(
        "VERDICT {}: reads p95<300ms {} | writes p95<1s {} | errors {} (http {}, turns {}, sockets {}) | statements<=1s {} | live-feed lag p95<2s {}",
        if v.pass { "PASS" } else { "FAIL" },
        mark(v.reads_within_bar),
        mark(v.writes_within_bar),
        if v.no_errors { "none" } else { "SOME" },
        v.http_errors,
        v.turn_failures,
        v.socket_faults,
        v.statements_within_bar
            .map_or("not read (no --database-url)", |ok| if ok {
                "ok"
            } else {
                "OVER"
            }),
        v.lag_within_bar
            .map_or("no change seen on a feed", |ok| if ok {
                "ok"
            } else {
                "OVER"
            })
    );
    println!();
    summary(r);
    println!();
    println!(
        "{:<6} {:<62} {:>7} {:>8} {:>8} {:>8} {:>8}  bar / errors",
        "class", "route (drive window)", "n", "p50 ms", "p95 ms", "p99 ms", "max ms"
    );
    for row in &m.routes {
        let errors = if row.errors.is_empty() {
            String::new()
        } else {
            row.errors
                .iter()
                .map(|(k, n)| format!("{k} x{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "{:<6} {:<62} {:>7} {:>8.0} {:>8.0} {:>8.0} {:>8.0}  {} {}",
            row.class,
            row.route,
            row.n,
            row.p50,
            row.p95,
            row.p99,
            row.max,
            mark(row.within_bar),
            errors
        );
    }
    println!();
    println!(
        "{:<48} {:>7} {:>8} {:>8} {:>8} {:>8}",
        "page (its reads at once, until the last)", "n", "p50 ms", "p95 ms", "p99 ms", "max ms"
    );
    for p in &m.pages {
        println!(
            "{:<48} {:>7} {:>8.0} {:>8.0} {:>8.0} {:>8.0}",
            p.name, p.n, p.p50, p.p95, p.p99, p.max
        );
    }
    println!();
    println!(
        "{:<48} {:>7} {:>8} {:>8} {:>8} {:>8}  over 2 s",
        "live feed: sent -> first frame on a socket", "n", "p50 ms", "p95 ms", "p99 ms", "max ms"
    );
    for l in &m.lag {
        println!(
            "{:<48} {:>7} {:>8.0} {:>8.0} {:>8.0} {:>8.0}  {}",
            l.name, l.n, l.p50, l.p95, l.p99, l.max, l.over_bar
        );
    }
    let p = &m.ping;
    println!(
        "{:<48} {:>7} {:>8.0} {:>8.0} {:>8.0} {:>8.0}",
        p.name, p.n, p.p50, p.p95, p.p99, p.max
    );
    println!();
    let t = &m.turns;
    println!(
        "turns: {} inputs sent, ended {:?}, blocked while waiting {:?} (providers refreshed {}), refused {:?}, not seen to end {}; input to completed p50 {:.1} s, p95 {:.1} s, max {:.1} s",
        t.sent,
        t.ended,
        t.blocked,
        t.providers_refreshed,
        t.refused,
        t.timed_out,
        t.completed.p50 / 1000.0,
        t.completed.p95 / 1000.0,
        t.completed.max / 1000.0
    );
    println!("sockets: {:?}", m.sockets);
    if let Some(slow) = &r.statements {
        println!();
        println!(
            "statements over the threshold in the window (pg_stat_statements; other clients of the database included):"
        );
        println!(
            "{:>9} {:>9} {:>8} {:>10}  statement",
            "max ms", "mean ms", "calls", "total ms"
        );
        for s in slow.iter().take(40) {
            println!(
                "{:>9} {:>9.1} {:>8} {:>10.0}  {}",
                s.max_ms
                    .map_or_else(|| "-".to_owned(), |m| format!("{m:.0}")),
                s.mean_ms,
                s.calls,
                s.total_ms,
                s.query.chars().take(300).collect::<String>()
            );
        }
    }
    if let Some(a) = &r.sessions {
        println!();
        println!(
            "database sessions ({} samples, every 2 s; transactions older than the window left out): peak {}",
            a.samples,
            a.peak
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!(
            "{:>7} {:>8}  held idle in a transaction for over 1 s, after",
            "seen", "max s"
        );
        for h in a.idle_in_transaction.iter().take(10) {
            println!(
                "{:>7} {:>8.1}  {}",
                h.seen,
                h.max_secs,
                h.query.chars().take(200).collect::<String>()
            );
        }
        println!(
            "{:>7} {:>8}  {:<20} waiting on a lock for over 1 s",
            "seen", "max s", "wait"
        );
        for h in a.lock_waits.iter().take(10) {
            println!(
                "{:>7} {:>8.1}  {:<20} {}",
                h.seen,
                h.max_secs,
                h.what,
                h.query.chars().take(180).collect::<String>()
            );
        }
    }
    if !m.notes.is_empty() {
        println!();
        println!("notes:");
        for n in &m.notes {
            println!("- {n}");
        }
    }
    if !m.setup_routes.is_empty() {
        println!();
        println!("setup requests (seeding, signing in; not judged):");
        for row in m.setup_routes.iter().take(25) {
            let errors: u64 = row.errors.values().sum();
            println!(
                "  {:<62} n {:>5}  p50 {:>6.0}  p95 {:>6.0}  max {:>6.0} ms  errors {}",
                row.route, row.n, row.p50, row.p95, row.max, errors
            );
        }
    }
}

/// One line per target: the threshold, the worst measurement, and whether it holds.
fn summary(r: &Report) {
    let m = &r.measured;
    let v = &r.verdict;
    let worst = |class: &str| {
        m.routes
            .iter()
            .filter(|row| row.class == class)
            .max_by(|a, b| a.p95.total_cmp(&b.p95))
            .map_or_else(
                || "no requests".to_owned(),
                |row| {
                    let over = m
                        .routes
                        .iter()
                        .filter(|x| x.class == class && !x.within_bar)
                        .count();
                    let all = m.routes.iter().filter(|x| x.class == class).count();
                    format!(
                        "worst p95 {:.0} ms ({}); {over} of {all} routes over",
                        row.p95, row.route
                    )
                },
            )
    };
    let lag = m
        .lag
        .iter()
        .max_by(|a, b| a.p95.total_cmp(&b.p95))
        .map_or_else(
            || "no change seen on a feed".to_owned(),
            |l| format!("worst p95 {:.0} ms ({}, n {})", l.p95, l.name, l.n),
        );
    let slowest = r.statements.as_ref().map_or_else(
        || "not read (give --database-url)".to_owned(),
        |s| {
            s.iter()
                .map(|x| x.max_ms.unwrap_or(x.mean_ms))
                .max_by(f64::total_cmp)
                .map_or_else(
                    || "none over the threshold".to_owned(),
                    |ms| {
                        format!(
                            "slowest in the window {ms:.0} ms; {} statements over the threshold",
                            s.len()
                        )
                    },
                )
        },
    );
    let rows = [
        (
            "reads: p95 under 300 ms",
            worst("read"),
            mark(v.reads_within_bar).to_owned(),
        ),
        (
            "writes: p95 under 1 s",
            worst("write"),
            mark(v.writes_within_bar).to_owned(),
        ),
        (
            "no errors",
            format!(
                "http {}, turns {}, sockets {}",
                v.http_errors, v.turn_failures, v.socket_faults
            ),
            (if v.no_errors { "ok" } else { "OVER" }).to_owned(),
        ),
        (
            "no statement over 1 s",
            slowest,
            v.statements_within_bar.map_or("-", mark).to_owned(),
        ),
        (
            "live-feed lag: p95 under 2 s",
            lag,
            v.lag_within_bar.map_or("-", mark).to_owned(),
        ),
    ];
    println!("{:<30} {:<100} holds", "bar", "measured");
    for (bar, measured, holds) in rows {
        println!(
            "{bar:<30} {:<100} {holds}",
            measured.chars().take(100).collect::<String>()
        );
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let password = cli
        .conn
        .password
        .clone()
        .context("--password (or ORCH_STRESS_PASSWORD) is needed")?;
    let rec = Arc::new(metrics::Recorder::default());
    let api = Arc::new(api::Api::new(&cli.conn.api, rec.clone())?);
    let admin = seed::admin(&api, &cli.conn.login, &password).await?;
    api.learn_routes(&admin).await;
    let mut state = state::State::load(&cli.conn.state)?;
    state.api = cli.conn.api.clone();
    let (figures, seed_args, drive_args) = match &cli.command {
        Command::Seed { figures, seed } => (figures, Some(seed), None),
        Command::Drive { figures, drive } => (figures, None, Some(drive)),
        Command::Run {
            figures,
            seed,
            drive,
        } => (figures, Some(seed), Some(drive)),
    };
    let wanted = figures.scaled();
    if let Some(s) = seed_args {
        let options = seed::SeedOptions {
            machine: s.machine.clone(),
            carrier: s.carrier.clone(),
            model_protocol: s.model_protocol.clone(),
            model_upstream: s.model_upstream.clone(),
            model_key: s.model_key.clone(),
            model: s.model.clone(),
            sandbox_mode: (s.sandbox_mode != "none").then(|| s.sandbox_mode.clone()),
            arrangement: s.arrangement.clone(),
            concurrency: s.seed_concurrency,
            orgs_per_person: s.orgs_per_person,
            password: password.clone(),
        };
        seed::seed(
            &api,
            &admin,
            &mut state,
            &cli.conn.state,
            &wanted,
            &options,
            &cli.conn.prefix,
        )
        .await?;
    }
    let Some(d) = drive_args else {
        let notes = rec.report().notes;
        if !notes.is_empty() {
            println!("seeded, with {} notes:", notes.len());
            for n in notes {
                println!("- {n}");
            }
        }
        return Ok(());
    };
    // The drive takes the first of what was seeded, up to the figures asked.
    state.threads.truncate(wanted.threads);
    let hosts_wanted = wanted.hosts;
    let made = Made {
        people: state.people.len().min(wanted.people),
        orgs: state.orgs.len(),
        projects: state.projects.len(),
        topics: state.topics.len(),
        threads_driven: if d.no_turns { 0 } else { state.threads.len() },
        docs: state.docs.len(),
        providers: state.orgs.iter().map(|o| o.providers.len()).sum(),
        agents: state.orgs.iter().map(|o| o.agents.len()).sum(),
        hosts: state.hosts.clone(),
        hosts_wanted,
        machine: state.machine.as_ref().map(|(_, name)| name.clone()),
    };
    if state.people.is_empty() || state.orgs.is_empty() {
        anyhow::bail!(
            "nothing was seeded in {}: run `orch-stress seed` first",
            cli.conn.state.display()
        );
    }
    let books = Arc::new(feed::Books {
        lag: feed::LagBook::default(),
        turns: feed::TurnBook::default(),
        rec: rec.clone(),
    });
    let options = drive::DriveOptions {
        duration: d.duration,
        browsers: wanted.people,
        searchers: wanted.searchers,
        turn_seconds: d.turn_seconds,
        pause: d.pause,
        dwell: d.dwell,
        turn_timeout: d.turn_timeout,
        password,
        ws: cli
            .conn
            .ws
            .clone()
            .unwrap_or_else(|| socket_url(&cli.conn.api)),
        turns: !d.no_turns,
        database_url: d.database_url.clone(),
        slow_ms: d.slow_ms,
        refresh_blocked: d.refresh_blocked,
    };
    let at = chrono::Utc::now().format("%Y-%m-%dT%H:%MZ").to_string();
    let database = drive::drive(api.clone(), books, Arc::new(state), admin, options).await?;
    let slow = database.slow;
    let measured = rec.report();
    let report = Report {
        api: cli.conn.api.clone(),
        at,
        window_secs: d.duration.as_secs(),
        scale: figures.scale,
        verdict: verdict(&measured, slow.as_ref(), !d.no_turns),
        made,
        measured,
        statements: slow,
        sessions: database.activity,
    };
    print(&report);
    if let Some(out) = &d.out {
        std::fs::write(out, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("writing {}", out.display()))?;
        eprintln!("the report is also in {}", out.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_socket_addresses_are_read() {
        assert_eq!(parse_duration("10m"), Ok(Duration::from_secs(600)));
        assert_eq!(parse_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert!(parse_duration("ten").is_err());
        assert_eq!(
            socket_url("http://127.0.0.1:41280/api/v1"),
            "ws://127.0.0.1:41280/api/v1/ws"
        );
        assert_eq!(
            socket_url("https://orch.example/api/v1/"),
            "wss://orch.example/api/v1/ws"
        );
    }
}
