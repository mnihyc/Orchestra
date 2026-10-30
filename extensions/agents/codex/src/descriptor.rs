//! Version support, arrangement types with their executor, a memory that is one store for a whole home and is therefore kept off, sessions held to the MCP servers they are given, and capabilities.
//!
//! The descriptor is what the adapter claims of the harness it found. It is built here from the version the
//! harness reported, and a claim is made only where the installed Codex does what the claim says (checked
//! against 0.165.0):
//!
//! * the exact pinned harness **0.165.0** is supported; newer versions are **untested** and older ones unsupported;
//! * it can run in the environment itself (`key_separated`, `together`) or with its provider, reaching the
//!   tools of the environment through its own executor (`separated`); it needs no copy of a credential to do
//!   so, which is why it declares neither `credential_shared` nor `handed_over`;
//! * its memory is one store for a whole home and is always off (`memory: home`);
//! * a session is held to the MCP servers it is given (`mcp.strict`), though a changed set reaches a session
//!   only when it is opened again (`mcp.reload` is not claimed);
//! * it reports no title of its own for a session, and does not let the adapter pin the models of the
//!   sub-agents it starts (`auxiliary_models: uncontrolled`).

use orch_agent_contract::descriptor::{
    AgentDescriptor, AgentTerminal, ApprovalMode, ApprovalReviewerSupport, ArrangementEntry,
    ArrangementType, AuxiliaryModels, Capabilities, EffortSupport, ExecutorBuild, HarnessInfo,
    McpSupport, MemoryKind, NativeProvider, OrchToolsSupport, PlanMechanics, PlanProceed,
    PlanProposal, ProviderFactKind, SandboxMode, SessionReload, SkillsSupport, UsageContext,
    classify_version,
};
use orch_agent_contract::events::HarnessRequestKind;
use orch_plugin_contract::Extensions;

use crate::{parameters, provider};

/// The harness kind this adapter drives.
pub const KIND: &str = "codex";

/// The version of the adapter's own declarations.
pub const ADAPTER_VERSION: u32 = 1;

/// The versions of the harness the adapter has been shown to drive, as a semver range. It is checked by
/// running the harness of each release in it.
pub const SUPPORTED_RANGE: &str = "=0.165.0";

/// What a subscription of it is called where one is added or listed, in English and in Chinese.
pub const PROVIDER_TITLE: &str = "Codex subscription";
/// The same, in Chinese.
const PROVIDER_TITLE_ZH: &str = "Codex 订阅";

/// The name of the executor the harness reaches its tools through when it runs with its provider.
pub const EXECUTOR: &str = "codex-exec-server";

/// The wire protocol the harness speaks to the forwarder.
pub const FORWARDED_PROTOCOL: &str = "openai-responses";

/// The version a harness reports of itself in the `userAgent` of its answer to `initialize`: the text after
/// the first `/`, up to the first space (`orch/0.162.1 (Debian 12.0.0; x86_64) …`).
#[must_use]
pub fn version_from_user_agent(user_agent: &str) -> Option<String> {
    let after = user_agent.split_once('/')?.1;
    let version = after.split_whitespace().next()?;
    semver::Version::parse(version.trim_start_matches('v'))
        .ok()
        .map(|parsed| parsed.to_string())
}

/// What the plans a sign-in can be on are called.
#[must_use]
pub fn plans() -> std::collections::BTreeMap<String, String> {
    [
        ("free", "Free"),
        ("go", "Go"),
        ("plus", "Plus"),
        ("pro", "Pro"),
        ("prolite", "Pro Lite"),
        ("promax", "Pro Max"),
        ("team", "Team"),
        ("business", "Business"),
        ("self_serve_business_prolite", "Business (Pro Lite)"),
        ("self_serve_business_usage_based", "Business (usage based)"),
        ("enterprise", "Enterprise"),
        ("edu", "Edu"),
        ("edu_plus", "Edu Plus"),
        ("edu_pro", "Edu Pro"),
    ]
    .into_iter()
    .map(|(id, name)| (id.to_owned(), name.to_owned()))
    .collect()
}

/// The capabilities the adapter claims.
#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        steer: true,
        interrupt: true,
        background_commands: true,
        approvals: vec![
            HarnessRequestKind::Command,
            HarnessRequestKind::FileChange,
            HarnessRequestKind::Permissions,
            HarnessRequestKind::UserInput,
            HarnessRequestKind::McpElicitation,
        ],
        subagents: true,
        subagent_limits: vec![
            orch_agent_contract::descriptor::SubagentControl::AtOnce,
            orch_agent_contract::descriptor::SubagentControl::Depth,
        ],
        plan: true,
        // A turn in collaboration mode `plan`, read-only for that turn: its `plan` item is the plan to proceed with.
        plan_mode: true,
        // The plan is an item of the turn; Proceed sends the harness a message.
        plan_mechanics: Some(PlanMechanics {
            proposal: PlanProposal::Item,
            proceed: PlanProceed::Message,
        }),
        reasoning_summary: true,
        input_images: true,
        effort: EffortSupport::Turn,
        mcp_discovery: true,
        mcp: McpSupport {
            stdio: true,
            http: true,
            reload: false,
            strict: true,
        },
        orch_tools: OrchToolsSupport::Mcp,
        // A thread's own history stays in its home, and no file of it is made: a thread resumes and forks natively
        // where the home is the same or comes along, and goes into another home as the newest turns told to a new
        // thread. So nothing is exported, and no transplant exists for a plan to choose. `import_native` is turned on
        // only by a complete build check, which shows that this build takes a thread over natively in those two cases.
        export_native: false,
        import_native: false,
        import_neutral: true,
        fork_native: true,
        // Exact checkpoints remain unavailable until goal-state and item-history checks pass.
        native_input_prefix: None,
        resume: true,
        discover_sessions: true,
        usage_tokens: true,
        // The size of the last prompt against the window, and no breakdown by part.
        usage_context: UsageContext::SizeOnly,
        phases: true,
        provider_facts: vec![
            ProviderFactKind::Plan,
            ProviderFactKind::Windows,
            ProviderFactKind::LimitReached,
            ProviderFactKind::OrdinaryUsageAllowed,
            ProviderFactKind::PlanCredits,
            ProviderFactKind::AvailableResets,
            ProviderFactKind::CredentialVerification,
        ],
        model_discovery: true,
        title: false,
        auxiliary_models: AuxiliaryModels::Uncontrolled,
        telemetry_off: true,
        memory: MemoryKind::Home,
        agent_terminal: AgentTerminal::None,
        // This pin caches the files of a directory while a session is open; a native resume reloads them.
        session_reload: SessionReload::Turn,
        // The harness looks up the tree for its instruction files; Orch's marker stops it at the workspace root.
        instruction_root_marker: true,
        // The adapter hands it the root that holds a thread's skills.
        skills: SkillsSupport::Directory,
        // The pinned build's reviewer, until a native review reports a model of its own; it can be used only by
        // someone who may use that model.
        approval_reviewer: Some(ApprovalReviewerSupport {
            model: crate::review::CODEX_REVIEW_MODEL.to_owned(),
            settable: false,
        }),
    }
}

/// The ways the harness can be placed, in the adapter's order of preference.
#[must_use]
pub fn arrangements() -> Vec<ArrangementEntry> {
    let plain = |kind| ArrangementEntry {
        kind,
        executor: None,
        executor_version: None,
        tool_sets: None,
    };
    vec![
        ArrangementEntry {
            kind: ArrangementType::Separated,
            executor: Some(EXECUTOR.to_owned()),
            executor_version: Some(ExecutorBuild::Same),
            tool_sets: None,
        },
        plain(ArrangementType::KeySeparated),
        plain(ArrangementType::Together),
    ]
}

/// Everything the adapter declares of a harness that reported `version`.
#[must_use]
pub fn descriptor(version: &str) -> AgentDescriptor {
    AgentDescriptor {
        kind: KIND.to_owned(),
        adapter_version: ADAPTER_VERSION,
        harness: HarnessInfo {
            name: manifest().name().to_owned(),
            version: version.to_owned(),
            supported_range: SUPPORTED_RANGE.to_owned(),
            status: classify_version(version, SUPPORTED_RANGE, true),
        },
        arrangements: arrangements(),
        native_provider: Some(NativeProvider {
            harness_kind: KIND.to_owned(),
            id: None,
            title: PROVIDER_TITLE.to_owned(),
            plans: plans(),
            login_methods: provider::login_methods(),
            options_schema: provider::options_schema(),
            arrangements: vec![ArrangementType::Separated],
        }),
        native_providers: Vec::new(),
        forwarded_protocols: vec![FORWARDED_PROTOCOL.to_owned()],
        direct_protocols: Vec::new(),
        credentials: orch_agent_contract::credentials::CredentialSupport::default(),
        runtime_template: None,
        approval_modes: vec![
            ApprovalMode::Ask,
            ApprovalMode::OnRequest,
            ApprovalMode::Bypass,
        ]
        .into_iter()
        // Auto only where the manifest has the conformance case that proves its reviewer on a build.
        .chain(auto_proven().then_some(ApprovalMode::Auto))
        .collect(),
        sandbox_modes: vec![
            SandboxMode::ReadOnly,
            SandboxMode::Workspace,
            SandboxMode::Open,
        ],
        // A side conversation's reading boundary is offered only once its conformance cases have proved it
        // on the pinned build and loaded configuration; the app server accepting a read-only selection
        // proves no boundary. Until then an aside is refused with why, never opened with writable tools.
        side_thread_modes: Vec::new(),
        capabilities: capabilities(),
        parameters: parameters::definitions(),
        text: text(),
        extensions: Extensions::new(),
    }
}

/// The manifest of this program, which holds what is true of it however it is built.
#[must_use]
pub fn manifest() -> &'static orch_agent_programs::ProgramManifest {
    orch_agent_programs::shipped(KIND)
}

/// The feature a conformance case proves for Auto.
const AUTO_FEATURE: &str = "approval_modes.auto";

/// Whether the manifest has the conformance case that proves Auto's reviewer on a build. Without it Auto is not
/// offered, whatever else says so.
#[must_use]
pub fn auto_proven() -> bool {
    manifest().proves(AUTO_FEATURE)
}

/// The words of the descriptor in the languages other than English: the program's name, its subscription and
/// plans, its ways of signing in and its parameters.
fn text() -> std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> {
    let mut words = manifest().name_text();
    let chinese = words.entry("zh-CN".to_owned()).or_default();
    for (place, text) in [
        ("native_provider.title", PROVIDER_TITLE_ZH),
        ("windows.weekly", "每周"),
        ("windows.5h", "5 小时"),
        ("windows.window", "时段"),
        ("native_provider.plans.free", "免费版"),
        ("native_provider.plans.team", "团队版"),
        ("native_provider.plans.business", "商业版"),
        (
            "native_provider.plans.self_serve_business_prolite",
            "商业版（Pro Lite）",
        ),
        (
            "native_provider.plans.self_serve_business_usage_based",
            "商业版（按用量）",
        ),
        ("native_provider.plans.enterprise", "企业版"),
        ("native_provider.plans.edu", "教育版"),
        ("native_provider.plans.edu_plus", "教育版 Plus"),
        ("native_provider.plans.edu_pro", "教育版 Pro"),
        (
            "native_provider.login_methods.device_code.title",
            "使用代码登录",
        ),
        (
            "native_provider.login_methods.device_code.summary",
            "Orch 会显示一个短代码，你在厂商的登录页面输入它。",
        ),
        (
            "native_provider.login_methods.device_code.requires",
            "需要在 ChatGPT 的安全设置中，或由工作区管理员，开启设备代码登录。",
        ),
        (
            "native_provider.login_methods.auth_file.title",
            "导入 auth.json",
        ),
        (
            "native_provider.login_methods.auth_file.summary",
            "复制一份已有的登录信息，此后这份副本由这里续期。一份副本只在一处使用：若同一个文件在别处继续使用，两处中先续期的一方会让另一方无法续期。在别处保持有效的登录信息，同样用这个方法重新复制进来。",
        ),
        (
            "native_provider.login_methods.auth_file.fields.auth_json.label",
            "auth.json 的内容",
        ),
        (
            "native_provider.login_methods.auth_file.fields.auth_json.help",
            "粘贴登录命令把登录信息保存在文件里时写下的文件。这一项和下面的路径二选一，不要同时给。",
        ),
        (
            "native_provider.login_methods.auth_file.fields.auth_path.label",
            "运行环境中 auth.json 的路径",
        ),
        (
            "native_provider.login_methods.auth_file.fields.auth_path.help",
            "适用于镜像或模板自带该文件的运行环境：填写它在其中的路径。这一项和上面的内容二选一，不要同时给。",
        ),
        (
            "native_provider.login_methods.access_token.title",
            "使用访问令牌",
        ),
        (
            "native_provider.login_methods.access_token.summary",
            "接受工作区令牌，或 ChatGPT 登录的访问令牌。Orch 从不续期：请在它过期前换上新令牌。它所来自的登录在原处继续有效。",
        ),
        (
            "native_provider.login_methods.access_token.requires",
            "工作区令牌需要工作区管理员允许。对 ChatGPT，只使用现有登录的访问令牌，绝不要使用它的刷新令牌。",
        ),
        (
            "native_provider.login_methods.access_token.fields.access_token.label",
            "访问令牌",
        ),
        (
            "native_provider.login_methods.access_token.fields.access_token.help",
            "使用工作区管理员签发的令牌，或现有 ChatGPT 登录的 auth.json 里 tokens 对象中的 access_token。只带访问令牌。Orch 从不续期它，它所来自的登录在原处继续有效。",
        ),
        (
            "native_provider.login_methods.access_token.fields.expires_at.label",
            "有效期至",
        ),
        (
            "native_provider.login_methods.access_token.fields.expires_at.help",
            "令牌失效的日期（如果知道）；到期前会发出通知。",
        ),
        ("parameters.service_tier.title", "速度"),
        (
            "parameters.service_tier.description",
            "模型的更快模式，会更快用完套餐。只提供提供方的管理者为该模型启用的速度。",
        ),
        ("parameters.context.title", "上下文"),
        (
            "parameters.context.description",
            "代理保留多少对话（以令牌计），最多到模型的管理者开放的最大值。对话达到该大小的十分之九时会被缩短（上下文为 300 000 时在 270 000），并且不会超过目录允许该模型使用的大小。",
        ),
        ("parameters.reasoning_summary.title", "推理摘要"),
        (
            "parameters.reasoning_summary.description",
            "代理在工作时总结多少自己的推理。",
        ),
        ("parameters.reasoning_summary.options.auto.label", "自动"),
        (
            "parameters.reasoning_summary.options.auto.help",
            "由模型决定说多少。",
        ),
        ("parameters.reasoning_summary.options.concise.label", "简短"),
        (
            "parameters.reasoning_summary.options.concise.help",
            "一段简短的摘要。",
        ),
        (
            "parameters.reasoning_summary.options.detailed.label",
            "详细",
        ),
        (
            "parameters.reasoning_summary.options.detailed.help",
            "对推理更完整的说明。",
        ),
        ("parameters.reasoning_summary.options.none.label", "无"),
        (
            "parameters.reasoning_summary.options.none.help",
            "不给摘要。",
        ),
        ("parameters.cyber_access_program.title", "访问计划"),
        (
            "parameters.cyber_access_program.description",
            "厂商授予账户的、用于安全工作的授权计划。只提供提供方的管理者为该模型启用的计划。",
        ),
    ] {
        chinese.insert(place.to_owned(), text.to_owned());
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::descriptor::HarnessSupport;

    #[test]
    fn a_codex_thread_going_into_another_home_is_never_carried_as_a_file() {
        use orch_agent_contract::history::{
            HistoryMethod, HistoryMode, HistoryRequest, choose_method,
        };
        let request = HistoryRequest {
            mode: HistoryMode::All,
            is_switch: true,
            same_harness_process: false,
            from_first_turn: true,
            same_kind: true,
            source_reachable: true,
        };
        for version in ["0.162.1", "0.165.0", "0.170.0"] {
            let mut target = descriptor(version).capabilities;
            assert!(
                !target.export_native,
                "no file of a thread's history is made"
            );
            // Whatever a build check proves of the native cases, the plan has no transplant to choose.
            for proved in [false, true] {
                target.import_native = proved;
                assert_eq!(
                    choose_method(&request, &capabilities(), &target, &[]),
                    HistoryMethod::NeutralImport
                );
            }
            target.import_neutral = false;
            assert_eq!(
                choose_method(&request, &capabilities(), &target, &[]),
                HistoryMethod::FileHandoff
            );
        }
    }

    #[test]
    fn the_source_declares_the_limit_and_usage_permission_the_harness_already_reports() {
        let facts = capabilities().provider_facts;
        assert!(facts.contains(&ProviderFactKind::LimitReached));
        assert!(facts.contains(&ProviderFactKind::OrdinaryUsageAllowed));
    }

    #[test]
    fn the_version_is_read_from_the_user_agent_the_harness_reports() {
        assert_eq!(
            version_from_user_agent("orch/0.162.1 (Debian 12.0.0; x86_64) unknown (orch; 0.1.0)")
                .as_deref(),
            Some("0.162.1")
        );
        assert_eq!(
            version_from_user_agent("Codex Desktop/0.161.0-alpha.2 (Mac OS 15; arm64)").as_deref(),
            Some("0.161.0-alpha.2")
        );
        assert_eq!(version_from_user_agent("something odd"), None);
        assert_eq!(version_from_user_agent("x/not-a-version (y)"), None);
    }

    #[test]
    fn a_version_inside_the_range_is_supported_a_newer_one_is_untested_and_an_older_one_is_not_driven()
     {
        assert_eq!(
            descriptor("0.160.0").harness.status,
            HarnessSupport::Unsupported
        );
        assert_eq!(
            descriptor("0.162.1").harness.status,
            HarnessSupport::Unsupported
        );
        assert_eq!(
            descriptor("0.165.0").harness.status,
            HarnessSupport::Supported
        );
        assert_eq!(
            descriptor("0.166.0").harness.status,
            HarnessSupport::Untested
        );
        assert_eq!(
            descriptor("0.159.9").harness.status,
            HarnessSupport::Unsupported
        );
    }

    #[test]
    fn what_the_descriptor_declares_holds_together() {
        let declared = descriptor("0.165.0");
        assert_eq!(declared.check(), Ok(()));
        assert_eq!(
            declared.forwarded_protocols,
            ["openai-responses"],
            "Chat Completions is not established for the pinned harness configuration"
        );
        assert!(
            declared.supports(ArrangementType::Separated)
                && declared.supports(ArrangementType::KeySeparated)
                && declared.supports(ArrangementType::Together)
        );
        assert!(
            !declared.supports(ArrangementType::CredentialShared)
                && !declared.supports(ArrangementType::HandedOver),
            "a harness that can be separated from its tools needs neither copy of a credential"
        );
        let first = &declared.arrangements[0];
        assert_eq!(first.executor.as_deref(), Some(EXECUTOR));
        assert_eq!(first.executor_version, Some(ExecutorBuild::Same));
    }

    #[test]
    fn a_capability_the_installed_harness_does_not_have_is_absent_and_not_a_false_promise() {
        let claimed = capabilities();
        assert!(
            !claimed.title,
            "Codex names a thread only when it is told to"
        );
        assert!(
            !claimed.mcp.reload,
            "a changed set of servers needs the session to be opened again"
        );
        assert_eq!(claimed.auxiliary_models, AuxiliaryModels::Uncontrolled);
        assert_eq!(claimed.memory, MemoryKind::Home);
        assert!(claimed.mcp.strict && claimed.telemetry_off);
        assert!(
            descriptor("0.165.0")
                .capabilities
                .native_input_prefix
                .is_none(),
            "exact input checkpoints stay unavailable until their native checks pass"
        );
    }

    #[test]
    fn the_descriptor_reads_and_writes_as_json_without_loss() {
        let declared = descriptor("0.165.0");
        let written = serde_json::to_value(&declared).unwrap();
        let read: AgentDescriptor = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(read, declared);
        assert_eq!(written["harness"]["status"], "supported");
        assert_eq!(
            written["native_provider"]["login_methods"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn every_word_of_the_descriptor_that_a_person_reads_has_its_chinese_text() {
        let declared = descriptor("0.162.1");
        let said = |place: String| {
            assert!(
                declared.text_in("zh-CN", &place).is_some(),
                "no Chinese text at {place}"
            );
        };
        said("harness.name".to_owned());
        said("native_provider.title".to_owned());
        for plan in declared
            .native_provider
            .iter()
            .flat_map(|native| native.plans.keys())
        {
            // A plan whose name is the same word in both languages needs no translation.
            let english = &declared.native_provider.as_ref().unwrap().plans[plan];
            if english.chars().any(|c| c.is_ascii_lowercase())
                && !["Pro", "Max", "Plus", "Go"].contains(&english.as_str())
                && !english.starts_with("Pro ")
            {
                said(format!("native_provider.plans.{plan}"));
            }
        }
        for method in declared
            .native_provider
            .iter()
            .flat_map(|native| &native.login_methods)
        {
            said(format!("native_provider.login_methods.{}.title", method.id));
            said(format!(
                "native_provider.login_methods.{}.summary",
                method.id
            ));
            if method.requires.is_some() {
                said(format!(
                    "native_provider.login_methods.{}.requires",
                    method.id
                ));
            }
            for field in &method.fields {
                said(format!(
                    "native_provider.login_methods.{}.fields.{}.label",
                    method.id, field.name
                ));
                if field.help.is_some() {
                    said(format!(
                        "native_provider.login_methods.{}.fields.{}.help",
                        method.id, field.name
                    ));
                }
            }
        }
        for parameter in &declared.parameters {
            said(format!("parameters.{}.title", parameter.key));
            said(format!("parameters.{}.description", parameter.key));
        }
    }
}
