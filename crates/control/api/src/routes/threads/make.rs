//! Making a thread in a topic: its binding to a provider with model and effort, an agent profile and an environment, checked for the caller by the access engine with everything missing in the refusal; the page it starts with; and what is said to it at once.
//!
//! What the request leaves out is taken from the settings of the topic: the
//! provider and the agent profile the place defaults to, the model and the
//! effort, and the environment the topic's threads share. A default the
//! caller may not use is not used; a thread made without any binding at all
//! begins blocked as unbound, and waits for one. Putting the thread on what
//! it runs on is asked of the access engine for the caller and not again: the
//! engine's own function makes the thread, with its first owner, its chat and
//! the page it starts with, in one transaction.

use super::{create_failed, one};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{clean_name, invalid, placed};
use crate::thread_access::AuthzAccess;
use orch_access_contract::pages::{PageChange, Whole};
use orch_agent_contract::descriptor::ArrangementType as AgentArrangement;
use orch_api_contract::access::{AccessPreset, ThreadAccess};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{CreateThread, NodeKind, ThreadCreate};
use orch_api_contract::threads::Thread;
use orch_engine::access::{Answer, ProviderUse, ResourceAccess};
use orch_engine::binding::{Birth, CreateThreadError, NewThread};
use orch_engine::provider_setup::Learning;
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use serde_json::json;
use std::sync::Arc;

/// The name a thread has until it is renamed, and its title is kept by
/// automation.
pub const NEW_THREAD: &str = "New thread";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(ThreadCreate, thread_create);
}

/// What a thread is bound to, once the request and the settings of the place
/// have both been heard.
struct Chosen {
    agent: Id,
    provider: Id,
    arrangement: Option<AgentArrangement>,
}

/// A word of the API's contract as the same word of the agent contract.
fn arrangement_of(word: orch_api_contract::binding::ArrangementType) -> Option<AgentArrangement> {
    serde_json::to_value(word)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

/// The agent profile and the provider a thread is bound to: the ones the
/// request names, else the ones the topic defaults to when the caller may use
/// both. None when there is no binding to make.
async fn binding_of(
    app: &crate::state::AppState,
    conn: &mut orch_db::Conn,
    topic: Id,
    person: Id,
    requested: Option<&orch_api_contract::nodes::CreateThreadBinding>,
    model: Option<&str>,
    effort: Option<&str>,
) -> ApiResult<Option<Chosen>> {
    if let Some(binding) = requested {
        return Ok(Some(Chosen {
            agent: binding.agent_id,
            provider: binding.provider_id,
            arrangement: binding.arrangement.and_then(arrangement_of),
        }));
    }
    let provider = setting::<Id>(conn, Place::Node(topic), keys::AGENT_DEFAULT_PROVIDER_ID)
        .await
        .map_err(ApiError::internal)?;
    let agent = setting::<Id>(conn, Place::Node(topic), keys::AGENT_DEFAULT_AGENT_ID)
        .await
        .map_err(ApiError::internal)?;
    let (Some(provider), Some(agent)) = (provider, agent) else {
        return Ok(None);
    };
    let access = AuthzAccess::new(Arc::clone(&app.clock));
    let may_agent = access
        .agent_use(conn, person, topic, agent)
        .await
        .map_err(ApiError::internal)?;
    let may_provider = access
        .provider_use(
            conn,
            &ProviderUse {
                person,
                node: topic,
                provider,
                model,
                effort,
            },
        )
        .await
        .map_err(ApiError::internal)?;
    if matches!(may_agent, Answer::Allowed { .. }) && matches!(may_provider, Answer::Allowed { .. })
    {
        Ok(Some(Chosen {
            agent,
            provider,
            arrangement: None,
        }))
    } else {
        Ok(None)
    }
}

/// A stale or inaccessible shared default follows the ordinary fresh-environment fallback.
async fn needs_new_default(
    app: &crate::state::AppState,
    conn: &mut orch_db::Conn,
    topic: Id,
    person: Id,
) -> ApiResult<bool> {
    let shared = setting::<String>(conn, Place::Node(topic), keys::ENV_THREAD_DEFAULT)
        .await
        .map_err(ApiError::internal)?
        .is_none_or(|value| value == "shared");
    if !shared {
        return Ok(true);
    }
    let Some(id) = setting::<Id>(conn, Place::Node(topic), keys::ENV_DEFAULT_ID)
        .await
        .map_err(ApiError::internal)?
    else {
        return Ok(true);
    };
    if !orch_db::queries::loss::accepts_threads(conn, id).await? {
        return Ok(true);
    }
    Ok(!matches!(
        AuthzAccess::new(Arc::clone(&app.clock))
            .environment_place(conn, person, id)
            .await
            .map_err(ApiError::internal)?,
        Answer::Allowed { .. }
    ))
}

/// The environment a thread is put into: the one the request names, else the
/// topic's own when threads share it and the caller manages it.
async fn environment_of(
    app: &crate::state::AppState,
    conn: &mut orch_db::Conn,
    topic: Id,
    person: Id,
    asked: Option<&EnvironmentChoice>,
) -> ApiResult<Id> {
    match asked {
        Some(EnvironmentChoice::Use(id)) => {
            if !orch_db::queries::loss::accepts_threads(conn, *id).await? {
                return Err(ApiError::new(
                    ErrorCode::EnvironmentUnavailable,
                    "The environment was lost or removed; choose another environment",
                ));
            }
            Ok(*id)
        }
        Some(EnvironmentChoice::New(_)) => Err(invalid(
            "environment.new",
            "an environment is made by its own request, and named here with `use`",
        )),
        None => {
            let shared = setting::<String>(conn, Place::Node(topic), keys::ENV_THREAD_DEFAULT)
                .await
                .map_err(ApiError::internal)?
                .is_none_or(|how| how == "shared");
            let default = if shared {
                setting::<Id>(conn, Place::Node(topic), keys::ENV_DEFAULT_ID)
                    .await
                    .map_err(ApiError::internal)?
            } else {
                None
            };
            let default = match default {
                Some(id) if orch_db::queries::loss::accepts_threads(conn, id).await? => Some(id),
                _ => None,
            };
            let Some(environment) = default else {
                return Err(ApiError::new(
                    ErrorCode::EnvironmentNoneUsable,
                    "the topic has no environment for its threads, and none was named",
                ));
            };
            let access = AuthzAccess::new(Arc::clone(&app.clock));
            match access
                .environment_place(conn, person, environment)
                .await
                .map_err(ApiError::internal)?
            {
                Answer::Allowed { .. } => Ok(environment),
                Answer::Refused(_) => Err(ApiError::new(
                    ErrorCode::EnvironmentNoneUsable,
                    "the caller manages none of the environments this topic's threads share",
                )),
            }
        }
    }
}

/// The page a thread starts with, as the rows its creator changes.
fn page_of(access: Option<&ThreadAccess>) -> ApiResult<Option<PageChange>> {
    match access {
        None | Some(ThreadAccess::Preset(AccessPreset::Defaults)) => Ok(None),
        Some(ThreadAccess::Preset(AccessPreset::Private)) => Ok(Some(PageChange {
            whole: Some(Whole::Private),
            ..PageChange::default()
        })),
        Some(ThreadAccess::Change(change)) => serde_json::to_value(change)
            .ok()
            .and_then(|value| serde_json::from_value::<PageChange>(value).ok())
            .map(Some)
            .ok_or_else(|| invalid("access", "does not read as a change of a page")),
        Some(ThreadAccess::As(_)) => Err(invalid(
            "access",
            "a page is carried over from another thread by a fork, not by making a thread",
        )),
    }
}

/// Makes a thread in a topic.
pub(crate) async fn scheduled_choices(
    app: &crate::state::AppState,
    conn: &mut orch_db::Conn,
    topic: Id,
    person: Id,
    body: &CreateThread,
    occurrence_name: &str,
) -> ApiResult<(NewThread, Birth)> {
    if body.first_input.is_some() {
        return Err(invalid(
            "target.config.first_input",
            "a schedule's content is its only first input",
        ));
    }
    let (_, kind) = placed(conn, topic).await?;
    if kind != NodeKind::Topic {
        return Err(ApiError::not_found());
    }
    let name = clean_name("name", occurrence_name)?;
    let model = match &body.model {
        Some(model) => Some(model.clone()),
        None => setting::<String>(conn, Place::Node(topic), keys::AGENT_MODEL)
            .await
            .map_err(ApiError::internal)?,
    };
    let chosen = binding_of(
        app,
        conn,
        topic,
        person,
        body.binding.as_ref(),
        model.as_deref(),
        body.effort.as_deref(),
    )
    .await?
    .ok_or_else(|| {
        ApiError::new(
            ErrorCode::BindingIncompatible,
            "no permitted agent profile and provider were named or defaulted",
        )
        .with("reason", "unbound")
    })?;
    let environment = environment_of(app, conn, topic, person, body.environment.as_ref()).await?;
    let mut worktree = body.worktree.clone();
    if let Some(choice) = &mut worktree {
        if !matches!(body.environment, Some(EnvironmentChoice::Use(_))) {
            return Err(invalid(
                "target.config.worktree",
                "an own attempt names an existing environment",
            ));
        }
        choice.repository_dir = crate::routes::workspace::paths::clean(
            "worktree.repository_dir",
            &choice.repository_dir,
            true,
        )?;
        choice.directory =
            crate::routes::workspace::paths::clean("worktree.directory", &choice.directory, false)?;
        if !orch_settings_contract::registry::is_git_branch(&choice.branch) {
            return Err(invalid("worktree.branch", "choose a valid new Git branch"));
        }
        if let Some(directory) = &body.directory
            && crate::routes::workspace::paths::clean("directory", directory, true)?
                != choice.directory
        {
            return Err(invalid(
                "directory",
                "the thread starts in its new worktree directory",
            ));
        }
    }
    let mut settings = Vec::new();
    if let Some(params) = &body.params {
        settings.push((keys::AGENT_PARAMS.to_owned(), json!(params)));
    }
    if let Some(cwd) = &body.cwd {
        settings.push((keys::THREAD_CWD.to_owned(), json!(cwd)));
    }
    Ok((
        NewThread {
            topic,
            name,
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
        },
        Birth {
            first_input: None,
            prepare_source: body.prepare_source,
            worktree,
            titled_by_automation: false,
            settings,
            page: page_of(body.access.as_ref())?,
        },
    ))
}

/// Makes a thread in a topic.
/// Called with the caller's existing guarded transaction, including for a topic and its first thread.
/// Every resource, page, job and input remains private until the caller commits the complete admission.
pub(crate) async fn create_in(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    topic: Id,
    body: &CreateThread,
) -> ApiResult<Thread> {
    let worktree = worktree_of(body)?;
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    let page = page_of(body.access.as_ref())?;
    let model = match &body.model {
        Some(model) => Some(model.clone()),
        None => setting::<String>(conn, Place::Node(topic), keys::AGENT_MODEL)
            .await
            .map_err(ApiError::internal)?,
    };
    let chosen = binding_of(
        &ctx.app,
        conn,
        topic,
        principal.user(),
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
    let fallback = if body.environment.is_none()
        && needs_new_default(&ctx.app, conn, topic, principal.user()).await?
    {
        Some(EnvironmentChoice::New(Box::new(
            orch_api_contract::binding::NewEnvironment {
                home_node_id: Some(topic),
                ..Default::default()
            },
        )))
    } else {
        None
    };
    let environment = match body.environment.as_ref().or(fallback.as_ref()) {
        Some(EnvironmentChoice::New(new)) => {
            let mut new = (**new).clone();
            let home = *new.home_node_id.get_or_insert(topic);
            if new.name.is_none() {
                let org = orch_db::queries::environments::hosting::root_org_of(conn, home)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                let topic_name = orch_db::queries::threads::node_name(conn, topic)
                    .await?
                    .unwrap_or_else(|| crate::routes::nodes::NEW_TOPIC.to_owned());
                // Preserve the previous bounded Main/topic/numbered naming choices without separate requests.
                let candidates = std::iter::once("Main".to_owned())
                    .chain(std::iter::once(topic_name.clone()))
                    .chain((2..=4).map(|number| format!("{topic_name} {number}")));
                for candidate in candidates {
                    if orch_db::queries::environments::routes::named(
                        conn,
                        principal.user(),
                        org,
                        &candidate,
                    )
                    .await?
                    .is_none()
                    {
                        new.name = Some(candidate);
                        break;
                    }
                }
                if new.name.is_none() {
                    return Err(invalid(
                        "environment.new.name",
                        "choose an unused environment name",
                    ));
                }
            }
            crate::routes::environments::hosted::create_in(ctx, conn, principal, new)
                .await?
                .id
        }
        _ => {
            environment_of(
                &ctx.app,
                conn,
                topic,
                principal.user(),
                body.environment.as_ref(),
            )
            .await?
        }
    };
    let mut settings = Vec::new();
    if let Some(params) = &body.params {
        settings.push((keys::AGENT_PARAMS.to_owned(), json!(params)));
    }
    if let Some(cwd) = &body.cwd {
        settings.push((keys::THREAD_CWD.to_owned(), json!(cwd)));
    }
    let new = NewThread {
        topic,
        name: name.unwrap_or_else(|| NEW_THREAD.to_owned()),
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
        first_input: body.first_input.clone(),
        prepare_source: body.prepare_source,
        worktree,
        titled_by_automation: body.name.is_none(),
        settings,
        page,
    };
    let made = ctx
        .app
        .engine
        .create_thread_in_transaction(
            conn,
            principal.caller(),
            &new,
            &birth,
            orch_types::new_id(),
            ctx.now,
        )
        .await;
    let made = match made {
        Ok(made) => made,
        Err(error) => return Err(create_failed(conn, error).await),
    };
    one(ctx, conn, made.thread).await
}

fn worktree_of(
    body: &CreateThread,
) -> ApiResult<Option<orch_api_contract::source::WorktreeChoice>> {
    let Some(mut choice) = body.worktree.clone() else {
        return Ok(None);
    };
    if !matches!(body.environment.as_ref(), Some(EnvironmentChoice::Use(_))) {
        return Err(ApiError::invalid(
            "worktree",
            "an own attempt names an existing environment",
        ));
    }
    choice.repository_dir = crate::routes::workspace::paths::clean(
        "worktree.repository_dir",
        &choice.repository_dir,
        true,
    )?;
    choice.directory =
        crate::routes::workspace::paths::clean("worktree.directory", &choice.directory, false)?;
    if !orch_settings_contract::registry::is_git_branch(&choice.branch) {
        return Err(ApiError::invalid(
            "worktree.branch",
            "choose a valid new Git branch",
        ));
    }
    if let Some(directory) = &body.directory
        && crate::routes::workspace::paths::clean("directory", directory, true)? != choice.directory
    {
        return Err(ApiError::invalid(
            "directory",
            "the thread starts in its new worktree directory",
        ));
    }
    Ok(Some(choice))
}

/// Makes a thread in a topic.
async fn thread_create(ctx: Ctx, (): (), mut body: CreateThread) -> ApiResult<Thread> {
    if body.environment.is_none() {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        let topic = ctx.id("id")?;
        if needs_new_default(&ctx.app, &mut conn, topic, principal.user()).await? {
            body.environment = Some(EnvironmentChoice::New(Box::new(
                orch_api_contract::binding::NewEnvironment {
                    home_node_id: Some(topic),
                    ..Default::default()
                },
            )));
        }
    }
    if matches!(body.environment.as_ref(), Some(EnvironmentChoice::New(_))) {
        let topic = ctx.id("id")?;
        let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
        let principal = guard.into_principal();
        let (_, kind) = placed(&mut tx, topic).await?;
        if kind != NodeKind::Topic {
            return Err(ApiError::not_found());
        }
        let shown = create_in(&ctx, &mut tx, &principal, topic, &body).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if body.first_input.is_some() {
            ctx.app.engine.wake(shown.base.id);
        }
        ctx.app.engine.kick();
        return Ok(shown);
    }
    let worktree = worktree_of(&body)?;
    let topic = ctx.id("id")?;
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    let page = page_of(body.access.as_ref())?;
    let caller = authorization::caller_of(ctx.user()?)?;
    let person = caller.user();
    let mut conn = ctx.app.db.acquire().await?;
    let (_, kind) = placed(&mut conn, topic).await?;
    if kind != NodeKind::Topic {
        return Err(ApiError::not_found());
    }
    let model = match &body.model {
        Some(model) => Some(model.clone()),
        None => setting::<String>(&mut conn, Place::Node(topic), keys::AGENT_MODEL)
            .await
            .map_err(ApiError::internal)?,
    };
    let chosen = binding_of(
        &ctx.app,
        &mut conn,
        topic,
        person,
        body.binding.as_ref(),
        model.as_deref(),
        body.effort.as_deref(),
    )
    .await?;
    let Some(chosen) = chosen else {
        return Err(ApiError::new(
            ErrorCode::BindingIncompatible,
            "no agent profile and provider were named, and the topic defaults to none the caller may use",
        )
        .with("reason", "unbound"));
    };
    let environment = environment_of(
        &ctx.app,
        &mut conn,
        topic,
        person,
        body.environment.as_ref(),
    )
    .await?;
    drop(conn);

    let mut settings: Vec<(String, serde_json::Value)> = Vec::new();
    if let Some(params) = &body.params {
        settings.push((keys::AGENT_PARAMS.to_owned(), json!(params)));
    }
    if let Some(cwd) = &body.cwd {
        settings.push((keys::THREAD_CWD.to_owned(), json!(cwd)));
    }
    let new = NewThread {
        topic,
        name: name.clone().unwrap_or_else(|| NEW_THREAD.to_owned()),
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
        first_input: body.first_input.clone(),
        prepare_source: body.prepare_source,
        worktree: worktree.clone(),
        titled_by_automation: name.is_none(),
        settings,
        page,
    };
    let mut discovered = false;
    let created = loop {
        // Discovery releases the transaction; every attempt checks the exact credential again.
        let (tx, guard) = transaction_guard::begin(&ctx).await?;
        let created = ctx
            .app
            .engine
            .create_thread_guarded(tx, guard.into_principal(), new.clone(), birth.clone(), None)
            .await;
        if matches!(&created, Err(CreateThreadError::AgentNotReported)) && !discovered {
            discovered = true;
            if matches!(
                ctx.app
                    .engine
                    .learn_descriptor(new.agent, new.provider, Some(new.environment))
                    .await
                    .map_err(ApiError::engine)?,
                Learning::Known
            ) {
                continue;
            }
        }
        break created;
    };
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            return Err(create_failed(&mut conn, error).await);
        }
    };
    // Creation and its optional first input have committed together. Native source/harness work
    // is driven only afterwards; a rejected input cannot leave an empty New thread behind.
    let mut conn = ctx.app.db.acquire().await?;
    one(&ctx, &mut conn, created.thread).await
}
