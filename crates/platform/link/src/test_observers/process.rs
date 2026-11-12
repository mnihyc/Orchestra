//! QA-only process lifetime. This module is absent with the parent observer feature off.
//!
//! An explicit output mount and enrolled-machine identity are supplied by the
//! owned test invocation. No product config, listener, payload or credential is read.

use super::{Recording, Snapshot, lock};
use orch_types::Id;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Options {
    directory: PathBuf,
    run: String,
    machine: Id,
    source_claim: String,
    max_commands: usize,
}

fn invalid(detail: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, detail.into())
}

impl Options {
    fn from_env() -> io::Result<Option<Self>> {
        let Some(directory) = std::env::var_os("ORCH_TEST_WSS_OBSERVER_DIR") else {
            return Ok(None);
        };
        let read = |name: &str| {
            std::env::var(name).map_err(|_| invalid(format!("missing QA observer input {name}")))
        };
        let options = Self {
            directory: PathBuf::from(directory),
            run: read("ORCH_TEST_WSS_RUN")?,
            machine: read("ORCH_TEST_WSS_MACHINE")?
                .parse()
                .map_err(|_| invalid("invalid QA machine identity"))?,
            source_claim: read("ORCH_TEST_WSS_SOURCE")?,
            // Instrumentation budget supplied by the finite run plan, not a product/latency limit.
            max_commands: read("ORCH_TEST_WSS_MAX_COMMANDS")?
                .parse()
                .map_err(|_| invalid("invalid QA command budget"))?,
        };
        options.validate()?;
        Ok(Some(options))
    }

    fn validate(&self) -> io::Result<()> {
        if self.run.is_empty()
            || !self
                .run
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            || self.machine.is_nil()
            || self.max_commands == 0
            || self.source_claim.len() != 40
            || !self
                .source_claim
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid(
                "QA run, machine, source claim and command budget must be explicit",
            ));
        }
        if !self.directory.is_absolute()
            || !self.directory.is_dir()
            || self.directory.canonicalize()? != self.directory
            || self.directory.ancestors().any(|path| path.is_symlink())
        {
            return Err(invalid(
                "QA output must be an existing canonical owned directory",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct Identity {
    run: String,
    component: String,
    process: u32,
    instance: Id,
    machine: Id,
    build: serde_json::Value,
    executable_blake3: String,
    executable_sha256: String,
    /// This comes from the test invocation, not from the compiled binary.
    declared_source_commit: String,
    max_commands: usize,
    max_queues: usize,
}

fn executable_hash() -> io::Result<(String, String)> {
    let mut file = std::fs::File::open(std::env::current_exe()?)?;
    let mut hasher = blake3::Hasher::new();
    let mut sha256 = Sha256::new();
    let mut bytes = [0u8; 64 * 1024]; // Bounded file-hash buffer; never retains executable bytes.
    loop {
        let read = file.read(&mut bytes)?;
        if read == 0 {
            break;
        }
        hasher.update(&bytes[..read]);
        sha256.update(&bytes[..read]);
    }
    Ok((
        hasher.finalize().to_hex().to_string(),
        hex::encode(sha256.finalize()),
    ))
}

/// Exactly the existing four workload phases plus idle and final shutdown.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    run: String,
    id: Id,
    phase: String,
    boundary: String,
}

impl Checkpoint {
    fn valid(&self, run: &str) -> bool {
        self.run == run
            && !self.id.is_nil()
            && matches!(
                (self.phase.as_str(), self.boundary.as_str()),
                ("idle", "start")
                    | ("shutdown", "pre_stop")
                    | (
                        "concurrent" | "cut" | "reconnect" | "floor",
                        "start" | "drained"
                    )
            )
    }
}

fn open_drained(snapshot: &Snapshot) -> bool {
    !snapshot.overflowed
        && snapshot.queues.iter().any(|queue| !queue.receiver_closed)
        && snapshot.queues.iter().all(|queue| {
            queue.drain_or_retired()
                && (!queue.receiver_closed
                    || snapshot
                        .queues
                        .iter()
                        .any(|active| !active.receiver_closed && active.label == queue.label))
        })
}

fn checkpoint(directory: &std::path::Path, run: &str) -> io::Result<Option<Checkpoint>> {
    let path = directory.join("checkpoint-request.json");
    if !path.exists() {
        return Ok(None);
    }
    if path.is_symlink()
        || path.metadata()?.len() > orch_api_contract::common::MAX_JSON_BYTES as u64
    {
        return Err(invalid("invalid QA checkpoint file"));
    }
    let request: Checkpoint =
        serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)?;
    if !request.valid(run) {
        return Err(invalid("QA checkpoint is outside this finite run"));
    }
    Ok(Some(request))
}

type Stop = Arc<(Mutex<Option<bool>>, Condvar)>;

/// A dedicated QA process recorder. Early exit records an abandoned scope;
/// only explicit finish acknowledges the composition returned normally.
pub struct ProcessRecording {
    _recording: Recording,
    stop: Stop,
    writer: Option<JoinHandle<io::Result<()>>>,
}

impl ProcessRecording {
    pub fn from_env(component: &str, build: &impl Serialize) -> io::Result<Option<Self>> {
        let Some(options) = Options::from_env()? else {
            return Ok(None);
        };
        Self::start(component, build, options).map(Some)
    }

    fn start(component: &str, build: &impl Serialize, options: Options) -> io::Result<Self> {
        options.validate()?;
        if !matches!(component, "api" | "gateway" | "connector") {
            return Err(invalid("unknown QA observer process"));
        }
        let max_queues = orch_link_contract::version::Limits::default().max_channels as usize;
        let recording = Recording::for_machine(options.machine, options.max_commands, max_queues)
            .map_err(invalid)?;
        let (executable_blake3, executable_sha256) = executable_hash()?;
        let identity = Identity {
            run: options.run,
            component: component.into(),
            process: std::process::id(),
            instance: orch_types::new_id(),
            machine: options.machine,
            build: serde_json::to_value(build).map_err(io::Error::other)?,
            executable_blake3,
            executable_sha256,
            declared_source_commit: options.source_claim,
            max_commands: options.max_commands,
            max_queues,
        };
        let output = options
            .directory
            .join(format!("{}-{}.json", component, identity.instance));
        // Reserve a new path; never overwrite an earlier invocation's evidence.
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        let stop: Stop = Arc::new((Mutex::new(None), Condvar::new()));
        let stopped = Arc::clone(&stop);
        let recorder = Arc::clone(&recording.recorder);
        let writer = thread::Builder::new().name("qa-wss-observer".into()).spawn(move || {
            let started = Instant::now();
            let mut checkpoints = BTreeMap::<Id, Checkpoint>::new();
            loop {
                let end = *lock(&stopped.0);
                let observations = recorder.snapshot();
                let snapshot = serde_json::json!({
                    "identity":&identity, "scope":"explicit_test_observers_qa_process",
                    "elapsed_micros":started.elapsed().as_micros(),
                    "state": match end { Some(true)=>"finished", Some(false)=>"abandoned", None=>"running" },
                    "clock_domain":identity.instance,
                    "observations":&observations, "complete_wss_release_gate":false,
                });
                // Atomic replacement of this invocation's own snapshot; ordinary
                // reader sees either a complete previous sample or a complete new one.
                let mut pending = tempfile::NamedTempFile::new_in(&options.directory)?;
                serde_json::to_writer(&mut pending, &snapshot).map_err(io::Error::other)?;
                pending.write_all(b"\n")?;
                pending.flush()?;
                pending.persist(&output).map_err(|error| error.error)?;
                if end.is_none() && let Some(request) = checkpoint(&options.directory, &identity.run)? {
                    if let Some(previous) = checkpoints.get(&request.id) {
                        if previous != &request { return Err(invalid("QA checkpoint identity was reused")); }
                    } else {
                        // Four start/drain pairs, one idle start and one pre-stop snapshot.
                        if checkpoints.len() >= 2 * 4 + 2 { return Err(invalid("QA checkpoint budget exceeded")); }
                        let drained = open_drained(&observations);
                        if request.boundary == "start" || drained {
                            let mut record = snapshot.clone();
                            record["checkpoint"] = serde_json::to_value(&request).map_err(io::Error::other)?;
                            record["open_drained"] = serde_json::json!(drained);
                            let target = options.directory.join(format!("checkpoint-{}-{}-{}.json", request.id, identity.component, identity.instance));
                            let mut file = tempfile::NamedTempFile::new_in(&options.directory)?;
                            serde_json::to_writer(&mut file, &record).map_err(io::Error::other)?;
                            file.write_all(b"\n")?;
                            file.flush()?;
                            file.persist_noclobber(target).map_err(|error| error.error)?;
                            checkpoints.insert(request.id, request);
                        }
                    }
                }
                if end.is_some() { return Ok(()); }
                let state = lock(&stopped.0);
                if state.is_none() {
                    // Existing heartbeat cadence is sampling density, not an SLA.
                    drop(stopped.1.wait_timeout(state, Duration::from_secs(u64::from(orch_link_contract::session::HEARTBEAT_SECS)))
                        .unwrap_or_else(std::sync::PoisonError::into_inner));
                }
            }
        })?;
        Ok(Self {
            _recording: recording,
            stop,
            writer: Some(writer),
        })
    }

    fn join(&mut self, complete: bool) -> io::Result<()> {
        *lock(&self.stop.0) = Some(complete);
        self.stop.1.notify_all();
        if let Some(writer) = self.writer.take() {
            writer
                .join()
                .map_err(|_| io::Error::other("QA observer writer panicked"))??;
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<()> {
        self.join(true)
    }
}

impl Drop for ProcessRecording {
    fn drop(&mut self) {
        if self.writer.is_some() {
            if let Err(error) = self.join(false) {
                eprintln!("QA WSS observation incomplete: {error}");
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "QA lifecycle controls fail by panicking"
)]
mod tests {
    use super::*;

    #[test]
    fn open_phase_drain_requires_conservation_and_refuses_unwitnessed_shutdown() {
        let mut snapshot = Snapshot {
            commands: Vec::new(),
            queues: vec![super::super::QueueSnapshot {
                label: "actual.queue",
                enqueue_attempts: 3,
                selected: 3,
                sent: 1,
                processed: 2,
                ..super::super::QueueSnapshot::default()
            }],
            overflowed: false,
            actual_connector_machine: None,
        };
        assert!(open_drained(&snapshot));
        snapshot.queues[0].in_flight_payload_bytes = 1;
        assert!(!open_drained(&snapshot));
        snapshot.queues[0].in_flight_payload_bytes = 0;
        snapshot.queues[0].processed = 1;
        assert!(!open_drained(&snapshot));
        snapshot.queues[0].processed = 2;
        snapshot.queues[0].receiver_closed = true;
        assert!(!open_drained(&snapshot));
        snapshot.queues[0].receiver_closed = false;
        snapshot.queues[0].unknown_sizes_seen = 1;
        assert!(!open_drained(&snapshot));
    }

    #[test]
    fn clean_retired_history_needs_an_active_replacement_and_keeps_failures() {
        let queue = super::super::QueueGauge::new("actual.queue");
        queue.enqueue(Some(7));
        queue.selected(Some(7)).processed();
        drop(queue.receiver());
        let mut snapshot = Snapshot {
            commands: vec![],
            queues: vec![queue.snapshot()],
            overflowed: false,
            actual_connector_machine: None,
        };
        assert!(!open_drained(&snapshot));
        snapshot.queues.push(super::super::QueueSnapshot {
            label: "actual.queue",
            ..super::super::QueueSnapshot::default()
        });
        assert!(open_drained(&snapshot));
        snapshot.queues[0].discarded_on_receiver_close = 1;
        assert!(!open_drained(&snapshot));
    }

    #[test]
    fn requested_checkpoint_is_immutable_and_correlated_to_this_running_instance() {
        let _serial = lock(&super::super::RECORDING_TEST_LOCK);
        let directory = tempfile::tempdir().unwrap();
        let request = Checkpoint {
            run: "ownedqa".into(),
            id: orch_types::new_id(),
            phase: "concurrent".into(),
            boundary: "start".into(),
        };
        std::fs::write(
            directory.path().join("checkpoint-request.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        let observer = ProcessRecording::start(
            "api",
            &serde_json::json!({"component":"api"}),
            options(directory.path().canonicalize().unwrap()),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30); // Existing QA observation hang guard.
        let record = loop {
            let completed = std::fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(&format!("checkpoint-{}-", request.id))
                });
            if let Some(path) = completed {
                break std::fs::read(path.path()).unwrap();
            }
            assert!(
                Instant::now() < deadline,
                "the running writer did not acknowledge its actual request"
            );
            std::thread::yield_now();
        };
        observer.finish().unwrap();
        let record: serde_json::Value = serde_json::from_slice(&record).unwrap();
        assert_eq!(
            record["checkpoint"],
            serde_json::to_value(&request).unwrap()
        );
        assert_eq!(record["state"], "running");
        assert_eq!(record["clock_domain"], record["identity"]["instance"]);
        assert_eq!(record["complete_wss_release_gate"], false);
    }

    fn options(directory: PathBuf) -> Options {
        Options {
            directory,
            run: "ownedqa".into(),
            machine: orch_types::new_id(),
            source_claim: "a".repeat(40),
            max_commands: 2,
        }
    }

    #[test]
    fn process_snapshot_distinguishes_explicit_finish_and_abandoned_lifetime() {
        let _serial = lock(&super::super::RECORDING_TEST_LOCK);
        for finished in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let observer = ProcessRecording::start(
                "api",
                &serde_json::json!({"component":"api"}),
                options(directory.path().canonicalize().unwrap()),
            )
            .unwrap();
            if finished {
                observer.finish().unwrap();
            } else {
                drop(observer);
            }
            let files: Vec<_> = std::fs::read_dir(directory.path())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            assert_eq!(files.len(), 1);
            let record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
            assert_eq!(
                record["state"],
                if finished { "finished" } else { "abandoned" }
            );
            assert_eq!(record["complete_wss_release_gate"], false);
            assert_eq!(record["identity"]["declared_source_commit"], "a".repeat(40));
            assert_eq!(
                record["identity"]["executable_blake3"]
                    .as_str()
                    .unwrap()
                    .len(),
                64
            );
            assert_eq!(record["clock_domain"], record["identity"]["instance"]);
        }
    }

    #[test]
    fn process_scope_requires_real_machine_and_explicit_finite_budget() {
        let directory = tempfile::tempdir().unwrap();
        let mut input = options(directory.path().canonicalize().unwrap());
        input.max_commands = 0;
        assert!(input.validate().is_err());
        input.max_commands = 1;
        input.machine = Id::nil();
        assert!(input.validate().is_err());
        input.machine = orch_types::new_id();
        input.directory = PathBuf::from("relative-output");
        assert!(input.validate().is_err());
    }
}
