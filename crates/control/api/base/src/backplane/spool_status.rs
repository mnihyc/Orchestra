//! Storage failures are current machine facts even when its durable output cannot be written.
use super::*;

impl Link {
    pub(super) async fn record_spool_status(&self, machine: Id, eph: &Eph) {
        if eph.stream != Stream::Machine.to_string() || eph.kind != "spool.status" {
            return;
        }
        let Ok(orch_link_contract::events::Report::SpoolStatus(status)) = eph.report() else {
            tracing::warn!(%machine,"a machine storage observation could not be read");
            return;
        };
        let changed = async {
            let mut conn = self.inner.db.acquire().await?;
            resources::set_machine_spool_status(&mut conn, machine, &status, self.inner.clock.now())
                .await
        }
        .await;
        match changed {
            Ok(true) => {
                if crate::realtime::ephemeral::broadcast(
                    &self.inner.db,
                    &crate::realtime::ephemeral::Message::ObjectChanged {
                        feed: orch_realtime_contract::feeds::Feed::Resource {
                            kind: orch_realtime_contract::feeds::ResourceKind::Machine,
                            id: machine,
                        },
                    },
                )
                .await
                .is_err()
                {
                    tracing::warn!(%machine,"a current machine storage observation could not be announced");
                }
            }
            Ok(false) => {}
            Err(_) => {
                tracing::warn!(%machine,"a current machine storage observation could not be recorded")
            }
        }
    }
}
