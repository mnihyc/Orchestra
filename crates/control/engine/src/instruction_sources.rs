//! Instruction source refreshes from native reports, kept in order without adding a model input.

use crate::{Engine, error::EngineError, unit::Unit};
use orch_agent_contract::session::InstructionFile;
use orch_db::queries::{environments::thread_engine as environments, threads};
use orch_types::{Id, Json};
use std::path::{Component, Path};

fn same(one: &Json, other: &Json) -> bool {
    one.get("kind") == other.get("kind")
        && one.get("path") == other.get("path")
        && one.get("label") == other.get("label")
        && one.get("home") == other.get("home")
}

/// New reports replace the same source in place. A documented limit remains separate from an unknown loaded size.
pub(crate) fn merge(kept: &Json, incoming: &[Json]) -> Json {
    let mut sources = kept.as_array().cloned().unwrap_or_default();
    for source in incoming {
        if let Some(old) = sources.iter_mut().find(|old| same(old, source)) {
            let mut refreshed = source.clone();
            for key in ["line_limit", "byte_limit"] {
                if refreshed.get(key).is_none_or(Json::is_null)
                    && let Some(value) = old.get(key)
                {
                    refreshed[key] = value.clone();
                }
            }
            *old = refreshed;
        } else {
            sources.push(source.clone());
        }
    }
    Json::Array(sources)
}

fn within(path: &str, root: &str) -> bool {
    let path = Path::new(path);
    path.is_absolute()
        && path.starts_with(root)
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
}

/// Safe source projection before keeping it: a native report cannot turn an outside path into a file door.
pub(crate) fn file_json(
    file: &InstructionFile,
    environment: Id,
    root: &str,
) -> Result<Json, EngineError> {
    let mut file = file.clone();
    if file.home || file.path.as_deref().is_some_and(|path| !within(path, root)) {
        file = file.of_home();
    }
    file.parent_path = file
        .parent_path
        .filter(|path| !file.home && within(path, root));
    let mut value = serde_json::to_value(&file)?;
    value["scope"] = Json::Null;
    value["environment"] = if file.home {
        Json::Null
    } else {
        serde_json::json!(environment)
    };
    Ok(value)
}

impl Engine {
    /// A metadata-only change stamped on the ordinary thread feed. No turn, input or native call is made.
    pub(crate) async fn record_instruction_sources(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        segment: Id,
        files: &[InstructionFile],
    ) -> Result<(), EngineError> {
        let Some(row) = threads::segment(&mut *unit.conn, segment)
            .await?
            .filter(|row| row.thread_id == thread)
        else {
            return Ok(());
        };
        let Some(environment) =
            environments::environment(&mut *unit.conn, row.environment_id).await?
        else {
            return Ok(());
        };
        let mut incoming = Vec::new();
        for file in files {
            let mut source = file_json(file, environment.id, &environment.workspace_root)?;
            source["observed_at"] = serde_json::json!(unit.now());
            incoming.push(source);
        }
        let current: Json =
            sqlx::query_scalar("SELECT instruction_sources FROM segments WHERE id=$1")
                .bind(segment)
                .fetch_one(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
        let next = merge(&current, &incoming);
        if next != current {
            let seq = unit.stamp(thread).await?;
            threads::set_instruction_sources(&mut *unit.conn, segment, &next, seq).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_later_report_refreshes_one_source_and_keeps_an_import_after_its_parent() {
        let original = json!([{"kind":"preamble"},{"kind":"file","path":"/workspace/CLAUDE.md","label":"CLAUDE.md","home":false,"evidence":"documented_rule","byte_limit":32768}]);
        let reported = json!({"kind":"file","path":"/workspace/CLAUDE.md","label":"CLAUDE.md","home":false,"evidence":"reported","loaded_bytes":null});
        let import = json!({"kind":"file","path":"/workspace/notes.md","label":"notes.md","home":false,"evidence":"reported","parent_path":"/workspace/CLAUDE.md"});
        let refreshed = merge(&original, &[reported, import.clone()]);
        assert_eq!(refreshed.as_array().map(Vec::len), Some(3));
        assert_eq!(refreshed[1]["evidence"], "reported");
        assert_eq!(refreshed[1]["byte_limit"], 32768);
        assert!(refreshed[1]["loaded_bytes"].is_null());
        assert_eq!(
            merge(&refreshed, &[import]),
            refreshed,
            "a repeated native report introduces no duplicate source"
        );
    }
}
