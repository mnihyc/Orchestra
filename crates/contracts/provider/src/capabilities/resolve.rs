//! No vendor, protocol-name or backend-kind dispatch. Routes and predicates are declarations.
use super::model::*;
use orch_types::Json;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// A conflicting contribution cannot partially replace existing declarations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// Two independently registered extensions claim the same declaration id differently.
    #[error("the {noun} declaration {id} is already registered")]
    Conflict {
        /// Declaration noun.
        noun: &'static str,
        /// Conflicting id, never a source body.
        id: String,
    },
    /// An empty extension name is not an identity.
    #[error("an extension registration needs a name")]
    MissingName,
    /// An extension links a route to an absent or incompatible capability/mode declaration.
    #[error("the route link {capability}/{mode}/{route} has no matching declaration")]
    InvalidLink {
        /// Capability receiving the additive link.
        capability: String,
        /// Existing mode receiving it.
        mode: String,
        /// Declared route being linked.
        route: String,
    },
}

/// One immutable snapshot may be shared by dispatch planning, inspectors and matrix producers.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    bundles: BTreeMap<String, Declarations>,
    merged: Declarations,
}

fn insert<K: Ord + Clone + ToString, V: Clone + PartialEq>(
    target: &mut BTreeMap<K, V>,
    source: &BTreeMap<K, V>,
    noun: &'static str,
) -> Result<(), RegistryError> {
    for (key, value) in source {
        if target.get(key).is_some_and(|prior| prior != value) {
            return Err(RegistryError::Conflict {
                noun,
                id: key.to_string(),
            });
        }
        target.insert(key.clone(), value.clone());
    }
    Ok(())
}

impl Registry {
    /// Add/replace one extension atomically. New names need no closed built-in list.
    pub fn register(&mut self, name: &str, decl: Declarations) -> Result<(), RegistryError> {
        if name.trim().is_empty() {
            return Err(RegistryError::MissingName);
        }
        let mut bundles = self.bundles.clone();
        bundles.insert(name.into(), decl);
        let merged = Self::merge(&bundles)?;
        self.bundles = bundles;
        self.merged = merged;
        Ok(())
    }

    /// Removing an extension recomputes the same derived snapshot without stored-model edits.
    pub fn remove(&mut self, name: &str) -> bool {
        if !self.bundles.contains_key(name) {
            return false;
        }
        let mut bundles = self.bundles.clone();
        bundles.remove(name);
        let Ok(merged) = Self::merge(&bundles) else {
            return false;
        };
        self.bundles = bundles;
        self.merged = merged;
        true
    }

    fn merge(bundles: &BTreeMap<String, Declarations>) -> Result<Declarations, RegistryError> {
        let mut merged = Declarations::default();
        for bundle in bundles.values() {
            insert(&mut merged.interfaces, &bundle.interfaces, "interface")?;
            insert(&mut merged.providers, &bundle.providers, "provider")?;
            insert(&mut merged.capabilities, &bundle.capabilities, "capability")?;
            insert(&mut merged.programs, &bundle.programs, "program")?;
            insert(&mut merged.routes, &bundle.routes, "route")?;
            merged.endpoint_rules.extend(bundle.endpoint_rules.clone());
        }
        // Resolve links only after every noun is composed, independently of bundle iteration order.
        for bundle in bundles.values() {
            for (capability, modes) in &bundle.route_links {
                for (mode, routes) in modes {
                    for route in routes {
                        let valid = merged
                            .routes
                            .get(route)
                            .is_some_and(|route| route.capabilities.contains(capability));
                        let target = merged
                            .capabilities
                            .get_mut(capability)
                            .and_then(|capability| capability.modes.get_mut(mode));
                        let Some(target) = target.filter(|_| valid) else {
                            return Err(RegistryError::InvalidLink {
                                capability: capability.clone(),
                                mode: mode.clone(),
                                route: route.clone(),
                            });
                        };
                        if !target.routes.contains(route) {
                            target.routes.push(route.clone());
                        }
                        let linked = merged
                            .route_links
                            .entry(capability.clone())
                            .or_default()
                            .entry(mode.clone())
                            .or_default();
                        if !linked.contains(route) {
                            linked.push(route.clone());
                        }
                    }
                }
            }
        }
        Ok(merged)
    }

    /// Actual registered declarations, including opaque unknown members.
    #[must_use]
    pub fn declarations(&self) -> &Declarations {
        &self.merged
    }

    /// Original per-extension documents retain future root fields independently.
    #[must_use]
    pub fn extension(&self, name: &str) -> Option<&Declarations> {
        self.bundles.get(name)
    }

    /// The only per-thread capability route computation.
    #[must_use]
    pub fn resolve(&self, context: &ThreadContext) -> Vec<Resolution> {
        resolve(&self.merged, context)
    }

    /// Derived cells use the exact thread resolver; no independent support matrix is maintained.
    #[must_use]
    pub fn matrix<'a>(
        &self,
        contexts: impl IntoIterator<Item = &'a ThreadContext>,
    ) -> Vec<MatrixCell> {
        contexts
            .into_iter()
            .flat_map(|context| {
                self.resolve(context)
                    .into_iter()
                    .map(|resolution| MatrixCell {
                        program: context.program.clone(),
                        access: context.access.clone(),
                        current: context
                            .current
                            .as_ref()
                            .filter(|reference| {
                                context
                                    .availability
                                    .get(&reference.provider)
                                    .and_then(|offerings| offerings.get(&reference.offering))
                                    .is_some_and(|state| state.show_provider && state.show_offering)
                            })
                            .cloned(),
                        resolution,
                    })
            })
            .collect()
    }
}

fn eligible(schema: &Json, facts: &Json) -> bool {
    schema.is_null() || orch_types::schema::validate(schema, facts).is_ok()
}

/// Optional display metadata never participates in route eligibility or authority.
fn support_reason(support: &Support) -> Option<String> {
    support
        .extensions
        .get("x_unavailable_reason")
        .and_then(Json::as_str)
        .map(str::to_owned)
}

#[derive(Debug)]
struct Refused {
    reason: &'static str,
    detail: Option<String>,
}
fn refused(reason: &'static str) -> Refused {
    Refused {
        reason,
        detail: None,
    }
}

struct Candidate<'a> {
    facts: Json,
    route: &'a Route,
    reference: Option<OfferingRef>,
    backend: Option<BackendRef>,
    provider: Option<&'a Provider>,
    offering: Option<&'a Offering>,
    availability: Option<&'a Availability>,
    binding: Option<&'a Binding>,
}

#[allow(
    clippy::too_many_arguments,
    reason = "one resolver evaluates the actual declared mode, route and thread facts"
)]
fn candidate<'a>(
    declarations: &'a Declarations,
    context: &'a ThreadContext,
    access: Option<&'a ProgramAccess>,
    capability: &str,
    mode: &str,
    route_id: &str,
    explicit: bool,
) -> Result<Candidate<'a>, Refused> {
    let route = declarations
        .routes
        .get(route_id)
        .ok_or_else(|| refused("route_not_declared"))?;
    if !route.capabilities.iter().any(|known| known == capability) {
        return Err(refused("route_does_not_carry_capability"));
    }
    if (route.requires_opt_in || route.uses_backend) && !explicit {
        return Err(refused("explicit_choice_required"));
    }
    if route.requires_orch_tools && !access.is_some_and(|access| access.orch_tools) {
        return Err(refused("program_cannot_carry_tools"));
    }
    if route
        .executor
        .as_ref()
        .is_some_and(|executor| !context.executors.contains(executor))
    {
        return Err(refused("executor_not_installed"));
    }
    let backend = if route.uses_backend {
        Some(
            context
                .backends
                .get(capability)
                .filter(|backend| backend.route == route_id)
                .ok_or_else(|| refused("backend_not_selected"))?
                .clone(),
        )
    } else {
        None
    };
    let reference = if route.uses_backend {
        backend
            .as_ref()
            .and_then(|backend| backend.offering.clone())
    } else {
        context.current.clone()
    };
    let mut provider = None;
    let mut offering = None;
    let mut availability = None;
    let mut endpoint = None;
    let mut interface = None;
    if let Some(reference) = &reference {
        let actual = declarations
            .providers
            .get(&reference.provider)
            .ok_or_else(|| refused("provider_not_declared"))?;
        let prepared = if route.uses_backend {
            context.backend_offerings.get(capability)
        } else {
            None
        };
        if prepared.is_some_and(|prepared| &prepared.reference != reference) {
            return Err(refused("current_authority_unavailable"));
        }
        let selected = prepared
            .map(|prepared| &prepared.offering)
            .or_else(|| actual.offerings.get(&reference.offering))
            .ok_or_else(|| refused("offering_not_declared"))?;
        if !selected.enabled {
            return Err(refused("offering_not_enabled"));
        }
        let state = prepared
            .map(|prepared| &prepared.availability)
            .or_else(|| {
                context
                    .availability
                    .get(&reference.provider)
                    .and_then(|offerings| offerings.get(&reference.offering))
            })
            .ok_or_else(|| refused("current_authority_unavailable"))?;
        if !state.allowed {
            return Err(Refused {
                reason: "current_authority_refused",
                detail: state.reason.clone(),
            });
        }
        let selected_endpoint = match prepared {
            Some(prepared) => prepared.endpoint.as_ref(),
            None => actual.endpoints.get(&selected.endpoint),
        };
        if let Some(actual_endpoint) = selected_endpoint {
            let actual_interface = declarations
                .interfaces
                .get(&actual_endpoint.interface)
                .ok_or_else(|| refused("interface_not_declared"))?;
            if actual_endpoint
                .credential_role
                .as_ref()
                .is_some_and(|role| {
                    !context
                        .credential_roles
                        .get(&reference.provider)
                        .is_some_and(|roles| roles.contains(role))
                })
            {
                return Err(refused("credential_role_unavailable"));
            }
            endpoint = Some(actual_endpoint);
            interface = Some(actual_interface);
        } else if route.requires_native_tool
            || route.requires_offering_support
            || route.requires_interface
        {
            return Err(refused("endpoint_not_declared"));
        }
        provider = Some(actual);
        offering = Some(selected);
        availability = Some(state);
    } else if route.requires_offering
        || route.requires_offering_support
        || route.requires_native_tool
    {
        return Err(refused("offering_not_selected"));
    }
    let mut facts = context.facts.as_object().cloned().unwrap_or_default();
    facts.insert("program".into(), json!(context.program));
    facts.insert("access".into(), json!(context.access));
    facts.insert("capability".into(), json!(capability));
    facts.insert("route_kind".into(), json!(route.kind));
    facts.insert("mode".into(), json!(mode));
    let mut offering_facts = offering.map_or(Json::Null, |offering| json!(offering));
    if let Some(facts) = offering_facts.as_object_mut() {
        facts.remove("native_switches");
    }
    facts.insert("offering".into(), offering_facts);
    facts.insert(
        "endpoint".into(),
        endpoint.map_or(Json::Null, |endpoint| json!(endpoint)),
    );
    let facts = Json::Object(facts);
    if !eligible(&route.eligible_when, &facts) {
        return Err(refused("route_not_applicable"));
    }
    if route.requires_interface
        && !endpoint.is_some_and(|endpoint| context.interfaces.contains(&endpoint.interface))
    {
        return Err(refused("interface_not_installed"));
    }
    let native = access.and_then(|access| access.native.get(capability));
    if route.requires_generation_interface
        && !endpoint.is_some_and(|endpoint| {
            context.interfaces.contains(&endpoint.interface)
                && access.is_some_and(|access| access.interfaces.contains(&endpoint.interface))
        })
    {
        return Err(refused("program_does_not_carry_interface"));
    }
    if route.requires_program_support {
        let support = native.ok_or_else(|| refused("program_does_not_carry_mode"))?;
        if !support.enabled || !support.modes.iter().any(|known| known == mode) {
            return Err(Refused {
                reason: "program_does_not_carry_mode",
                detail: support_reason(support),
            });
        }
        if !support.interfaces.is_empty()
            && !endpoint.is_some_and(|endpoint| support.interfaces.contains(&endpoint.interface))
        {
            return Err(refused("program_does_not_carry_interface"));
        }
        if !eligible(&support.eligible_when, &facts) {
            return Err(Refused {
                reason: "program_support_not_applicable",
                detail: support_reason(support),
            });
        }
    }
    if route.requires_offering_support {
        let offering_mode = route.offering_mode.as_deref().unwrap_or(mode);
        let projected = offering.and_then(|offering| {
            declarations
                .capabilities
                .get(capability)
                .and_then(|capability| capability.offering_support.as_ref())
                .map(|projection| project_support(offering, projection))
        });
        let support = offering
            .and_then(|offering| offering.capabilities.get(capability))
            .or(projected.as_ref())
            .filter(|support| {
                support.enabled
                    && (support.modes.is_empty()
                        || support.modes.iter().any(|known| known == offering_mode))
            })
            .ok_or_else(|| refused("offering_does_not_enable_mode"))?;
        if !eligible(&support.eligible_when, &facts) {
            return Err(refused("offering_support_not_applicable"));
        }
    }
    let mut selected_binding = interface.map(|interface| &interface.shape);
    if route.requires_native_tool {
        let binding = if let Some(binding) =
            offering.and_then(|offering| offering.native_tools.get(capability))
        {
            binding.as_ref()
        } else if let Some(binding) =
            endpoint.and_then(|endpoint| endpoint.native_tools.get(capability))
        {
            binding.as_ref()
        } else {
            interface.and_then(|interface| interface.native_tools.get(capability))
        };
        let binding = binding.ok_or_else(|| refused("native_tool_not_declared"))?;
        selected_binding = Some(binding);
        if !eligible(&binding.eligible_when, &facts) {
            return Err(refused("native_tool_not_applicable"));
        }
    }
    let mut native_facts = facts;
    native_facts["offering_id"] = reference
        .as_ref()
        .map_or(Json::Null, |reference| json!(reference.offering));
    native_facts["endpoint_facts"] = endpoint
        .and_then(|endpoint| {
            url::Url::parse(&endpoint.address).ok().map(|address| {
                super::endpoint_facts(
                    &endpoint.interface,
                    &address,
                    reference
                        .as_ref()
                        .map_or("", |reference| reference.offering.as_str()),
                )
            })
        })
        .unwrap_or(Json::Null);
    Ok(Candidate {
        facts: native_facts,
        route,
        reference,
        backend,
        provider,
        offering,
        availability,
        binding: selected_binding,
    })
}

fn project_support(offering: &Offering, projection: &SupportProjection) -> Support {
    let value = json!(offering);
    let modes = projection
        .modes
        .as_ref()
        .and_then(|pointer| value.pointer(pointer))
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let enabled = projection
        .enabled
        .as_ref()
        .and_then(|pointer| value.pointer(pointer))
        .and_then(Json::as_bool)
        .unwrap_or_else(|| projection.modes.is_some() && !modes.is_empty());
    let default_mode = projection
        .default_mode
        .as_ref()
        .and_then(|pointer| value.pointer(pointer))
        .and_then(Json::as_str)
        .map(str::to_owned);
    Support {
        enabled,
        modes,
        default_mode,
        ..Support::default()
    }
}

fn fallback_views(
    declarations: &Declarations,
    context: &ThreadContext,
    capability_id: &str,
    mode: Option<&str>,
    capability: &Capability,
) -> Vec<FallbackView> {
    let (Some(policy), Some(purpose)) = (&context.recovery, &capability.recovery_purpose) else {
        return Vec::new();
    };
    let selected_backend = mode
        .and_then(|mode| capability.modes.get(mode))
        .is_some_and(|mode| {
            mode.routes.iter().any(|route_id| {
                declarations
                    .routes
                    .get(route_id)
                    .is_some_and(|route| route.uses_backend)
                    && context
                        .backends
                        .get(capability_id)
                        .is_some_and(|backend| &backend.route == route_id)
            })
        });
    let current = if selected_backend {
        context
            .backends
            .get(capability_id)
            .and_then(|backend| backend.offering.as_ref())
    } else {
        context.current.as_ref()
    };
    policy
        .rules
        .iter()
        .filter(|rule| {
            rule.when.provider_id.is_none_or(|provider| {
                current.is_some_and(|reference| reference.provider == provider)
            }) && rule
                .when
                .model
                .as_ref()
                .is_none_or(|model| current.is_some_and(|reference| &reference.offering == model))
        })
        .filter(|rule| {
            rule.when
                .purpose
                .is_none_or(|rule_purpose| rule_purpose.as_str() == purpose.as_str())
        })
        .flat_map(|rule| {
            rule.steps.iter().filter_map(move |step| {
                let crate::calls::Step::Failover { candidate } = step else {
                    return None;
                };
                let reference = candidate
                    .route
                    .as_ref()
                    .and_then(|route| route.offering.as_ref());
                let offering_id = reference.map_or(candidate.model.as_str(), |reference| {
                    reference.offering.as_str()
                });
                let state = context
                    .availability
                    .get(&candidate.provider_id)
                    .and_then(|offerings| offerings.get(offering_id));
                Some(FallbackView {
                    rule_id: rule.id.clone(),
                    scope: policy.scope.clone(),
                    when: rule.when.kind.clone().unwrap_or_else(|| "any".into()),
                    provider: state
                        .filter(|state| state.show_provider)
                        .and_then(|_| declarations.providers.get(&candidate.provider_id))
                        .map(|provider| ProviderView {
                            id: candidate.provider_id,
                            title: provider.title.clone(),
                        }),
                    offering: state
                        .filter(|state| state.show_offering)
                        .map(|_| offering_id.into()),
                })
            })
        })
        .collect()
}

/// One function computes every capability from this exact thread snapshot. It does not
/// execute, authorize a call or mutate stored models. Execution rechecks through its existing path.
#[must_use]
pub fn resolve(declarations: &Declarations, context: &ThreadContext) -> Vec<Resolution> {
    let program = declarations.programs.get(&context.program);
    let access = program.and_then(|program| program.access.get(&context.access));
    declarations
        .capabilities
        .iter()
        .map(|(id, capability)| {
            let explicit = context.choices.get(id).and_then(Option::as_deref);
            let native_default = access
                .and_then(|access| access.native.get(id))
                .and_then(|support| support.default_mode.as_deref());
            let mode = explicit
                .or(native_default)
                .or(capability.default_mode.as_deref());
            let mut result = Resolution {
                capability: id.clone(),
                title: capability.title.clone(),
                setting: capability.setting.clone(),
                mode: mode.map(str::to_owned),
                program_default: explicit.is_none(),
                item: capability.item.clone(),
                ledger_unit: capability.ledger_unit.clone(),
                fallbacks: fallback_views(declarations, context, id, mode, capability),
                facts: program_fact_views(program, access, context, id),
                native_config: native_configuration_defaults(program, id),
                ..Resolution::default()
            };
            result.modes = capability
                .modes
                .iter()
                .filter(|(mode, definition)| {
                    definition.routes.is_empty()
                        || definition.routes.iter().any(|route| {
                            candidate(declarations, context, access, id, mode, route, true).is_ok()
                        })
                })
                .map(|(mode, _)| mode.clone())
                .collect();
            let Some(mode) = mode else {
                result.reason = Some("program_default_unreported".into());
                return result;
            };
            let Some(definition) = capability.modes.get(mode) else {
                result.reason = Some("mode_not_declared".into());
                return result;
            };
            // An explicit off/service intent still closes its native counterpart when the
            // selected service is unavailable. It cannot fall back to the program's paid tool.
            if explicit.is_some() && (definition.routes.is_empty() || definition.routes.iter().all(|id| {
                declarations.routes.get(id).is_some_and(|route| route.disable_native)
            })) {
                let facts = json!({"program":context.program,"access":context.access,"capability":id,"mode":mode,
                    "route_kind":"","offering":null,"offering_id":null,"endpoint":null,"endpoint_facts":null});
                result.native_config.extend(native_configuration(program, id, &definition.native_value, &facts, false, None));
            }
            if definition.routes.is_empty() {
                result.reason = Some("disabled".into());
                return result;
            }
            let selected_route = context.backends.get(id).filter(|backend| {
                definition.routes.contains(&backend.route)
                    && declarations.routes.get(&backend.route).is_some_and(|route| route.uses_backend)
            }).map(|backend| &backend.route);
            if selected_route.is_none() && definition.routes.iter().all(|route| {
                declarations.routes.get(route).is_some_and(|route| route.uses_backend)
            }) {
                result.reason = Some("backend_not_selected".into());
                return result;
            }
            let mut last = refused("route_unavailable");
            for route_id in definition.routes.iter().filter(|route| selected_route.is_none_or(|selected| selected == *route)) {
                match candidate(
                    declarations,
                    context,
                    access,
                    id,
                    mode,
                    route_id,
                    explicit.is_some(),
                ) {
                    Ok(selected) => {
                        result.route = Some(ResolvedRoute {
                            id: route_id.clone(),
                            kind: selected.route.kind.clone(),
                            executor: selected.route.executor.clone(),
                            offering: selected.reference.clone(),
                            backend: selected.backend,
                            orch_tool: selected.route.orch_tool.clone(),
                            disable_native: selected.route.disable_native,
                            native_value: if explicit.is_none() {
                                Json::Null
                            } else {
                                definition.native_value.clone()
                            },
                        });
                        if let Some(route) = &result.route {
                            result.native_config.extend(native_configuration(
                                program, id, &route.native_value, &selected.facts, true,
                                selected.offering.and_then(|offering| offering.native_switches.get(&context.program)),
                            ));
                        }
                        result.route_view = Some(RouteView {
                            id: route_id.clone(),
                            kind: selected.route.kind.clone(),
                            disable_native: selected.route.disable_native,
                            orch_tool: selected.route.orch_tool.clone(),
                        });
                        if let (Some(reference), Some(provider), Some(state)) = (
                            &selected.reference,
                            selected.provider,
                            selected.availability,
                        ) {
                            if state.show_provider {
                                result.provider = Some(ProviderView {
                                    id: reference.provider,
                                    title: provider.title.clone(),
                                });
                            }
                            if state.show_offering {
                                result.offering = Some(reference.offering.clone());
                            }
                            if state.show_price
                                && let Some(offering) = selected.offering
                            {
                                let prices = if offering.prices.is_empty() {
                                    selected
                                        .binding
                                        .map(|binding| &binding.prices)
                                        .unwrap_or(&offering.prices)
                                } else {
                                    &offering.prices
                                };
                                result.prices = prices
                                    .iter()
                                    .map(|(unit, price)| {
                                        let mut price = price.clone();
                                        price.extensions.clear();
                                        (unit.clone(), price)
                                    })
                                    .collect();
                            }
                        }
                        return result;
                    }
                    Err(reason) => last = reason,
                }
            }
            result.reason = Some(last.reason.into());
            result.detail = last.detail.or_else(|| {
                matches!(
                    last.reason,
                    "route_not_applicable"
                        | "program_does_not_carry_mode"
                        | "program_support_not_applicable"
                )
                .then(|| {
                    access
                        .and_then(|access| access.native.get(id))
                        .and_then(support_reason)
                })
                .flatten()
            });
            result
        })
        .collect()
}

/// Only route-dependent switches opt into defaults for an unresolved or refused route.
/// Existing switches continue to preserve the native program's default when no override is requested.
fn native_configuration_defaults(
    program: Option<&Program>,
    capability: &str,
) -> BTreeMap<String, Json> {
    program
        .into_iter()
        .flat_map(|program| &program.native_switches)
        .filter(|(name, switch)| {
            !name.is_empty() && switch.capability == capability && switch.resolved_value.is_some()
        })
        .filter_map(|(name, switch)| {
            switch
                .value
                .as_ref()
                .filter(|value| public_native_scalar(value))
                .map(|value| (name.clone(), value.clone()))
        })
        .collect()
}

/// Values are cloned only from public program constants, never from an opaque route/backend object.
fn native_configuration(
    program: Option<&Program>,
    capability: &str,
    native_value: &Json,
    facts: &Json,
    route_admitted: bool,
    provider_switches: Option<&BTreeMap<String, NativeSwitch>>,
) -> BTreeMap<String, Json> {
    let mut facts = facts.clone();
    facts["native_value"] = native_value.clone();
    program
        .into_iter()
        .flat_map(|program| &program.native_switches)
        .filter(|(name, switch)| {
            !name.is_empty() && switch.capability == capability && eligible(&switch.when, &facts)
        })
        .filter_map(|(name, switch)| {
            let value = provider_switches
                .filter(|_| switch.provider_overrides)
                .and_then(|switches| switches.get(name))
                .filter(|provided| {
                    (provided.capability.is_empty() || provided.capability == capability)
                        && eligible(&provided.when, &facts)
                })
                .and_then(|provided| {
                    native_switch_value(provided, name, native_value, route_admitted)
                })
                .filter(|value| {
                    switch
                        .values
                        .values()
                        .chain(switch.value.iter())
                        .chain(switch.resolved_value.iter())
                        .any(|allowed| allowed == *value)
                })
                .or_else(|| native_switch_value(switch, name, native_value, route_admitted))?;
            public_native_scalar(value).then(|| (name.clone(), value.clone()))
        })
        .collect()
}

fn public_native_scalar(value: &Json) -> bool {
    value.is_boolean() || value.is_string() || value.as_i64().is_some()
}

fn native_switch_value<'a>(
    switch: &'a NativeSwitch,
    name: &str,
    native_value: &Json,
    route_admitted: bool,
) -> Option<&'a Json> {
    if route_admitted
        && let Some(value) = switch
            .resolved_value
            .as_ref()
            .filter(|value| public_native_scalar(value))
    {
        return Some(value);
    }
    if let Some(value) = switch
        .value
        .as_ref()
        .filter(|value| public_native_scalar(value))
    {
        return Some(value);
    }
    if native_value.is_null() {
        return None;
    }
    let token = native_value
        .as_str()
        .and_then(|token| switch.values.get(token));
    let direct = native_value
        .as_object()
        .and_then(|value| value.get(name))
        .or_else(|| (!native_value.is_object()).then_some(native_value));
    token
        .or_else(|| direct.and_then(|direct| switch.values.values().find(|known| *known == direct)))
}

/// This read-only projection cannot affect candidate selection, offered modes, prices or account visibility.
fn program_fact_views(
    program: Option<&Program>,
    access: Option<&ProgramAccess>,
    context: &ThreadContext,
    capability: &str,
) -> Vec<ProgramFactView> {
    let mut facts = context.facts.as_object().cloned().unwrap_or_default();
    facts.insert("program".into(), json!(context.program));
    facts.insert("access".into(), json!(context.access));
    facts.insert("capability".into(), json!(capability));
    let facts = Json::Object(facts);
    let mut seen = BTreeSet::new();
    program
        .into_iter()
        .flat_map(|program| &program.facts)
        .chain(access.into_iter().flat_map(|access| &access.facts))
        .filter(|fact| {
            !fact.id.is_empty()
                && fact.text.values().any(|text| !text.trim().is_empty())
                && (fact.capabilities.is_empty()
                    || fact.capabilities.iter().any(|id| id == capability))
                && eligible(&fact.when, &facts)
                && seen.insert(fact.id.clone())
        })
        .map(|fact| ProgramFactView {
            id: fact.id.clone(),
            text: fact.text.clone(),
        })
        .collect()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
