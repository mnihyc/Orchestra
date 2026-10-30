//! Health checks of a new side, which spend no token.
//!
//! A unit that was just made, or started again, is healthy when its connector holds a session, it
//! has reported what it is and where its workspace is (which it does when the environment is
//! attached to it), and the runtime row does not say that the unit failed. Nothing here asks a
//! model for anything: the checks look at what the unit reported of itself.

use orch_types::Json;

/// What was looked at about a new side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side<'a> {
    /// Whether the unit's connector holds a session now.
    pub online: bool,
    /// What the connector reported of the environment when it was attached to it.
    pub facts: &'a Json,
    /// The state the unit's runtime stands in.
    pub runtime_state: &'a str,
}

/// What is wrong with a new side: a sentence for each check that fails; none when it is healthy.
#[must_use]
pub fn findings(side: &Side<'_>) -> Vec<String> {
    let mut wrong = Vec::new();
    if side.runtime_state == "failed" {
        wrong.push("the unit failed".to_owned());
    }
    if !side.online {
        wrong.push("the connector in the unit holds no session".to_owned());
    }
    match side.facts.get("workspace_root").and_then(Json::as_str) {
        None | Some("") => {
            wrong.push("the connector has not reported where the workspace is".to_owned());
        }
        Some(root) if !is_absolute(root) => {
            wrong.push(format!(
                "the workspace root {root:?} is not an absolute path"
            ));
        }
        Some(_) => {}
    }
    if side.facts.get("os").and_then(Json::as_str).is_none() {
        wrong.push("the connector has not reported what the environment is".to_owned());
    }
    wrong
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with("\\\\")
        || path.as_bytes().get(..3).is_some_and(|start| {
            start[0].is_ascii_alphabetic() && start[1] == b':' && matches!(start[2], b'\\' | b'/')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_side_is_healthy_when_it_is_online_attached_and_not_failed() {
        let facts = json!({ "os": "linux", "workspace_root": "/workspace" });
        let healthy = Side {
            online: true,
            facts: &facts,
            runtime_state: "creating",
        };
        assert!(findings(&healthy).is_empty());
        let offline = Side {
            online: false,
            ..healthy.clone()
        };
        assert_eq!(findings(&offline).len(), 1);
        let empty = json!({});
        let unattached = Side {
            online: true,
            facts: &empty,
            runtime_state: "failed",
        };
        assert_eq!(
            findings(&unattached).len(),
            3,
            "failed, no root, no operating system"
        );
    }

    #[test]
    fn a_workspace_root_is_an_absolute_path_of_either_platform() {
        for root in ["/workspace", "C:\\work", "c:/work", "\\\\host\\share"] {
            assert!(is_absolute(root), "{root}");
        }
        for root in ["work", "./work", "", "C:"] {
            assert!(!is_absolute(root), "{root}");
        }
    }
}
