//! Authoritative target facts for the existing operation and projection producers.
//!
//! This module never grants access. It reuses the authorization graph and the stored
//! identity readers, then supplies typed current ancestry to the credential restriction.
//! It never infers authority from a URL, a resource-family filter, an owner hint, or a
//! submitted ancestor list. Call it on the producer's guarded connection so target facts
//! and the ordinary authorization describe the same operation.

use std::collections::{BTreeMap, btree_map::Entry};

use chrono::{DateTime, Utc};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyFacet, KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::input_schedules::InputScheduleTarget;
use orch_api_contract::operations::{OperationFacet, TargetResolver};
use orch_authz::engine::{Graph, Site};
use orch_db::Conn;
use orch_db::queries::{access, chats, hierarchy, identity, tokens};
use orch_engine::operation_scope::targets::{self as shared, TargetError};
use orch_types::Id;

use crate::middleware::authorization::{self, Subject, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;

fn reference(kind: KeyTargetKind, id: Id) -> KeyObjectRef {
    KeyObjectRef { kind, id: Some(id) }
}

fn unresolved() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "this key cannot be used for the requested target",
    )
}

async fn authz_result<T>(conn: &mut Conn, result: Result<T, orch_authz::Error>) -> ApiResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

async fn target_result<T>(conn: &mut Conn, result: Result<T, TargetError>) -> ApiResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(TargetError::Missing) => Err(ApiError::not_found()),
        Err(TargetError::Db(error)) => Err(error.into()),
        Err(TargetError::Authz(error)) => Err(refusals::problem(conn, error).await),
    }
}

/// Typed placement facts come from the same neutral Graph/Site producer used by workers.
pub(crate) fn from_site(graph: &Graph, site: &Site) -> ApiResult<KeyResolvedTarget> {
    orch_engine::operation_scope::targets::from_site(graph, site).ok_or_else(ApiError::not_found)
}

/// An existing node, whose actual kind and current ancestry come from the same Graph
/// used by ordinary node authorization. Retained inspection uses its existing task scope.
pub(crate) async fn node(conn: &mut Conn, id: Id) -> ApiResult<KeyResolvedTarget> {
    let result = shared::node(conn, id).await;
    target_result(conn, result).await
}

/// Resolve only the stored typed identity and current path, including a retained node.
/// This grants no visibility or restoration authority; the same ordinary inspection
/// producer still decides whether a selected retained operation may read it.
pub(crate) async fn node_identity(conn: &mut Conn, id: Id) -> ApiResult<KeyResolvedTarget> {
    let result = shared::node_identity(conn, id).await;
    target_result(conn, result).await
}

/// An exact permission-page object. A same-ID object of a different kind never matches.
/// Restoration producers that already loaded an AsItWas site use `from_site` directly;
/// this ordinary lookup keeps the same liveness rules as normal page reads.
pub(crate) async fn page_object(
    conn: &mut Conn,
    object: ObjectRef,
) -> ApiResult<KeyResolvedTarget> {
    let result = shared::page_object(conn, object).await;
    target_result(conn, result).await
}

/// Current retained resource identity selected by a native kept-reference producer.
/// This adds no ordinary View, inspection or lifecycle authority.
pub(crate) async fn resource_identity(
    conn: &mut Conn,
    object: ObjectRef,
) -> ApiResult<KeyResolvedTarget> {
    let result = shared::resource_identity(conn, object).await;
    target_result(conn, result).await
}

/// A physical machine's canonical live permission-page subject, as check_machine
/// uses it. This opt-in adapter never rewrites a Host identity to a Machine ID.
pub(crate) async fn machine_subject(conn: &mut Conn, machine: Id) -> ApiResult<KeyResolvedTarget> {
    let result = shared::machine_subject(conn, machine).await;
    target_result(conn, result).await
}

/// Bulk canonical physical-machine subjects, sharing the same neutral resolver as workers.
pub(crate) async fn machine_subjects(
    conn: &mut Conn,
    machines: &[Id],
) -> ApiResult<std::collections::HashMap<Id, KeyResolvedTarget>> {
    let result = shared::machine_subjects(conn, machines).await;
    target_result(conn, result).await
}

/// Current canonical identity for an already native-selected physical reference, including retained.
/// This supplies no live read, private inspection or lifecycle authority.
pub(crate) async fn machine_identities(
    conn: &mut Conn,
    machines: &[Id],
) -> ApiResult<std::collections::HashMap<Id, KeyResolvedTarget>> {
    let result = shared::machine_identities(conn, machines).await;
    target_result(conn, result).await
}

/// The very same subject producer used by `require_on_path_in`, followed by its
/// authoritative stored site. This is an adapter over the existing permission path;
/// it does not maintain another URL-to-authority mapping.
pub(crate) async fn permission_object(ctx: &Ctx, conn: &mut Conn) -> ApiResult<KeyResolvedTarget> {
    if !matches!(
        ctx.route.access,
        orch_api_contract::routes::Access::Permission(_)
    ) {
        return Err(unresolved());
    }
    match authorization::subject(ctx)? {
        Subject::Node(id) => node(conn, id).await,
        Subject::Address(address) => {
            let id = hierarchy::org_by_address(conn, &address)
                .await?
                .ok_or_else(ApiError::not_found)?;
            page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Org,
                    id,
                },
            )
            .await
        }
        Subject::Document(id) => {
            let loaded = orch_authz::documents::site_for_page(conn, id).await;
            let (graph, site) = authz_result(conn, loaded).await?;
            from_site(&graph, &site)
        }
        Subject::Resource(kind, id) => {
            page_object(
                conn,
                ObjectRef {
                    kind: kind.into(),
                    id,
                },
            )
            .await
        }
    }
}

/// Reuse an already read chat row. Node chats inherit their node's current hierarchy;
/// direct/group chats belong to their stored root org. Neither becomes the node itself.
pub(crate) async fn from_chat(
    conn: &mut Conn,
    chat: &chats::ChatRow,
) -> ApiResult<KeyResolvedTarget> {
    let result = shared::from_chat(conn, chat).await;
    target_result(conn, result).await
}

/// An exact chat identity, preserving its stored place instead of treating its URL ID as a node.
pub(crate) async fn chat(conn: &mut Conn, id: Id) -> ApiResult<KeyResolvedTarget> {
    let result = shared::chat(conn, id).await;
    target_result(conn, result).await
}

/// Stored chat identity and retained current placement for subscription eligibility only.
/// Ordinary chat readers retain their own live/deleted and permission decisions.
pub(crate) async fn chat_identity(conn: &mut Conn, id: Id) -> ApiResult<KeyResolvedTarget> {
    let result = shared::chat_identity(conn, id).await;
    target_result(conn, result).await
}

/// Reuse the ordinary group reader and its actual keeping org.
pub(crate) async fn from_group(
    conn: &mut Conn,
    group: &access::GroupFull,
) -> ApiResult<KeyResolvedTarget> {
    let mut target = page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Org,
            id: group.org_id,
        },
    )
    .await?;
    target.object = reference(KeyTargetKind::Identity(KeyIdentityKind::Group), group.id);
    Ok(target)
}

/// An exact group identity. Group membership does not create hierarchy edges.
pub(crate) async fn group(conn: &mut Conn, id: Id) -> ApiResult<KeyResolvedTarget> {
    let row = access::groups_full(conn, &[id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)?;
    from_group(conn, &row).await
}

/// An existing account. A service account has a stored owning org; a person's membership,
/// personal org, roles and shares are not placement and never become its ancestors.
#[expect(dead_code, reason = "retained adapter for native account producers")]
pub(crate) async fn from_account(
    conn: &mut Conn,
    account: &identity::UserRow,
) -> ApiResult<KeyResolvedTarget> {
    let result = shared::from_account(conn, account).await;
    target_result(conn, result).await
}

/// A stored account identity, without granting access to its private fields.
pub(crate) async fn account(conn: &mut Conn, id: Id) -> ApiResult<KeyResolvedTarget> {
    let result = shared::account(conn, id).await;
    target_result(conn, result).await
}

/// Reuse actual native account identity rows with one batch of owning-org ancestry.
/// Human memberships never become account placement.
pub(crate) async fn account_identities(
    conn: &mut Conn,
    accounts: &[(Id, &str, Option<Id>)],
) -> ApiResult<std::collections::HashMap<Id, KeyResolvedTarget>> {
    let homes: Vec<_> = accounts.iter().filter_map(|(_, _, org)| *org).collect();
    let graph = orch_authz::engine::Graph::load(conn, &homes)
        .await
        .map_err(crate::middleware::authorization::restoration_error)?;
    accounts
        .iter()
        .map(|(id, kind, org)| {
            shared::from_account_identity(&graph, *id, kind, *org)
                .map(|target| (*id, target))
                .ok_or_else(ApiError::not_found)
        })
        .collect()
}

/// The system target is the only identity without an object ID.
pub(crate) fn system() -> KeyResolvedTarget {
    shared::system()
}

/// A key read by the existing owner-filtered producer. Its legacy `node_id` is a
/// credential restriction, not the key's placement, and is deliberately never ancestry.
pub(crate) async fn key_of_account(
    conn: &mut Conn,
    id: Id,
    owner: Id,
) -> ApiResult<KeyResolvedTarget> {
    let row = identity::api_token_listed(conn, id, owner)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let account = account(conn, owner).await?;
    from_key_subject(&account, row.id, row.user_id)
}

/// A key subject read from the existing owner-filtered credential row. The
/// legacy node restriction is deliberately not placement or an account witness.
pub(crate) fn from_key_subject(
    account: &KeyResolvedTarget,
    key: Id,
    user: Id,
) -> ApiResult<KeyResolvedTarget> {
    let owner = reference(KeyTargetKind::Identity(KeyIdentityKind::Account), user);
    if account.object != owner {
        return Err(ApiError::not_found());
    }
    let mut target = account.clone();
    target.object = reference(KeyTargetKind::Identity(KeyIdentityKind::Key), key);
    target.credential_account = Some(owner);
    Ok(target)
}

/// A retained key's actual owning account. Revocation stops authentication, not
/// the owner's ability to inspect its recent history through an admitted reader.
pub(crate) async fn retained_key_of_account(
    conn: &mut Conn,
    id: Id,
    owner: Id,
) -> ApiResult<KeyResolvedTarget> {
    if !orch_db::queries::key_activity::owned(conn, id, owner).await? {
        return Err(ApiError::not_found());
    }
    let mut target = account(conn, owner).await?;
    target.object = reference(KeyTargetKind::Identity(KeyIdentityKind::Key), id);
    target.credential_account = Some(reference(
        KeyTargetKind::Identity(KeyIdentityKind::Account),
        owner,
    ));
    Ok(target)
}

/// A currently stored live session of the actual owning account. Current ordinary
/// session-management authorization remains the caller's existing producer's job.
#[expect(dead_code, reason = "retained adapter for live session producers")]
pub(crate) async fn session_of_account(
    conn: &mut Conn,
    id: Id,
    owner: Id,
    now: DateTime<Utc>,
) -> ApiResult<KeyResolvedTarget> {
    if tokens::live_session_user(conn, id, now).await? != Some(owner) {
        return Err(ApiError::not_found());
    }
    let mut target = account(conn, owner).await?;
    target.credential_account = Some(target.object);
    target.object = reference(KeyTargetKind::Identity(KeyIdentityKind::Session), id);
    Ok(target)
}

/// An exact stored session subject from the normal revoke materializer. Only
/// this real account relation is retained; arbitrary owned resources do not inherit it.
pub(crate) fn from_session_subject(
    account: &KeyResolvedTarget,
    session: Id,
    user: Id,
) -> ApiResult<KeyResolvedTarget> {
    let owner = reference(KeyTargetKind::Identity(KeyIdentityKind::Account), user);
    if account.object != owner {
        return Err(ApiError::not_found());
    }
    let mut target = account.clone();
    target.object = reference(KeyTargetKind::Identity(KeyIdentityKind::Session), session);
    target.credential_account = Some(owner);
    Ok(target)
}

/// A real active upload session, after its ordinary producer checked ownership/state.
/// Completed-upload hashes are deliberately unsupported here: their selected readable
/// source must supply Chat/Thread/Document/Environment facts through the normal producer.
#[expect(dead_code, reason = "retained adapter for native upload producers")]
pub(crate) async fn from_upload_session(
    conn: &mut Conn,
    row: &orch_db::queries::storage::SessionRow,
) -> ApiResult<KeyResolvedTarget> {
    let mut target = account(conn, row.user_id).await?;
    target.object = reference(KeyTargetKind::Identity(KeyIdentityKind::Upload), row.id);
    Ok(target)
}

/// The actual destination of a decoded stored schedule, as the existing scheduling
/// producer defines it. A new thread is checked at its real owning topic, not a future ID.
pub(crate) async fn schedule_destination(
    conn: &mut Conn,
    topic: Id,
    target: &InputScheduleTarget,
) -> ApiResult<KeyResolvedTarget> {
    let object = match target {
        InputScheduleTarget::Thread { thread_id } => ObjectRef {
            kind: ObjectKind::Thread,
            id: *thread_id,
        },
        InputScheduleTarget::NewThread { .. } => ObjectRef {
            kind: ObjectKind::Topic,
            id: topic,
        },
    };
    page_object(conn, object).await
}

/// Load one exact typed object using its ordinary source. Opaque jobs, artifacts,
/// approvals and schedules need their operation's selected source/destination producer;
/// no broad union of IDs from an opaque row is accepted as a substitute.
pub(crate) async fn object(conn: &mut Conn, object: KeyObjectRef) -> ApiResult<KeyResolvedTarget> {
    match (object.kind, object.id) {
        (KeyTargetKind::Object(kind), Some(id)) => page_object(conn, ObjectRef { kind, id }).await,
        (KeyTargetKind::Identity(KeyIdentityKind::Chat), Some(id)) => chat(conn, id).await,
        (KeyTargetKind::Identity(KeyIdentityKind::Group), Some(id)) => group(conn, id).await,
        (KeyTargetKind::Identity(KeyIdentityKind::Account), Some(id)) => account(conn, id).await,
        (KeyTargetKind::Identity(KeyIdentityKind::System), None) => Ok(system()),
        _ => Err(unresolved()),
    }
}

/// Values obtained by the shared operation's typed producers on its current guarded
/// connection. This is intentionally not deserializable from requests or persisted JSON.
#[derive(Debug, Default)]
pub(crate) struct TargetBindings {
    permission_object: Option<KeyResolvedTarget>,
    paths: BTreeMap<&'static str, Vec<KeyResolvedTarget>>,
    resolved: BTreeMap<&'static str, Vec<KeyResolvedTarget>>,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "binding setters are exercised by unit controls")
)]
fn bind(
    map: &mut BTreeMap<&'static str, Vec<KeyResolvedTarget>>,
    name: &'static str,
    targets: Vec<KeyResolvedTarget>,
) -> ApiResult<()> {
    match map.entry(name) {
        Entry::Vacant(entry) => {
            entry.insert(targets);
            Ok(())
        }
        Entry::Occupied(_) => Err(unresolved()),
    }
}

impl TargetBindings {
    /// Bind the exact site obtained by the existing fixed-permission subject producer.
    #[expect(
        dead_code,
        reason = "retained binding for prepared permission subjects"
    )]
    pub(crate) fn permission_object(&mut self, target: KeyResolvedTarget) -> ApiResult<()> {
        if self.permission_object.is_some() {
            return Err(unresolved());
        }
        self.permission_object = Some(target);
        Ok(())
    }

    /// A path parameter's authoritative typed targets, including governing objects of an
    /// opaque ID selected by its existing producer. Rebinding cannot silently drop targets.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "binding setters are exercised by unit controls")
    )]
    pub(crate) fn path(
        &mut self,
        parameter: &'static str,
        targets: Vec<KeyResolvedTarget>,
    ) -> ApiResult<()> {
        bind(&mut self.paths, parameter, targets)
    }

    /// Targets selected by a named shared producer (sources, destination, results, etc.).
    /// Empty means that the producer actually resolved no targets, not that it was skipped.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "binding setters are exercised by unit controls")
    )]
    pub(crate) fn resolved(
        &mut self,
        role: &'static str,
        targets: Vec<KeyResolvedTarget>,
    ) -> ApiResult<()> {
        bind(&mut self.resolved, role, targets)
    }

    /// Bind a direct path ID of a known kind, validating existence and exact kind through
    /// the stored source. The kind comes from the typed producer, never facet.resources.
    #[expect(dead_code, reason = "retained binding for typed native subjects")]
    pub(crate) async fn typed_path(
        &mut self,
        ctx: &Ctx,
        conn: &mut Conn,
        parameter: &'static str,
        kind: KeyTargetKind,
    ) -> ApiResult<()> {
        let target = object(conn, reference(kind, ctx.id(parameter)?)).await?;
        self.path(parameter, vec![target])
    }
}

/// Resolve catalogue target metadata using authoritative bindings. A missing binding
/// fails closed; resource families and URL prefixes never manufacture a target.
pub(crate) async fn resolve(
    ctx: &Ctx,
    conn: &mut Conn,
    facet: &OperationFacet,
    bindings: &TargetBindings,
) -> ApiResult<Vec<KeyResolvedTarget>> {
    match facet.target {
        TargetResolver::CurrentAccount => Ok(vec![account(conn, ctx.user()?.id()).await?]),
        TargetResolver::System => Ok(vec![system()]),
        TargetResolver::PermissionObject => match &bindings.permission_object {
            Some(target) => Ok(vec![target.clone()]),
            None => Ok(vec![permission_object(ctx, conn).await?]),
        },
        TargetResolver::Path(parameter) => bindings
            .paths
            .get(parameter)
            .cloned()
            .ok_or_else(unresolved),
        TargetResolver::TypedPath { parameter, kind } => Ok(vec![
            object(conn, reference(kind, ctx.id(parameter)?)).await?,
        ]),
        TargetResolver::Resolved(role) | TargetResolver::ResolvedCollection(role) => {
            bindings.resolved.get(role).cloned().ok_or_else(unresolved)
        }
    }
}

/// Attach an operation's actual business resource family to its target facts. Creation
/// commonly writes a Document or Thread at a parent of another kind. The caller resolves
/// that family from the actual operation; this function only validates the declaration.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "facet adapter is exercised by unit controls")
)]
pub(crate) fn facets(
    selection: &str,
    metadata: &OperationFacet,
    resource_kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<Vec<KeyFacet>> {
    orch_engine::operation_scope::facets(selection, metadata, resource_kind, targets).map_err(
        |error| match error {
            orch_engine::operation_scope::BindingError::Refused => unresolved(),
            other => super::admission::binding_error(other),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_access_contract::key_scope::{KeyAction, KeyReach, KeyTarget};
    use orch_access_contract::permissions::Placement;
    use orch_access_contract::roles::NodeKind;
    use orch_api_contract::operations::FacetUsage;
    use orch_authz::engine::NodeFacts;

    fn graph() -> Graph {
        let org = Id::from_u128(1);
        let project = Id::from_u128(2);
        Graph::of(vec![
            NodeFacts {
                id: org,
                kind: NodeKind::Org,
                parent: None,
                root: org,
                path: vec![org],
                name: String::new(),
                first_owner: Id::from_u128(9),
                archived: false,
                deleted: false,
                placement: Placement::default(),
            },
            NodeFacts {
                id: project,
                kind: NodeKind::Project,
                parent: Some(org),
                root: org,
                path: vec![org, project],
                name: String::new(),
                first_owner: Id::from_u128(9),
                archived: false,
                deleted: false,
                placement: Placement::default(),
            },
        ])
    }

    #[test]
    fn typed_current_ancestry_comes_from_the_graph_and_same_id_does_not_change_kind() {
        let graph = graph();
        let target = from_site(&graph, &graph.site_of(Id::from_u128(2)).unwrap()).unwrap();
        assert_eq!(
            target.object.kind,
            KeyTargetKind::Object(ObjectKind::Project)
        );
        assert_eq!(
            target.node_ancestors,
            vec![
                reference(KeyTargetKind::Object(ObjectKind::Org), Id::from_u128(1)),
                reference(KeyTargetKind::Object(ObjectKind::Project), Id::from_u128(2))
            ]
        );
        assert!(
            KeyTarget {
                kind: KeyTargetKind::Object(ObjectKind::Org),
                id: Some(Id::from_u128(1)),
                reach: KeyReach::Subtree
            }
            .matches(&target)
        );
        assert!(
            !KeyTarget {
                kind: KeyTargetKind::Object(ObjectKind::DocumentContainer),
                id: Some(Id::from_u128(1)),
                reach: KeyReach::Subtree
            }
            .matches(&target)
        );
        assert!(target.document_ancestors.is_empty());
    }

    #[test]
    fn resource_home_is_placement_even_when_its_site_has_no_node_path() {
        let graph = graph();
        let mut site = graph.site_of(Id::from_u128(2)).unwrap();
        site.kind = ObjectKind::Provider;
        site.id = Id::from_u128(3);
        site.role_place = Some(Id::from_u128(2));
        site.path = None;
        let target = from_site(&graph, &site).unwrap();
        assert_eq!(
            target.object.kind,
            KeyTargetKind::Object(ObjectKind::Provider)
        );
        assert_eq!(target.node_ancestors.len(), 2);
        assert_eq!(target.node_ancestors[1].id, Some(Id::from_u128(2)));
    }

    #[test]
    fn document_uses_its_actual_beside_node_without_inventing_folder_ancestry() {
        let graph = graph();
        let mut site = graph.site_of(Id::from_u128(2)).unwrap();
        site.kind = ObjectKind::Document;
        site.id = Id::from_u128(4);
        site.beside = Some(Id::from_u128(2));
        // A role place is not the document's placement.
        site.role_place = Some(Id::from_u128(1));
        let target = from_site(&graph, &site).unwrap();
        assert_eq!(target.object.id, Some(Id::from_u128(4)));
        assert_eq!(target.node_ancestors.len(), 2);
        assert_eq!(target.node_ancestors[1].id, Some(Id::from_u128(2)));
        assert!(target.document_ancestors.is_empty());
    }

    #[test]
    fn creation_family_does_not_relabel_its_real_parent() {
        let graph = graph();
        let parent = from_site(&graph, &graph.site_of(Id::from_u128(2)).unwrap()).unwrap();
        let metadata = OperationFacet {
            action: KeyAction::Write,
            resources: vec![KeyTargetKind::Object(ObjectKind::Document)],
            target: TargetResolver::Resolved("destination"),
            usage: FacetUsage::Required,
        };
        let resolved = facets(
            "NodeDocumentCreate",
            &metadata,
            KeyTargetKind::Object(ObjectKind::Document),
            &[parent],
        )
        .unwrap();
        assert_eq!(
            resolved[0].resource_kind,
            KeyTargetKind::Object(ObjectKind::Document)
        );
        assert_eq!(
            resolved[0].target.object.kind,
            KeyTargetKind::Object(ObjectKind::Project)
        );
        assert!(
            facets(
                "NodeDocumentCreate",
                &metadata,
                KeyTargetKind::Object(ObjectKind::Provider),
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn bindings_cannot_silently_replace_resolved_targets() {
        let mut bindings = TargetBindings::default();
        bindings.resolved("sources", vec![system()]).unwrap();
        assert!(bindings.resolved("sources", Vec::new()).is_err());
        assert_eq!(bindings.resolved["sources"].len(), 1);
        bindings.path("id", vec![system()]).unwrap();
        assert!(bindings.path("id", Vec::new()).is_err());
    }
}
