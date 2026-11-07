//! Actual logical effects selected by the native Host cascade, before any response is formatted.

use super::*;
use std::collections::BTreeSet;

/// Intrinsic physical cleanup stays under the Host; independent logical acts keep their own subjects.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Footprint {
    pub managed: BTreeSet<(ObjectKind, Id)>,
    pub used: BTreeSet<(ObjectKind, Id)>,
    pub copy_targets: BTreeSet<Id>,
}
impl Footprint {
    /// Native accepted actions supply the subjects; neither URL hints nor DTO UUIDs do.
    pub(super) fn of(stored: &Stored) -> Self {
        let mut value = Self::default();
        for action in &stored.environments {
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
        for action in &stored.providers {
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
        if let Some(data) = &stored.data {
            value.managed.extend(
                data.purges
                    .iter()
                    .map(|(id, _)| (ObjectKind::Environment, *id)),
            );
            for mount in data.custody.mounts.iter().filter(|mount| {
                matches!(
                    mount.fate,
                    api::HostDataFate::Move | api::HostDataFate::Delete
                )
            }) {
                value
                    .used
                    .insert((ObjectKind::Mount, mount.location.mount_id));
            }
            for copy in &data.custody.copies.copies {
                value.used.insert((ObjectKind::Mount, copy.mount_id));
                value.copy_targets.insert(copy.target_machine);
                // The native copy selected a physical target. Its actual Host/Machine
                // governor is resolved on the checkpoint connection, not inferred here.
            }
        }
        value
    }
}
