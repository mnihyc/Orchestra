//! Environment consent for the current version of a confirmed machine program.

use std::collections::BTreeMap;

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A manager's decision for one confirmed program version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionOverride {
    /// The confirmed path on the machine.
    pub path: String,
    /// The program version the machine reported, or `unknown`.
    pub version: String,
    /// The manager who allowed it.
    pub by: Id,
    /// When the manager allowed it.
    pub at: Time,
}

/// The environment's per-harness decisions.
pub type VersionOverrides = BTreeMap<String, VersionOverride>;

/// Whether the two known versions match without a manager's override.
#[must_use]
pub fn verified_equal(program: &str, agent: &str) -> bool {
    !program.is_empty()
        && !agent.is_empty()
        && program != "unknown"
        && agent != "unknown"
        && program == agent
}

/// Whether this exact confirmed program may work with this agent version.
#[must_use]
pub fn allowed(
    overrides: &VersionOverrides,
    kind: &str,
    path: &str,
    program: &str,
    agent: &str,
) -> bool {
    verified_equal(program, agent)
        || overrides.get(kind).is_some_and(|entry| {
            !path.is_empty()
                && !program.is_empty()
                && entry.path == path
                && entry.version == program
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval(path: &str, version: &str) -> VersionOverride {
        VersionOverride {
            path: path.into(),
            version: version.into(),
            by: Id::nil(),
            at: "2026-10-31T00:00:00.000Z".parse().unwrap(),
        }
    }

    #[test]
    fn known_equal_versions_work_and_unknown_needs_consent() {
        let empty = VersionOverrides::new();
        assert!(allowed(&empty, "codex", "/program/codex", "1.2.3", "1.2.3"));
        assert!(!allowed(
            &empty,
            "codex",
            "/program/codex",
            "1.2.3",
            "1.2.4"
        ));
        assert!(!allowed(
            &empty,
            "codex",
            "/program/codex",
            "unknown",
            "unknown"
        ));
        assert!(!allowed(
            &empty,
            "codex",
            "/program/codex",
            "unknown",
            "1.2.3"
        ));
    }

    #[test]
    fn consent_covers_any_agent_version_only_for_the_confirmed_path_and_version() {
        let overrides =
            VersionOverrides::from([("codex".into(), approval("/program/codex", "unknown"))]);
        assert!(allowed(
            &overrides,
            "codex",
            "/program/codex",
            "unknown",
            "1.2.3"
        ));
        assert!(allowed(
            &overrides,
            "codex",
            "/program/codex",
            "unknown",
            "1.2.4"
        ));
        assert!(!allowed(
            &overrides,
            "claude-code",
            "/program/codex",
            "unknown",
            "1.2.3"
        ));
        assert!(!allowed(
            &overrides,
            "codex",
            "/other/codex",
            "unknown",
            "1.2.3"
        ));
        assert!(!allowed(
            &overrides,
            "codex",
            "/program/codex",
            "1.2.5",
            "1.2.3"
        ));
        assert!(!allowed(
            &VersionOverrides::new(),
            "codex",
            "/program/codex",
            "unknown",
            "1.2.3"
        ));
    }
}
