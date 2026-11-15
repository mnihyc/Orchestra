//! Exact resource targets and live pool definitions, without execution or authority.

use crate::capabilities::BackendRef;
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The interchangeable resource family a choice requires.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "PoolKind")]
pub enum Kind {
    /// An exact model or capability backend.
    Model,
    /// A provider, retaining the consumer's model or service requirement.
    Provider,
    /// An existing environment or an explicitly described new one.
    Environment,
    /// A host for an explicitly admitted creation or move.
    Host,
}

impl Kind {
    /// Its stored family name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Provider => "provider",
            Self::Environment => "environment",
            Self::Host => "host",
        }
    }
}

/// A concrete target; meaningful backend placement/config is retained in full.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "ResourceLeaf")]
pub enum Leaf {
    /// An exact route, account, offering and adapter configuration.
    Model { backend: BackendRef },
    /// The account alone; its caller supplies the unchanged model/service requirements.
    Provider { id: Id },
    /// An existing execution environment.
    Environment { id: Id },
    /// A new environment made under the existing template and host authorities.
    EnvironmentTemplate { template_id: Id, host_id: Id },
    /// An actual host.
    Host { id: Id },
}

impl Leaf {
    /// The family this leaf can supply.
    #[must_use]
    pub const fn kind(&self) -> Kind {
        match self {
            Self::Model { .. } => Kind::Model,
            Self::Provider { .. } => Kind::Provider,
            Self::Environment { .. } | Self::EnvironmentTemplate { .. } => Kind::Environment,
            Self::Host { .. } => Kind::Host,
        }
    }

    /// Checks structure only; an identifier is never permission or support.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Model { backend } => {
                if backend.route.trim().is_empty()
                    || backend.offering.as_ref().is_none_or(|offering| {
                        offering.provider.is_nil() || offering.offering.trim().is_empty()
                    })
                    || !(backend.config.is_object() || backend.config.is_null())
                {
                    return Err("choose an exact route and account offering".into());
                }
            }
            Self::Provider { id } | Self::Environment { id } | Self::Host { id } => {
                if id.is_nil() {
                    return Err("choose a resource".into());
                }
            }
            Self::EnvironmentTemplate {
                template_id,
                host_id,
            } => {
                if template_id.is_nil() || host_id.is_nil() {
                    return Err("choose a template and a host".into());
                }
            }
        }
        Ok(())
    }
}

/// Stored intent is separate from the concrete member eventually admitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "ResourceTarget")]
pub enum Target {
    /// One explicitly selected resource.
    Single { resource: Leaf },
    /// A configured pool of the same family as the consuming control.
    Pool { id: Id, kind: Kind },
}

impl Target {
    /// Checks the resource family before native preview or admission.
    pub fn validate(&self, expected: Kind) -> Result<(), String> {
        let actual = match self {
            Self::Single { resource } => {
                resource.validate()?;
                resource.kind()
            }
            Self::Pool { id, kind } => {
                if id.is_nil() {
                    return Err("choose a pool".into());
                }
                *kind
            }
        };
        if actual != expected {
            return Err(format!(
                "choose a {} or a pool of {} resources",
                expected.as_str(),
                expected.as_str()
            ));
        }
        Ok(())
    }

    /// Canonical content equality includes all backend configuration and extension fields.
    pub fn identity(&self) -> Result<String, serde_json::Error> {
        fn ordered(value: Json) -> Json {
            match value {
                Json::Object(map) => {
                    let sorted = map
                        .into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>();
                    Json::Object(
                        sorted
                            .into_iter()
                            .map(|(key, value)| (key, ordered(value)))
                            .collect(),
                    )
                }
                Json::Array(values) => Json::Array(values.into_iter().map(ordered).collect()),
                other => other,
            }
        }
        serde_json::to_string(&ordered(serde_json::to_value(self)?))
    }
}

/// One live member rule; every expansion is constrained to its pool's root and current authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "PoolMemberRule")]
pub enum MemberRule {
    /// One concrete resource/backend.
    Single { resource: Leaf },
    /// A same-family inner pool, retaining its own policy.
    Pool { id: Id, kind: Kind },
    /// Enabled models of one account, optionally narrowed to its provider-local group.
    ProviderModels {
        provider_id: Id,
        group: Option<String>,
    },
    /// Existing environments on the named host.
    HostEnvironments { host_id: Id },
    /// A deliberately new environment from this exact template on this host.
    TemplateOnHost { template_id: Id, host_id: Id },
    /// The exact existing display-group identity, including missing values.
    Grouping { kind: Kind, group: DisplayGroup },
    /// A live metadata rule, never the page of rows currently loaded in a browser.
    Matching {
        kind: Kind,
        home_id: Option<Id>,
        tag_id: Option<Id>,
        owner_id: Option<Id>,
        machine_id: Option<Id>,
    },
}

impl MemberRule {
    /// Checks kind and structural identity without resolving a grant.
    pub fn validate(&self, expected: Kind) -> Result<(), String> {
        match self {
            Self::Single { resource } => Target::Single {
                resource: resource.clone(),
            }
            .validate(expected),
            Self::Pool { id, kind } => Target::Pool {
                id: *id,
                kind: *kind,
            }
            .validate(expected),
            Self::ProviderModels { provider_id, group } => {
                if expected != Kind::Model
                    || provider_id.is_nil()
                    || group.as_ref().is_some_and(|g| g.trim().is_empty())
                {
                    return Err(
                        "choose a provider model rule with a nonempty group when supplied".into(),
                    );
                }
                Ok(())
            }
            Self::HostEnvironments { host_id } => {
                if expected != Kind::Environment || host_id.is_nil() {
                    return Err("a host environment rule belongs to an environment pool".into());
                }
                Ok(())
            }
            Self::TemplateOnHost {
                template_id,
                host_id,
            } => Target::Single {
                resource: Leaf::EnvironmentTemplate {
                    template_id: *template_id,
                    host_id: *host_id,
                },
            }
            .validate(expected),
            Self::Grouping { kind, group } => {
                if *kind != expected || group.ids().into_iter().flatten().any(|id| id.is_nil()) {
                    return Err("the display group must name the pool's resource family".into());
                }
                Ok(())
            }
            Self::Matching {
                kind,
                home_id,
                tag_id,
                owner_id,
                machine_id,
            } => {
                if *kind != expected
                    || [home_id, tag_id, owner_id, machine_id]
                        .into_iter()
                        .flatten()
                        .any(|id| id.is_nil())
                {
                    return Err("the live member rule must name the pool's resource family".into());
                }
                Ok(())
            }
        }
    }
}

/// A native measured quantity, never an invented capacity percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "PoolLoadMeasure")]
pub enum LoadMeasure {
    /// Observed CPU usage as a ratio.
    Cpu,
    /// Observed used memory over actual capacity.
    Memory,
    /// The largest of the reported CPU and memory ratios.
    Combined,
    /// Actual running threads, in threads.
    Threads,
    /// Actual running processes, in processes.
    Processes,
}

/// The explicitly selected rule within the first feasible priority tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "PoolPolicy")]
pub enum Policy {
    /// Saved order.
    Ordered,
    /// Random proportional weights.
    Weighted,
    /// Durable smooth weighted rotation.
    RoundRobin,
    /// Comparable request estimates, with unknown rates retained after known ones.
    LeastCost,
    /// A native measured quantity with truthful unknown states.
    LeastLoaded { measure: LoadMeasure },
}

/// A written member with optional inherited numeric defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PoolMember")]
pub struct Member {
    /// Stable member identity through edits and reorders.
    pub id: Id,
    /// Live membership rule.
    pub rule: MemberRule,
    /// Whether this row participates; distinct from a zero weight.
    pub enabled: bool,
    /// Lower first; omitted uses the effective registry default.
    pub priority: Option<u64>,
    /// Positive exact decimal weight; omitted uses the effective registry default.
    pub weight: Option<String>,
}

/// Stored pool behavior; ordinary ownership, home and revision remain resource facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PoolDefinition")]
pub struct Definition {
    /// Immutable member family.
    pub kind: Kind,
    /// Disabled drafts remain inspectable.
    pub enabled: bool,
    /// Required before enabling.
    pub policy: Option<Policy>,
    /// Pool-side consent; member-side consent and current giver authority are also required.
    pub lend_members: bool,
    /// Written order, without a total member cap.
    pub members: Vec<Member>,
    /// Registered pool own-setting overrides, never authority or credential material.
    pub config: Json,
}

impl Definition {
    /// Checks the definition before the transactional native graph/access checks.
    pub fn validate(&self) -> Result<(), String> {
        if self.enabled && self.policy.is_none() {
            return Err("choose a policy before enabling the pool".into());
        }
        if !self.config.is_object() {
            return Err("pool setting overrides must be an object".into());
        }
        if matches!(self.policy, Some(Policy::LeastCost))
            && !matches!(self.kind, Kind::Model | Kind::Provider)
        {
            return Err("least cost needs a model or provider request".into());
        }
        if matches!(self.policy, Some(Policy::LeastLoaded { .. }))
            && !matches!(self.kind, Kind::Host | Kind::Environment)
        {
            return Err("least loaded needs a host or environment".into());
        }
        let mut ids = BTreeSet::new();
        for member in &self.members {
            if member.id.is_nil() || !ids.insert(member.id) {
                return Err("each member needs a distinct identity".into());
            }
            member.rule.validate(self.kind)?;
            if let Some(weight) = &member.weight
                && !valid_weight(weight)
            {
                return Err("a member weight must be a positive decimal".into());
            }
        }
        Ok(())
    }
}

/// Exact positive decimal syntax, without an arbitrary precision or magnitude cap.
#[must_use]
pub fn valid_weight(value: &str) -> bool {
    let mut dot = false;
    let mut positive = false;
    !value.is_empty()
        && value.chars().all(|character| {
            if character == '.' && !dot {
                dot = true;
                true
            } else if character.is_ascii_digit() {
                positive |= character != '0';
                true
            } else {
                false
            }
        })
        && positive
}

/// Display groups describe exact metadata identities; a missing field does not mean any value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "by", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "PoolDisplayGroup")]
pub enum DisplayGroup {
    Home {
        id: Option<Id>,
    },
    Owner {
        owner_id: Option<Id>,
        home_id: Option<Id>,
    },
    Machine {
        id: Option<Id>,
    },
    Tag {
        id: Option<Id>,
    },
}
impl DisplayGroup {
    fn ids(&self) -> [Option<Id>; 2] {
        match self {
            Self::Home { id } | Self::Machine { id } | Self::Tag { id } => [*id, None],
            Self::Owner { owner_id, home_id } => [*owner_id, *home_id],
        }
    }
}
