//! Hierarchy traversal down to the thread, the environment or the provider concerned, and environment-supplied defaults.
//!
//! A [`Snapshot`] is what is read for a [`Place`] once: the nodes on the way
//! from the root org down to it (found by the hierarchy's own query), the
//! rows of the system and of those nodes, the own values of an environment or
//! of a provider's runtime, and the defaults that something other than the
//! registry supplies. Reading takes two statements for a node, three for an
//! environment or a provider, and the few more that a supplied default needs
//! at the place where it means something; none of it depends on how many keys
//! are asked for, so the settings screen of a node asks for all of them at
//! once. A key nobody set costs nothing beyond those rows.
//!
//! After that every key is resolved without touching the database, by the
//! contract's rules, and carries its provenance. Code that needs several
//! chosen keys at one place loads a [`Snapshot`] for them once
//! ([`Wanted::Keys`]) and asks it for each; [`resolve`] and [`get`] are that
//! for one key.

use orch_access_contract::roles::NodeKind;
use orch_db::Conn;
use orch_db::queries::hierarchy::{self, NodeRow};
use orch_db::queries::settings as queries;
use orch_settings_contract::provenance::{OwnRow, Resolved};
use orch_settings_contract::registry::{self, DefaultOf, Scope, SettingDef, Supplier, keys};
use orch_types::{Id, Json};
use serde::de::DeserializeOwned;

use crate::locks::Here;
use crate::merge::{Link, OwnValues, Rows, Stack, Supply, resolve_key};
use crate::provenance::SettingView;
use crate::supplied::{self, Supplies};
use crate::validation::keys_at;
use crate::{Error, Place};

/// How many keys deep a default that names another key is followed. The
/// registry has one hop; this stops a cycle from going round for ever.
const SAME_AS_DEPTH: usize = 4;

/// Which keys a [`Snapshot`] is read for.
#[derive(Debug, Clone, Copy)]
pub enum Wanted<'a> {
    /// Every key that is shown for the place.
    All,
    /// Only these.
    Keys(&'a [&'static SettingDef]),
}

/// Everything read for one place, from which any of its keys is resolved.
#[derive(Debug, Clone)]
pub struct Snapshot {
    place: Place,
    scope: Scope,
    chain: Vec<Link>,
    rows: Rows,
    own: Option<OwnValues>,
    supplies: Supplies,
    defs: Vec<&'static SettingDef>,
}

/// The nodes from the root org down to `id`, each with the scope its rows
/// stand at, found among `nodes` (the node with all its ancestors). A node
/// that is not there, or that was deleted, is not found.
pub(crate) fn chain_of(nodes: &[NodeRow], id: Id) -> Result<Vec<Link>, Error> {
    let node = nodes
        .iter()
        .find(|found| found.id == id)
        .ok_or(Error::NotFound)?;
    let mut chain = Vec::with_capacity(node.path.len());
    for step in &node.path {
        let found = nodes
            .iter()
            .find(|candidate| candidate.id == *step)
            .ok_or(Error::NotFound)?;
        if found.deleted_at.is_some() && !orch_db::queries::trace::retained_read(id) {
            return Err(Error::NotFound);
        }
        let kind: NodeKind = found.kind.parse().map_err(|_| Error::Unreadable {
            what: "the kind of a node",
        })?;
        chain.push(Link {
            id: *step,
            scope: Scope::from(kind),
        });
    }
    Ok(chain)
}

/// What is known of a place before its rows are read.
struct PlaceFacts {
    scope: Scope,
    chain: Vec<Link>,
    own: Option<OwnValues>,
    environment: Option<queries::EnvironmentRow>,
}

pub(crate) async fn chain_to(conn: &mut Conn, node: Id) -> Result<Vec<Link>, Error> {
    let nodes = hierarchy::with_ancestors(conn, &[node]).await?;
    chain_of(&nodes, node)
}

async fn load_place(conn: &mut Conn, place: Place) -> Result<PlaceFacts, Error> {
    Ok(match place {
        Place::System => PlaceFacts {
            scope: Scope::System,
            chain: Vec::new(),
            own: None,
            environment: None,
        },
        Place::Node(id) => {
            let chain = chain_to(conn, id).await?;
            let scope = chain.last().map(|link| link.scope).ok_or(Error::NotFound)?;
            PlaceFacts {
                scope,
                chain,
                own: None,
                environment: None,
            }
        }
        Place::Environment(id) => {
            let environment = queries::environment(conn, id)
                .await?
                .ok_or(Error::NotFound)?;
            let chain = chain_to(conn, environment.home_node_id).await?;
            PlaceFacts {
                scope: Scope::Environment,
                chain,
                own: Some(OwnValues {
                    scope: Scope::Environment,
                    values: environment.config.clone(),
                    revision: environment.revision,
                }),
                environment: Some(environment),
            }
        }
        Place::Provider(id) => {
            let provider = queries::provider_runtime(conn, id)
                .await?
                .ok_or(Error::NotFound)?;
            let chain = chain_to(conn, provider.home_id).await?;
            PlaceFacts {
                scope: Scope::Provider,
                chain,
                own: Some(OwnValues {
                    scope: Scope::Provider,
                    values: provider.runtime_config,
                    revision: provider.revision,
                }),
                environment: None,
            }
        }
        Place::Host(id) => {
            let row: Option<(Option<i64>, i64)> =
                sqlx::query_as("SELECT cleanup_keep_days,revision FROM hosts WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            let (keep_days, revision) = row.ok_or(Error::NotFound)?;
            let values = keep_days.map_or_else(
                || serde_json::json!({}),
                |value| serde_json::json!({(keys::CLEANUP_UNUSED_KEEP_DAYS): value}),
            );
            PlaceFacts {
                scope: Scope::Host,
                chain: Vec::new(),
                own: Some(OwnValues {
                    scope: Scope::Host,
                    values,
                    revision,
                }),
                environment: None,
            }
        }
    })
}

/// The keys whose rows a key is resolved from, besides its own: the other
/// key it takes its default from, and for an effort the model it belongs to.
fn dependencies(def: &SettingDef) -> impl Iterator<Item = &'static str> {
    let same_as = match def.default {
        DefaultOf::SameAs(other) => Some(other),
        _ => None,
    };
    let model = (def.key == keys::AGENT_EFFORT).then_some(keys::AGENT_MODEL);
    same_as.into_iter().chain(model)
}

impl Snapshot {
    /// Reads `place` for the keys `wanted`.
    pub async fn load(conn: &mut Conn, place: Place, wanted: Wanted<'_>) -> Result<Self, Error> {
        let facts = load_place(conn, place).await?;
        let (defs, row_keys): (Vec<&'static SettingDef>, Option<Vec<String>>) = match wanted {
            Wanted::All => (keys_at(facts.scope).collect(), None),
            Wanted::Keys(asked) => {
                let mut names: Vec<String> = asked
                    .iter()
                    .flat_map(|def| std::iter::once(def.key).chain(dependencies(def)))
                    .map(str::to_owned)
                    .collect();
                names.sort_unstable();
                names.dedup();
                (asked.to_vec(), Some(names))
            }
        };
        let ids: Vec<Id> = facts.chain.iter().map(|link| link.id).collect();
        let rows = Rows::new(queries::rows_on_chain(conn, &ids, row_keys.as_deref()).await?);
        let mut snapshot = Self {
            place,
            scope: facts.scope,
            chain: facts.chain,
            rows,
            own: facts.own,
            supplies: Supplies::default(),
            defs,
        };
        let thread = match place {
            Place::Node(id) if facts.scope == Scope::Thread => Some(id),
            _ => None,
        };
        let model_for_effort = snapshot.model_for_effort(thread.is_some());
        let context = supplied::Context {
            thread,
            environment: facts.environment.as_ref(),
            model_for_effort: model_for_effort.as_deref(),
        };
        snapshot.supplies = supplied::load(conn, &context, &snapshot.defs).await?;
        Ok(snapshot)
    }

    /// The model a thread resolves to, when the effort of the thread is asked
    /// for and nothing sets it: then what the model lists is its default.
    fn model_for_effort(&self, is_thread: bool) -> Option<String> {
        let effort = registry::get(keys::AGENT_EFFORT)?;
        if !is_thread || !self.defs.iter().any(|def| def.key == effort.key) {
            return None;
        }
        if self.resolve(effort).value.is_some() {
            return None;
        }
        let model = registry::get(keys::AGENT_MODEL)?;
        self.resolve(model)
            .value
            .and_then(|value| value.as_str().map(str::to_owned))
    }

    /// The place that was read.
    #[must_use]
    pub fn place(&self) -> Place {
        self.place
    }

    /// The scope the place's own rows stand at: that of a node's kind, the
    /// system, an environment or a provider's runtime.
    #[must_use]
    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// The nodes from the root org down to the place, or to the home of an
    /// environment or a provider.
    #[must_use]
    pub fn chain(&self) -> &[Link] {
        &self.chain
    }

    /// Whether the place is a side thread, which has no rows of its own: its
    /// settings are its parent's.
    #[must_use]
    pub fn is_side_thread(&self) -> bool {
        self.chain
            .iter()
            .filter(|link| link.scope == Scope::Thread)
            .count()
            > 1
    }

    /// The keys this snapshot was read for.
    #[must_use]
    pub fn keys(&self) -> &[&'static SettingDef] {
        &self.defs
    }

    pub(crate) fn here(&self) -> Here {
        Here::from(self.place)
    }

    pub(crate) fn rows_mut(&mut self) -> &mut Rows {
        &mut self.rows
    }

    /// What `def` resolves to at this place, with where it comes from.
    #[must_use]
    pub fn resolve(&self, def: &'static SettingDef) -> Resolved<Id> {
        self.resolve_following(def, 0)
    }

    fn resolve_following(&self, def: &'static SettingDef, depth: usize) -> Resolved<Id> {
        let stack = Stack {
            rows: &self.rows,
            chain: &self.chain,
            own: self.own.as_ref(),
        };
        let other = match def.default {
            DefaultOf::SameAs(name) if depth < SAME_AS_DEPTH => {
                registry::get(name).and_then(|other| self.resolve_following(other, depth + 1).value)
            }
            _ => None,
        };
        let supply = match def.default {
            DefaultOf::Supplied(Supplier::Environment) => {
                self.supplies.get(def.key).map(Supply::Environment)
            }
            DefaultOf::Supplied(_) => self.supplies.get(def.key).map(Supply::Default),
            DefaultOf::SameAs(_) => other.as_ref().map(Supply::Default),
            DefaultOf::Value(_) | DefaultOf::Unset => None,
        };
        resolve_key(def, &stack, supply)
    }

    /// The row of `def` at the place itself, or its own value for an
    /// environment or a provider's runtime.
    #[must_use]
    pub fn own_row(&self, def: &SettingDef) -> Option<OwnRow> {
        if !def.set_at(self.scope) {
            return None;
        }
        match &self.own {
            Some(own) => own.get(def.key).map(|value| OwnRow {
                value: value.clone(),
                locked: false,
                revision: own.revision,
            }),
            None => {
                let node = match self.place {
                    Place::Node(id) => Some(id),
                    _ => None,
                };
                self.rows.get(node, def.key).map(|row| OwnRow {
                    value: row.value.clone(),
                    locked: row.locked,
                    revision: row.revision,
                })
            }
        }
    }

    /// `def` as it holds here: the value with its provenance, and the row at
    /// the place itself.
    #[must_use]
    pub fn view(&self, def: &'static SettingDef) -> SettingView {
        SettingView::new(
            def,
            self.here(),
            def.set_at(self.scope),
            self.resolve(def),
            self.own_row(def),
        )
    }

    /// Every key the snapshot was read for, as it holds here, in the order of
    /// the registry.
    #[must_use]
    pub fn views(&self) -> Vec<SettingView> {
        self.defs.iter().map(|def| self.view(def)).collect()
    }
}

/// The key `key` as it holds at `place`.
pub async fn resolve(conn: &mut Conn, place: Place, key: &str) -> Result<SettingView, Error> {
    let def = registry::get(key).ok_or_else(|| Error::UnknownKey(key.to_owned()))?;
    let snapshot = Snapshot::load(conn, place, Wanted::Keys(&[def])).await?;
    Ok(snapshot.view(def))
}

/// Every key that is shown for `place`, as it holds there, in the order of the
/// registry. The statements it takes do not depend on how many keys that is.
pub async fn resolve_all(conn: &mut Conn, place: Place) -> Result<Vec<SettingView>, Error> {
    Ok(Snapshot::load(conn, place, Wanted::All).await?.views())
}

/// Resolves hosts' cache policy in one read of system rows and one indexed read of host overrides.
/// The ordinary merge decides locks and zero, exactly as on an individual host settings page.
pub async fn cleanup_at_hosts(
    conn: &mut Conn,
    ids: &[Id],
) -> Result<std::collections::HashMap<Id, i64>, Error> {
    let key = keys::CLEANUP_UNUSED_KEEP_DAYS;
    let def = registry::get(key).ok_or_else(|| Error::UnknownKey(key.to_owned()))?;
    let system = Snapshot::load(conn, Place::System, Wanted::Keys(&[def])).await?;
    let hosts: Vec<(Id, Option<i64>, i64)> =
        sqlx::query_as("SELECT id,cleanup_keep_days,revision FROM hosts WHERE id=ANY($1)")
            .bind(ids)
            .fetch_all(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    let mut result = std::collections::HashMap::new();
    for (id, days, revision) in hosts {
        let own = OwnValues {
            scope: Scope::Host,
            values: days.map_or_else(
                || serde_json::json!({}),
                |days| serde_json::json!({(key):days}),
            ),
            revision,
        };
        let resolved = resolve_key(
            def,
            &Stack {
                rows: &system.rows,
                chain: &[],
                own: Some(&own),
            },
            None,
        );
        let days = resolved
            .value
            .and_then(|value| value.as_i64())
            .ok_or_else(|| Error::NotOfThatType {
                key: key.to_owned(),
                reason: "the cleanup policy is not an integer".into(),
            })?;
        result.insert(id, days);
    }
    Ok(result)
}

/// Reads one inherited key for several nodes using the same resolver and locks, with shared ancestor and row reads.
/// Keys whose default needs runtime facts retain their ordinary single-place loading path.
pub async fn get_at_nodes<T: DeserializeOwned>(
    conn: &mut Conn,
    ids: &[Id],
    key: &str,
) -> Result<std::collections::HashMap<Id, Option<T>>, Error> {
    let def = registry::get(key).ok_or_else(|| Error::UnknownKey(key.to_owned()))?;
    let mut result = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(result);
    }
    if !matches!(def.default, DefaultOf::Value(_) | DefaultOf::Unset) {
        for id in ids {
            result.insert(*id, get(conn, Place::Node(*id), key).await?);
        }
        return Ok(result);
    }
    let nodes = hierarchy::with_ancestors(conn, ids).await?;
    let ancestors: Vec<Id> = nodes.iter().map(|node| node.id).collect();
    let rows = Rows::new(queries::rows_on_chain(conn, &ancestors, Some(&[key.to_owned()])).await?);
    for id in ids {
        let chain = chain_of(&nodes, *id)?;
        let value = resolve_key(
            def,
            &Stack {
                rows: &rows,
                chain: &chain,
                own: None,
            },
            None,
        )
        .value
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| Error::NotOfThatType {
            key: key.to_owned(),
            reason: error.to_string(),
        })?;
        result.insert(*id, value);
    }
    Ok(result)
}

/// One provider's own runtime values and home, already read with the provider page.
#[derive(Debug, Clone, Copy)]
pub struct ProviderAt<'a> {
    /// The provider.
    pub id: Id,
    /// The node whose inherited settings it takes.
    pub home_id: Id,
    /// Values written on this provider alone.
    pub runtime_config: &'a Json,
    /// Revision of those own values.
    pub revision: i64,
}

/// Resolves a provider runtime key for a page of providers with shared ancestor and setting-row
/// reads. The keys used here have fixed, unset or installation-seeded defaults, so their supplier
/// does not depend on an individual provider's runtime facts.
pub async fn get_at_providers<T: DeserializeOwned>(
    conn: &mut Conn,
    providers: &[ProviderAt<'_>],
    key: &str,
) -> Result<std::collections::HashMap<Id, Option<T>>, Error> {
    let def = registry::get(key).ok_or_else(|| Error::UnknownKey(key.to_owned()))?;
    if !matches!(
        def.default,
        DefaultOf::Value(_) | DefaultOf::Unset | DefaultOf::Supplied(Supplier::Seeds)
    ) {
        return Err(Error::Unreadable {
            what: "a provider setting whose default needs its own runtime facts",
        });
    }
    let mut result = std::collections::HashMap::new();
    if providers.is_empty() {
        return Ok(result);
    }
    let homes: Vec<Id> = providers.iter().map(|provider| provider.home_id).collect();
    let nodes = hierarchy::with_ancestors(conn, &homes).await?;
    let ancestors: Vec<Id> = nodes.iter().map(|node| node.id).collect();
    let rows = Rows::new(queries::rows_on_chain(conn, &ancestors, Some(&[key.to_owned()])).await?);
    let supplies = supplied::load(
        conn,
        &supplied::Context {
            thread: None,
            environment: None,
            model_for_effort: None,
        },
        &[def],
    )
    .await?;
    for provider in providers {
        let snapshot = Snapshot {
            place: Place::Provider(provider.id),
            scope: Scope::Provider,
            chain: chain_of(&nodes, provider.home_id)?,
            rows: rows.clone(),
            own: Some(OwnValues {
                scope: Scope::Provider,
                values: provider.runtime_config.clone(),
                revision: provider.revision,
            }),
            supplies: supplies.clone(),
            defs: vec![def],
        };
        let value = snapshot
            .resolve(def)
            .value
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| Error::NotOfThatType {
                key: key.to_owned(),
                reason: error.to_string(),
            })?;
        result.insert(provider.id, value);
    }
    Ok(result)
}

/// The value of `key` at `place`, as the type `T` that the caller knows it to
/// be; none while the key is unset. A value that is not of that type is an
/// error: validation happens when a value is written, and one that stopped
/// being usable since is for the caller to report.
pub async fn get<T: DeserializeOwned>(
    conn: &mut Conn,
    place: Place,
    key: &str,
) -> Result<Option<T>, Error> {
    let view = resolve(conn, place, key).await?;
    match view.resolved.value {
        None => Ok(None),
        Some(value) => {
            serde_json::from_value(value)
                .map(Some)
                .map_err(|error| Error::NotOfThatType {
                    key: key.to_owned(),
                    reason: error.to_string(),
                })
        }
    }
}
