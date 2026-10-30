//! The environment host as a whole, through the functions that carry out the commands: attaching, files, snapshots, watching, manifests, shells and gates.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orch_envhost::blobs::{BlobError, BlobSource, BlobWriter, MemoryBlobs};
use orch_envhost::environment::{EnvHost, Gates, HostSettings, Report, Reported, Reports};
use orch_envhost::error::HostError;
use orch_envhost::manifest;
use orch_envhost::snapshot::ProgressLog;
use orch_envhost::terminal::Event;
use orch_link_contract::commands::{
    EnvAttach, EnvAttachMount, EnvDetach, ErrorCode, ExpectedHash, FsCopy, FsFetch, FsList,
    FsManifest, FsMaterialize, FsMkdir, FsRemove, FsRename, FsSnapshot, FsStat, FsUnwatch, FsWrite,
    IndexMode, MaterializePurpose, PtyClose, PtyOpen, Selection, WatchSet,
};
use orch_link_contract::events::{EnvironmentFacts, PathStyle};
use orch_link_contract::session::Os;
use orch_types::{Hash, Id, SystemClock};
use tokio::sync::mpsc;

/// How long a test waits for something the host does by itself.
const PATIENCE: Duration = Duration::from_secs(15);

/// A shell must answer or end in the required order; this generous guard only
/// prevents hanging while a busy machine starts or reaps its real process.
const SHELL_PATIENCE: Duration = Duration::from_secs(300);

/// How long a test waits to be sure that the host does not do something.
const QUIET: Duration = Duration::from_millis(2500);

/// An environment host with one environment in a temporary directory that holds everything the
/// test makes: the workspace, the connector's data directory and the store of content.
struct Rig {
    dir: tempfile::TempDir,
    root: PathBuf,
    host: EnvHost,
    env: Id,
    store: MemoryBlobs,
    reports: mpsc::Receiver<Reported>,
    facts: EnvironmentFacts,
}

fn body(env: Id, root: &Path) -> EnvAttach {
    EnvAttach {
        resources: None,
        data_generation: None,
        storage: None,
        data_sources: None,
        environment_id: env,
        workspace_root: Some(root.to_string_lossy().into_owned()),
        ignore: Vec::new(),
        index: IndexMode::Full,
        mounts: None,
        variables: BTreeMap::new(),
        secrets: BTreeMap::new(),
    }
}

fn write_tree(root: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }
}

/// A host that offers the temporary directory, with `files` in the workspace, tuned by `tune`.
async fn rig_with(
    files: &[(&str, &str)],
    tune: impl FnOnce(&mut HostSettings, &mut EnvAttach),
) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    write_tree(&root, files);
    let mut settings =
        HostSettings::connected(dir.path().join("data"), vec![dir.path().to_path_buf()]);
    settings.gates = Gates::OPEN;
    settings.generation = 7;
    let env = orch_types::new_id();
    let mut attach = body(env, &root);
    tune(&mut settings, &mut attach);
    let store = MemoryBlobs::new();
    let (reports, receiver) = Reports::channel(256);
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(store.clone()),
        Vec::new(),
    );
    let facts = host.env_attach(&attach, &BTreeMap::new()).await.unwrap();
    Rig {
        root: root.canonicalize().unwrap(),
        dir,
        host,
        env,
        store,
        reports: receiver,
        facts,
    }
}

async fn rig(files: &[(&str, &str)]) -> Rig {
    rig_with(files, |_, _| {}).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_unit_home_store_keeps_its_publication_lease_through_upload_and_refuses_destructive_work()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use orch_envhost::environment::ManagedPlaces;
    use orch_link_contract::source::{
        EnvironmentDataPart, FsDataImport, FsRecord, RecordKind, RecordState,
    };
    let rig = rig_with(&[("code.txt", "original")], |settings, attach| {
        let Some(root) = &attach.workspace_root else {
            panic!("the rig supplies a workspace root");
        };
        settings.managed = Some(ManagedPlaces {
            workspace_root: PathBuf::from(root),
            home: Some(settings.data_dir.join("unit-home")),
            mounts_file: None,
            tools_dir: None,
            mounts_enforced: false,
        });
    })
    .await;
    let environment = rig.host.environment(rig.env)?;
    tokio::time::timeout(PATIENCE, async {
        while !environment.initial_scan_complete() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let request = FsRecord {
        environment_id: rig.env,
        snapshot_id: orch_types::new_id(),
        directory: String::new(),
        kind: RecordKind::Turn,
        record_days: orch_envhost::file_states::DEFAULT_RECORD_DAYS,
        max_bytes: orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
    };
    let until = orch_types::Time::new(
        chrono::Utc::now()
            + chrono::Duration::seconds(i64::from(
                orch_link_contract::commands::CommandKind::FsRecord.deadline_secs(),
            )),
    );
    let captured = rig
        .host
        .fs_record_capture(
            &request,
            orch_types::new_id(),
            until,
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;
    assert_eq!(captured.result.state, RecordState::Complete);
    let root = PathBuf::from(&rig.facts.home)
        .join(".orch/file-states")
        .join(rig.env.to_string())
        .join("roots")
        .join(format!("{}.json", request.snapshot_id));
    let original_root = std::fs::read(&root)?;
    let orch_envhost::file_states::CapturedState { input, hold, .. } = captured;
    input
        .ok_or("capture has no manifest input")?
        .publish(&rig.store)
        .await?;
    let Err(busy) = rig
        .host
        .env_detach(&EnvDetach {
            environment_id: rig.env,
        })
        .await
    else {
        return Err("detach discarded a live publication/finalization lease".into());
    };
    assert_eq!(busy.command_error().code, "busy");
    assert!(busy.command_error().retryable);
    for part in [EnvironmentDataPart::Home, EnvironmentDataPart::FileStates] {
        let result = rig
            .host
            .fs_data_import(
                &FsDataImport {
                    work: None,
                    environment_id: rig.env,
                    job_id: orch_types::new_id(),
                    part,
                    manifest_blob: Hash::of(b"import is forbidden before reading this content"),
                },
                &rig.store,
                &ProgressLog::new(),
            )
            .await;
        let Err(busy) = result else {
            return Err("import replaced a live recorded-state store".into());
        };
        assert_eq!(busy.command_error().code, "busy");
        assert!(busy.command_error().retryable);
        assert_eq!(std::fs::read(&root)?, original_root);
    }
    drop(hold);
    rig.host
        .env_detach(&EnvDetach {
            environment_id: rig.env,
        })
        .await?;
    assert_eq!(std::fs::read(&root)?, original_root);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn recorded_exports_send_saved_bytes_and_the_original_manifest_after_live_files_change() {
    use orch_link_contract::source::{FsRecord, FsRecordRead, RecordKind, RecordState};
    let rig = rig(&[("code.txt", "before")]).await;
    let snapshot = orch_types::new_id();
    let recorded = rig
        .host
        .fs_record(
            &FsRecord {
                environment_id: rig.env,
                snapshot_id: snapshot,
                directory: String::new(),
                kind: RecordKind::Turn,
                record_days: orch_envhost::file_states::DEFAULT_RECORD_DAYS,
                max_bytes: orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
            },
            &rig.store,
        )
        .await
        .unwrap();
    assert_eq!(
        recorded.state,
        RecordState::Complete,
        "the first recording identity must be usable: {recorded:?}"
    );
    std::fs::write(rig.root.join("code.txt"), "after").unwrap();
    let exported = rig
        .host
        .fs_record_read(
            &FsRecordRead {
                environment_id: rig.env,
                snapshot_id: snapshot,
                path: None,
                export: true,
            },
            orch_types::new_id(),
            &rig.store,
        )
        .await
        .unwrap();
    assert_eq!(Some(exported.blob), recorded.manifest_blob);
    let manifest =
        orch_envhost::manifest::parse(&rig.store.get_bytes(&exported.blob).unwrap()).unwrap();
    let entry = manifest
        .entries
        .iter()
        .find(|entry| entry.p == "code.txt")
        .unwrap();
    assert_eq!(
        rig.store.get_bytes(&entry.h.unwrap()).unwrap().as_ref(),
        b"before"
    );
    let single = rig
        .host
        .fs_record_read(
            &FsRecordRead {
                environment_id: rig.env,
                snapshot_id: snapshot,
                path: Some("code.txt".to_owned()),
                export: false,
            },
            orch_types::new_id(),
            &rig.store,
        )
        .await
        .unwrap();
    assert_eq!(single.blob, entry.h.unwrap());
    assert_eq!(single.size, 6);
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_commands_transfer_the_reviewed_plan_and_undo_the_exact_prior_files() {
    use orch_link_contract::source::{FsRecord, FsRestore, FsRestorePlan, RecordKind, RecordState};
    let rig = rig(&[("code.txt", "before")]).await;
    let snapshot = orch_types::new_id();
    let recorded = rig
        .host
        .fs_record(
            &FsRecord {
                environment_id: rig.env,
                snapshot_id: snapshot,
                directory: String::new(),
                kind: RecordKind::Turn,
                record_days: orch_envhost::file_states::DEFAULT_RECORD_DAYS,
                max_bytes: orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
            },
            &rig.store,
        )
        .await
        .unwrap();
    assert_eq!(
        recorded.state,
        RecordState::Complete,
        "a first turn needs no recording retry: {recorded:?}"
    );
    std::fs::write(rig.root.join("code.txt"), "current").unwrap();
    std::fs::write(rig.root.join("later.txt"), "made later").unwrap();
    let make_plan = FsRestorePlan {
        environment_id: rig.env,
        snapshot_id: snapshot,
        directory: String::new(),
        turn_id: None,
        record_days: None,
        max_bytes: None,
    };
    let plan = rig
        .host
        .fs_restore_plan(&make_plan, &rig.store)
        .await
        .unwrap();
    assert!(!plan.writers_active && plan.undo_available);
    assert!(
        rig.store.get_bytes(&plan.plan_blob).is_some(),
        "the private plan must reach control-plane content storage"
    );
    let apply = FsRestore {
        work: None,
        environment_id: rig.env,
        job_id: orch_types::new_id(),
        snapshot_id: snapshot,
        undo_snapshot_id: orch_types::new_id(),
        plan_blob: plan.plan_blob,
        generation: rig.facts.generation,
    };
    let result = rig.host.fs_restore(&apply, &rig.store).await.unwrap();
    assert!(!result.undone);
    assert_eq!(std::fs::read(rig.root.join("code.txt")).unwrap(), b"before");
    assert!(!rig.root.join("later.txt").exists());
    let plan = rig
        .host
        .fs_restore_plan(
            &FsRestorePlan {
                snapshot_id: result.undo_snapshot_id,
                ..make_plan
            },
            &rig.store,
        )
        .await
        .unwrap();
    rig.host
        .fs_restore(
            &FsRestore {
                work: None,
                job_id: orch_types::new_id(),
                snapshot_id: result.undo_snapshot_id,
                undo_snapshot_id: orch_types::new_id(),
                plan_blob: plan.plan_blob,
                ..apply
            },
            &rig.store,
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(rig.root.join("code.txt")).unwrap(),
        b"current"
    );
    assert_eq!(
        std::fs::read(rig.root.join("later.txt")).unwrap(),
        b"made later"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_first_turn_in_a_new_directory_records_and_restores_with_the_shipped_defaults() {
    use orch_link_contract::source::{FsRecord, FsRestore, FsRestorePlan, RecordKind, RecordState};
    use orch_settings_contract::registry::{self, keys};
    let rig = rig(&[("web/feature/code.txt", "before"), ("other.txt", "outside")]).await;
    let record_days = registry::get(keys::WORKSPACE_RECORD_DAYS)
        .unwrap()
        .default_value()
        .unwrap()
        .as_i64()
        .unwrap();
    let max_bytes = registry::get(keys::WORKSPACE_RECORD_MAX_BYTES)
        .unwrap()
        .default_value()
        .unwrap()
        .as_i64()
        .unwrap();
    assert_eq!(record_days, orch_envhost::file_states::DEFAULT_RECORD_DAYS);
    assert_eq!(
        max_bytes,
        orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES
    );
    let request = FsRecord {
        environment_id: rig.env,
        snapshot_id: orch_types::new_id(),
        directory: "web/feature".into(),
        kind: RecordKind::Turn,
        record_days,
        max_bytes,
    };
    let recorded = rig.host.fs_record(&request, &rig.store).await.unwrap();
    assert_eq!(
        recorded.state,
        RecordState::Complete,
        "one first-turn request must finish its prepared scope: {recorded:?}"
    );
    assert_eq!(recorded.files, 1);
    assert_eq!(recorded.bytes, 6);
    let manifest = manifest::parse(
        &rig.store
            .get_bytes(&recorded.manifest_blob.unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.header.generation, rig.facts.generation);
    assert_eq!(
        manifest.header.root,
        rig.root.join("web/feature").display().to_string()
    );
    assert!(
        manifest
            .entries
            .iter()
            .any(|entry| entry.p == "code.txt" && entry.h == Some(Hash::of(b"before")))
    );
    let store = rig
        .dir
        .path()
        .join("data/file-states")
        .join(rig.env.to_string());
    assert!(
        store
            .join("roots")
            .join(format!("{}.json", request.snapshot_id))
            .is_file()
    );
    assert!(
        !store.starts_with(&rig.root),
        "recorded files use the connector's persistent data, outside the project"
    );

    std::fs::write(rig.root.join("web/feature/code.txt"), "after").unwrap();
    std::fs::write(rig.root.join("web/feature/later.txt"), "later").unwrap();
    assert_eq!(
        rig.host.fs_record(&request, &rig.store).await.unwrap(),
        recorded,
        "a retry never captures the later files"
    );
    let plan = rig
        .host
        .fs_restore_plan(
            &FsRestorePlan {
                environment_id: rig.env,
                snapshot_id: request.snapshot_id,
                directory: request.directory.clone(),
                turn_id: None,
                record_days: None,
                max_bytes: None,
            },
            &rig.store,
        )
        .await
        .unwrap();
    assert!(plan.undo_available);
    rig.host
        .fs_restore(
            &FsRestore {
                work: None,
                environment_id: rig.env,
                job_id: orch_types::new_id(),
                snapshot_id: request.snapshot_id,
                undo_snapshot_id: orch_types::new_id(),
                plan_blob: plan.plan_blob,
                generation: rig.facts.generation,
            },
            &rig.store,
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(rig.root.join("web/feature/code.txt")).unwrap(),
        b"before"
    );
    assert!(!rig.root.join("web/feature/later.txt").exists());
    assert_eq!(
        std::fs::read(rig.root.join("other.txt")).unwrap(),
        b"outside"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn re_enabling_the_shipped_recording_policy_prepares_the_next_identity_before_answering() {
    use orch_link_contract::source::{FsRecord, RecordKind, RecordState};
    let rig = rig(&[("code.txt", "before")]).await;
    let off = FsRecord {
        environment_id: rig.env,
        snapshot_id: orch_types::new_id(),
        directory: String::new(),
        kind: RecordKind::Turn,
        record_days: 0,
        max_bytes: orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
    };
    let disabled = rig.host.fs_record(&off, &rig.store).await.unwrap();
    assert_eq!(disabled.state, RecordState::Unavailable);
    assert_eq!(disabled.reason.as_deref(), Some("off"));
    let on = FsRecord {
        snapshot_id: orch_types::new_id(),
        record_days: orch_envhost::file_states::DEFAULT_RECORD_DAYS,
        ..off.clone()
    };
    assert_eq!(
        rig.host.fs_record(&on, &rig.store).await.unwrap().state,
        RecordState::Complete
    );
    assert_eq!(
        rig.host.fs_record(&off, &rig.store).await.unwrap(),
        disabled,
        "the previous disabled result stays immutable"
    );
}

impl Drop for Rig {
    /// A test that fails half way leaves no shell behind: each is closed with everything it started.
    fn drop(&mut self) {
        for environment in self.host.environments() {
            if tokio::runtime::Handle::try_current().is_ok() {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(environment.terminals().close_all());
                });
            }
        }
    }
}

impl Rig {
    fn list(&self, path: &str) -> FsList {
        FsList {
            environment_id: self.env,
            mount: None,
            path: path.to_owned(),
            watch_id: None,
            watch_instance: None,
            page: None,
        }
    }

    async fn names(&self, path: &str) -> Vec<String> {
        let listed = tokio::time::timeout(PATIENCE, async {
            loop {
                match self.host.fs_list(&self.list(path)).await {
                    Err(HostError::Changed(_)) => tokio::task::yield_now().await,
                    result => break result.unwrap(),
                }
            }
        })
        .await
        .expect("a fresh directory observation settles within the fixture patience");
        let mut names: Vec<String> = listed
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        names.sort();
        names
    }

    /// The next report the test is interested in; fails if none comes.
    async fn report(&mut self, what: &str, mut wanted: impl FnMut(&Report) -> bool) -> Report {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let reported = tokio::time::timeout_at(deadline, self.reports.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
                .expect("the host stopped reporting");
            assert_eq!(reported.environment_id, self.env);
            if wanted(&reported.report) {
                return reported.report;
            }
        }
    }

    /// Fails if a report the test is interested in comes within the quiet time.
    async fn no_report(&mut self, what: &str, mut unwanted: impl FnMut(&Report) -> bool) {
        let deadline = tokio::time::Instant::now() + QUIET;
        while let Ok(Some(reported)) = tokio::time::timeout_at(deadline, self.reports.recv()).await
        {
            assert!(
                !unwanted(&reported.report),
                "unexpected {what}: {:?}",
                reported.report
            );
        }
    }

    fn store_text(&self, text: &str) -> Hash {
        self.store.insert(text.as_bytes().to_vec())
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.root.join(path)).unwrap()
    }
}

fn git_exists() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn code_of<T: std::fmt::Debug>(result: Result<T, HostError>) -> ErrorCode {
    result.unwrap_err().code()
}

// ---------------------------------------------------------------------------------------------
// Attaching

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attaching_an_environment_says_where_it_is_and_what_it_can_do() {
    let rig = rig(&[("src/main.rs", "fn main() {}\n")]).await;
    let facts = &rig.facts;
    assert_eq!(facts.workspace_root, rig.root.to_string_lossy());
    assert_eq!(facts.os, Os::Linux);
    assert_eq!(facts.path_style, PathStyle::Posix);
    assert!(
        facts.case_sensitive,
        "a temporary directory of Linux tells 'A' from 'a'"
    );
    assert_eq!(facts.index, IndexMode::Full);
    assert!(facts.suspend, "processes can be held still here");
    assert_eq!(facts.generation, 7);
    assert_eq!(facts.git, git_exists());
    assert!(!facts.shell.path.is_empty(), "a person gets a shell");
    assert_eq!(rig.host.environment(rig.env).unwrap().facts(), rig.facts);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_managed_units_manifest_and_live_facts_use_the_generation_sent_when_attaching() {
    let mut rig = rig_with(&[("code.txt", "generation two")], |settings, attach| {
        settings.generation = 0;
        attach.data_generation = Some(2);
        settings.managed = Some(orch_envhost::environment::ManagedPlaces {
            workspace_root: PathBuf::from(attach.workspace_root.as_ref().unwrap()),
            home: Some(settings.data_dir.join("unit-home")),
            mounts_file: None,
            tools_dir: None,
            mounts_enforced: false,
        });
    })
    .await;
    assert_eq!(rig.facts.generation, 2);
    let Report::Manifest(published) = rig
        .report("the generation-bound initial manifest", |report| {
            matches!(report, Report::Manifest(_))
        })
        .await
    else {
        unreachable!()
    };
    let automatic =
        manifest::parse(&rig.store.get_bytes(&published.manifest_blob).unwrap()).unwrap();
    assert_eq!(automatic.header.generation, 2);
    let current = rig
        .host
        .fs_manifest(
            &FsManifest {
                environment_id: rig.env,
                selection: Selection::default(),
            },
            &rig.store,
        )
        .await
        .unwrap();
    let explicit = manifest::parse(&rig.store.get_bytes(&current.manifest_blob).unwrap()).unwrap();
    assert_eq!(explicit.header.generation, 2);
    rig.host.before_offline().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_after_managed_reattachment_keeps_generation_two_and_its_actual_workspace_bytes()
{
    use orch_link_contract::source::{FsRecord, RecordKind, RecordState};
    let mut rig = rig_with(&[("code.txt", "generation one")], |settings, attach| {
        settings.generation = 0;
        attach.data_generation = Some(1);
        settings.managed = Some(orch_envhost::environment::ManagedPlaces {
            workspace_root: PathBuf::from(attach.workspace_root.as_ref().unwrap()),
            home: Some(settings.data_dir.join("unit-home")),
            mounts_file: None,
            tools_dir: None,
            mounts_enforced: false,
        });
    })
    .await;
    assert_eq!(rig.facts.generation, 1);
    rig.report("the first generation's manifest", |report| {
        matches!(report, Report::Manifest(_))
    })
    .await;
    rig.host
        .env_detach(&EnvDetach {
            environment_id: rig.env,
        })
        .await
        .unwrap();
    std::fs::write(rig.root.join("code.txt"), b"generation two").unwrap();
    let mut attach = body(rig.env, &rig.root);
    attach.data_generation = Some(2);
    rig.facts = rig
        .host
        .env_attach(&attach, &BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(rig.facts.generation, 2);
    assert_eq!(rig.host.environment(rig.env).unwrap().facts().generation, 2);
    let store = rig.store.clone();
    rig.report("the reattached generation's manifest", |report| {
        matches!(report, Report::Manifest(published)
            if manifest::parse(&store.get_bytes(&published.manifest_blob).unwrap()).unwrap().header.generation == 2)
    })
    .await;
    let captured = rig
        .host
        .fs_record(
            &FsRecord {
                environment_id: rig.env,
                snapshot_id: orch_types::new_id(),
                directory: String::new(),
                kind: RecordKind::Turn,
                record_days: orch_envhost::file_states::DEFAULT_RECORD_DAYS,
                max_bytes: orch_envhost::file_states::DEFAULT_RECORD_MAX_BYTES,
            },
            &rig.store,
        )
        .await
        .unwrap();
    assert_eq!(captured.state, RecordState::Complete);
    let blob = captured
        .manifest_blob
        .expect("a complete capture has its manifest");
    let recorded = manifest::parse(&rig.store.get_bytes(&blob).unwrap()).unwrap();
    assert_eq!(recorded.header.generation, 2);
    assert!(
        recorded
            .entries
            .iter()
            .any(|entry| { entry.p == "code.txt" && entry.h == Some(Hash::of(b"generation two")) })
    );
    rig.host.before_offline().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_root_the_machines_owner_does_not_offer_is_refused_however_it_is_spelled() {
    let dir = tempfile::tempdir().unwrap();
    let offered = dir.path().join("offered");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(offered.join("ws")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, offered.join("link")).unwrap();
    let settings = HostSettings::connected(dir.path().join("data"), vec![offered.clone()]);
    let (reports, _receiver) = Reports::channel(16);
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );

    for spelled in [
        outside.clone(),
        offered.join("..").join("outside"),
        offered.join("link"),
        dir.path().to_path_buf(),
    ] {
        let attach = body(orch_types::new_id(), &spelled);
        assert_eq!(
            code_of(host.env_attach(&attach, &BTreeMap::new()).await),
            ErrorCode::PolicyRootNotAllowed,
            "{} is not offered",
            spelled.display()
        );
    }
    let missing = body(orch_types::new_id(), &offered.join("nowhere"));
    assert!(host.env_attach(&missing, &BTreeMap::new()).await.is_err());
    let inside = body(orch_types::new_id(), &offered.join("ws"));
    assert!(host.env_attach(&inside, &BTreeMap::new()).await.is_ok());
    assert!(host.environment(inside.environment_id).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tree_above_the_entry_limit_is_listed_live_and_not_indexed() {
    let files: Vec<(String, &str)> = (1..=3)
        .flat_map(|d| (0..10).map(move |f| (format!("d{d}/f{f}.txt"), "x")))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), *text))
        .collect();
    let mut rig = rig_with(&refs, |settings, _| settings.limits.index_max_entries = 10).await;
    assert_eq!(
        rig.facts.index,
        IndexMode::Live,
        "thirty-three entries do not fit in ten"
    );
    let environment = rig.host.environment(rig.env).unwrap();
    assert!(environment.index().is_empty(), "nothing is kept");

    assert_eq!(
        rig.names("d2").await.len(),
        10,
        "it is listed from the file system as it is now"
    );
    let whole = FsManifest {
        environment_id: rig.env,
        selection: Selection::default(),
    };
    assert_eq!(
        code_of(rig.host.fs_manifest(&whole, &rig.store).await),
        ErrorCode::Unsupported,
        "the whole of it is not listed in one go"
    );
    let part = FsManifest {
        environment_id: rig.env,
        selection: Selection {
            include: vec!["d1".into()],
            exclude: vec![],
        },
    };
    assert_eq!(
        rig.host
            .fs_manifest(&part, &rig.store)
            .await
            .unwrap()
            .file_count,
        10
    );

    rig.host.turn_completed(rig.env);
    rig.no_report("manifest", |report| matches!(report, Report::Manifest(_)))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tree_that_grows_past_the_entry_limit_after_attaching_turns_live_and_says_so() {
    let mut rig = rig_with(&[("a.txt", "1")], |settings, _| {
        settings.limits.index_max_entries = 5
    })
    .await;
    assert_eq!(rig.facts.index, IndexMode::Full);
    // The first manifest of a small tree is not held back.
    rig.report("the first manifest", |report| {
        matches!(report, Report::Manifest(_))
    })
    .await;
    for n in 0..10 {
        std::fs::write(rig.root.join(format!("more{n}.txt")), "x").unwrap();
    }
    let report = rig
        .report("the facts of a tree that is live now", |report| {
            matches!(report, Report::Facts(_))
        })
        .await;
    let Report::Facts(facts) = report else {
        unreachable!()
    };
    assert_eq!(facts.index, IndexMode::Live);
    assert_eq!(
        rig.host.environment(rig.env).unwrap().facts().index,
        IndexMode::Live
    );
}

// ---------------------------------------------------------------------------------------------
// Files

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_is_listed_described_and_fetched_into_the_store_once() {
    let rig = rig(&[
        ("src/main.rs", "fn main() {}\n"),
        ("README.md", "# readme\n"),
    ])
    .await;
    assert_eq!(rig.names("").await, ["README.md", "src"]);
    let stat = rig
        .host
        .fs_stat(&FsStat {
            environment_id: rig.env,
            mount: None,
            path: "src/main.rs".into(),
        })
        .await
        .unwrap();
    assert_eq!(stat.entry.size, 13);
    assert!(stat.is_text);

    let fetch = FsFetch {
        environment_id: rig.env,
        mount: None,
        path: "src/main.rs".into(),
        expect: None,
        max_bytes: None,
    };
    let first = rig.host.fs_fetch(&fetch, &rig.store).await.unwrap();
    assert_eq!(first.hash, Hash::of(b"fn main() {}\n"));
    let times_up = || {
        rig.store
            .uploads()
            .iter()
            .filter(|hash| **hash == first.hash)
            .count()
    };
    assert_eq!(times_up(), 1);
    let again = rig.host.fs_fetch(&fetch, &rig.store).await.unwrap();
    assert_eq!(again.hash, first.hash);
    assert_eq!(times_up(), 1, "the store has it, so nothing goes up again");

    let wrong = FsFetch {
        expect: Some(orch_link_contract::commands::FileVersion {
            size: 1,
            mtime_ns: 1,
        }),
        ..fetch
    };
    assert_eq!(
        code_of(rig.host.fs_fetch(&wrong, &rig.store).await),
        ErrorCode::Changed,
        "a file that is not the version expected is not fetched"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_path_that_leaves_the_workspace_reaches_nothing_whatever_the_command() {
    let rig = rig(&[("inside.txt", "inside")]).await;
    std::fs::write(rig.dir.path().join("outside.txt"), "outside").unwrap();
    std::os::unix::fs::symlink("../outside.txt", rig.root.join("escape.txt")).unwrap();
    std::os::unix::fs::symlink(rig.dir.path(), rig.root.join("up")).unwrap();
    let blob = rig.store_text("changed");
    let env = rig.env;

    // Spelled to go out, or going out through a link on the way: no command gets anywhere.
    for path in [
        "../outside.txt",
        "/etc/passwd",
        "a/../../outside.txt",
        "up/outside.txt",
        "up/ws/inside.txt",
    ] {
        let stat = rig
            .host
            .fs_stat(&FsStat {
                environment_id: env,
                mount: None,
                path: path.into(),
            })
            .await;
        let fetch = rig
            .host
            .fs_fetch(
                &FsFetch {
                    environment_id: env,
                    mount: None,
                    path: path.into(),
                    expect: None,
                    max_bytes: None,
                },
                &rig.store,
            )
            .await;
        let write = rig
            .host
            .fs_write(
                &FsWrite {
                    work: None,
                    environment_id: env,
                    path: path.into(),
                    blob,
                    mode: None,
                    expected_hash: None,
                },
                &rig.store,
            )
            .await;
        let remove = rig
            .host
            .fs_remove(&FsRemove {
                environment_id: env,
                path: path.into(),
                recursive: true,
            })
            .await;
        let rename = rig
            .host
            .fs_rename(&FsRename {
                environment_id: env,
                from: "inside.txt".into(),
                to: path.into(),
            })
            .await;
        let copy = rig
            .host
            .fs_copy(&FsCopy {
                environment_id: env,
                from: path.into(),
                to: "copy.txt".into(),
            })
            .await;
        let mkdir = rig
            .host
            .fs_mkdir(&FsMkdir {
                environment_id: env,
                path: format!("{path}/dir"),
            })
            .await;
        assert!(
            stat.is_err()
                && fetch.is_err()
                && write.is_err()
                && remove.is_err()
                && rename.is_err()
                && copy.is_err()
                && mkdir.is_err(),
            "{path} must reach nothing"
        );
    }
    assert_eq!(
        std::fs::read_to_string(rig.dir.path().join("outside.txt")).unwrap(),
        "outside",
        "nothing outside was written or removed"
    );
    assert_eq!(rig.read("inside.txt"), "inside");
    assert!(!rig.root.join("copy.txt").exists());
    assert!(!rig.dir.path().join("dir").exists());

    // A link that is the last component and leaves the workspace is a link and no more: it is
    // described, but nothing is read or written through it.
    let described = rig
        .host
        .fs_stat(&FsStat {
            environment_id: env,
            mount: None,
            path: "escape.txt".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        described.entry.link_target.as_deref(),
        Some("../outside.txt")
    );
    assert_eq!(
        code_of(
            rig.host
                .fs_fetch(
                    &FsFetch {
                        environment_id: env,
                        mount: None,
                        path: "escape.txt".into(),
                        expect: None,
                        max_bytes: None,
                    },
                    &rig.store
                )
                .await
        ),
        ErrorCode::NotFound
    );
    let write = FsWrite {
        work: None,
        environment_id: env,
        path: "escape.txt".into(),
        blob,
        mode: None,
        expected_hash: None,
    };
    assert_eq!(
        code_of(rig.host.fs_write(&write, &rig.store).await),
        ErrorCode::NotFound
    );
    assert_eq!(
        std::fs::read_to_string(rig.dir.path().join("outside.txt")).unwrap(),
        "outside"
    );
    // Removing it removes the link, not the file it points to.
    rig.host
        .fs_remove(&FsRemove {
            environment_id: env,
            path: "escape.txt".into(),
            recursive: false,
        })
        .await
        .unwrap();
    assert!(std::fs::symlink_metadata(rig.root.join("escape.txt")).is_err());
    assert_eq!(
        std::fs::read_to_string(rig.dir.path().join("outside.txt")).unwrap(),
        "outside"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_over_a_newer_version_is_refused_and_leaves_the_file_as_it_was() {
    let rig = rig(&[("notes.txt", "version one")]).await;
    let env = rig.env;
    let fetch = FsFetch {
        environment_id: env,
        mount: None,
        path: "notes.txt".into(),
        expect: None,
        max_bytes: None,
    };
    let seen = rig.host.fs_fetch(&fetch, &rig.store).await.unwrap().hash;
    // Somebody else saves a newer version after the writer looked.
    std::fs::write(rig.root.join("notes.txt"), "version two, by someone else").unwrap();
    let blob = rig.store_text("version three");

    let write = |expected| FsWrite {
        work: None,
        environment_id: env,
        path: "notes.txt".into(),
        blob,
        mode: None,
        expected_hash: expected,
    };
    let refused = rig
        .host
        .fs_write(&write(Some(ExpectedHash::Is(seen))), &rig.store)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), ErrorCode::Conflict);
    let wire = serde_json::to_value(refused.command_error()).unwrap();
    assert_eq!(
        wire["details"]["current"]["hash"],
        serde_json::json!(Hash::of(b"version two, by someone else"))
    );
    let HostError::Stale { current } = &refused else {
        panic!("expected a stale write, got {refused:?}")
    };
    assert_eq!(
        current.as_ref().and_then(|stat| stat.hash),
        Some(Hash::of(b"version two, by someone else")),
        "the writer is told what is there now"
    );
    assert_eq!(
        rig.read("notes.txt"),
        "version two, by someone else",
        "the newer version is untouched"
    );
    let leftovers: Vec<_> = std::fs::read_dir(&rig.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        ["notes.txt"],
        "no half-written file stays behind"
    );

    assert_eq!(
        rig.host
            .fs_write(&write(Some(ExpectedHash::Absent)), &rig.store)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::Conflict,
        "'must not exist' does not hold either"
    );
    let now = Hash::of(b"version two, by someone else");
    rig.host
        .fs_write(&write(Some(ExpectedHash::Is(now))), &rig.store)
        .await
        .unwrap();
    assert_eq!(rig.read("notes.txt"), "version three");
}

/// A store that gives half of what it was asked for and then stops.
struct CutOff;

#[async_trait::async_trait]
impl BlobSource for CutOff {
    async fn get(&self, _hash: Hash, target: &mut dyn BlobWriter) -> Result<i64, BlobError> {
        target
            .write(0, bytes::Bytes::from_static(b"half of the new"))
            .await
            .map_err(|error| BlobError::Local(error.to_string()))?;
        Err(BlobError::StoodStill)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_that_is_cut_off_leaves_the_old_file_whole() {
    let rig = rig(&[("notes.txt", "the old content, whole")]).await;
    let write = FsWrite {
        work: None,
        environment_id: rig.env,
        path: "notes.txt".into(),
        blob: Hash::of(b"half of the new content"),
        mode: None,
        expected_hash: None,
    };
    assert!(rig.host.fs_write(&write, &CutOff).await.is_err());
    assert_eq!(rig.read("notes.txt"), "the old content, whole");
    assert_eq!(
        std::fs::read_dir(&rig.root).unwrap().count(),
        1,
        "the part that arrived is gone"
    );

    let fresh = FsWrite {
        work: None,
        path: "created/new.txt".into(),
        expected_hash: Some(ExpectedHash::Absent),
        ..write
    };
    assert!(rig.host.fs_write(&fresh, &CutOff).await.is_err());
    assert!(
        !rig.root.join("created/new.txt").exists(),
        "a file that was never whole never exists"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ignored_file_is_neither_listed_nor_fetched_nor_snapshotted() {
    let rig = rig_with(
        &[
            ("src/lib.rs", "pub fn f() {}"),
            ("target/debug/big.o", "object"),
            ("keep.txt", "kept"),
            ("secret.env", "KEY=1"),
        ],
        |_, attach| {
            attach.ignore = vec!["target/".into(), "*.env".into()];
        },
    )
    .await;
    assert_eq!(rig.names("").await, ["keep.txt", "src"]);
    assert_eq!(
        code_of(
            rig.host
                .fs_stat(&FsStat {
                    environment_id: rig.env,
                    mount: None,
                    path: "secret.env".into()
                })
                .await
        ),
        ErrorCode::Ignored
    );
    let fetch = FsFetch {
        environment_id: rig.env,
        mount: None,
        path: "target/debug/big.o".into(),
        expect: None,
        max_bytes: None,
    };
    assert_eq!(
        code_of(rig.host.fs_fetch(&fetch, &rig.store).await),
        ErrorCode::Ignored
    );

    let snapshot = FsSnapshot {
        work: None,
        environment_id: rig.env,
        job_id: orch_types::new_id(),
        selection: Selection::default(),
    };
    let result = rig
        .host
        .fs_snapshot(&snapshot, &rig.store, &ProgressLog::new())
        .await
        .unwrap();
    let listed = manifest::parse(&rig.store.get_bytes(&result.manifest_blob).unwrap()).unwrap();
    let paths: Vec<&str> = listed
        .entries
        .iter()
        .map(|entry| entry.p.as_str())
        .collect();
    assert!(
        paths
            .iter()
            .all(|path| !path.starts_with("target") && !path.ends_with(".env")),
        "{paths:?}"
    );
    assert_eq!(result.file_count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gates_decide_whether_a_remote_may_change_files_and_open_shells() {
    let rig = rig_with(&[("a.txt", "a")], |settings, _| {
        settings.gates = Gates::CLOSED
    })
    .await;
    let env = rig.env;
    let blob = rig.store_text("b");
    let attempts = [
        code_of(
            rig.host
                .fs_write(
                    &FsWrite {
                        work: None,
                        environment_id: env,
                        path: "b.txt".into(),
                        blob,
                        mode: None,
                        expected_hash: None,
                    },
                    &rig.store,
                )
                .await,
        ),
        code_of(
            rig.host
                .fs_mkdir(&FsMkdir {
                    environment_id: env,
                    path: "dir".into(),
                })
                .await,
        ),
        code_of(
            rig.host
                .fs_rename(&FsRename {
                    environment_id: env,
                    from: "a.txt".into(),
                    to: "c.txt".into(),
                })
                .await,
        ),
        code_of(
            rig.host
                .fs_copy(&FsCopy {
                    environment_id: env,
                    from: "a.txt".into(),
                    to: "d.txt".into(),
                })
                .await,
        ),
        code_of(
            rig.host
                .fs_remove(&FsRemove {
                    environment_id: env,
                    path: "a.txt".into(),
                    recursive: false,
                })
                .await,
        ),
    ];
    assert!(
        attempts
            .iter()
            .all(|code| *code == ErrorCode::PolicyRemoteWriteDisabled),
        "{attempts:?}"
    );
    assert_eq!(rig.names("").await, ["a.txt"], "nothing changed");
    let open = PtyOpen {
        environment_id: env,
        terminal_id: orch_types::new_id(),
        cols: 80,
        rows: 24,
        cwd: String::new(),
        record: false,
    };
    assert_eq!(
        code_of(rig.host.pty_open(&open).await),
        ErrorCode::Unsupported
    );

    // The machine's owner changes the gates while the host runs.
    rig.host.set_policy(
        Gates {
            remote_write: true,
            deliver_files: false,
            terminal: false,
            browser: false,
            vnc: false,
        },
        Some(vec![rig.dir.path().to_path_buf()]),
        None,
        orch_envhost::mcp::McpGates {
            import: true,
            add: false,
        },
    );
    rig.host
        .fs_mkdir(&FsMkdir {
            environment_id: env,
            path: "dir".into(),
        })
        .await
        .unwrap();
    assert!(rig.root.join("dir").is_dir());
    assert_eq!(
        code_of(rig.host.pty_open(&open).await),
        ErrorCode::Unsupported,
        "shells stay off"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_directory_of_a_thread_is_made_with_an_orch_directory_that_a_repository_does_not_list()
{
    let rig = rig(&[("web/app/index.html", "<html>")]).await;
    let made = rig.host.thread_directory(rig.env, "web/app").await.unwrap();
    assert_eq!(made.path.to_string(), "web/app");
    assert_eq!(made.orch.to_string(), "web/app/.orch");
    assert_eq!(rig.read("web/app/.orch/.gitignore"), "*\n");
    assert_eq!(
        rig.names("web/app").await,
        [".orch", "index.html"],
        ".orch is visible in Orch"
    );
    // Again, and a thread that shares the directory: nothing changes.
    rig.host.thread_directory(rig.env, "web/app").await.unwrap();
    assert_eq!(rig.read("web/app/.orch/.gitignore"), "*\n");
    // Not inside an .orch, and not out of the workspace.
    assert!(
        rig.host
            .thread_directory(rig.env, "web/app/.orch")
            .await
            .is_err()
    );
    assert!(
        rig.host
            .thread_directory(rig.env, "../elsewhere")
            .await
            .is_err()
    );
    assert!(!rig.dir.path().join("elsewhere").exists());

    let closed = rig_with(&[], |settings, _| settings.gates = Gates::CLOSED).await;
    assert_eq!(
        code_of(closed.host.thread_directory(closed.env, "work").await),
        ErrorCode::PolicyRemoteWriteDisabled
    );
    assert!(!closed.root.join("work").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_are_made_moved_copied_and_removed_inside_the_workspace() {
    let rig = rig(&[("a/one.txt", "1"), ("a/two.txt", "2")]).await;
    let env = rig.env;
    rig.host
        .fs_mkdir(&FsMkdir {
            environment_id: env,
            path: "b/c".into(),
        })
        .await
        .unwrap();
    rig.host
        .fs_copy(&FsCopy {
            environment_id: env,
            from: "a".into(),
            to: "b/c/a2".into(),
        })
        .await
        .unwrap();
    rig.host
        .fs_rename(&FsRename {
            environment_id: env,
            from: "a/one.txt".into(),
            to: "b/one.txt".into(),
        })
        .await
        .unwrap();
    assert_eq!(rig.names("b").await, ["c", "one.txt"]);
    assert_eq!(rig.names("b/c/a2").await, ["one.txt", "two.txt"]);
    assert_eq!(
        code_of(
            rig.host
                .fs_remove(&FsRemove {
                    environment_id: env,
                    path: "b".into(),
                    recursive: false
                })
                .await
        ),
        ErrorCode::Conflict,
        "a directory with things in it is not removed unless it is asked to"
    );
    rig.host
        .fs_remove(&FsRemove {
            environment_id: env,
            path: "b".into(),
            recursive: true,
        })
        .await
        .unwrap();
    assert_eq!(rig.names("").await, ["a"]);
}

// ---------------------------------------------------------------------------------------------
// Snapshots and moves

fn tree_of(root: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            let relative = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let kind = if meta.file_type().is_symlink() {
                format!("link -> {}", std::fs::read_link(&path).unwrap().display())
            } else if meta.is_dir() {
                walk(base, &path, out);
                "dir".to_owned()
            } else {
                use std::os::unix::fs::PermissionsExt;
                format!(
                    "file {:o} {}",
                    meta.permissions().mode() & 0o777,
                    Hash::of(&std::fs::read(&path).unwrap())
                )
            };
            out.insert(relative, kind);
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_uploads_only_what_the_store_lacks_and_materializes_to_an_equal_tree() {
    use std::os::unix::fs::PermissionsExt;
    let rig = rig_with(
        &[
            ("src/lib.rs", "pub fn f() {}\n"),
            ("docs/readme.md", "# docs\n"),
            ("data/blob.bin", &"0123456789abcdef".repeat(200_000)),
            ("target/junk", "ignored"),
        ],
        |_, attach| attach.ignore = vec!["target/".into()],
    )
    .await;
    std::fs::create_dir(rig.root.join("empty")).unwrap();
    std::fs::create_dir_all(rig.root.join("bin")).unwrap();
    std::fs::write(rig.root.join("bin/run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(
        rig.root.join("bin/run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::os::unix::fs::symlink("src/lib.rs", rig.root.join("link")).unwrap();

    let progress = ProgressLog::new();
    let snapshot = FsSnapshot {
        work: None,
        environment_id: rig.env,
        job_id: orch_types::new_id(),
        selection: Selection::default(),
    };
    let first = rig
        .host
        .fs_snapshot(&snapshot, &rig.store, &progress)
        .await
        .unwrap();
    assert_eq!(
        first.file_count, 4,
        "four files; the ignored one is not among them"
    );
    assert!(
        first.uploaded_bytes >= 3_200_000,
        "everything went up the first time"
    );
    let uploads_after_first = rig.store.uploads().len();
    let mut as_snapshotted = tree_of(&rig.root);
    as_snapshotted.remove("target");
    as_snapshotted.remove("target/junk");
    assert!(
        !progress.reports().is_empty(),
        "the job reported its progress"
    );
    assert_eq!(progress.reports().last().unwrap().percent, 100);

    // One small file changes: only it and the new list go up.
    std::fs::write(rig.root.join("docs/readme.md"), "# docs, edited\n").unwrap();
    let second = rig
        .host
        .fs_snapshot(&snapshot, &rig.store, &ProgressLog::new())
        .await
        .unwrap();
    assert_eq!(
        rig.store.uploads().len(),
        uploads_after_first + 2,
        "the changed file and the new list; the big file stays where it is"
    );
    let list_bytes =
        i64::try_from(rig.store.get_bytes(&second.manifest_blob).unwrap().len()).unwrap();
    assert_eq!(
        second.uploaded_bytes,
        15 + list_bytes,
        "the changed file's bytes and the list's"
    );
    assert_ne!(second.manifest_blob, first.manifest_blob);

    // The list of the first snapshot becomes the tree of another environment, equal to the first one as it was then.
    let other = orch_types::new_id();
    let other_root = rig.dir.path().join("other");
    std::fs::create_dir_all(&other_root).unwrap();
    rig.host
        .env_attach(&body(other, &other_root), &BTreeMap::new())
        .await
        .unwrap();
    let materialize = FsMaterialize {
        work: None,
        environment_id: other,
        job_id: None,
        manifest_blob: first.manifest_blob,
        target_root: String::new(),
        purpose: MaterializePurpose::Move,
    };
    let report = rig
        .host
        .fs_materialize(&materialize, &rig.store, &ProgressLog::new())
        .await
        .unwrap();
    assert_eq!(report.files, 4);
    assert_eq!(
        tree_of(&other_root.canonicalize().unwrap()),
        as_snapshotted,
        "the trees are equal, including modes, links and empty directories"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_list_of_files_from_the_store_that_is_not_what_it_claims_to_be_writes_nothing() {
    let rig = rig(&[]).await;
    let junk = rig.store_text("this is no list of files");
    let materialize = FsMaterialize {
        work: None,
        environment_id: rig.env,
        job_id: None,
        manifest_blob: junk,
        target_root: String::new(),
        purpose: MaterializePurpose::Move,
    };
    assert!(
        rig.host
            .fs_materialize(&materialize, &rig.store, &ProgressLog::new())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&rig.root).unwrap().count(), 0);
    let absent = FsMaterialize {
        work: None,
        manifest_blob: Hash::of(b"not in the store"),
        ..materialize
    };
    assert!(
        rig.host
            .fs_materialize(&absent, &rig.store, &ProgressLog::new())
            .await
            .is_err()
    );
}

// ---------------------------------------------------------------------------------------------
// Watching and manifests

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_directory_that_is_watched_reports_what_appears_changes_and_goes() {
    let mut rig = rig(&[("a/old.txt", "old")]).await;
    let watch_id = orch_types::new_id();
    let listing = rig
        .host
        .fs_list(&FsList {
            environment_id: rig.env,
            mount: None,
            path: "a".into(),
            watch_id: Some(watch_id),
            watch_instance: None,
            page: None,
        })
        .await
        .unwrap();
    assert_eq!(listing.entries.len(), 1);

    let Report::FsChanged(initial) = rig
        .report(
            "the watched directory's initial state",
            |report| matches!(report, Report::FsChanged(data) if data.reset),
        )
        .await
    else {
        unreachable!()
    };
    assert_eq!(initial.watch_id, watch_id);
    assert_eq!(initial.changed[0].name, "old.txt");
    assert!(!initial.truncated);

    std::fs::write(rig.root.join("a/new.txt"), "new").unwrap();
    let Report::FsChanged(changed) = rig
        .report("the new file", |report| {
            matches!(report, Report::FsChanged(_))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(changed.watch_id, watch_id);
    assert_eq!(changed.changed.len(), 1);
    assert_eq!(changed.changed[0].name, "new.txt");
    assert!(changed.removed.is_empty());

    std::fs::remove_file(rig.root.join("a/old.txt")).unwrap();
    let Report::FsChanged(gone) = rig
        .report("the removed file", |report| {
            matches!(report, Report::FsChanged(_))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(gone.removed, ["old.txt"]);

    rig.host.fs_unwatch(&FsUnwatch { watch_id }).unwrap();
    std::fs::write(rig.root.join("a/after.txt"), "x").unwrap();
    rig.no_report("a change of a directory nobody watches", |report| {
        matches!(report, Report::FsChanged(_))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_complete_empty_index_publishes_its_first_manifest_and_then_stays_quiet() {
    let mut rig = rig(&[]).await;
    let Report::Manifest(first) = rig
        .report("the initial empty tree", |report| {
            matches!(report, Report::Manifest(_))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(first.file_count, 0);
    assert_eq!(first.total_bytes, 0);
    let bytes = rig
        .store
        .get_bytes(&first.manifest_blob)
        .expect("the initial empty manifest was uploaded");
    let tree = manifest::parse(&bytes).unwrap();
    assert!(tree.entries.is_empty());
    assert_eq!(tree.header.root, rig.root.display().to_string());
    rig.no_report("an unchanged empty tree", |report| {
        matches!(report, Report::Manifest(_))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_manifest_is_published_for_the_first_time_and_again_when_a_turn_completes_after_a_change()
{
    let mut rig = rig(&[("src/a.rs", "a"), ("src/b.rs", "bb")]).await;
    let Report::Manifest(first) = rig
        .report("the first manifest", |report| {
            matches!(report, Report::Manifest(_))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(first.file_count, 2);
    assert_eq!(first.total_bytes, 3);
    let listed = manifest::parse(
        &rig.store
            .get_bytes(&first.manifest_blob)
            .expect("the system holds the list"),
    )
    .unwrap();
    let described: Vec<(&str, i64)> = listed
        .entries
        .iter()
        .map(|entry| (entry.p.as_str(), entry.s))
        .collect();
    assert_eq!(described, [("src", 0), ("src/a.rs", 1), ("src/b.rs", 2)]);

    // A turn that completes without a change publishes nothing.
    rig.host.turn_completed(rig.env);
    rig.no_report("a manifest of an unchanged tree", |report| {
        matches!(report, Report::Manifest(_))
    })
    .await;

    // A change, and then the end of a turn, publishes at once rather than at the next interval.
    std::fs::write(rig.root.join("src/c.rs"), "ccc").unwrap();
    rig.host.turn_completed(rig.env);
    let Report::Manifest(second) = rig
        .report("the manifest after the turn", |report| {
            matches!(report, Report::Manifest(_))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(second.file_count, 3);
    assert_eq!(second.total_bytes, 6);
    assert_ne!(second.manifest_blob, first.manifest_blob);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_waiting_to_be_published_goes_out_before_the_machine_goes_offline() {
    let mut rig = rig(&[("a.txt", "a")]).await;
    rig.report("the first manifest", |report| {
        matches!(report, Report::Manifest(_))
    })
    .await;
    std::fs::write(rig.root.join("b.txt"), "b").unwrap();
    // The change is seen after the watch's quiet time; going offline publishes what is waiting then.
    let environment = rig.host.environment(rig.env).unwrap();
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !environment.index().is_dirty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the change was never seen"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    rig.host.before_offline().await;
    let Report::Manifest(last) = rig
        .report("the manifest before going offline", |report| {
            matches!(report, Report::Manifest(_))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(last.file_count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_measurements_asked_for_are_reported_and_stop_when_they_are_not_asked_for_any_more() {
    let mut rig = rig(&[]).await;
    let environment_id = rig.env;
    let ask = |stats, procs| WatchSet {
        environment_id,
        stats,
        procs,
        ports: false,
        terminals: false,
        interval_ms: 1000,
    };
    rig.host.watch_set(&ask(true, true)).await.unwrap();
    rig.report("a sample of what the environment uses", |report| {
        matches!(report, Report::Stats(_))
    })
    .await;
    rig.report("the processes", |report| {
        matches!(report, Report::Procs { .. })
    })
    .await;

    rig.host.watch_set(&ask(false, false)).await.unwrap();
    while rig.reports.try_recv().is_ok() {}
    rig.no_report("a measurement nobody asked for", |report| {
        matches!(report, Report::Stats(_) | Report::Procs { .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_ports_and_terminal_foreground_are_reported_only_while_their_views_are_watched() {
    let mut rig = rig(&[]).await;
    let terminal_id = orch_types::new_id();
    rig.host
        .watch_set(&WatchSet {
            environment_id: rig.env,
            stats: false,
            procs: false,
            ports: true,
            terminals: false,
            interval_ms: 1000,
        })
        .await
        .unwrap();
    let Report::Ports(initial) = rig
        .report("the initial scoped port observation", |report| {
            matches!(report, Report::Ports(_))
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(initial.request_ref, None);
    assert_eq!(initial.snapshot.environment_id, rig.env);
    rig.host
        .watch_set(&WatchSet {
            environment_id: rig.env,
            stats: false,
            procs: false,
            ports: true,
            terminals: true,
            interval_ms: 1000,
        })
        .await
        .unwrap();
    let Report::Terminals(initial) = rig
        .report("the initial terminal list", |report| {
            matches!(report, Report::Terminals(_))
        })
        .await
    else {
        unreachable!()
    };
    assert!(initial.terminals.is_empty());
    rig.host
        .pty_open(&PtyOpen {
            environment_id: rig.env,
            terminal_id,
            cols: 80,
            rows: 24,
            cwd: String::new(),
            record: false,
        })
        .await
        .unwrap();
    let Report::Terminals(opened) = rig.report("the new shell", |report|
        matches!(report, Report::Terminals(data) if data.terminals.iter().any(|shell| shell.terminal_id == terminal_id))).await
        else { unreachable!() };
    assert_eq!(opened.terminals.len(), 1);
    rig.host
        .watch_set(&WatchSet {
            environment_id: rig.env,
            stats: false,
            procs: false,
            ports: false,
            terminals: false,
            interval_ms: 1000,
        })
        .await
        .unwrap();
    while rig.reports.try_recv().is_ok() {}
    rig.host
        .pty_close(&orch_link_contract::commands::PtyClose { terminal_id })
        .await
        .unwrap();
    rig.no_report(
        "a foreground or port observation after the view closed",
        |report| matches!(report, Report::Ports(_) | Report::Terminals(_)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_git_view_gets_its_initial_state_and_native_changes_and_unwatch_stops_it() {
    if !git_exists() {
        return;
    }
    let mut rig = rig(&[("tracked.txt", "one\n")]).await;
    let git = |args: &[&str]| {
        let result = std::process::Command::new("git")
            .args(args)
            .current_dir(&rig.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", rig.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "tracked.txt"]);
    git(&["commit", "-q", "-m", "first"]);
    let watch_id = orch_types::new_id();
    let current = rig
        .host
        .git_status(&orch_link_contract::commands::GitStatus {
            environment_id: rig.env,
            path: None,
            watch_id: Some(watch_id),
        })
        .await
        .unwrap();
    assert!(current.available);
    assert!(current.changes.is_empty());
    let Report::GitChanged(initial) = rig
        .report(
            "the initial repository status",
            |report| matches!(report, Report::GitChanged(data) if data.watch_id == watch_id),
        )
        .await
    else {
        unreachable!()
    };
    assert_eq!(initial.status, current);
    std::fs::write(rig.root.join("tracked.txt"), "changed\n").unwrap();
    let Report::GitChanged(changed) = rig
        .report(
            "a native repository edit",
            |report| matches!(report, Report::GitChanged(data) if !data.status.changes.is_empty()),
        )
        .await
    else {
        unreachable!()
    };
    assert_eq!(changed.watch_id, watch_id);
    assert_eq!(changed.status.changes.len(), 1);
    rig.host.fs_unwatch(&FsUnwatch { watch_id }).unwrap();
    while rig.reports.try_recv().is_ok() {}
    std::fs::write(rig.root.join("tracked.txt"), "after unsubscribe\n").unwrap();
    rig.no_report("Git status after its view closed", |report| {
        matches!(report, Report::GitChanged(_))
    })
    .await;
}

// ---------------------------------------------------------------------------------------------
// Updating and detaching

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attaching_again_takes_the_new_ignore_rules_and_keeps_what_runs_in_the_environment() {
    let mut rig = rig(&[("visible.txt", "v"), ("hidden.txt", "h")]).await;
    let terminal_id = orch_types::new_id();
    let open = PtyOpen {
        environment_id: rig.env,
        terminal_id,
        cols: 80,
        rows: 24,
        cwd: String::new(),
        record: false,
    };
    rig.host.pty_open(&open).await.unwrap();
    let watch_id = orch_types::new_id();
    rig.host
        .fs_list(&FsList {
            environment_id: rig.env,
            mount: None,
            path: String::new(),
            watch_id: Some(watch_id),
            watch_instance: None,
            page: None,
        })
        .await
        .unwrap();
    rig.host
        .watch_set(&WatchSet {
            environment_id: rig.env,
            stats: true,
            procs: false,
            ports: false,
            terminals: false,
            interval_ms: 1000,
        })
        .await
        .unwrap();
    assert_eq!(rig.names("").await, ["hidden.txt", "visible.txt"]);

    let mut again = body(rig.env, &rig.root);
    again.ignore = vec!["hidden.txt".into()];
    again
        .variables
        .insert("ORCH_TEST_VARIABLE".into(), "after".into());
    rig.host.env_attach(&again, &BTreeMap::new()).await.unwrap();

    assert_eq!(
        rig.names("").await,
        ["visible.txt"],
        "the new rules are in force"
    );
    let mut screen = rig
        .host
        .pty_attach(terminal_id, true)
        .expect("the shell is still open");
    screen
        .send_input(b"echo variable-is-$ORCH_TEST_VARIABLE\nexit\n".to_vec())
        .unwrap();
    let mut seen = String::new();
    while !seen.contains("variable-is-") {
        match tokio::time::timeout(PATIENCE, screen.next()).await.unwrap() {
            Some(Event::Data(bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
            other => panic!("the shell ended before it answered: {other:?}; saw {seen:?}"),
        }
    }
    // Shells keep the variables they started with; a shell opened now gets the new ones.
    assert!(!seen.contains("variable-is-after"), "{seen}");

    rig.report("a sample, as asked before the update", |report| {
        matches!(report, Report::Stats(_))
    })
    .await;
    std::fs::write(rig.root.join("later.txt"), "x").unwrap();
    rig.report(
        "a change of the watched directory after the update",
        |report| matches!(report, Report::FsChanged(_)),
    )
    .await;
    rig.host.pty_close(&PtyClose { terminal_id }).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shells_a_previous_run_left_open_are_reported_as_ended_when_the_environment_is_attached_again()
 {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let env = orch_types::new_id();
    let (lost_one, lost_two) = (orch_types::new_id(), orch_types::new_id());
    // What the previous run wrote down before it stopped.
    let record_dir = dir.path().join("data/terminals").join(env.to_string());
    std::fs::create_dir_all(&record_dir).unwrap();
    std::fs::write(
        record_dir.join("shells.json"),
        format!(r#"{{"open":["{lost_one}","{lost_two}"]}}"#),
    )
    .unwrap();

    let settings = HostSettings::connected(dir.path().join("data"), vec![dir.path().to_path_buf()]);
    let (reports, mut receiver) = Reports::channel(16);
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );
    host.env_attach(&body(env, &root), &BTreeMap::new())
        .await
        .unwrap();

    let mut ended = Vec::new();
    while ended.len() < 2 {
        let reported = tokio::time::timeout(PATIENCE, receiver.recv())
            .await
            .expect("the lost shells were never reported")
            .unwrap();
        if let Report::PtyExited(exited) = reported.report {
            assert_eq!(reported.environment_id, env);
            assert_eq!(
                (exited.exit_code, exited.kept),
                (None, None),
                "nobody knows how a shell ended that the connector no longer has"
            );
            ended.push(exited.terminal_id);
        }
    }
    ended.sort();
    let mut expected = vec![lost_one, lost_two];
    expected.sort();
    assert_eq!(ended, expected);
    assert!(
        host.pty_attach(lost_one, true).is_err(),
        "there is nothing to attach to"
    );

    // Reported once: a second attach (a changed mount, say) has nothing more to say.
    host.env_attach(&body(env, &root), &BTreeMap::new())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if let Some(Reported {
                    report: Report::PtyExited(_),
                    ..
                }) = receiver.recv().await
                {
                    return;
                }
            }
        })
        .await
        .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detaching_ends_the_shells_of_the_environment_and_forgets_it() {
    let rig = rig(&[]).await;
    let terminal_id = orch_types::new_id();
    rig.host
        .pty_open(&PtyOpen {
            environment_id: rig.env,
            terminal_id,
            cols: 80,
            rows: 24,
            cwd: String::new(),
            record: false,
        })
        .await
        .unwrap();
    let pid = shell_pid(&rig.host, terminal_id).await;
    assert!(process_alive(pid));

    rig.host
        .env_detach(&EnvDetach {
            environment_id: rig.env,
        })
        .await
        .unwrap();
    wait_for_within(SHELL_PATIENCE, "the shell to end", || !process_alive(pid)).await;
    assert!(rig.host.pty_attach(terminal_id, true).is_err());
    assert_eq!(
        code_of(rig.host.fs_list(&rig.list("")).await),
        ErrorCode::NotFound
    );
    rig.host
        .env_detach(&EnvDetach {
            environment_id: rig.env,
        })
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------------------------
// Shells and processes

/// Asks the shell for its own process number.
async fn shell_pid(host: &EnvHost, terminal_id: Id) -> u32 {
    let mut screen = host.pty_attach(terminal_id, true).unwrap();
    screen
        .send_input(b"echo shell-pid=$$=end\n".to_vec())
        .unwrap();
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + SHELL_PATIENCE;
    loop {
        let event = tokio::time::timeout_at(deadline, screen.next())
            .await
            .expect("the shell never answered");
        match event {
            Some(Event::Data(bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
            other => panic!("the shell ended: {other:?}; saw {seen:?}"),
        }
        // The echo of what was typed has '$$' in it; the answer has digits.
        if let Some(found) = seen.split("shell-pid=").skip(1).find_map(|rest| {
            rest.split("=end")
                .next()
                .filter(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        }) {
            return found.parse().unwrap();
        }
    }
}

fn process_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|text| {
        text.rsplit_once(')')
            .is_some_and(|(_, rest)| !rest.trim_start().starts_with(['Z', 'X']))
    })
}

async fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    wait_for_within(PATIENCE, what, &mut condition).await;
}

async fn wait_for_within(patience: Duration, what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + patience;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_opened_through_the_host_runs_where_it_was_asked_with_the_variables_and_is_listed_as_a_process()
 {
    let rig = rig_with(&[("sub/file.txt", "x")], |_, attach| {
        attach
            .variables
            .insert("ORCH_TEST_VARIABLE".into(), "plain".into());
        attach.secrets.insert(
            "ORCH_TEST_SECRET".into(),
            orch_link_contract::commands::Secret::Ref(orch_types::new_id()),
        );
    })
    .await;
    // The caller unseals secrets; here the shell is given one by a second attach.
    let mut again = body(rig.env, &rig.root);
    again
        .variables
        .insert("ORCH_TEST_VARIABLE".into(), "plain".into());
    let secrets = BTreeMap::from([("ORCH_TEST_SECRET".to_owned(), "hunter2".to_owned())]);
    rig.host.env_attach(&again, &secrets).await.unwrap();

    let terminal_id = orch_types::new_id();
    let opened = rig
        .host
        .pty_open(&PtyOpen {
            environment_id: rig.env,
            terminal_id,
            cols: 100,
            rows: 30,
            cwd: "sub".into(),
            record: false,
        })
        .await
        .unwrap();
    assert_eq!(opened.service, format!("pty/{terminal_id}"));
    let mut screen = rig.host.pty_attach(terminal_id, true).unwrap();
    screen
        .send_input(b"pwd; echo v=$ORCH_TEST_VARIABLE s=$ORCH_TEST_SECRET\n".to_vec())
        .unwrap();
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + SHELL_PATIENCE;
    while !seen.contains("v=plain s=hunter2") {
        match tokio::time::timeout_at(deadline, screen.next())
            .await
            .expect("the shell never answered")
        {
            Some(Event::Data(bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
            other => panic!("the shell ended: {other:?}; saw {seen:?}"),
        }
    }
    assert!(
        seen.contains(&rig.root.join("sub").to_string_lossy().into_owned()),
        "it starts in the directory asked for: {seen}"
    );
    drop(screen);

    let pid = shell_pid(&rig.host, terminal_id).await;
    let listed = rig
        .host
        .proc_list(&orch_link_contract::commands::ProcList {
            environment_id: rig.env,
        })
        .await
        .unwrap();
    assert!(
        listed.processes.iter().any(|process| process.pid == pid),
        "the shell is among the processes of the environment"
    );

    let outside = PtyOpen {
        environment_id: rig.env,
        terminal_id: orch_types::new_id(),
        cols: 80,
        rows: 24,
        cwd: "../".into(),
        record: false,
    };
    assert!(
        rig.host.pty_open(&outside).await.is_err(),
        "a shell starts inside the workspace"
    );
    let closed = rig.host.pty_close(&PtyClose { terminal_id }).await.unwrap();
    assert!(closed.bytes_out > 0);
    wait_for_within(SHELL_PATIENCE, "the shell to end", || !process_alive(pid)).await;
    assert!(
        rig.host.pty_close(&PtyClose { terminal_id }).await.is_err(),
        "a shell that is closed is gone"
    );
}

/// Processes a test starts, ended when the test ends: each in a group of its own, so only those
/// groups and numbers are ever signalled.
struct Sleepers {
    children: Vec<std::process::Child>,
}

impl Sleepers {
    fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }

    fn start(&mut self) -> u32 {
        let mut command = std::process::Command::new("sleep");
        command
            .arg("300")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        orch_envhost::platform::prepare_group(&mut command);
        let child = command.spawn().unwrap();
        let pid = child.id();
        self.children.push(child);
        pid
    }
}

impl Drop for Sleepers {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_reaches_a_process_the_host_started_and_nobody_else() {
    use orch_envhost::processes::RootKind;
    use orch_link_contract::commands::{ProcList, ProcResume, ProcSignal, ProcSuspend, Signal};
    let rig = rig(&[]).await;
    let mut sleepers = Sleepers::new();
    let bystander = sleepers.start();

    let environment = rig.host.environment(rig.env).unwrap();
    let mut command = std::process::Command::new("sleep");
    command
        .arg("300")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let ours = environment
        .processes()
        .spawn_tracked(&mut command, RootKind::Executor)
        .unwrap();
    let ours_pid = ours.id();
    sleepers.children.push(ours);
    wait_for("the process to be sleep", || {
        std::fs::read_to_string(format!("/proc/{ours_pid}/cmdline"))
            .is_ok_and(|text| text.starts_with("sleep"))
    })
    .await;

    let listed = rig
        .host
        .proc_list(&ProcList {
            environment_id: rig.env,
        })
        .await
        .unwrap();
    assert!(
        listed
            .processes
            .iter()
            .any(|process| process.pid == ours_pid)
    );
    assert!(
        listed
            .processes
            .iter()
            .all(|process| process.pid != bystander),
        "a process the host did not start is not the environment's"
    );
    let started_at = listed
        .processes
        .iter()
        .find(|process| process.pid == ours_pid)
        .map(|process| process.started_at)
        .unwrap();

    let signal = |pid: u32, started_at, signal| ProcSignal {
        environment_id: rig.env,
        pid: i64::from(pid),
        started_at,
        signal,
    };
    assert!(
        rig.host
            .proc_signal(&signal(bystander, started_at, Signal::Kill))
            .await
            .is_err(),
        "not the environment's process"
    );
    assert!(process_alive(bystander), "the bystander was not touched");
    let wrong_start =
        orch_types::Time::new(chrono::DateTime::from_timestamp(1_000_000, 0).unwrap());
    assert!(
        rig.host
            .proc_signal(&signal(ours_pid, wrong_start, Signal::Kill))
            .await
            .is_err(),
        "the number now belongs to another process, as far as the caller knows"
    );
    assert!(process_alive(ours_pid));

    // Held still, then let go.
    let held = rig
        .host
        .proc_suspend(&ProcSuspend {
            environment_id: rig.env,
        })
        .await
        .unwrap();
    assert!(held.processes >= 1);
    wait_for("the process to be stopped", || {
        std::fs::read_to_string(format!("/proc/{ours_pid}/stat"))
            .is_ok_and(|text| text.contains(") T "))
    })
    .await;
    let released = rig
        .host
        .proc_resume(&ProcResume {
            environment_id: rig.env,
        })
        .await
        .unwrap();
    assert_eq!(released.processes, held.processes);
    wait_for("the process to run again", || {
        std::fs::read_to_string(format!("/proc/{ours_pid}/stat"))
            .is_ok_and(|text| !text.contains(") T "))
    })
    .await;

    rig.host
        .proc_signal(&signal(ours_pid, started_at, Signal::Term))
        .await
        .unwrap();
    wait_for("the process to end", || !process_alive(ours_pid)).await;
    assert!(process_alive(bystander), "still untouched");
}

// ---------------------------------------------------------------------------------------------
// Mounts, executors and repositories

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mount_that_is_attached_in_place_is_listed_and_read_by_name_and_stays_inside_itself() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("shared-data");
    write_tree(&source, &[("table.csv", "a,b\n1,2\n")]);
    std::fs::write(dir.path().join("private.txt"), "not part of the mount").unwrap();
    let mount_id = orch_types::new_id();
    let mut settings =
        HostSettings::connected(dir.path().join("data"), vec![dir.path().to_path_buf()]);
    settings.gates = Gates::OPEN;
    let (reports, _receiver) = Reports::channel(16);
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let env = orch_types::new_id();
    let mut attach = body(env, &root);
    attach.mounts = Some(vec![EnvAttachMount {
        volume: None,
        mount_id,
        name: "data".into(),
        source: source.to_string_lossy().into_owned(),
        mode: orch_link_contract::commands::AttachMode::ReadOnly,
    }]);
    let facts = host.env_attach(&attach, &BTreeMap::new()).await.unwrap();
    assert_eq!(facts.mounts.len(), 1);
    assert_eq!(facts.mounts[0].name, "data");

    let listed = host
        .fs_list(&FsList {
            environment_id: env,
            mount: Some("data".into()),
            path: String::new(),
            watch_id: None,
            watch_instance: None,
            page: None,
        })
        .await
        .unwrap();
    assert_eq!(listed.entries.len(), 1);
    let store = MemoryBlobs::new();
    let fetch = FsFetch {
        environment_id: env,
        mount: Some("data".into()),
        path: "table.csv".into(),
        expect: None,
        max_bytes: None,
    };
    assert_eq!(
        host.fs_fetch(&fetch, &store).await.unwrap().hash,
        Hash::of(b"a,b\n1,2\n")
    );
    let escape = FsFetch {
        path: "../private.txt".into(),
        ..fetch.clone()
    };
    assert!(
        host.fs_fetch(&escape, &store).await.is_err(),
        "a mount ends where it ends"
    );
    let unknown = FsFetch {
        mount: Some("nothing".into()),
        ..fetch
    };
    assert_eq!(
        code_of(host.fs_fetch(&unknown, &store).await),
        ErrorCode::NotFound
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_executor_is_started_only_where_the_machines_owner_lets_its_exec_node_offer_it() {
    use async_trait::async_trait;
    use orch_agent_contract::adapter::AdapterError;
    use orch_agent_contract::descriptor::SandboxMode;
    use orch_envhost::environment::ExecutorFactory;
    use orch_executor_contract::context::EnvironmentContext;
    use orch_executor_contract::descriptor::{ExecutorAvailability, ExecutorDescriptor};
    use orch_executor_contract::executor::{Executor, ExecutorEndpoint};

    struct Stub {
        started_in: Arc<std::sync::Mutex<Option<EnvironmentContext>>>,
    }

    #[async_trait]
    impl Executor for Stub {
        fn kind(&self) -> &'static str {
            "stub-exec"
        }
        fn describe(&self) -> ExecutorDescriptor {
            serde_json::from_value(serde_json::json!({
                "kind": "stub-exec", "title": "A stand-in", "protocol": "mcp",
                "tool_sets": [{ "name": "plain", "summary": "Plain tools." }],
                "sandbox_modes": ["open"],
                "attribution": { "thread_id": "ORCH_THREAD_ID", "tool_call_id": "ORCH_ITEM_ID" }
            }))
            .unwrap()
        }
        async fn available(&self, _env: &EnvironmentContext) -> ExecutorAvailability {
            ExecutorAvailability::available("0.1.0", vec![SandboxMode::Open])
        }
        async fn start(
            &mut self,
            env: &EnvironmentContext,
        ) -> Result<ExecutorEndpoint, AdapterError> {
            *self.started_in.lock().unwrap() = Some(env.clone());
            Ok(ExecutorEndpoint {
                address: "127.0.0.1:41999".into(),
                token: None,
                version: "0.1.0".into(),
                tool_sets: vec![],
                sandbox_modes: vec![SandboxMode::Open],
            })
        }
        async fn stop(&mut self) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    let started_in = Arc::new(std::sync::Mutex::new(None));
    let make = |allowed: Option<Vec<String>>| {
        let started_in = Arc::clone(&started_in);
        let factory: ExecutorFactory = Arc::new(move || {
            Box::new(Stub {
                started_in: Arc::clone(&started_in),
            })
        });
        (allowed, factory)
    };

    // The owner offers nothing: refused, and nothing started.
    let (allowed, factory) = make(Some(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let mut settings =
        HostSettings::connected(dir.path().join("data"), vec![dir.path().to_path_buf()]);
    settings.exec_offers_allowed = allowed;
    let (reports, mut receiver) = Reports::channel(16);
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        vec![factory],
    );
    let env = orch_types::new_id();
    let facts = host
        .env_attach(&body(env, &root), &BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(facts.executors.len(), 1);
    assert!(facts.executors[0].available);
    let start = orch_link_contract::commands::ExecStart {
        environment_id: env,
        executor: "stub-exec".into(),
    };
    assert_eq!(
        code_of(host.exec_start(&start).await),
        ErrorCode::PolicyExecNotOffered
    );
    assert!(started_in.lock().unwrap().is_none());
    assert_eq!(
        code_of(
            host.exec_start(&orch_link_contract::commands::ExecStart {
                executor: "no-such".into(),
                ..start.clone()
            })
            .await
        ),
        ErrorCode::NotFound
    );

    // The owner offers it (by its kind): started, with the environment as the executor needs it.
    host.set_policy(
        Gates::OPEN,
        Some(vec![dir.path().to_path_buf()]),
        Some(vec!["stub-exec".into()]),
        orch_envhost::mcp::McpGates {
            import: true,
            add: false,
        },
    );
    let started = host.exec_start(&start).await.unwrap();
    assert_eq!(
        started.service, "mcp/exec",
        "an executor that offers MCP tools is reached by the server that is called exec"
    );
    let context = started_in
        .lock()
        .unwrap()
        .clone()
        .expect("the executor was started");
    assert_eq!(context.environment_id, env);
    assert_eq!(
        context.workspace_root,
        root.canonicalize().unwrap().to_string_lossy()
    );
    assert!(
        context.data_dir.starts_with(&*dir.path().to_string_lossy()),
        "an executor keeps its files under the connector's directory: {}",
        context.data_dir
    );
    while let Ok(Some(reported)) =
        tokio::time::timeout(Duration::from_millis(200), receiver.recv()).await
    {
        assert_eq!(reported.environment_id, env);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_server_definition_is_kept_by_the_connector_only_and_forgotten_on_request() {
    use orch_agent_contract::discovery::McpDefinition;
    use orch_link_contract::commands::McpImport;
    let rig = rig(&[]).await;
    let server_id = orch_types::new_id();
    let definition: McpDefinition = serde_json::from_value(serde_json::json!({
        "spec": { "transport": "stdio", "command": ["github-mcp"], "secret_env": ["GITHUB_TOKEN"] },
        "secrets": { "GITHUB_TOKEN": "ghp_very_secret" }
    }))
    .unwrap();
    let request = McpImport {
        expected_fingerprint: None,
        environment_id: rig.env,
        server_id,
        source: "codex".into(),
        scope: "user".into(),
        name: "github".into(),
        directory: None,
    };

    let answer = rig.host.mcp_import(&request, &definition, Some(3)).unwrap();
    assert_eq!(
        (answer.transport.as_str(), answer.tools),
        ("stdio", Some(3))
    );
    assert!(
        !serde_json::to_string(&answer)
            .unwrap()
            .contains("ghp_very_secret"),
        "the system is told the shape and never the secret"
    );
    let copy = rig
        .dir
        .path()
        .join("data/mcp")
        .join(format!("{server_id}.json"));
    assert!(copy.is_file());
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
            0o600,
            "only the connector's user reads it"
        );
    }

    rig.host.mcp_forget(server_id).unwrap();
    assert!(!copy.exists());
    rig.host.mcp_forget(server_id).unwrap();

    // A machine that lets none be imported refuses, and keeps nothing.
    rig.host.set_policy(
        Gates::OPEN,
        Some(vec![rig.dir.path().to_path_buf()]),
        None,
        orch_envhost::mcp::McpGates {
            import: false,
            add: false,
        },
    );
    assert_eq!(
        code_of(rig.host.mcp_import(&request, &definition, None)),
        ErrorCode::PolicyMcpImportDisabled
    );
    assert!(!copy.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_state_of_a_repository_is_read_through_the_host() {
    if !git_exists() {
        eprintln!("git is not installed here; skipped");
        return;
    }
    let rig = rig(&[("tracked.txt", "one\n")]).await;
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&rig.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", rig.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "tracked.txt"]);
    git(&["commit", "-q", "-m", "first"]);
    std::fs::write(rig.root.join("tracked.txt"), "one\ntwo\n").unwrap();

    let status = rig
        .host
        .git_status(&orch_link_contract::commands::GitStatus {
            environment_id: rig.env,
            path: None,
            watch_id: None,
        })
        .await
        .unwrap();
    assert!(status.available);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.changes.len(), 1);
    let diff = rig
        .host
        .git_diff(&orch_link_contract::commands::GitDiff {
            environment_id: rig.env,
            path: None,
            staged: None,
        })
        .await
        .unwrap();
    assert!(diff.text.contains("+two"), "{}", diff.text);
    assert!(!diff.truncated);
}
