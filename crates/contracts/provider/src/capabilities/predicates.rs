//! A registry owns the grammar of its route decisions independently of the general schema cache.
use super::model::{Binding, Declarations, Endpoint, NativeSwitch, Offering};
use orch_types::Json;
use orch_types::schema::{self, PreparedSchema};
use std::collections::BTreeMap;

type Prepared = Result<PreparedSchema, schema::Error>;

#[derive(Clone, Debug, Default)]
pub(super) struct RoutePredicates {
    routes: BTreeMap<String, Prepared>,
    related: Vec<Prepared>,
}

impl RoutePredicates {
    pub(super) fn new(declarations: &Declarations) -> Self {
        let routes = declarations
            .routes
            .iter()
            .filter(|(_, route)| !route.eligible_when.is_null())
            .map(|(id, route)| (id.clone(), schema::prepare(&route.eligible_when)))
            .collect();
        let mut related = Vec::new();
        let mut keep = |value: &Json| {
            if !value.is_null() {
                related.push(schema::prepare(value));
            }
        };
        for interface in declarations.interfaces.values() {
            for binding in interface.native_tools.values() {
                keep(&binding.eligible_when);
            }
        }
        for program in declarations.programs.values() {
            for switch in program.native_switches.values() {
                keep(&switch.when);
            }
            for fact in &program.facts {
                keep(&fact.when);
            }
            for access in program.access.values() {
                for support in access.native.values() {
                    keep(&support.eligible_when);
                }
                for fact in &access.facts {
                    keep(&fact.when);
                }
            }
        }
        for provider in declarations.providers.values() {
            account_predicates(&provider.endpoints, &provider.offerings, &mut keep);
        }
        for rule in &declarations.endpoint_rules {
            keep(&rule.when);
            switches(&rule.native_switches, &mut keep);
            bindings(&rule.native_tools, &mut keep);
            account_predicates(&rule.endpoints, &rule.offerings, &mut keep);
        }
        Self { routes, related }
    }

    pub(super) fn route(&self, id: &str) -> Option<&PreparedSchema> {
        self.routes.get(id)?.as_ref().ok()
    }

    pub(super) fn check(&self) -> Result<(), schema::Error> {
        for entry in self.routes.values().chain(&self.related) {
            entry.as_ref().map_err(Clone::clone)?.check()?;
        }
        Ok(())
    }
}

fn bindings(tools: &BTreeMap<String, Option<Binding>>, keep: &mut impl FnMut(&Json)) {
    for binding in tools.values().flatten() {
        keep(&binding.eligible_when);
    }
}

fn switches(
    programs: &BTreeMap<String, BTreeMap<String, NativeSwitch>>,
    keep: &mut impl FnMut(&Json),
) {
    for program in programs.values() {
        for switch in program.values() {
            keep(&switch.when);
        }
    }
}

fn account_predicates(
    endpoints: &BTreeMap<String, Endpoint>,
    offerings: &BTreeMap<String, Offering>,
    keep: &mut impl FnMut(&Json),
) {
    for endpoint in endpoints.values() {
        bindings(&endpoint.native_tools, keep);
    }
    for offering in offerings.values() {
        bindings(&offering.native_tools, keep);
        switches(&offering.native_switches, keep);
        for support in offering.capabilities.values() {
            keep(&support.eligible_when);
        }
    }
}
