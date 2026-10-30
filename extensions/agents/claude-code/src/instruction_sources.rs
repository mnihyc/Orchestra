//! Reported instruction loads and imports, observed without changing the harness's decisions or text.

use orch_agent_contract::session::{InstructionEvidence, InstructionFile, SessionSpec};
use orch_types::Json;
use std::path::{Component, Path};

/// A workspace path which cannot traverse into a person's home through a link or `..`.
fn inside(path: &Path, root: &Path) -> bool {
    path.is_absolute()
        && path.starts_with(root)
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        && path
            .canonicalize()
            .ok()
            .zip(root.canonicalize().ok())
            .is_some_and(|(path, root)| path.starts_with(root))
}

/// The native InstructionsLoaded callback says which file was loaded, including an imported file's parent.
/// Only the metadata of a local workspace file is measured; personal and managed sources stay labelled and unread.
#[must_use]
pub fn loaded(input: &Json, spec: &SessionSpec) -> Option<InstructionFile> {
    if input.get("hook_event_name").and_then(Json::as_str) != Some("InstructionsLoaded") {
        return None;
    }
    let path = Path::new(input.get("file_path")?.as_str()?);
    let root = Path::new(&spec.settings.workspace_root);
    let reason = input.get("load_reason").and_then(Json::as_str);
    let local = spec.exec.executor.is_none();
    let mut source = if local
        && inside(path, root)
        && input.get("memory_type").and_then(Json::as_str) != Some("User")
    {
        let label = path.strip_prefix(root).ok()?.display().to_string();
        let mut measured =
            InstructionFile::measured(label, path, InstructionEvidence::Reported, None);
        // The hook reports a load, not the number of bytes or lines the native context kept.
        measured.loaded_bytes = None;
        measured.loaded_lines = None;
        measured.cut_bytes = None;
        measured.reason =
            Some("the harness reported loading this file, but not its loaded size".to_owned());
        measured
    } else {
        let label = if input.get("memory_type").and_then(Json::as_str) == Some("Managed") {
            "Managed instruction file"
        } else {
            "Personal or external instruction file"
        };
        InstructionFile::unmeasured(
            label,
            None,
            InstructionEvidence::Reported,
            "outside the local workspace: its path and contents stay private",
        )
        .of_home()
    };
    source.load_reason = reason
        .filter(|reason| {
            matches!(
                *reason,
                "session_start" | "nested_traversal" | "path_glob_match" | "include" | "compact"
            )
        })
        .map(str::to_owned);
    source.parent_path = input
        .get("parent_file_path")
        .and_then(Json::as_str)
        .map(Path::new)
        .filter(|parent| !source.home && inside(parent, root))
        .map(|parent| parent.display().to_string());
    Some(source)
}

/// Reports supersede a documented file's measurements, while unreported sources keep their honest evidence.
#[must_use]
pub fn merge(
    mut documented: Vec<InstructionFile>,
    reported: Vec<InstructionFile>,
) -> Vec<InstructionFile> {
    for mut source in reported {
        if let Some(old) = documented.iter_mut().find(|old| {
            old.path == source.path && old.label == source.label && old.home == source.home
        }) {
            source.line_limit = source.line_limit.or(old.line_limit);
            source.byte_limit = source.byte_limit.or(old.byte_limit);
            *old = source;
        } else {
            documented.push(source);
        }
    }
    documented
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_reported_import_is_linked_to_its_parent_and_later_loads_are_reported_without_a_guess_at_the_cut()
     {
        let root = std::env::temp_dir().join(format!("orch-loaded-{}", orch_types::new_id()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "@nested/rule.md").unwrap();
        std::fs::write(root.join("nested/rule.md"), "Keep the tests readable\n").unwrap();
        let spec: SessionSpec = serde_json::from_value(json!({"agent_id":orch_types::new_id(),"thread_id":orch_types::new_id(),"segment_id":orch_types::new_id(),"mode":"new","native_session_ref":null,"upto_turn_ordinal":null,"workspace_root":root,"instruction_root_marker":false,"directory":root,"cwd":root,"mounts":[],"model":"claude-opus-5","effort":null,"params":{},"approval_mode":"on_request","sandbox_mode":"workspace","instructions":"","memory":null,"arrangement":"together","orch_tools":[],"raw_events":false,"session_key":"abc","archive":null,"history":null,"provider":{"path":"native","provider_id":orch_types::new_id()},"exec":{"executor":null,"environment_id":orch_types::new_id(),"generation":1},"mcp":[]})).unwrap();
        let imported = loaded(&json!({"hook_event_name":"InstructionsLoaded","file_path":root.join("nested/rule.md"),"parent_file_path":root.join("CLAUDE.md"),"load_reason":"include","memory_type":"Project"}), &spec).unwrap();
        assert_eq!(imported.evidence, InstructionEvidence::Reported);
        assert_eq!(imported.load_reason.as_deref(), Some("include"));
        assert_eq!(
            imported.parent_path,
            Some(root.join("CLAUDE.md").display().to_string())
        );
        assert_eq!(imported.bytes, Some(24));
        assert!(imported.loaded_bytes.is_none() && imported.cut_bytes.is_none());
        let private = loaded(&json!({"hook_event_name":"InstructionsLoaded","file_path":"/private-home/CLAUDE.md","parent_file_path":root.join("CLAUDE.md"),"load_reason":"include","memory_type":"User"}), &spec).unwrap();
        assert!(
            private.home
                && private.path.is_none()
                && private.parent_path.is_none()
                && private.bytes.is_none()
        );
        assert!(loaded(&json!({"hook_event_name":"PostToolUse"}), &spec).is_none());
    }
}
