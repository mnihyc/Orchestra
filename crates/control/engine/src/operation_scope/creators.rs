//! Required effects of the two existing thread creators, shared by HTTP and the
//! ordinary writer's last rollbackable checkpoint before birth effects.
//!
//! A prefix owns only its current caller and canonical operation. It never exposes
//! partial caller admission or pretends a future Thread/Environment already exists.

use super::{BindingError, Prepared, blob_sources, targets};
use crate::EngineError;
use crate::binding::{Birth, BirthAgentAccess, CreateThreadError, NewThread};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::nodes::{AgentAccessChoice, AgentAccessWhere};
use orch_api_contract::operations::TargetResolver;
use orch_api_contract::share::ShareParts;
use orch_authz::{Caller, Policy, Principal};
use orch_db::Conn;
use orch_types::Id;

fn unreadable(error: impl std::fmt::Display) -> CreateThreadError {
    CreateThreadError::Internal(EngineError::unreadable(error))
}
fn scope(error: BindingError) -> CreateThreadError {
    match error {
        BindingError::Refused => CreateThreadError::OperationScope,
        error => unreadable(error),
    }
}
fn index(plan: &Prepared, role: &str) -> Result<usize, CreateThreadError> {
    plan.variant().facets.iter().position(|facet|matches!(facet.target,TargetResolver::Resolved(name)|TargetResolver::ResolvedCollection(name) if name==role)).ok_or_else(||unreadable(format!("the creator role {role} is not declared")))
}
fn empty(plan: &mut Prepared, role: &str) -> Result<(), CreateThreadError> {
    plan.bind_empty(index(plan, role)?).map_err(scope)
}
fn bind(
    plan: &mut Prepared,
    role: &str,
    kind: ObjectKind,
    values: &[orch_access_contract::key_scope::KeyResolvedTarget],
) -> Result<(), CreateThreadError> {
    let slot = index(plan, role)?;
    if values.is_empty() {
        plan.bind_empty(slot)
    } else {
        plan.bind(slot, KeyTargetKind::Object(kind), values)
    }
    .map_err(scope)
}
async fn with_caller(
    conn: &mut Conn,
    original: &Principal,
    caller: Caller,
) -> Result<Principal, CreateThreadError> {
    let current = Principal::load(conn, caller, original.now()).await?;
    Ok(match original.second_step_hold() {
        Some(hold) => current.held_for_second_step(hold.clone()),
        None => current,
    })
}
async fn project(conn: &mut Conn, topic: Id) -> Result<Id, CreateThreadError> {
    let rows = orch_db::queries::hierarchy::with_ancestors(conn, &[topic]).await?;
    let topic = rows
        .iter()
        .find(|row| row.id == topic)
        .ok_or_else(|| unreadable("the creator Topic disappeared"))?;
    topic
        .path
        .iter()
        .rev()
        .find_map(|id| {
            rows.iter()
                .find(|row| row.id == *id && row.kind == "project")
                .map(|row| row.id)
        })
        .ok_or_else(|| unreadable("the creator Topic has no Project"))
}

/// Unserializable request-local proof for one canonical creator's rollbackable
/// database prefix. Only the shared writer can complete it against its inserted Thread.
pub struct Prefix {
    principal: Principal,
    selection: String,
    new_environment_home: Option<Id>,
    new_environment_job: Option<Id>,
    complete: Option<Completed>,
}
/// Required admission after the writer's actual Thread has been inserted.
pub struct Completed {
    pub caller: Caller,
    pub plan: Prepared,
    pub agent_access: BirthAgentAccess,
}

impl Prefix {
    /// Check only the current known parent and an explicitly requested Environment
    /// creation home. This proof cannot be captured as an admitted operation.
    pub async fn prepare(
        conn: &mut Conn,
        principal: &Principal,
        selection: &str,
        new: &NewThread,
        newly_created: Option<(Id, Id)>,
    ) -> Result<Self, CreateThreadError> {
        if !matches!(selection, "ThreadCreate" | "TopicCreate.with_thread") {
            return Err(unreadable(
                "an unrelated operation cannot enter a creator prefix",
            ));
        }
        let plan = Prepared::for_selection(selection).map_err(scope)?;
        plan.compatible(principal.caller()).map_err(scope)?;
        let mut known = subject_plan(conn, selection, new.topic).await?;
        let new_environment_home = newly_created.map(|(home, _)| home);
        let new_environment_job = newly_created.map(|(_, job)| job);
        new_environment(&mut known, conn, new_environment_home).await?;
        // The returned partial Caller remains private in this opaque proof. It is used
        // only by the one writer's rollbackable prefix, never persisted or returned.
        let caller = known
            .preparation_caller(principal.caller())
            .map_err(scope)?;
        let principal = with_caller(conn, principal, caller).await?;
        Ok(Self {
            principal,
            selection: selection.into(),
            new_environment_home,
            new_environment_job,
            complete: None,
        })
    }
    pub(crate) fn principal(&self) -> &Principal {
        &self.principal
    }
    pub(crate) async fn complete(
        &mut self,
        conn: &mut Conn,
        new: &NewThread,
        birth: &Birth,
        thread: Id,
    ) -> Result<(Principal, BirthAgentAccess), CreateThreadError> {
        if self.complete.is_some() {
            return Err(unreadable("a creator prefix was completed twice"));
        }
        let mut plan = subject_plan(conn, &self.selection, new.topic).await?;
        new_environment(&mut plan, conn, self.new_environment_home).await?;
        let env = orch_db::queries::environments::thread_engine::environment(conn, new.environment)
            .await?
            .ok_or_else(|| unreadable("the creator Environment disappeared"))?;
        let source = crate::jobs::source::resolved(conn, thread).await?;
        let pending = crate::jobs::source::preparation_pending(
            birth.prepare_source,
            birth.worktree.is_some(),
            source.repository_url.is_some(),
            env.status == "running",
        );
        super::creator_files::bind(
            conn,
            &mut plan,
            new.environment,
            pending,
            birth.worktree.is_some(),
        )
        .await?;
        let frozen = birth
            .agent_access
            .clone()
            .freeze(conn, new.topic)
            .await
            .map_err(|error| CreateThreadError::AgentAccess(Box::new(error)))?;
        let (mut choice, preset) = match frozen {
            BirthAgentAccess::Exact(choice) => (choice, false),
            BirthAgentAccess::FrozenPreset(choice) => (choice, true),
            BirthAgentAccess::ResolvePreset => {
                return Err(unreadable("the creator preset was not frozen"));
            }
        };
        let source = if choice.parts == ShareParts::default()
            || choice.where_ == AgentAccessWhere::Nothing
        {
            None
        } else {
            Some(match choice.where_ {
                AgentAccessWhere::Thread => thread,
                AgentAccessWhere::Topic => new.topic,
                AgentAccessWhere::Project => project(conn, new.topic).await?,
                AgentAccessWhere::Nothing => unreachable!(),
            })
        };
        let target = match source {
            Some(id) => Some(targets::node(conn, id).await.map_err(unreadable)?),
            None => None,
        };
        for (role, kind) in [
            ("creator_agent_projects", ObjectKind::Project),
            ("creator_agent_topics", ObjectKind::Topic),
            ("creator_agent_threads", ObjectKind::Thread),
        ] {
            let mut values = target
                .as_ref()
                .filter(|target| target.object.kind == KeyTargetKind::Object(kind))
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            if !values.is_empty()
                && !plan
                    .permits_binding(
                        self.principal.caller(),
                        index(&plan, role)?,
                        KeyTargetKind::Object(kind),
                        &values,
                    )
                    .map_err(scope)?
            {
                if !preset {
                    return Err(CreateThreadError::OperationScope);
                }
                choice = AgentAccessChoice {
                    where_: AgentAccessWhere::Nothing,
                    parts: ShareParts::default(),
                };
                values.clear();
            }
            bind(&mut plan, role, kind, &values)?;
        }
        let mut initial = if let Some(job) = self.new_environment_job {
            let owner = orch_db::queries::jobs::job(conn, job)
                .await?
                .filter(|job| job.environment_id == Some(new.environment))
                .ok_or_else(|| unreadable("the creator's new Environment job disappeared"))?;
            owner
                .plan
                .get("initial_files")
                .filter(|value| !value.is_null())
                .cloned()
                .map(serde_json::from_value::<crate::initial_files::Plan>)
                .transpose()
                .map_err(unreadable)?
        } else {
            None
        };
        let source_thread = match &initial {
            Some(initial) => Some(
                targets::node(conn, initial.source_thread())
                    .await
                    .map_err(unreadable)?,
            ),
            None => None,
        };
        let source_environment = match &initial {
            Some(initial) => Some(
                targets::page_object(
                    conn,
                    orch_access_contract::decisions::ObjectRef {
                        kind: ObjectKind::Environment,
                        id: initial.source_environment(),
                    },
                )
                .await
                .map_err(unreadable)?,
            ),
            None => None,
        };
        bind(
            &mut plan,
            "creator_initial_file_threads",
            ObjectKind::Thread,
            &source_thread.into_iter().collect::<Vec<_>>(),
        )?;
        bind(
            &mut plan,
            "creator_initial_file_environments",
            ObjectKind::Environment,
            &source_environment.into_iter().collect::<Vec<_>>(),
        )?;
        let preparing = plan
            .preparation_caller(self.principal.caller())
            .map_err(scope)?;
        let principal = with_caller(conn, &self.principal, preparing).await?;
        if pending {
            require_environment(
                conn,
                &principal,
                new.environment,
                Permission::EnvironmentManage,
            )
            .await?;
        }
        if birth.worktree.is_some() {
            require_environment(
                conn,
                &principal,
                new.environment,
                Permission::EnvironmentView,
            )
            .await?;
        }
        let hashes = birth
            .first_input
            .as_ref()
            .map(|first| orch_chat::following::content_blobs(&first.content))
            .unwrap_or_default();
        let sources = blob_sources::prepare(conn, &principal, &plan, &hashes, &[])
            .await
            .map_err(|error| match error {
                blob_sources::Error::Binding(error) => scope(error),
                blob_sources::Error::Missing => CreateThreadError::FirstInput {
                    thread,
                    error: Box::new(crate::inputs::InputError::NotFound),
                },
                error => unreadable(error),
            })?;
        blob_sources::bind(&mut plan, &sources.bindings).map_err(unreadable)?;
        let caller = plan.admit(self.principal.caller(), false).map_err(scope)?;
        let principal = with_caller(conn, &self.principal, caller.clone()).await?;
        let agent_access = if preset {
            BirthAgentAccess::FrozenPreset(choice)
        } else {
            BirthAgentAccess::Exact(choice)
        };
        if let (Some(home), Some(job)) = (self.new_environment_home, self.new_environment_job) {
            let origin =
                orch_authz::CapturedOperation::capture(&caller.clone().with_admin_mode(false))
                    .map_err(unreadable)?;
            crate::jobs::provision::retain_creator_provision_origin(
                conn,
                &origin,
                thread,
                new.environment,
                home,
                job,
                true,
                self.principal.now(),
            )
            .await?;
            if let Some(initial) = initial.as_mut() {
                initial.capture_creator(&caller, thread, new.environment, env.generation)?;
                orch_db::queries::jobs::merge_plan(
                    conn,
                    job,
                    &serde_json::json!({"initial_files":initial}),
                    self.principal.now().get(),
                )
                .await?;
            }
        }
        // The ordinary live-gift writer still performs MayGive and its atomic fallback.
        // Thread creation's local permission checks are not replaced by scope admission.
        self.complete = Some(Completed {
            caller,
            plan,
            agent_access: agent_access.clone(),
        });
        Ok((principal, agent_access))
    }
    pub(crate) fn finish(self) -> Result<Completed, CreateThreadError> {
        self.complete.ok_or_else(|| {
            unreadable("the creator transaction ended without full effect admission")
        })
    }
}

async fn require_environment(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permission: Permission,
) -> Result<(), CreateThreadError> {
    let decision = orch_authz::resources::check(
        conn,
        principal,
        orch_access_contract::decisions::ObjectRef {
            kind: ObjectKind::Environment,
            id,
        },
        permission,
        Policy::default(),
    )
    .await?;
    if let orch_access_contract::decisions::Decision::Refused(refusal) = decision {
        return Err(CreateThreadError::Denied(Box::new(refusal)));
    }
    Ok(())
}
async fn subject_plan(
    conn: &mut Conn,
    selection: &str,
    topic: Id,
) -> Result<Prepared, CreateThreadError> {
    let mut plan = Prepared::for_selection(selection).map_err(scope)?;
    let topic_target = targets::node(conn, topic).await.map_err(unreadable)?;
    if topic_target.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
        return Err(unreadable("a thread creator needs an actual Topic"));
    }
    if selection == "TopicCreate.with_thread" {
        let parent = project(conn, topic).await?;
        let parent = targets::node(conn, parent).await.map_err(unreadable)?;
        plan.bind(0, KeyTargetKind::Object(ObjectKind::Topic), &[parent])
            .map_err(scope)?;
        let slot = index(&plan, "first_thread_destination")?;
        plan.bind(
            slot,
            KeyTargetKind::Object(ObjectKind::Thread),
            &[topic_target],
        )
        .map_err(scope)?;
    } else {
        plan.bind(
            0,
            KeyTargetKind::Object(ObjectKind::Thread),
            &[topic_target],
        )
        .map_err(scope)?;
    }
    Ok(plan)
}
async fn new_environment(
    plan: &mut Prepared,
    conn: &mut Conn,
    home: Option<Id>,
) -> Result<(), CreateThreadError> {
    match home {
        Some(home) => {
            let target = targets::node(conn, home).await.map_err(unreadable)?;
            bind(
                plan,
                "creator_new_environment_homes",
                ObjectKind::Environment,
                &[target],
            )
        }
        None => empty(plan, "creator_new_environment_homes"),
    }
}

/// Re-admit only the still-pending immutable initial-copy effect of its original
/// creator. Current source rights and the concrete snapshot/candidate proof remain
/// in the existing initial-files producer, independently of these credential bounds.
pub async fn restore_initial_files(
    conn: &mut Conn,
    origin: &orch_authz::CapturedOperation,
    created_thread: Id,
    target_environment: Id,
    source_thread: Id,
    source_environment: Id,
    now: orch_types::Time,
) -> Result<Option<Caller>, EngineError> {
    let Some(caller) = origin.restore(conn, now).await? else {
        return Ok(None);
    };
    let plan = Prepared::for_selection(origin.operation()).map_err(EngineError::unreadable)?;
    if !super::compatible_selection(&caller, plan.variant()) {
        return Ok(None);
    }
    let Some(mut facts) = super::creator_files::parent_facets(
        conn,
        origin.operation(),
        created_thread,
        target_environment,
    )
    .await?
    else {
        return Ok(None);
    };
    let env = orch_db::queries::environments::thread_engine::environment(conn, target_environment)
        .await?
        .ok_or_else(|| {
            EngineError::unreadable("the initial-copy target Environment disappeared")
        })?;
    let home = targets::node(conn, env.home_node_id)
        .await
        .map_err(EngineError::unreadable)?;
    let source = targets::node(conn, source_thread)
        .await
        .map_err(EngineError::unreadable)?;
    let source_env = targets::page_object(
        conn,
        orch_access_contract::decisions::ObjectRef {
            kind: ObjectKind::Environment,
            id: source_environment,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    for (role, action, kind, target) in [
        (
            "creator_new_environment_homes",
            orch_access_contract::key_scope::KeyAction::Write,
            ObjectKind::Environment,
            &home,
        ),
        (
            "creator_initial_file_threads",
            orch_access_contract::key_scope::KeyAction::Read,
            ObjectKind::Thread,
            &source,
        ),
        (
            "creator_initial_file_environments",
            orch_access_contract::key_scope::KeyAction::Read,
            ObjectKind::Environment,
            &source_env,
        ),
    ] {
        facts.extend(super::creator_files::required_facet(
            &plan,
            TargetResolver::ResolvedCollection(role),
            action,
            KeyTargetKind::Object(kind),
            target,
        )?);
    }
    let admitted = match caller.admit_facets(&facts) {
        Ok(admitted) => admitted.into_caller(),
        Err(_) => return Ok(None),
    };
    let principal = Principal::load(conn, admitted.clone(), now).await?;
    let permission = match home.object.kind {
        KeyTargetKind::Object(ObjectKind::Org) => Permission::OrgInteract,
        KeyTargetKind::Object(ObjectKind::Project) => Permission::ProjectInteract,
        KeyTargetKind::Object(ObjectKind::Topic) => Permission::TopicInteract,
        KeyTargetKind::Object(ObjectKind::Thread) => Permission::ThreadInteract,
        _ => return Ok(None),
    };
    if !orch_authz::nodes::check(
        conn,
        &principal,
        env.home_node_id,
        permission,
        orch_authz::Act::Use,
        Policy::default(),
    )
    .await?
    .is_allowed()
    {
        return Ok(None);
    }
    Ok(Some(admitted))
}
