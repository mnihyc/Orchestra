//! Actual selected backend facts on the same current permission/read snapshot.
use super::*;
use crate::tools::capability_service::{self, selection};
use orch_db::queries::resources;
use orch_provider_contract::capabilities::{Binding, SelectedBackend};
use orch_settings::{Place, Snapshot as Settings, Wanted};
use orch_settings_contract::registry::keys;

struct Backend {
    endpoint: Endpoint,
    offering: Offering,
    model: Option<capability_service::ModelBackendFacts>,
    quote: Option<Price>,
    rate_source: crate::rates::Source,
}

fn model_quote(
    capability: &cap::Capability,
    parameters: &Json,
    binding: &Binding,
) -> Option<Price> {
    let explicit = capability
        .counter_accounting
        .as_ref()
        .and_then(|accounting| accounting.price.as_deref())
        .and_then(|pointer| parameters.get("rate")?.pointer(pointer))
        .and_then(Json::as_str);
    explicit
        .map(|amount| Price {
            amount: amount.into(),
            currency: "credits".into(),
            unit: capability.ledger_unit.clone(),
            ..Price::default()
        })
        .or_else(|| binding.prices.get(&capability.ledger_unit).cloned())
}

async fn allowed(
    engine: &Engine,
    conn: &mut Conn,
    reader: &BackendRead<'_>,
    selection: &selection::Selection,
    capability: (&str, &cap::Capability),
    backend: &Backend,
) -> Result<bool, EngineError> {
    let Some(reference) = selection.offering() else {
        return Ok(false);
    };
    let place = reader.place;
    if reader.principals.is_empty()
        || !backend.offering.enabled
        || crate::provider_policy::forbidden(conn, place, reference.provider)
            .await?
            .is_some()
    {
        return Ok(false);
    }
    let policy_key = if backend.model.is_some() {
        keys::MODELS_ALLOWED
    } else {
        keys::SERVICES_ALLOWED
    };
    let definition = orch_settings_contract::registry::get(policy_key)
        .ok_or_else(|| EngineError::unreadable("the offering policy is not installed"))?;
    let policies = Settings::load(conn, Place::Node(place), Wanted::Keys(&[definition]))
        .await?
        .resolve(definition);
    let patterns = policies
        .parts
        .into_iter()
        .map(|part| serde_json::from_value::<Vec<String>>(part.value))
        .collect::<Result<Vec<_>, _>>()?;
    let resource_policies = crate::access::resource_policies(conn, reference.provider).await?;
    let groups = &backend.offering.groups;
    for principal in reader.principals {
        let usable = orch_authz::usable::usable(
            conn,
            principal,
            reference.provider,
            Some(place),
            &resource_policies,
        )
        .await?;
        let Some(granted) = usable.granted() else {
            return Ok(false);
        };
        let options = granted.provider();
        if let Some(model) = &backend.model {
            if !orch_authz::options::allows_model(&options, &reference.offering, groups)
                || patterns.iter().any(|patterns| {
                    !orch_agent_contract::discovery::ModelPattern::list_names(
                        patterns,
                        &reference.offering,
                        groups,
                    )
                })
            {
                return Ok(false);
            }
            let effort = orch_provider_contract::calls::requested_effort(&model.options)
                .map_err(EngineError::unreadable)?
                .or(model.parameters.default_effort.as_deref());
            if options.efforts.is_some()
                && effort.is_none_or(|effort| !options.allows_effort(effort))
            {
                return Ok(false);
            }
        } else if !orch_authz::options::allows_service(
            &options,
            true,
            &reference.offering,
            groups,
            None,
        ) || patterns.iter().any(|patterns| {
            !orch_authz::options::allows_service(
                &options,
                true,
                &reference.offering,
                groups,
                Some(patterns),
            )
        }) {
            return Ok(false);
        }
        if !principal
            .caller()
            .key_deadline_allows(engine.inner.clock.as_ref())
        {
            return Ok(false);
        }
    }
    // Caps follow the actual issuer, independently of an approver's ancillary Use.
    if let Some(model) = &backend.model {
        let tier = orch_model_contract::parameters::requested_service_tier(
            &model.endpoint.interface,
            &model.options,
        );
        crate::caps::own_room(
            conn,
            &crate::caps::OwnCapAsk {
                place,
                provider: reference.provider,
                model: &reference.offering,
                tier: tier.as_deref(),
                purpose: capability.0,
                person: reader.person,
            },
            engine.inner.clock.now(),
        )
        .await
    } else {
        let book = crate::rates::RateBook::load(conn, place).await?;
        let priced = book
            .price_service(
                conn,
                backend.quote.as_ref(),
                backend.rate_source,
                &capability.1.ledger_unit,
                &orch_api_contract::usage::UsageMeasurement::Told { value: 1 },
            )
            .await?;
        crate::caps::service_room(
            conn,
            &crate::caps::ServiceCapAsk {
                place,
                provider: reference.provider,
                offering: &reference.offering,
                groups,
                purpose: capability.0,
                person: reader.person,
                priced: priced.credits_per_unit.is_some(),
                harness_kind: reader.harness_kind,
            },
            engine.inner.clock.now(),
        )
        .await
    }
}

impl Engine {
    /// Join actual enabled backends using these current principals and already visible settings.
    /// Optional provider metadata must come from this SAME operation's existing Read supplier.
    /// Private endpoint/Use/cap facts never grant execution, response Read or another source scope.
    pub async fn join_capability_backends(
        &self,
        conn: &mut Conn,
        snapshot: &mut Snapshot,
        settings: &BTreeMap<String, Json>,
        reader: BackendRead<'_>,
    ) -> Result<(), EngineError> {
        let sources = self
            .inner
            .capability_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(sources) = sources else {
            return Ok(());
        };
        let declarations = sources.capabilities().declarations();
        let work = sqlx::query_scalar::<_,Id>("SELECT t.input_id FROM threads h JOIN turns t ON t.id=h.current_turn_id WHERE h.node_id=$1")
            .bind(reader.place).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if let Some(input) = work
            && let Some(work) = orch_db::queries::capability_work::for_input(conn, input).await?
            && let Some(mode) = snapshot
                .registry
                .declarations()
                .programs
                .get(&snapshot.context.program)
                .and_then(|program| program.access.get(&snapshot.context.access))
                .and_then(|access| access.native.get(&work.purpose))
                .and_then(|support| support.delegated_mode.clone())
        {
            snapshot.context.choices.insert(work.purpose, Some(mode));
        }
        let mut thread = snapshot
            .registry
            .extension("thread")
            .cloned()
            .unwrap_or_default();
        for (id, capability) in &declarations.capabilities {
            let Some(value) = capability
                .backend_setting
                .as_deref()
                .and_then(|key| setting(settings, key))
            else {
                continue;
            };
            let Some(selected) = selection::from_value(declarations, id, value.clone()) else {
                continue;
            };
            if !matches!(selected.executor.as_str(), "api-backed" | "model-backed") {
                continue;
            }
            let Some(reference) = selected.offering() else {
                continue;
            };
            snapshot
                .context
                .backends
                .insert(id.clone(), selected.reference.clone());
            let Some(row) = resources::routes::providers(conn, &[reference.provider])
                .await?
                .into_iter()
                .next()
            else {
                continue;
            };
            if row.kind != "api" {
                continue;
            }
            let backend = if selected.executor == "model-backed" {
                let Some(model) =
                    resources::thread_engine::model(conn, row.id, &reference.offering).await?
                else {
                    continue;
                };
                let Some(facts) =
                    capability_service::model_backend_facts(&sources, &row.upstreams, &model, id)?
                else {
                    continue;
                };
                let parameters = serde_json::to_value(&facts.parameters)?;
                let quote = model_quote(capability, &parameters, &facts.binding);
                let modes = declarations
                    .routes
                    .get(&selected.reference.route)
                    .and_then(|route| route.offering_mode.clone())
                    .into_iter()
                    .collect();
                Backend {
                    endpoint: facts.endpoint.clone(),
                    offering: Offering {
                        title: model.display_name,
                        endpoint: facts.endpoint.interface.clone(),
                        kind: "model".into(),
                        enabled: model.enabled,
                        groups: model.groups,
                        parameters,
                        capabilities: BTreeMap::from([(
                            id.clone(),
                            Support {
                                enabled: true,
                                modes,
                                ..Support::default()
                            },
                        )]),
                        native_tools: BTreeMap::from([(id.clone(), Some(facts.binding.clone()))]),
                        native_switches: facts.native_switches.clone(),
                        prices: quote
                            .clone()
                            .map(|quote| BTreeMap::from([(capability.ledger_unit.clone(), quote)]))
                            .unwrap_or_default(),
                        ..Offering::default()
                    },
                    model: Some(facts),
                    quote,
                    rate_source: crate::rates::Source::Catalogue,
                }
            } else {
                let configs = serde_json::from_value(row.service_offerings)?;
                let catalog = orch_provider_contract::services::catalogue(
                    declarations,
                    &capability_service::endpoints(&row.upstreams)?,
                    &configs,
                );
                let Some(offering) = catalog.offerings.get(&reference.offering).cloned() else {
                    continue;
                };
                let Some(endpoint) = catalog.endpoints.get(&offering.endpoint).cloned() else {
                    continue;
                };
                if !declarations
                    .interfaces
                    .get(&endpoint.interface)
                    .is_some_and(|interface| {
                        orch_provider_contract::services::json_implementation(
                            &interface.implementation,
                        )
                    })
                {
                    continue;
                }
                let Some(binding) = orch_provider_contract::services::service_binding(
                    declarations,
                    &endpoint,
                    &offering,
                    id,
                ) else {
                    continue;
                };
                let quote = offering
                    .prices
                    .get(&capability.ledger_unit)
                    .or_else(|| binding.prices.get(&capability.ledger_unit))
                    .cloned();
                let explicit = catalog
                    .explicit_prices
                    .get(&reference.offering)
                    .is_some_and(|units| units.contains(&capability.ledger_unit));
                Backend {
                    endpoint,
                    offering,
                    model: None,
                    quote,
                    rate_source: if explicit {
                        crate::rates::Source::Provider
                    } else {
                        crate::rates::Source::Catalogue
                    },
                }
            };
            let metadata = reader
                .provider
                .filter(|provider| provider.base.id == reference.provider);
            let account = thread.providers.entry(reference.provider).or_default();
            if let Some(metadata) = metadata {
                account.title.clone_from(&metadata.base.name);
            }
            let has_key: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM secrets WHERE owner_kind='provider' AND owner_id=$1 AND name='api_key')")
                .bind(reference.provider).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
            if has_key {
                let roles = snapshot
                    .context
                    .credential_roles
                    .entry(reference.provider)
                    .or_default();
                if !roles.iter().any(|role| role == "model") {
                    roles.push("model".into());
                }
            }
            let supported_role = match backend.endpoint.credential_role.as_deref() {
                None => true,
                Some("model") => has_key,
                _ => false,
            };
            let allowed = row.key_machine_id.is_some()
                && supported_role
                && crate::providers::request_status_available(&row.status)
                && allowed(self, conn, &reader, &selected, (id, capability), &backend).await?;
            snapshot.context.backend_offerings.insert(
                id.clone(),
                SelectedBackend {
                    reference: reference.clone(),
                    endpoint: backend.endpoint,
                    offering: backend.offering,
                    availability: Availability {
                        allowed,
                        show_provider: metadata.is_some(),
                        show_offering: metadata.is_some(),
                        show_price: metadata.is_some(),
                        ..Availability::default()
                    },
                },
            );
        }
        // These two runners are actually installed by attach_capability_services in this process.
        for executor in ["api-backed", "model-backed"] {
            if !snapshot
                .context
                .executors
                .iter()
                .any(|known| known == executor)
            {
                snapshot.context.executors.push(executor.into());
            }
        }
        for (id, interface) in &declarations.interfaces {
            if orch_provider_contract::services::json_implementation(&interface.implementation)
                && !snapshot.context.interfaces.contains(id)
            {
                snapshot.context.interfaces.push(id.clone());
            }
        }
        snapshot
            .registry
            .register("thread", thread)
            .map_err(EngineError::unreadable)?;
        Ok(())
    }
}
