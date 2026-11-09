//! A restore's actual file-tool coverage, folded over selected native turn records.
use crate::EngineError;
use orch_api_contract::{
    source::{FileChangeImage, TouchedRestore, TouchedRestorePath},
    threads::{FileChange, FileChangeKind},
};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The public diff shape and private complete harness material in one recorded file change.
#[derive(Debug, Clone, Deserialize)]
pub struct RecordedChange {
    #[serde(flatten)]
    pub change: FileChange,
    #[serde(default)]
    pub before: Option<FileChangeImage>,
    #[serde(default)]
    pub after: Option<FileChangeImage>,
}
fn valid(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && part != ".git")
}
fn touch(
    paths: &mut BTreeMap<String, TouchedRestorePath>,
    path: &str,
    before: Option<FileChangeImage>,
    after: Option<FileChangeImage>,
) -> Result<(), EngineError> {
    if !valid(path) {
        return Err(EngineError::unreadable(
            "a recorded file change lies outside its directory",
        ));
    }
    paths
        .entry(path.into())
        .and_modify(|recorded| recorded.after = after.clone())
        .or_insert_with(|| TouchedRestorePath {
            path: path.into(),
            before,
            after,
        });
    Ok(())
}
/// Each path uses its first actual pre-image and final actual write, including creation/deletion chains.
pub fn fold(
    thread: Id,
    turn: Id,
    reason: String,
    changes: impl IntoIterator<Item = RecordedChange>,
) -> Result<TouchedRestore, EngineError> {
    let mut paths = BTreeMap::new();
    for recorded in changes {
        let change = recorded.change;
        if change.change == FileChangeKind::Rename {
            let old = change
                .old_path
                .as_deref()
                .ok_or_else(|| EngineError::unreadable("a recorded rename has no original path"))?;
            touch(
                &mut paths,
                old,
                recorded.before,
                Some(FileChangeImage::Absent),
            )?;
            touch(&mut paths, &change.path, None, recorded.after)?;
        } else {
            touch(&mut paths, &change.path, recorded.before, recorded.after)?;
        }
    }
    Ok(TouchedRestore {
        thread_id: thread,
        turn_id: turn,
        paths: paths.into_values().collect(),
        whole_unavailable: reason,
        overwrite: Vec::new(),
    })
}
fn relative_recorded(path: &str, root: &str, directory: &str) -> Result<String, EngineError> {
    // Absolute harness paths are resolved only against the actual recorded workspace and cwd.
    let windows = root.as_bytes().get(1) == Some(&b':') || root.starts_with("\\\\");
    let normalize = |value: &str| {
        if windows {
            value.replace('\\', "/")
        } else {
            value.to_owned()
        }
    };
    let path = normalize(path);
    let root = normalize(root);
    let directory = normalize(directory);
    let base = if directory.is_empty() || directory == "." {
        root.trim_end_matches('/').to_owned()
    } else {
        format!(
            "{}/{}",
            root.trim_end_matches('/'),
            directory.trim_matches('/')
        )
    };
    let absolute = path.starts_with('/') || path.as_bytes().get(1) == Some(&b':');
    let relative = if absolute {
        path.strip_prefix(&format!("{base}/"))
            .ok_or_else(|| EngineError::unreadable("a file change belongs to another directory"))?
    } else {
        path.as_str()
    };
    let relative = relative.strip_prefix("./").unwrap_or(relative);
    if !valid(relative) {
        return Err(EngineError::unreadable(
            "a file change lies outside the recorded directory",
        ));
    }
    Ok(relative.to_owned())
}
/// Select only actual file-change items of this thread and its recorded directory, in native turn/item order.
/// Whole-directory unavailability does not change the visibility of the environment's file changes.
pub async fn load(
    conn: &mut Conn,
    environment: Id,
    thread: Id,
    turn: Id,
    directory: &str,
    reason: String,
) -> Result<TouchedRestore, EngineError> {
    let source: Option<(Id, i32)> =
        sqlx::query_as("SELECT thread_id,ordinal FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let (owner, ordinal) =
        source.ok_or_else(|| EngineError::unreadable("the source turn is unavailable"))?;
    if owner != thread {
        return Err(EngineError::unreadable(
            "the source turn belongs to another thread",
        ));
    }
    let rows:Vec<Json>=sqlx::query_scalar("SELECT i.payload#>'{tool,changes}' FROM items i JOIN turns t ON t.id=i.turn_id JOIN segments g ON g.id=t.segment_id LEFT JOIN snapshots s ON s.id=t.before_snapshot_id WHERE t.thread_id=$1 AND t.ordinal>=$2 AND g.environment_id=$3 AND s.directory=$4 AND i.kind='tool_call' AND i.status='completed' AND i.payload#>>'{tool,kind}'='file_change' ORDER BY t.ordinal,i.seq_created,i.id")
        .bind(thread).bind(ordinal).bind(environment).bind(directory).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let root: String = sqlx::query_scalar("SELECT workspace_root FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let mut changes = Vec::new();
    for row in rows {
        for mut recorded in serde_json::from_value::<Vec<RecordedChange>>(row)? {
            recorded.change.path = relative_recorded(&recorded.change.path, &root, directory)?;
            recorded.change.old_path = recorded
                .change
                .old_path
                .as_deref()
                .map(|path| relative_recorded(path, &root, directory))
                .transpose()?;
            changes.push(recorded);
        }
    }
    fold(thread, turn, reason, changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_harness_paths_keep_the_recorded_directory_boundary() {
        assert_eq!(
            relative_recorded("/workspace/web/code", "/workspace", "web").unwrap(),
            "code"
        );
        assert_eq!(
            relative_recorded("C:\\work\\web\\code", "C:\\work", "web").unwrap(),
            "code"
        );
        assert!(relative_recorded("/workspace/web-other/code", "/workspace", "web").is_err());
        assert!(relative_recorded("../outside", "/workspace", "web").is_err());
        assert!(relative_recorded(".git/config", "/workspace", "web").is_err());
    }
}
