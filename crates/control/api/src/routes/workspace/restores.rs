//! Current-authority restore plans bind immutable manifests, whole namespaces and visible writers.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::forks::{
    ActiveTurn, AppliedChangeOptions, ChangeOptions, PREFLIGHT_VALID_MINUTES,
};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::source::{
    EnvironmentFilesRestore, EnvironmentFilesRestoreBody, EnvironmentRestorePreflight,
    EnvironmentRestorePreflightBody, RestorePlan, RestoreWriter,
};
use orch_api_contract::workspace::Job;
use orch_authz::{Act, Policy};
use orch_db::queries::{jobs, source, storage, threads};
use orch_engine::jobs::source::{Action, Plan};
use orch_link_contract::commands::SerialKey;
use orch_link_contract::source::{FsRestore, FsRestorePlan};
use orch_settings::{Place, get};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

mod confirm;
mod preflight;

/// Binds the whole restore preflight and confirmation on the owning environment.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_initial_operation(
            EnvironmentRestorePreflight,
            crate::key_operations::native::effects::initial::<preflight::ReviewPlan>,
            prepare::<preflight::ReviewPlan>,
            preflight::run,
        )
        .on_with_operation(
            EnvironmentFilesRestore,
            prepare::<confirm::Start>,
            confirm::run,
        );
}

/// Entry discovery and later native guards use the same connection and transactional source loader.
fn prepare<'a, F: crate::key_operations::native::effects::Effect>(
    ctx: &'a Ctx,
    conn: &'a mut orch_db::Conn,
    caller: &'a orch_authz::Caller,
    query: F::Query,
    body: F::Body,
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        if orch_db::queries::transaction_guard::in_transaction(conn) {
            return crate::key_operations::native::effects::prepare::<F>(
                ctx, conn, caller, query, body,
            )
            .await;
        }
        use sqlx::Connection;
        let mut tx = conn.begin().await.map_err(orch_db::DbError::from)?;
        let prepared =
            crate::key_operations::native::effects::prepare::<F>(ctx, &mut tx, caller, query, body)
                .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(prepared)
    })
}

fn unavailable(reason: &str) -> ApiError {
    ApiError::new(
        ErrorCode::WorkspaceRestoreUnavailable,
        "complete recorded files are unavailable; review and undo individual files instead",
    )
    .with("reason", reason)
}

fn decode<T: serde::de::DeserializeOwned>(value: &Json, name: &str) -> ApiResult<T> {
    serde_json::from_value(
        value
            .get(name)
            .cloned()
            .ok_or_else(|| ApiError::internal("the stored restore plan lacks a required member"))?,
    )
    .map_err(ApiError::internal)
}

async fn options(
    conn: &mut orch_db::Conn,
    environment: Id,
    given: Option<&ChangeOptions>,
) -> ApiResult<AppliedChangeOptions> {
    let place = Place::Environment(environment);
    let active_turn = match given.and_then(|options| options.active_turn) {
        Some(value) => value,
        None => {
            if get::<String>(conn, place, keys::SWITCH_ACTIVE_TURN)
                .await
                .map_err(ApiError::internal)?
                .as_deref()
                == Some("interrupt")
            {
                ActiveTurn::Interrupt
            } else {
                ActiveTurn::Wait
            }
        }
    };
    let wait_minutes = match given.and_then(|options| options.wait_minutes) {
        Some(value) => value,
        None => get::<i64>(conn, place, keys::SWITCH_WAIT_MINUTES)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("the wait default is missing"))?,
    };
    let verify_timeout_secs = match given.and_then(|options| options.verify_timeout_secs) {
        Some(value) => value,
        None => get::<i64>(conn, place, keys::SWITCH_VERIFY_TIMEOUT_SECS)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("the verification default is missing"))?,
    };
    for (key, value) in [
        (keys::SWITCH_WAIT_MINUTES, wait_minutes),
        (keys::SWITCH_VERIFY_TIMEOUT_SECS, verify_timeout_secs),
    ] {
        registry::validate(key, &json!(value))
            .map_err(|error| ApiError::invalid("options", &error.to_string()))?;
    }
    Ok(AppliedChangeOptions {
        active_turn,
        wait_minutes,
        verify_timeout_secs,
        keep_previous: given
            .and_then(|options| options.keep_previous)
            .unwrap_or(true),
        stop_processes: given
            .and_then(|options| options.stop_processes)
            .unwrap_or(false),
    })
}

#[derive(Clone, Serialize, Deserialize)]
struct Frozen {
    shown: RestorePlan,
    recipe: Plan,
    plan_blob: Hash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    review: Option<orch_engine::jobs::restore::operation::Review>,
}

async fn require(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    environment: Id,
    permission: Permission,
) -> ApiResult<()> {
    authorization::require_resource(
        conn,
        principal,
        ResourceKind::Environment,
        environment,
        permission,
        Policy::default(),
    )
    .await
}

/// Included semantic hashes are compared with the complete end map, preserving unknown coverage.
fn later(paths: &mut [orch_api_contract::source::RestorePath], current: &Json, end: Option<&Json>) {
    let Some(end) = end.and_then(Json::as_object) else {
        return;
    };
    let mut now = std::collections::BTreeMap::new();
    for entry in current.as_array().into_iter().flatten() {
        let Some(path) = entry.get("p").and_then(Json::as_str) else {
            continue;
        };
        let hash = if entry.get("t").and_then(Json::as_str) == Some("l") {
            entry
                .get("l")
                .and_then(Json::as_str)
                .map(|link| json!(Hash::of(link.as_bytes())))
                .unwrap_or(Json::Null)
        } else {
            entry.get("h").cloned().unwrap_or(Json::Null)
        };
        now.insert(
            path,
            json!({ "h": hash, "t": entry["t"], "x": entry["x"].as_u64().unwrap_or(0) & 0o111 }),
        );
    }
    for path in paths {
        path.later_change = Some(now.get(path.path.as_str()) != end.get(&path.path));
    }
}

fn attribute(
    paths: &mut [orch_api_contract::source::RestorePath],
    directory: &str,
    writer_directory: &str,
    before: &Json,
    end: &Json,
    writer: &NodeRef,
) {
    let (Some(before), Some(end)) = (before.as_object(), end.as_object()) else {
        return;
    };
    for path in paths {
        let absolute = if directory.is_empty() {
            path.path.clone()
        } else {
            format!("{directory}/{}", path.path)
        };
        let relative = if writer_directory.is_empty() {
            Some(absolute.as_str())
        } else {
            absolute
                .strip_prefix(writer_directory)
                .and_then(|rest| rest.strip_prefix('/'))
        };
        let Some(relative) = relative else { continue };
        if before.get(relative) != end.get(relative)
            && !path.later_writers.iter().any(|known| known.id == writer.id)
        {
            path.later_writers.push(writer.clone());
        }
    }
}

/// Names only a visible later turn whose complete before/end maps prove an included write.
/// Separate threads at the same clock instant have unknown order; one thread's ordinals do not.
async fn known_writers(
    ctx: &Ctx,
    review: &orch_engine::jobs::restore::operation::Review,
    environment: Id,
    directory: &str,
    target: Option<&threads::TurnRow>,
    writers: &[RestoreWriter],
    paths: &mut [orch_api_contract::source::RestorePath],
) -> ApiResult<()> {
    let Some(target) = target.filter(|turn| turn.completed_at.is_some()) else {
        return Ok(());
    };
    for writer in writers.iter().filter_map(|writer| writer.thread.as_ref()) {
        let mut before_ordinal: Option<i32> = None;
        loop {
            let turns: Vec<(i32, Id, Json)> = {
                let mut conn = ctx.app.db.acquire().await?;
                sqlx::query_as(
                "SELECT t.ordinal, t.before_snapshot_id, t.files_end_manifest FROM turns t
                 JOIN snapshots s ON s.id = t.before_snapshot_id JOIN segments g ON g.id = t.segment_id
                 WHERE t.thread_id = $1 AND t.completed_at IS NOT NULL AND t.files_end_manifest IS NOT NULL
                   AND s.environment_id = $2 AND g.environment_id = $2 AND s.env_generation = g.env_generation
                   AND s.storage = 'environment' AND s.state = 'complete' AND s.manifest_blob IS NOT NULL
                   AND (t.started_at > $3 OR t.thread_id = $4 AND t.ordinal > $5)
                   AND ($6::int IS NULL OR t.ordinal < $6) ORDER BY t.ordinal DESC LIMIT $7")
                .bind(writer.id).bind(environment).bind(target.completed_at).bind(target.thread_id).bind(target.ordinal)
                .bind(before_ordinal).bind(i64::from(orch_api_contract::common::MAX_LIMIT))
                .fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
            };
            if turns.is_empty() {
                break;
            }
            before_ordinal = turns.last().map(|(ordinal, _, _)| *ordinal);
            for (_, snapshot, end) in turns {
                let recorded = {
                    let mut conn = ctx.app.db.acquire().await?;
                    storage::snapshot(&mut conn, snapshot).await?
                };
                let Some(recorded) = recorded else {
                    continue;
                };
                if recorded
                    .expires_at
                    .is_none_or(|until| until <= ctx.app.clock.now())
                    || !preflight::observed_writer(ctx, review, writer.id).await?
                {
                    continue;
                }
                let (Some(hash), Some(writer_directory)) =
                    (recorded.manifest_blob, recorded.directory)
                else {
                    continue;
                };
                // Missing/expired old coverage cannot establish a writer. It leaves the original
                // later_change value alone; neither an unknown map nor a hidden name becomes proof.
                let Ok(bytes) = ctx
                    .app
                    .blobs
                    .read_whole(
                        &hash,
                        i64::try_from(orch_engine::manifest::MANIFEST_MAX_EXPANDED_BYTES)
                            .map_err(ApiError::internal)?,
                    )
                    .await
                else {
                    continue;
                };
                if Hash::of(&bytes) != hash {
                    continue;
                }
                let Ok(manifest) = orch_engine::manifest::Manifest::parse(&bytes) else {
                    continue;
                };
                if manifest.header().generation != i64::from(recorded.env_generation) {
                    continue;
                }
                attribute(
                    paths,
                    directory,
                    &writer_directory,
                    &orch_engine::source::end_namespace(&manifest),
                    &end,
                    writer,
                );
            }
        }
    }
    Ok(())
}

async fn writer_ids(
    conn: &mut orch_db::Conn,
    environment: Id,
    directory: &str,
) -> ApiResult<Vec<Id>> {
    Ok(sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
        WHERE t.environment_id = $1 AND n.deleted_at IS NULL AND t.side_of IS NULL
          AND (t.current_turn_id IS NOT NULL OR $2 = '' OR t.workspace_dir = '' OR t.workspace_dir = $2 OR starts_with(t.workspace_dir, $2 || '/') OR starts_with($2, t.workspace_dir || '/')) ORDER BY t.node_id")
        .bind(environment).bind(directory).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn later_writer_names_follow_proved_paths_across_overlapping_directories() {
        use orch_api_contract::source::{
            ExpectedAbsent, RestoreAction, RestoreExpected, RestorePath,
        };
        let writer = NodeRef {
            thread_display: None,
            id: new_id(),
            kind: NodeKind::Thread,
            name: Some("Own attempt".into()),
            redacted: false,
        };
        let make = |path: &str| RestorePath {
            path: path.into(),
            action: RestoreAction::Change,
            expected: RestoreExpected::Absent(ExpectedAbsent::None),
            later_change: None,
            later_writers: Vec::new(),
        };
        let mut paths = vec![
            make("nested/code"),
            make("removed"),
            make("unchanged"),
            make("nested-sibling/code"),
        ];
        let before = json!({ "code": { "h": Hash::of(b"old"), "t": "f", "x": 0 } });
        let after = json!({ "code": { "h": Hash::of(b"old"), "t": "f", "x": 0o111 } });
        attribute(&mut paths, "web", "web/nested", &before, &after, &writer);
        attribute(&mut paths, "web", "web/nested", &before, &after, &writer);
        assert_eq!(
            paths[0].later_writers,
            vec![writer.clone()],
            "a mode-only write is known once"
        );
        assert!(
            paths
                .iter()
                .skip(1)
                .all(|path| path.later_writers.is_empty()),
            "a prefix sibling is outside the writer's directory"
        );
        attribute(
            &mut paths,
            "web",
            "web",
            &json!({ "removed": { "h": Hash::of(b"gone"), "t": "f", "x": 0 } }),
            &json!({}),
            &writer,
        );
        assert_eq!(paths[1].later_writers, vec![writer]);
        assert!(paths[2].later_writers.is_empty());
        assert!(
            paths.iter().all(|path| path.later_change.is_none()),
            "a writer never fills an unknown end comparison"
        );
    }
    #[test]
    fn later_changes_compare_added_removed_link_type_and_mode_without_guessing_unknowns() {
        use orch_api_contract::source::{ExpectedAbsent, RestoreAction, RestoreExpected};
        let mut paths = vec![orch_api_contract::source::RestorePath {
            path: "code".to_owned(),
            action: RestoreAction::Change,
            expected: RestoreExpected::Absent(ExpectedAbsent::None),
            later_change: None,
            later_writers: Vec::new(),
        }];
        let bytes = Hash::of(b"content");
        let current = json!([{ "p": "code", "t": "f", "x": 0o755, "h": bytes, "l": null }]);
        later(&mut paths, &current, None);
        assert_eq!(paths[0].later_change, None);
        later(
            &mut paths,
            &current,
            Some(&json!({ "code": { "h": bytes, "t": "f", "x": 0o111 } })),
        );
        assert_eq!(paths[0].later_change, Some(false));
        later(
            &mut paths,
            &current,
            Some(&json!({ "code": { "h": bytes, "t": "f", "x": 0 } })),
        );
        assert_eq!(paths[0].later_change, Some(true));
        later(&mut paths, &current, Some(&json!({})));
        assert_eq!(paths[0].later_change, Some(true));
    }
}
