//! Resource change signals and private provider progress for transient subscribers.
//!
//! Only watched resources are compared, on the coordinator once per second. Row projections stay
//! inside the process and are hashed. Delivery replaces a signal with each recipient's native GET view.

use super::ephemeral::Coordinator;
use futures::FutureExt;
use std::sync::Arc;
use tokio::task::JoinHandle;

use super::Fail;

use super::Inner;

use orch_db::queries::jobs;

use orch_db::queries::resources;

use orch_realtime_contract::feeds::Feed;

use orch_realtime_contract::feeds::ResourceKind;

use orch_types::Hash;

use orch_types::Id;

use orch_types::Json;

use std::collections::HashMap;

use std::collections::HashSet;

use std::time::Duration;

use tokio::time::Instant;
/// Invalidation and job updates have a one-second ceiling, so one read per second suffices.
const LOOK_EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Snapshot {
    fingerprint: Option<Hash>,
    login: Option<Json>,
    job: Option<Json>,
}

struct Reading {
    task: JoinHandle<Result<Snapshot, Fail>>,
    received_login: bool,
}

impl Drop for Reading {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
/// The last resource projections and private progress observed by the coordinator.
pub struct Resources {
    reading: HashMap<Feed, Reading>,
    fingerprints: HashMap<Feed, Hash>,
    logins: HashMap<Id, Json>,
    jobs: HashMap<Id, Json>,
    last: Option<Instant>,
}

impl Resources {
    /// Remembers sign-in steps received directly from ingestion so the row sweep does not repeat them.
    pub(crate) fn observed(&mut self, feed: &Feed, data: &Json) {
        if let Feed::Resource {
            kind: ResourceKind::Provider,
            id,
        } = feed
            && data["what"] == "login"
        {
            self.logins.insert(*id, data["state"].clone());
            if let Some(reading) = self.reading.get_mut(feed) {
                reading.received_login = true;
            }
        }
    }
    /// Compares each watched resource's row and the extra state its page shows.
    pub(crate) async fn reconcile(
        &mut self,
        inner: &Arc<Inner>,
        active: &HashSet<Feed>,
        coordinator: &mut Coordinator,
    ) {
        self.reading.retain(|feed, _| active.contains(feed));
        let ready: Vec<_> = self
            .reading
            .iter()
            .filter(|(_, r)| r.task.is_finished())
            .map(|(feed, _)| feed.clone())
            .collect();
        for feed in ready {
            if let Some(mut read) = self.reading.remove(&feed) {
                match (&mut read.task)
                    .await
                    .map_err(Fail::other)
                    .and_then(|result| result)
                {
                    Ok(snapshot) => self.apply(&feed, snapshot, read.received_login, coordinator),
                    Err(error) => {
                        tracing::warn!(%feed, %error, "a watched resource could not be read")
                    }
                }
            }
        }
        if self.last.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return;
        }
        self.last = Some(Instant::now());
        self.fingerprints.retain(|feed, _| active.contains(feed));
        let provider_active = |id: &Id| {
            active.contains(&Feed::Resource {
                kind: ResourceKind::Provider,
                id: *id,
            })
        };
        self.logins.retain(|id, _| provider_active(id));
        self.jobs.retain(|id, _| provider_active(id));
        let wanted: Vec<_> = active
            .iter()
            .filter(|feed| {
                matches!(feed, Feed::Resource { .. } | Feed::ProviderRow(_))
                    && !self.reading.contains_key(*feed)
            })
            .cloned()
            .collect();
        if wanted.is_empty() {
            return;
        }
        let providers: Vec<_> = wanted
            .iter()
            .filter_map(|feed| match feed {
                Feed::Resource {
                    kind: ResourceKind::Provider,
                    id,
                } => Some(*id),
                _ => None,
            })
            .collect();
        // Readers share the epoch and providers share one harness query. A slow provider
        // table never holds the coordinator or a read of a machine's independent table.
        let db = inner.db.clone();
        let epoch = async move {
            let mut conn = db.acquire().await?;
            orch_db::queries::access::epoch(&mut conn).await
        }
        .map(|result| result.map_err(Arc::new))
        .shared();
        let db = inner.db.clone();
        let harnesses = async move {
            let mut conn = db.acquire().await?;
            let mut by_provider = HashMap::<Id, Vec<_>>::new();
            for report in resources::routes::harness_processes(&mut conn, &providers).await? {
                by_provider
                    .entry(report.provider_id)
                    .or_default()
                    .push(report);
            }
            Ok::<_, orch_db::DbError>(Arc::new(by_provider))
        }
        .map(|result| result.map_err(Arc::new))
        .shared();

        for feed in wanted {
            let epoch = epoch.clone();
            let harnesses = harnesses.clone();
            let db = inner.db.clone();
            let key = feed.clone();
            let task = inner.tasks.spawn(async move {
                let epoch = epoch.await.map_err(Fail::other)?;
                let reports = if matches!(
                    feed,
                    Feed::Resource {
                        kind: ResourceKind::Provider,
                        ..
                    }
                ) {
                    Some(harnesses.await.map_err(Fail::other)?)
                } else {
                    None
                };
                let mut conn = db.acquire().await?;
                read(&mut conn, &feed, epoch, reports.as_deref()).await
            });
            self.reading.insert(
                key,
                Reading {
                    task,
                    received_login: false,
                },
            );
        }
    }

    fn apply(
        &mut self,
        feed: &Feed,
        snapshot: Snapshot,
        received_login: bool,
        coordinator: &mut Coordinator,
    ) {
        if let Feed::Resource {
            kind: ResourceKind::Provider,
            id,
        } = feed
        {
            if let Some(state) = snapshot.login
                && !received_login
                && self
                    .logins
                    .insert(*id, state.clone())
                    .is_some_and(|last| last != state)
            {
                coordinator.queue(
                    feed.clone(),
                    serde_json::json!({"what":"login","state":state}),
                    true,
                );
            }
            if let Some(job) = snapshot.job
                && self.jobs.get(id) != Some(&job)
            {
                self.jobs.insert(*id, job.clone());
                coordinator.queue(
                    feed.clone(),
                    serde_json::json!({"what":"job","job":job}),
                    false,
                );
            }
        }
        if let Some(hash) = snapshot.fingerprint
            && self.fingerprints.insert(feed.clone(), hash) != Some(hash)
        {
            coordinator.queue(feed.clone(), serde_json::json!({"what":"changed"}), false);
        }
    }
}

async fn read(
    conn: &mut orch_db::Conn,
    feed: &Feed,
    epoch: i64,
    harnesses: Option<&HashMap<Id, Vec<resources::routes::HarnessProcessRow>>>,
) -> Result<Snapshot, Fail> {
    let mut snapshot = Snapshot::default();
    if let Feed::ProviderRow(id) = feed {
        // Only first-paint facts and aggregate membership are watched here. In particular,
        // neither accepted model declarations nor the upstream catalogue is materialized.
        let row: Option<Json> = sqlx::query_scalar(
                    "SELECT jsonb_build_object('name',p.name,'status',p.status,'revision',p.revision,
                       'facts',p.facts-ARRAY['models','spend','available_resets','extra_usage']::text[],
                       'facts_overrides',p.facts_overrides-ARRAY['models','spend','available_resets','extra_usage']::text[],
                       'listed',CASE WHEN jsonb_typeof(p.facts->'models')='array' THEN jsonb_array_length(p.facts->'models') ELSE NULL END,
                       'models',(SELECT COUNT(*) FROM provider_models m WHERE m.provider_id=p.id),
                       'enabled',(SELECT COUNT(*) FROM provider_models m WHERE m.provider_id=p.id AND m.enabled),
                       'running',(SELECT COUNT(*) FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.provider_id=p.id AND t.state='running' AND n.deleted_at IS NULL))
                     FROM providers p WHERE p.id=$1",
                ).bind(id).fetch_optional(&mut *conn).await.map_err(Fail::other)?;
        snapshot.fingerprint = row.map(|row| Hash::of(format!("{epoch}:{row}").as_bytes()));
        return Ok(snapshot);
    }
    let Feed::Resource { kind, id } = feed else {
        return Ok(snapshot);
    };
    // Table names come from the closed resource catalogue; identifiers are always bound.
    let table = match kind {
        ResourceKind::Provider => "providers",
        ResourceKind::Agent => "agents",
        ResourceKind::Host => "hosts",
        ResourceKind::Machine => "machines",
        ResourceKind::Template => "env_templates",
        ResourceKind::McpServer => "mcp_servers",
        ResourceKind::Mount => "mounts",
        ResourceKind::Skill => "skills",
    };
    let row: Option<Json> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT to_jsonb(r) FROM {table} r WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(Fail::other)?;
    let Some(row) = row else {
        return Ok(snapshot);
    };
    let mut contents = format!("{epoch}:{}", row);
    // Host progress lives on its physical machine, and update progress has its own
    // durable row. Include both in the existing permission-filtered invalidation feed.
    if matches!(kind, ResourceKind::Host | ResourceKind::Machine) {
        let machine = if *kind == ResourceKind::Host {
            row.get("machine_id")
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok())
        } else {
            Some(*id)
        };
        if let Some(machine) = machine {
            // A node can connect without changing its Host row or connector version.
            // Compare its physical presence too; this private fingerprint is never sent.
            if *kind == ResourceKind::Host {
                let presence: Option<Json> =
                    sqlx::query_scalar("SELECT to_jsonb(m) FROM machines m WHERE id=$1")
                        .bind(machine)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(Fail::other)?;
                contents.push_str(&format!("{presence:?}"));
            }
            let progress = orch_db::queries::component_updates::among(conn, &[machine]).await?;
            contents.push_str(&format!("{progress:?}"));
        }
    }
    if *kind == ResourceKind::Provider {
        let runtime = resources::provider_runtime::live_runtime(conn, *id).await?;
        let models = resources::models_of_provider(conn, *id).await?;
        // One stored batch covers all watched providers, including a CLI's retained exit reason.
        contents.push_str(&format!(
            "{runtime:?}{models:?}{:?}",
            harnesses.and_then(|reports| reports.get(id))
        ));
        if let Some(state) = resources::provider_runtime::state(conn, *id).await?
            && let Some(state) = state.login_state
        {
            // Reception bookkeeping is not another step of the public sign-in.
            let state: orch_api_contract::resources::LoginState =
                serde_json::from_value(state).map_err(Fail::other)?;
            let state = serde_json::to_value(state).map_err(Fail::other)?;
            snapshot.login = Some(state);
        }
        if let Some(job) = jobs::provider_jobs::latest_of_provider(conn, *id).await? {
            let job = crate::routes::environments::jobs::view(conn, &job)
                .await
                .map_err(Fail::other)?;
            snapshot.job = Some(serde_json::to_value(job).map_err(Fail::other)?);
        }
    }
    snapshot.fingerprint = Some(Hash::of(contents.as_bytes()));
    Ok(snapshot)
}
