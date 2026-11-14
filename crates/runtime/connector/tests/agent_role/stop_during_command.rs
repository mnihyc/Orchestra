use super::*;
use async_trait::async_trait;
use orch_agent_contract::adapter::{
    AdapterError, AgentAdapter, AgentStart, AgentStarted, EventSink, Interrupt,
};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_agent_contract::history::{ExportKind, SessionArchive};
use orch_agent_contract::session::{SessionOpened, SessionSpec, TurnStart};
use orch_connector_core::harnesses::AdapterRegistry;
use std::sync::Arc;
use tokio::sync::Notify;

struct SlowExport {
    inner: orch_adapter_fake::FakeAdapter,
    began: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl AgentAdapter for SlowExport {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }
    async fn start(
        &mut self,
        start: AgentStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        self.inner.start(start, sink).await
    }
    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.inner.stop().await
    }
    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        self.inner.probe().await
    }
    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        self.inner.open_session(spec).await
    }
    async fn close_session(&mut self, thread: Id) -> Result<(), AdapterError> {
        self.inner.close_session(thread).await
    }
    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
        self.inner.start_turn(turn).await
    }
    fn interrupt_handle(&self) -> Option<Interrupt> {
        self.inner.interrupt_handle()
    }
    async fn interrupt(&mut self, thread: Id, turn: Id) -> Result<(), AdapterError> {
        self.inner.interrupt(thread, turn).await
    }
    async fn export_session(
        &mut self,
        thread: Id,
        kind: ExportKind,
        upto: Option<u32>,
    ) -> Result<SessionArchive, AdapterError> {
        self.began.notify_one();
        self.release.notified().await;
        self.inner.export_session(thread, kind, upto).await
    }
}

#[tokio::test]
async fn a_stop_reaches_the_harness_while_export_holds_the_adapter_and_the_export_can_finish() {
    let mut rig = Rig::new();
    let began = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut registration = common::fake_registration();
    let (observed, allowed) = (Arc::clone(&began), Arc::clone(&release));
    registration.factory = Arc::new(move |clock| {
        Box::new(SlowExport {
            inner: orch_adapter_fake::FakeAdapter::new(clock),
            began: Arc::clone(&observed),
            release: Arc::clone(&allowed),
        })
    });
    rig.adapters = AdapterRegistry::new();
    rig.adapters.register(registration).unwrap();
    rig.start().await;
    rig.agent_start(json!({})).await;
    let (thread, segment) = (new_id(), new_id());
    rig.open_session(rig.open_body(thread, segment, "/tmp/work"))
        .await;
    let turn = rig
        .start_turn(thread, "#!fake\nsleep 60000\nsay finished")
        .await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    let export = orch_link_contract::commands::SessionExport {
        thread_id: thread,
        kind: orch_link_contract::commands::ExportKind::Native,
        upto_turn_ordinal: None,
    };
    let exporting = rig
        .controller
        .issue(&SerialKey::Thread(thread), rig.deadline(), &export);
    tokio::time::timeout(PATIENCE, began.notified())
        .await
        .expect("the adapter is exporting");
    rig.succeed(
        &SerialKey::None,
        &TurnInterrupt {
            thread_id: thread,
            turn_id: turn,
        },
    )
    .await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    assert!(!matches!(
        rig.controller.progress(exporting),
        Some(Progress::Succeeded(_))
    ));
    release.notify_one();
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(exporting))
            .await
            .unwrap(),
        Progress::Succeeded(_)
    ));
    rig.connector.unwrap().stop().await;
}
