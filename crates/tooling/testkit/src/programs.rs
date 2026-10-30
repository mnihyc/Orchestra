//! A third agent program that is nothing but a manifest, for the tests of whatever lists, checks, offers and runs programs.
//!
//! The scripted adapter drives it (its protocol is `fake`), so no code stands behind it: whatever a manifest alone
//! can make of a program, it makes of this one. Its interface probe asks a build for `--help` and one flag, and it has
//! no native case, so a build of it is checked by its interface alone.

use orch_agent_programs::ProgramManifest;
use serde_json::{Value, json};

/// The kind of the third program.
pub const THIRD_KIND: &str = "fake-third";

/// The manifest of the third program, as an adapter that is not part of Orch would send it in its `hello`.
#[must_use]
pub fn third_manifest_json() -> Value {
    json!({
        "manifest_version": 1,
        "kind": THIRD_KIND,
        "names": { "en": "Third program", "zh-CN": "第三个程序" },
        "vendor": { "name": "Example", "licence": "Apache-2.0" },
        "protocol": "fake",
        "launch": { "programs": ["fake-third"] },
        "placement": { "colocated": true, "home": "connector", "needs_binary": false },
        "builds": { "channels": [ {
            "type": "pypi", "id": "pypi", "project": "fake-third-runtime",
            "files": [ { "arch": "x86_64", "name": "*manylinux*_x86_64.whl" } ]
        } ] },
        "checks": { "interface": { "args": ["--help"], "printed": "text", "needs": [
            { "name": "--version" }, { "name": "--steer", "capability": "steer" } ] } },
        "files": { "instruction_files": [ { "path": "AGENTS.md", "scope": "workspace" } ], "workspace_config": [".fake-third"] }
    })
}

/// The manifest of the third program.
///
/// # Panics
/// Never: the manifest is written here and a test of this module reads it.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "the manifest is written in this module and its test reads it"
)]
pub fn third_manifest() -> ProgramManifest {
    serde_json::from_value(third_manifest_json()).expect("the third program's manifest reads")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_third_program_is_a_manifest_that_holds_together_and_takes_no_shipped_kind() {
        let manifest = third_manifest();
        assert_eq!(manifest.check(), Ok(()));
        assert!(
            orch_agent_programs::Programs::builtin()
                .with_program(manifest)
                .is_ok()
        );
    }
}
