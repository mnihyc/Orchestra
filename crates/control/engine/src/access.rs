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
        /// The arrangement types the entries that let them do what is asked
        /// allow (an entry is one grant, so these are the entries that allow
        /// the model, the effort and the place together), which can only
        /// narrow what the provider's owner allows: the ones an entry names,
        /// or where it names none those in which the credential stays where
        /// it is kept. A use held as an owner or a manager allows all of
        /// them. None where the answer says nothing of the entry.
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
    /// The work this particular request is admitted for.
    pub purpose: orch_access_contract::permissions::ProviderPurpose,
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
    /// Where the work runs: the entries by which the person uses the provider have to let work
    /// run there. None for work that runs in no environment (Ask, summaries, the semantic index),
    /// which no entry holds to a place.
    pub runs_in: Option<RunsIn>,
}

/// Recovery and provider grants name the same actual work even when their wire words differ.
#[must_use]
pub fn provider_purpose(
    purpose: orch_provider_contract::calls::Purpose,
) -> orch_access_contract::permissions::ProviderPurpose {
    use orch_access_contract::permissions::ProviderPurpose as P;
    use orch_provider_contract::calls::Purpose;
    match purpose {
        Purpose::Agent => P::Turn,
        Purpose::Summary => P::Summary,
        Purpose::Embedding => P::Embeddings,
        Purpose::Review => P::Review,
        Purpose::WebSearch => P::WebSearch,
        Purpose::PageFetch => P::PageFetch,
        Purpose::ImageGeneration => P::ImageGeneration,
    }
}

/// The environment that work runs in, as far as the entries of a Use row are concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunsIn {
    /// An environment that exists.
    Environment(Id),
    /// An environment that is still to be made on a host.
    NewOnHost(Id),
    /// An environment that is still to be made as a directory of a machine.
    NewOnMachine(Id),
}

impl RunsIn {
    /// The environment a thread has, where it has one: the place its turns run in.
    #[must_use]
    pub fn of(environment: Option<Id>) -> Option<Self> {
        environment.map(Self::Environment)
    }
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

/// The environment a thread runs in now, where it has one.
pub(crate) async fn thread_environment(
    conn: &mut Conn,
    thread: Id,
) -> Result<Option<Id>, crate::EngineError> {
    let found: Option<Option<Id>> =
        sqlx::query_scalar("SELECT environment_id FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_optional(conn)
            .await
            .map_err(orch_db::DbError::from)?;
    Ok(found.flatten())
}

/// Where the work for `node` runs, when `node` is a thread that works in an environment: what the
/// entries by which a provider is used are held to for the tool calls an agent makes from it. A
/// node that is no thread, or a thread in no environment, has none: the work that belongs to no
/// environment (Ask, summaries, embeddings, the semantic index) is not held to a place.
pub(crate) async fn thread_place(
    conn: &mut Conn,
    node: Id,
) -> Result<Option<orch_access_contract::whereabouts::Whereabouts>, crate::EngineError> {
    Ok(match thread_environment(conn, node).await? {
        Some(environment) => Some(orch_authz::usable::place_of(conn, environment).await?),
        None => None,
    })
}

/// The arrangement types in which the credential stays where it is kept: what an entry that names
/// no type allows. A chooser is held to these when the answer about the entry of a person is a
/// refusal that says nothing of the place, so that it never stores a type that lets the credential
/// leave on the strength of an entry that does not name it.
#[must_use]
pub(crate) fn staying_types() -> Vec<ArrangementType> {
    orch_access_contract::permissions::ProviderUse::default()
        .arrangements_allowed()
        .into_iter()
        .filter_map(|arrangement| {
            serde_json::from_value(serde_json::Value::String(arrangement.as_str().to_owned())).ok()
        })
        .collect()
}

/// What a person is told who puts somebody else's thread in an environment that the entry of the
/// thread's first owner leaves out: that place, and nothing of who could give the entry, who owns
/// the provider or what it is called, since it is not their own access that was asked about. No
/// request can be made from it, and nothing forces it.
#[must_use]
pub(crate) fn only_the_place(
    provider: Id,
    missing: Vec<MissingAccess>,
) -> orch_access_contract::decisions::Refusal {
    use orch_access_contract::decisions::{MissingObject, MissingOptions, ObjectRef, Refusal};
    use orch_access_contract::permissions::{ObjectKind, Permission};
    Refusal {
        required: Permission::ProviderUse,
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
        requestable: false,
        r#override: false,
        missing: missing
            .into_iter()
            .filter(MissingAccess::leaves_out_the_place)
            .map(|found| MissingAccess {
                // Neither the owner nor the name of the provider: a placer who may not see it learns
                // nothing of it from the refusal.
                object: MissingObject {
                    name: None,
                    owner: None,
                    ..found.object
                },
                permissions: found.permissions,
                options: found.options.map(|options| MissingOptions {
                    r#where: options.r#where,
                    ..MissingOptions::default()
                }),
                ask: Vec::new(),
                ask_more: 0,
                r#override: false,
                closed: None,
                locked_by: None,
            })
            .collect(),
    }
}
