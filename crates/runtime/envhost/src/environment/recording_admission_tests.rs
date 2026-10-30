use super::*;
use crate::blobs::{BlobError, BlobWriter, MemoryBlobs};
use crate::processes::ScopeKind;
use crate::snapshot::NoProgress;
use crate::state_store::StateRoot;
use orch_link_contract::commands::IndexMode;
use orch_link_contract::source::{
    EnvironmentDataPart, FsDataExport, FsDataImport, FsRecord, FsRecordRead, RecordKind,
    RecordState,
};
use orch_types::{SystemClock, new_id};
use tokio::sync::Semaphore;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PATIENCE: Duration = Duration::from_secs(30);

pub(super) struct ReadGate {
    entered: Semaphore,
    released: Semaphore,
}
impl ReadGate {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            released: Semaphore::new(0),
        }
    }
    pub(super) async fn pass(&self) {
        self.entered.add_permits(1);
        if let Ok(permit) = self.released.acquire().await {
            permit.forget();
        }
    }
    async fn entered(&self) -> TestResult {
        tokio::time::timeout(PATIENCE, self.entered.acquire())
            .await??
            .forget();
        Ok(())
    }
    fn release(&self) {
        self.released.add_permits(1);
    }
}

struct Rig {
    host: EnvHost,
    attach: EnvAttach,
    request: FsRecord,
    blobs: MemoryBlobs,
    retained: Vec<(PathBuf, Vec<u8>)>,
    _reports: tokio::sync::mpsc::Receiver<Reported>,
    _dir: tempfile::TempDir,
}

impl Rig {
    async fn new() -> TestResult<Self> {
        let dir = tempfile::tempdir()?;
        let workspace = dir.path().join("workspace");
        let home = dir.path().join("home");
        std::fs::create_dir(&workspace)?;
        std::fs::create_dir(&home)?;
        std::fs::write(workspace.join("code.bin"), b"original\0\xffbytes")?;
        let mut settings = HostSettings::managed(
            dir.path().join("connector"),
            ManagedPlaces {
                workspace_root: workspace.clone(),
                home: Some(home.clone()),
                mounts_file: None,
                tools_dir: None,
                mounts_enforced: false,
            },
            ScopeKind::Started,
        );
        settings.generation = 7;
        let blobs = MemoryBlobs::new();
        let (reports, receiver) = Reports::channel(256);
        let host = EnvHost::new(
            settings,
            Arc::new(SystemClock),
            reports,
            Arc::new(blobs.clone()),
            Vec::new(),
        );
        let environment = new_id();
        let attach = EnvAttach {
            resources: None,
            data_generation: None,
            storage: None,
            data_sources: None,
            environment_id: environment,
            workspace_root: Some(workspace.to_string_lossy().into_owned()),
            ignore: Vec::new(),
            index: IndexMode::Full,
            mounts: None,
            variables: BTreeMap::new(),
            secrets: BTreeMap::new(),
        };
        host.env_attach(&attach, &BTreeMap::new()).await?;
        let attached = host.environment(environment)?;
        tokio::time::timeout(PATIENCE, async {
            while !attached.initial_scan_complete() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let request = FsRecord {
            environment_id: environment,
            snapshot_id: new_id(),
            directory: String::new(),
            kind: RecordKind::Turn,
            record_days: crate::file_states::DEFAULT_RECORD_DAYS,
            max_bytes: crate::file_states::DEFAULT_RECORD_MAX_BYTES,
        };
        let recorded = host.fs_record(&request, &blobs).await?;
        assert_eq!(recorded.state, RecordState::Complete);
        let store = home.join(".orch/file-states").join(environment.to_string());
        let path = store
            .join("roots")
            .join(format!("{}.json", request.snapshot_id));
        let bytes = std::fs::read(&path)?;
        let root: StateRoot = serde_json::from_slice(&bytes)?;
        let mut retained = vec![(path, bytes)];
        for oid in root.objects.values().chain(std::iter::once(&root.tree_ref)) {
            let path = store.join("git/objects").join(&oid[..2]).join(&oid[2..]);
            let bytes = std::fs::read(&path)?;
            retained.push((path, bytes));
        }
        Ok(Self {
            host,
            attach,
            request,
            blobs,
            retained,
            _reports: receiver,
            _dir: dir,
        })
    }

    fn read(&self) -> FsRecordRead {
        FsRecordRead {
            environment_id: self.request.environment_id,
            snapshot_id: self.request.snapshot_id,
            path: Some("code.bin".to_owned()),
            export: false,
        }
    }

    fn unchanged(&self) -> TestResult {
        for (path, bytes) in &self.retained {
            assert_eq!(&std::fs::read(path)?, bytes, "{}", path.display());
        }
        Ok(())
    }

    async fn reads_original(&self) -> TestResult {
        let read = self
            .host
            .fs_record_read(&self.read(), new_id(), &self.blobs)
            .await?;
        assert_eq!(
            self.blobs
                .get_bytes(&read.blob)
                .ok_or("recorded bytes absent")?
                .as_ref(),
            b"original\0\xffbytes"
        );
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_paused_after_attachment_lookup_cannot_overtake_reserved_or_completed_detach()
-> TestResult {
    for (finish_detach, replace_attachment) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let rig = Rig::new().await?;
        let gate = Arc::new(ReadGate::new());
        *lock(&rig.host.inner.before_record_pin) = Some(Arc::clone(&gate));
        let host = rig.host.clone();
        let request = rig.read();
        let blobs = rig.blobs.clone();
        let read =
            tokio::spawn(async move { host.fs_record_read(&request, new_id(), &blobs).await });
        gate.entered().await?; // The real command now owns the old environment Arc, but no pin.
        if replace_attachment {
            // In-place updates/recovery may have replaced the map's Arc before later detach.
            rig.host.env_attach(&rig.attach, &BTreeMap::new()).await?;
        }
        let mut reservation = Some(rig.host.reserve_detach(rig.request.environment_id)?);
        if finish_detach {
            rig.host
                .detach_reserved(reservation.as_ref().ok_or("reservation absent")?)
                .await?;
            drop(reservation.take()); // Prove retirement survives the exclusive lease ending.
        }
        gate.release();
        let Err(error) = tokio::time::timeout(PATIENCE, read).await?? else {
            return Err("a delayed reader escaped destructive admission".into());
        };
        assert_eq!(
            error.command_error().code,
            if finish_detach || replace_attachment {
                "not_found"
            } else {
                "busy"
            }
        );
        assert!(
            rig.blobs
                .get_bytes(&Hash::of(b"original\0\xffbytes"))
                .is_none(),
            "refused pin admission starts no transfer"
        );
        rig.unchanged()?;
        if let Some(reservation) = reservation {
            rig.host.detach_reserved(&reservation).await?;
        }
        *lock(&rig.host.inner.before_record_pin) = None;
        // A deliberate new attachment may reopen the store; it never revives the old handle.
        rig.host.env_attach(&rig.attach, &BTreeMap::new()).await?;
        rig.reads_original().await?;
        rig.host
            .env_detach(&EnvDetach {
                environment_id: rig.request.environment_id,
            })
            .await?;
    }
    Ok(())
}

struct HeldSource {
    blobs: MemoryBlobs,
    hash: Hash,
    gate: Arc<ReadGate>,
}
#[async_trait::async_trait]
impl BlobSource for HeldSource {
    async fn get(
        &self,
        hash: Hash,
        target: &mut dyn BlobWriter,
    ) -> std::result::Result<i64, BlobError> {
        if hash == self.hash {
            self.gate.pass().await;
        }
        self.blobs.get(hash, target).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn file_state_import_holds_admission_while_its_source_is_paused_and_releases_on_completion_or_cancel()
-> TestResult {
    for cancel_import in [false, true] {
        let rig = Rig::new().await?;
        let exported = rig
            .host
            .fs_data_export(
                &FsDataExport {
                    work: None,
                    environment_id: rig.request.environment_id,
                    job_id: new_id(),
                    part: EnvironmentDataPart::FileStates,
                },
                &rig.blobs,
                &NoProgress,
            )
            .await?;
        let gate = Arc::new(ReadGate::new());
        let source = HeldSource {
            blobs: rig.blobs.clone(),
            hash: exported.manifest_blob,
            gate: Arc::clone(&gate),
        };
        let body = FsDataImport {
            work: None,
            environment_id: rig.request.environment_id,
            job_id: new_id(),
            part: EnvironmentDataPart::FileStates,
            manifest_blob: exported.manifest_blob,
        };
        let host = rig.host.clone();
        let import =
            tokio::spawn(async move { host.fs_data_import(&body, &source, &NoProgress).await });
        gate.entered().await?; // Real import has reserved admission and is waiting on the manifest.
        let Err(error) = rig
            .host
            .fs_record_read(&rig.read(), new_id(), &rig.blobs)
            .await
        else {
            return Err("an unkeyed read acquired a pin during destructive import".into());
        };
        assert_eq!(error.command_error().code, "busy");
        assert!(error.command_error().retryable);
        let Err(error) = rig
            .host
            .env_detach(&EnvDetach {
                environment_id: rig.request.environment_id,
            })
            .await
        else {
            return Err("detach bypassed an importing store".into());
        };
        assert_eq!(error.command_error().code, "busy");
        rig.unchanged()?;
        if cancel_import {
            import.abort();
            let joined = tokio::time::timeout(PATIENCE, import).await?;
            assert!(joined.is_err_and(|error| error.is_cancelled()));
        } else {
            gate.release();
            tokio::time::timeout(PATIENCE, import).await???;
        }
        rig.unchanged()?;
        rig.reads_original().await?;
        rig.host
            .env_detach(&EnvDetach {
                environment_id: rig.request.environment_id,
            })
            .await?;
    }
    Ok(())
}
