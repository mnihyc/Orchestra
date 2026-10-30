//! The line every act leaves in the history of the thing it was done to: a marker in a thread's timeline and its History view, an entry of a place's Activity, a line on a resource's page, and for an environment what was done in it: shells, files written, an agent's changes turn by turn.
//!
//! Every act that needed a permission and changed something leaves one audit row marked as a line of
//! the history of what it was done to, with a sentence for a summary, in the transaction of the act
//! (`orch_access_contract::audit`); there is no second record to keep in step. What is here is how
//! those rows are read back by the request that lists a thing's history, which is the same for a
//! node, a resource, a document and an environment:
//!
//! - which kind of line in an environment's history an action is (where it is, who works there,
//!   what its agents work with, its shells, its files, its page), so that a history can be asked
//!   for by kinds;
//! - the thread a line is about, which decides whether a reader may see it: a line about a thread
//!   the reader may not view is left out;
//! - what a line points at, from the few facts it holds, so that a client knows what to open. A line
//!   carries no content, only the way to it.

use orch_access_contract::audit::AuditAction;
use orch_api_contract::access::{HistoryAbout, HistoryKind};
use orch_api_contract::nodes::NodeRef;
use orch_types::{Id, Json};

/// The history vocabulary for known actions. No English sentence is interpreted as an action.
#[must_use]
pub fn wording(action: &str) -> Option<orch_api_contract::access::HistoryWording> {
    let verb = match action {
        "node.created" | "document.created" | "resource.created" => "created",
        "node.renamed" | "document.renamed" => "renamed",
        "node.described" => "described",
        "thread.model_changed" => "model",
        "node.updated" => "changed",
        "settings.updated"
        | "settings.reset"
        | "settings.secret_set"
        | "settings.secret_deleted" => "setting",
        "permission.added"
        | "permission.updated"
        | "permission.row_changed"
        | "permission.removed"
        | "page.changed"
        | "access.changed"
        | "role.given"
        | "role.changed"
        | "role.removed" => "access",
        "node.transferred" | "resource.transferred" | "owner.added" | "owner.removed" => "owner",
        "node.moved" | "thread.moved" | "environment.moved" => "moved",
        "thread.forked" | "thread.forked_from" => "forked",
        "thread.bound" | "thread.switched" => "switched",
        "environment.started" | "provider.runtime_started" | "terminal.opened" | "side.started" => {
            "started"
        }
        "environment.stopped"
        | "provider.runtime_stopped"
        | "terminal.closed"
        | "turn.interrupted"
        | "side.closed" => "stopped",
        "node.archived" | "document.archived" => "archived",
        "node.unarchived" | "document.unarchived" => "unarchived",
        "node.deleted" | "document.deleted" | "environment.deleted" => "deleted",
        "node.restored" | "document.restored" | "workspace.state_restored" => "restored",
        "share.created" | "share.copied" | "document.shared" => "shared",
        "share.ended" | "share.removed" | "document.unshared" => "unshared",
        "approval.decided" => "decided",
        "queue.held" | "input.held" => "held",
        "queue.resumed" | "input.recovered" => "resumed",
        "workspace.file_written"
        | "workspace.file_deleted"
        | "workspace.file_renamed"
        | "git.committed"
        | "git.pushed" => "files",
        "capability.granted" => "capabilityGranted",
        "capability.revoked" => "capabilityRevoked",
        "environment.exec_changed" | "mcp.imported" => "tools",
        "environment.recreated" | "provider.runtime_recreated" => "recreated",
        "approval.rule_added" => "ruleAdded",
        "approval.rule_revoked" => "ruleRevoked",
        _ => return None,
    };
    Some(orch_api_contract::access::HistoryWording {
        version: 1,
        verb: verb.to_owned(),
        target: None,
    })
}

/// Structured access facts refine the same vocabulary on Activity and History, with only a permitted target.
#[must_use]
pub fn wording_of(
    action: &str,
    details: &Json,
    target: Option<NodeRef>,
) -> Option<orch_api_contract::access::HistoryWording> {
    let mut words = wording(action)?;
    words.target = target;
    if matches!(
        action,
        "permission.added" | "permission.updated" | "permission.row_changed" | "permission.removed"
    ) {
        let agents = details.pointer("/holder/kind").and_then(Json::as_str) == Some("agents");
        let defaults = details.get("page").and_then(Json::as_str) == Some("defaults");
        words.verb = match (agents, defaults) {
            (true, true) => "agentDefaults",
            (true, false) => "agentAccess",
            (false, true) => "defaults",
            (false, false) => "access",
        }
        .to_owned();
    }
    Some(words)
}

/// The kind of line in the history of an environment that an action is, if it is one of those a
/// history is asked for by.
#[must_use]
pub fn kind_of(action: AuditAction) -> Option<HistoryKind> {
    let key = action.as_str();
    Some(match action {
        AuditAction::EnvironmentExecChanged => HistoryKind::Tools,
        AuditAction::ThreadBound | AuditAction::ThreadSwitched => HistoryKind::Threads,
        AuditAction::ResourceTransferred => HistoryKind::Access,
        _ if key.starts_with("environment.") => HistoryKind::Lifecycle,
        _ if key.starts_with("capability.") || key.starts_with("mcp.") => HistoryKind::Tools,
        _ if key.starts_with("terminal.") || key == "process.signalled" => HistoryKind::Shells,
        _ if key.starts_with("workspace.") => HistoryKind::Files,
        _ if key.starts_with("permission.") => HistoryKind::Access,
        _ => return None,
    })
}

/// The actions whose lines are of one of `kinds`, written out in full. Only actions that are lines
/// of a history are among them.
#[must_use]
pub fn actions_of(kinds: &[HistoryKind]) -> Vec<String> {
    AuditAction::ALL
        .iter()
        .filter(|action| action.is_history_line())
        .filter(|action| kind_of(**action).is_some_and(|kind| kinds.contains(&kind)))
        .map(|action| action.as_str().to_owned())
        .collect()
}

fn id_of(details: &Json, member: &str) -> Option<Id> {
    details.get(member)?.as_str()?.parse().ok()
}

/// The thread a line is about: the thread that is its object, or the one its facts name.
#[must_use]
pub fn thread_of(object_kind: &str, object: Option<Id>, details: &Json) -> Option<Id> {
    if object_kind == "thread" {
        return object;
    }
    id_of(details, "thread_id").or_else(|| id_of(details, "thread"))
}

/// What a line points at, from the facts it holds, with the thread named as the reader may know it.
/// Nothing where the line points at nothing.
#[must_use]
pub fn about_of(details: &Json, thread: Option<NodeRef>) -> Option<HistoryAbout> {
    let about = HistoryAbout {
        thread,
        turn_id: id_of(details, "turn_id"),
        terminal_id: id_of(details, "terminal_id"),
        kept: details.get("kept").and_then(Json::as_bool),
        path: details
            .get("path")
            .and_then(Json::as_str)
            .map(str::to_owned),
        paths: details
            .get("paths")
            .and_then(|paths| serde_json::from_value(paths.clone()).ok()),
        more: details.get("more").and_then(Json::as_i64),
    };
    (about != HistoryAbout::default()).then_some(about)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_was_done_in_an_environment_is_told_by_kind() {
        assert_eq!(
            kind_of(AuditAction::TerminalOpened),
            Some(HistoryKind::Shells)
        );
        assert_eq!(
            kind_of(AuditAction::WorkspaceFileWritten),
            Some(HistoryKind::Files)
        );
        assert_eq!(
            kind_of(AuditAction::EnvironmentStopped),
            Some(HistoryKind::Lifecycle)
        );
        assert_eq!(
            kind_of(AuditAction::EnvironmentExecChanged),
            Some(HistoryKind::Tools),
            "what its agents work with is not where it is"
        );
        assert_eq!(
            kind_of(AuditAction::ThreadBound),
            Some(HistoryKind::Threads)
        );
        assert_eq!(
            kind_of(AuditAction::PermissionAdded),
            Some(HistoryKind::Access)
        );
        assert_eq!(kind_of(AuditAction::NodeCreated), None);
    }

    #[test]
    fn a_history_asked_for_by_kinds_holds_only_lines_and_only_those_kinds() {
        let shells = actions_of(&[HistoryKind::Shells]);
        assert!(shells.contains(&"terminal.opened".to_owned()));
        assert!(!shells.contains(&"workspace.file_written".to_owned()));
        for action in actions_of(&[
            HistoryKind::Lifecycle,
            HistoryKind::Threads,
            HistoryKind::Tools,
            HistoryKind::Shells,
            HistoryKind::Files,
            HistoryKind::Access,
        ]) {
            let parsed: AuditAction = action.parse().unwrap();
            assert!(parsed.is_history_line(), "{action} is a line of a history");
        }
    }

    #[test]
    fn a_line_is_about_the_thread_that_is_its_object_or_that_its_facts_name() {
        let thread = orch_types::new_id();
        assert_eq!(thread_of("thread", Some(thread), &json!({})), Some(thread));
        assert_eq!(
            thread_of("terminal", None, &json!({ "thread_id": thread })),
            Some(thread)
        );
        assert_eq!(thread_of("terminal", None, &json!({})), None);
    }

    #[test]
    fn a_line_points_at_what_its_facts_name_and_at_nothing_else() {
        let turn = orch_types::new_id();
        let about = about_of(
            &json!({ "turn_id": turn, "paths": ["a.rs", "b.rs"], "more": 3, "other": 1 }),
            None,
        )
        .unwrap();
        assert_eq!(about.turn_id, Some(turn));
        assert_eq!(
            about.paths.as_deref(),
            Some(&["a.rs".to_owned(), "b.rs".to_owned()][..])
        );
        assert_eq!(about.more, Some(3));
        assert_eq!(about_of(&json!({ "role": "viewer" }), None), None);
    }
}
