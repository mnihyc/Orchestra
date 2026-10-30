//! Explicitly requested Git work facts, shared by threads using the same files.

use crate::commands::{Outcome, Outgoing};
use crate::unit::Unit;
use crate::{Engine, EngineError};
use orch_api_contract::source::DirectoryWork;
use orch_api_contract::workspace::{GitStatus, GitUnavailable};
use orch_db::queries::{source, threads};
use orch_link_contract::commands::{GitStatus as GitStatusCommand, SerialKey};
use orch_types::{Id, Time};

/// Match the retention page bound: finish other independent turns between bounded batches.
const INVALIDATION_PAGE: i64 = 200;

/// Link suggestions are read from the last answer only and never replace accepted links.
#[must_use]
pub fn suggested_links(payload: &orch_types::Json) -> Vec<orch_api_contract::source::ThreadLink> {
    let text = payload
        .get("text")
        .and_then(orch_types::Json::as_str)
        .or_else(|| {
            payload
                .pointer("/data/text")
                .and_then(orch_types::Json::as_str)
        })
        .unwrap_or_default();
    let mut links = Vec::new();
    for word in text.split_whitespace() {
        for start in word.match_indices("http").map(|(offset, _)| offset) {
            let candidate = word[start..]
                .split(['<', '>', '"', '\'', '`'])
                .next()
                .unwrap_or_default()
                .trim_end_matches([')', ']', ',', '.', ';']);
            let Ok(url) = url::Url::parse(candidate) else {
                continue;
            };
            if !matches!(url.scheme(), "https" | "http")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                continue;
            }
            let parts: Vec<&str> = url.path().trim_matches('/').split('/').collect();
            let pull = parts.len() >= 4 && parts[parts.len() - 2] == "pull";
            let merge = parts.len() >= 5
                && parts[parts.len() - 3..parts.len() - 1] == ["-", "merge_requests"];
            if !(pull || merge)
                || !parts.last().is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                })
            {
                continue;
            }
            let link = orch_api_contract::source::ThreadLink {
                url: url.to_string(),
                label: if pull {
                    "Pull request"
                } else {
                    "Merge request"
                }
                .to_owned(),
            };
            if !links.contains(&link) {
                links.push(link);
            }
        }
    }
    links
}

/// Publication facts preserve independent unknowns and explain a branch without upstream.
#[must_use]
pub fn from_status(status: &GitStatus, at: Time) -> DirectoryWork {
    if !status.available {
        return DirectoryWork {
            uncommitted: None,
            unpushed: None,
            changed_files: None,
            ahead: None,
            as_of: at,
            stale: status.reason == Some(GitUnavailable::Offline),
            reason: Some(
                match status.reason.as_ref() {
                    Some(GitUnavailable::NotARepository) => "not_a_repository",
                    Some(GitUnavailable::GitMissing) => "git_missing",
                    Some(GitUnavailable::Offline) => "source_offline",
                    None => "git_unavailable",
                }
                .to_owned(),
            ),
        };
    }
    let (unpushed, reason) = if status.head.is_none() {
        (Some(false), None)
    } else if status.upstream.is_none() {
        (Some(true), Some("no_upstream".to_owned()))
    } else {
        (
            status.ahead.map(|ahead| ahead > 0),
            status
                .ahead
                .is_none()
                .then(|| "upstream_unknown".to_owned()),
        )
    };
    DirectoryWork {
        uncommitted: Some(!status.changes.is_empty()),
        unpushed,
        changed_files: i64::try_from(status.changes.len()).ok(),
        ahead: status.ahead,
        as_of: at,
        stale: false,
        reason,
    }
}

impl Engine {
    /// Remembers a requested status for every thread sharing the exact requested directory.
    /// Subdirectories are refreshed through their own protected read, so nested repositories stay distinct.
    pub async fn remember_git(
        &self,
        environment: Id,
        directory: &str,
        status: &GitStatus,
    ) -> Result<(), EngineError> {
        let request = orch_types::new_id();
        let at = Time::new(self.inner.clock.now());
        self.begin_git_read(environment, directory, request, at)
            .await?;
        self.finish_git_read(environment, directory, request, at, status)
            .await
    }

    /// A private request identity orders answers independently of a held or coarse clock.
    pub async fn begin_git_read(
        &self,
        environment: Id,
        directory: &str,
        request: Id,
        at: Time,
    ) -> Result<(), EngineError> {
        self.invalidate_work().await?;
        let mut tx = self.db().begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        for thread in source::work_threads(&mut *unit.conn, environment, directory).await? {
            let seq = unit.stamp(thread).await?;
            source::reserve_work(&mut *unit.conn, thread, request, at, seq).await?;
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Keeps the observation's request time and any change that occurred while its answer waited.
    pub async fn finish_git_read(
        &self,
        environment: Id,
        directory: &str,
        request: Id,
        at: Time,
        status: &GitStatus,
    ) -> Result<(), EngineError> {
        let work = from_status(status, at);
        let value = serde_json::to_value(work)?;
        let mut tx = self.inner.db.begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let candidates = source::work_threads(&mut *unit.conn, environment, directory).await?;
        for thread in candidates {
            let Some(row) = threads::lock(&mut *unit.conn, thread).await? else {
                continue;
            };
            if row.environment_id != Some(environment) || row.workspace_dir != directory {
                continue;
            }
            let seq = unit.stamp(thread).await?;
            source::finish_work(&mut *unit.conn, thread, request, &value, seq).await?;
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Replays durable turn invalidations after their transaction, with no cross-thread lock wait.
    pub(crate) async fn invalidate_work(&self) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        let pending: Vec<(Id, Id, i32, String, chrono::DateTime<chrono::Utc>, bool)> = sqlx::query_as(
            "SELECT t.id, g.environment_id, g.env_generation, COALESCE(s.directory, th.workspace_dir), t.completed_at,
                EXISTS(SELECT 1 FROM audit_log a WHERE a.node_id = t.thread_id AND a.action = 'workspace.changed_by_agent' AND a.details->>'turn_id' = t.id::text)
             FROM turns t JOIN segments g ON g.id = t.segment_id JOIN threads th ON th.node_id = t.thread_id
             LEFT JOIN snapshots s ON s.id = t.before_snapshot_id
             WHERE t.files_work_pending AND t.completed_at IS NOT NULL
             ORDER BY t.id LIMIT $1 FOR UPDATE OF t SKIP LOCKED")
            .bind(INVALIDATION_PAGE).fetch_all(&mut *tx).await.map_err(orch_db::DbError::from)?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        for (turn, environment, generation, directory, at, known_change) in pending {
            let candidates: Vec<Id> = sqlx::query_scalar(
                "SELECT th.node_id FROM threads th JOIN nodes n ON n.id = th.node_id
                 JOIN environments e ON e.id = th.environment_id
                 WHERE th.environment_id = $1 AND e.generation = $2 AND n.deleted_at IS NULL
                   AND (th.workspace_dir = $3 OR th.workspace_dir = '' OR $3 = ''
                       OR starts_with(th.workspace_dir, $3 || '/') OR starts_with($3, th.workspace_dir || '/'))
                 ORDER BY th.node_id")
                .bind(environment).bind(generation).bind(&directory).fetch_all(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
            let held: Vec<Id> = sqlx::query_scalar("SELECT node_id FROM threads WHERE node_id = ANY($1) ORDER BY node_id FOR UPDATE SKIP LOCKED")
                .bind(&candidates).fetch_all(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
            for thread in &held {
                let seq = unit.stamp(*thread).await?;
                source::stale_work_reason(
                    &mut *unit.conn,
                    *thread,
                    at,
                    seq,
                    if known_change {
                        "files_changed"
                    } else {
                        "coverage_unknown"
                    },
                )
                .await?;
            }
            if held.len() == candidates.len() {
                source::work_pending(&mut *unit.conn, turn, false).await?;
            }
        }
        unit.finish().await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// An explicit commit/push refreshes affected directories once, including every sharing thread.
    /// Each discovered root must still be the acted-on repository; a nested repository keeps its facts.
    pub async fn refresh_git_repository(
        &self,
        environment: Id,
        _repository: &str,
        job: Id,
    ) -> Result<(), EngineError> {
        let (machine, directories) = {
            let mut conn = self.inner.db.acquire().await?;
            let env =
                orch_db::queries::environments::thread_engine::environment(&mut conn, environment)
                    .await?
                    .ok_or_else(|| {
                        EngineError::unreadable(
                            "the environment disappeared before the Git refresh",
                        )
                    })?;
            let machine = env
                .machine_id
                .ok_or_else(|| EngineError::unreadable("the Git environment has no machine"))?;
            // A push changes common tracking refs across sibling worktrees. Each distinct
            // directory gets its own protected observation, including a nested repository.
            (
                machine,
                source::work_directories(&mut conn, environment, "").await?,
            )
        };
        for directory in directories {
            if self.is_stopped() {
                return Err(crate::commands::CommandsError::Stopping.into());
            }
            let command = Outgoing::new(
                crate::ids::derived(
                    "git.status.after-act",
                    &[job.as_bytes(), directory.as_bytes()],
                ),
                machine,
                SerialKey::Environment(environment),
                &GitStatusCommand {
                    environment_id: environment,
                    path: Some(directory.clone()),
                    watch_id: None,
                },
            )?;
            let request = command.id;
            let existing = self.inner.commands.status(request).await?;
            let at = Time::new(
                existing
                    .as_ref()
                    .map_or_else(|| self.inner.clock.now(), |info| info.issued_at),
            );
            if existing.is_none() {
                self.begin_git_read(environment, &directory, request, at)
                    .await?;
            }
            if let Outcome::Succeeded(value) =
                crate::commands::run(self.inner.commands.as_ref(), command).await?
            {
                let status: GitStatus = serde_json::from_value(value)?;
                self.finish_git_read(environment, &directory, request, at, &status)
                    .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;

    #[test]
    fn suggestions_keep_safe_request_urls_separate_from_accepted_links() {
        let links = suggested_links(
            &serde_json::json!({ "text": "Review [this](https://github.com/team/repo/pull/12). Also https://gitlab.com/team/sub/repo/-/merge_requests/7 and https://github.com/token@other/pull/1 and https://token@github.com/team/repo/pull/2 and https://github.com/team/repo/pull/12" }),
        );
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].url, "https://github.com/team/repo/pull/12");
        assert_eq!(
            links[1].url,
            "https://gitlab.com/team/sub/repo/-/merge_requests/7"
        );
        assert!(!links.iter().any(|link| link.url.contains("token")));
    }

    fn status() -> GitStatus {
        GitStatus {
            available: true,
            reason: None,
            repository_dir: Some(String::new()),
            status_hash: None,
            remotes: Vec::new(),
            upstream: None,
            branch: Some("work".to_owned()),
            head: Some("commit".to_owned()),
            ahead: None,
            behind: None,
            changes: Vec::new(),
        }
    }

    #[test]
    fn no_upstream_is_explained_and_unreadable_counts_stay_unknown() {
        let at: Time = "2026-10-14T14:00:00.000Z".parse().unwrap();
        let mut status = status();
        let work = from_status(&status, at);
        assert_eq!(work.uncommitted, Some(false));
        assert_eq!(work.unpushed, Some(true));
        assert_eq!(work.reason.as_deref(), Some("no_upstream"));
        status.upstream = Some("origin/work".to_owned());
        let work = from_status(&status, at);
        assert!(work.unpushed.is_none());
        assert_eq!(work.reason.as_deref(), Some("upstream_unknown"));
        status.ahead = Some(0);
        assert_eq!(from_status(&status, at).unpushed, Some(false));
        status.ahead = Some(2);
        assert_eq!(from_status(&status, at).unpushed, Some(true));
        status.available = false;
        status.reason = Some(GitUnavailable::NotARepository);
        let work = from_status(&status, at);
        assert!(
            work.uncommitted.is_none() && work.unpushed.is_none() && work.changed_files.is_none()
        );
        assert_eq!(work.reason.as_deref(), Some("not_a_repository"));
    }
}
