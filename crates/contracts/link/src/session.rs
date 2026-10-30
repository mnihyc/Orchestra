//! Hello, authentication, readiness and machine facts.
//!
//! Every machine holds one session to a gateway: a WebSocket that the
//! machine opens. The gateway speaks first and says which protocol
//! versions it has and how it knows the machine: by the client
//! certificate of the connection, or, where the connection came through
//! a public edge that presents none, not yet. The machine answers with
//! who it is, and in the second case proves that it holds the key of its
//! certificate by signing what the gateway sent. The gateway then either
//! declares the session ready, with its limits and with how far each of
//! the machine's streams was taken in, or refuses it and closes.
//!
//! A session is kept alive by whoever has been silent, and a machine has
//! one session: a new one takes the place of the old.

use crate::policy::LocalPolicy;
use crate::version::{ByeCode, ConnectorVersions, Limits, ProtocolRange, RejectCode};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_types::{Id, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// How long a side stays silent before it sends a ping. Twenty seconds
/// keeps every proxy on the way from closing an idle socket, and costs
/// nothing worth counting.
pub const HEARTBEAT_SECS: u32 = 20;
/// How long a side hears nothing before it closes the socket: three
/// heartbeats, which is many round trips even of a slow link, so that a
/// link is given up for being dead and never for being slow.
pub const DEAD_AFTER_SECS: u32 = 60;
/// How long connecting may take before the next endpoint is tried.
pub const CONNECT_TIMEOUT_SECS: u32 = 10;
/// How long each of the three steps of the handshake may take before the
/// socket is closed and the machine tries again. Ten seconds are some
/// tens of round trips of a slow link.
pub const HANDSHAKE_STEP_TIMEOUT_SECS: u32 = 10;
/// How far a machine's clock may be from the gateway's when it signs its
/// proof: far enough for clocks nobody tends, near enough that a
/// recorded proof is soon worthless.
pub const CLOCK_SKEW_SECS: i64 = 120;
/// The bytes of the gateway's nonce.
pub const NONCE_BYTES: usize = 32;
/// The first pause before a machine connects again.
pub const RECONNECT_FIRST: Duration = Duration::from_secs(1);
/// The longest pause before a machine connects again: a machine that was
/// away is back within half a minute of its link.
pub const RECONNECT_LONGEST: Duration = Duration::from_secs(30);
/// How far a pause is moved at random, as a share of itself, so that
/// machines that lost a gateway together do not come back together.
pub const RECONNECT_JITTER: f64 = 0.2;
/// The first line of what a machine signs to prove itself. It names the
/// purpose and the version, so that the signature is good for nothing
/// else.
pub const AUTH_CONTEXT: &str = "orch-link-auth-v1";

/// What a machine does for the system.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// It keeps a provider's credential and forwards model calls.
    Provider,
    /// It hosts harness processes.
    Agent,
    /// It is an environment: a workspace, its processes, its tools.
    Environment,
    /// It is a host's node: it makes the units of managed runtimes.
    Node,
}

/// An operating system a connector runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    /// Linux.
    Linux,
    /// macOS.
    Macos,
    /// Windows.
    Windows,
}

/// A processor architecture a connector is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Arch {
    /// 64-bit x86.
    #[serde(rename = "x86_64")]
    X86_64,
    /// 64-bit ARM.
    #[serde(rename = "aarch64")]
    Aarch64,
}

/// What a machine is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Platform {
    /// Its operating system.
    pub os: Os,
    /// Its processor architecture.
    pub arch: Arch,
}

/// How a gateway knows the machine at the other end of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    /// By the client certificate the connection was made with.
    Mutual,
    /// Not by the connection, which only proves the server: the machine
    /// has to prove itself in what it sends.
    Server,
}

/// The gateway's first word on a new connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hello {
    /// The protocol versions the gateway speaks.
    pub protocol: ProtocolRange,
    /// Random bytes of this connection alone, as unpadded base64url.
    pub nonce: String,
    /// The gateway's clock.
    pub server_time: Time,
    /// The gateway's name.
    pub gateway_id: String,
    /// How the gateway knows the machine.
    pub tls: TlsMode,
}

/// The proof that a machine holds the key of its certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthProof {
    /// The machine's certificate.
    pub certificate_pem: String,
    /// The signature over [`auth_signing_input`], as unpadded base64url.
    pub signature: String,
}

/// A machine's answer to [`Hello`]: who it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Auth {
    /// The machine owner's override; absent means the authenticated server preference list is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<Vec<crate::enrollment::Endpoint>>,
    /// A node's explicit address override for its hosted units; absent follows the node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted_endpoints: Option<Vec<crate::enrollment::Endpoint>>,
    /// The machine.
    pub machine_id: Id,
    /// The protocol version it will speak.
    pub protocol_version: u32,
    /// The connector's own version.
    pub connector_version: String,
    /// What it is built for.
    pub platform: Platform,
    /// The roles it has switched on.
    pub roles: Vec<Role>,
    /// The machine's clock at the moment it answered.
    pub sent_at: Time,
    /// The proof of its key, where the connection carried no client
    /// certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<AuthProof>,
}

/// The gateway's word that the session stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ready {
    /// Current system addresses; a machine's own configured list takes precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<Vec<crate::enrollment::Endpoint>>,
    /// The session.
    pub session_id: Id,
    /// What the machine presents, for the life of this session, where it
    /// moves content beside it through an edge that sees no client
    /// certificate.
    pub session_token: String,
    /// How long a side stays silent before it sends a ping.
    pub heartbeat_secs: u32,
    /// The limits of the session.
    pub limits: Limits,
    /// For each of the machine's streams the system knows: the last
    /// sequence number it has taken in.
    pub cursors: BTreeMap<String, Seq>,
    /// The connector builds the system accepts and prefers.
    pub versions: ConnectorVersions,
}

/// The gateway's refusal of a session, after which it closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Reject {
    /// A node rejected below the accepted build may install only after this authenticated idle grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_permit: Option<UpdatePermit>,
    /// Why.
    pub code: RejectCode,
    /// A sentence for whoever reads the machine's log.
    #[serde(default)]
    pub detail: String,
    /// How long to wait before trying again, where waiting helps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u32>,
}

/// The server has held new turns and drained every unit of this authenticated node.
/// It accompanies upgrade_required only; it does not admit a session on the old executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdatePermit {
    /// The server recommendation whose verified platform build may replace the executable.
    pub recommended: String,
}

/// The gateway's word that it ends a session that was ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Bye {
    /// Why.
    pub code: ByeCode,
}

/// A question whether the other side is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Ping {
    /// A number the answer repeats, by which the round trip is measured.
    pub n: i64,
}

/// The answer to a [`Ping`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Pong {
    /// The number of the ping it answers.
    pub n: i64,
}

/// The nonce of a new connection, as it is written.
#[must_use]
pub fn encode_nonce(bytes: &[u8; NONCE_BYTES]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The bytes a machine signs to prove that it holds the key of its
/// certificate: the purpose, the gateway's nonce, the machine, the host
/// the machine connected to, and the machine's clock, each on a line of
/// its own. A signature made for one connection, one machine or one
/// host therefore proves nothing on another, and an old one nothing
/// today.
#[must_use]
pub fn auth_signing_input(nonce: &str, machine_id: Id, host: &str, sent_at: Time) -> String {
    format!("{AUTH_CONTEXT}\n{nonce}\n{machine_id}\n{host}\n{sent_at}")
}

/// Whether a proof signed at `sent_at` is fresh enough at `now`, by the
/// gateway's clock.
#[must_use]
pub fn clock_agrees(sent_at: Time, now: Time) -> bool {
    const SECOND_MS: i64 = 1_000;
    (now.unix_millis() - sent_at.unix_millis()).abs() <= CLOCK_SKEW_SECS * SECOND_MS
}

/// Whether a side that last sent something at `last_sent` owes a ping.
#[must_use]
pub fn ping_due(last_sent: Time, now: Time, heartbeat_secs: u32) -> bool {
    const SECOND_MS: i64 = 1_000;
    now.unix_millis() - last_sent.unix_millis() >= i64::from(heartbeat_secs) * SECOND_MS
}

/// Whether a side that last heard something at `last_received` gives
/// the session up.
#[must_use]
pub fn peer_silent(last_received: Time, now: Time) -> bool {
    const SECOND_MS: i64 = 1_000;
    now.unix_millis() - last_received.unix_millis() >= i64::from(DEAD_AFTER_SECS) * SECOND_MS
}

/// How long a machine waits before it connects again, having failed
/// `failures` times in a row: the first pause, doubled for each further
/// failure up to the longest, then moved by `jitter`, a number from -1
/// to 1 drawn at random, times [`RECONNECT_JITTER`] of itself.
#[must_use]
pub fn reconnect_delay(failures: u32, jitter: f64) -> Duration {
    backoff(
        RECONNECT_FIRST,
        RECONNECT_LONGEST,
        RECONNECT_JITTER,
        failures,
        jitter,
    )
}

/// A pause that starts at `first`, doubles with each failure after the
/// first up to `longest`, and is then moved by `jitter` (from -1 to 1)
/// times `spread` of itself.
pub(crate) fn backoff(
    first: Duration,
    longest: Duration,
    spread: f64,
    failures: u32,
    jitter: f64,
) -> Duration {
    let doublings = failures.saturating_sub(1).min(u32::BITS - 1);
    let base = first.saturating_mul(1u32 << doublings).min(longest);
    base.mul_f64(1.0 + spread * jitter.clamp(-1.0, 1.0))
}

/// A harness found on a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineFactsHarness {
    /// The harness kind.
    pub kind: String,
    /// Where its program is.
    pub path: String,
    /// The version found there.
    pub version: String,
    /// Whether it runs as a daemon the connector attaches to.
    pub daemon: bool,
}

/// An adapter a connector has: a harness kind it can drive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineFactsAdapter {
    /// False when the supervised adapter already supplies its own harness; omission needs a listed program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_binary: Option<bool>,
    /// The harness kind.
    pub kind: String,
    /// The version of the adapter's own contract with the system.
    pub adapter_version: u32,
    /// Whether it is part of the connector, and no program beside it.
    pub builtin: bool,
    /// Whether an unconfigured harness gets a private home made by Orch; false leaves its own default.
    #[serde(default)]
    pub default_home_owned: bool,
    /// The program's manifest, sent by an adapter that is not part of the connector, so that the system lists
    /// it, checks its builds and offers it like one it ships. A built-in adapter's is the system's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<orch_agent_programs::ProgramManifest>,
}

/// An implementation from outside that a machine's owner registered on
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineFactsPlugin {
    /// What it implements: an adapter, an executor, a translator.
    pub kind: String,
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// Whether it runs.
    pub state: String,
    /// For a provider source, what it said it is: what it reports, its fields, and what it implements.
    /// Absent for the other kinds, and until the program has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<orch_provider_contract::SourceDescriptor>,
    /// For an executor, what it said it is: its kind, its title in each language and its tool sets, so that the
    /// system can say what the machine can offer. Absent for the other kinds, and until the program has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<orch_executor_contract::descriptor::ExecutorDescriptor>,
}

/// A machine's link to its gateway, as the connector measures it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineFactsRates {
    /// The rate at which it sends.
    pub up_bps: i64,
    /// The rate at which it receives.
    pub down_bps: i64,
    /// The round trip, in milliseconds.
    pub round_trip_ms: i64,
}

/// What a machine is and has, as its connector reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MachineFacts {
    /// Whether the physical program can apply delegated role changes while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_management: Option<orch_api_contract::resources::MachineRoleCapability>,
    /// Current external-program registry, with the machine's enforcing registration rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub programs: Option<orch_api_contract::machine_programs::MachinePrograms>,
    /// Observed installation and local lifecycle commands for this exact instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation: Option<orch_api_contract::machine_programs::MachineInstallation>,
    /// Whether this machine can fetch a requested harness build, with the constraint when it cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_fetch: Option<orch_driver_contract::descriptor::OperationSupport>,
    /// Named storage capacity reported by the machine, independent of its roles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub storages: Vec<crate::events::NodeStorage>,
    /// The directory environment services this machine actually offers, declared like a driver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_descriptor: Option<orch_driver_contract::descriptor::DriverDescriptor>,
    /// Directory harness-runtime services offered by the agent role, even without an environment role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_runtime_descriptor: Option<orch_driver_contract::descriptor::DriverDescriptor>,
    /// Its host name.
    pub hostname: String,
    /// The version of its operating system.
    pub os_version: String,
    /// How many processors it has.
    pub cpus: u32,
    /// How much memory it has.
    pub memory_bytes: i64,
    /// The harnesses found on it.
    pub harnesses: Vec<MachineFactsHarness>,
    /// Programs found by search which the machine's policy has not confirmed; absent before a search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_harnesses: Option<Vec<MachineFactsHarness>>,
    /// Harness kinds included in that search, including those for which no program was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub searched_harness_kinds: Option<Vec<String>>,
    /// The harness kinds its connector can drive.
    pub adapters: Vec<MachineFactsAdapter>,
    /// The implementations from outside registered on it.
    pub plugins: Vec<MachineFactsPlugin>,
    /// The policy enforced by this machine, for its offered roles; absent only on an older report.
    #[serde(serialize_with = "crate::policy::serialize_reported")]
    pub policy: Option<LocalPolicy>,
    /// Its link to the gateway.
    pub rates: MachineFactsRates,
    /// How far its clock is ahead of the gateway's, in milliseconds.
    pub clock_offset_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    const MACHINE: &str = "0199c1a0-0000-7000-8000-00000000000a";
    const SESSION: &str = "0199c1a0-0000-7000-8000-00000000000b";

    fn time(text: &str) -> Time {
        text.parse().unwrap()
    }

    fn round_trip<T: Serialize + DeserializeOwned>(example: Value) -> T {
        let read: T = serde_json::from_value(example.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), example);
        read
    }

    #[test]
    fn the_handshake_reads_and_writes_as_the_protocol_has_it() {
        let hello: Hello = round_trip(json!({
            "protocol": { "min": 1, "max": 1 }, "nonce": encode_nonce(&[7; NONCE_BYTES]),
            "server_time": "2026-10-08T17:04:05.000Z", "gateway_id": "compose-gateway", "tls": "server"
        }));
        assert_eq!(hello.tls, TlsMode::Server);
        let auth: Auth = round_trip(json!({
            "machine_id": MACHINE, "protocol_version": 1, "connector_version": "0.1.0",
            "platform": { "os": "linux", "arch": "x86_64" }, "roles": ["agent", "environment"],
            "sent_at": "2026-10-08T17:04:05.120Z",
            "proof": { "certificate_pem": "-----BEGIN CERTIFICATE-----\n…", "signature": "c2ln" }
        }));
        assert_eq!(auth.roles, [Role::Agent, Role::Environment]);
        let ready: Ready = round_trip(json!({
            "session_id": SESSION, "session_token": "t0k3n", "heartbeat_secs": 20,
            "limits": { "max_frame_bytes": 262144, "tunnel_window_bytes": 1048576, "max_channels": 256 },
            "cursors": { "machine": 12, "env:0199c1a0-0000-7000-8000-00000000000c": 43 },
            "versions": { "min": "0.1.0", "recommended": "0.1.3" }
        }));
        assert_eq!(ready.limits, Limits::default());
        assert_eq!(ready.cursors["machine"], 12);
    }

    #[test]
    fn a_machine_known_by_its_connection_sends_no_proof() {
        let auth: Auth = round_trip(json!({
            "machine_id": MACHINE, "protocol_version": 1, "connector_version": "0.1.0",
            "platform": { "os": "macos", "arch": "aarch64" }, "roles": ["node"],
            "sent_at": "2026-10-08T17:04:05.120Z"
        }));
        assert_eq!(auth.proof, None);
    }

    #[test]
    fn a_refusal_says_why_and_when_to_try_again() {
        let busy: Reject = round_trip(
            json!({ "code": "busy", "detail": "the API is unreachable", "retry_after_secs": 5 }),
        );
        assert_eq!(busy.retry_after_secs, Some(5));
        let revoked: Reject = round_trip(json!({ "code": "revoked", "detail": "" }));
        assert_eq!(revoked.retry_after_secs, None);
        round_trip::<Bye>(json!({ "code": "replaced" }));
        round_trip::<Ping>(json!({ "n": 41 }));
        round_trip::<Pong>(json!({ "n": 41 }));
    }

    #[test]
    fn what_a_machine_signs_names_the_connection_the_machine_the_host_and_the_moment() {
        let machine: Id = MACHINE.parse().unwrap();
        let input = auth_signing_input(
            "bm9uY2U",
            machine,
            "orch.example.com",
            time("2026-10-08T17:04:05.120Z"),
        );
        assert_eq!(
            input,
            format!(
                "orch-link-auth-v1\nbm9uY2U\n{MACHINE}\norch.example.com\n2026-10-08T17:04:05.120Z"
            )
        );
        let elsewhere = auth_signing_input(
            "bm9uY2U",
            machine,
            "evil.example.com",
            time("2026-10-08T17:04:05.120Z"),
        );
        assert_ne!(input, elsewhere);
    }

    #[test]
    fn a_nonce_is_written_without_padding() {
        let nonce = encode_nonce(&[0xff; NONCE_BYTES]);
        assert_eq!(nonce.len(), 43);
        assert!(!nonce.contains('='));
    }

    #[test]
    fn a_proof_signed_within_two_minutes_of_the_gateways_clock_is_fresh() {
        let now = time("2026-10-08T17:04:05.000Z");
        assert!(clock_agrees(time("2026-10-08T17:02:05.000Z"), now));
        assert!(clock_agrees(time("2026-10-08T17:06:05.000Z"), now));
        assert!(!clock_agrees(time("2026-10-08T17:02:04.999Z"), now));
        assert!(!clock_agrees(time("2026-10-08T17:06:05.001Z"), now));
    }

    #[test]
    fn a_side_that_has_been_silent_for_a_heartbeat_pings() {
        let last = time("2026-10-08T17:04:00.000Z");
        assert!(!ping_due(
            last,
            time("2026-10-08T17:04:19.999Z"),
            HEARTBEAT_SECS
        ));
        assert!(ping_due(
            last,
            time("2026-10-08T17:04:20.000Z"),
            HEARTBEAT_SECS
        ));
    }

    #[test]
    fn a_side_that_has_heard_nothing_for_a_minute_gives_up() {
        let last = time("2026-10-08T17:04:00.000Z");
        assert!(!peer_silent(last, time("2026-10-08T17:04:59.999Z")));
        assert!(peer_silent(last, time("2026-10-08T17:05:00.000Z")));
    }

    #[test]
    fn pauses_before_connecting_again_double_from_a_second_to_half_a_minute() {
        let pauses: Vec<u64> = (1..=7)
            .map(|failures| reconnect_delay(failures, 0.0).as_secs())
            .collect();
        assert_eq!(pauses, [1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(reconnect_delay(u32::MAX, 0.0), RECONNECT_LONGEST);
    }

    #[test]
    fn a_pause_is_moved_by_at_most_a_fifth_of_itself() {
        assert_eq!(reconnect_delay(3, 1.0), Duration::from_millis(4_800));
        assert_eq!(reconnect_delay(3, -1.0), Duration::from_millis(3_200));
        assert_eq!(reconnect_delay(3, 9.0), Duration::from_millis(4_800));
    }

    #[test]
    fn a_machine_reports_what_it_is_with_the_limits_its_owner_set() {
        let policy = serde_json::to_value(LocalPolicy::default()).unwrap();
        let facts: MachineFacts = round_trip(json!({
            "hostname": "alice-laptop", "os_version": "Debian 13", "cpus": 8, "memory_bytes": 16000000000u64,
            "harnesses": [ { "kind": "codex", "path": "/usr/local/bin/codex", "version": "0.161.0", "daemon": true } ],
            "adapters": [ { "kind": "codex", "adapter_version": 1, "builtin": true, "default_home_owned": false } ],
            "plugins": [ { "kind": "adapter", "name": "acme", "version": "0.3.0", "state": "running" } ],
            "policy": policy,
            "rates": { "up_bps": 4100000, "down_bps": 9000000, "round_trip_ms": 42 },
            "clock_offset_ms": -12
        }));
        assert_eq!(facts.policy, Some(LocalPolicy::default()));
        assert_eq!(facts.clock_offset_ms, -12);
        assert_eq!(
            facts.plugins[0].descriptor, None,
            "only a provider source says what it is"
        );
    }

    #[test]
    fn an_older_adapter_report_defaults_home_ownership_without_omitting_the_false_fact()
    -> Result<(), Box<dyn std::error::Error>> {
        let old = json!({"kind":"codex","adapter_version":1,"builtin":true});
        let read: MachineFactsAdapter = serde_json::from_value(old)?;
        assert!(!read.default_home_owned);
        assert_eq!(
            serde_json::to_value(read)?,
            json!({"kind":"codex","adapter_version":1,"builtin":true,"default_home_owned":false})
        );
        Ok(())
    }

    #[test]
    fn a_machine_reports_each_provider_program_with_what_it_said_it_is() {
        let plugin: MachineFactsPlugin = round_trip(json!({
            "kind": "provider", "name": "acme-plan", "version": "1.0.0", "state": "running",
            "descriptor": {
                "name": "acme-plan", "title": "Acme plan", "summary": "Reads the Acme plan.", "contract": 1,
                "facts": { "reports": ["windows", "balances"], "spends_resets": false, "credential": "none",
                           "min_interval_secs": null, "plans": {} },
                "models": null, "presents": null, "fields": [], "docs_url": null
            }
        }));
        let descriptor = plugin.descriptor.unwrap();
        assert_eq!(descriptor.reports().len(), 2);
        assert!(!descriptor.lists_models());
    }

    #[test]
    fn a_machine_reports_each_executor_program_with_the_tool_sets_it_said_it_has() {
        let plugin: MachineFactsPlugin = round_trip(json!({
            "kind": "executor", "name": "acme-tools", "version": "1.0.0", "state": "running",
            "executor": {
                "kind": "acme-exec", "title": "Acme tools", "titles": { "zh-CN": "Acme 工具" },
                "protocol": "mcp", "tool_sets": [ { "name": "all", "summary": "Everything it has." } ],
                "sandbox_modes": ["open"], "attribution": null
            }
        }));
        let executor = plugin.executor.unwrap();
        assert_eq!(executor.kind, "acme-exec");
        assert_eq!(executor.tool_sets[0].name, "all");
    }

    #[test]
    fn a_machine_the_system_made_reports_no_policy_of_its_own() {
        let facts: MachineFacts = round_trip(json!({
            "hostname": "orch-environment-0199abcd1234", "os_version": "Debian 13", "cpus": 4,
            "memory_bytes": 8589934592u64, "harnesses": [], "adapters": [], "plugins": [], "policy": null,
            "rates": { "up_bps": 0, "down_bps": 0, "round_trip_ms": 1 }, "clock_offset_ms": 0
        }));
        assert_eq!(facts.policy, None);
    }
}
