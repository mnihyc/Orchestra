//! The names that figures and records are shown by: people, providers, models by what they are shown as, agents by kind, and places, each place a redacted reference where the reader may not see it.
//!
//! A figure is about a member of a dimension, and a record is about places,
//! people and resources; the ledger keeps their identifiers as they were. They
//! are named here, for a whole answer at once. A place the reader may not see is
//! a reference without a name (`redacted`), and its figures stay in the totals:
//! nothing says where below it the use was.

use crate::middleware::requests::ApiResult;
use crate::routes::nodes::view as node_view;
use crate::routes::resources::view as resource_view;
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::resources::ProviderRef;
use orch_api_contract::usage::{UsageDimension, UsageEnvironmentKind, UsageKey, UsageKeyAgent};
use orch_api_contract::users::UserRef;
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::usage::rollup;
use orch_types::Id;
use serde_json::json;
use std::collections::HashMap;

/// Everything a set of keys or records is named by.
#[derive(Debug, Default)]
pub struct Names {
    users: HashMap<Id, UserRef>,
    providers: HashMap<Id, ProviderRef>,
    nodes: HashMap<Id, NodeRef>,
    displays: HashMap<String, String>,
    agents: HashMap<String, String>,
}

/// A word of one contract as the same word of another.
fn same<A: serde::Serialize, B: serde::de::DeserializeOwned>(word: &A) -> Option<B> {
    serde_json::to_value(word)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn ids_of<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<Id> {
    let mut ids: Vec<Id> = values
        .into_iter()
        .filter_map(|text| text.parse().ok())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

impl Names {
    /// The people, providers and places among `users`, `providers` and `nodes`, named for the reader.
    pub async fn load(
        conn: &mut Conn,
        principal: &Principal,
        users: &[Id],
        providers: &[Id],
        nodes: &[Id],
    ) -> ApiResult<Self> {
        let mut names = Self {
            users: crate::routes::access::page::user_refs_at(conn, principal, None, users).await?,
            ..Self::default()
        };
        if !providers.is_empty() {
            names.providers = resource_view::provider_refs(conn, providers, &[])
                .await?
                .into_iter()
                .map(|(_, each)| (each.id, each))
                .collect();
        }
        if !nodes.is_empty() {
            // The screen already supplies scope, and records have separate place columns.
            // Keep the familiar name first rather than repeating the entire path in every cell.
            names.nodes = node_view::refs(conn, principal, nodes, Policy::default()).await?;
        }
        Ok(names)
    }

    /// The names the members of a dimension need, for the members `values`.
    pub async fn for_dimension(
        conn: &mut Conn,
        principal: &Principal,
        dimension: UsageDimension,
        values: &[Option<String>],
    ) -> ApiResult<Self> {
        let texts = values.iter().flatten().map(String::as_str);
        let mut names = match dimension {
            UsageDimension::Person => Self::load(conn, principal, &ids_of(texts), &[], &[]).await?,
            UsageDimension::Provider => {
                Self::load(conn, principal, &[], &ids_of(texts), &[]).await?
            }
            UsageDimension::Org
            | UsageDimension::Project
            | UsageDimension::Topic
            | UsageDimension::Thread => {
                Self::load(conn, principal, &[], &[], &ids_of(texts)).await?
            }
            _ => Self::default(),
        };
        match dimension {
            UsageDimension::Model => {
                let models: Vec<String> = values.iter().flatten().cloned().collect();
                names.displays = rollup::display_names(conn, &models).await?;
            }
            UsageDimension::Agent => {
                let kinds: Vec<String> = values.iter().flatten().cloned().collect();
                names.agents = rollup::agent_names(conn, &kinds).await?;
            }
            _ => {}
        }
        Ok(names)
    }

    /// A person as the reader sees them, even one who is no longer there.
    #[must_use]
    pub fn user(&self, id: Id) -> UserRef {
        self.users.get(&id).cloned().unwrap_or_else(|| {
            serde_json::from_value(json!({
                "id": id, "kind": "human", "username": "", "display_name": "",
                "avatar": { "text": "", "color": 0 }
            }))
            .unwrap_or_else(|_| unreachable_user(id))
        })
    }

    /// A provider as the reader sees it; one that is gone is told so.
    #[must_use]
    pub fn provider(&self, id: Id) -> Option<ProviderRef> {
        self.providers.get(&id).cloned().or_else(|| {
            serde_json::from_value(json!({
                "id": id, "name": "(removed)", "kind": "api", "harness_kind": null, "harness": null, "address": "",
                "placement": "managed", "status": "disabled"
            }))
            .ok()
        })
    }

    /// A place, by name where the reader may know it, else a reference without one.
    #[must_use]
    pub fn node(&self, id: Id, kind: NodeKind) -> NodeRef {
        self.nodes.get(&id).cloned().unwrap_or(NodeRef {
            thread_display: None,
            id,
            kind,
            name: None,
            redacted: true,
        })
    }

    /// What a model is shown as: its display name where an entry gives one, else its routing name.
    #[must_use]
    pub fn display(&self, model: &str) -> String {
        self.displays.get(model).cloned().unwrap_or_default()
    }

    /// The key a member of a dimension is shown as.
    #[must_use]
    pub fn key(&self, dimension: UsageDimension, value: Option<&str>) -> UsageKey {
        let id = value.and_then(|text| text.parse::<Id>().ok());
        let mut key = UsageKey::default();
        match dimension {
            UsageDimension::Person => key.person = Some(id.map(|id| self.user(id))),
            UsageDimension::Model => {
                key.model = value.map(str::to_owned);
                key.model_display = value.map(|model| self.display(model));
            }
            UsageDimension::Provider => key.provider = id.and_then(|id| self.provider(id)),
            UsageDimension::Agent => {
                key.agent = Some(value.map(|kind| {
                    UsageKeyAgent {
                        kind: kind.to_owned(),
                        name: self
                            .agents
                            .get(kind)
                            .cloned()
                            .unwrap_or_else(|| kind.to_owned()),
                    }
                }));
            }
            UsageDimension::Environment => {
                key.environment = Some(value.and_then(|kind| match kind {
                    "managed" => Some(UsageEnvironmentKind::Managed),
                    "connected" => Some(UsageEnvironmentKind::Connected),
                    _ => None,
                }));
            }
            UsageDimension::Effort => key.effort = Some(value.map(str::to_owned)),
            UsageDimension::Tier => key.tier = Some(value.map(str::to_owned)),
            UsageDimension::Program => key.program = Some(value.map(str::to_owned)),
            UsageDimension::Org => key.org = Some(id.map(|id| self.node(id, NodeKind::Org))),
            UsageDimension::Project => {
                key.project = id.map(|id| self.node(id, NodeKind::Project));
            }
            UsageDimension::Topic => key.topic = id.map(|id| self.node(id, NodeKind::Topic)),
            UsageDimension::Thread => key.thread = id.map(|id| self.node(id, NodeKind::Thread)),
        }
        key
    }
}

/// A person who cannot be read from the JSON above, which cannot happen: the shape is fixed.
fn unreachable_user(id: Id) -> UserRef {
    UserRef {
        id,
        kind: orch_api_contract::users::UserKind::Human,
        username: String::new(),
        address: None,
        display_name: String::new(),
        avatar: orch_api_contract::users::Avatar {
            text: String::new(),
            color: 0,
        },
    }
}

/// A rate of one contract as a rate of the API's.
#[must_use]
pub fn rate_of(value: &orch_types::Json) -> Option<orch_api_contract::resources::Rate> {
    same(value)
}
