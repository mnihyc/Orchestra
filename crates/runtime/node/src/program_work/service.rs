//! The node keeps isolated work and exact process identities behind an authenticated program channel.
use super::{Record, io::Live};
use crate::{drivers::DriverHost, ledger::Ledger};
use async_trait::async_trait;
use orch_connector_core::tunnels::TunnelServer;
use orch_driver_contract::{
    driver::DriverError,
    work::{WorkFile, WorkProcessSpec, WorkUnitSpec},
};
use orch_link::{
    machine::MachineLink,
    program_work::{Answer, read_json, write_json},
    tunnels::channel::{IncomingTunnel, Tunnel},
};
use orch_link_contract::{
    program_work::{Reply, Request},
    tunnels::{OpenErrorCode, Service, TicketClaims, TicketSource},
};
use orch_types::{Clock, Hash, Id};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError, Weak},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// No control method takes an actor or reconstructs source authority from work metadata.
pub(crate) struct Programs {
    drivers: Arc<DriverHost>,
    ledger: Arc<Ledger>,
    clock: Arc<dyn Clock>,
    link: MachineLink,
    runtimes: crate::runtimes::Runtimes,
    grace: Duration,
    stop: CancellationToken,
    locks: Mutex<BTreeMap<Id, Weak<tokio::sync::Mutex<()>>>>,
    live: Mutex<BTreeMap<(Id, Id), Arc<Live>>>,
}
impl Programs {
    pub(crate) fn new(
        drivers: Arc<DriverHost>,
        ledger: Arc<Ledger>,
        clock: Arc<dyn Clock>,
        link: MachineLink,
        runtimes: crate::runtimes::Runtimes,
        grace: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            drivers,
            ledger,
            clock,
            link,
            runtimes,
            grace,
            stop: CancellationToken::new(),
            locks: Mutex::new(BTreeMap::new()),
            live: Mutex::new(BTreeMap::new()),
        })
    }
    fn lock(&self, work: Id) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&work).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(work, Arc::downgrade(&lock));
        lock
    }
    fn current(
        &self,
        work: Id,
        claims: &TicketClaims,
        from: TicketSource,
    ) -> Result<Record, DriverError> {
        let record = self
            .ledger
            .program(work)
            .filter(|record| record.accepts_channel(claims, from, self.clock.time()))
            .ok_or_else(|| {
                DriverError::Unavailable(
                    "the admitted program work or its channel has ended".into(),
                )
            })?;
        if self.stop.is_cancelled() {
            return Err(DriverError::Unavailable("the node is stopping".into()));
        }
        Ok(record)
    }
    fn process(&self, work: Id, invocation: Id) -> Result<Arc<Live>, DriverError> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner).get(&(work,invocation)).cloned()
            .ok_or_else(||DriverError::Unavailable("the existing program stream is unavailable; its invocation was not started again".into()))
    }
    async fn prepare(&self, record: &Record, files: Vec<WorkFile>) -> Result<(), DriverError> {
        let parent = self
            .ledger
            .get(record.admission.parent)
            .filter(|parent| parent.driver == record.driver && parent.unit == record.provider)
            .ok_or_else(|| {
                DriverError::Unavailable("the program's actual provider placement is gone".into())
            })?;
        let driver = self.drivers.get(&record.driver).ok_or_else(|| {
            DriverError::Unavailable("the admitted program driver is unavailable".into())
        })?;
        let hashes: Vec<_> = files
            .iter()
            .map(|file| json!({"path":file.path,"hash":Hash::of(&file.bytes)}))
            .collect();
        let input = Hash::of(&serde_json::to_vec(&hashes).map_err(|_| {
            DriverError::InvalidSpec("the frozen program material is invalid".into())
        })?);
        if record.input.is_some_and(|old| old != input) {
            return Err(DriverError::InvalidSpec(
                "this work already has different frozen material".into(),
            ));
        }
        let unit = driver
            .contract()
            .create_work(&WorkUnitSpec {
                work: record.admission.work,
                provider: parent.unit,
                template: record.template.clone(),
                tools: record.tools.clone(),
                endpoints: record.admission.endpoints.clone(),
                deadline: record.admission.deadline,
                files,
            })
            .await?;
        if !self
            .ledger
            .bind_program_unit(record.admission.work, &unit, input)
            .map_err(|_| {
                DriverError::Unavailable("the prepared program unit could not be journaled".into())
            })?
        {
            return Err(DriverError::InvalidSpec(
                "the prepared program unit differs from its work journal".into(),
            ));
        }
        Ok(())
    }
    async fn start(
        &self,
        record: &Record,
        invocation: Id,
        spec: WorkProcessSpec,
    ) -> Result<(), DriverError> {
        let options = Hash::of(
            &serde_json::to_vec(&spec)
                .map_err(|_| DriverError::InvalidSpec("the program options are invalid".into()))?,
        );
        if let Ok(live) = self.process(record.admission.work, invocation) {
            if live.options != options {
                return Err(DriverError::InvalidSpec(
                    "this invocation already has different program options".into(),
                ));
            }
            return live.process.start().await;
        }
        if record.processes.contains_key(&invocation) {
            return Err(DriverError::Unavailable("the existing program stream was interrupted; its retained invocation was not started again".into()));
        }
        let unit = record.unit.as_ref().ok_or_else(|| {
            DriverError::Unavailable("the isolated program unit is not prepared".into())
        })?;
        let driver = self.drivers.get(&record.driver).ok_or_else(|| {
            DriverError::Unavailable("the admitted program driver is unavailable".into())
        })?;
        let process = driver.contract().prepare_work_process(unit, &spec).await?;
        if !self
            .ledger
            .retain_program_process(
                record.admission.work,
                invocation,
                process.reference(),
                options,
            )
            .map_err(|_| {
                DriverError::Unavailable(
                    "the prepared program invocation could not be journaled".into(),
                )
            })?
        {
            return Err(DriverError::InvalidSpec(
                "the program invocation differs from its work journal".into(),
            ));
        }
        let live = Arc::new(Live::new(process, options));
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((record.admission.work, invocation), live.clone());
        live.process.start().await
    }
    async fn write(
        &self,
        work: Id,
        invocation: Id,
        offset: u64,
        bytes: &[u8],
    ) -> Result<Reply, DriverError> {
        self.process(work, invocation)?.write(offset, bytes).await
    }

    async fn output(&self, work: Id, invocation: Id, offset: u64) -> Result<Answer, DriverError> {
        self.process(work, invocation)?
            .output(offset, self.link.message_bytes())
            .await
    }

    pub(crate) async fn retire(&self, work: Id) -> Result<(), DriverError> {
        let lock = self.lock(work);
        let _held = lock.lock().await;
        let Some(record) = self.ledger.program(work) else {
            return Ok(());
        };
        if record.cleaned {
            return Ok(());
        }
        self.ledger.retire_program(work).map_err(|_| {
            DriverError::Unavailable("the program retirement could not be journaled".into())
        })?;
        let driver = self.drivers.get(&record.driver).ok_or_else(|| {
            DriverError::Unavailable("the admitted program driver is unavailable".into())
        })?;
        match driver.contract().retire_work(work, &record.system).await {
            Ok(()) => {}
            Err(DriverError::Unsupported(_)) if record.unit.is_some() => {
                let unit = record.unit.as_ref().ok_or_else(|| {
                    DriverError::Unavailable("the isolated program unit is not prepared".into())
                })?;
                if driver.contract().inspect(unit).await?.state
                    != orch_driver_contract::units::UnitStateKind::Absent
                {
                    driver.contract().stop(unit, self.grace).await?;
                    driver.contract().remove(unit).await?;
                }
            }
            Err(error) => return Err(error),
        }
        if !self
            .ledger
            .finish_program_unit(work, record.unit.as_ref())
            .map_err(|_| {
                DriverError::Unavailable(
                    "the completed program cleanup could not be journaled".into(),
                )
            })?
        {
            return Err(DriverError::Unavailable(
                "the isolated program cleanup changed while it was retained".into(),
            ));
        }
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(owner, _), _| *owner != work);
        Ok(())
    }
    pub(crate) async fn sweep(&self) {
        for record in self.ledger.programs() {
            if !record.cleaned
                && (record.retired || self.clock.time() >= record.admission.deadline)
                && let Err(error) = self.retire(record.admission.work).await
            {
                tracing::debug!(
                    error_kind = error.code(),
                    "an isolated program unit still needs cleanup"
                );
            }
        }
        if let Err(_error) = self.ledger.prune_programs(self.clock.time()) {
            tracing::debug!("expired program cleanup records could not yet be released");
        }
    }
    pub(crate) fn stop(&self) {
        self.stop.cancel();
    }

    async fn handle(
        &self,
        work: Id,
        claims: &TicketClaims,
        from: TicketSource,
        tunnel: &mut Tunnel,
    ) -> Result<Answer, DriverError> {
        let request: Request =
            read_json(tunnel, self.link.message_bytes())
                .await
                .map_err(|_| {
                    DriverError::InvalidSpec("the complete program request is unreadable".into())
                })?;
        let lock = self.lock(work);
        let _resources = if matches!(request, Request::Prepare { .. } | Request::Start { .. }) {
            Some(self.runtimes.hold_resources().await)
        } else {
            None
        };
        let _held = if matches!(request, Request::Prepare { .. } | Request::Start { .. }) {
            Some(lock.lock().await)
        } else {
            None
        };
        let record = self.current(work, claims, from)?;
        let reply = match request {
            Request::Prepare { files } => {
                let mut material = Vec::with_capacity(files.len());
                for file in files {
                    let mut bytes =
                        vec![
                            0;
                            usize::try_from(file.bytes).map_err(|_| DriverError::InvalidSpec(
                                "the frozen file cannot fit this platform".into()
                            ))?
                        ];
                    tunnel.read_exact(&mut bytes).await.map_err(|_| {
                        DriverError::InvalidSpec(
                            "a frozen program file was not completely received".into(),
                        )
                    })?;
                    material.push(WorkFile {
                        path: file.path,
                        bytes,
                    });
                }
                self.current(work, claims, from)?;
                self.prepare(&record, material).await?;
                Reply::Ready
            }
            Request::Start {
                invocation,
                process,
            } => {
                self.start(&record, invocation, process).await?;
                Reply::Started {
                    reference: self
                        .process(work, invocation)?
                        .process
                        .reference()
                        .to_owned(),
                }
            }
            Request::Write {
                invocation,
                offset,
                bytes,
            } => {
                if bytes > u64::try_from(self.link.message_bytes()).unwrap_or(u64::MAX) {
                    return Err(DriverError::InvalidSpec(
                        "a native input chunk exceeds the current link allowance".into(),
                    ));
                }
                let mut body = vec![
                    0;
                    usize::try_from(bytes).map_err(|_| DriverError::InvalidSpec(
                        "the native input cannot fit this platform".into()
                    ))?
                ];
                tunnel.read_exact(&mut body).await.map_err(|_| {
                    DriverError::InvalidSpec("the native input was not completely received".into())
                })?;
                self.current(work, claims, from)?;
                self.write(work, invocation, offset, &body).await?
            }
            Request::CloseInput { invocation } => {
                self.process(work, invocation)?
                    .process
                    .close_input()
                    .await?;
                Reply::Ready
            }
            Request::Output { invocation, offset } => {
                return self.output(work, invocation, offset).await;
            }
            Request::Wait { invocation } => Reply::Exited {
                code: self.process(work, invocation)?.process.wait().await?,
            },
            Request::ReadFile { path, max_bytes } => {
                let unit = record.unit.as_ref().ok_or_else(|| {
                    DriverError::Unavailable("the isolated program unit is not prepared".into())
                })?;
                let driver = self.drivers.get(&record.driver).ok_or_else(|| {
                    DriverError::Unavailable("the admitted program driver is unavailable".into())
                })?;
                let bytes = driver
                    .contract()
                    .read_work_file(unit, &path, max_bytes)
                    .await?;
                self.current(work, claims, from)?;
                return Ok(Answer {
                    reply: Reply::File {
                        bytes: u64::try_from(bytes.len()).map_err(|_| {
                            DriverError::InvalidSpec("program file length overflow".into())
                        })?,
                    },
                    bytes,
                });
            }
            Request::Retire => {
                self.retire(work).await?;
                Reply::Ready
            }
        };
        Ok(Answer {
            reply,
            bytes: vec![],
        })
    }
}

#[async_trait]
impl TunnelServer for Programs {
    async fn serve(&self, incoming: IncomingTunnel) {
        let Ok(Service::Program { work_id: work }) = incoming.service.parse() else {
            incoming.refuse(
                OpenErrorCode::NoSuchService,
                "a program channel names its actual admitted work",
            );
            return;
        };
        let Ok(record) = self.current(work, &incoming.claims, incoming.from) else {
            incoming.refuse(
                OpenErrorCode::Refused,
                "the program channel does not match its admitted helper and peer",
            );
            return;
        };
        let claims = incoming.claims.clone();
        let from = incoming.from;
        let Ok(mut tunnel) = incoming.accept() else {
            return;
        };
        let remaining = record
            .admission
            .deadline
            .get()
            .signed_duration_since(self.clock.now())
            .to_std()
            .unwrap_or_default();
        let operation = async {
            let mut answer = match self.handle(work, &claims, from, &mut tunnel).await {
                Ok(answer) => answer,
                Err(error) => Answer {
                    reply: Reply::Failed {
                        code: error.code().into(),
                        detail: error.to_string(),
                        retryable: error.retryable(),
                    },
                    bytes: vec![],
                },
            };
            if !answer.bytes.is_empty() && self.current(work, &claims, from).is_err() {
                answer = Answer {
                    reply: Reply::Failed {
                        code: "unavailable".into(),
                        detail: "the program work ended before its output could be delivered"
                            .into(),
                        retryable: false,
                    },
                    bytes: vec![],
                };
            }
            if write_json(&mut tunnel, &answer.reply, self.link.message_bytes())
                .await
                .is_ok()
            {
                let _ = tunnel.write_all(&answer.bytes).await;
                let _ = tunnel.shutdown().await;
            }
        };
        tokio::select! {
            ()=self.stop.cancelled()=>{},
            ()=tokio::time::sleep(remaining)=>{},
            ()=operation=>{},
        }
    }
}
