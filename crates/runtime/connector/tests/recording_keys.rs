//! Real connector commands keep capture ordered while a controlled content endpoint holds publication.
mod rig_environment;

use async_trait::async_trait;
use bytes::Bytes;
use orch_link::commands::Progress;
use orch_link::content::{BlobEndpoint, PutRefusal};
use orch_link::journal::{Entry, Step};
use orch_link_contract::commands::{CommandBody, EnvDetach, FsList, FsWrite, SerialKey};
use orch_link_contract::content::{BlobProgress, PutQuery};
use orch_link_contract::source::{
    EnvironmentDataPart, FsDataImport, FsRecord, FsRecordRead, FsRecordResult, RecordKind,
    RecordState,
};
use orch_testkit::doubles::link::ShapedBlobs;
use orch_types::{Hash, Id, Json, new_id};
use rig_environment::{EnvRig, PATIENCE};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{Notify, Semaphore};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Pause {
    entered: Semaphore,
    changed: Notify,
    open: AtomicBool,
    calls: AtomicUsize,
}
impl Pause {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            changed: Notify::new(),
            open: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        }
    }
    async fn pass(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.open.load(Ordering::SeqCst) {
                return;
            }
            changed.await;
        }
    }
    async fn entered(&self, rig: &EnvRig, command: Id, phase: &'static str) -> TestResult {
        eprintln!("recording_keys phase begin: {phase}");
        tokio::time::timeout(PATIENCE, self.entered.acquire())
            .await
            .map_err(|_| {
                phase_timeout(
                    rig,
                    phase,
                    &[command],
                    &format!("HEAD calls={}", self.calls.load(Ordering::SeqCst)),
                )
            })??
            .forget();
        eprintln!("recording_keys phase complete: {phase}");
        Ok(())
    }
    fn release(&self) {
        self.open.store(true, Ordering::SeqCst);
        self.changed.notify_waiters();
    }
}

struct Endpoint {
    inner: ShapedBlobs,
    held: Mutex<BTreeMap<Id, Arc<Pause>>>,
}
impl Endpoint {
    fn hold(&self, id: Id) -> Arc<Pause> {
        let pause = Arc::new(Pause::new());
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::clone(&pause));
        pause
    }
}

#[async_trait]
impl BlobEndpoint for Endpoint {
    async fn head(&self, command: Id, hash: &Hash) -> Result<Option<BlobProgress>, PutRefusal> {
        let pause = {
            self.held
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&command)
                .cloned()
        };
        if let Some(pause) = pause {
            pause.pass().await;
        }
        self.inner.head(command, hash).await
    }
    async fn put(
        &self,
        command: Id,
        hash: &Hash,
        query: PutQuery,
        body: Bytes,
    ) -> Result<BlobProgress, PutRefusal> {
        self.inner.put(command, hash, query, body).await
    }
    async fn get(
        &self,
        command: Id,
        hash: &Hash,
        offset: i64,
        len: i64,
    ) -> Result<Bytes, PutRefusal> {
        self.inner.get(command, hash, offset, len).await
    }
    async fn missing(&self, command: Id, hashes: &[Hash]) -> Result<Vec<Hash>, PutRefusal> {
        self.inner.missing(command, hashes).await
    }
}

fn fixture() -> (EnvRig, Arc<Endpoint>) {
    let mut rig = EnvRig::new();
    let endpoint = Arc::new(Endpoint {
        inner: ShapedBlobs::new(Arc::clone(&rig.blobs), None, i64::MAX),
        held: Mutex::new(BTreeMap::new()),
    });
    rig.blob_endpoint = Some(endpoint.clone());
    (rig, endpoint)
}

fn request(environment: Id) -> FsRecord {
    FsRecord {
        environment_id: environment,
        snapshot_id: new_id(),
        directory: String::new(),
        kind: RecordKind::Turn,
        record_days: 1,
        max_bytes: 1024 * 1024,
    }
}

fn issue<B: CommandBody>(rig: &EnvRig, id: Id, environment: Id, body: &B) {
    rig.blobs.open_command(id);
    rig.controller.issue_with_id(
        id,
        &SerialKey::Environment(environment),
        rig.deadline(),
        body,
    );
}

fn command_state(rig: &EnvRig, id: Id) -> String {
    let progress = match rig.controller.progress(id) {
        None => "absent".to_owned(),
        Some(Progress::Queued) => "queued".to_owned(),
        Some(Progress::Sent) => "sent".to_owned(),
        Some(Progress::Accepted) => "accepted".to_owned(),
        Some(Progress::Succeeded(_)) => "succeeded".to_owned(),
        Some(Progress::Failed(error)) => format!("failed({})", error.code),
        Some(Progress::Expired) => "expired".to_owned(),
        Some(Progress::Cancelled) => "cancelled".to_owned(),
    };
    let journal = match journal(rig, id).map(|entry| entry.step) {
        Ok(Step::Accepted) => "accepted".to_owned(),
        Ok(Step::Started) => "started".to_owned(),
        Ok(Step::Done { result }) => match result.outcome {
            Ok(_) => "done(success)".to_owned(),
            Err(error) => format!("done({})", error.code),
        },
        Err(error) => format!("unreadable({error})"),
    };
    format!("{id}: controller={progress}, journal={journal}")
}

fn phase_timeout(
    rig: &EnvRig,
    phase: &str,
    commands: &[Id],
    detail: &str,
) -> Box<dyn std::error::Error + Send + Sync> {
    let states = commands
        .iter()
        .map(|id| command_state(rig, *id))
        .collect::<Vec<_>>()
        .join("; ");
    let reports = rig
        .controller
        .reports()
        .iter()
        .filter(|kind| *kind == "fs.changed")
        .count();
    format!("recording_keys phase '{phase}' timed out after {PATIENCE:?}; {states}; fs.changed reports={reports}; {detail}").into()
}

async fn progress(
    rig: &EnvRig,
    id: Id,
    phase: &'static str,
    related: &[Id],
) -> TestResult<Progress> {
    eprintln!("recording_keys phase begin: {phase}");
    let answer = tokio::time::timeout(PATIENCE, rig.controller.wait_command(id))
        .await
        .map_err(|_| {
            let mut commands = vec![id];
            commands.extend_from_slice(related);
            phase_timeout(rig, phase, &commands, "waiting for command result")
        })?;
    eprintln!("recording_keys phase complete: {phase}");
    Ok(answer)
}

async fn answer(rig: &EnvRig, id: Id, phase: &'static str, related: &[Id]) -> TestResult<Json> {
    match progress(rig, id, phase, related).await? {
        Progress::Succeeded(answer) => Ok(answer),
        other => Err(
            format!("recording_keys phase '{phase}': command did not succeed: {other:?}").into(),
        ),
    }
}

async fn send_as<B: CommandBody>(
    rig: &EnvRig,
    id: Id,
    body: &B,
    phase: &'static str,
) -> TestResult<Progress> {
    // Preserve the rig's catalogue key (notably unkeyed fs.record.read), wire deadline and content authorization.
    rig.blobs.open_command(id);
    rig.controller
        .issue_with_id(id, &rig.key_of::<B>(), rig.deadline(), body);
    progress(rig, id, phase, &[]).await
}

async fn succeed<B: CommandBody>(rig: &EnvRig, body: &B, phase: &'static str) -> TestResult<Json> {
    match send_as(rig, new_id(), body, phase).await? {
        Progress::Succeeded(answer) => Ok(answer),
        other => Err(
            format!("recording_keys phase '{phase}': command did not succeed: {other:?}").into(),
        ),
    }
}

async fn failure<B: CommandBody>(
    rig: &EnvRig,
    body: &B,
    phase: &'static str,
) -> TestResult<(String, String)> {
    match send_as(rig, new_id(), body, phase).await? {
        Progress::Failed(error) => Ok((error.code, error.detail)),
        other => {
            Err(format!("recording_keys phase '{phase}': command did not fail: {other:?}").into())
        }
    }
}

async fn stop(rig: &mut EnvRig, phase: &'static str, commands: &[Id]) -> TestResult {
    eprintln!("recording_keys phase begin: {phase}");
    tokio::time::timeout(PATIENCE, rig.stop())
        .await
        .map_err(|_| phase_timeout(rig, phase, commands, "waiting for connector stop"))?;
    eprintln!("recording_keys phase complete: {phase}");
    Ok(())
}

async fn ready(rig: &EnvRig, environment: Id, phase: &'static str) -> TestResult {
    let environment = rig
        .connector
        .as_ref()
        .ok_or("connector absent")?
        .environment_role()
        .ok_or("environment role absent")?
        .host()
        .environment(environment)?;
    tokio::time::timeout(PATIENCE, async {
        while !environment.initial_scan_complete() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| {
        phase_timeout(
            rig,
            phase,
            &[],
            &format!(
                "initial scan={}, index={:?}, revision={}",
                environment.initial_scan_complete(),
                environment.index().state(),
                environment.index().revision()
            ),
        )
    })?;
    assert!(environment.index().state().is_full());
    Ok(())
}

fn journal(rig: &EnvRig, id: Id) -> TestResult<Entry> {
    Ok(serde_json::from_slice(&std::fs::read(
        rig.data.path().join("journal").join(format!("{id}.json")),
    )?)?)
}

#[tokio::test]
async fn slow_manifest_publication_allows_ordered_mutation_and_another_complete_capture_with_original_bytes_kept()
-> TestResult {
    let (mut rig, endpoint) = fixture();
    let directory = rig.directory("workspace");
    let original = [0, 255, 1, 0, 9];
    std::fs::write(directory.join("code.bin"), original)?;
    rig.start().await;
    rig.attach(&directory).await;
    ready(&rig, rig.environment, "initial workspace scan").await?;
    succeed(
        &rig,
        &FsList {
            environment_id: rig.environment,
            mount: None,
            path: String::new(),
            watch_id: Some(new_id()),
            watch_instance: None,
            page: None,
        },
        "install directory watch",
    )
    .await?;
    assert!(
        rig.controller
            .wait_until(PATIENCE, |controller| controller
                .reports()
                .iter()
                .filter(|kind| *kind == "fs.changed")
                .count()
                >= 1)
            .await,
        "phase 'initial directory watch report' did not arrive within {PATIENCE:?}"
    );
    let first = new_id();
    let request = request(rig.environment);
    let pause = endpoint.hold(first);
    issue(&rig, first, rig.environment, &request);
    pause
        .entered(&rig, first, "first publication reaches held HEAD")
        .await?;
    assert!(matches!(journal(&rig, first)?.step, Step::Started));
    let before_write = rig
        .controller
        .reports()
        .iter()
        .filter(|kind| *kind == "fs.changed")
        .count();
    let write = new_id();
    let sent = rig.stored(b"changed after capture");
    issue(
        &rig,
        write,
        rig.environment,
        &FsWrite {
            work: None,
            environment_id: rig.environment,
            path: "code.bin".to_owned(),
            blob: sent,
            mode: None,
            expected_hash: None,
        },
    );
    answer(
        &rig,
        write,
        "same-key write while first publication is held",
        &[first],
    )
    .await?;
    // The watcher refreshes the prepared baseline before reporting this write's directory change.
    assert!(
        rig.controller
            .wait_until(PATIENCE, |controller| controller
                .reports()
                .iter()
                .filter(|kind| *kind == "fs.changed")
                .count()
                > before_write)
            .await,
        "phase 'write watcher/baseline refresh' did not arrive within {PATIENCE:?}; before={before_write}, after={}",
        rig.controller
            .reports()
            .iter()
            .filter(|kind| *kind == "fs.changed")
            .count()
    );
    let next = new_id();
    let newer = FsRecord {
        snapshot_id: new_id(),
        ..request.clone()
    };
    issue(&rig, next, rig.environment, &newer);
    let second: FsRecordResult = serde_json::from_value(
        answer(
            &rig,
            next,
            "next capture while first publication is held",
            &[first, write],
        )
        .await?,
    )?;
    assert_eq!(second.state, RecordState::Complete);
    let new_hash = second.manifest_blob.ok_or("new capture has no manifest")?;
    let manifest =
        orch_envhost::manifest::parse(&rig.blobs.content(&new_hash).ok_or("new manifest absent")?)?;
    assert_eq!(
        manifest
            .entries
            .iter()
            .find(|entry| entry.p == "code.bin")
            .and_then(|entry| entry.h),
        Some(Hash::of(b"changed after capture"))
    );
    assert!(
        matches!(journal(&rig, first)?.step, Step::Started),
        "another capture may complete without acknowledging the held one"
    );
    let (code, detail) = failure(
        &rig,
        &EnvDetach {
            environment_id: rig.environment,
        },
        "Busy detach refusal during publication",
    )
    .await?;
    assert_eq!(code, "busy");
    assert!(detail.contains("recorded files"));
    let (code, _) = failure(
        &rig,
        &FsDataImport {
            work: None,
            environment_id: rig.environment,
            job_id: new_id(),
            part: EnvironmentDataPart::FileStates,
            manifest_blob: Hash::of(b"not a permitted import while busy"),
        },
        "Busy import refusal during publication",
    )
    .await?;
    assert_eq!(code, "busy");
    pause.release();
    let first_result: FsRecordResult = serde_json::from_value(
        answer(
            &rig,
            first,
            "released first publication and durable result",
            &[write, next],
        )
        .await?,
    )?;
    assert_eq!(first_result.state, RecordState::Complete);
    assert_eq!((first_result.files, first_result.bytes), (1, 5));
    let saved = succeed(
        &rig,
        &FsRecordRead {
            environment_id: rig.environment,
            snapshot_id: request.snapshot_id,
            path: Some("code.bin".to_owned()),
            export: false,
        },
        "final read of original immutable bytes",
    )
    .await?;
    let hash: Hash = serde_json::from_value(saved["blob"].clone())?;
    assert_eq!(
        rig.blobs
            .content(&hash)
            .ok_or("saved bytes missing")?
            .as_ref(),
        original
    );
    let replay = send_as(&rig, first, &request, "durable replay of first publication").await?;
    let Progress::Succeeded(replay) = replay else {
        return Err("durable replay failed".into());
    };
    assert_eq!(
        serde_json::from_value::<FsRecordResult>(replay)?,
        first_result
    );
    assert_eq!(
        pause.calls.load(Ordering::SeqCst),
        1,
        "replay performs no publication"
    );
    succeed(
        &rig,
        &EnvDetach {
            environment_id: rig.environment,
        },
        "detach after publications finish",
    )
    .await?;
    stop(&mut rig, "final connector stop", &[first, write, next]).await?;
    Ok(())
}

#[tokio::test]
async fn a_second_environment_captures_and_publishes_while_the_first_environment_upload_is_held()
-> TestResult {
    let (mut rig, endpoint) = fixture();
    let first_environment = rig.environment;
    let first_dir = rig.directory("first");
    let second_dir = rig.directory("second");
    std::fs::write(first_dir.join("a.bin"), b"first bytes")?;
    std::fs::write(second_dir.join("b.bin"), b"second bytes")?;
    rig.start().await;
    rig.attach(&first_dir).await;
    ready(&rig, first_environment, "first environment initial scan").await?;
    let other_environment = new_id();
    rig.environment = other_environment;
    rig.attach(&second_dir).await;
    ready(&rig, other_environment, "second environment initial scan").await?;
    rig.environment = first_environment;
    let first = new_id();
    let pause = endpoint.hold(first);
    issue(&rig, first, first_environment, &request(first_environment));
    pause
        .entered(&rig, first, "first environment reaches held HEAD")
        .await?;
    let other = new_id();
    issue(&rig, other, other_environment, &request(other_environment));
    let other_result: FsRecordResult = serde_json::from_value(
        answer(&rig, other, "independent environment capture", &[first]).await?,
    )?;
    assert_eq!(
        (other_result.state, other_result.files, other_result.bytes),
        (RecordState::Complete, 1, 12)
    );
    assert!(matches!(journal(&rig, first)?.step, Step::Started));
    pause.release();
    answer(
        &rig,
        first,
        "release first environment publication",
        &[other],
    )
    .await?;
    stop(
        &mut rig,
        "independent environments final stop",
        &[first, other],
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn graceful_stop_cancels_a_held_publication_and_restart_replays_its_durable_failure()
-> TestResult {
    let (mut rig, endpoint) = fixture();
    let directory = rig.directory("workspace");
    std::fs::write(directory.join("code.bin"), b"original")?;
    rig.start().await;
    rig.attach(&directory).await;
    ready(&rig, rig.environment, "shutdown fixture initial scan").await?;
    let id = new_id();
    let request = request(rig.environment);
    let pause = endpoint.hold(id);
    issue(&rig, id, rig.environment, &request);
    pause
        .entered(&rig, id, "shutdown fixture reaches held HEAD")
        .await?;
    stop(&mut rig, "graceful stop during held publication", &[id]).await?;
    let entry = journal(&rig, id)?;
    let Step::Done { result } = entry.step else {
        return Err("stop returned before durable finalization".into());
    };
    let Err(error) = result.outcome else {
        return Err("cancelled publication succeeded".into());
    };
    assert_eq!(error.code, "conflict");
    std::fs::write(directory.join("code.bin"), b"new workspace after stop")?;
    rig.start().await;
    let Progress::Failed(replayed) =
        send_as(&rig, id, &request, "restart replay of durable cancellation").await?
    else {
        return Err("restart did not replay the terminal refusal".into());
    };
    assert_eq!(replayed.code, "conflict");
    assert_eq!(pause.calls.load(Ordering::SeqCst), 1);
    stop(&mut rig, "restarted connector final stop", &[id]).await?;
    Ok(())
}
