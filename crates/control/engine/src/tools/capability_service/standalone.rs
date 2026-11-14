//! Native producers join the same paid command checks, runner and measured ledger.
use super::*;
use orch_link_contract::commands::{CommandKind, ProviderComplete};

impl Engine {
    /// Add the shared paid guard to a producer's actual, already source-bound native command.
    /// This accepts no caller/id surrogate: the retained native authority still admits every effect.
    /// The producer journals the returned command and owns current result Read/materialization.
    pub async fn prepare_paid_service_command(
        &self,
        mut command: Outgoing,
        attribution: PaidAttribution,
    ) -> Result<Outgoing, EngineError> {
        if attribution.place.is_nil()
            || attribution.topic.is_nil()
            || attribution.org.is_nil()
            || attribution.project.is_nil()
            || attribution.purpose.is_empty()
        {
            return Err(EngineError::StaleToolSource);
        }
        let native = command
            .authority
            .take()
            .ok_or(EngineError::StaleToolSource)?;
        if let Some(existing) = native.paid_service() {
            if !matches!(&existing.origin, Origin::Native { attribution: saved, .. } if saved.as_ref() == &attribution)
                || existing.command != command.id
                || existing.machine != command.machine
                || existing.body != source::hash_json(command.body.clone())?
            {
                return Err(EngineError::ToolCallIdentity);
            }
            command.authority = Some(native);
            return Ok(command);
        }
        if !native.requires_issue_checkpoint() || native.command_kind() != command.kind.as_str() {
            return Err(EngineError::StaleToolSource);
        }
        let sources = self
            .inner
            .capability_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| EngineError::unreadable("the paid service runner is not installed"))?;
        let provider = match command.kind {
            CommandKind::ProviderGenerate | CommandKind::ProviderComplete => {
                model::RequestFacts::read(
                    command.kind == CommandKind::ProviderComplete,
                    &command.body,
                )?
                .provider_id
            }
            CommandKind::ProviderService => {
                serde_json::from_value::<ProviderService>(command.body.clone())?.provider_id
            }
            _ => return Err(EngineError::ToolCallIdentity),
        };
        if command.key != SerialKey::Provider(provider) {
            return Err(EngineError::ToolCallIdentity);
        }
        let mut conn = self.db().begin().await?;
        let row = resources::routes::providers(&mut conn, &[provider])
            .await?
            .into_iter()
            .next()
            .ok_or(EngineError::StaleToolSource)?;
        if row.kind != "api"
            || row.key_machine_id != Some(command.machine)
            || !crate::providers::request_status_available(&row.status)
        {
            return Err(EngineError::StaleToolSource);
        }
        let mut evidence = ServiceCommand {
            recovery: None,
            model: None,
            origin: Origin::Unbound,
            command: command.id,
            machine: command.machine,
            body: source::hash_json(command.body.clone())?,
            provider,
            offering: String::new(),
            capability: attribution.purpose.clone(),
            upstreams: row.upstreams.clone(),
            config: ServiceConfig::default(),
            backend: None,
            groups: Vec::new(),
            unit: String::new(),
            counter: None,
            quote: None,
            explicit_price: false,
            entry: None,
        };
        match command.kind {
            CommandKind::ProviderGenerate | CommandKind::ProviderComplete => {
                let complete = command.kind == CommandKind::ProviderComplete;
                let request = model::RequestFacts::read(complete, &command.body)?;
                if complete {
                    let original: ProviderComplete = serde_json::from_value(command.body.clone())?;
                    if original.purpose != orch_link_contract::commands::CompletionPurpose::Summary
                        || attribution.purpose != "summary"
                    {
                        return Err(EngineError::ToolCallIdentity);
                    }
                }
                if request
                    .native_capability
                    .as_ref()
                    .is_some_and(|native| native.capability != attribution.purpose)
                    || request.native_capability.is_none()
                        && !matches!(attribution.purpose.as_str(), "summary" | "ask")
                {
                    return Err(EngineError::ToolCallIdentity);
                }
                let selected =
                    resources::thread_engine::model(&mut conn, provider, &request.model_name)
                        .await?
                        .filter(|selected| selected.enabled)
                        .ok_or(EngineError::StaleToolSource)?;
                if model::parameters(&selected, &request.protocol)? != request.model_parameters {
                    return Err(EngineError::StaleToolSource);
                }
                evidence.model = Some(model::ModelPlan {
                    protocol: request.protocol,
                    complete,
                });
                evidence.offering = request.model_name;
                evidence.groups = selected.groups;
            }
            CommandKind::ProviderService => {
                let request: ProviderService = serde_json::from_value(command.body.clone())?;
                if request.capability != attribution.purpose {
                    return Err(EngineError::ToolCallIdentity);
                }
                let declarations = sources.capabilities().declarations();
                let configs: BTreeMap<String, ServiceConfig> =
                    serde_json::from_value(row.service_offerings)?;
                let catalog = orch_provider_contract::services::catalogue(
                    declarations,
                    &endpoints(&row.upstreams)?,
                    &configs,
                );
                let selected = catalog
                    .offerings
                    .get(&request.offering)
                    .filter(|offering| {
                        offering.enabled
                            && offering
                                .capabilities
                                .get(&request.capability)
                                .is_some_and(|support| support.enabled)
                    })
                    .ok_or(EngineError::StaleToolSource)?;
                let endpoint = catalog
                    .endpoints
                    .get(&selected.endpoint)
                    .ok_or(EngineError::StaleToolSource)?;
                if !declarations
                    .interfaces
                    .get(&endpoint.interface)
                    .is_some_and(|interface| {
                        orch_provider_contract::services::json_implementation(
                            &interface.implementation,
                        )
                    })
                {
                    return Err(EngineError::StaleToolSource);
                }
                let binding = orch_provider_contract::services::service_binding(
                    declarations,
                    endpoint,
                    selected,
                    &request.capability,
                )
                .ok_or(EngineError::StaleToolSource)?;
                if orch_provider_contract::services::definition_hash(endpoint, binding)?
                    != request.definition
                {
                    return Err(EngineError::StaleToolSource);
                }
                orch_provider_contract::capabilities::render_request(binding, &request.input)
                    .map_err(EngineError::unreadable)?;
                let definition = declarations
                    .capabilities
                    .get(&request.capability)
                    .ok_or(EngineError::StaleToolSource)?;
                evidence.unit = definition.ledger_unit.clone();
                let mut counters = binding
                    .counters
                    .iter()
                    .filter(|(_, counter)| !counter.reported_cost && counter.unit == evidence.unit);
                evidence.counter = counters.next().map(|(name, _)| name.clone());
                if counters.next().is_some() {
                    return Err(EngineError::unreadable(
                        "the service declares ambiguous charged quantities",
                    ));
                }
                evidence.quote = selected
                    .prices
                    .get(&evidence.unit)
                    .or_else(|| binding.prices.get(&evidence.unit))
                    .cloned();
                evidence.explicit_price = catalog
                    .explicit_prices
                    .get(&request.offering)
                    .is_some_and(|units| units.contains(&evidence.unit));
                evidence.groups.clone_from(&selected.groups);
                evidence.config = configs
                    .get(&request.offering)
                    .cloned()
                    .ok_or(EngineError::StaleToolSource)?;
                evidence.offering = request.offering;
            }
            _ => return Err(EngineError::ToolCallIdentity),
        }
        // Keep raw HTTP identity outside the common guard, so its durable narrowing remains original.
        command.authority = Some(match native {
            CommandAuthority::Http(http) => http.paid_service(evidence, attribution)?,
            native => CommandAuthority::CapabilityService(Box::new(
                evidence.bind_native(NativeSource::Channel(Box::new(native)), attribution),
            )),
        });
        let authority = command
            .authority
            .as_ref()
            .ok_or(EngineError::StaleToolSource)?;
        let deadline = command.deadline.map_or_else(
            || {
                self.inner.clock.now()
                    + chrono::Duration::seconds(i64::from(command.kind.deadline_secs()))
            },
            Time::get,
        );
        let key = command.key.wire();
        let proposed = link::NewCommand {
            id: command.id,
            machine_id: command.machine,
            kind: command.kind.as_str(),
            body: &command.body,
            serial_key: key.as_deref(),
            deadline_at: deadline,
        };
        authority.lock_native_source(&mut conn).await?;
        let callers = Box::pin(authority.current_issue_callers(
            &mut conn,
            &proposed,
            self.inner.clock.as_ref(),
            self.inner.access.as_ref(),
        ))
        .await?
        .ok_or(EngineError::StaleToolSource)?;
        let paid = authority
            .paid_service()
            .ok_or(EngineError::ToolCallIdentity)?;
        let mut entry = None;
        for (caller, _) in callers {
            let caller = caller.ok_or(EngineError::StaleToolSource)?;
            let granted = if paid.model.is_some() {
                model::model_use(
                    &mut conn,
                    &caller,
                    paid,
                    &model::RequestFacts::read(
                        paid.model.as_ref().is_some_and(|plan| plan.complete),
                        &command.body,
                    )?,
                    self.inner.clock.as_ref(),
                )
                .await?
            } else {
                service_use(
                    &mut conn,
                    &caller,
                    paid.origin.place()?,
                    provider,
                    (&paid.offering, &paid.groups),
                    orch_access_contract::permissions::ProviderPurpose::for_capability(
                        &paid.capability,
                    ),
                    self.inner.clock.as_ref(),
                )
                .await?
            }
            .ok_or(EngineError::StaleToolSource)?;
            if Some(caller.user()) == paid.native_person()
                || paid.native_person().is_none() && entry.is_none()
            {
                entry = granted;
            }
        }
        command
            .authority
            .as_mut()
            .ok_or(EngineError::StaleToolSource)?
            .paid_service_mut()
            .ok_or(EngineError::ToolCallIdentity)?
            .entry = entry;
        conn.commit().await.map_err(orch_db::DbError::from)?;
        Ok(command)
    }

    /// Run only a command prepared with the shared guard and the producer's real native authority.
    /// The returned native result is private to that producer; publishing it still needs its Read guard.
    pub async fn run_paid_service_command(
        &self,
        command: Outgoing,
    ) -> Result<crate::commands::Outcome, EngineError> {
        let paid = command
            .authority
            .as_ref()
            .and_then(CommandAuthority::paid_service)
            .ok_or(EngineError::StaleToolSource)?;
        if !matches!(paid.origin, Origin::Native { .. }) {
            return Err(EngineError::StaleToolSource);
        }
        if self.inner.commands.status(command.id).await?.is_none() {
            let mut tx = self.db().begin().await?;
            let key = command.key.wire();
            let proposed = link::NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: key.as_deref(),
                deadline_at: command.deadline.map_or_else(
                    || {
                        self.inner.clock.now()
                            + chrono::Duration::seconds(i64::from(command.kind.deadline_secs()))
                    },
                    Time::get,
                ),
            };
            let authority = command
                .authority
                .as_ref()
                .ok_or(EngineError::StaleToolSource)?;
            authority.lock_native_source(&mut tx).await?;
            Box::pin(authority.current_issue_callers(
                &mut tx,
                &proposed,
                self.inner.clock.as_ref(),
                self.inner.access.as_ref(),
            ))
            .await?
            .ok_or(EngineError::StaleToolSource)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            self.ensure_provider_up(paid.provider)
                .await?
                .map_err(EngineError::unreadable)?;
        }
        let id = command.id;
        // The real command writer rechecks native source, raw identity, every Use and caps after its waits.
        self.inner.commands.issue(command).await?;
        let outcome = self.inner.commands.wait(id).await?;
        reconcile(self, id).await?;
        Ok(outcome)
    }

    /// Reconcile an actual completed shared command, including after native worker/process loss.
    pub async fn reconcile_paid_service_command(&self, command: Id) -> Result<(), EngineError> {
        reconcile(self, command).await
    }
}

impl ServiceCommand {
    fn native_person(&self) -> Option<Id> {
        match &self.origin {
            Origin::Native { attribution, .. } => attribution.person,
            _ => None,
        }
    }
}

impl Engine {
    /// Completion accounting is independent of a still-live native waiter or readable result.
    pub(crate) async fn reap_paid_service_commands(&self, limit: i64) -> Result<(), EngineError> {
        let ids: Vec<Id> = {
            let mut conn = self.db().acquire().await?;
            sqlx::query_scalar("SELECT c.id FROM commands c WHERE c.kind IN ('provider.service','provider.generate','provider.complete')
                AND (c.state='succeeded' OR (c.state='failed' AND (c.error#>'{details,service_result}' IS NOT NULL OR c.error#>'{details,model_request}' IS NOT NULL)))
                AND (c.login_authority->>'kind'='capability_service' OR (c.login_authority->>'kind'='http_origin' AND c.login_authority#>>'{authority,native,evidence,kind}'='capability_service'))
                AND NOT EXISTS (SELECT 1 FROM usage_events u WHERE u.id=c.id)
                ORDER BY c.updated_at,c.id LIMIT $1")
                .bind(limit).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
        };
        for id in ids {
            if let Err(error) = reconcile(self, id).await {
                tracing::warn!(%error, command=%id, "paid service usage will be reconciled on the next sweep");
            }
        }
        Ok(())
    }
}
