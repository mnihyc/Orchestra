//! The waits and the pace of the engine, from the deployment's configuration.
//!
//! Where the configuration has a value ([`orch_config::ServerConfig`],
//! section `[engine]`) it is read from there; the rest are the numbers the
//! design states, named here with the reason for each, so that a test can
//! shorten them and nothing else in the engine carries a bare number.

use crate::hosting::HostingConfig;
use orch_config::ServerConfig;
use std::time::Duration;

/// How long a turn that was asked to start may take to be accepted before it
/// is failed as `start_timeout`: the deadline the design gives `turn.start`.
pub const TURN_START_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a turn whose harness took the command may go on without the harness saying that it began
/// (`turn.started`), counted from the latest moment the command could have been answered: the same minute the
/// design gives a start. A turn that never begins is given up as `start_timeout`, its input waits again, and the
/// harness is asked to stop it; without this wait a thread could stand `starting` for ever.
pub const TURN_STARTED_WAIT: Duration = Duration::from_secs(60);
/// How often the supervisor looks for what is due: a forced end after an
/// unanswered interrupt is at most this late, and so is a turn that a
/// restart left at the start. A second is short against the waits it
/// serves (30 and 60 seconds) and costs a handful of indexed reads.
pub const SWEEP_EVERY: Duration = Duration::from_secs(1);
/// How often the supervisor looks again at threads that are blocked or idle
/// with inputs waiting, and at threads blocked by a cause of their own, though
/// nothing told it that a cause cleared. Every
/// change the engine knows of re-checks at once; this is the net under the
/// ones it does not hear of, and ten seconds keeps that cheap.
pub const RECHECK_EVERY: Duration = Duration::from_secs(10);
/// How long a wake is held back so that the transaction that asked for it
/// has committed. A wake is only a hint (the sweep finds the same work), so
/// this is not a guarantee, only the usual case made prompt.
pub const POST_COMMIT_GRACE: Duration = Duration::from_millis(25);
/// How many threads one sweep takes up for each kind of work, so that a
/// system with a great many waiting threads is worked through over several
/// sweeps and no sweep holds the database for long.
pub const SWEEP_BATCH: i64 = 200;
/// How far back the supervisor looks for approvals that were decided and not
/// yet handed to their harness: a decision is handed over within a second
/// or two, so the answer to one older than this has been given or is lost
/// with its command.
pub const DECISION_LOOKBACK: Duration = Duration::from_secs(15 * 60);

/// How long the word of a plan of `manage_shared` is good for: ten minutes. Long enough to read a list and decide,
/// and short enough that a word found in an old context is no longer one.
pub const MANAGE_PLAN_KEPT: Duration = Duration::from_secs(10 * 60);
/// The seconds of a day, for what the configuration says in days.
const SECONDS_PER_DAY: u64 = 86_400;

/// The moment from which the supervisor looks for decisions and closed
/// leases that were not yet handed over.
#[must_use]
pub fn decisions_since(now: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    now - chrono::Duration::from_std(DECISION_LOOKBACK).unwrap_or_default()
}

/// What a deployment says of itself in the header of an Orch package it writes: its name, where it is reached and the
/// version of the product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    /// What the system calls itself (`[system] name`).
    pub name: String,
    /// The origin a browser opens (`[system] public_url`).
    pub url: String,
    /// The version of the product that wrote the package.
    pub version: String,
}

impl Deployment {
    /// What a deployment's configuration says of it.
    #[must_use]
    pub fn of(config: &ServerConfig) -> Self {
        Self {
            name: config.system.name.clone(),
            url: config.system.public_url.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// The waits of the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// How long an interrupt may go unanswered before the engine ends the
    /// turn itself (`[engine] interrupt_force_secs`).
    pub interrupt_force: Duration,
    /// How long `turn.start` may go unaccepted.
    pub start_timeout: Duration,
    /// How long a turn its harness took may go without `turn.started` after that ([`TURN_STARTED_WAIT`]).
    pub started_wait: Duration,
    /// How often the supervisor sweeps.
    pub sweep_every: Duration,
    /// How often blocked threads are looked at again.
    pub recheck_every: Duration,
    /// How long a wake waits for the commit it follows.
    pub post_commit_grace: Duration,
    /// How many threads of each kind a sweep takes up.
    pub sweep_batch: i64,
    /// The waits of the part that makes, stops and removes environments.
    pub hosting: HostingConfig,
    /// The shortest time between two looks at the usage caps of a running turn as its usage is reported
    /// (`[engine] cap_check_secs`).
    pub cap_check: Duration,
    /// How long the text an item gains is held before it is written to the item (`[engine] stream_flush_ms`; zero
    /// writes each piece as it comes).
    pub stream_flush: Duration,
    /// The provider-owned observation cadence, from the deployment.
    pub provider_facts: crate::provider_fact_reads::ReadConfig,
    /// How long what was deleted can be brought back before the purge takes it (`[retention] soft_delete_days`):
    /// what an agent that deletes a document is told the document can be brought back until.
    pub soft_delete: Duration,
    /// How often the configured topic/node retention policy is examined (`[schedule] retention`).
    pub retention_every: Duration,
    /// How long the word that `manage_shared` gives with a plan is good for ([`MANAGE_PLAN_KEPT`]).
    pub manage_plan_kept: Duration,
    /// How many archives of one person are written at the same time (`[limits] exports_per_user`).
    pub exports_per_user: u32,
    /// How many imports of one person run at the same time (`[limits] imports_per_user`).
    pub imports_per_user: u32,
    /// How many projects may lie one inside another on a path (`[limits] project_depth`).
    pub project_depth: u32,
    /// What the deployment says of itself in the header of an Orch package it writes.
    pub deployment: Deployment,
    /// The largest single file an archive takes in (`[limits] export_file_max_bytes`).
    pub export_file_max_bytes: i64,
    /// Where an archive is written before it is put in the store of content: the `tmp/` of the storage root, which
    /// holds scratch files of several kinds, for a program that has one; the system's directory for temporary files
    /// otherwise.
    pub scratch: Option<std::path::PathBuf>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            interrupt_force: Duration::from_secs(
                orch_config::ServerConfig::default()
                    .engine
                    .interrupt_force_secs,
            ),
            start_timeout: TURN_START_TIMEOUT,
            started_wait: TURN_STARTED_WAIT,
            sweep_every: SWEEP_EVERY,
            recheck_every: RECHECK_EVERY,
            post_commit_grace: POST_COMMIT_GRACE,
            sweep_batch: SWEEP_BATCH,
            hosting: HostingConfig::default(),
            cap_check: Duration::from_secs(
                orch_config::ServerConfig::default().engine.cap_check_secs,
            ),
            stream_flush: Duration::from_millis(
                orch_config::ServerConfig::default().engine.stream_flush_ms,
            ),
            provider_facts: crate::provider_fact_reads::ReadConfig::default(),
            soft_delete: soft_delete_of(&orch_config::ServerConfig::default()),
            retention_every: Duration::from_secs(3600),
            manage_plan_kept: MANAGE_PLAN_KEPT,
            exports_per_user: orch_config::ServerConfig::default().limits.exports_per_user,
            imports_per_user: orch_config::ServerConfig::default().limits.imports_per_user,
            project_depth: orch_config::ServerConfig::default().limits.project_depth,
            deployment: Deployment::of(&orch_config::ServerConfig::default()),
            export_file_max_bytes: i64::try_from(
                orch_config::ServerConfig::default()
                    .limits
                    .export_file_max_bytes,
            )
            .unwrap_or(i64::MAX),
            scratch: None,
        }
    }
}

/// The time something deleted is kept, as the configuration says it in days.
fn soft_delete_of(config: &ServerConfig) -> Duration {
    Duration::from_secs(
        config
            .retention
            .soft_delete_days
            .saturating_mul(SECONDS_PER_DAY),
    )
}

impl EngineConfig {
    /// The waits a deployment's configuration gives.
    #[must_use]
    pub fn from_server(config: &ServerConfig) -> Self {
        Self {
            interrupt_force: Duration::from_secs(config.engine.interrupt_force_secs),
            hosting: HostingConfig::from_server(config),
            cap_check: Duration::from_secs(config.engine.cap_check_secs),
            stream_flush: Duration::from_millis(config.engine.stream_flush_ms),
            provider_facts: crate::provider_fact_reads::ReadConfig::from_server(config),
            soft_delete: soft_delete_of(config),
            retention_every: Duration::from_secs(
                config
                    .schedule
                    .get("retention")
                    .copied()
                    .unwrap_or(3600)
                    .max(1),
            ),
            exports_per_user: config.limits.exports_per_user,
            imports_per_user: config.limits.imports_per_user,
            project_depth: config.limits.project_depth,
            deployment: Deployment::of(config),
            export_file_max_bytes: i64::try_from(config.limits.export_file_max_bytes)
                .unwrap_or(i64::MAX),
            scratch: Some(config.storage.scratch_dir()),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configured_pause_between_looks_at_the_caps_is_the_one_used() {
        let mut config = ServerConfig::default();
        config.engine.cap_check_secs = 9;
        assert_eq!(
            EngineConfig::from_server(&config).cap_check,
            Duration::from_secs(9)
        );
        assert_eq!(
            EngineConfig::default().cap_check,
            Duration::from_secs(5),
            "five seconds, as shipped"
        );
    }

    #[test]
    fn the_configured_wait_for_an_interrupt_is_the_one_used() {
        let mut config = ServerConfig::default();
        config.engine.interrupt_force_secs = 7;
        assert_eq!(
            EngineConfig::from_server(&config).interrupt_force,
            Duration::from_secs(7)
        );
        assert_eq!(
            EngineConfig::default().interrupt_force,
            Duration::from_secs(30),
            "thirty seconds, as shipped"
        );
    }
}
