//! Recorded move facts and the one notice delivered with the thread's next real input.

use crate::EngineError;
use orch_db::{Conn, queries::resources::mounts};
use orch_link_contract::events::EnvironmentFactsMount;
use orch_types::{Id, Json};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct MoveNotice {
    pub detached: Vec<String>,
    pub remapped: Vec<(String, String)>,
    pub missing: i64,
}

fn reported(facts: &Json) -> Vec<EnvironmentFactsMount> {
    facts
        .get("mounts")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| serde_json::from_value(value.clone()).ok())
        .collect()
}

impl MoveNotice {
    pub async fn prepare(
        conn: &mut Conn,
        before: &Json,
        after: &Json,
        detached: &[Id],
        missing: i64,
    ) -> Result<Self, EngineError> {
        let before = reported(before);
        let mut names: HashMap<_, _> = mounts::mounts(conn, detached)
            .await?
            .into_iter()
            .map(|row| (row.id, row.name))
            .collect();
        for mount in &before {
            names.insert(mount.mount_id, mount.name.clone());
        }
        let detached = detached
            .iter()
            .map(|id| names.get(id).cloned().unwrap_or_else(|| id.to_string()))
            .collect();
        let mut remapped = Vec::new();
        for after in reported(after) {
            if let Some(old) = before.iter().find(|old| old.mount_id == after.mount_id)
                && let (Some(old), Some(new)) = (&old.path, after.path)
                && old != &new
            {
                remapped.push((after.name, new));
            }
        }
        Ok(Self {
            detached,
            remapped,
            missing,
        })
    }

    pub fn text(&self, directory: &str, history: &str) -> String {
        let mut text = format!(
            "The environment moved to another machine. Your directory is still {directory}.\nProcesses from before the move are not running here."
        );
        if self.missing > 0 {
            text.push_str(&format!(
                " {} files could not be copied; see .orch/handoff/missing-files.txt.",
                self.missing
            ));
        }
        if !self.detached.is_empty() {
            text.push_str(&format!(
                "\nThese directories are no longer available: {}.",
                self.detached.join(", ")
            ));
        }
        for (name, path) in &self.remapped {
            text.push_str(&format!(" {name} is now at {path}."));
        }
        if !history.is_empty() {
            text.push('\n');
            text.push_str(history);
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_reports_only_actual_copy_and_mount_changes_before_retained_history() {
        let plain = MoveNotice::default().text("web", "");
        assert_eq!(
            plain,
            "The environment moved to another machine. Your directory is still web.\nProcesses from before the move are not running here."
        );
        let changed = MoveNotice {
            detached: vec!["Old data".into()],
            remapped: vec![("Assets".into(), "/mnt/assets".into())],
            missing: 2,
        }
        .text("web", "Retained conversation");
        assert!(
            changed.contains("2 files could not be copied; see .orch/handoff/missing-files.txt.")
        );
        assert!(changed.contains("These directories are no longer available: Old data."));
        assert!(changed.contains("Assets is now at /mnt/assets."));
        assert!(changed.ends_with("\nRetained conversation"));
    }
}
