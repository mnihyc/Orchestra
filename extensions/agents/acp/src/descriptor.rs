//! What the adapter claims of an ACP program: from what the program said in `initialize`, what its companion plugin announced, and what its manifest declares.
//!
//! A claim is made only where the program does what the claim says. The protocol's own announcements turn on the
//! claims it can support (a session that resumes, a prompt that takes images, MCP servers by address); a companion
//! plugin's extras turn on the claims they exist for ([`crate::extensions`]); and the manifest's `capabilities`
//! member is a patch merged over the result for what only the program's authors know. Nothing else is claimed, so
//! that a program is offered only what it has.

use orch_agent_contract::descriptor::{
    AgentDescriptor, AgentTerminal, ArrangementEntry, ArrangementType, AuxiliaryModels,
    Capabilities, EffortSupport, ExecutorBuild, HarnessInfo, McpSupport, MemoryKind,
    OrchToolsSupport, PlanMechanics, PlanProceed, PlanProposal, SessionReload, SkillsSupport,
    UsageContext, classify_version,
};
use orch_agent_contract::events::HarnessRequestKind;
use orch_plugin_contract::Extensions;
use serde_json::Value as Json;

use crate::extensions::{Extra, Hello};
use crate::program::{AcpProgram, SkillsMode};
use crate::wire::Initialized;

/// The version of the adapter's own declarations.
pub const ADAPTER_VERSION: u32 = 1;

/// The executor an agent that uses the client's file and terminal methods works through: Orch's own.
pub const ORCH_EXECUTOR: &str = "orch-exec";

/// The tool set of that executor whose tools the client's methods map to one for one.
pub const PLAIN_TOOL_SET: &str = "plain";

/// What was learned of a program when it was started: the answer to `initialize`, the plugin's hello, the version.
#[derive(Debug, Clone, Default)]
pub struct Learned {
    /// The version of the program.
    pub version: String,
    /// What the program answered to `initialize`.
    pub initialized: Initialized,
    /// What its companion plugin announced, if it did.
    pub hello: Option<Hello>,
    /// The extras that were accepted.
    pub extras: Vec<Extra>,
}

impl Learned {
    /// Whether an extra is on.
    #[must_use]
    pub fn has(&self, extra: Extra) -> bool {
        self.extras.contains(&extra)
    }
}

/// Whether the program asks questions by the protocol's own elicitation, as its manifest says.
fn agent_elicits(program: &AcpProgram) -> bool {
    program.elicitation
}

/// The capabilities the protocol and the plugin give, before the manifest's patch.
#[must_use]
pub fn baseline(program: &AcpProgram, learned: &Learned) -> Capabilities {
    let agent = &learned.initialized.agent_capabilities;
    let sessions = &agent.session_capabilities;
    let images = program.images.unwrap_or(agent.prompt_capabilities.image);
    let mut approvals = vec![
        HarnessRequestKind::Command,
        HarnessRequestKind::FileChange,
        HarnessRequestKind::Other,
    ];
    // The program's own questions to a person arrive as elicitations, and the plugin's as questions of its own.
    if agent_elicits(program) {
        approvals.push(HarnessRequestKind::UserInput);
    }
    if learned.has(Extra::Questions) && !approvals.contains(&HarnessRequestKind::UserInput) {
        approvals.push(HarnessRequestKind::UserInput);
    }
    if learned.has(Extra::PlanMode) {
        approvals.push(HarnessRequestKind::Plan);
    }
    let effort = if program.effort_option.is_empty() {
        EffortSupport::None
    } else {
        EffortSupport::Turn
    };
    Capabilities {
        steer: learned.has(Extra::Steering),
        interrupt: true,
        background_commands: false,
        approvals,
        subagents: false,
        subagent_limits: Vec::new(),
        plan: false,
        plan_mode: learned.has(Extra::PlanMode),
        // A plan is put to the person as a request that waits, and Proceed answers it.
        plan_mechanics: learned.has(Extra::PlanMode).then_some(PlanMechanics {
            proposal: PlanProposal::Request,
            proceed: PlanProceed::Allow,
        }),
        reasoning_summary: false,
        input_images: images,
        effort,
        mcp_discovery: false,
        mcp: McpSupport {
            stdio: true,
            http: agent.mcp_capabilities.http,
            reload: false,
            strict: false,
        },
        orch_tools: if agent.mcp_capabilities.http {
            OrchToolsSupport::Mcp
        } else {
            OrchToolsSupport::None
        },
        export_native: false,
        import_native: false,
        import_neutral: learned.has(Extra::NativeCarry),
        fork_native: false,
        native_input_prefix: None,
        resume: sessions.resume.is_some() || agent.load_session,
        discover_sessions: sessions.list.is_some(),
        usage_tokens: learned.has(Extra::Usage),
        // Plain Agent Client Protocol gives no count of tokens and, unless the program is known to send
        // `usage_update`, no size of the context either: what is not reported is declared unknown, never zero.
        usage_context: if learned.has(Extra::Usage) || program.reports_context {
            UsageContext::SizeOnly
        } else {
            UsageContext::None
        },
        phases: learned.has(Extra::Compaction),
        provider_facts: Vec::new(),
        model_discovery: false,
        title: false,
        auxiliary_models: AuxiliaryModels::Uncontrolled,
        telemetry_off: false,
        memory: MemoryKind::None,
        agent_terminal: AgentTerminal::None,
        session_reload: SessionReload::default(),
        instruction_root_marker: false,
        skills: if program.skills == SkillsMode::Directory || learned.has(Extra::Skills) {
            SkillsSupport::Directory
        } else {
            SkillsSupport::None
        },
        approval_reviewer: None,
    }
}

/// Merges `patch` over `base`: an object member by member, anything else replaced, `null` removing a member.
pub fn merge(base: &mut Json, patch: &Json) {
    match (base, patch) {
        (Json::Object(base), Json::Object(patch)) => {
            for (key, value) in patch {
                if value.is_null() {
                    base.remove(key);
                } else {
                    merge(base.entry(key.clone()).or_insert(Json::Null), value);
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// The capabilities with the manifest's patch over them; the baseline where the patch is not readable.
#[must_use]
pub fn capabilities(program: &AcpProgram, learned: &Learned) -> Capabilities {
    let base = baseline(program, learned);
    let Some(patch) = program.capabilities.as_ref() else {
        return base;
    };
    let Ok(mut written) = serde_json::to_value(&base) else {
        return base;
    };
    merge(&mut written, patch);
    serde_json::from_value(written).unwrap_or(base)
}

/// The ways the program can be placed: what the manifest lists, with `separated` only where the client's methods
/// or the plugin's backends make it real.
#[must_use]
pub fn arrangements(program: &AcpProgram, learned: &Learned) -> Vec<ArrangementEntry> {
    let uses_client_methods = program.client_methods;
    let mut entries = Vec::new();
    for kind in &program.arrangements {
        match kind {
            ArrangementType::Separated => {
                if uses_client_methods || learned.has(Extra::ExecutorBackends) {
                    entries.push(ArrangementEntry {
                        kind: ArrangementType::Separated,
                        executor: Some(ORCH_EXECUTOR.to_owned()),
                        executor_version: Some(ExecutorBuild::Any),
                        tool_sets: Some(vec![PLAIN_TOOL_SET.to_owned()]),
                    });
                }
            }
            other => entries.push(ArrangementEntry {
                kind: *other,
                executor: None,
                executor_version: None,
                tool_sets: None,
            }),
        }
    }
    if entries.is_empty() {
        entries.push(ArrangementEntry {
            kind: ArrangementType::Together,
            executor: None,
            executor_version: None,
            tool_sets: None,
        });
    }
    entries
}

/// Everything the adapter declares of a program.
#[must_use]
pub fn descriptor(program: &AcpProgram, learned: &Learned) -> AgentDescriptor {
    let capabilities = capabilities(program, learned);
    let mut approval_modes = program.approval_modes();
    if approval_modes.is_empty() {
        approval_modes = vec![orch_agent_contract::descriptor::ApprovalMode::Ask];
    }
    AgentDescriptor {
        kind: program.kind.clone(),
        adapter_version: ADAPTER_VERSION,
        harness: HarnessInfo {
            name: program.title.clone(),
            version: learned.version.clone(),
            supported_range: program.supported_range.clone(),
            status: classify_version(&learned.version, &program.supported_range, true),
        },
        arrangements: arrangements(program, learned),
        native_provider: None,
        native_providers: Vec::new(),
        text: program.text.clone(),
        forwarded_protocols: program.protocol_order(),
        direct_protocols: Vec::new(),
        credentials: orch_agent_contract::credentials::CredentialSupport::default(),
        runtime_template: None,
        approval_modes,
        sandbox_modes: program.sandbox_modes.clone(),
        side_thread_modes: Vec::new(),
        capabilities,
        parameters: Vec::new(),
        extensions: Extensions::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn program() -> AcpProgram {
        AcpProgram::from_json(&json!({
            "kind": "demo", "title": "Demo", "supported_range": ">=0.2.1-rc.2, <0.3.0",
            "arrangements": ["separated", "key_separated", "together"],
            "approvals": { "ask": {}, "bypass": {} },
            "protocols": { "api": { "openai-chat": "openai-completions" } },
            "capabilities": { "telemetry_off": true, "mcp": { "strict": true } },
            "companion": { "extras": ["steering", "usage"] }
        }))
        .unwrap()
    }

    fn learned() -> Learned {
        let initialized = serde_json::from_value(json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "mcpCapabilities": { "http": true },
                "promptCapabilities": { "image": true },
                "sessionCapabilities": { "close": {}, "list": {}, "resume": {} }
            }
        }))
        .unwrap();
        Learned {
            version: "0.2.1-rc.2".into(),
            initialized,
            hello: None,
            extras: Vec::new(),
        }
    }

    #[test]
    fn the_protocol_alone_gives_a_session_that_resumes_images_and_mcp_by_address_and_nothing_more()
    {
        let declared = descriptor(&program(), &learned());
        assert_eq!(declared.check(), Ok(()));
        let caps = &declared.capabilities;
        assert!(caps.resume && caps.discover_sessions && caps.input_images && caps.interrupt);
        assert!(
            caps.mcp.http && caps.mcp.stdio && caps.mcp.strict,
            "the manifest's patch is merged"
        );
        assert!(caps.telemetry_off);
        assert!(!caps.steer && !caps.plan_mode && !caps.usage_tokens && !caps.fork_native);
        assert_eq!(caps.orch_tools, OrchToolsSupport::Mcp);
        assert_eq!(
            declared.harness.status,
            orch_agent_contract::descriptor::HarnessSupport::Supported
        );
        assert_eq!(declared.forwarded_protocols, ["openai-chat"]);
        assert!(
            !declared.supports(ArrangementType::Separated),
            "a program that uses neither the client's methods nor a plugin's backends is not separated"
        );
        assert!(
            declared.supports(ArrangementType::KeySeparated)
                && declared.supports(ArrangementType::Together)
        );
    }

    #[test]
    fn a_plugins_extras_turn_on_exactly_the_claims_they_exist_for() {
        let mut learned = learned();
        learned.extras = vec![
            Extra::Steering,
            Extra::Usage,
            Extra::PlanMode,
            Extra::ExecutorBackends,
            Extra::Questions,
            Extra::Fork,
            Extra::Compaction,
        ];
        let declared = descriptor(&program(), &learned);
        let caps = &declared.capabilities;
        assert!(caps.steer && caps.usage_tokens && caps.plan_mode && caps.phases);
        assert!(
            !caps.fork_native,
            "a copy of a session is claimed only where the adapter can make one"
        );
        assert!(caps.approvals.contains(&HarnessRequestKind::Plan));
        assert!(caps.approvals.contains(&HarnessRequestKind::UserInput));
        let separated = declared
            .separated()
            .next()
            .expect("the plugin's backends make the separated arrangement real");
        assert_eq!(separated.executor.as_deref(), Some(ORCH_EXECUTOR));
        assert_eq!(
            separated.tool_sets.as_deref(),
            Some(&[PLAIN_TOOL_SET.to_owned()][..])
        );
        assert_eq!(declared.check(), Ok(()));
    }

    #[test]
    fn a_build_newer_than_the_range_is_untested_and_an_older_one_is_not_driven() {
        let mut learned = learned();
        learned.version = "0.3.0".into();
        assert_eq!(
            descriptor(&program(), &learned).harness.status,
            orch_agent_contract::descriptor::HarnessSupport::Untested
        );
        learned.version = "0.1.0".into();
        assert_eq!(
            descriptor(&program(), &learned).harness.status,
            orch_agent_contract::descriptor::HarnessSupport::Unsupported
        );
    }

    #[test]
    fn a_patch_removes_a_member_with_null_and_replaces_a_list_whole() {
        let mut value = json!({ "a": { "b": 1, "c": 2 }, "d": [1, 2] });
        merge(&mut value, &json!({ "a": { "b": null }, "d": [3] }));
        assert_eq!(value, json!({ "a": { "c": 2 }, "d": [3] }));
    }

    #[test]
    fn a_program_that_reports_neither_tokens_nor_context_declares_both_unknown() {
        let declared = descriptor(&program(), &learned());
        assert!(!declared.capabilities.usage_tokens);
        assert_eq!(declared.capabilities.usage_context, UsageContext::None);
        assert_eq!(declared.check(), Ok(()));
    }

    #[test]
    fn the_plugins_usage_extra_turns_on_the_tokens_and_the_size_of_the_context() {
        let mut learned = learned();
        learned.extras = vec![Extra::Usage];
        let declared = descriptor(&program(), &learned);
        assert!(declared.capabilities.usage_tokens);
        assert_eq!(declared.capabilities.usage_context, UsageContext::SizeOnly);
    }

    #[test]
    fn a_program_known_to_send_usage_updates_reports_its_context_but_not_tokens_without_a_plugin() {
        let known = AcpProgram::from_json(&json!({
            "kind": "demo", "title": "Demo", "supported_range": "*", "reports_context": true
        }))
        .unwrap();
        let declared = descriptor(&known, &learned());
        assert_eq!(declared.capabilities.usage_context, UsageContext::SizeOnly);
        assert!(
            !declared.capabilities.usage_tokens,
            "the size of the context is not a count of tokens"
        );
        let shipped = crate::programs::dsh().unwrap();
        assert!(
            shipped.reports_context,
            "DeepSeek Harness sends usage_update without its plugin: seen in the real runs"
        );
    }
}
