//! Exact operands for the existing thread creator, without changing its ordinary writer.
//!
//! Existing operands resolve before the writer. New Environments and This-thread
//! contributions complete against their actual inserted targets in a private transaction.

use crate::key_operations::{admission::Prepared, blob_sources, resolver::Disclosure, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationFuture, OptionalOperationFuture};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::{ErrorCode, ReceiptProjection};
use orch_api_contract::nodes::{
    AgentAccessChoice, AgentAccessWhere, CreateThread, Node, ThreadCreateReceipt, TopicCreateBody,
};
use orch_api_contract::operations::TargetResolver;
use orch_api_contract::share::ShareParts;
use orch_api_contract::threads::Thread;
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::Conn;
use orch_engine::binding::{Birth, BirthAgentAccess, NewThread};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(crate) struct Creation {
    pub new: NewThread,
    pub birth: Birth,
    pub created: Option<orch_engine::binding::CreatedThread>,
    plan: Prepared,
}

fn modern(caller: &Caller) -> bool {
    caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}
fn denied() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the key does not allow the complete creation",
    )
}
fn slot(plan: &Prepared, role: &str) -> ApiResult<usize> {
    plan.variant()
        .facets
        .iter()
        .position(|facet| {
            matches!(facet.target,
        TargetResolver::Resolved(name) | TargetResolver::ResolvedCollection(name) if name==role)
        })
        .ok_or_else(|| ApiError::internal(format!("the creator role {role} is not declared")))
}
fn bind(
    plan: &mut Prepared,
    role: &str,
    kind: ObjectKind,
    values: &[orch_access_contract::key_scope::KeyResolvedTarget],
) -> ApiResult<()> {
    let index = slot(plan, role)?;
    if values.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, KeyTargetKind::Object(kind), values)
    }
}
fn nothing() -> AgentAccessChoice {
    AgentAccessChoice {
        where_: AgentAccessWhere::Nothing,
        parts: ShareParts::default(),
    }
}
async fn project(conn: &mut Conn, topic: Id) -> ApiResult<Id> {
    let rows = orch_db::queries::hierarchy::with_ancestors(conn, &[topic]).await?;
    let topic = rows
        .iter()
        .find(|row| row.id == topic)
        .ok_or_else(ApiError::not_found)?;
    topic
        .path
        .iter()
        .rev()
        .find_map(|id| {
            rows.iter()
                .find(|row| row.id == *id && row.kind == "project")
                .map(|row| row.id)
        })
        .ok_or_else(ApiError::not_found)
}

/// Resolve exactly the existing Environment path. A missing future object is never
/// represented by a fabricated target or an empty required singleton.
pub(super) async fn materialize(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    topic: Id,
    body: &CreateThread,
    mut plan: Prepared,
) -> ApiResult<Option<Prepared>> {
    if body.program.is_some() {
        if ctx.route.name != "ThreadCreate" {
            return Err(ApiError::invalid(
                "first_thread.program",
                "Create an interactive program in an existing topic",
            ));
        }
        return crate::ask::birth::prepare(ctx, conn, caller, topic, body, plan)
            .await
            .map(Some);
    }
    if matches!(body.environment, Some(EnvironmentChoice::New(_))) {
        return Ok(None);
    }
    let parent = targets::node(conn, topic).await?;
    if parent.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
        return Err(ApiError::not_found());
    }
    let required = if plan.variant().operation == "TopicCreate.with_thread" {
        slot(&plan, "first_thread_destination")?
    } else {
        0
    };
    plan.bind(
        required,
        KeyTargetKind::Object(ObjectKind::Thread),
        std::slice::from_ref(&parent),
    )?;
    bind(
        &mut plan,
        "creator_new_environment_homes",
        ObjectKind::Environment,
        &[],
    )?;
    bind(
        &mut plan,
        "creator_initial_file_threads",
        ObjectKind::Thread,
        &[],
    )?;
    bind(
        &mut plan,
        "creator_initial_file_environments",
        ObjectKind::Environment,
        &[],
    )?;
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    authorization::require(
        conn,
        &principal,
        topic,
        authorization::exact(Permission::TopicStartThreads),
        Act::Use,
        Policy::default(),
    )
    .await?;
    let environment = match super::make::environment_of(
        &ctx.app,
        conn,
        topic,
        caller.user(),
        body.environment.as_ref(),
    )
    .await
    {
        Ok(environment) => environment,
        Err(error)
            if body.environment.is_none() && error.code() == ErrorCode::EnvironmentNoneUsable =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let model = match &body.model {
        Some(value) => Some(value.clone()),
        None => setting::<String>(conn, Place::Node(topic), keys::AGENT_MODEL)
            .await
            .map_err(ApiError::internal)?,
    };
    let chosen = super::make::binding_of(
        &ctx.app,
        conn,
        topic,
        caller.user(),
        body.binding.as_ref(),
        model.as_deref(),
        body.effort.as_deref(),
    )
    .await?
    .ok_or_else(|| {
        ApiError::new(
            ErrorCode::BindingIncompatible,
            "No permitted binding was selected",
        )
        .with("reason", "unbound")
    })?;
    let worktree = super::make::worktree_of(body)?;
    let source = orch_engine::jobs::source::resolved(conn, topic)
        .await
        .map_err(ApiError::engine)?;
    let env = orch_db::queries::environments::thread_engine::environment(conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let pending = orch_engine::jobs::source::preparation_pending(
        body.prepare_source,
        worktree.is_some(),
        source.repository_url.is_some(),
        env.status == "running",
    );
    orch_engine::operation_scope::creator_files::bind(
        conn,
        plan.shared_mut(),
        environment,
        pending,
        worktree.is_some(),
    )
    .await
    .map_err(ApiError::engine)?;
    orch_engine::operation_scope::creator_files::bind_readers(
        conn,
        plan.shared_mut(),
        environment,
        body.let_into_environment.unwrap_or(false),
    )
    .await
    .map_err(ApiError::engine)?;
    let file_principal = plan.preparation_principal(ctx, conn, caller).await?;
    if pending {
        authorization::require_resource_projected(
            conn,
            &file_principal,
            orch_access_contract::permissions::ResourceKind::Environment,
            environment,
            Permission::EnvironmentManage,
            Policy::default(),
            true,
        )
        .await?;
    }
    if worktree.is_some() {
        authorization::require_resource_projected(
            conn,
            &file_principal,
            orch_access_contract::permissions::ResourceKind::Environment,
            environment,
            Permission::EnvironmentView,
            Policy::default(),
            true,
        )
        .await?;
    }
    let explicit = body.agent_access.is_some();
    let mut choice = match body.agent_access {
        Some(choice) => choice,
        None => setting::<AgentAccessChoice>(conn, Place::Node(topic), keys::THREADS_AGENT_ACCESS)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("the thread creation preset is unset"))?,
    };
    if choice.where_ == AgentAccessWhere::Thread && choice.parts != ShareParts::default() {
        return Ok(None);
    }
    let shared =
        if choice.parts == ShareParts::default() || choice.where_ == AgentAccessWhere::Nothing {
            None
        } else {
            let source = match choice.where_ {
                AgentAccessWhere::Topic => topic,
                AgentAccessWhere::Project => project(conn, topic).await?,
                _ => return Ok(None),
            };
            Some(targets::node(conn, source).await?)
        };
    for (role, kind) in [
        ("creator_agent_projects", ObjectKind::Project),
        ("creator_agent_topics", ObjectKind::Topic),
        ("creator_agent_threads", ObjectKind::Thread),
    ] {
        let value = shared
            .as_ref()
            .filter(|target| target.object.kind == KeyTargetKind::Object(kind));
        let mut values = value.into_iter().cloned().collect::<Vec<_>>();
        if !values.is_empty()
            && !plan.permits_binding(
                caller,
                slot(&plan, role)?,
                KeyTargetKind::Object(kind),
                &values,
            )?
        {
            if explicit {
                return Err(denied());
            }
            choice = nothing();
            values.clear();
        }
        bind(&mut plan, role, kind, &values)?;
    }
    let name = body
        .name
        .as_deref()
        .map(|name| crate::routes::nodes::clean_name("name", name))
        .transpose()?;
    let mut settings = Vec::new();
    if let Some(params) = &body.params {
        settings.push((keys::AGENT_PARAMS.to_owned(), serde_json::json!(params)));
    }
    if let Some(cwd) = &body.cwd {
        settings.push((keys::THREAD_CWD.to_owned(), serde_json::json!(cwd)));
    }
    let new = NewThread {
        topic,
        name: name
            .clone()
            .unwrap_or_else(|| super::make::NEW_THREAD.to_owned()),
        agent: chosen.agent,
        provider: chosen.provider,
        environment,
        directory: worktree
            .as_ref()
            .map(|choice| choice.directory.clone())
            .or_else(|| body.directory.clone())
            .unwrap_or_default(),
        arrangement: chosen.arrangement,
        model: body.model.clone(),
        effort: body.effort.clone(),
    };
    let birth = Birth {
        let_into_environment: body.let_into_environment.unwrap_or(false),
        agent_access: if explicit {
            BirthAgentAccess::Exact(choice)
        } else {
            BirthAgentAccess::FrozenPreset(choice)
        },
        first_input: body.first_input.clone(),
        prepare_source: body.prepare_source,
        worktree,
        titled_by_automation: name.is_none(),
        settings,
        page: super::make::page_of(body.access.as_ref())?,
    };
    let hashes = body
        .first_input
        .as_ref()
        .map(|first| orch_chat::following::content_blobs(&first.content))
        .unwrap_or_default();
    let sources = blob_sources::prepare(ctx, conn, caller, &plan, &hashes, &[]).await?;
    blob_sources::bind(&mut plan, &sources.bindings)?;
    Ok(Some(plan.clone().with_data(Creation {
        new,
        birth,
        created: None,
        plan,
    })))
}

pub(super) fn thread<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: CreateThread,
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !modern(caller) {
            return Ok(None);
        }
        let plan = Prepared::new(ctx.route, "ThreadCreate")?;
        match materialize(ctx, conn, caller, ctx.id("id")?, &body, plan).await? {
            Some(plan) => Ok(Some(plan)),
            None => Prepared::new(ctx.route, "ThreadCreate")?
                .thread_creation_prefix()
                .map(Some),
        }
    })
}

pub(crate) fn complete_topic<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    topic: Id,
    body: &'a TopicCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        complete_staged(
            ctx,
            conn,
            caller,
            topic,
            body.first_thread.as_ref().ok_or_else(denied)?,
        )
        .await
    })
}

pub(crate) fn complete_staged<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    topic: Id,
    body: &'a CreateThread,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let now = orch_types::Time::new(ctx.app.clock.now());
        let principal = authorization::load_principal(conn, caller.clone(), now).await?;
        let principal = crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?;
        let (new, mut birth, home) =
            super::make::resolve_in(ctx, conn, &principal, topic, body, true).await?;
        let selection = if ctx.route.name == "ThreadCreate" {
            "ThreadCreate"
        } else {
            "TopicCreate.with_thread"
        };
        let prefix = match orch_engine::operation_scope::creators::Prefix::prepare(
            conn, &principal, selection, &new, home,
        )
        .await
        {
            Ok(prefix) => prefix,
            Err(error) => return Err(super::create_failed(conn, error).await),
        };
        let result = ctx
            .app
            .engine
            .create_thread_staged_in_transaction(
                conn,
                prefix,
                &new,
                &birth,
                orch_types::new_id(),
                principal.now().get(),
            )
            .await;
        let (created, complete) = match result {
            Ok(value) => value,
            Err(error) => return Err(super::create_failed(conn, error).await),
        };
        birth.agent_access = complete.agent_access;
        let plan = Prepared::from_shared(complete.plan);
        Ok(plan.clone().with_data(Creation {
            new,
            birth,
            created: Some(created),
            plan,
        }))
    })
}

/// The only Environment database prefix for an admitted creator stage. It checks
/// the exact current parent/home facets and calls the existing ordinary writer;
/// no partial caller or transaction is returned to another producer.
pub(super) async fn environment_prefix(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    new: orch_api_contract::binding::NewEnvironment,
) -> ApiResult<orch_engine::environments::Made> {
    if ctx.topic_creation_prefix().is_none() {
        return Err(ApiError::internal(
            "a creator Environment prefix needs its private transaction stage",
        ));
    }
    let selection = if ctx.route.name == "ThreadCreate" {
        "ThreadCreate"
    } else {
        "TopicCreate.with_thread"
    };
    let home = new.home_node_id.unwrap_or(topic);
    let mut plan = Prepared::new(ctx.route, selection)?;
    let subject = targets::node(
        conn,
        if selection == "ThreadCreate" {
            topic
        } else {
            ctx.id("id")?
        },
    )
    .await?;
    plan.bind(
        0,
        KeyTargetKind::Object(if selection == "ThreadCreate" {
            ObjectKind::Thread
        } else {
            ObjectKind::Topic
        }),
        &[subject],
    )?;
    if selection == "TopicCreate.with_thread" {
        plan.bind(
            slot(&plan, "first_thread_destination")?,
            KeyTargetKind::Object(ObjectKind::Thread),
            &[targets::node(conn, topic).await?],
        )?;
    }
    let target = targets::node(conn, home).await?;
    bind(
        &mut plan,
        "creator_new_environment_homes",
        ObjectKind::Environment,
        &[target],
    )?;
    // Scope the known destination before resolving any optional copy source.
    let _ = plan
        .preparation_principal(ctx, conn, principal.caller())
        .await?;
    let initial_identity = match new.files.as_ref() {
        Some(files) => Some(
            orch_engine::initial_files::source_identity(conn, files)
                .await
                .map_err(crate::routes::environments::hosted::initial_failed)?,
        ),
        None => None,
    };
    let initial_thread = match initial_identity {
        Some((Some(id), _)) => Some(targets::node(conn, id).await?),
        Some((None, _)) | None => None,
    };
    let initial_environment = match initial_identity {
        Some((_, id)) => Some(
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?,
        ),
        None => None,
    };
    bind(
        &mut plan,
        "creator_initial_file_threads",
        ObjectKind::Thread,
        &initial_thread.into_iter().collect::<Vec<_>>(),
    )?;
    bind(
        &mut plan,
        "creator_initial_file_environments",
        ObjectKind::Environment,
        &initial_environment.into_iter().collect::<Vec<_>>(),
    )?;
    // This canonical, hardwired prefix is the declared Environment Write at its
    // actual home. All dependent identities complete in the same private transaction.
    let facets = plan
        .shared()
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| {
            facet.usage == orch_api_contract::operations::FacetUsage::Required
                && matches!(
                    facet.target,
                    TargetResolver::PermissionObject
                        | TargetResolver::Resolved("first_thread_destination")
                        | TargetResolver::ResolvedCollection("creator_new_environment_homes")
                        | TargetResolver::ResolvedCollection("creator_initial_file_threads")
                        | TargetResolver::ResolvedCollection("creator_initial_file_environments")
                )
        })
        .filter(|(_, facet)| {
            initial_identity.is_some()
                || !matches!(
                    facet.target,
                    TargetResolver::ResolvedCollection(
                        "creator_initial_file_threads" | "creator_initial_file_environments"
                    )
                )
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut known = Vec::new();
    for index in facets {
        let facet = &plan.variant().facets[index];
        let (kind, target) = match facet.target {
            TargetResolver::PermissionObject => (
                KeyTargetKind::Object(if selection == "ThreadCreate" {
                    ObjectKind::Thread
                } else {
                    ObjectKind::Topic
                }),
                targets::node(
                    conn,
                    if selection == "ThreadCreate" {
                        topic
                    } else {
                        ctx.id("id")?
                    },
                )
                .await?,
            ),
            TargetResolver::Resolved("first_thread_destination") => (
                KeyTargetKind::Object(ObjectKind::Thread),
                targets::node(conn, topic).await?,
            ),
            TargetResolver::ResolvedCollection("creator_initial_file_threads") => {
                let Some(thread) = initial_identity.and_then(|(thread, _)| thread) else {
                    continue;
                };
                (
                    KeyTargetKind::Object(ObjectKind::Thread),
                    targets::node(conn, thread).await?,
                )
            }
            TargetResolver::ResolvedCollection("creator_initial_file_environments") => (
                KeyTargetKind::Object(ObjectKind::Environment),
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: initial_identity.ok_or_else(ApiError::not_found)?.1,
                    },
                )
                .await?,
            ),
            _ => (
                KeyTargetKind::Object(ObjectKind::Environment),
                targets::node(conn, home).await?,
            ),
        };
        known.extend(
            orch_engine::operation_scope::facets(selection, facet, kind, &[target])
                .map_err(crate::key_operations::admission::binding_error)?,
        );
    }
    let caller = principal
        .caller()
        .admit_facets(&known)
        .map_err(|_| denied())?
        .into_caller();
    let checked = authorization::load_principal(conn, caller, principal.now()).await?;
    let checked = crate::sign_in::second_step::hold(conn, ctx.user()?, checked).await?;
    let prepared = match new.files.as_ref() {
        Some(files) => Some(
            orch_engine::initial_files::plan(conn, &checked, files)
                .await
                .map_err(crate::routes::environments::hosted::initial_failed)?,
        ),
        None => None,
    };
    if let Some(initial) = prepared.as_ref().filter(|plan| plan.copies_files()) {
        crate::routes::duplicates::copy_to(
            conn,
            &checked,
            orch_engine::copy_out::Subject::Object(ObjectRef {
                kind: ObjectKind::Environment,
                id: initial.source_environment(),
            }),
            Some(home),
        )
        .await?;
    }
    // The materialized plan may reveal a concurrent source binding change. Carry
    // its actual source evidence, with the known parent/home evidence, into the writer.
    let checked = if let Some(initial) = &prepared {
        let mut actual = Vec::new();
        for (role, kind, target) in [
            (
                "creator_initial_file_threads",
                ObjectKind::Thread,
                match initial.source_thread() {
                    Some(thread) => Some(targets::node(conn, thread).await?),
                    None => None,
                },
            ),
            (
                "creator_initial_file_environments",
                ObjectKind::Environment,
                Some(
                    targets::page_object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id: initial.source_environment(),
                        },
                    )
                    .await?,
                ),
            ),
        ] {
            let Some(target) = target else {
                continue;
            };
            let facet = &plan.variant().facets[slot(&plan, role)?];
            actual.extend(
                orch_engine::operation_scope::facets(
                    selection,
                    facet,
                    KeyTargetKind::Object(kind),
                    &[target],
                )
                .map_err(crate::key_operations::admission::binding_error)?,
            );
        }
        known.extend(actual);
        let caller = checked
            .caller()
            .admit_facets(&known)
            .map_err(|_| denied())?
            .into_caller();
        let principal = authorization::load_principal(conn, caller, checked.now()).await?;
        crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?
    } else {
        checked
    };
    crate::routes::environments::hosted::create_with_prepared_initial(
        ctx, conn, &checked, new, prepared,
    )
    .await
}

pub(crate) async fn receipt(
    conn: &mut Conn,
    actor: Id,
    created: Id,
    topic: Id,
    first: Option<&orch_api_contract::nodes::FirstInput>,
) -> ApiResult<ThreadCreateReceipt> {
    let input_id = if let Some(first) = first {
        orch_db::queries::threads::input_by_client(conn, created, Some(actor), &first.client_id)
            .await?
            .map(|row| row.id)
    } else {
        None
    };
    Ok(ThreadCreateReceipt {
        projection: ReceiptProjection::Receipt,
        thread_id: created,
        topic_id: topic,
        input_id,
    })
}

/// Complete the actual created-node and independently protected live source reads
/// inside the same transaction as creation, before any content renderer runs.
pub(crate) async fn read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    creation: &Creation,
    thread: Id,
    topic_created: bool,
) -> ApiResult<Option<(Thread, Option<orch_api_contract::nodes::Topic>)>> {
    read_selected(
        ctx,
        conn,
        principal,
        ReadCreation {
            topic: creation.new.topic,
            environment: Some(creation.new.environment),
            plan: &creation.plan,
        },
        thread,
        topic_created,
    )
    .await
}

/// Built-in creation uses the same exact created-node and name-source projection, with no environment.
pub(crate) async fn read_program(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    plan: &Prepared,
    thread: Id,
) -> ApiResult<Option<Thread>> {
    read_selected(
        ctx,
        conn,
        principal,
        ReadCreation {
            topic,
            environment: None,
            plan,
        },
        thread,
        false,
    )
    .await
    .map(|shown| shown.map(|(thread, _)| thread))
}

struct ReadCreation<'a> {
    topic: Id,
    environment: Option<Id>,
    plan: &'a Prepared,
}

async fn read_selected(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    creation: ReadCreation<'_>,
    thread: Id,
    topic_created: bool,
) -> ApiResult<Option<(Thread, Option<orch_api_contract::nodes::Topic>)>> {
    if !modern(principal.caller()) {
        return super::one(ctx, conn, thread)
            .await
            .map(|value| Some((value, None)));
    }
    let mut plan = Prepared::clone(creation.plan);
    let thread_target = targets::node(conn, thread).await?;
    let topic_target = targets::node(conn, creation.topic).await?;
    if topic_created {
        plan.bind(
            slot(&plan, "created_resources")?,
            KeyTargetKind::Object(ObjectKind::Topic),
            std::slice::from_ref(&topic_target),
        )?;
        plan.bind(
            slot(&plan, "created_thread")?,
            KeyTargetKind::Object(ObjectKind::Thread),
            std::slice::from_ref(&thread_target),
        )?;
    } else {
        plan.bind(
            slot(&plan, "created_resource")?,
            KeyTargetKind::Object(ObjectKind::Thread),
            std::slice::from_ref(&thread_target),
        )?;
    }
    let mut allowed = BTreeSet::from([thread]);
    if topic_created {
        allowed.insert(creation.topic);
    }
    for (role, kind) in [
        ("creator_names_orgs", ObjectKind::Org),
        ("creator_names_projects", ObjectKind::Project),
        ("creator_names_topics", ObjectKind::Topic),
    ] {
        let mut named = Vec::new();
        for ancestor in &thread_target.node_ancestors {
            if ancestor.kind != KeyTargetKind::Object(kind)
                || ancestor.id.is_some_and(|id| allowed.contains(&id))
            {
                continue;
            }
            let target = targets::node(conn, ancestor.id.ok_or_else(ApiError::not_found)?).await?;
            if plan.permits_binding(
                principal.caller(),
                slot(&plan, role)?,
                ancestor.kind,
                std::slice::from_ref(&target),
            )? {
                allowed.insert(ancestor.id.ok_or_else(ApiError::not_found)?);
                named.push(target);
            }
        }
        bind(&mut plan, role, kind, &named)?;
    }
    let topic_sources = if topic_created {
        Some(
            crate::routes::environments::reading::topic_summary_sources(
                conn,
                principal,
                &[creation.topic],
            )
            .await?,
        )
    } else {
        None
    };
    let mut environment_ids: BTreeSet<Id> = creation.environment.into_iter().collect();
    if let Some(sources) = &topic_sources {
        environment_ids.extend(sources.bindings.iter().map(|(_, environment)| *environment));
    }
    let mut environments = Vec::new();
    for id in environment_ids {
        environments.push(
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?,
        );
    }
    bind(
        &mut plan,
        "creator_environments",
        ObjectKind::Environment,
        &environments,
    )?;
    let now = ctx.app.clock.now();
    let topic = creation.topic;
    let result = plan
        .read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                authorization::require(
                    conn,
                    reader,
                    thread,
                    authorization::exact(Permission::ThreadView),
                    Act::Use,
                    Policy::default(),
                )
                .await?;
                let shown = crate::routes::nodes::view::one_with_name_sources(
                    conn,
                    reader,
                    thread,
                    now,
                    Policy::default(),
                    &allowed,
                    None,
                )
                .await?
                .ok_or_else(ApiError::not_found)?;
                let Node::Thread(thread) = shown else {
                    return Err(ApiError::not_found());
                };
                let shown_topic = if topic_created {
                    authorization::require(
                        conn,
                        reader,
                        topic,
                        authorization::exact(Permission::TopicView),
                        Act::Use,
                        Policy::default(),
                    )
                    .await?;
                    let shown = crate::routes::nodes::view::one_with_name_sources(
                        conn,
                        reader,
                        topic,
                        now,
                        Policy::default(),
                        &allowed,
                        topic_sources.as_ref(),
                    )
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                    let Node::Topic(topic) = shown else {
                        return Err(ApiError::not_found());
                    };
                    Some(topic)
                } else {
                    None
                };
                Ok((thread, shown_topic))
            })
        })
        .await;
    match result {
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}
