//! What a request that is the same for every kind of thing names by `{object}` and `{id}` of its path, and the checks that are the same for all of them.
//!
//! Nodes, documents and every resource kind have permission pages decided by
//! the same access engine, each by the machinery of its own side. A chat has no page of its
//! own (its people are read as the people of a chat, and what was shared
//! with agents is listed where agents are shared with), so a request for the
//! page of a chat, or of anything the path invents, is answered plainly as not
//! there and never as a fault of the server.

use crate::middleware::authorization::{self, VIEW, exact, resource_kind_of};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::nodes::policy_at;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::common::ErrorCode;
use orch_authz::engine::Site;
use orch_authz::things::{self, Thing};
use orch_authz::usable::ResourcePolicy;
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::Id;

/// The thing a path names, with the word the path calls its kind by.
#[derive(Debug, Clone, Copy)]
pub struct Named {
    /// The thing.
    pub thing: Thing,
}

impl Named {
    /// Its identifier.
    #[must_use]
    pub fn id(&self) -> Id {
        self.thing.id()
    }
}

/// The refusal to answer for a kind of thing that has no permission page to read: the thing is
/// there, or not, and a plain 404 says that nothing is kept here for it.
fn without_a_page(word: &str) -> ApiError {
    let what = if word == "chats" {
        "a chat has no permission page: the people in it read it, and what was shared with agents is listed where agents are shared with".to_owned()
    } else {
        format!("there is nothing called {word} that has a permission page")
    };
    ApiError::new(ErrorCode::NotFound, what)
}

/// What `{object}/{id}` of the request's path names.
pub fn named(ctx: &Ctx) -> ApiResult<Named> {
    let id = ctx.id("id")?;
    let word = ctx.param("object")?;
    let thing = match word {
        "nodes" => Thing::Node(id),
        "documents" => Thing::Other(ObjectRef {
            kind: ObjectKind::Document,
            id,
        }),
        "document-containers" => Thing::Other(ObjectRef {
            kind: ObjectKind::DocumentContainer,
            id,
        }),
        other => match resource_kind_of(other) {
            Some(kind) => Thing::Other(ObjectRef {
                kind: kind.into(),
                id,
            }),
            None => return Err(without_a_page(other)),
        },
    };
    Ok(Named { thing })
}

/// The kind of thing a word of a request's `kind` member names: a node of a kind, a resource or a
/// document, in the words the engine uses (`thread`, `provider`, `mcp_server`) or in the plural
/// the paths use (`providers`, `mcp-servers`).
#[must_use]
pub fn kind_of_word(word: &str) -> Option<ObjectKind> {
    match resource_kind_of(word) {
        Some(kind) => Some(kind.into()),
        None => match word {
            "documents" => Some(ObjectKind::Document),
            "document-containers" => Some(ObjectKind::DocumentContainer),
            other => other.parse().ok(),
        },
    }
}

/// The thing an object reference names.
#[must_use]
pub fn of(object: ObjectRef) -> Thing {
    orch_authz::things::thing_of(object)
}

/// The policy that decides whether the owners of a thing may name somebody from outside the root
/// org: the setting that resolves at the place the thing is in, which for a node is the node, for
/// a resource its home and for a document the node it lies beside.
pub async fn policy_of(conn: &mut Conn, thing: Thing, site: &Site) -> ApiResult<Policy> {
    let place = match thing {
        Thing::Node(id) => Some(id),
        Thing::Other(object) if object.kind.is_document() => site.beside,
        Thing::Other(_) => site.role_place,
    };
    policy_at_place(conn, place).await
}

/// The sharing policy resolved at a node, or at the system for a subject without a home.
pub async fn policy_at_place(conn: &mut Conn, place: Option<Id>) -> ApiResult<Policy> {
    if let Some(place) = place {
        return policy_at(conn, place).await;
    }
    let external = setting::<bool>(conn, Place::System, keys::SHARING_ALLOW_EXTERNAL)
        .await
        .map_err(ApiError::internal)?;
    let holders = setting::<bool>(conn, Place::System, keys::SHARING_ALLOW_HOLDERS)
        .await
        .map_err(ApiError::internal)?;
    Ok(Policy {
        owners_name_outsiders: external.unwrap_or(true),
        holders_share: holders.unwrap_or(true),
    })
}

/// The settings that govern a change of the page of `thing` at its home: whether its owners may name
/// somebody from outside the root org, and for a provider which subscriptions its home shares.
pub async fn resource_policy(
    conn: &mut Conn,
    thing: Thing,
    site: &Site,
) -> ApiResult<ResourcePolicy> {
    let policy = policy_of(conn, thing, site).await?;
    let sharing = match thing {
        Thing::Other(object) if object.kind == ObjectKind::Provider => match site.role_place {
            Some(home) => {
                crate::routes::resources::providers::home_policies(conn, home)
                    .await?
                    .subscription_sharing
            }
            None => None,
        },
        _ => None,
    };
    Ok(ResourcePolicy {
        owners_name_outsiders: policy.owners_name_outsiders,
        shared_subscriptions: sharing,
    })
}

/// The content View row of a kind of thing that is not a node.
const fn viewing(kind: ObjectKind) -> Option<Permission> {
    Some(match kind {
        ObjectKind::Provider => Permission::ProviderView,
        ObjectKind::Agent => Permission::AgentView,
        ObjectKind::Environment => Permission::EnvironmentView,
        ObjectKind::Host => Permission::HostView,
        ObjectKind::Machine => Permission::MachineView,
        ObjectKind::Template => Permission::TemplateView,
        ObjectKind::McpServer => Permission::McpView,
        ObjectKind::Mount => Permission::MountView,
        ObjectKind::Skill => Permission::SkillView,
        ObjectKind::Document | ObjectKind::DocumentContainer => Permission::DocView,
        _ => return None,
    })
}

/// Requires on `thing` the permission `needed` stands for, by the kind it is, and refuses with what
/// is missing and who can give it when the caller does not hold it.
async fn require(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    needed: Needed,
    policy: Policy,
) -> ApiResult<()> {
    match thing {
        Thing::Node(id) => {
            let name = match needed {
                Needed::View => VIEW,
                Needed::Manage => exact(Permission::AccessManage),
            };
            authorization::require(conn, principal, id, name, Act::Use, policy).await
        }
        Thing::Other(object) => {
            let permission = match needed {
                Needed::View => viewing(object.kind),
                Needed::Manage => Some(Permission::AccessManage),
            }
            .ok_or_else(ApiError::not_found)?;
            if object.kind.is_document() {
                authorization::require_document(conn, principal, object.id, permission, policy)
                    .await
            } else {
                let kind = object.kind.resource().ok_or_else(ApiError::not_found)?;
                if matches!(needed, Needed::View) {
                    return require_resource_reader(conn, principal, object, policy).await;
                }
                authorization::require_resource(
                    conn, principal, kind, object.id, permission, policy,
                )
                .await
            }
        }
    }
}

/// Requires any current scoped permission on a resource for its generic metadata readers.
/// Content readers keep their own View or door permission requirements.
pub async fn require_resource_reader(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    policy: Policy,
) -> ApiResult<()> {
    let kind = object.kind.resource().ok_or_else(ApiError::not_found)?;
    let held = things::effective(conn, principal, Thing::Other(object), policy)
        .await
        .map_err(|error| match error {
            orch_authz::Error::NotFound(_) => ApiError::not_found(),
            other => ApiError::internal(other),
        })?;
    if !held.held.is_empty() {
        return Ok(());
    }
    let view = viewing(object.kind).ok_or_else(ApiError::not_found)?;
    authorization::require_resource(conn, principal, kind, object.id, view, policy).await
}

/// What a request needs of the thing it is about.
#[derive(Debug, Clone, Copy)]
enum Needed {
    /// To see it.
    View,
    /// To change its page.
    Manage,
}

/// Requires that the caller may view the thing: any permission on a resource, `node.view` on a
/// node, `doc.view` on a document.
pub async fn require_view(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    policy: Policy,
) -> ApiResult<()> {
    require(conn, principal, thing, Needed::View, policy).await
}

/// Requires permission to inspect a page. Scoped page management permits reading its authority
/// without granting any permission to read the content of the thing.
pub async fn require_page(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    policy: Policy,
) -> ApiResult<()> {
    if let Thing::Other(object) = thing
        && object.kind.resource().is_some()
    {
        return require_resource_reader(conn, principal, object, policy).await;
    }
    let held = things::effective(conn, principal, thing, policy)
        .await
        .map_err(|error| match error {
            orch_authz::Error::NotFound(_) => ApiError::not_found(),
            other => ApiError::internal(other),
        })?;
    if held
        .held
        .iter()
        .any(|held| held.permission == Permission::AccessManage)
    {
        return Ok(());
    }
    require_view(conn, principal, thing, policy).await
}

/// Requires that the caller may change the page of the thing.
pub async fn require_manage(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    policy: Policy,
) -> ApiResult<()> {
    require(conn, principal, thing, Needed::Manage, policy).await
}
