//! Pool-side typed limits and explicit member-side lending destinations.
use crate::permissions::{HostUse, ProviderUse, ResourceLimits, TemplateUse};
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One pool Use entry keeps its complete limits together through the whole choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PoolUse {
    pub provider: ProviderUse,
    pub host: HostUse,
    pub template: TemplateUse,
}
impl PoolUse {
    /// Management and ownership carry no implicit credential-copy restriction of a Use entry.
    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            provider: ProviderUse::unlimited(),
            ..Default::default()
        }
    }
    /// A display union; admission must still use one complete entry, never this union.
    #[must_use]
    pub fn or(&self, other: &Self) -> Self {
        Self {
            provider: self.provider.or(&other.provider),
            host: self.host.or(&other.host),
            template: self.template.or(&other.template),
        }
    }
    /// A share cannot introduce limits or choices absent from its giver's complete entry.
    #[must_use]
    pub fn within(&self, other: &Self) -> bool {
        fn list<T: PartialEq>(one: Option<&Vec<T>>, other: Option<&Vec<T>>) -> bool {
            other.is_none_or(|other| {
                one.is_some_and(|one| one.iter().all(|value| other.contains(value)))
            })
        }
        fn cap<T: PartialOrd>(one: Option<T>, other: Option<T>) -> bool {
            other.is_none_or(|other| one.is_some_and(|one| one <= other))
        }
        fn limits(one: Option<&ResourceLimits>, other: Option<&ResourceLimits>) -> bool {
            other.is_none_or(|other| {
                one.is_some_and(|one| {
                    cap(one.cpu_millis, other.cpu_millis)
                        && cap(one.memory_mb, other.memory_mb)
                        && cap(one.pids, other.pids)
                        && cap(one.disk_gb, other.disk_gb)
                })
            })
        }
        self.provider.within(&other.provider)
            && cap(self.host.max_environments, other.host.max_environments)
            && list(self.host.storages.as_ref(), other.host.storages.as_ref())
            && list(self.host.sources.as_ref(), other.host.sources.as_ref())
            && list(self.host.templates.as_ref(), other.host.templates.as_ref())
            && limits(
                self.host.resources_max.as_ref(),
                other.host.resources_max.as_ref(),
            )
            && list(
                self.template.capabilities.as_ref(),
                other.template.capabilities.as_ref(),
            )
            && limits(
                self.template.resources_max.as_ref(),
                other.template.resources_max.as_ref(),
            )
    }
}

/// Which actual pools may use this member-side consent; no destination grants direct member Use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LendingMatcher {
    Pool { pool_id: Id },
    Home { home_id: Id },
    Managers { manager_ids: Vec<Id> },
}

/// Retained membership observations, never a serialized grant; native admission rereads every hop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathMember {
    pub pool_id: Id,
    pub revision: i64,
    pub member_id: Id,
}

/// The actual member page or inherited home whose lending policy is edited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "PoolLendingScope")]
pub enum LendingScope {
    Member {
        object: crate::decisions::ObjectRef,
    },
    Defaults {
        at: crate::roles::PlaceRef,
        resource_kind: crate::permissions::ResourceKind,
    },
}
