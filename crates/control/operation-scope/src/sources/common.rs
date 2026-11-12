//! Read preparation shared by every candidate of a not-yet-resolved native variant.

use super::{Facts, bind_known, declaration_error, indices};
use crate::{BindingError, Prepared};
use orch_access_contract::key_scope::KeyAction;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::{self, FacetUsage};
use orch_api_contract::routes::Route;
use orch_authz::Caller;

/// Request-local known sources, never a selected operation or complete effect proof.
#[derive(Clone)]
pub struct Common {
    plans: Vec<Prepared>,
    read_only: bool,
}

#[cfg(test)]
mod tests;

impl Common {
    /// All canonical variants must declare the same supplied required Read roles.
    /// Missing later roles stay unresolved; response roles and effects cannot enter this phase.
    pub fn bind<S: Eq + 'static>(
        route: Route,
        rows: &[Declaration<S>],
        facts: &[Facts<S>],
    ) -> Result<Self, BindingError> {
        Self::bind_phase(route, rows, facts, true)
    }

    /// Complete known native preparation roles, including actual setup prerequisites.
    /// Later business roles remain unresolved and no exact operation is admitted.
    pub fn bind_preparation<S: Eq + 'static>(
        route: Route,
        rows: &[Declaration<S>],
        facts: &[Facts<S>],
    ) -> Result<Self, BindingError> {
        Self::bind_phase(route, rows, facts, false)
    }

    fn bind_phase<S: Eq + 'static>(
        route: Route,
        rows: &[Declaration<S>],
        facts: &[Facts<S>],
        read_only: bool,
    ) -> Result<Self, BindingError> {
        let catalogue = operations::for_route(&route)?;
        if rows.is_empty()
            || rows.len() != catalogue.variants.len()
            || facts.is_empty()
            || rows.iter().enumerate().any(|(index, row)| {
                rows[..index]
                    .iter()
                    .any(|earlier| earlier.operation == row.operation)
            })
        {
            return Err(declaration_error(route.name));
        }
        let mut plans = Vec::with_capacity(rows.len());
        let mut common = None;
        for row in rows {
            let plan = bind_known(route, row, facts)?;
            let mut roles = Vec::new();
            for fact in facts {
                let bindings = row
                    .bindings
                    .iter()
                    .filter(|binding| binding.source == fact.source)
                    .collect::<Vec<_>>();
                if bindings.is_empty() {
                    return Err(declaration_error(row.operation));
                }
                for binding in bindings {
                    for index in indices(&plan, binding)? {
                        let facet = &plan.variant().facets[index];
                        if facet.usage != FacetUsage::Required
                            || (read_only && facet.action != KeyAction::Read)
                        {
                            return Err(declaration_error(row.operation));
                        }
                        roles.push((facet.clone(), binding.cardinality));
                    }
                }
            }
            if common.as_ref().is_some_and(|common| *common != roles)
                || plan.known_required_facets().is_empty()
            {
                return Err(declaration_error(row.operation));
            }
            common = Some(roles);
            plans.push(plan);
        }
        Ok(Self { plans, read_only })
    }

    /// The union contains only independently allowed copies of the same native Read.
    /// A key selecting just one candidate can inspect the source; it cannot later execute
    /// another candidate without that candidate's full, freshly bound admission.
    pub fn preparation_caller(&self, caller: &Caller) -> Result<Caller, BindingError> {
        let mut facets = Vec::new();
        for plan in &self.plans {
            match plan.compatible(caller) {
                Ok(()) => {}
                Err(BindingError::Refused) => continue,
                Err(error) => return Err(error),
            }
            let known = plan.known_required_facets();
            if caller.permits_facets(&known) {
                facets.extend(known);
            }
        }
        let admitted = if self.read_only {
            caller.admit_common_read(&facets)
        } else {
            caller.admit_native_preparation(&facets)
        };
        admitted.map_err(|_| BindingError::Refused)
    }

    /// Current preparation keeps exactly the same known native subjects and roles.
    pub fn covers_common(&self, earlier: &Self) -> bool {
        self.read_only == earlier.read_only
            && self.plans.len() == earlier.plans.len()
            && earlier.plans.iter().all(|old| {
                self.plans.iter().any(|fresh| {
                    fresh.variant().operation == old.variant().operation
                        && fresh.covers_known_required_sources(old)
                })
            })
    }

    /// The resolved native producer must rebind the original byte subjects in its own slot.
    /// Current placement and all remaining roles still belong to its new full admission.
    pub fn covers(&self, resolved: &Prepared) -> bool {
        self.plans
            .iter()
            .any(|known| resolved.covers_known_required_sources(known))
    }
}
