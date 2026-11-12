//! What the engine asks of the access engine about resources.
//!
//! Who may do something to a node is asked of `orch-authz` directly. The
//! resource side (may this person use this provider, with this model and
//! this effort; use this agent profile; manage this environment) belongs to
//! the access engine's resource part, which the engine does not depend on:
//! it asks through [`ResourceAccess`], and the system that wires the engine
//! connects the two. Every decision stays the access engine's. The engine
//! decides nothing about a person, and when the question cannot be answered
//! it does not guess: what depended on the answer waits, or is refused.

use async_trait::async_trait;
use orch_access_contract::decisions::MissingAccess;
use orch_agent_contract::descriptor::ArrangementType;
use orch_db::Conn;
use orch_types::Id;

/// The ordinary resource-use policy at its actual home, shared by native and HTTP consumers.
pub async fn resource_policies(
    conn: &mut Conn,
    resource: Id,
) -> Result<orch_authz::usable::Policies, crate::EngineError> {
    let facts = orch_db::queries::resources::resource_facts(conn, &[resource]).await?;
    let home = facts.first().and_then(|facts| facts.home_id);
    let place = home.map_or(orch_settings::Place::System, orch_settings::Place::Node);
    let shared = orch_settings::get(
        conn,
        place,
        orch_settings_contract::registry::keys::PROVIDERS_SUBSCRIPTION_SHARING,
    )
    .await?;
    let outsiders: Option<bool> = orch_settings::get(
        conn,
        place,
        orch_settings_contract::registry::keys::SHARING_ALLOW_EXTERNAL,
    )
    .await?;
    Ok(orch_authz::usable::Policies::everywhere(
        orch_authz::usable::ResourcePolicy {
            owners_name_outsiders: outsiders.unwrap_or(false),
            shared_subscriptions: shared,
        },
    ))
}

/// The same inherited resource policies for a native-selected set, with shared home reads.
/// These are snapshot facts, never reusable authority or cached permission outcomes.
pub async fn resource_policies_among(
    conn: &mut Conn,
    resources: &[Id],
) -> Result<orch_authz::usable::Policies, crate::EngineError> {
    use orch_authz::usable::{Policies, ResourcePolicy};
    use orch_settings_contract::registry::keys;
    let facts = orch_db::queries::resources::resource_facts(conn, resources).await?;
    let homes = facts
        .iter()
        .filter_map(|row| row.home_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let sharing = orch_settings::get_at_nodes::<Vec<String>>(
        conn,
        &homes,
        keys::PROVIDERS_SUBSCRIPTION_SHARING,
    )
    .await?;
    let external =
        orch_settings::get_at_nodes::<bool>(conn, &homes, keys::SHARING_ALLOW_EXTERNAL).await?;
    let system = if facts.iter().any(|row| row.home_id.is_none()) {
        let shared_subscriptions = orch_settings::get(
            conn,
            orch_settings::Place::System,
            keys::PROVIDERS_SUBSCRIPTION_SHARING,
        )
        .await?;
        let owners_name_outsiders = orch_settings::get::<bool>(
            conn,
            orch_settings::Place::System,
            keys::SHARING_ALLOW_EXTERNAL,
        )
        .await?
        .unwrap_or(false);
        ResourcePolicy {
            owners_name_outsiders,
            shared_subscriptions,
        }
    } else {
        ResourcePolicy::default()
    };
    let mut policies = Policies::everywhere(system);
    for home in homes {
        policies = policies.at_home(
            home,
            ResourcePolicy {
                owners_name_outsiders: external.get(&home).copied().flatten().unwrap_or(false),
                shared_subscriptions: sharing.get(&home).cloned().flatten(),
            },
        );
    }
    Ok(policies)
}

/// Resolve a resource-page gift's existing sharing settings beside its ordinary use policy.
pub async fn resource_sharing_policies(
    conn: &mut Conn,
    resource: Id,
) -> Result<orch_authz::usable::Policies, crate::EngineError> {
    let policies = resource_policies(conn, resource).await?;
    let rows = orch_db::queries::resources::resource_facts(conn, &[resource]).await?;
    let home = rows.first().and_then(|row| row.home_id);
    let place = home.map_or(orch_settings::Place::System, orch_settings::Place::Node);
    let sharing: Option<bool> = orch_settings::get(
        conn,
        place,
        orch_settings_contract::registry::keys::SHARING_ALLOW_HOLDERS,
    )
    .await?;
    let mut policy = policies.at(home).node_policy();
    policy.holders_share = sharing.unwrap_or(true);
    Ok(policies.with_sharing(home, policy))
}

/// The resource side of the access engine could not answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccessFault {
    /// Nothing is connected to answer.
    #[error("the resource side of the access engine is not connected")]
    NotConnected,
    /// The question was asked and the answer could not be made.
    #[error("{0}")]
    Failed(String),
}

/// The answer to a question about a person and a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// They may.
    Allowed {
        /// The arrangement types the entry they use the provider through
        /// names, which can only narrow what the provider's owner allows;
        /// none where it names none.
        arrangements: Option<Vec<ArrangementType>>,
    },
    /// They may not, and this is everything they lack.
    Refused(Vec<MissingAccess>),
}

impl Answer {
    /// They may, through an entry that narrows nothing.
    #[must_use]
    pub const fn allowed() -> Self {
        Self::Allowed { arrangements: None }
    }

    /// Whether they may.
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed { .. })
    }
}

/// May a person run a provider, as a thread is set to use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderUse<'a> {
    /// The person: the author of an input, the thread's first owner for an
    /// input that has none, or whoever puts a thread on the provider.
    pub person: Id,
    /// The node the work is at: the thread, or the topic a thread is being
    /// made in.
    pub node: Id,
    /// The provider.
    pub provider: Id,
    /// The model; none asks about the provider alone.
    pub model: Option<&'a str>,
    /// The reasoning effort.
    pub effort: Option<&'a str>,
}

/// A limit that stands in the way of a turn, which somebody may let it past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Legacy provider bypass-permission query, retained for implementer compatibility.
    /// Current inputs bypass only configured caps; vendor quota is not a gate.
    ProviderLimit {
        /// The provider.
        provider: Id,
    },
    /// A usage cap.
    Cap {
        /// The cap.
        cap: Id,
    },
}

/// What the engine asks about resources.
#[async_trait]
pub trait ResourceAccess: Send + Sync {
    /// Whether the person may use the provider with the model and the
    /// effort, for work at the node (`provider.use`, with the entry's options
    /// and the allowlists that apply there).
    async fn provider_use(
        &self,
        conn: &mut Conn,
        ask: &ProviderUse<'_>,
    ) -> Result<Answer, AccessFault>;

    /// Checks a setting writer's provider and model for background work, independently of creating a thread.
    async fn background_provider_use(
        &self,
        conn: &mut Conn,
        ask: &ProviderUse<'_>,
    ) -> Result<Answer, AccessFault> {
        self.provider_use(conn, ask).await
    }

    /// Whether the person may use the agent profile for work at the node.
    async fn agent_use(
        &self,
        conn: &mut Conn,
        person: Id,
        node: Id,
        agent: Id,
    ) -> Result<Answer, AccessFault>;

    /// Whether the person may put a new thread into the environment: its Use
    /// (`environment.use`), which also lets the thread's agent act for them there.
    async fn environment_place(
        &self,
        conn: &mut Conn,
        person: Id,
        environment: Id,
    ) -> Result<Answer, AccessFault>;

    /// Whether the person manages the environment (`environment.manage`), which
    /// moving, forking or copying work into it needs.
    async fn environment_manage(
        &self,
        conn: &mut Conn,
        person: Id,
        environment: Id,
    ) -> Result<Answer, AccessFault>;

    /// Whether the person holds the bypass row for the limit, so that a turn
    /// of theirs may go past it. `as_admin` is whether they act with As admin on,
    /// as the session they let it past from did: a system admin passes a cap of
    /// the system only so.
    async fn may_pass(
        &self,
        conn: &mut Conn,
        person: Id,
        as_admin: bool,
        limit: &Limit,
    ) -> Result<bool, AccessFault>;
}

/// What stands in until the access engine's resource side is connected: it
/// answers nothing, so that nothing that depends on it goes ahead.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unconnected;

#[async_trait]
impl ResourceAccess for Unconnected {
    async fn provider_use(&self, _: &mut Conn, _: &ProviderUse<'_>) -> Result<Answer, AccessFault> {
        Err(AccessFault::NotConnected)
    }

    async fn agent_use(&self, _: &mut Conn, _: Id, _: Id, _: Id) -> Result<Answer, AccessFault> {
        Err(AccessFault::NotConnected)
    }

    async fn environment_place(&self, _: &mut Conn, _: Id, _: Id) -> Result<Answer, AccessFault> {
        Err(AccessFault::NotConnected)
    }

    async fn environment_manage(&self, _: &mut Conn, _: Id, _: Id) -> Result<Answer, AccessFault> {
        Err(AccessFault::NotConnected)
    }

    async fn may_pass(&self, _: &mut Conn, _: Id, _: bool, _: &Limit) -> Result<bool, AccessFault> {
        Err(AccessFault::NotConnected)
    }
}
