//! What the tests with real programs share: the scripted plugin, a clock that is moved by hand, and a directory.
//!
//! Time in these tests is a [`ManualClock`]: the host judges pings, deadlines and pauses by it, so a plugin that
//! must be silent for thirty seconds is silent for as long as it takes the test to move the clock, on a machine
//! that is busy or not. Real time passes only for the programs to start and to answer.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is shared")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use orch_plugin::supervisor::{Options, Timing};
use orch_plugin_contract::{PluginKind, PluginRegistration};
use orch_types::{Json, ManualClock};
use serde_json::json;

/// How often the supervisor looks at the clock in these tests: a few milliseconds, so that moving the clock is
/// seen at once.
pub const TICK: Duration = Duration::from_millis(5);

/// The longest a test waits for something to happen in real time. Programs start in a few milliseconds;
/// this is for a machine that is very busy.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// The python of this machine.
pub const PYTHON: &str = "python3";

/// The directory of the package.
pub fn package() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The scripted plugin.
pub fn scripted() -> String {
    package()
        .join("tests/support/scripted_plugin.py")
        .to_string_lossy()
        .into_owned()
}

/// A file of the example plugins of the repository.
pub fn example(directory: &str, file: &str) -> String {
    package()
        .join("../../../examples/plugins")
        .join(directory)
        .join(file)
        .to_string_lossy()
        .into_owned()
}

/// A clock that stands at a fixed moment until it is moved.
pub fn clock() -> ManualClock {
    ManualClock::new(Utc.with_ymd_and_hms(2026, 10, 11, 10, 0, 0).unwrap())
}

/// Options for a host on `clock`.
pub fn options(clock: &ManualClock) -> Options {
    Options::new("0.1.0")
        .with_clock(Arc::new(clock.clone()))
        .with_timing(Timing { tick: TICK })
}

/// Moves the clock forward by `seconds`.
pub fn advance(clock: &ManualClock, seconds: i64) {
    clock.advance(chrono::Duration::seconds(seconds));
}

/// Waits, in real time, until `ready` holds.
pub async fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < PATIENCE, "gave up waiting until {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Lets the host's tasks run for a moment of real time, so that something that must not happen has had its chance.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(80)).await;
}

/// Whether a process of this number still exists. Nothing is sent to it.
pub fn alive(pid: u64) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// A directory of a test: the script of the plugin, its log, whatever else the test needs.
pub struct Work {
    dir: tempfile::TempDir,
}

impl Work {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn text(&self, name: &str) -> String {
        self.path(name).to_string_lossy().into_owned()
    }

    /// Writes the script of the plugin and returns where.
    pub fn script(&self, script: &Json) -> String {
        std::fs::write(self.path("script.json"), script.to_string()).unwrap();
        self.text("script.json")
    }

    /// Everything the scripted plugin recorded: its starts and the lines it received.
    pub fn records(&self) -> Vec<Json> {
        std::fs::read_to_string(self.path("log.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The process numbers of the starts of the scripted plugin, in order.
    pub fn pids(&self) -> Vec<u64> {
        self.records()
            .iter()
            .filter_map(|record| record["started"].as_u64())
            .collect()
    }

    /// The requests of `method` the plugin received.
    pub fn received(&self, method: &str) -> Vec<Json> {
        self.records()
            .into_iter()
            .filter_map(|record| record.get("received").cloned())
            .filter(|line| line["method"] == method)
            .collect()
    }
}

/// The registration of the scripted plugin of `kind` in the directory `work`, started with the `script` and the
/// further arguments `args`.
pub fn registration(
    kind: PluginKind,
    name: &str,
    work: &Work,
    script: &Json,
    args: &[&str],
) -> PluginRegistration {
    let mut command = vec![
        PYTHON.to_owned(),
        scripted(),
        "--script".to_owned(),
        work.script(script),
        "--log".to_owned(),
        work.text("log.jsonl"),
    ];
    command.extend(args.iter().map(|arg| (*arg).to_owned()));
    PluginRegistration {
        kind,
        name: name.to_owned(),
        command: Some(command),
        connect: None,
        env: BTreeMap::new(),
        restart: None,
    }
}

/// The descriptor of each kind, the smallest that is consistent.
pub fn descriptor(kind: PluginKind) -> Json {
    match kind {
        PluginKind::Summarizer => json!({
            "name": "scripted", "title": "Scripted", "config_schema": {},
            "fields": ["title", "description", "tags"]
        }),
        PluginKind::Driver => json!({
            "name": "scripted", "version": "1", "title": "A scripted driver",
            "spec_schemas": { "environment": {}, "provider": {} },
            "confinement": "none", "recreate": "restart", "process_scope": "started"
        }),
        PluginKind::Executor => json!({
            "kind": "scripted-exec", "title": "A scripted executor", "protocol": "harness",
            "sandbox_modes": ["open"]
        }),
        PluginKind::Translator => json!({
            "from": "alpha", "to": "beta", "title": "Alpha to Beta",
            "summary": "Speaks Beta to an Alpha harness.",
            "losses": { "preserved": ["messages"], "approximated": [], "dropped": ["images"] }
        }),
        PluginKind::Notifier => json!({
            "name": "scripted", "title": "Scripted", "summary": "Does what the script says.",
            "targets": ["destination"], "fields": [], "owners": [], "single": false,
            "reaches": "channel", "body": "text", "max_chars": 4096,
            "templates": { "*": { "single": { "subject": null, "body": "{{title}}" },
                                  "digest": { "subject": null, "body": "{{#notifications}}{{title}}\n{{/notifications}}" } } }
        }),
        PluginKind::Adapter => json!({
            "kind": "tiny", "adapter_version": 1,
            "harness": { "name": "Tiny", "version": "1.0.0", "supported_range": ">=1.0.0", "status": "supported" },
            "arrangements": [ { "type": "together" } ],
            "approval_modes": ["auto"], "sandbox_modes": ["open"]
        }),
        PluginKind::Provider => json!({
            "name": "scripted", "title": "Scripted", "summary": "Tells what the script says.", "contract": 1,
            "facts": { "reports": ["plan", "windows"] }, "models": {}
        }),
    }
}

/// A script for a plugin of `kind` that declares `methods` and answers as `responses` say.
pub fn script(kind: PluginKind, methods: &[&str], responses: &Json) -> Json {
    json!({
        "kind": kind.as_str(), "name": "scripted", "version": "0.3.0",
        "methods": methods, "descriptor": descriptor(kind), "responses": responses
    })
}

/// The log lines the host wrote, apart from its own, captured so that a test can look at them.
#[derive(Debug, Clone)]
pub struct Logged {
    pub target: String,
    pub level: tracing::Level,
    pub fields: BTreeMap<String, String>,
}

static LOGGED: Mutex<Vec<Logged>> = Mutex::new(Vec::new());

struct Capture;

#[derive(Default)]
struct Fields(BTreeMap<String, String>);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        LOGGED.lock().unwrap().push(Logged {
            target: event.metadata().target().to_owned(),
            level: *event.metadata().level(),
            fields: fields.0,
        });
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Starts capturing what the host logs, for the whole of this test program.
pub fn capture_logs() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Another subscriber may be set already by the harness; then nothing is captured and the test says so.
        let _ = tracing::subscriber::set_global_default(Capture);
    });
}

/// What was logged by the plugin named `plugin`.
pub fn logged_by(plugin: &str) -> Vec<Logged> {
    LOGGED
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.fields.get("plugin").is_some_and(|name| name == plugin))
        .cloned()
        .collect()
}
