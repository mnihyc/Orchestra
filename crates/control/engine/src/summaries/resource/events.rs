//! Native change events become paid work only after current standing/source/model/cap admission.
use super::*;
use chrono::{DateTime, Utc};
use orch_api_contract::summaries::{SummaryRefreshBody, SummaryTrigger};
use serde_json::json;

fn event_target(event: &rows::Event) -> Result<SummaryTarget, EngineError> {
    Ok(match event.kind.as_str() {
        "thread" => SummaryTarget::Thread {
            id: event.target_id,
        },
        "topic" => SummaryTarget::Topic {
            id: event.target_id,
        },
        "document" => SummaryTarget::Document {
            id: event.target_id,
        },
        "chat_title" => SummaryTarget::ChatTitle {
            id: event.target_id,
        },
        "chat" => SummaryTarget::Chat {
            id: event.target_id,
        },
        _ => return Err(refused()),
    })
}
/// Native thread/topic events use the same transaction as the original activity writer.
/// Parent topics receive a private change hint; only materialization can decide a contribution.
pub async fn node_event(
    conn: &mut Conn,
    node: Id,
    event: &str,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let Some(row) = orch_db::queries::hierarchy::node(conn, node).await? else {
        return Ok(());
    };
    let event = match event {
        "turn_completed" => "turn_completed",
        "milestone" => "milestone",
        "interval" => "interval",
        "child_summary" => "child_changed",
        _ => return Ok(()),
    };
    let target = match row.kind.as_str() {
        "thread" => SummaryTarget::Thread { id: node },
        "topic" => SummaryTarget::Topic { id: node },
        _ => return Ok(()),
    };
    rows::changed(conn, target, event, at).await?;
    if event != "interval" {
        let parents:Vec<Id>=sqlx::query_scalar("SELECT p.id FROM nodes n JOIN nodes p ON p.id=ANY(n.path) AND p.kind='topic' AND p.id<>n.id WHERE n.id=$1").bind(node).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        for parent in parents {
            rows::changed(
                conn,
                SummaryTarget::Topic { id: parent },
                "child_changed",
                at,
            )
            .await?;
        }
    }
    Ok(())
}
impl Engine {
    pub(crate) async fn queue_automatic_summaries(&self) -> Result<(), EngineError> {
        let mut conn = self.db().acquire().await?;
        let events = rows::events(&mut conn, super::super::SUMMARY_BATCH).await?;
        drop(conn);
        for event in events {
            if let Err(error) = self.queue_automatic_summary(&event).await {
                // A pending source/Use/cap check rotates without a request, ledger row or retry.
                // A real native source change can always coalesce a newer event meanwhile.
                let mut conn = self.db().acquire().await?;
                rows::checked(&mut conn, &event, false).await?;
                if matches!(error, EngineError::Db(_)) {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    async fn queue_automatic_summary(&self, event: &rows::Event) -> Result<(), EngineError> {
        let target = event_target(event)?;
        let trigger: SummaryTrigger = serde_json::from_value(json!(event.event))?;
        let mut tx = self.db().begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let identity = match sources::identify(&mut tx, target).await {
            Ok(identity) => identity,
            Err(
                sources::SourceError::Unavailable
                | sources::SourceError::Authz(orch_authz::Error::NotFound(_)),
            ) => {
                rows::checked(&mut tx, event, true).await?;
                tx.commit().await.map_err(DbError::from)?;
                return Ok(());
            }
            Err(error) => return Err(source_error(error)),
        };
        if orch_db::queries::hierarchy::with_ancestors(&mut tx, &[identity.place()])
            .await?
            .iter()
            .any(|node| node.archived_at.is_some() || node.deleted_at.is_some())
        {
            rows::checked(&mut tx, event, true).await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(());
        }
        let policy = options::resolve(&mut tx, &identity).await?;
        let consumed = trigger != SummaryTrigger::Interval;
        if !policy.options.automatic || policy.producer.is_none() {
            rows::checked(&mut tx, event, true).await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(());
        }
        if trigger != SummaryTrigger::Interval && policy.on.contains(&SummaryTrigger::Interval) {
            rows::changed(&mut tx, target, "interval", event.changed_at).await?;
        }
        if !policy.on.contains(&trigger) {
            rows::checked(&mut tx, event, true).await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(());
        }
        if let SummaryTarget::ChatTitle { id } = target
            && orch_db::queries::chats::chat(&mut tx, id)
                .await?
                .is_some_and(|chat| chat.name.is_some())
        {
            rows::checked(&mut tx, event, true).await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(());
        }
        let payer = policy
            .producer
            .as_ref()
            .and_then(|producer| producer.writer)
            .ok_or_else(refused)?;
        orch_db::queries::transaction_guard::lock_native_actor(&mut tx, payer)
            .await?
            .ok_or_else(refused)?;
        // This is the automatic setting writer's ordinary standing. Person work always
        // restores its captured operation in the separate runner and never takes this path.
        let caller = Caller::session(payer).with_admin_mode(false);
        let range = if let SummaryTarget::Chat { id } | SummaryTarget::ChatTitle { id } = target {
            let row = orch_db::queries::chats::chat(&mut tx, id)
                .await?
                .ok_or_else(refused)?;
            Some(SummaryChatRange {
                after: 0,
                through: row.last_message_seq,
            })
        } else {
            None
        };
        let initial = select(
            &mut tx,
            &caller,
            target,
            range,
            None,
            self.inner.clock.time(),
        )
        .await?;
        lock_selection(&mut tx, &initial).await?;
        let selected = select_with_snapshot(
            &mut tx,
            &caller,
            target,
            range,
            None,
            self.inner.clock.time(),
            Some(&initial.material.snapshot),
        )
        .await?;
        let plan = operation_scope::sources::bind_required(
            SummaryRefresh::ROUTE,
            &summary_sources::RESOURCE_REFRESH,
            &selected.facts(),
        )
        .map_err(|_| refused())?;
        let principal = Principal::load(
            &mut tx,
            plan.admit(&caller, false).map_err(|_| refused())?,
            self.inner.clock.time(),
        )
        .await?;
        require_model(
            &mut tx,
            &principal,
            selected.material.identity.place(),
            &selected.config,
        )
        .await?;
        if selected.material.activity.is_empty() {
            rows::checked(&mut tx, event, consumed).await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(());
        }
        let cache = rows::get(&mut tx, target, None).await?;
        let latest = match &cache {
            Some(cache) => rows::latest(&mut tx, cache.id).await?,
            None => None,
        };
        let changed = latest.as_ref().is_none_or(|request| {
            !request
                .accepted_source
                .get("snapshot")
                .is_some_and(|source| sources::same_content(source, &selected.material.snapshot))
                || request.accepted_source.get("policy")
                    != Some(&json!(selected.policy.fingerprint))
        });
        let now = self.inner.clock.now();
        let due = selected.policy.earliest(
            trigger,
            changed,
            latest.as_ref().map(|row| row.created_at),
            now,
        );
        let Some(due) = due else {
            rows::checked(&mut tx, event, consumed).await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(());
        };
        if !self
            .room_for_own_use(
                &mut tx,
                &crate::caps::OwnCapAsk {
                    place: selected.material.identity.place(),
                    provider: selected.config.provider_id,
                    model: &selected.config.model_name,
                    tier: None,
                    purpose: "summary",
                    person: None,
                },
            )
            .await?
        {
            rows::checked(&mut tx, event, false).await?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(());
        }
        let cache = rows::ensure(&mut tx, target, None, now)
            .await?
            .ok_or_else(refused)?;
        let body = SummaryRefreshBody {
            client_id: orch_types::new_id(),
            model: None,
            range,
        };
        let source = json!({"target":target,"range":selected.material.range,"snapshot":selected.material.snapshot,"policy":selected.policy.fingerprint});
        let accepted = rows::enqueue(
            &mut tx,
            cache.id,
            rows::Origin::Automatic { writer: payer },
            &serde_json::to_value(body)?,
            &source,
            now,
            due,
        )
        .await?;
        let consumed =
            consumed && matches!(accepted, rows::Enqueued::New(_) | rows::Enqueued::Replay(_));
        rows::checked(&mut tx, event, consumed).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
}
