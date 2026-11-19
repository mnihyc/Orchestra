//! Factual declaration rows shared by transport adapters and native continuing work.
//! A bound plan carries no caller admission; the original operation and ordinary gate still apply.

use super::{BindingError, Prepared};
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::operation_sources::{Binding, Cardinality, Declaration};
use orch_api_contract::operations::{CatalogueError, TargetResolver};
use orch_api_contract::routes::Route;

/// Actual native source facts supplied before any response serialization or effect.
#[derive(Clone)]
pub struct Facts<S> {
    /// The server-owned domain role.
    pub source: S,
    /// Explicit singleton or genuinely selected collection.
    pub cardinality: Cardinality,
    /// Actual business families and their real target placement, not guessed governor kinds.
    pub groups: Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>,
}

impl<S> Facts<S> {
    /// One actual subject of its explicit business family.
    pub fn one(source: S, family: KeyTargetKind, target: KeyResolvedTarget) -> Self {
        Self {
            source,
            cardinality: Cardinality::One,
            groups: vec![(family, vec![target])],
        }
    }
    /// One actually selected collection, possibly empty; absence is never this proof.
    pub fn collection(source: S, groups: Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>) -> Self {
        Self {
            source,
            cardinality: Cardinality::Collection,
            groups,
        }
    }
}

fn declaration_error(selection: &str) -> BindingError {
    BindingError::Catalogue(CatalogueError::Invalid {
        operation: selection.to_owned(),
        reason: "the native source declaration does not match its operation".into(),
    })
}

/// Every canonical action of this role/usage, without restating an action policy in the row.
pub fn indices<S>(plan: &Prepared, binding: &Binding<S>) -> Result<Vec<usize>, BindingError> {
    let indices = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.target == binding.target && facet.usage == binding.usage)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if indices.is_empty() {
        return Err(declaration_error(&plan.variant().operation));
    }
    Ok(indices)
}

/// Complete binding of every declared required and response role for an explicit native stage.
pub fn bind<S: Eq + 'static>(
    route: Route,
    row: &Declaration<S>,
    facts: &[Facts<S>],
) -> Result<Prepared, BindingError> {
    bind_facts(route, row, facts, true)
}

/// Only actual known facts for effect-free preparation. Missing roles remain unresolved.
pub fn bind_known<S: Eq + 'static>(
    route: Route,
    row: &Declaration<S>,
    facts: &[Facts<S>],
) -> Result<Prepared, BindingError> {
    bind_facts(route, row, facts, false)
}

/// Every source declared by an actual native response shape is present. Other canonical
/// roles remain unresolved, including unproduced success singletons and unperformed effects.
/// This is factual response preparation only, never complete operation admission.
pub fn bind_declared<S: Eq + 'static>(
    route: Route,
    row: &Declaration<S>,
    facts: &[Facts<S>],
) -> Result<Prepared, BindingError> {
    let plan = bind_known(route, row, facts)?;
    if row
        .bindings
        .iter()
        .any(|binding| !facts.iter().any(|fact| fact.source == binding.source))
    {
        return Err(declaration_error(row.operation));
    }
    Ok(plan)
}

/// Every required native role is complete; optional response sources may remain unresolved.
pub fn bind_required<S: Eq + 'static>(
    route: Route,
    row: &Declaration<S>,
    facts: &[Facts<S>],
) -> Result<Prepared, BindingError> {
    let plan = bind_facts(route, row, facts, false)?;
    plan.required_facets()?;
    Ok(plan)
}

fn bind_facts<S: Eq + 'static>(
    route: Route,
    row: &Declaration<S>,
    facts: &[Facts<S>],
    complete: bool,
) -> Result<Prepared, BindingError> {
    let mut plan = Prepared::new(route, row.operation)?;
    let mut filled = vec![false; plan.variant().facets.len()];
    for binding in row.bindings {
        let binding_indices = indices(&plan, binding)?;
        let mut matches = facts.iter().filter(|fact| fact.source == binding.source);
        let Some(fact) = matches.next() else {
            if complete {
                return Err(declaration_error(row.operation));
            }
            continue;
        };
        if matches.next().is_some() || fact.cardinality != binding.cardinality {
            return Err(declaration_error(row.operation));
        }
        for index in binding_indices {
            if filled[index] {
                return Err(declaration_error(row.operation));
            }
            match binding.cardinality {
                Cardinality::One => {
                    let [(family, targets)] = fact.groups.as_slice() else {
                        return Err(declaration_error(row.operation));
                    };
                    if targets.len() != 1
                        || matches!(binding.target, TargetResolver::ResolvedCollection(_))
                    {
                        return Err(declaration_error(row.operation));
                    }
                    plan.bind(index, *family, targets)?;
                }
                Cardinality::Collection => {
                    if !matches!(binding.target, TargetResolver::ResolvedCollection(_)) {
                        return Err(declaration_error(row.operation));
                    }
                    if fact.groups.is_empty() {
                        plan.bind_empty(index)?;
                    } else {
                        plan.bind_collection(index, &fact.groups)?;
                    }
                }
            }
            filled[index] = true;
        }
    }
    if complete && filled.iter().any(|filled| !filled)
        || facts.iter().any(|fact| {
            !row.bindings
                .iter()
                .any(|binding| binding.source == fact.source)
        })
    {
        return Err(declaration_error(row.operation));
    }
    Ok(plan)
}
