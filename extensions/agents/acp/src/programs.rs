//! The launch data of the programs this build ships, which the generic adapter carries for the manifests that name
//! the protocol: what only a program of the Agent Client Protocol has (how its configuration is written, how it
//! names an approval mode, what its tools are called, what it must not send), beside its manifest in the registry
//! of programs.
//!
//! A program that speaks the protocol is data, so each is a file under `programs/` read into the crate: nothing
//! here is code about a program. Who the program is (its kind, its name, the names of its file, the files it reads
//! its instructions from) is its manifest's and is read from there, so that nothing is written twice; a program
//! with no file here is driven from its manifest alone: its arguments, its variables and its files.

use orch_agent_programs::ProgramManifest;
use orch_agent_programs::manifest::InstructionScope;
use serde_json::{Value as Json, json};

use crate::program::{AcpProgram, ProgramError};

/// The launch data of DeepSeek Harness (`dsh --profile acp`).
pub const DSH: &str = include_str!("../programs/dsh.json");

/// The kind of DeepSeek Harness.
pub const DSH_KIND: &str = "dsh";

/// The companion plugin the adapter carries for a kind of program, where it carries one: a file of the program's own
/// plugin system, built from `dsh-bridge/` and kept in the repository so that the adapter holds exactly what was
/// checked. It is written into each session's home before the program starts.
#[must_use]
pub fn plugin_bundle(kind: &str) -> Option<&'static [u8]> {
    match kind {
        DSH_KIND => Some(include_bytes!("../dsh-bridge/dist/orch-dsh-bridge.mjs")),
        _ => None,
    }
}

/// The launch data the adapter carries for a kind of program, where it carries any.
fn data_of(kind: &str) -> Option<&'static str> {
    match kind {
        DSH_KIND => Some(DSH),
        _ => None,
    }
}

/// The program a manifest describes, ready for the adapter: the launch data of its kind with what the manifest
/// says laid in, or, for a kind the adapter carries nothing for, what the manifest's launch says alone.
pub fn for_manifest(manifest: &ProgramManifest) -> Result<AcpProgram, ProgramError> {
    let mut data: Json = match data_of(&manifest.kind) {
        Some(text) => serde_json::from_str(text)
            .map_err(|error| ProgramError::Unreadable(error.to_string()))?,
        None => json!({ "supported_range": "*" }),
    };
    let Some(object) = data.as_object_mut() else {
        return Err(ProgramError::Unreadable(
            "the launch data is not an object".to_owned(),
        ));
    };
    object.insert("kind".to_owned(), json!(manifest.kind));
    object.insert("title".to_owned(), json!(manifest.name()));
    object.insert("programs".to_owned(), json!(manifest.program_names()));
    object.insert("text".to_owned(), json!(manifest.name_text()));
    let launch = &manifest.launch;
    if !object.contains_key("args") && !launch.args.is_empty() {
        object.insert("args".to_owned(), json!(launch.args));
    }
    if !object.contains_key("env") && !launch.env.is_empty() {
        object.insert("env".to_owned(), json!(launch.env));
    }
    if !object.contains_key("files") && !launch.files.is_empty() {
        let files: Vec<Json> = launch
            .files
            .iter()
            .map(|file| json!({ "path": file.path, "template": file.content }))
            .collect();
        object.insert("files".to_owned(), json!(files));
    }
    let instructions: Vec<&str> = manifest
        .files
        .instruction_files
        .iter()
        .filter(|file| file.scope == InstructionScope::Workspace)
        .map(|file| file.path.as_str())
        .collect();
    object.insert("instruction_files".to_owned(), json!(instructions));
    AcpProgram::from_json(&data)
}

/// The program of DeepSeek Harness, as the registry's manifest and the launch data make it.
pub fn dsh() -> Result<AcpProgram, ProgramError> {
    for_manifest(orch_agent_programs::shipped(DSH_KIND))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{Variables, render};
    use std::path::PathBuf;

    fn variables(models: &[&str]) -> Variables {
        let mut variables = Variables::new(PathBuf::from("/h/threads/t/s"));
        variables.set("profile_home", "/h");
        variables.set("cwd", "/w/app");
        variables.set(
            "llm_url",
            "http://127.0.0.1:41000/llm/0123456789abcdef0123456789abcdef/v1",
        );
        variables.set("llm_token", "secret-model-token-0123456789");
        variables.set("model", models[0]);
        variables.models = models.iter().map(|model| (*model).to_owned()).collect();
        variables.set("api", "openai-completions");
        variables.set("context_window", "200000");
        let program = dsh().unwrap();
        variables.approval = program.approval(orch_agent_contract::descriptor::ApprovalMode::Ask);
        variables.set(
            "permission_mode",
            variables.approval.permission_mode.clone(),
        );
        variables
    }

    #[test]
    fn the_patch_of_a_session_switches_off_the_programs_own_goal_rows_so_orchs_goal_is_the_only_one()
     {
        let program = dsh().unwrap();
        let plan = program.plan(&variables(&["scripted"])).unwrap();
        let patch = String::from_utf8(plan.files[0].content.clone()).unwrap();
        // The service, the driver that starts a turn by itself, the slash command and the three tools
        // (`create_goal`, `get_goal`, `update_goal`) of dsh: a goal continues only through Orch's checks.
        for row in ["goal", "goal-round-driver", "command-goal", "tool-goal"] {
            assert!(
                patch.contains(&format!("- id: {row}\n  disabled: true\n")),
                "the row {row} is not switched off: {patch}"
            );
        }
    }

    #[test]
    fn the_launch_data_of_deepseek_harness_reads_and_names_the_two_privacy_rows() {
        let program = dsh().unwrap();
        assert_eq!(program.kind, DSH_KIND);
        assert_eq!(program.title, "DeepSeek Harness");
        assert_eq!(
            program.programs,
            [
                "deepseek_harness_runtime/runtime/deepseek-harness-sdk-runtime-linux-x64",
                "dsh"
            ],
            "the names of its file are the manifest's"
        );
        assert_eq!(program.instruction_files, ["AGENTS.md", "CLAUDE.md"]);
        assert_eq!(
            program.privacy.refuse_members,
            ["dsh_session_log", "dsh_plugin_packages"]
        );
        assert_eq!(program.privacy.strip_headers, ["x-deepseek-harness-*"]);
        let patch = &program.files[0].template;
        for row in ["session-log-deepseek", "plugin-package-inventory-deepseek"] {
            assert!(
                patch.contains(&format!("- id: {row}\n  disabled: true")),
                "{row} is not loaded at all"
            );
        }
        assert_eq!(program.env["DSH_TELEMETRY_DISABLED"], "1");
        assert!(
            program.env.contains_key("DSH_HOME")
                || program.home_variable.as_deref() == Some("DSH_HOME")
        );
    }

    #[test]
    fn the_patch_of_a_session_is_a_document_with_a_model_for_each_the_session_may_use() {
        let program = dsh().unwrap();
        let variables = variables(&["deepseek/deepseek-v4.1-flash", "z-ai/glm-5.3-flash"]);
        let plan = program.plan(&variables).unwrap();
        let patch = String::from_utf8(plan.files[0].content.clone()).unwrap();
        assert!(patch.contains(
            "baseURL: \"http://127.0.0.1:41000/llm/0123456789abcdef0123456789abcdef/v1\""
        ));
        assert_eq!(
            patch.matches("- id: \"").count(),
            2,
            "one model entry for each: {patch}"
        );
        assert!(patch.contains("policy: \"ask\"") && patch.contains("defaultPreset: \"orch-ask\""));
        assert!(
            !patch.contains("secret-model-token-0123456789"),
            "no credential is written into a file: {patch}"
        );
        assert_eq!(plan.env["ORCH_LLM_TOKEN"], "secret-model-token-0123456789");
        assert_eq!(plan.env["DSH_HOME"], "/h/threads/t/s");
        assert_eq!(plan.env["HOME"], "/h/user");
        assert_eq!(plan.args[..2], ["--profile", "acp"]);
        assert_eq!(plan.files.len(), 1, "no plugin file, no patch for it");
    }

    fn provider_patch(serves: &[&str]) -> (String, String) {
        use orch_agent_contract::arrangement::{
            self, CredentialLocation, Permission, PlaceSide, Policy, ProviderSide, Serves,
        };
        use orch_agent_contract::credentials::CredentialForm;
        use orch_agent_contract::descriptor::ArrangementType;
        let program = dsh().unwrap();
        let descriptor =
            crate::descriptor::descriptor(&program, &crate::descriptor::Learned::default());
        let provider = ProviderSide {
            serves: serves
                .iter()
                .map(|protocol| Serves::Forwarded {
                    protocol: (*protocol).to_owned(),
                })
                .collect(),
            issues: vec![CredentialForm::api_key()],
            credential: CredentialLocation::AtHome,
        };
        let place = PlaceSide {
            environment_id: orch_types::new_id(),
            offers: Vec::new(),
            executors: Vec::new(),
            can_run_harness: true,
            same_machine: false,
            harness_version: None,
        };
        let policy = Policy {
            permission: Permission {
                provider: ArrangementType::ALL.to_vec(),
                ..Permission::default()
            },
            ..Policy::default()
        };
        let arrangement = arrangement::choose(
            &descriptor,
            &provider,
            &place,
            &policy,
            Some(ArrangementType::KeySeparated),
        )
        .unwrap();
        let protocol = arrangement.protocol.unwrap();
        let mut variables = variables(&["scripted"]);
        variables.set("api", program.api_of(&protocol).unwrap());
        let plan = program.plan(&variables).unwrap();
        (
            protocol,
            String::from_utf8(plan.files[0].content.clone()).unwrap(),
        )
    }

    #[test]
    fn a_responses_only_provider_gets_no_chat_completions_compatibility_options() {
        let (protocol, patch) = provider_patch(&["openai-responses"]);
        assert_eq!(protocol, "openai-responses");
        assert!(patch.contains("api: \"openai-responses\""), "{patch}");
        for field in ["compat:", "maxTokensField", "supportsDeveloperRole"] {
            assert!(!patch.contains(field), "{patch}");
        }
        let (_, anthropic) = provider_patch(&["anthropic-messages"]);
        assert!(!anthropic.contains("compat:"), "{anthropic}");
    }

    #[test]
    fn a_provider_serving_both_protocols_keeps_chat_completions_compatibility_options() {
        let (protocol, patch) = provider_patch(&["openai-responses", "openai-chat"]);
        assert_eq!(protocol, "openai-chat", "the program's protocol order wins");
        assert!(patch.contains("api: \"openai-completions\""), "{patch}");
        assert!(patch.contains("        compat:\n          supportsDeveloperRole: false\n          maxTokensField: max_tokens\n        models:"), "{patch}");
    }

    #[test]
    fn with_the_plugin_there_its_patch_is_written_and_loaded() {
        let program = dsh().unwrap();
        let mut variables = variables(&["deepseek/deepseek-v4.1-flash"]);
        variables.set("plugin", "/cache/orch-dsh-bridge/orch-dsh-bridge.mjs");
        let plan = program.plan(&variables).unwrap();
        assert_eq!(plan.files.len(), 2);
        let patch = String::from_utf8(plan.files[1].content.clone()).unwrap();
        assert!(patch.contains("- insert:\n    - id: orch-bridge\n      name: \"/cache/orch-dsh-bridge/orch-dsh-bridge.mjs\""));
        assert!(
            patch.contains("approvals:\n          mode: \"ask\""),
            "{patch}"
        );
        assert!(patch.contains("name: '@deepseek-ai/dsh-tool-ask-user'"));
        assert!(
            !patch.contains("id: acp"),
            "the row of acp stays the base patch's alone: {patch}"
        );
        assert!(plan.args.ends_with(&[
            "--patch".to_owned(),
            "/h/threads/t/s/bridge.patch.yml".to_owned()
        ]));
        let _ = render;
    }

    #[test]
    fn a_manifest_with_no_launch_data_of_its_own_is_driven_from_what_it_says() {
        let manifest: ProgramManifest = serde_json::from_value(json!({
            "manifest_version": 1, "kind": "tiny", "names": { "en": "Tiny Agent" }, "protocol": "acp",
            "launch": { "programs": ["tiny"], "args": ["--acp"], "env": { "HOME": "{home}" },
                        "files": [ { "path": "c.toml", "content": "x = 1" } ] },
            "files": { "instruction_files": [ { "path": "TINY.md", "scope": "workspace" }, { "path": "TINY.md", "scope": "home" } ] }
        }))
        .unwrap();
        let program = for_manifest(&manifest).unwrap();
        assert_eq!(
            (program.kind.as_str(), program.title.as_str()),
            ("tiny", "Tiny Agent")
        );
        assert_eq!(program.args, ["--acp"]);
        assert_eq!(program.env["HOME"], "{home}");
        assert_eq!(program.files[0].path, "c.toml");
        assert_eq!(program.instruction_files, ["TINY.md"]);
    }

    #[test]
    fn a_separated_session_takes_the_environments_tools_from_dsh_and_gives_them_to_the_plugin() {
        let program = dsh().unwrap();
        let mut variables = variables(&["m"]);
        variables.set("plugin", "/cache/orch-dsh-bridge/orch-dsh-bridge.mjs");
        let beside = program.plan(&variables).unwrap();
        assert_eq!(
            beside.files.len(),
            2,
            "no backends patch where the arrangement is not separated"
        );
        variables.set("separated", "true");
        let plan = program.plan(&variables).unwrap();
        assert_eq!(plan.files.len(), 3);
        let patch = String::from_utf8(plan.files[2].content.clone()).unwrap();
        for row in [
            "tool-bash",
            "tool-fs",
            "tool-fs-search",
            "tool-jobs",
            "tool-working-directory",
        ] {
            assert!(
                patch.contains(&format!("- id: {row}\n  disabled: true")),
                "{row}: {patch}"
            );
        }
        assert!(patch.contains("backends:\n      enabled: true"), "{patch}");
        assert!(
            patch.contains("mode: \"ask\""),
            "the plugin's row is given again whole: {patch}"
        );
        let tail: Vec<&str> = plan.args.iter().rev().take(4).map(String::as_str).collect();
        assert_eq!(
            tail,
            [
                "/h/threads/t/s/backends.patch.yml",
                "--patch",
                "/h/threads/t/s/bridge.patch.yml",
                "--patch"
            ],
            "the backends patch is applied after the plugin's"
        );
    }
}
