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

use super::create_failed;

use super::one;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::clean_name;

use crate::routes::nodes::invalid;

use crate::routes::nodes::placed;

use crate::thread_access::AuthzAccess;

use orch_access_contract::pages::PageChange;

use orch_access_contract::pages::Whole;

use orch_agent_contract::descriptor::ArrangementType as AgentArrangement;

use orch_api_contract::access::AccessPreset;

use orch_api_contract::access::ThreadAccess;

use orch_api_contract::binding::EnvironmentChoice;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::Problem;

use orch_api_contract::nodes::CreateThread;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::ThreadCreate;

use orch_api_contract::nodes::ThreadCreateResponse;

use orch_api_contract::threads::Thread;

use orch_engine::access::Answer;

use orch_engine::access::ProviderUse;

use orch_engine::access::ResourceAccess;

use orch_engine::binding::Birth;

use orch_engine::binding::CreateThreadError;

use orch_engine::binding::NewThread;

use orch_engine::provider_setup::Learning;

use orch_settings::Place;

use orch_settings::get as setting;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Json;

use serde_json::json;

use std::sync::Arc;

pub use orch_api_base::routes::threads::make::NEW_THREAD;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(ThreadCreate, super::creation_replay::binding())
        .on_with_optional_initial_operation(
            ThreadCreate,
            super::creator_discovery::initial,
            super::creation_operation::thread,
            thread_create,
        );
}

/// What a thread is bound to, once the request and the settings of the place
/// have both been heard.
pub(super) struct Chosen {
    pub agent: Id,
    pub provider: Id,
    pub arrangement: Option<AgentArrangement>,
}

/// A word of the API's contract as the same word of the agent contract.
pub(super) fn arrangement_of(
    word: orch_api_contract::binding::ArrangementType,
) -> Option<AgentArrangement> {
    serde_json::to_value(word)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

pub(super) fn requires_birth(body: &CreateThread) -> bool {
    body.environment
        .as_ref()
        .is_some_and(EnvironmentChoice::requires_birth)
        || body
            .binding
            .as_ref()
            .is_some_and(|binding| binding.target.single_provider().is_none())
}

/// Materialize an inherited target in the same shape as an explicitly selected binding.
/// A concrete default provider keeps the ordinary discovery and permission checks.
pub(super) async fn with_target_default<'a>(
    conn: &mut orch_db::Conn,
    topic: Id,
    body: &'a CreateThread,
) -> ApiResult<std::borrow::Cow<'a, CreateThread>> {
    use orch_provider_contract::pools::Target;
    use orch_settings_contract::thread_defaults::ModelDefault;
    if body.program.is_some() {
        return Ok(std::borrow::Cow::Borrowed(body));
    }
    let definitions = [
        keys::AGENT_MODEL,
        keys::AGENT_DEFAULT_PROVIDER_ID,
        keys::AGENT_DEFAULT_AGENT_ID,
    ]
    .map(|key| {
        orch_settings_contract::registry::get(key).ok_or_else(|| {
            ApiError::internal("The thread default is missing from the settings registry")
        })
    });
    let [model, provider, agent] = definitions;
    let definitions = [model?, provider?, agent?];
    let settings = orch_settings::Snapshot::load(
        conn,
        Place::Node(topic),
        orch_settings::Wanted::Keys(&definitions),
    )
    .await
    .map_err(ApiError::internal)?;
    let model_setting = settings.resolve(definitions[0]);
    let provider_setting = settings.resolve(definitions[1]);
    let model = model_setting
        .value
        .filter(|value| !value.is_null())
        .map(serde_json::from_value::<ModelDefault>)
        .transpose()
        .map_err(ApiError::internal)?;
    let provider = provider_setting
        .value
        .filter(|value| !value.is_null())
        .map(serde_json::from_value::<Target>)
        .transpose()
        .map_err(ApiError::internal)?;
    if let Some(binding) = body.binding.as_ref() {
        if provider_setting.locked_by.is_some()
            && let Some(fixed) = &provider
            && fixed != &binding.target
            && !(fixed.single_provider().is_some()
                && binding
                    .target
                    .validate(orch_provider_contract::pools::Kind::Model)
                    .is_ok())
        {
            return Err(invalid(
                "binding.target",
                "The locked provider default decides this thread’s provider.",
            ));
        }
        if model_setting.locked_by.is_some()
            && matches!(&model, Some(ModelDefault::Target(target)) if target != &binding.target
                && !(matches!(target, Target::Single { .. }) && matches!(&binding.target, Target::Pool { .. })))
        {
            return Err(invalid(
                "binding.target",
                "The locked model default decides this thread’s model.",
            ));
        }
        return Ok(std::borrow::Cow::Borrowed(body));
    }
    let target = match model {
        Some(ModelDefault::Target(target))
            if body.model.is_none() || model_setting.locked_by.is_some() =>
        {
            Some(target)
        }
        _ => provider.filter(|target| matches!(target, Target::Pool { .. })),
    };
    let Some(target) = target else {
        return Ok(std::borrow::Cow::Borrowed(body));
    };
    let agent_id = settings
        .resolve(definitions[2])
        .value
        .filter(|value| !value.is_null())
        .map(serde_json::from_value::<Id>)
        .transpose()
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            invalid(
                "binding.agent_id",
                "Choose the agent to use with the default pool.",
            )
        })?;
    let mut body = body.clone();
    body.binding = Some(orch_api_contract::nodes::CreateThreadBinding {
        agent_id,
        target,
        arrangement: None,
    });
    Ok(std::borrow::Cow::Owned(body))
}

/// The agent profile and the provider a thread is bound to: the ones the
/// request names, else the ones the topic defaults to when the caller may use
/// both. None when there is no binding to make.
pub(super) async fn binding_of(
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
            provider: binding.target.single_provider().ok_or_else(|| {
                invalid(
                    "binding.target",
                    "This creation requires a concrete provider",
                )
            })?,
            arrangement: binding.arrangement.and_then(arrangement_of),
        }));
    }
    let provider = orch_settings::thread_provider(conn, Place::Node(topic))
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
                pool: None,
                purpose: orch_access_contract::permissions::UsagePurpose::Turn,
                person,
                node: topic,
                provider,
                model,
                effort,
                // The environment is asked about when the thread is made in it.
                runs_in: None,
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

/// An implicit environment's availability failure keeps its cause under the thread's refusal.
fn implicit_environment_error(mut error: ApiError) -> ApiError {
    let extra = &error.problem.extra;
    let unavailable = match error.code() {
        ErrorCode::PlanBlocked => {
            matches!(extra.get("reason").and_then(Json::as_str), Some("no_host"))
        }
        ErrorCode::RequestInvalid => {
            extra
                .get("errors")
                .and_then(Json::as_array)
                .is_some_and(|errors| {
                    !errors.is_empty()
                        && errors.iter().all(|fault| {
                            fault.get("path").and_then(Json::as_str) == Some("template_id")
                        })
                })
        }
        _ => false,
    };
    if !unavailable {
        return error;
    }
    let Problem { detail, extra, .. } = *error.problem;
    let mut problem = Problem::new(ErrorCode::EnvironmentNoneUsable, detail);
    problem.extra = extra;
    error.problem = Box::new(problem);
    error
}

/// The environment a thread is put into: the one the request names, else the
/// topic's own when threads share it and the caller manages it.
pub(super) async fn environment_of(
    app: &crate::state::AppState,
    conn: &mut orch_db::Conn,
    topic: Id,
    person: Id,
    asked: Option<&EnvironmentChoice>,
) -> ApiResult<Id> {
    match asked {
        Some(EnvironmentChoice::Use(target)) => {
            let id = target.single_environment().ok_or_else(|| {
                invalid(
                    "environment.use",
                    "Choose a single existing environment for this operation",
                )
            })?;
            if !orch_db::queries::loss::accepts_threads(conn, id).await? {
                return Err(ApiError::new(
                    ErrorCode::EnvironmentUnavailable,
                    "The environment was lost or removed; choose another environment",
                ));
            }
            Ok(id)
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
pub(crate) fn page_of(access: Option<&ThreadAccess>) -> ApiResult<Option<PageChange>> {
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
orch_api_base::__api_bind_5ad940dfb4!();

/// Makes a thread in a topic.
pub(crate) async fn scheduled_choices(
    app: &crate::state::AppState,
    conn: &mut orch_db::Conn,
    topic: Id,
    person: Id,
    body: &CreateThread,
    occurrence_name: &str,
) -> ApiResult<(NewThread, Birth)> {
    if body.program.is_some() {
        return Err(invalid(
            "target.config.program",
            "An interactive program starts from a person's current request",
        ));
    }
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
        None => orch_settings::thread_model(conn, Place::Node(topic))
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
            model_pool: None,
            let_into_environment: body.let_into_environment.unwrap_or(false),
            agent_access: body.agent_access.into(),
            first_input: None,
            prepare_source: body.prepare_source,
            workspace_edits: body.workspace_edits.clone(),
            worktree,
            titled_by_automation: false,
            settings,
            page: page_of(body.access.as_ref())?,
        },
    ))
}
orch_api_base::__api_bind_8a3eb8d954!();

async fn create_workspace(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    topic: Id,
    mut new: orch_api_contract::binding::NewEnvironment,
    creator_prefix: bool,
    recipe: Option<orch_engine::binding::environment_pool::Recipe>,
) -> ApiResult<(Id, (Id, Id))> {
    let home = *new.home_node_id.get_or_insert(topic);
    if new.name.is_none() {
        let org = orch_db::queries::environments::hosting::root_org_of(conn, home)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let topic_name = orch_db::queries::threads::node_name(conn, topic)
            .await?
            .unwrap_or_else(|| crate::routes::nodes::NEW_TOPIC.to_owned());
        new.name = Some(
            orch_db::queries::environments::routes::next_workspace_name(
                conn,
                principal.user(),
                org,
                &topic_name,
            )
            .await?,
        );
    }
    let made = if creator_prefix {
        super::creation_operation::environment_prefix(ctx, conn, principal, topic, new, recipe)
            .await?
    } else {
        crate::routes::environments::hosted::create_with_prepared_initial(
            ctx, conn, principal, new, None, recipe,
        )
        .await?
    };
    Ok((made.id, (home, made.job.id)))
}

/// Makes a thread in a topic.
/// Called with the caller's existing guarded transaction, including for a topic and its first thread.
/// Every resource, page, job and input remains private until the caller commits the complete admission.
pub(crate) async fn resolve_in(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    topic: Id,
    body: &CreateThread,
    creator_prefix: bool,
    thread: Id,
) -> ApiResult<(NewThread, Birth, Option<(Id, Id)>)> {
    if body.program.is_some() {
        return Err(invalid(
            "first_thread.program",
            "Create an interactive program in an existing topic",
        ));
    }
    let worktree = worktree_of(body)?;
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    let page = page_of(body.access.as_ref())?;
    let model = match &body.model {
        Some(model) => Some(model.clone()),
        None => orch_settings::thread_model(conn, Place::Node(topic))
            .await
            .map_err(ApiError::internal)?,
    };
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
    let mut new_environment_home = None;
    use orch_provider_contract::pools::Target as ResourceTarget;
    let requested_environment = body.environment.as_ref().or(fallback.as_ref());
    let environment_pool = match requested_environment {
        Some(EnvironmentChoice::Use(target @ ResourceTarget::Pool { .. })) => Some(target),
        _ => None,
    };
    let concrete_environment = match requested_environment {
        Some(EnvironmentChoice::Use(ResourceTarget::Pool { .. })) => None,
        Some(EnvironmentChoice::New(new)) => {
            let made = create_workspace(
                ctx,
                conn,
                principal,
                topic,
                (**new).clone(),
                creator_prefix,
                None,
            )
            .await;
            let made = made.map_err(|error| {
                if fallback.is_some() {
                    implicit_environment_error(error)
                } else {
                    error
                }
            })?;
            new_environment_home = Some(made.1);
            Some(made.0)
        }
        _ => Some(
            environment_of(
                &ctx.app,
                conn,
                topic,
                principal.user(),
                body.environment.as_ref(),
            )
            .await?,
        ),
    };
    let (chosen, selected_model, provider_selection) = if let Some(binding) = body
        .binding
        .as_ref()
        .filter(|b| b.target.single_provider().is_none())
    {
        use orch_engine::binding::model_pool::{Placement, Request, choose};
        let placement = match (concrete_environment, environment_pool) {
            (Some(id), _) => Placement::Environment(id),
            (_, Some(ResourceTarget::Pool { id, .. })) => Placement::Pool(*id),
            _ => return Err(invalid("environment", "Choose where this thread runs")),
        };
        let arrangement = binding.arrangement.and_then(arrangement_of);
        let selected = match choose(
            conn,
            &ctx.app.engine,
            principal,
            Request {
                thread,
                topic,
                agent: binding.agent_id,
                model: model.as_deref(),
                effort: body.effort.as_deref(),
                arrangement,
                placement,
                target: &binding.target,
            },
        )
        .await
        {
            Ok(Some(selected)) => selected,
            Ok(None) => {
                return Err(ApiError::new(
                    ErrorCode::BindingIncompatible,
                    "No available member supports this agent, model and placement.",
                )
                .with("reason", "pool_exhausted"));
            }
            Err(error) => return Err(create_failed(conn, error).await),
        };
        (
            Chosen {
                agent: binding.agent_id,
                provider: selected.provider,
                arrangement,
            },
            Some(selected.model),
            selected.pool,
        )
    } else {
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
        (chosen, body.model.clone(), None)
    };
    let environment =
        if let Some(target) = environment_pool {
            target
                .validate(orch_provider_contract::pools::Kind::Environment)
                .map_err(|reason| invalid("environment.use", &reason))?;
            use orch_engine::binding::environment_pool::{Request, choose};
            match choose(
                conn,
                &ctx.app.engine,
                principal,
                Request {
                    thread,
                    topic,
                    target,
                    agent: chosen.agent,
                    provider: chosen.provider,
                    model: selected_model.as_deref(),
                    provider_selection: provider_selection.clone(),
                    effort: body.effort.as_deref(),
                    arrangement: chosen.arrangement,
                },
            )
            .await
            {
                Ok(Some(orch_engine::binding::environment_pool::Selected::Existing(
                    environment,
                ))) => environment,
                Ok(Some(orch_engine::binding::environment_pool::Selected::New(recipe))) => {
                    let new = orch_api_contract::binding::NewEnvironment {
                        template_id: Some(recipe.template()),
                        host_id: Some(Some(ResourceTarget::host(recipe.host()))),
                        ..Default::default()
                    };
                    let (id, registration) = create_workspace(
                        ctx,
                        conn,
                        principal,
                        topic,
                        new,
                        creator_prefix,
                        Some(recipe),
                    )
                    .await?;
                    new_environment_home = Some(registration);
                    id
                }
                Ok(None) => return Err(ApiError::new(
                    ErrorCode::EnvironmentNoneUsable,
                    "No available member of this environment pool accepts the selected binding.",
                )
                .with("reason", "pool_exhausted")),
                Err(error) => return Err(create_failed(conn, error).await),
            }
        } else {
            concrete_environment
                .ok_or_else(|| invalid("environment", "Choose where this thread runs"))?
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
        model: selected_model,
        effort: body.effort.clone(),
    };
    let birth = Birth {
        model_pool: provider_selection,
        let_into_environment: body.let_into_environment.unwrap_or(false),
        agent_access: body.agent_access.into(),
        first_input: body.first_input.clone(),
        prepare_source: body.prepare_source,
        workspace_edits: body.workspace_edits.clone(),
        worktree,
        titled_by_automation: body.name.is_none(),
        settings,
        page,
    };
    Ok((new, birth, new_environment_home))
}

pub(crate) async fn create_in(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    topic: Id,
    body: &CreateThread,
) -> ApiResult<Thread> {
    let thread = orch_types::new_id();
    let body = with_target_default(conn, topic, body).await?;
    let (new, birth, _) = resolve_in(ctx, conn, principal, topic, &body, false, thread).await?;
    let made = ctx
        .app
        .engine
        .create_thread_in_transaction(conn, principal.caller(), &new, &birth, thread, ctx.now)
        .await;
    let made = match made {
        Ok(made) => made,
        Err(error) => return Err(create_failed(conn, error).await),
    };
    one(ctx, conn, made.thread).await
}
orch_api_base::__api_bind_db6ac243da!();

pub(super) fn worktree_of(
    body: &CreateThread,
) -> ApiResult<Option<orch_api_contract::source::WorktreeChoice>> {
    orch_engine::jobs::source::validate_workspace_edits(&body.workspace_edits)
        .map_err(|why| ApiError::invalid("workspace_edits", why))?;
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
async fn thread_create(ctx: Ctx, (): (), body: CreateThread) -> ApiResult<ThreadCreateResponse> {
    if body.program.is_some() {
        return crate::ask::birth::create(ctx, body).await;
    }
    if authorization::caller_of(ctx.user()?)?
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
    {
        return create_scoped(ctx, body).await;
    }
    thread_create_ordinary(ctx, body)
        .await
        .map(ThreadCreateResponse::full)
}

async fn create_scoped(ctx: Ctx, body: CreateThread) -> ApiResult<ThreadCreateResponse> {
    let (ctx, mut tx, guard) = if ctx.topic_creation_prefix().is_some() {
        crate::key_operations::creation_stage::begin_thread(&ctx, &body).await?
    } else {
        let (tx, guard) = transaction_guard::begin(&ctx).await?;
        (ctx, tx, guard)
    };
    let creation = guard.operation_data::<super::creation_operation::Creation>()?;
    let principal = guard.into_principal();
    let created = if let Some(created) = &creation.created {
        created.clone()
    } else {
        let created = ctx
            .app
            .engine
            .create_thread_in_transaction(
                &mut tx,
                principal.caller(),
                &creation.new,
                &creation.birth,
                orch_types::new_id(),
                principal.now().get(),
            )
            .await;

        match created {
            Ok(created) => created,
            Err(error) => return Err(create_failed(&mut tx, error).await),
        }
    };
    let receipt = super::creation_operation::receipt(
        &mut tx,
        principal.user(),
        created.thread,
        creation.new.topic,
        body.first_input.as_ref(),
    )
    .await?;
    let shown = super::creation_operation::read(
        &ctx,
        &mut tx,
        &principal,
        &creation,
        created.thread,
        false,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if body.first_input.is_some() {
        ctx.app.engine.wake(created.thread);
    }
    ctx.app.engine.kick();
    Ok(match shown {
        Some((thread, _)) if !ctx.key_receipt_required() => ThreadCreateResponse::full(thread),
        _ => ThreadCreateResponse::Receipt(receipt),
    })
}

async fn thread_create_ordinary(ctx: Ctx, body: CreateThread) -> ApiResult<Thread> {
    let topic = ctx.id("id")?;
    let staged = if requires_birth(&body) {
        true
    } else if body.binding.is_none() || body.environment.is_none() {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        requires_birth(with_target_default(&mut conn, topic, &body).await?.as_ref())
            || (body.environment.is_none()
                && needs_new_default(&ctx.app, &mut conn, topic, principal.user()).await?)
    } else {
        false
    };
    if staged {
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
        None => orch_settings::thread_model(&mut conn, Place::Node(topic))
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
        model_pool: None,
        let_into_environment: body.let_into_environment.unwrap_or(false),
        agent_access: body.agent_access.into(),
        first_input: body.first_input.clone(),
        prepare_source: body.prepare_source,
        workspace_edits: body.workspace_edits.clone(),
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
            match ctx
                .app
                .engine
                .learn_descriptor(new.agent, new.provider, Some(new.environment))
                .await
                .map_err(ApiError::engine)?
            {
                Learning::Known => continue,
                Learning::Unknown(orch_engine::provider_setup::Unlearned::Policy(key)) => {
                    return Err(super::harness_start_failed(Some(&key)));
                }
                Learning::Unknown(orch_engine::provider_setup::Unlearned::Failed(_)) => {
                    return Err(super::harness_start_failed(None));
                }
                Learning::Unknown(_) => {}
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
