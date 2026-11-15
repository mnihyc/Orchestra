//! An admitted start keeps its own policy and absolute deadlines through retries and restart.

use super::*;
use orch_api_contract::settings::SettingScope;
use orch_api_contract::threads::{ThreadStartWait, ThreadStartWaitStage};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Allowance {
    seconds: u64,
    nanos: u32,
    source: Option<SettingScope>,
    source_node: Option<Id>,
}

impl Allowance {
    fn of(duration: Duration, source: Option<SettingScope>) -> Self {
        Self {
            seconds: duration.as_secs(),
            nanos: duration.subsec_nanos(),
            source,
            source_node: None,
        }
    }
    fn duration(&self) -> Duration {
        Duration::new(self.seconds, self.nanos)
    }
    fn millis(&self) -> Option<u64> {
        u64::try_from(self.duration().as_millis()).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Receipt {
    #[serde(flatten)]
    pub(super) fact: ThreadStartWait,
    command: Id,
    issued_at: Time,
    acceptance: Allowance,
    confirmation: Allowance,
    event_grace: Allowance,
}

impl Receipt {
    pub(super) fn of(turn: &TurnRow, command: Id) -> Result<Self, EngineError> {
        let receipt: Self = turn
            .start_wait
            .clone()
            .map(serde_json::from_value)
            .transpose()?
            .ok_or_else(|| EngineError::unreadable("the admitted start has no captured wait"))?;
        if receipt.command != command {
            return Err(EngineError::unreadable(
                "the captured wait belongs to another start",
            ));
        }
        Ok(receipt)
    }
    fn deadline(&self, stage: ThreadStartWaitStage) -> Option<chrono::DateTime<chrono::Utc>> {
        let wait = match stage {
            ThreadStartWaitStage::Acceptance => self.acceptance.duration(),
            ThreadStartWaitStage::Start => self
                .acceptance
                .duration()
                .checked_add(self.confirmation.duration())?,
            ThreadStartWaitStage::Events => self
                .acceptance
                .duration()
                .checked_add(self.confirmation.duration())?
                .checked_add(self.event_grace.duration())?,
        };
        self.issued_at
            .get()
            .checked_add_signed(chrono::Duration::from_std(wait).ok()?)
    }
    pub(super) fn due(&self, stage: ThreadStartWaitStage) -> chrono::DateTime<chrono::Utc> {
        self.deadline(stage)
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC)
    }
    pub(super) fn remaining(&self, now: chrono::DateTime<chrono::Utc>) -> Duration {
        (self.due(ThreadStartWaitStage::Acceptance) - now)
            .to_std()
            .unwrap_or_default()
    }
    fn change_stage(&mut self, stage: ThreadStartWaitStage, now: chrono::DateTime<chrono::Utc>) {
        if self.fact.stage == stage {
            return;
        }
        let allowance = match stage {
            ThreadStartWaitStage::Acceptance => &self.acceptance,
            ThreadStartWaitStage::Start => &self.confirmation,
            ThreadStartWaitStage::Events => &self.event_grace,
        };
        self.fact = ThreadStartWait {
            stage,
            began_at: Time::new(now),
            deadline: self.deadline(stage).map(Time::new),
            after_millis: allowance.millis(),
            admitted_default_source: allowance.source,
            closed_at: None,
        };
    }
}

impl Engine {
    pub(super) async fn capture_start_wait_in(
        &self,
        conn: &mut orch_db::Conn,
        thread: Id,
        command: Id,
        issued_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Receipt, EngineError> {
        let definitions = [
            keys::THREADS_START_ACCEPTANCE_SECONDS,
            keys::THREADS_START_CONFIRMATION_SECONDS,
            keys::THREADS_START_EVENT_GRACE_SECONDS,
        ]
        .map(|key| {
            registry::get(key)
                .ok_or_else(|| EngineError::unreadable("a start wait setting is undeclared"))
        })
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        let snapshot =
            Snapshot::load(conn, Place::Node(thread), Wanted::Keys(&definitions)).await?;
        let read =
            |at: usize, override_value: Option<Duration>| -> Result<Allowance, EngineError> {
                if let Some(duration) = override_value {
                    return Ok(Allowance::of(duration, None));
                }
                let setting = snapshot.view(definitions[at]);
                let seconds = setting.value().and_then(Json::as_u64).ok_or_else(|| {
                    EngineError::unreadable("a start wait is not nonnegative seconds")
                })?;
                let mut captured = Allowance::of(
                    Duration::from_secs(seconds),
                    Some(serde_json::from_value(serde_json::to_value(
                        setting.resolved.source.scope,
                    )?)?),
                );
                captured.source_node = setting.resolved.source.node;
                Ok(captured)
            };
        // Embedders and deterministic fixtures can override the clock; deployed defaults use the registry.
        let acceptance_override = (self.inner.config.start_timeout
            != crate::config::TURN_START_TIMEOUT)
            .then_some(self.inner.config.start_timeout);
        let confirmation_override = (self.inner.config.started_wait
            != crate::config::TURN_STARTED_WAIT)
            .then_some(self.inner.config.started_wait);
        let acceptance = read(0, acceptance_override)?;
        let confirmation = read(1, confirmation_override)?;
        let event_grace = read(2, confirmation_override.map(|wait| wait.saturating_mul(10)))?;
        let mut receipt = Receipt {
            fact: ThreadStartWait {
                stage: ThreadStartWaitStage::Acceptance,
                began_at: Time::new(issued_at),
                deadline: None,
                after_millis: acceptance.millis(),
                admitted_default_source: acceptance.source,
                closed_at: None,
            },
            command,
            issued_at: Time::new(issued_at),
            acceptance,
            confirmation,
            event_grace,
        };
        receipt.fact.deadline = receipt
            .deadline(ThreadStartWaitStage::Acceptance)
            .map(Time::new);
        Ok(receipt)
    }

    pub(super) async fn observe_start_wait(
        &self,
        snap: &StartSnapshot,
        command: Id,
        stage: ThreadStartWaitStage,
    ) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(unit.conn, snap.thread.node_id).await? else {
                return Ok(());
            };
            let Some(turn) = threads::turn(unit.conn, snap.turn.id).await? else {
                return Ok(());
            };
            if thread.current_turn_id != Some(turn.id)
                || turn.state != turn_state::STARTING
                || turn.interrupted_by.is_some()
            {
                return Ok(());
            }
            let mut receipt = Receipt::of(&turn, command)?;
            if receipt.fact.stage == stage {
                return Ok(());
            }
            receipt.change_stage(stage, unit.now());
            let seq = unit.stamp(thread.node_id).await?;
            sqlx::query("UPDATE turns SET start_wait=$2, seq_updated=$3 WHERE id=$1")
                .bind(turn.id)
                .bind(serde_json::to_value(receipt)?)
                .bind(seq)
                .execute(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }
}
