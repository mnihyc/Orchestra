//! Version range, arrangement types with their executor and tool sets, credential forms, what a new subscription proposes, a memory it can keep in a directory, sessions held to the MCP servers they are given, and capabilities.
//!
//! [`descriptor`] is everything the adapter declares of the harness, and it is meant to be true of the
//! version that is installed: a capability the harness does not have is absent, not claimed. What was
//! checked against the installed harness is in the notes of this package; what could not be (it needs a
//! real sign-in) is declared as the vendor documents it and marked there.

use orch_agent_contract::credentials::{CredentialForm, CredentialSupport, Custody};
use orch_agent_contract::descriptor::{
    AgentDescriptor, AgentTerminal, ApprovalMode, ApprovalReviewerSupport, ArrangementEntry,
    ArrangementType, AuxiliaryModels, Capabilities, EffortSupport, ExecutorBuild, HarnessInfo,
    HarnessSupport, McpSupport, MemoryKind, NativeProvider, OrchToolsSupport, PlanMechanics,
    PlanProceed, PlanProposal, RuntimeTemplate, SandboxMode, SessionReload, SkillsSupport,
    UsageContext, classify_version,
};
use orch_agent_contract::events::HarnessRequestKind;
use orch_agent_contract::login::{LoginKind, LoginMethodDef, LoginYields};
use orch_types::{Field, FieldType, Json};
use serde_json::json;

use crate::config::{ADAPTER_VERSION, KIND, SUBSCRIPTION_TITLE, SUPPORTED_RANGE};
use crate::parameters::definitions;
use crate::tools::tool_set;

/// The executor the harness works through when it is with its provider.
pub const EXECUTOR: &str = "orch-exec";

/// The form of a short-lived access token of the login, which the provider's runtime issues without the
/// token that renews it.
pub const FORM_ACCESS_TOKEN: &str = "access_token";
/// The form of the long-lived token that the vendor's `setup-token` command prints.
pub const FORM_OAUTH_TOKEN: &str = "oauth_token";
/// The form of the stored login itself.
pub const FORM_LOGIN: &str = "login";

/// The sign-in method that brings the long-lived token.
pub const METHOD_SETUP_TOKEN: &str = "setup_token";
/// The sign-in method that leads the person through the vendor's sign-in page.
pub const METHOD_BROWSER_CODE: &str = "browser_code";

/// How long an access token of the login lasts, as a copy is declared: an hour, the shortest life any
/// renewal has to plan for. The real end is stated with each copy that is issued.
const ACCESS_TOKEN_LIFETIME_SECS: i64 = 3600;

/// How long the token that `setup-token` prints is good for: a year, by the vendor's documentation.
const SETUP_TOKEN_LIFETIME_SECS: i64 = 31_536_000;

/// Where the harness stands against the versions this adapter drives.
#[must_use]
pub fn support_of(version: &str) -> HarnessSupport {
    classify_version(version, SUPPORTED_RANGE, true)
}

/// The sign-in methods of a subscription.
#[must_use]
pub fn login_methods() -> Vec<LoginMethodDef> {
    vec![
        LoginMethodDef {
            id: METHOD_SETUP_TOKEN.to_owned(),
            title: "Bring a long-lived token".to_owned(),
            summary: "Import a token created on another computer.".to_owned(),
            kind: LoginKind::Import,
            fields: vec![{
                let mut field = Field::new("token", "Token", FieldType::Secret);
                field.help = Some("Run claude setup-token on a computer where you are signed in, then paste its token.".to_owned());
                field
            }],
            yields: LoginYields::Token,
            lifetime_secs: Some(SETUP_TOKEN_LIFETIME_SECS),
            requires: Some("A Claude Pro, Max, Team or Enterprise plan.".to_owned()),
            recommended: false,
        },
        LoginMethodDef {
            id: METHOD_BROWSER_CODE.to_owned(),
            title: "Sign in with a code".to_owned(),
            summary: "Open the vendor's sign-in page and paste its authorization code here."
                .to_owned(),
            kind: LoginKind::Interactive,
            fields: Vec::new(),
            yields: LoginYields::Login,
            lifetime_secs: None,
            requires: None,
            recommended: true,
        },
    ]
}

/// The forms of credential this adapter can issue from the login its harness holds, and can run on.
#[must_use]
pub fn credentials() -> CredentialSupport {
    CredentialSupport {
        issues: vec![
            CredentialForm {
                form: FORM_ACCESS_TOKEN.to_owned(),
                custody: Custody::Shared,
                lifetime_secs: Some(ACCESS_TOKEN_LIFETIME_SECS),
            },
            CredentialForm {
                form: FORM_OAUTH_TOKEN.to_owned(),
                custody: Custody::Shared,
                lifetime_secs: None,
            },
            CredentialForm {
                form: FORM_LOGIN.to_owned(),
                custody: Custody::Transferred,
                lifetime_secs: None,
            },
        ],
        accepts: vec![
            FORM_ACCESS_TOKEN.to_owned(),
            FORM_OAUTH_TOKEN.to_owned(),
            FORM_LOGIN.to_owned(),
        ],
    }
}

/// The template of the provider's runtime that this adapter brings: the harness is not a static binary, so
/// the image has the C library it needs, and it is about a gibibyte for each running agent, so the
/// runtime has four.
#[must_use]
pub fn runtime_template() -> RuntimeTemplate {
    RuntimeTemplate {
        name: "Claude Code provider runtime".to_owned(),
        driver: "docker".to_owned(),
        spec: json!({
            "image": "gcr.io/distroless/cc-debian12:nonroot",
            "image_digest": null,
            "runtime": "runc",
            "user": { "name": "nonroot", "uid": 65532, "gid": 65532 },
            "resources": { "cpu_millis": 2000, "memory_mb": 4096, "pids": 1024 },
            "network": { "mode": "bridge", "egress": "open", "dns": [] },
            "security": { "read_only_rootfs": true, "no_new_privileges": true, "cap_add": [], "cap_drop": ["ALL"] },
            "env": {},
            "mounts": [ { "type": "tmpfs", "target": "/tmp" } ],
            "host_labels": []
        }),
        host_labels: Vec::new(),
    }
}

/// The capabilities of the harness as installed.
#[must_use]
pub fn capabilities() -> Capabilities {
    Capabilities {
        // A message written while a turn runs is taken in at the next model request of that turn.
        steer: true,
        interrupt: true,
        background_commands: false,
        approvals: vec![
            HarnessRequestKind::Command,
            HarnessRequestKind::FileChange,
            HarnessRequestKind::Permissions,
            HarnessRequestKind::UserInput,
        ],
        subagents: true,
        subagent_limits: vec![
            orch_agent_contract::descriptor::SubagentControl::AtOnce,
            orch_agent_contract::descriptor::SubagentControl::Depth,
        ],
        plan: true,
        // The permission mode `plan`: the harness proposes its plan by asking to leave plan mode.
        plan_mode: true,
        // The plan is a request that waits for an answer; Proceed allows it.
        plan_mechanics: Some(PlanMechanics {
            proposal: PlanProposal::Request,
            proceed: PlanProceed::Allow,
        }),
        reasoning_summary: true,
        input_images: true,
        // Changed between turns without a restart; a harness that refuses is restarted.
        effort: EffortSupport::Turn,
        mcp_discovery: true,
        mcp: McpSupport {
            stdio: true,
            http: true,
            // A changed set reaches the session when it is opened again, at the next turn.
            reload: false,
            strict: true,
        },
        orch_tools: OrchToolsSupport::Mcp,
        export_native: true,
        import_native: true,
        import_neutral: false,
        fork_native: true,
        native_input_prefix: Some(
            orch_agent_contract::descriptor::NativeInputPrefixSupport::TurnStart,
        ),
        resume: true,
        discover_sessions: true,
        usage_tokens: true,
        // The size of the prompt, and what fills it by part.
        usage_context: UsageContext::Breakdown,
        // Compaction is announced when it begins and when it ends.
        phases: true,
        provider_facts: crate::provider::fact_kinds(),
        model_discovery: true,
        // In this mode the harness reports no title of its own.
        title: false,
        auxiliary_models: AuxiliaryModels::Pinned,
        telemetry_off: true,
        memory: MemoryKind::Directory,
        agent_terminal: AgentTerminal::None,
        // The harness reads the files of a directory as they change.
        session_reload: SessionReload::OnChange,
        // It looks up the tree for its instruction files, but the marker is not a thing it stops at.
        instruction_root_marker: false,
        // The adapter builds a plugin of the session's skills root for each session.
        skills: SkillsSupport::Plugin,
        // The documented classifier default when this person may use it, else the session's own model.
        approval_reviewer: Some(ApprovalReviewerSupport {
            model: crate::review::CLAUDE_REVIEW_MODEL.to_owned(),
            settable: true,
        }),
    }
}

/// The approval modes the harness has here. The harness refuses to bypass permissions when it runs as the
/// administrator, so `bypass` is declared only where it does not. Native auto review stays gated.
#[must_use]
pub fn approval_modes(as_root: bool) -> Vec<ApprovalMode> {
    let mut modes = vec![ApprovalMode::Ask, ApprovalMode::OnRequest];
    if !as_root {
        modes.push(ApprovalMode::Bypass);
    }
    if auto_proven() {
        modes.push(ApprovalMode::Auto);
    }
    modes
}

/// The feature a conformance case proves for Auto.
const AUTO_FEATURE: &str = "approval_modes.auto";

/// Whether the manifest has the conformance case that proves Auto's reviewer on a build. Without it Auto is not
/// offered, whatever else says so.
#[must_use]
pub fn auto_proven() -> bool {
    manifest().proves(AUTO_FEATURE)
}

/// The manifest of this program, which holds what is true of it however it is built.
#[must_use]
pub fn manifest() -> &'static orch_agent_programs::ProgramManifest {
    orch_agent_programs::shipped(KIND)
}

/// The words of the descriptor in the languages other than English.
fn text() -> std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> {
    let mut words = manifest().name_text();
    let chinese = words.entry("zh-CN".to_owned()).or_default();
    for (place, text) in [
        ("native_provider.title", "Claude 订阅"),
        ("windows.weekly", "每周"),
        ("windows.5h", "5 小时"),
        ("windows.window", "时段"),
        ("native_provider.plans.team", "团队版"),
        ("native_provider.plans.enterprise", "企业版"),
        ("parameters.service_tier.title", "速度"),
        (
            "parameters.service_tier.description",
            "程序的快速模式，适用于有此模式的模型：回答更快，每个令牌的价格更高。使用订阅时，它消耗用量额度，而不是套餐。只在提供方的管理者为该模型启用时可用。",
        ),
        ("parameters.context.title", "上下文"),
        (
            "parameters.context.description",
            "代理保留多少对话（以令牌计），最多到模型的管理者开放的最大值。程序会在该大小内缩短对话，并自行留出回答所需的余量：上下文为 1 000 000 时约在 967 000。",
        ),
        ("parameters.small_model.title", "后台工作使用的模型"),
        (
            "parameters.small_model.description",
            "程序的辅助步骤所使用的允许模型；留空则使用该线程的模型。",
        ),
        ("parameters.tool_search.title", "按需加载工具"),
        (
            "parameters.tool_search.description",
            "让程序在需要时才加载 MCP 工具的定义，而不是一次全部加载。未设置时，在转发器之后保持关闭（这是程序在另一个端点之后的自身选择），其他情况下开启。",
        ),
        ("parameters.webfetch_preflight.title", "向厂商核对网页"),
        (
            "parameters.webfetch_preflight.description",
            "加载网页之前，程序会把该网页的主机告诉厂商的 API，用黑名单核对。自动：对厂商自己的上游保持开启，对其他上游关闭，否则会让第三方知道抓取了哪些主机。适用于程序使用自带工具的情况。",
        ),
        (
            "native_provider.login_methods.browser_code.title",
            "使用授权码登录",
        ),
        (
            "native_provider.login_methods.browser_code.summary",
            "打开厂商的登录页面，再把页面给出的授权码粘贴到这里。",
        ),
        (
            "native_provider.login_methods.setup_token.title",
            "导入长期令牌",
        ),
        (
            "native_provider.login_methods.setup_token.summary",
            "导入在另一台计算机上创建的令牌。",
        ),
        (
            "native_provider.login_methods.setup_token.requires",
            "需要 Claude Pro、Max、Team 或 Enterprise 方案。",
        ),
        (
            "native_provider.login_methods.setup_token.fields.token.label",
            "令牌",
        ),
        (
            "native_provider.login_methods.setup_token.fields.token.help",
            "在已登录的计算机上运行 claude setup-token，然后粘贴它输出的令牌。",
        ),
    ] {
        chinese.insert(place.to_owned(), text.to_owned());
    }
    words
}

/// Everything the adapter declares of the harness `version`.
#[must_use]
pub fn descriptor(version: &str, as_root: bool) -> AgentDescriptor {
    AgentDescriptor {
        kind: KIND.to_owned(),
        adapter_version: ADAPTER_VERSION,
        harness: HarnessInfo {
            name: manifest().name().to_owned(),
            version: version.to_owned(),
            supported_range: SUPPORTED_RANGE.to_owned(),
            status: support_of(version),
        },
        arrangements: vec![
            entry(ArrangementType::KeySeparated),
            entry(ArrangementType::CredentialShared),
            entry(ArrangementType::HandedOver),
            ArrangementEntry {
                kind: ArrangementType::Separated,
                executor: Some(EXECUTOR.to_owned()),
                executor_version: Some(ExecutorBuild::Any),
                tool_sets: Some(vec![
                    tool_set::CLAUDE_CODE.to_owned(),
                    tool_set::PLAIN.to_owned(),
                    tool_set::CUSTOM.to_owned(),
                ]),
            },
            entry(ArrangementType::Together),
        ],
        native_provider: Some(NativeProvider {
            harness_kind: KIND.to_owned(),
            id: None,
            title: SUBSCRIPTION_TITLE.to_owned(),
            plans: [
                ("pro", "Pro"),
                ("max", "Max"),
                ("team", "Team"),
                ("enterprise", "Enterprise"),
            ]
            .into_iter()
            .map(|(id, name)| (id.to_owned(), name.to_owned()))
            .collect(),
            login_methods: login_methods(),
            options_schema: Json::Object(serde_json::Map::new()),
            arrangements: vec![
                ArrangementType::Separated,
                ArrangementType::CredentialShared,
            ],
        }),
        native_providers: Vec::new(),
        forwarded_protocols: vec!["anthropic-messages".to_owned()],
        direct_protocols: Vec::new(),
        credentials: credentials(),
        runtime_template: Some(runtime_template()),
        approval_modes: approval_modes(as_root),
        sandbox_modes: vec![SandboxMode::Workspace, SandboxMode::Open],
        // Offered only once the exact loaded build's conformance proves every action path (plan-mode naming or
        // an allowed-tools list proves neither); until then an aside is refused with why.
        side_thread_modes: Vec::new(),
        capabilities: {
            let mut declared = capabilities();
            if version != "2.1.296" {
                declared.native_input_prefix = None;
            }
            declared
        },
        parameters: definitions(),
        text: text(),
        extensions: Default::default(),
    }
}

fn entry(kind: ArrangementType) -> ArrangementEntry {
    ArrangementEntry {
        kind,
        executor: None,
        executor_version: None,
        tool_sets: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_adapter_declares_holds_together() {
        assert_eq!(descriptor("2.1.296", false).check(), Ok(()));
        assert_eq!(descriptor("2.1.296", true).check(), Ok(()));
    }

    #[test]
    fn the_harness_stands_against_the_range_as_the_contract_says() {
        assert_eq!(support_of("2.1.285"), HarnessSupport::Unsupported);
        assert_eq!(support_of("2.1.296"), HarnessSupport::Unsupported);
        assert_eq!(support_of("2.1.312"), HarnessSupport::Supported);
        assert_eq!(support_of("2.2.0"), HarnessSupport::Untested);
        assert_eq!(support_of("2.1.284"), HarnessSupport::Unsupported);
        assert_eq!(
            descriptor("2.1.312", false).harness.status,
            HarnessSupport::Supported
        );
    }

    #[test]
    fn the_arrangements_are_in_the_order_the_adapter_prefers_them() {
        let declared = descriptor("2.1.296", false);
        let kinds: Vec<_> = declared
            .arrangements
            .iter()
            .map(|entry| entry.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                ArrangementType::KeySeparated,
                ArrangementType::CredentialShared,
                ArrangementType::HandedOver,
                ArrangementType::Separated,
                ArrangementType::Together
            ]
        );
        let separated = declared.separated().next().unwrap();
        assert_eq!(separated.executor.as_deref(), Some("orch-exec"));
        assert_eq!(separated.executor_version, Some(ExecutorBuild::Any));
        assert_eq!(
            separated.tool_sets.as_deref(),
            Some(
                &[
                    "claude-code".to_owned(),
                    "plain".to_owned(),
                    "custom".to_owned()
                ][..]
            )
        );
    }

    #[test]
    fn a_new_subscription_may_share_a_token_or_keep_it_and_work_through_orchs_tools() {
        let declared = descriptor("2.1.296", false);
        let native = declared.native_provider.unwrap();
        assert_eq!(
            native.arrangements,
            [
                ArrangementType::Separated,
                ArrangementType::CredentialShared
            ]
        );
        let methods: Vec<_> = native
            .login_methods
            .iter()
            .map(|m| (m.id.as_str(), m.recommended))
            .collect();
        assert_eq!(methods, [("setup_token", false), ("browser_code", true)]);
        assert_eq!(native.login_methods[0].lifetime_secs, Some(31_536_000));
    }

    #[test]
    fn a_token_that_lapses_is_taken_before_one_that_lasts_and_the_login_is_handed_over_whole() {
        let forms = credentials();
        let usable = orch_agent_contract::credentials::usable_forms(
            &forms.issues,
            &forms.accepts,
            Some(Custody::Shared),
        );
        let names: Vec<_> = usable.iter().map(|form| form.form.as_str()).collect();
        assert_eq!(names, ["access_token", "oauth_token"]);
        let login = forms
            .issues
            .iter()
            .find(|form| form.form == "login")
            .unwrap();
        assert_eq!(login.custody, Custody::Transferred);
    }

    #[test]
    fn administrators_do_not_get_the_mode_that_bypasses_permissions() {
        assert!(
            descriptor("2.1.296", false)
                .approval_modes
                .contains(&ApprovalMode::Bypass)
        );
        assert!(
            !descriptor("2.1.296", true)
                .approval_modes
                .contains(&ApprovalMode::Bypass)
        );
    }

    #[test]
    fn what_the_harness_does_not_have_is_not_claimed() {
        let capabilities = descriptor("2.1.296", false).capabilities;
        assert!(
            !capabilities.import_neutral,
            "a history from another harness arrives by summary"
        );
        assert!(!capabilities.title);
        assert_eq!(capabilities.agent_terminal, AgentTerminal::None);
        assert!(capabilities.mcp.strict && capabilities.interrupt && capabilities.resume);
        assert!(
            !capabilities.mcp.reload,
            "server changes take effect through a same-conversation resume, not an unsupported live call"
        );
        assert_eq!(capabilities.memory, MemoryKind::Directory);
        assert_eq!(
            descriptor("2.1.296", false).sandbox_modes,
            [SandboxMode::Workspace, SandboxMode::Open]
        );
    }

    #[test]
    fn every_word_of_the_descriptor_that_a_person_reads_has_its_chinese_text() {
        let declared = descriptor("2.1.296", false);
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
