//! Built-in sources enter the same native target planner without a fictitious workspace.

use super::*;
use orch_api_contract::threads::BuiltinThreadProgram;
use orch_db::queries::ask::ProgramBinding;

pub(super) struct Request<'a> {
    pub clock: &'a dyn orch_types::Clock,
    pub access: &'a dyn crate::access::ResourceAccess,
    pub principal: &'a Principal,
    pub thread: ThreadRow,
    pub binding: ProgramBinding,
    pub request: &'a ForkRequest,
    pub reach: Reach,
    pub own_batch: Option<Id>,
    pub agent_access: Option<binding::BirthAgentAccess>,
}

/// Actual selected execution, independent of a later default-model setting.
pub(crate) async fn execution(
    conn: &mut Conn,
    thread: &ThreadRow,
) -> Result<Option<(BuiltinThreadProgram, ProviderRow, String)>, PlanError> {
    let Some(binding) = orch_db::queries::ask::program_binding(conn, thread.node_id).await? else {
        return Ok(None);
    };
    let recorded = match thread.current_segment_id {
        Some(segment) => {
            orch_db::queries::ask::segment_binding(conn, thread.node_id, segment).await?
        }
        None => None,
    };
    let selected = match recorded {
        Some(recorded) => Some((recorded.provider_id, recorded.model)),
        None => binding
            .model
            .map(serde_json::from_value::<orch_api_contract::ask::AskModel>)
            .transpose()?
            .map(|model| (model.provider_id, model.model)),
    };
    let (provider, model) = selected.ok_or_else(|| {
        PlanError::invalid(
            "source",
            "The built-in program has not selected a provider and model yet",
        )
    })?;
    if thread.agent_id.is_some()
        || thread.environment_id.is_some()
        || thread.provider_id.is_some_and(|actual| actual != provider)
    {
        return Err(PlanError::invalid(
            "source",
            "The program's actual binding changed",
        ));
    }
    let provider = resources::provider(conn, provider)
        .await?
        .filter(|provider| provider.deleted_at.is_none())
        .ok_or_else(|| PlanError::invalid("source", "The program's provider is unavailable"))?;
    Ok(Some((
        BuiltinThreadProgram::orch(binding.root_org_id),
        provider,
        model,
    )))
}

/// A confirmed fork keeps its actual source segment even while the original goes on.
pub(crate) async fn execution_at(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
) -> Result<Option<(BuiltinThreadProgram, ProviderRow, String)>, PlanError> {
    let Some(binding) = orch_db::queries::ask::segment_binding(conn, thread, segment).await? else {
        return Ok(None);
    };
    let root = orch_db::queries::ask::segment_root(conn, segment)
        .await?
        .ok_or_else(|| {
            PlanError::invalid(
                "source",
                "The captured program's knowledge domain is unavailable",
            )
        })?;
    let provider = resources::provider(conn, binding.provider_id)
        .await?
        .filter(|provider| provider.deleted_at.is_none())
        .ok_or_else(|| {
            PlanError::invalid("source", "The captured program's provider is unavailable")
        })?;
    Ok(Some((
        BuiltinThreadProgram::orch(root),
        provider,
        binding.model,
    )))
}

fn validate(input: &Request<'_>) -> Result<(), PlanError> {
    if input.reach != Reach::Seen {
        return Err(PlanError::invalid(
            "source",
            "A built-in program has no environment to manage",
        ));
    }
    for (name, value) in [
        ("directory", &input.request.directory),
        ("cwd", &input.request.cwd),
    ] {
        if value
            .as_deref()
            .is_some_and(|value| !binding::directory_is_valid(value))
        {
            return Err(PlanError::invalid(
                name,
                "is relative to the workspace root and does not go up",
            ));
        }
    }
    if input
        .request
        .name
        .as_deref()
        .is_some_and(|name| name.trim().is_empty() || name.chars().count() > NAME_MAX_CHARS)
    {
        return Err(PlanError::invalid(
            "name",
            format!("is between 1 and {NAME_MAX_CHARS} characters"),
        ));
    }
    let carry = carry_of(input.request);
    if input.request.worktree.is_some()
        || input.request.files_at.is_some()
        || carry.home
        || carry
            .selection
            .as_ref()
            .is_some_and(|files| !matches!(files, FileSelection::Nothing))
        || input
            .request
            .mounts
            .as_ref()
            .is_some_and(|mounts| !mounts.is_empty())
    {
        return Err(PlanError::invalid(
            "environment",
            "The source program has no workspace files, worktree, home or mounts; conversation attachments travel with its history",
        ));
    }
    if input.request.result == ForkResult::Replace && input.request.agent_access.is_some() {
        return Err(PlanError::invalid(
            "agent_access",
            "a switch keeps its existing agent contributions",
        ));
    }
    Ok(())
}

async fn landing(
    conn: &mut Conn,
    input: &Request<'_>,
    blockers: &mut Blockers,
) -> Result<Landing, PlanError> {
    match &input.request.environment {
        Some(ForkEnvironment::Existing { environment_id, .. }) => {
            let row = environments::environment(conn, *environment_id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or_else(|| {
                    PlanError::invalid("environment", "the destination environment is not there")
                })?;
            if row.status == "disabled" {
                blockers.add(
                    code::ENVIRONMENT_DISABLED,
                    "the environment is switched off",
                );
            }
            if matches!(row.status.as_str(), "lost" | "removed") {
                blockers.add(
                    code::UNAVAILABLE,
                    "The destination environment was lost or removed",
                );
            }
            if row.remove_at.is_some() {
                blockers.add(
                    code::REMOVAL_SCHEDULED,
                    "The destination environment's removal is scheduled",
                );
            }
            crate::jobs::environment_move::preparation::select_destinations(
                conn,
                &[(
                    orch_access_contract::decisions::ObjectRef {
                        kind: orch_access_contract::permissions::ObjectKind::Environment,
                        id: row.id,
                    },
                    Permission::EnvironmentManage,
                )],
            )
            .await?;
            Ok(Landing::Existing(Box::new(row)))
        }
        Some(ForkEnvironment::New { new }) => {
            let topic = input.request.topic_id.unwrap_or(input.thread.topic_id);
            let (host, promised) = choose_host(
                conn,
                input.principal,
                new,
                topic,
                input.clock.now(),
                blockers,
            )
            .await?;
            Ok(Landing::New {
                name: new
                    .name
                    .clone()
                    .unwrap_or_else(|| crate::environments::DEFAULT_NAME.into()),
                host,
                promised,
            })
        }
        _ => Err(PlanError::invalid(
            "environment",
            "Choose an existing or new destination environment; the source program has none",
        )),
    }
}

struct ModelChoice {
    provider: ProviderRow,
    model: String,
    effort: Option<String>,
    entry: Option<Vec<AgentArrangement>>,
}

async fn model(
    conn: &mut Conn,
    input: &Request<'_>,
    source: &ProgramSource,
    landing: &Landing,
    blockers: &mut Blockers,
) -> Result<ModelChoice, PlanError> {
    let to = input.request.to.as_ref();
    let id = to
        .and_then(|to| to.provider_id)
        .unwrap_or(source.provider.id);
    let provider = resources::provider(conn, id)
        .await?
        .filter(|provider| provider.deleted_at.is_none())
        .ok_or_else(|| {
            PlanError::invalid("to.provider_id", "the destination provider is not there")
        })?;
    let model = to
        .and_then(|to| to.model.clone())
        .unwrap_or_else(|| source.model.clone());
    let mut effort = to
        .and_then(|to| to.effort.clone())
        .or_else(|| source.settings.effort.clone());
    let found = resources::model(conn, id, &model).await?;
    if crate::models::check_allowed(
        found.as_ref(),
        source.settings.models_allowed.as_deref(),
        &model,
    )
    .is_err()
    {
        blockers.add(
            code::MODEL_NOT_ALLOWED,
            "The model is unavailable or excluded by this place's policy",
        );
    }
    if let (Some(wanted), Some(found)) = (&effort, &found) {
        let parameters = binding::model_parameters(found)?;
        if !parameters.efforts().contains(wanted) {
            if to.is_some_and(|to| to.effort.is_some()) {
                blockers.add(
                    code::EFFORT_NOT_LISTED,
                    "The model does not take the requested effort",
                );
            } else {
                effort = parameters.default_effort;
            }
        }
    }
    let place = if input.request.result == ForkResult::Replace {
        input.thread.node_id
    } else {
        input.request.topic_id.unwrap_or(input.thread.topic_id)
    };
    if let Some(why) = crate::provider_policy::forbidden(conn, place, id).await? {
        blockers.add("provider_not_allowed", why.to_string());
    }
    if provider.status == "disabled" {
        blockers.add(code::PROVIDER_DISABLED, "the provider is switched off");
    } else if !crate::providers::request_status_available(&provider.status) {
        blockers.add(
            code::PROVIDER_NOT_READY,
            "The destination provider is not ready",
        );
    }
    if !input.principal.caller().permits(Permission::ProviderUse) {
        blockers.add(
            code::ACCESS,
            "this credential does not include provider.use for the requested new binding",
        );
    }
    let answer = input
        .access
        .provider_use(
            conn,
            &ProviderUse {
                person: input.principal.user(),
                node: place,
                provider: id,
                model: Some(&model),
                effort: effort.as_deref(),
                runs_in: landing.runs_in(None),
            },
        )
        .await
        .map_err(EngineError::from)?;
    let entry = match answer {
        Answer::Allowed { arrangements } => arrangements,
        Answer::Refused(missing) => {
            blockers.lack(
                if missing.iter().any(Missing::leaves_out_the_place) {
                    "You may not use the destination provider where the result would run"
                } else {
                    "You may not use the destination provider, model or effort"
                },
                missing,
            );
            None
        }
    };
    Ok(ModelChoice {
        provider,
        model,
        effort,
        entry,
    })
}

async fn rights(
    conn: &mut Conn,
    input: &Request<'_>,
    target: &Target,
    blockers: &mut Blockers,
) -> Result<(), PlanError> {
    let replace = input.request.result == ForkResult::Replace;
    let principal = input.principal;
    let place = if replace {
        input.thread.node_id
    } else {
        target.topic
    };
    if replace {
        if let Some(missing) = node_lacks(
            conn,
            principal,
            input.thread.node_id,
            Permission::ThreadManage,
        )
        .await?
        {
            blockers.lack("you may not manage this thread", missing);
        }
    } else {
        if node_lacks(
            conn,
            principal,
            input.thread.node_id,
            Permission::ThreadManage,
        )
        .await?
        .is_some()
            && let Some(missing) = node_lacks(
                conn,
                principal,
                input.thread.node_id,
                Permission::ThreadFork,
            )
            .await?
        {
            blockers.lack("you may neither fork nor manage this thread", missing);
        }
        if let Some(missing) =
            node_lacks(conn, principal, target.topic, Permission::TopicStartThreads).await?
        {
            blockers.lack("you may not start a thread in that topic", missing);
        }
        if let Some(orch_api_contract::access::ThreadAccess::As(page)) = &input.request.access
            && page.source != input.thread.node_id
            && let Some(missing) =
                node_lacks(conn, principal, page.source, Permission::ThreadManage).await?
        {
            blockers.lack("you may not carry over the page of that thread", missing);
        }
    }
    if !principal.caller().permits(Permission::AgentUse) {
        blockers.add(
            code::ACCESS,
            "this credential does not include agent.use for the requested new binding",
        );
    }
    if let Answer::Refused(missing) = input
        .access
        .agent_use(conn, principal.user(), place, target.agent.id)
        .await
        .map_err(EngineError::from)?
    {
        blockers.lack("you may not use the destination agent profile", missing);
    }
    if matches!(target.landing, Landing::Existing(_))
        && let Decision::Refused(refusal) = orch_authz::environments::check_move(
            conn,
            principal,
            &orch_authz::environments::Moving {
                thread: replace.then_some(input.thread.node_id),
                from: None,
                to: target.environment_id(),
                all_of_the_environment: false,
            },
            orch_authz::Policy::default(),
        )
        .await?
    {
        blockers.lack("you may not decide where this thread runs", refusal.missing);
    }
    Ok(())
}

async fn busy(
    conn: &mut Conn,
    input: &Request<'_>,
    target: &Target,
    blockers: &mut Blockers,
) -> Result<(), PlanError> {
    if let Some(environment) = target.environment_id()
        && (fork_rows::open_touching_environment(conn, environment)
            .await?
            .iter()
            .any(|job| input.own_batch.is_none_or(|own| job.batch_id != Some(own)))
            || fork_rows::environment_is_being_changed(conn, environment)
                .await?
                .is_some())
    {
        blockers.add(
            code::ALREADY_RUNNING,
            "another job is changing the destination environment",
        );
    }
    if crate::locks::held_by(conn, input.thread.node_id)
        .await?
        .is_some()
        || orch_db::queries::agent_history::removing(conn, input.thread.node_id).await?
    {
        blockers.add(
            code::ALREADY_RUNNING,
            "a fork or a switch holds this thread already",
        );
    }
    if crate::background_wake::running(conn, Some(input.thread.node_id), None, None).await? {
        blockers.add(
            "background_tasks_running",
            "Review and end the running background tasks before forking or switching this session.",
        );
    }
    Ok(())
}

async fn target(
    conn: &mut Conn,
    input: &Request<'_>,
    source: &ProgramSource,
    blockers: &mut Blockers,
) -> Result<(Target, Vec<binding::ArrangementOption>, ApprovalMode), PlanError> {
    let landing = landing(conn, input, blockers).await?;
    let choice = model(conn, input, source, &landing, blockers).await?;
    let (environment, promised) = match &landing {
        Landing::Existing(environment) => (Some(environment.as_ref()), None),
        Landing::New { promised, .. } => (None, Some(promised)),
        Landing::Same => {
            return Err(PlanError::invalid(
                "environment",
                "A program has no source environment",
            ));
        }
    };
    let scope = if input.request.result == ForkResult::Replace {
        input.thread.node_id
    } else {
        input.request.topic_id.unwrap_or(input.thread.topic_id)
    };
    let (agent, descriptor) = choose_agent(
        conn,
        None,
        scope,
        input.request,
        &choice.provider,
        environment,
        promised,
        choice.entry.clone(),
        &source.settings,
        blockers,
    )
    .await?;
    let offers = match environment {
        Some(environment) => binding::environment_offers(conn, environment).await?,
        None => binding::EnvironmentOffers::default(),
    };
    let facts = Facts {
        descriptor: &descriptor,
        provider: &choice.provider,
        environment,
        offers: promised.map_or(offers.offers, |promised| promised.offers.clone()),
        colocated: promised.map_or(offers.colocated, |promised| promised.colocated.clone()),
        settings: &source.settings,
        entry: choice.entry,
        runtime: binding::runtime_facts_for_target(conn, &agent, &choice.provider, environment)
            .await?,
    };
    let place = match promised {
        Some(promised) => promised.place(&descriptor),
        None => binding::initial_place(conn, &facts).await?,
    };
    let machine = binding::machine_arrangements_of(conn, &choice.provider).await?;
    let requested = input
        .request
        .to
        .as_ref()
        .and_then(|to| to.arrangement)
        .map(|word| convert(&word))
        .transpose()?;
    let options = binding::options_placed(&facts, &place, machine.clone());
    let arrangement = match binding::choose_placed(&facts, &place, machine, requested) {
        Ok(choice) => Some(choice),
        Err(why) => {
            blockers.add(code::NO_ARRANGEMENT, binding::sentence(&why));
            None
        }
    };
    let approval_mode = binding::offered_approval_mode(&descriptor, source.settings.approval_mode)
        .unwrap_or_else(|why| {
            blockers.add("approval_mode_unsupported", why);
            source.settings.approval_mode
        });
    if !source.settings.allowed_modes.contains(&approval_mode) {
        blockers.add(
            "approval_mode_not_allowed",
            crate::sessions::APPROVAL_NOT_ALLOWED,
        );
    }
    if approval_mode != source.settings.approval_mode {
        let configured =
            orch_settings::resolve(conn, SettingsPlace::Node(scope), keys::APPROVAL_MODE).await?;
        if !configured.open_to_write()
            || (input.request.result == ForkResult::New && configured.resolved.locked_by.is_some())
        {
            blockers.add(
                "approval_mode_locked",
                "The place locks an approval mode the destination program does not offer",
            );
        }
    }
    Ok((
        Target {
            agent,
            provider: choice.provider,
            descriptor,
            model: choice.model,
            effort: choice.effort,
            arrangement,
            landing,
            directory: input.request.directory.clone().unwrap_or_default(),
            topic: input.request.topic_id.unwrap_or(input.thread.topic_id),
        },
        options,
        approval_mode,
    ))
}

pub(super) async fn prepare(
    conn: &mut Conn,
    input: Request<'_>,
) -> Result<PreparedFork, PlanError> {
    validate(&input)?;
    let now = input.clock.now();
    let (program, provider, model) = execution(conn, &input.thread)
        .await?
        .ok_or_else(|| PlanError::invalid("source", "The built-in binding is no longer there"))?;
    if program.root_org_id != input.binding.root_org_id {
        return Err(PlanError::invalid(
            "source",
            "The program's knowledge domain changed",
        ));
    }
    let scope = if input.request.result == ForkResult::Replace {
        input.thread.node_id
    } else {
        input.request.topic_id.unwrap_or(input.thread.topic_id)
    };
    let source = ProgramSource {
        thread: input.thread.clone(),
        program,
        provider,
        model,
        settings: resolved::load(conn, scope).await?,
    };
    let mut blockers = Blockers::default();
    let (target, options_listed, approval_mode) =
        target(conn, &input, &source, &mut blockers).await?;
    rights(conn, &input, &target, &mut blockers).await?;
    busy(conn, &input, &target, &mut blockers).await?;
    let history = match input.request.history.clone() {
        Some(history) => history,
        None => orch_api_contract::forks::ForkHistory {
            mode: default_history_mode(conn, source.thread.node_id).await?,
            from_turn: None,
            to_turn: None,
        },
    };
    Ok(PreparedFork {
        principal: input.principal.clone(),
        source: Source::Program(Box::new(source)),
        target,
        request: input.request.clone(),
        history,
        options: applied_options(conn, input.thread.node_id, input.request.options.as_ref())
            .await?,
        options_listed,
        approval_mode,
        blockers,
        reach: input.reach,
        now,
        captured_agent_access: input.agent_access,
    })
}

async fn history(
    conn: &mut Conn,
    prepared: &PreparedFork,
    source: &ProgramSource,
    blockers: &mut Blockers,
) -> Result<ForkPlanHistory, PlanError> {
    let mode = prepared.history.mode;
    let request = ForkRequest {
        history: Some(prepared.history.clone()),
        ..prepared.request.clone()
    };
    let range = history_range_view(
        conn,
        source.thread.node_id,
        source.thread.active_continuation_id,
        &request,
    )
    .await?;
    let (low, high) = range.map_or((1, 0), |(low, high)| (i64::from(low), i64::from(high)));
    let counts = if mode == HistoryMode::Nothing {
        crate::fork_history::Counts::default()
    } else {
        let counts = orch_db::queries::thread_history::range_counts(
            conn,
            source.thread.node_id,
            source.thread.active_continuation_id,
            low,
            high,
        )
        .await?;
        crate::fork_history::Counts {
            turns: counts.turns,
            items: counts.items,
            left_out_before: i32::try_from(low).ok().filter(|low| *low > 1),
            bytes: counts.bytes,
        }
    };
    if mode != HistoryMode::Nothing
        && orch_db::queries::thread_history_cuts::unconfirmed_inputs(
            conn,
            source.thread.node_id,
            source.thread.active_continuation_id,
            Some(range.unwrap_or((1, 0))),
        )
        .await?
    {
        blockers.add(
            "history_source_unconfirmed",
            "The selected history contains words whose delivery is unconfirmed",
        );
    }
    let method = if counts.turns == 0 && counts.items_total() == 0 {
        HistoryMethod::Nothing
    } else if mode == HistoryMode::Summary {
        HistoryMethod::SummaryHandoff
    } else if prepared.target.descriptor.capabilities.import_neutral {
        HistoryMethod::NeutralImport
    } else {
        HistoryMethod::FileHandoff
    };
    let report = crate::fork_history::report_counts(method, &counts, None);
    Ok(ForkPlanHistory {
        mode,
        turns: counts.turns,
        method,
        source_available: false,
        report,
        archive_bytes: (mode != HistoryMode::Nothing).then_some(counts.bytes),
        native_import_unavailable: None,
        summary: if mode == HistoryMode::Summary {
            Some(
                crate::summaries::handoff_estimate(conn, source.thread.node_id, counts.turns)
                    .await?,
            )
        } else {
            None
        },
    })
}

async fn destination(
    conn: &mut Conn,
    prepared: &PreparedFork,
) -> Result<ForkPlanTarget, PlanError> {
    match &prepared.target.landing {
        Landing::Existing(environment) => Ok(ForkPlanTarget::Existing(
            environment_ref(conn, environment, Time::new(prepared.now)).await?,
        )),
        Landing::New {
            name,
            host: Some(host),
            ..
        } => Ok(ForkPlanTarget::Made {
            new: PlannedEnvironment {
                name: name.clone(),
                place: orch_api_contract::forks::Place::Host(host.place.clone()),
            },
        }),
        _ => Err(PlanError::invalid(
            "environment",
            "Choose an available destination host or environment",
        )),
    }
}

pub(super) async fn assemble(
    engine: &Engine,
    conn: &mut Conn,
    prepared: &PreparedFork,
    source: &ProgramSource,
    preparation: Option<&operation::Preparation>,
) -> Result<Draft, PlanError> {
    let replace = prepared.request.result == ForkResult::Replace;
    let target = &prepared.target;
    let mut blockers = prepared.blockers.clone();
    let history = history(conn, prepared, source, &mut blockers).await?;
    let option = target
        .arrangement
        .as_ref()
        .and_then(|chosen| {
            prepared
                .options_listed
                .iter()
                .find(|option| option.kind == chosen.kind)
        })
        .or_else(|| prepared.options_listed.first())
        .ok_or_else(|| PlanError::invalid("to.agent_id", "The profile declares no arrangement"))?;
    let kind = target
        .arrangement
        .as_ref()
        .map_or(option.kind, |chosen| chosen.kind);
    let environment = destination(conn, prepared).await?;
    let owner = ForkPlanOwner {
        before: if replace {
            Some(user_ref(conn, source.thread.owner_id).await?)
        } else {
            None
        },
        after: user_ref(
            conn,
            if replace {
                source.thread.owner_id
            } else {
                prepared.principal.user()
            },
        )
        .await?,
    };
    let agent_access = if replace {
        binding::BirthAgentAccess::nothing()
    } else {
        match &prepared.captured_agent_access {
            Some(choice) => choice.clone(),
            None => binding::BirthAgentAccess::from(prepared.request.agent_access)
                .freeze(conn, target.topic)
                .await
                .map_err(|error| PlanError::AgentAccess(Box::new(error)))?,
        }
    };
    engine
        .check_birth_source_in(conn, &prepared.principal, target.topic, &agent_access)
        .await
        .map_err(|error| PlanError::AgentAccess(Box::new(error)))?;
    let plan = ForkPlan {
        preflight_id: Id::nil(),
        expires_at: Time::new(prepared.now + Duration::minutes(PREFLIGHT_VALID_MINUTES)),
        source: thread_ref(&source.thread),
        result: prepared.request.result,
        from: ForkPlanFrom::Program {
            program: source.program.clone(),
            provider: provider_ref(conn, &source.provider).await?,
            model: source.model.clone(),
        },
        to: ForkPlanTo {
            agent: agent_ref(conn, &target.agent).await?,
            provider: provider_ref(conn, &target.provider).await?,
            model: target.model.clone(),
            effort: target.effort.clone(),
            approval_mode: Some(convert(&prepared.approval_mode)?),
            arrangement: convert(&kind)?,
            isolation: isolation_view(option, &target.provider, &target.agent, &target.descriptor)?,
            predicted: matches!(environment, ForkPlanTarget::Made { .. }),
            environment,
            directory: target.directory.clone(),
        },
        history,
        files: None,
        worktree: None,
        files_at: None,
        memory: None,
        mounts: Vec::new(),
        mcp: Vec::new(),
        options: prepared.options.clone(),
        agent_access: None,
        access: ForkPlanAccess {
            summary: orch_api_contract::access::AccessSummary {
                view: Vec::new(),
                interact: Vec::new(),
                manage: Vec::new(),
                custom: false,
                from: None,
            },
            profile: match &target.landing {
                Landing::Existing(environment) if environment.placement == "connected" => {
                    orch_api_contract::access::PageProfile::Connected
                }
                _ => orch_api_contract::access::PageProfile::Managed,
            },
            differs: Vec::new(),
        },
        owner,
        depends: Vec::new(),
        locks: if replace {
            vec![thread_ref(&source.thread)]
        } else {
            Vec::new()
        },
        leaves: replace.then_some(ForkPlanLeaves {
            session: true,
            environment: None,
        }),
        estimate: estimates::estimate(
            &Work {
                bytes_up: 0,
                bytes_down: 0,
                files: 0,
                archive_bytes: None,
                making: if matches!(target.landing, Landing::New { .. }) {
                    Making::Environment
                } else {
                    Making::StartRuntime
                },
            },
            &Rates {
                up: Rate::of(None),
                down: Rate::of(None),
            },
        ),
        turn_running: source.thread.current_turn_id.is_some(),
        blockers: std::mem::take(&mut blockers.list),
    };
    let disclosure = disclosure(conn, prepared, source).await?;
    if let Some(preparation) = preparation {
        preparation.check(engine).await?;
    }
    Ok(Draft {
        request: ForkRequest {
            history: Some(prepared.history.clone()),
            ..prepared.request.clone()
        },
        plan,
        agent_access,
        lacks: blockers.lacks,
        copy_worktree: false,
        source_scope: SourceScope::of(&source.thread, prepared.reach),
        mount_decisions: crate::mount_copies::MountDecisions::default(),
        disclosure,
    })
}

async fn disclosure(
    conn: &mut Conn,
    prepared: &PreparedFork,
    source: &ProgramSource,
) -> Result<Vec<orch_access_contract::decisions::ObjectRef>, PlanError> {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind as K};
    let mut subjects = std::collections::BTreeSet::from([
        (K::Thread, source.thread.node_id),
        (K::Provider, source.provider.id),
        (K::Provider, prepared.target.provider.id),
        (K::Agent, prepared.target.agent.id),
        (K::Topic, prepared.target.topic),
    ]);
    let mut nodes = vec![source.thread.node_id, prepared.target.topic];
    match &prepared.target.landing {
        Landing::Existing(environment) => {
            subjects.insert((K::Environment, environment.id));
            nodes.push(environment.home_node_id);
        }
        Landing::New {
            host: Some(host), ..
        } => {
            subjects.insert((K::Host, host.host.id));
            subjects.insert((K::Machine, host.host.machine_id));
        }
        _ => {}
    }
    if let Some(ForkEnvironment::New { new }) = &prepared.request.environment {
        let home = new.home_node_id.unwrap_or(prepared.target.topic);
        nodes.push(home);
        if let Some(template) = match new.template_id {
            Some(id) => Some(id),
            None => setting::<Id>(conn, SettingsPlace::Node(home), keys::ENV_TEMPLATE_ID).await?,
        } {
            subjects.insert((K::Template, template));
        }
    }
    let graph = orch_authz::engine::Graph::load(conn, &nodes).await?;
    for node in graph.nodes() {
        subjects.insert((K::from(node.kind), node.id));
    }
    if let Some(orch_api_contract::access::ThreadAccess::As(page)) = &prepared.request.access {
        subjects.insert((K::Thread, page.source));
    }
    Ok(subjects
        .into_iter()
        .map(|(kind, id)| ObjectRef { kind, id })
        .collect())
}
