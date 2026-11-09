//! Native model catalogues are immutable per process; the provider home and history stay shared.
use super::*;
use crate::model_catalogue::{self, Catalogue, Identity};
use std::sync::Mutex;

pub(super) struct ProcessEvents {
    sink: EventSink,
    statuses: Mutex<BTreeMap<Option<Identity>, orch_agent_contract::facts::HarnessStatus>>,
}
impl ProcessEvents {
    pub fn sink(&self) -> &EventSink {
        &self.sink
    }
    pub fn new(sink: EventSink) -> Arc<Self> {
        Arc::new(Self {
            sink,
            statuses: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn sink_for(self: &Arc<Self>, identity: Option<Identity>) -> EventSink {
        let events = Arc::clone(self);
        self.sink.map_events(move |event| match event {
            AdapterEvent::Status(status) => {
                let mut statuses = events
                    .statuses
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                statuses.insert(identity, status.clone());
                let mut reported = statuses.get(&None).cloned().unwrap_or(status);
                reported.sessions_open = statuses.values().fold(0_u32, |total, status| {
                    total.saturating_add(status.sessions_open)
                });
                AdapterEvent::Status(reported)
            }
            other => other,
        })
    }
    pub fn forget(&self, identity: Option<Identity>) {
        if identity.is_some() {
            self.statuses
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&identity);
        }
    }
}

impl CodexAdapter {
    pub(super) fn live_session(
        &self,
        thread: ThreadId,
    ) -> Result<(Arc<Shared>, &Running), AdapterError> {
        let selected = self
            .model_processes
            .values()
            .chain(self.running.iter())
            .find(|running| running.shared.with_session(thread, |_| ()).is_ok());
        match selected {
            Some(running) if !running.shared.is_dead() => {
                Ok((Arc::clone(&running.shared), running))
            }
            Some(_) => Err(AdapterError::HarnessUnavailable(
                "the model's native process has ended".into(),
            )),
            None => self.live(),
        }
    }
    pub(super) fn live_native(
        &self,
        native: &str,
    ) -> Result<(Arc<Shared>, &Running), AdapterError> {
        let selected = self
            .model_processes
            .values()
            .chain(self.running.iter())
            .find(|running| running.shared.owns_native(native));
        match selected {
            Some(running) if !running.shared.is_dead() => {
                Ok((Arc::clone(&running.shared), running))
            }
            Some(_) => Err(AdapterError::HarnessUnavailable(
                "the model's native process has ended".into(),
            )),
            None => self.live(),
        }
    }
    pub(super) async fn prepare_model_process(
        &mut self,
        spec: &SessionSpec,
    ) -> Result<Option<Identity>, AdapterError> {
        let identity = match model_catalogue::document(spec) {
            Some(document) => {
                let ProviderAccess::Forwarded { provider_id, .. } = spec.provider else {
                    return Err(AdapterError::Internal(
                        "native catalogue has no forwarded provider".into(),
                    ));
                };
                let identity = Identity {
                    provider: provider_id,
                    fingerprint: model_catalogue::fingerprint(&document)?,
                };
                if self
                    .model_processes
                    .get(&identity)
                    .is_some_and(|running| running.shared.is_dead())
                    && let Some(running) = self.model_processes.remove(&identity)
                {
                    Self::stop_running(running).await
                }
                let idle: Vec<_> = self
                    .model_processes
                    .iter()
                    .filter(|(key, running)| **key != identity && running.shared.quiescent())
                    .map(|(key, _)| *key)
                    .collect();
                for key in idle {
                    if let Some(running) = self.model_processes.remove(&key) {
                        Self::stop_running(running).await
                    }
                }
                self.ensure_model_process(identity, &document).await?;
                Some(identity)
            }
            None => None,
        };
        if let Ok((old, running)) = self.live_session(spec.settings.thread_id)
            && old.with_session(spec.settings.thread_id, |_| ()).is_ok()
            && running
                .catalogue
                .as_ref()
                .map(|catalogue| catalogue.identity)
                != identity
        {
            let busy = old.with_session(spec.settings.thread_id, |record| {
                record.mapper.turn_in_progress() || record.mapper.background_running()
            })?;
            if busy {
                return Err(AdapterError::Other {
                    code: "conflict".into(),
                    message: "the running native work keeps its model metadata until it ends"
                        .into(),
                    retryable: true,
                });
            }
            let native = old.with_session(spec.settings.thread_id, |record| {
                record.mapper.native_thread().to_owned()
            })?;
            self.close_session(spec.settings.thread_id).await?;
            old.unload_before_resume(&native).await?;
        }
        Ok(identity)
    }
    async fn ensure_model_process(
        &mut self,
        identity: Identity,
        document: &Json,
    ) -> Result<(), AdapterError> {
        if self.model_processes.contains_key(&identity) {
            return Ok(());
        }
        let (_, control) = self.live()?;
        let start = control.start.clone();
        let events = Arc::clone(&control.events);
        let directory = control
            .shared
            .outputs()
            .directory()
            .parent()
            .unwrap_or(control.shared.outputs().directory())
            .join("model-catalogues");
        let catalogue = Catalogue::write(identity.provider, document, &directory)?;
        let running = self.launch_model(start, events, Some(catalogue)).await?;
        self.model_processes.insert(identity, running);
        Ok(())
    }
    pub(super) fn live_model(
        &self,
        identity: Option<Identity>,
    ) -> Result<(Arc<Shared>, &Running), AdapterError> {
        match identity {
            Some(identity) => self
                .model_processes
                .get(&identity)
                .filter(|running| !running.shared.is_dead())
                .map(|running| (Arc::clone(&running.shared), running))
                .ok_or_else(|| {
                    AdapterError::HarnessUnavailable(
                        "the admitted model process is unavailable".into(),
                    )
                }),
            None => self.live(),
        }
    }
}
