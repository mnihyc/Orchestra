//! Resource change signals and private provider progress for transient subscribers.
//!
//! Only watched resources are compared, on the coordinator once per second. Row projections stay
//! inside the process and are hashed. Delivery replaces a signal with each recipient's native GET view.

use super::ephemeral::Coordinator;
use super::{Fail, Inner};
use orch_db::queries::{jobs, resources};
use orch_realtime_contract::feeds::{Feed, ResourceKind};
use orch_types::{Hash, Id, Json};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::time::Instant;

/// Invalidation and job updates have a one-second ceiling, so one read per second suffices.
const LOOK_EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
/// The last resource projections and private progress observed by the coordinator.
pub(crate) struct Resources {
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
        }
    }
    /// Compares each watched resource's row and the extra state its page shows.
    pub(crate) async fn reconcile(
        &mut self,
        inner: &Inner,
        active: &HashSet<Feed>,
        coordinator: &mut Coordinator,
    ) -> Result<(), Fail> {
        if self.last.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return Ok(());
        }
        self.last = Some(Instant::now());
        self.fingerprints.retain(|feed, _| active.contains(feed));
        let providers: HashSet<_> = active
            .iter()
            .filter_map(|feed| match feed {
                Feed::Resource {
                    kind: ResourceKind::Provider,
                    id,
                } => Some(*id),
                _ => None,
            })
            .collect();
        self.logins.retain(|id, _| providers.contains(id));
        self.jobs.retain(|id, _| providers.contains(id));
        let mut conn = inner.db.acquire().await?;
        let epoch = orch_db::queries::access::epoch(&mut conn).await?;
        let mut harnesses: HashMap<Id, Vec<resources::routes::HarnessProcessRow>> = HashMap::new();
        if !providers.is_empty() {
            let ids: Vec<Id> = providers.iter().copied().collect();
            for report in resources::routes::harness_processes(&mut conn, &ids).await? {
                harnesses
                    .entry(report.provider_id)
                    .or_default()
                    .push(report);
            }
        }
        for feed in active {
            let Feed::Resource { kind, id } = feed else {
                continue;
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
                continue;
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
                    let progress =
                        orch_db::queries::component_updates::among(&mut conn, &[machine]).await?;
                    contents.push_str(&format!("{progress:?}"));
                }
            }
            if *kind == ResourceKind::Provider {
                let runtime = resources::provider_runtime::live_runtime(&mut conn, *id).await?;
                let models = resources::models_of_provider(&mut conn, *id).await?;
                // One stored batch covers all watched providers, including a CLI's retained exit reason.
                contents.push_str(&format!("{runtime:?}{models:?}{:?}", harnesses.get(id)));
                if let Some(state) = resources::provider_runtime::state(&mut conn, *id).await?
                    && let Some(state) = state.login_state
                {
                    // Reception bookkeeping is not another step of the public sign-in.
                    let state: orch_api_contract::resources::LoginState =
                        serde_json::from_value(state).map_err(Fail::other)?;
                    let state = serde_json::to_value(state).map_err(Fail::other)?;
                    // The private stream checks provider.manage again for every recipient.
                    if self
                        .logins
                        .insert(*id, state.clone())
                        .is_some_and(|before| before != state)
                    {
                        coordinator.queue(
                            feed.clone(),
                            serde_json::json!({"what": "login", "state": state}),
                            true,
                        );
                    }
                }
                if let Some(job) = jobs::provider_jobs::latest_of_provider(&mut conn, *id).await? {
                    let job = crate::routes::environments::jobs::view(&mut conn, &job)
                        .await
                        .map_err(Fail::other)?;
                    let job = serde_json::to_value(job).map_err(Fail::other)?;
                    if self.jobs.get(id) != Some(&job) {
                        self.jobs.insert(*id, job.clone());
                        coordinator.queue(
                            feed.clone(),
                            serde_json::json!({"what": "job", "job": job}),
                            false,
                        );
                    }
                }
            }
            let fingerprint = Hash::of(contents.as_bytes());
            if self
                .fingerprints
                .insert(feed.clone(), fingerprint)
                .is_none_or(|before| before != fingerprint)
            {
                coordinator.queue(feed.clone(), serde_json::json!({"what": "changed"}), false);
            }
        }
        Ok(())
    }
}
