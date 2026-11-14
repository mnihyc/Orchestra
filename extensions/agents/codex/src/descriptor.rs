//! Version support, arrangement types with their executor, a memory that is one store for a whole home and is therefore kept off, sessions held to the MCP servers they are given, and capabilities.
//!
//! The descriptor is what the adapter claims of the harness it found. It is built here from the version the
//! harness reported, and a claim is made only where the installed Codex does what the claim says (checked
//! against the shipped 0.167.2 interface by the release conformance suite):
//!
//! * the exact pinned harness **0.167.2** is supported; newer versions are **untested** and older ones unsupported;
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

use crate::{parameters, provider};

/// The harness kind this adapter drives.
pub const KIND: &str = "codex";

/// The version of the adapter's own declarations.
pub const ADAPTER_VERSION: u32 = 1;

/// The builds the adapter says it supports: the line (the same major and minor version) of the oldest build the suite
/// was proven on, from that build upward. A build of a later line is started and called untested; one older than
/// [`OLDEST_PROVEN_BUILD`] does not start. This is the status shown to people; the gates below do not follow it.
pub const SUPPORTED_RANGE: &str = ">=0.167.0, <0.168.0";

/// The oldest build the suite was proven on (0.167.0 and 0.167.2 both had it; their app-server protocols are byte for
/// byte the same). Nothing that a new release alone could switch back on is withheld from this build or any later one.
pub const OLDEST_PROVEN_BUILD: &str = "0.167.0";

/// The build Orch installs itself and the newest one the suite was proven on.
pub const SHIPPED_BUILD: &str = "0.167.2";

/// Whether the gates that follow the native interface (ending a background command, the proof of web actions, the
/// steer id, the exact checkpoints) are open for `version`: for every build at or above [`OLDEST_PROVEN_BUILD`],
/// compared as versions, with no upper bound. A later build that has really changed a method fails at the use of the
/// act, with the program's own error, not beforehand. A number that cannot be read is not at or above it.
pub(crate) fn is_pinned_build(version: &str) -> bool {
    match (
        semver::Version::parse(version),
        semver::Version::parse(OLDEST_PROVEN_BUILD),
    ) {
        (Ok(version), Ok(oldest)) => version >= oldest,
        _ => false,
    }
}

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
        ("pro", "Pro (More)"),
        ("prolite", "Pro"),
        ("promax", "Pro (Max)"),
        ("team", "Team"),
        ("business", "Business"),
        ("self_serve_business_prolite", "Business (Pro Lite)"),
        ("self_serve_business_usage_based", "Business (usage based)"),
        ("ent26", "Enterprise"),
        ("enterprise_cbp_automation", "Enterprise (Automation)"),
        ("enterprise_cbp_usage_based", "Enterprise CBP Usage Based"),
        ("enterprise", "Enterprise"),
        ("edu", "Edu"),
        ("edu_plus", "Edu Plus"),
        ("edu_pro", "Edu Pro"),
        ("sci", "Academic Researchers"),
        ("sci_plus", "Academic Researchers Plus"),
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
        background_commands: false,
        prompt_cache_ttl: false,
        turn_changes_require_restart: false,
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
        image_generation: true,
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
        provider_fact_fields: orch_provider_contract::credit_fields::normalize_reports(
            [(
                "balances.plan".to_owned(),
                ["remaining", "unlimited", "unit"]
                    .map(str::to_owned)
                    .to_vec(),
            )]
            .into(),
        ),
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
        // A sign-in uses its preferred reviewer. A forwarded catalogue contains only its admitted
        // session offering, so native review falls back to that model under the same Use policy.
        approval_reviewer: Some(ApprovalReviewerSupport {
            model: crate::review::CODEX_REVIEW_MODEL.to_owned(),
            settable: false,
            session_model_paths: vec![orch_agent_contract::credentials::CredentialPath::Forwarded],
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
        capabilities: Capabilities {
            background_commands: crate::native_schema::pinned_background_control(version),
            ..capabilities()
        },
        parameters: parameters::definitions(),
        text: text(),
        extensions: serde_json::from_value(serde_json::json!({
            "x_capability_program": serde_json::from_str::<orch_types::Json>(include_str!("capability-program.json")).unwrap_or(orch_types::Json::Null),
            "x_profile_parameters": crate::parameters::profile_parameters(),
            "x_turn_end_delays": {"version":1,"delays":[{
                "pointer":"/native_result_wait_ms", "also_ms":0,
                "default_ms":crate::runtime::DEFAULT_NATIVE_RESULT_WAIT_MS,
                "default_setting":"agents.native_result_wait_ms"
            }]}
        })).unwrap_or_default(),
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
        ("native_provider.plans.pro", "Pro（更多）"),
        ("native_provider.plans.promax", "Pro（Max）"),
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
        ("native_provider.plans.ent26", "企业版"),
        (
            "native_provider.plans.enterprise_cbp_automation",
            "企业版（自动化）",
        ),
        (
            "native_provider.plans.enterprise_cbp_usage_based",
            "企业 CBP（按用量）",
        ),
        ("native_provider.plans.edu", "教育版"),
        ("native_provider.plans.edu_plus", "教育版 Plus"),
        ("native_provider.plans.edu_pro", "教育版 Pro"),
        ("native_provider.plans.sci", "学术研究人员"),
        ("native_provider.plans.sci_plus", "学术研究人员 Plus"),
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
        ("parameters.web_search.title", "网络搜索"),
        (
            "parameters.web_search.description",
            "选择原生网络搜索，或在可用时选择已启用的托管提供方服务。搜索可能另行计费。",
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
    fn native_profile_aliases_survive_the_wire_and_cannot_be_removed_by_profile_overrides()
    -> Result<(), Box<dyn std::error::Error>> {
        use orch_agent_contract::parameters::ModelParameters;
        use orch_agent_contract::profile_parameters::{EXTENSION, ProfileParameters};
        use serde_json::json;
        let actual = descriptor("0.167.2");
        let read: AgentDescriptor = serde_json::from_value(serde_json::to_value(&actual)?)?;
        assert_eq!(
            read.extensions.get(EXTENSION),
            actual.extensions.get(EXTENSION)
        );
        let overridden = read.with_overrides(&json!({(EXTENSION):null}))?;
        assert_eq!(
            overridden.extensions.get(EXTENSION),
            actual.extensions.get(EXTENSION)
        );
        let declared: ProfileParameters = serde_json::from_value(
            overridden
                .extensions
                .get(EXTENSION)
                .ok_or("declared native profile parameters")?
                .clone(),
        )?;
        let model: ModelParameters = serde_json::from_value(
            json!({"context_max":64000,"efforts":["low"],
            "service_tiers":[{"id":"fast","label":"Fast"}], "web_search":["indexed"],
            "request_options":{"native":{"type":"object", "properties":{"service_tier":{"const":"fast"}}}}}),
        )?;
        let config = json!({"service_tier":"priority", "model_context_window":32000,
            "model_reasoning_summary":"concise", "model_reasoning_effort":"low", "web_search":"indexed", "other_option":true});
        let values = declared.validate(&config, &model, &std::collections::BTreeMap::new())?;
        assert_eq!(values.get("service_tier"), Some(&json!("fast")));
        assert_eq!(values.get("context"), Some(&json!(32000)));
        assert_eq!(values.get("effort"), Some(&json!("low")));
        for change in [
            json!({"model_context_window":128000}),
            json!({"model_reasoning_effort":"high"}),
            json!({"service_tier":"flex"}),
            json!({"web_search":"live"}),
        ] {
            let mut invalid = config.clone();
            let changes = change.as_object().ok_or("a config change")?;
            let into = invalid.as_object_mut().ok_or("a native config object")?;
            into.extend(changes.clone());
            assert!(
                declared
                    .validate(&invalid, &model, &std::collections::BTreeMap::new())
                    .is_err()
            );
        }
        Ok(())
    }

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
            descriptor("0.166.9").harness.status,
            HarnessSupport::Unsupported
        );
        for version in ["0.167.0", "0.167.2", "0.167.3"] {
            assert_eq!(
                descriptor(version).harness.status,
                HarnessSupport::Supported,
                "{version}"
            );
            assert!(is_pinned_build(version), "{version}");
            assert!(
                descriptor(version).capabilities.background_commands,
                "{version}: a later patch of the line ends background commands"
            );
        }
        for version in ["0.168.0", "0.170.2", "1.0.0"] {
            assert!(is_pinned_build(version), "{version}");
            assert!(
                descriptor(version).capabilities.background_commands,
                "{version}: a later line keeps the acts"
            );
        }
        assert!(!is_pinned_build("0.166.9") && !is_pinned_build("not a version"));
        assert!(!descriptor("0.166.9").capabilities.background_commands);
        assert_eq!(
            descriptor("0.168.0").harness.status,
            HarnessSupport::Untested
        );
        assert_eq!(
            descriptor("0.159.9").harness.status,
            HarnessSupport::Unsupported
        );
    }

    #[test]
    fn what_the_descriptor_declares_holds_together() {
        let declared = descriptor("0.167.2");
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
            descriptor("0.167.2")
                .capabilities
                .native_input_prefix
                .is_none(),
            "exact input checkpoints stay unavailable until their native checks pass"
        );
    }

    #[test]
    fn the_descriptor_reads_and_writes_as_json_without_loss() {
        let declared = descriptor("0.167.2");
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
    fn image_eligibility_uses_the_shared_resolver_and_reported_subscription_facts() {
        use orch_provider_contract::capabilities::{
            self as cap, Availability, Declarations, Offering, OfferingRef, Provider, Registry,
            ThreadContext,
        };
        use serde_json::json;
        let original = descriptor("0.167.2");
        let descriptor: AgentDescriptor =
            serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
        assert_eq!(
            descriptor.capability_program(),
            original.capability_program()
        );
        let provider = orch_types::new_id();
        let mut registry = Registry::default();
        registry
            .register("common", cap::catalogue().unwrap())
            .unwrap();
        let mut declarations = Declarations::default();
        declarations
            .programs
            .insert(KIND.into(), descriptor.capability_program());
        declarations.providers.insert(
            provider,
            Provider {
                offerings: [(
                    "model".into(),
                    Offering {
                        enabled: true,
                        ..Offering::default()
                    },
                )]
                .into(),
                ..Provider::default()
            },
        );
        registry.register("program", declarations).unwrap();
        let base = ThreadContext {
            program: KIND.into(),
            access: "native".into(),
            current: Some(OfferingRef {
                provider,
                offering: "model".into(),
            }),
            availability: [(
                provider,
                [(
                    "model".into(),
                    Availability {
                        allowed: true,
                        ..Availability::default()
                    },
                )]
                .into(),
            )]
            .into(),
            ..ThreadContext::default()
        };
        for (facts, available) in [
            (
                json!({"plan":"pro","credential":{"yields":"login"},"credential_verified":true}),
                true,
            ),
            (
                json!({"plan":"plus","credential":{"yields":"token"},"credential_verified":true}),
                true,
            ),
            (
                json!({"plan":"free","credential":{"yields":"login"},"credential_verified":true}),
                false,
            ),
            (
                json!({"plan":"pro","credential":{"yields":"key"},"credential_verified":true}),
                false,
            ),
            (
                json!({"plan":null,"credential":{"yields":"login"},"credential_verified":true}),
                false,
            ),
            (
                json!({"plan":"","credential":{"yields":"login"},"credential_verified":true}),
                false,
            ),
            (
                json!({"plan":"pro","credential":null,"credential_verified":true}),
                false,
            ),
            (
                json!({"plan":"pro","credential":{"yields":"login"},"credential_verified":false}),
                false,
            ),
            (json!({"plan":"pro","credential":{"yields":"login"}}), false),
        ] {
            let mut context = base.clone();
            context.facts = facts.clone();
            let result = registry
                .resolve(&context)
                .into_iter()
                .find(|row| row.capability == "image_generation")
                .unwrap();
            assert_eq!(result.route.is_some(), available, "{facts}");
            assert_eq!(result.modes.iter().any(|mode| mode == "on"), available);
            assert!(!result.modes.iter().any(|mode| mode == "service"));
            if let Some(route) = result.route {
                assert_eq!(route.kind, "native-program");
                assert!(
                    route.native_value.is_null(),
                    "unset preserves the native default"
                );
            } else {
                assert_eq!(
                    result.detail.as_deref(),
                    Some("native_image_sign_in_required")
                );
            }
            let mut forwarded = context.clone();
            forwarded.access = "forwarded".into();
            let result = registry
                .resolve(&forwarded)
                .into_iter()
                .find(|row| row.capability == "image_generation")
                .unwrap();
            assert!(result.route.is_none());
            assert_eq!(
                result.detail.as_deref(),
                Some("native_image_sign_in_required")
            );
        }
        let mut denied = base;
        denied
            .availability
            .get_mut(&provider)
            .unwrap()
            .get_mut("model")
            .unwrap()
            .allowed = false;
        let result = registry
            .resolve(&denied)
            .into_iter()
            .find(|row| row.capability == "image_generation")
            .unwrap();
        assert_eq!(result.reason.as_deref(), Some("current_authority_refused"));
        assert!(result.detail.is_none());
        denied
            .choices
            .insert("image_generation".into(), Some("off".into()));
        let off = registry
            .resolve(&denied)
            .into_iter()
            .find(|row| row.capability == "image_generation")
            .unwrap();
        assert_eq!(off.reason.as_deref(), Some("disabled"));
        assert!(off.detail.is_none() && off.route.is_none());
    }

    #[test]
    fn newly_reported_native_plans_keep_their_ids_and_the_names_of_the_program() {
        let declared = descriptor("0.167.2");
        let native = declared.native_provider.as_ref().unwrap();
        for (id, english, chinese) in [
            ("pro", "Pro (More)", "Pro（更多）"),
            ("promax", "Pro (Max)", "Pro（Max）"),
            ("ent26", "Enterprise", "企业版"),
            (
                "enterprise_cbp_automation",
                "Enterprise (Automation)",
                "企业版（自动化）",
            ),
            (
                "enterprise_cbp_usage_based",
                "Enterprise CBP Usage Based",
                "企业 CBP（按用量）",
            ),
            ("sci", "Academic Researchers", "学术研究人员"),
            ("sci_plus", "Academic Researchers Plus", "学术研究人员 Plus"),
        ] {
            assert_eq!(native.plans.get(id).map(String::as_str), Some(english));
            assert_eq!(
                declared.text_in("zh-CN", &format!("native_provider.plans.{id}")),
                Some(chinese)
            );
        }
        assert_eq!(native.plans.get("prolite").map(String::as_str), Some("Pro"));
        assert_eq!(native.plans.len(), 19);
        let read: AgentDescriptor =
            serde_json::from_value(serde_json::to_value(&declared).unwrap()).unwrap();
        assert_eq!(read.native_provider.as_ref().unwrap().plans, native.plans);
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

#[cfg(test)]
mod model_metadata_tests {
    use super::*;
    use orch_provider_contract::capabilities::{
        self as cap, Availability, Declarations, Endpoint, NativeSwitch, Offering, OfferingRef,
        Provider, Registry, ThreadContext,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn image_viewing_follows_the_admitted_native_route_and_its_refusals()
    -> Result<(), Box<dyn std::error::Error>> {
        for (access, images, allowed, choice, expected) in [
            ("forwarded", Some(true), true, None, true),
            ("forwarded", Some(false), true, None, false),
            ("forwarded", None, true, None, false),
            ("native", Some(true), true, None, true),
            ("native", Some(false), true, None, false),
            ("native", None, true, None, true),
            ("forwarded", Some(true), false, None, false),
            ("forwarded", Some(true), true, Some("off"), false),
            ("forwarded", Some(true), true, Some("unknown"), false),
            ("forwarded", Some(true), true, Some("service"), false),
        ] {
            let provider = orch_types::new_id();
            let mut registry = Registry::default();
            registry.register("common", cap::catalogue()?)?;
            let mut declarations = Declarations::default();
            declarations
                .programs
                .insert(KIND.into(), descriptor("0.167.2").capability_program());
            declarations.providers.insert(
                provider,
                Provider {
                    endpoints: BTreeMap::from([(
                        "generation".into(),
                        Endpoint {
                            interface: "openai-responses".into(),
                            address: "https://endpoint.example.invalid/v1".into(),
                            ..Endpoint::default()
                        },
                    )]),
                    offerings: BTreeMap::from([(
                        "model".into(),
                        Offering {
                            enabled: true,
                            endpoint: "generation".into(),
                            kind: "model".into(),
                            parameters: images
                                .map_or_else(|| json!({}), |images| json!({"input_images":images})),
                            ..Offering::default()
                        },
                    )]),
                    ..Provider::default()
                },
            );
            registry.register("fixture", declarations)?;
            let context = ThreadContext {
                program: KIND.into(),
                access: access.into(),
                current: Some(OfferingRef {
                    provider,
                    offering: "model".into(),
                }),
                choices: choice
                    .into_iter()
                    .map(|mode| ("image_input".into(), Some(mode.into())))
                    .collect(),
                availability: BTreeMap::from([(
                    provider,
                    BTreeMap::from([(
                        "model".into(),
                        Availability {
                            allowed,
                            ..Availability::default()
                        },
                    )]),
                )]),
                interfaces: vec!["openai-responses".into()],
                ..ThreadContext::default()
            };
            let resolution = registry
                .resolve(&context)
                .into_iter()
                .find(|row| row.capability == "image_input")
                .ok_or("the shared registry did not resolve image input")?;
            assert_eq!(
                resolution.route.is_some(),
                expected,
                "{access} images={images:?} allowed={allowed} choice={choice:?}"
            );
            assert_eq!(
                resolution.native_config.get("features.view_image"),
                Some(&json!(expected))
            );
            let wire = serde_json::to_value(&resolution)?;
            assert_eq!(wire["native_config"]["features.view_image"], expected);
            assert!(
                wire.get("route").is_none(),
                "only public flags enter the native carrier"
            );
        }
        Ok(())
    }

    #[test]
    fn standalone_search_requirements_use_declared_metadata_and_keep_older_entries_unchanged()
    -> Result<(), Box<dyn std::error::Error>> {
        for legacy in [false, true] {
            for required in [false, true] {
                for supported in [false, true] {
                    let provider = orch_types::new_id();
                    let mut registry = Registry::default();
                    registry.register("common", cap::catalogue()?)?;
                    let mut offering = Offering {
                        enabled: true,
                        endpoint: "generation".into(),
                        kind: "model".into(),
                        parameters: json!({"web_search":["live"]}),
                        ..Offering::default()
                    };
                    if legacy {
                        offering.parameters["requires_standalone_search"] = json!(required);
                    } else {
                        offering.extensions.insert(
                            "model_metadata".into(),
                            json!({"codex":{"requires_standalone_search":required}}),
                        );
                    }
                    if supported {
                        let interfaces = cap::protocol_interfaces()?;
                        let mut binding = interfaces
                            .get("openai-responses")
                            .and_then(|interface| interface.native_tools.get("web_search"))
                            .cloned()
                            .ok_or("the test needs its declared native search interface")?;
                        binding.request.as_object_mut().ok_or("the native request declaration must be an object")?
                            .insert("forms".into(),json!([{
                                "paths":["/alpha/search"],
                                "native_session":{"enabled_setting":"supports_standalone_web_search", "owned_fields":["model","query"],"parameter_fields":[]}
                            }]));
                        offering
                            .native_tools
                            .insert("web_search".into(), Some(binding));
                        offering.native_switches.insert(
                            KIND.into(),
                            BTreeMap::from([(
                                "supports_standalone_web_search".into(),
                                NativeSwitch {
                                    capability: "web_search".into(),
                                    value: Some(json!(true)),
                                    ..NativeSwitch::default()
                                },
                            )]),
                        );
                    }
                    let mut declarations = Declarations::default();
                    declarations
                        .programs
                        .insert(KIND.into(), descriptor("0.167.2").capability_program());
                    declarations.providers.insert(
                        provider,
                        Provider {
                            endpoints: BTreeMap::from([(
                                "generation".into(),
                                Endpoint {
                                    interface: "openai-responses".into(),
                                    address: "https://endpoint.example.invalid/v1".into(),
                                    ..Endpoint::default()
                                },
                            )]),
                            offerings: BTreeMap::from([("model".into(), offering)]),
                            ..Provider::default()
                        },
                    );
                    registry.register("fixture", declarations)?;
                    let context = ThreadContext {
                        program: KIND.into(),
                        access: "forwarded".into(),
                        current: Some(OfferingRef {
                            provider,
                            offering: "model".into(),
                        }),
                        choices: BTreeMap::from([("web_search".into(), Some("live".into()))]),
                        availability: BTreeMap::from([(
                            provider,
                            BTreeMap::from([(
                                "model".into(),
                                Availability {
                                    allowed: true,
                                    ..Availability::default()
                                },
                            )]),
                        )]),
                        interfaces: vec!["openai-responses".into()],
                        ..ThreadContext::default()
                    };
                    let resolution = registry
                        .resolve(&context)
                        .into_iter()
                        .find(|row| row.capability == "web_search")
                        .ok_or("the shared registry did not resolve web search")?;
                    let expected = !required || supported;
                    assert_eq!(
                        resolution.route.is_some(),
                        expected,
                        "legacy={legacy} required={required} supported={supported}"
                    );
                    assert_eq!(resolution.modes.iter().any(|mode| mode == "live"), expected);
                }
            }
        }
        Ok(())
    }
}
