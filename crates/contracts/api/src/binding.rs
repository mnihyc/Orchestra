//! Binding options (provider and the agents it offers, model and effort, existing and new environments) and arrangement views with their type, executor, the tool set that the environment's offers come to, whether Orch's own tools are offered, and isolation facts.
//!
//! A thread is bound to an agent profile on a provider, placed in an
//! environment. [`BindingOptions`] lists what the caller could choose, with
//! why something cannot be chosen. An [`Arrangement`] is one way to place the
//! harness, its tools and the credential, and its [`Isolation`] says what that
//! amounts to for the thread: where the harness runs, what carries out its
//! tool calls, who holds the credential, and who counts its usage.

use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::{AccessSummary, MissingAccess};
use crate::common::absent_or_null;
use crate::forks::FileSelection;
use crate::nodes::{EnvironmentRef, NodeOrSystem, NodeRef};
use crate::ownership::MachineRef;
use crate::routes::Access::{Permission, Rule};
use crate::settings::ModelOption;
use crate::threads::{AgentRef, ProviderRef, Thread};

/// The ways to place a harness, its tools and its credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArrangementType {
    /// Harness, tools and credential each in their own place.
    Separated,
    /// As separated, with the key kept apart from the harness.
    KeySeparated,
    /// The credential is shared into the environment.
    CredentialShared,
    /// The credential is handed over to the environment.
    HandedOver,
    /// Harness and tools together.
    Together,
}

/// The route by which the harness reaches the model. It follows from the arrangement type and the
/// provider, and is never chosen on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialPath {
    /// With its own credential.
    Native,
    /// Through the forwarder.
    Forwarded,
    /// With a credential shared into the environment.
    Shared,
    /// With a credential handed over.
    Handover,
}

/// Where the harness runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessPlace {
    /// With its provider.
    Provider,
    /// In the environment.
    Environment,
}

/// Whose tools the agent works with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IsolationTools {
    /// The harness's own.
    Own,
    /// The executor's, in their place.
    Executor,
}

/// Why the agent has no tools of Orch's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrchToolsReason {
    /// Its harness takes no MCP server.
    Harness,
    /// Its environment switched them off.
    Environment,
    /// The setting that offers them resolves to none.
    Setting,
}

/// Whether the agent gets Orch's own tools, which the server answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IsolationOrchTools {
    /// Whether they are offered.
    pub offered: bool,
    /// Why not, when not.
    pub reason: Option<OrchToolsReason>,
}

/// Who holds and renews the credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialHolder {
    /// The provider.
    Provider,
    /// The environment, only when the credential is handed over.
    Environment,
}

/// How a plan is put to the person in plan mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanProposal {
    /// As a request that waits for an answer, like an approval.
    Request,
    /// As an item of the turn, which ends when the plan is written.
    Item,
}

/// What Proceed does for a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanProceed {
    /// It answers the waiting request: allow.
    Allow,
    /// It sends the harness a message asking it to carry the plan out.
    Message,
}

/// How plan mode works in one harness. Declared next to `plan_mode`, which says that it can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PlanMechanics {
    /// How the plan is put to the person.
    pub proposal: PlanProposal,
    /// What Proceed does.
    pub proceed: PlanProceed,
}

/// Where the list of models a thread may use is enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelEnforcement {
    /// On the wire.
    Wire,
    /// Only when a turn starts and by the adapter.
    Adapter,
}

/// Who reports the usage of a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Metering {
    /// The forwarder, for every request.
    Wire,
    /// The harness itself.
    Harness,
    /// Nobody reports usage, so no cap can count this thread.
    #[serde(rename = "none")]
    Nobody,
}

/// What an arrangement comes to, worked out from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Isolation {
    /// The arrangement type.
    #[serde(rename = "type")]
    pub kind: ArrangementType,
    /// Where the harness runs.
    pub harness_runs_in: HarnessPlace,
    /// The executor that runs its tool calls; `null` is the harness itself, where it runs in the
    /// environment.
    pub executor: Option<String>,
    /// The same executor and tool set, named in each language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_ref: Option<crate::programs::ExecutorRef>,
    /// Whose tools the agent works with.
    pub tools: IsolationTools,
    /// Which of that executor's tool sets the agent works with, where it has several. This and the
    /// executor both follow from what the thread's environment offers.
    pub tool_set: Option<String>,
    /// Whether the agent has Orch's own tools, and when not why.
    pub orch_tools: IsolationOrchTools,
    /// How the harness reaches the model.
    pub credential_path: CredentialPath,
    /// Who holds and renews the credential.
    pub held_by: CredentialHolder,
    /// For a shared or handed-over credential: the adapter's name for what leaves.
    pub form: Option<String>,
    /// How long a copy of the credential is good for; `null` until it is
    /// rotated or revoked.
    pub lifetime_secs: Option<i64>,
    /// Where the list of allowed models is enforced.
    pub model_enforcement: ModelEnforcement,
    /// Who reports usage.
    pub metering: Metering,
}

/// Whose decision keeps an arrangement from being permitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotAllowedBy {
    /// The provider's owner.
    Provider,
    /// An entry on a page.
    Entry,
    /// A policy.
    Policy,
    /// The machine's local policy.
    Machine,
}

/// One way of placing harness, tools and credential for a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Arrangement {
    /// The name a request uses to ask for it.
    #[serde(rename = "type")]
    pub kind: ArrangementType,
    /// For `separated`, what would carry out the tool calls in this environment: the first offer of
    /// the environment that the harness can work through. Nobody names it.
    pub executor: Option<String>,
    /// With which tool set.
    pub tool_set: Option<String>,
    /// The same executor and tool set, named in each language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_ref: Option<crate::programs::ExecutorRef>,
    /// What the arrangement comes to with this provider and environment.
    pub isolation: Isolation,
    /// `1` is what a binding with no preference would take, following the adapter's own order after
    /// whatever the preference setting puts first.
    pub rank: u32,
    /// Whether it can be done at all.
    pub available: bool,
    /// Whether it is allowed.
    pub permitted: bool,
    /// Why not, when not.
    pub reason: Option<String>,
    /// Whose decision it is, for an arrangement that is not allowed.
    pub not_allowed_by: Option<NotAllowedBy>,
    /// For a policy: the place that set it.
    pub scope: Option<NodeRef>,
    /// A sentence for the user about the reason.
    pub detail: Option<String>,
}

pub use crate::resources::HostRef;

/// What settings choose when a thread is bound without being told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BindingDefaults {
    /// The provider.
    pub provider_id: Option<Id>,
    /// The agent profile.
    pub agent_id: Option<Id>,
    /// The model, by its routing name.
    pub model_name: Option<String>,
    /// The effort.
    pub effort: Option<String>,
    /// Whether a lock above holds these.
    pub locked: bool,
}

/// An agent profile a provider offers, and the ways it can be placed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BindingAgent {
    /// The profile.
    pub agent: AgentRef,
    /// Whether the caller may use it and at least one arrangement can run in the judged environment.
    pub usable: bool,
    /// Why not, when not.
    pub reason: Option<String>,
    /// Each arrangement the adapter could support on that provider and the thread's environment,
    /// including ones that are unavailable or not permitted, each with its reason.
    pub arrangements: Vec<Arrangement>,
}

/// A provider the thread could be bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BindingProvider {
    /// The provider.
    pub provider: ProviderRef,
    /// Whether the caller may use it.
    pub usable: bool,
    /// Why not, when not.
    pub reason: Option<String>,
    /// The current controlling place when provider address policy forbids this choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_scope: Option<NodeOrSystem>,
    /// Its models, with the efforts of each.
    pub models: Vec<ModelOption>,
    /// The profiles it offers. An API provider offers every profile whose harness speaks one of its
    /// protocols; a subscription offers the profile that holds the login, and other profiles of the
    /// same harness where the adapter can run them on it.
    pub agents: Vec<BindingAgent>,
}

/// Where an environment is used, seen from one topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentUse {
    /// By this topic.
    Topic,
    /// By another topic of this project.
    Project,
    /// Only elsewhere.
    Elsewhere,
    /// By no thread.
    Unused,
}

/// An existing environment a thread could join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BindingEnvironment {
    /// The environment.
    pub environment: EnvironmentRef,
    /// The host it is on, if managed.
    pub host: Option<HostRef>,
    /// The machine it is on, if it is a directory on one.
    pub machine: Option<MachineRef>,
    /// How many threads are bound to it.
    pub threads: i64,
    /// Whether the caller may put the thread there.
    pub usable: bool,
    /// Why not, when not.
    pub reason: Option<String>,
    /// Whether it is the topic's default.
    pub is_default: bool,
    /// How it is used at present, from the point of view of this topic.
    pub used: EnvironmentUse,
    /// Those of the topics it serves that the caller can view.
    pub topics: Vec<NodeRef>,
    /// How many more it serves.
    pub topics_hidden: i64,
    /// The directory a thread would get there.
    pub directory: String,
}

/// What `env.thread_default` resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentDefault {
    /// A new thread joins the default environment of the topic.
    Shared,
    /// A new thread gets an environment of its own.
    Fresh,
}

/// The permission page that a thread created here would begin with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BindingAccess {
    /// In an environment made from a template.
    pub managed: AccessSummary,
    /// In an environment on a person's machine.
    pub connected: AccessSummary,
}

/// Whether the caller may make a new environment here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BindingCanCreate {
    /// Whether they may.
    pub allowed: bool,
    /// What they lack, when not.
    pub missing: Vec<MissingAccess>,
}

/// Everything the caller could choose when binding a thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BindingOptions {
    /// What settings choose.
    pub defaults: BindingDefaults,
    /// The providers, each with its models and agents.
    pub providers: Vec<BindingProvider>,
    /// Every existing environment the caller manages. This topic's come first (its default, then
    /// those its threads use or that were made for it), and the rest follow.
    pub environments: Vec<BindingEnvironment>,
    /// What a new thread does about its environment unless told.
    pub environment_default: EnvironmentDefault,
    /// Safe resolved repository settings and the place supplying the repository address.
    pub source: BindingSource,
    /// The permission page that a thread created here would begin with.
    pub access: BindingAccess,
    /// Who would be able to see, and who to manage, an environment made now for this topic.
    pub new_environment: AccessSummary,
    /// Whether the caller may make a new environment.
    pub can_create: BindingCanCreate,
    /// The hosts a new environment for a thread here could be made on, the one with the most free memory first:
    /// what each has, what is left of it, whether it can be chosen and why not, what it warns about, and whether
    /// the scheduler would choose it. Empty where the caller may make no environment here or the place names no
    /// template.
    pub hosts: Vec<crate::environment::PlaceHost>,
    /// The caller's own connected machines: a thread runs on one through an environment that is a directory there.
    pub machines: Vec<BindingMachine>,
}

/// Repository defaults shown before preparing a thread's directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BindingSource {
    /// The repository names; secret values are never included.
    #[serde(flatten)]
    pub repository: crate::source::RepositorySource,
    /// The visible place supplying the repository address, or null for the default.
    pub from: Option<crate::nodes::NodeRef>,
}

/// A machine of the caller's own, as a place a thread could run on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BindingMachine {
    /// The machine, and whether it is connected now.
    pub machine: MachineRef,
    /// The environments on it that the caller manages, among `environments`; none where nothing was made there yet.
    pub environments: Vec<Id>,
}

/// Who may use a new environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NewEnvironmentAccess {
    /// As its home's defaults say.
    Defaults,
    /// Its maker alone: its Manage row is cut loose.
    OnlyMe,
}

/// The files a new environment starts with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum NewEnvironmentFiles {
    /// Those of a thread.
    FromThread {
        /// The thread.
        from_thread: Id,
        /// Which of its files; all when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection: Option<FileSelection>,
    },
    /// Those of a snapshot.
    Snapshot {
        /// The snapshot.
        snapshot_id: Id,
    },
}

/// An environment to make now, owned by the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewEnvironment {
    /// Its name, unique among the caller's own; `Main`, then the thread's
    /// name, when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The place whose settings it takes: a topic, a project or an org; the topic at hand when left out, or
    /// with the request that makes an environment alone, the caller's domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_node_id: Option<Id>,
    /// The template; what its home resolves when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<Id>,
    /// The host. When left out, whatever the home resolves; when `null`, the host with the most
    /// room among those the caller may use.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "absent_or_null"
    )]
    #[schemars(with = "Option<Id>")]
    pub host_id: Option<Option<Id>>,
    /// Values of its own for the keys whose scope is environment: mounts, storage, resources, what
    /// it needs, variables, idle stop, update, removal when unused, and what its exec node offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Json>,
    /// Who may use it; by its home's defaults when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewEnvironmentAccess>,
    /// Files to start with, instead of an empty workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<NewEnvironmentFiles>,
    /// Workspace/home storage choices, independently of placement. Defaults to made by Orch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_sources: Option<orch_driver_contract::runtime::EnvironmentDataSources>,
}

/// Where a thread's tools run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentChoice {
    /// An environment that already exists and that the caller manages, whether running or not and
    /// whether or not other threads and topics use it.
    Use(Id),
    /// One made now, owned by the caller.
    New(Box<NewEnvironment>),
}

/// Binds a thread that has no execution history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadBindingSetBody {
    /// The agent profile.
    pub agent_id: Id,
    /// The provider.
    pub provider_id: Id,
    /// How to place the agent; the first permitted arrangement when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<ArrangementType>,
    /// The model, by its routing name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// Puts a thread that has no execution history into an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadEnvironmentSetBody {
    /// The environment: `use` names one, `new` describes one to make.
    #[serde(flatten)]
    pub choice: EnvironmentChoice,
    /// The thread's directory there, relative to the workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

/// The environment used to judge a binding before a thread exists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeBindingOptionsGetQuery {
    /// An existing environment the caller manages. When omitted, use the topic's shared default
    /// if the caller manages it, otherwise the resolved template's declaration for a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<Id>,
    /// Judge the new-environment template rather than the topic's existing shared default; cannot be combined with environment_id.
    #[serde(default)]
    pub environment_new: bool,
}

crate::endpoints! {
    /// Lists what the caller could choose before starting a thread in a topic, project or folder.
    NodeBindingOptionsGet: GET "/nodes/{id}/binding/options", Rule("topic.interact on a topic or project.interact on a project or folder, and environment.manage on an environment named"), query NodeBindingOptionsGetQuery, none, json BindingOptions, 200;
    /// Lists what the caller could choose when binding a thread: providers, agents, models, efforts and environments.
    ThreadBindingOptionsGet: GET "/threads/{id}/binding/options", Permission("thread.manage"), none, none, json BindingOptions, 200;
    /// Binds a thread that has no execution history to an agent profile and provider.
    ThreadBindingSet: PUT "/threads/{id}/binding", Rule("thread.manage on the thread, and the caller's own use of the agent profile and provider with the model and effort"), none, json ThreadBindingSetBody, json Thread, 200;
    /// Puts a thread that has no execution history into an environment, an existing one or a new one.
    ThreadEnvironmentSet: PUT "/threads/{id}/environment", Rule("thread.manage on the thread, and environment.manage on the environment chosen or what making one needs"), none, json ThreadEnvironmentSetBody, json Thread, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_thread_is_put_into_an_environment_that_exists_or_one_made_now() {
        let existing: ThreadEnvironmentSetBody = serde_json::from_value(
            json!({ "use": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "directory": "web" }),
        )
        .unwrap();
        assert!(matches!(existing.choice, EnvironmentChoice::Use(_)));
        assert_eq!(existing.directory.as_deref(), Some("web"));
        let made: ThreadEnvironmentSetBody = serde_json::from_value(json!({
            "new": { "name": "Main", "host_id": null, "access": "only_me", "files": { "snapshot_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c" } }
        }))
        .unwrap();
        match made.choice {
            EnvironmentChoice::New(new) => {
                assert_eq!(
                    new.host_id,
                    Some(None),
                    "null asks for the host with most room"
                );
                assert_eq!(new.access, Some(NewEnvironmentAccess::OnlyMe));
                assert!(matches!(
                    new.files,
                    Some(NewEnvironmentFiles::Snapshot { .. })
                ));
            }
            EnvironmentChoice::Use(_) => panic!("a new environment was written"),
        }
        assert!(
            serde_json::from_value::<ThreadEnvironmentSetBody>(
                json!({ "use": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "bogus": 1 })
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<ThreadEnvironmentSetBody>(json!({ "directory": "web" }))
                .is_err()
        );
    }

    #[test]
    fn an_arrangement_is_written_with_its_type_under_the_word_type() {
        let written = json!({
            "type": "separated",
            "executor": "codex",
            "tool_set": null,
            "isolation": {
                "type": "separated", "harness_runs_in": "provider", "executor": "codex", "tools": "executor", "tool_set": null,
                "orch_tools": { "offered": false, "reason": "environment" }, "credential_path": "native", "held_by": "provider",
                "form": null, "lifetime_secs": null, "model_enforcement": "adapter", "metering": "none"
            },
            "rank": 1, "available": true, "permitted": false,
            "reason": "arrangement_not_allowed", "not_allowed_by": "policy",
            "scope": null, "detail": "The org leaves this arrangement out."
        });
        let arrangement: Arrangement = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(arrangement.kind, ArrangementType::Separated);
        assert_eq!(arrangement.isolation.metering, Metering::Nobody);
        assert_eq!(serde_json::to_value(&arrangement).unwrap(), written);
    }
}
