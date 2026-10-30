//! A thread as the API shows it to one caller: what the engine knows of it, what it runs on and what that comes to, whether the caller may give it input, and what they may do with it.
//!
//! The thread is a node, so what every node has (its place, its owner, what
//! the caller holds on it) is made with the other nodes, in
//! [`crate::routes::nodes::view`], and handed here as the base. What is added
//! is the thread's own: the engine's facts, the binding with what it comes
//! to, where the agent's notes are kept, and three answers that are the access
//! engine's and nobody else's: whether the caller may give the thread input
//! now, with everything they lack when not, what they hold on the environment
//! it runs in (which the views beside the conversation ask, since a thread's
//! own page holds none of it), and whether they manage that environment.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::thread_access::AuthzAccess;
use chrono::{DateTime, Utc};
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{Area, ObjectKind, Permission};
use orch_agent_contract::arrangement::{metering, model_enforcement};
use orch_agent_contract::descriptor::{MemoryKind, OrchToolsSupport};
use orch_agent_contract::session::MemorySetting;
use orch_api_contract::binding::{
    ArrangementType, CredentialHolder, CredentialPath, HarnessPlace, Isolation, IsolationOrchTools,
    IsolationTools, OrchToolsReason,
};
use orch_api_contract::nodes::{NodeBase, NodeRef};
use orch_api_contract::threads::{
    MemoryKeeper, MemoryWhy, Thread, ThreadBinding, ThreadCan, ThreadInputAccess, ThreadKind,
    ThreadMemory, ThreadSides,
};
use orch_authz::{Act, Policy, Principal, nodes, resources};
use orch_db::Conn;
use orch_db::queries::threads as rows;
use orch_engine::access::{Answer, ProviderUse, ResourceAccess};
use orch_engine::views::{BindingParts, ThreadParts, thread_parts};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, ManualClock};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;

/// A word of one contract as the same word of another.
fn same<A: Serialize, B: DeserializeOwned>(word: &A) -> Option<B> {
    serde_json::to_value(word)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

/// Whether the permissions the caller holds on the thread include `permission`.
fn holds(base: &NodeBase<ThreadKind>, permission: Permission) -> bool {
    base.access
        .permissions
        .iter()
        .any(|held| held == permission.as_str())
}

/// Where the agent's notes are kept for a thread: by the harness where it has
/// a memory it can be told a directory for and runs beside the directory, by
/// the agent with its tools where it has none, or by nobody where the setting
/// says so.
fn memory_of(
    setting: MemorySetting,
    kind: MemoryKind,
    arrangement: ArrangementType,
) -> ThreadMemory {
    let beside_provider = arrangement == ArrangementType::Separated;
    let native = kind == MemoryKind::Directory && !beside_provider;
    let why = if native {
        None
    } else if beside_provider {
        Some(MemoryWhy::WithProvider)
    } else if kind == MemoryKind::Home {
        Some(MemoryWhy::HarnessHome)
    } else {
        Some(MemoryWhy::HarnessNone)
    };
    let (kept_by, why) = match setting {
        MemorySetting::Off => (MemoryKeeper::Nobody, Some(MemoryWhy::SettingOff)),
        MemorySetting::Harness if native => (MemoryKeeper::Harness, None),
        MemorySetting::Harness => (MemoryKeeper::Nobody, why),
        MemorySetting::On if native => (MemoryKeeper::Harness, None),
        MemorySetting::On => (MemoryKeeper::Agent, why),
    };
    ThreadMemory {
        kept_by,
        native,
        why,
    }
}

/// What an arrangement comes to for a thread, worked out from what it was
/// given and what its harness declares.
fn isolation_of(
    parts: &BindingParts,
    orch_tools: IsolationOrchTools,
    form: Option<String>,
) -> Isolation {
    let path: Option<orch_agent_contract::credentials::CredentialPath> =
        same(&parts.credential_path);
    let usage_tokens = parts
        .descriptor
        .as_ref()
        .is_some_and(|descriptor| descriptor.capabilities.usage_tokens);
    let harness_runs_in = match parts.arrangement {
        ArrangementType::Separated => HarnessPlace::Provider,
        _ => HarnessPlace::Environment,
    };
    let metered = path.map(|path| metering(path, usage_tokens));
    let enforced = path.map(model_enforcement);
    Isolation {
        kind: parts.arrangement,
        harness_runs_in,
        executor: parts.executor.clone(),
        executor_ref: crate::programs::executor_ref(
            orch_agent_programs::Programs::builtin(),
            parts.executor.as_deref(),
            parts.tool_set.as_deref(),
        ),
        tools: if parts.executor.is_some() {
            IsolationTools::Executor
        } else {
            IsolationTools::Own
        },
        tool_set: parts.tool_set.clone(),
        orch_tools,
        credential_path: parts.credential_path,
        held_by: if parts.credential_path == CredentialPath::Handover {
            CredentialHolder::Environment
        } else {
            CredentialHolder::Provider
        },
        form,
        lifetime_secs: None,
        model_enforcement: enforced
            .and_then(|how| same(&how))
            .unwrap_or(orch_api_contract::binding::ModelEnforcement::Adapter),
        metering: metered
            .and_then(|who| same(&who))
            .unwrap_or(orch_api_contract::binding::Metering::Nobody),
    }
}

/// Whether the agent of the thread is given Orch's own tools, and when not
/// why: its harness takes no MCP server, its environment switched them off,
/// or the setting that offers them resolves to none.
async fn orch_tools_of(
    conn: &mut Conn,
    thread: Id,
    environment: Option<Id>,
    parts: &BindingParts,
) -> ApiResult<IsolationOrchTools> {
    let takes = parts
        .descriptor
        .as_ref()
        .is_some_and(|descriptor| descriptor.capabilities.orch_tools == OrchToolsSupport::Mcp);
    let reason = if !takes {
        Some(OrchToolsReason::Harness)
    } else if let Some(environment) = environment
        && setting::<bool>(conn, Place::Environment(environment), keys::EXEC_ORCH_TOOLS)
            .await
            .map_err(ApiError::internal)?
            == Some(false)
    {
        Some(OrchToolsReason::Environment)
    } else if setting::<Vec<String>>(conn, Place::Node(thread), keys::ORCH_TOOLS_ENABLED)
        .await
        .map_err(ApiError::internal)?
        .is_some_and(|enabled| enabled.is_empty())
    {
        Some(OrchToolsReason::Setting)
    } else {
        None
    };
    Ok(IsolationOrchTools {
        offered: reason.is_none(),
        reason,
    })
}

/// Whether the caller may give the thread input now, and everything they lack
/// when they may not: the access engine's answer about the thread and, where it
/// runs on a provider with a model, about the provider.
async fn input_access(
    conn: &mut Conn,
    principal: &Principal,
    base: &NodeBase<ThreadKind>,
    parts: &ThreadParts,
    now: DateTime<Utc>,
) -> ApiResult<ThreadInputAccess> {
    let thread = base.id;
    let provider = parts
        .binding
        .as_ref()
        .map(|binding| binding.provider.id)
        .zip(parts.facts.model.as_deref());
    let missing = if let Some((provider, model)) = provider {
        let access = AuthzAccess::new(Arc::new(ManualClock::new(now)));
        let answer = access
            .provider_use(
                conn,
                &ProviderUse {
                    person: principal.user(),
                    node: thread,
                    provider,
                    model: Some(model),
                    effort: parts.facts.effort.as_deref(),
                },
            )
            .await
            .map_err(ApiError::internal)?;
        match answer {
            Answer::Allowed { .. } => None,
            Answer::Refused(missing) => Some(missing),
        }
    } else {
        match nodes::check(
            conn,
            principal,
            thread,
            Permission::ThreadInteract,
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(Decision::Allowed { .. }) => None,
            Ok(Decision::Refused(refusal)) => Some(refusal.missing),
            // A thread that is archived, or whose org the caller is suspended
            // in, takes no input: there is nothing the caller could ask for.
            Err(orch_authz::Error::Archived { .. } | orch_authz::Error::Suspended { .. }) => {
                return Ok(ThreadInputAccess {
                    allowed: false,
                    missing: Vec::new(),
                });
            }
            Err(error) => return Err(ApiError::internal(error)),
        }
    };
    Ok(match missing {
        None => ThreadInputAccess {
            allowed: true,
            missing: Vec::new(),
        },
        Some(missing) => ThreadInputAccess {
            allowed: false,
            missing: refusals::missing_access(conn, &missing).await?,
        },
    })
}

/// What the caller holds on the environment the thread runs in, sorted: its two levels and the
/// rows of its four doors, which are the environment's and on no thread's page. Nothing for a
/// thread that has no environment, or whose environment is gone. Whether the caller manages it,
/// which is what deciding where the thread runs takes, is among them.
async fn held_on_environment(
    conn: &mut Conn,
    principal: &Principal,
    environment: Option<Id>,
) -> ApiResult<Vec<String>> {
    let Some(environment) = environment else {
        return Ok(Vec::new());
    };
    let policies = AuthzAccess::policies(conn, environment)
        .await
        .map_err(ApiError::internal)?;
    let effective = match resources::effective(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        policies.at(None).node_policy(),
    )
    .await
    {
        Ok(effective) => effective,
        Err(orch_authz::Error::NotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(ApiError::internal(error)),
    };
    let mut held: Vec<String> = effective
        .held
        .iter()
        .map(|held| held.permission)
        .filter(|permission| permission.area() == Area::Environment || permission.area().is_door())
        .map(|permission| permission.as_str().to_owned())
        .collect();
    held.sort();
    Ok(held)
}

/// How many side threads a thread has open and may have.
async fn sides_of(conn: &mut Conn, thread: Id) -> ApiResult<ThreadSides> {
    let allowed = setting::<bool>(conn, Place::Node(thread), keys::THREAD_SIDE_THREADS)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(true);
    let max = setting::<i64>(conn, Place::Node(thread), keys::THREAD_SIDE_MAX)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    let open = rows::open_side_threads(conn, thread).await?;
    // How an aside here would read, as the harness of the thread's binding verified it; or why none can.
    let agent = orch_db::queries::threads::thread(conn, thread)
        .await?
        .and_then(|row| row.agent_id);
    let aside = match agent {
        Some(agent) => orch_engine::side_threads::aside_of(conn, agent)
            .await
            .map_err(ApiError::internal)?,
        None => Err("the thread is bound to no agent yet".to_owned()),
    };
    let (mode, reason) = match aside {
        Ok((reading, _)) => (
            serde_json::from_value(serde_json::Value::String(
                orch_engine::side_threads::reading_word(reading).to_owned(),
            ))
            .ok(),
            None,
        ),
        Err(why) => (None, Some(why)),
    };
    Ok(ThreadSides {
        open,
        max,
        allowed,
        mode,
        reason,
    })
}

/// The thread whose base is `base`, as `principal` sees it.
pub async fn thread(
    conn: &mut Conn,
    principal: &Principal,
    base: NodeBase<ThreadKind>,
    now: DateTime<Utc>,
) -> ApiResult<Thread> {
    let id = base.id;
    let inspecting = orch_db::queries::trace::retained_read(id);
    let parts = thread_parts(conn, id, Some(principal.user()), orch_types::Time::new(now))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let environment = parts
        .facts
        .environment
        .as_ref()
        .map(|environment| environment.id);
    let binding = match &parts.binding {
        Some(binding) => {
            let orch_tools = if inspecting {
                IsolationOrchTools {
                    offered: false,
                    reason: None,
                }
            } else {
                orch_tools_of(conn, id, environment, binding).await?
            };
            Some(ThreadBinding {
                program: crate::routes::admin::catalogue::reported_program(conn, id).await?,
                agent: binding.agent.clone(),
                provider: binding.provider.clone(),
                arrangement: binding.arrangement,
                isolation: isolation_of(binding, orch_tools, None),
                plan_mode: binding
                    .descriptor
                    .as_ref()
                    .is_some_and(|descriptor| descriptor.capabilities.plan_mode),
                plan_mechanics: binding
                    .descriptor
                    .as_ref()
                    .and_then(|descriptor| descriptor.capabilities.plan_style()),
            })
        }
        None => None,
    };
    let memory = parts.binding.as_ref().map_or_else(
        || ThreadMemory {
            kept_by: MemoryKeeper::Nobody,
            native: false,
            why: Some(MemoryWhy::HarnessNone),
        },
        |binding| {
            memory_of(
                parts.memory_setting,
                binding
                    .descriptor
                    .as_ref()
                    .map_or(MemoryKind::None, |descriptor| {
                        descriptor.capabilities.memory
                    }),
                binding.arrangement,
            )
        },
    );
    let input_access = if inspecting {
        ThreadInputAccess {
            allowed: false,
            missing: Vec::new(),
        }
    } else {
        input_access(conn, principal, &base, &parts, now).await?
    };
    let environment_permissions = if inspecting {
        Vec::new()
    } else {
        held_on_environment(conn, principal, environment).await?
    };
    let can = ThreadCan {
        fork: !inspecting
            && (holds(&base, Permission::ThreadManage) || holds(&base, Permission::ThreadFork)),
        archive: !inspecting
            && (holds(&base, Permission::ThreadManage) || holds(&base, Permission::ThreadArchive)),
        export: !inspecting && holds(&base, Permission::ThreadExport),
        place: environment_permissions
            .iter()
            .any(|held| held == Permission::EnvironmentManage.as_str()),
    };
    let sides = if parts.facts.side_of.is_none() {
        let mut sides = sides_of(conn, id).await?;
        sides.allowed &= !inspecting;
        Some(sides)
    } else {
        None
    };
    let side_of = match parts.facts.side_of {
        Some(parent) => Some(NodeRef {
            thread_display: None,
            id: parent,
            kind: orch_api_contract::nodes::NodeKind::Thread,
            name: rows::node_name(conn, parent).await?,
            redacted: false,
        }),
        None => None,
    };
    let mut facts = parts.facts;
    if !inspecting
        && facts.blocked.as_ref().is_some_and(|blocked| {
            blocked.reason == orch_api_contract::threads::BlockedReason::BindingInvalid
                && blocked
                    .detail
                    .as_deref()
                    .is_some_and(orch_engine::sessions::is_sandbox_refusal)
        })
        && let Some(environment) = facts.environment.clone()
        && let Some(row) = rows::thread(conn, id).await?
    {
        let readiness = crate::sandbox_readiness::environment(
            conn,
            principal,
            environment,
            Some(Place::Node(id)),
            row.agent_id,
            row.executor.as_deref(),
        )
        .await?;
        if !readiness.sandbox.ready
            && let Some(blocked) = facts.blocked.as_mut()
        {
            blocked.sandbox = Some(readiness);
        }
    }
    let mut base = base;
    for reference in &mut base.path {
        if reference.id == facts.id {
            reference.thread_display = Some(Box::new(facts.display.clone()));
        }
    }
    Ok(Thread {
        active_continuation_id: Some(facts.active_continuation_id),
        execution_revision: Some(facts.execution_revision),
        base,
        topic_id: facts.topic_id,
        state: facts.state,
        side_of,
        side_from_turn: facts.side_from_turn,
        sides,
        side_reading: facts.side_reading,
        starting_step: facts.starting_step,
        blocked: facts.blocked,
        queue: facts.queue,
        binding,
        environment: facts.environment,
        environment_permissions,
        directory: facts.directory,
        source: facts.source,
        links: facts.links,
        suggested_links: facts.suggested_links,
        work: facts.work,
        attention: facts.attention,
        display: facts.display,
        model: facts.model,
        model_display: facts.model_display,
        effort: facts.effort,
        cwd: facts.cwd,
        directory_threads: parts.directory_threads,
        memory,
        lock: orch_engine::locks::lock_ref_of(conn, id)
            .await
            .map_err(ApiError::internal)?,
        input_access,
        can,
        current_turn_id: facts.current_turn_id,
        current_segment_id: facts.current_segment_id,
        pending_approvals: facts.pending_approvals,
        pending_inputs: facts.pending_inputs,
        goal: if inspecting {
            None
        } else {
            orch_engine::goals::view(conn, id, now)
                .await
                .map_err(ApiError::internal)?
        },
        usage: facts.usage,
        costs: facts.costs,
        last_activity_at: facts.last_activity_at,
        seq: facts.seq,
        activity_seq: facts.activity_seq,
        my_last_read_seq: facts.my_last_read_seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_with_a_memory_of_its_own_keeps_the_notes_beside_the_directory_and_one_with_none_leaves_them_to_the_agent()
     {
        let native = memory_of(
            MemorySetting::On,
            MemoryKind::Directory,
            ArrangementType::Together,
        );
        assert_eq!(native.kept_by, MemoryKeeper::Harness);
        assert!(native.native && native.why.is_none());
        let none = memory_of(
            MemorySetting::On,
            MemoryKind::None,
            ArrangementType::Together,
        );
        assert_eq!(none.kept_by, MemoryKeeper::Agent);
        assert_eq!(none.why, Some(MemoryWhy::HarnessNone));
    }

    #[test]
    fn a_harness_that_runs_with_its_provider_cannot_keep_notes_in_the_directory() {
        let away = memory_of(
            MemorySetting::On,
            MemoryKind::Directory,
            ArrangementType::Separated,
        );
        assert!(!away.native);
        assert_eq!(away.kept_by, MemoryKeeper::Agent);
        assert_eq!(away.why, Some(MemoryWhy::WithProvider));
    }

    #[test]
    fn the_setting_decides_whether_anyone_keeps_notes_at_all() {
        let off = memory_of(
            MemorySetting::Off,
            MemoryKind::Directory,
            ArrangementType::Together,
        );
        assert_eq!(off.kept_by, MemoryKeeper::Nobody);
        assert_eq!(off.why, Some(MemoryWhy::SettingOff));
        let harness_only = memory_of(
            MemorySetting::Harness,
            MemoryKind::None,
            ArrangementType::Together,
        );
        assert_eq!(harness_only.kept_by, MemoryKeeper::Nobody);
    }
}
