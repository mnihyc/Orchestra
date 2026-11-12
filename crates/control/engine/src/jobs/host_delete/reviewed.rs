//! Native Host cascade actions and their factual subjects, shared by planning and retained execution.
use super::Custody;
use crate::{
    jobs::{environment_move, machine_move},
    recovery, updates,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::{environment::EnvironmentMovePreflightBody, loss as api};
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum EnvironmentAction {
    Recreate {
        recipe: Box<recovery::Recipe>,
    },
    Move {
        recipe: Box<environment_move::MoveRecipe>,
        request: EnvironmentMovePreflightBody,
    },
    Portable {
        recipe: Box<machine_move::Recipe>,
        request: EnvironmentMovePreflightBody,
    },
    Remove {
        id: Id,
        revision: i64,
        lost: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ProviderAction {
    Leave {
        provider: Id,
        runtime: Id,
    },
    Replace {
        recipe: Box<updates::Recipe>,
        host: Id,
    },
    Make {
        recipe: Box<crate::provider_recovery::Recipe>,
    },
}
/// Native independent management acts, selected placements and physical copy destinations.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Footprint {
    pub managed: BTreeSet<(ObjectKind, Id)>,
    pub used: BTreeSet<(ObjectKind, Id)>,
    pub copy_targets: BTreeSet<Id>,
}
impl Footprint {
    /// Select directly from authored native actions, before any public plan projection.
    pub fn of(
        environments: &[EnvironmentAction],
        providers: &[ProviderAction],
        data: Option<(&Custody, &[(Id, i64)])>,
    ) -> Self {
        let mut value = Self::default();
        for action in environments {
            let environment = match action {
                EnvironmentAction::Recreate { recipe } => {
                    value.used.insert((ObjectKind::Host, recipe.host));
                    value.used.insert((ObjectKind::Template, recipe.template));
                    recipe.environment
                }
                EnvironmentAction::Move { recipe, .. } => {
                    value.used.insert((ObjectKind::Host, recipe.destination));
                    value
                        .used
                        .insert((ObjectKind::Template, recipe.replacement.template));
                    recipe.replacement.target.id()
                }
                EnvironmentAction::Portable { recipe, .. } => {
                    value.used.extend(
                        recipe
                            .destination_access()
                            .into_iter()
                            .map(|(object, _)| (object.kind, object.id)),
                    );
                    recipe.environment
                }
                EnvironmentAction::Remove { id, .. } => *id,
            };
            value.managed.insert((ObjectKind::Environment, environment));
        }
        for action in providers {
            match action {
                // Leaving this Host's runtime behind is the Host manager's intrinsic control.
                ProviderAction::Leave { .. } => {}
                ProviderAction::Replace { recipe, host } => {
                    value
                        .managed
                        .insert((ObjectKind::Provider, recipe.target.id()));
                    value.used.insert((ObjectKind::Host, *host));
                    value.used.insert((ObjectKind::Template, recipe.template));
                }
                ProviderAction::Make { recipe } => {
                    value
                        .managed
                        .insert((ObjectKind::Provider, recipe.provider));
                    value.used.insert((ObjectKind::Host, recipe.host));
                    value.used.insert((ObjectKind::Template, recipe.template));
                }
            }
        }
        if let Some((custody, purges)) = data {
            value
                .managed
                .extend(purges.iter().map(|(id, _)| (ObjectKind::Environment, *id)));
            for mount in custody.mounts.iter().filter(|mount| {
                matches!(
                    mount.fate,
                    api::HostDataFate::Move | api::HostDataFate::Delete
                )
            }) {
                value
                    .used
                    .insert((ObjectKind::Mount, mount.location.mount_id));
            }
            for copy in &custody.copies.copies {
                value.used.insert((ObjectKind::Mount, copy.mount_id));
                value.copy_targets.insert(copy.target_machine);
                // The native copy selected a physical target. Its actual Host/Machine
                // governor is resolved on the checkpoint connection, not inferred here.
            }
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intrinsic_provider_retirement_keeps_its_host_role_while_independent_removal_keeps_its_environment()
     {
        let environment = orch_types::new_id();
        let provider = orch_types::new_id();
        let footprint = Footprint::of(
            &[EnvironmentAction::Remove {
                id: environment,
                revision: 1,
                lost: false,
            }],
            &[ProviderAction::Leave {
                provider,
                runtime: orch_types::new_id(),
            }],
            None,
        );
        assert_eq!(
            footprint.managed,
            [(ObjectKind::Environment, environment)]
                .into_iter()
                .collect()
        );
        assert!(footprint.used.is_empty());
        assert!(footprint.copy_targets.is_empty());
    }
}
