//! Profile identities, ordered native pins and their protected projection sources.

use super::{Opened, may_message, open_person, shown_parts};
use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan};
use crate::routes::documents::build::{ProjectedDocumentRef, WrittenLabelSource};
use crate::routes::nodes::{node_id, projection, read_sources, view};
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{NodePermission, ObjectKind, Permission, PermissionName};
use orch_api_contract::account_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_api_contract::users::{ProfilePin, UserProfile};
use orch_authz::engine::Graph;
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::Conn;
use orch_db::queries::{documents, identity, profiles};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);
type ObjectKey = (ObjectKind, Id);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PinKey {
    Node(Id),
    Document(Id),
}

fn object_kind(kind: KeyTargetKind) -> ApiResult<ObjectKind> {
    match kind {
        KeyTargetKind::Object(kind) => Ok(kind),
        _ => Err(ApiError::internal(
            "a native profile pin has no object kind",
        )),
    }
}

pub(super) struct Profile;
pub(super) struct Selection {
    opened: Opened,
    pins: Vec<ProfilePin>,
    places: BTreeSet<Id>,
    associations: BTreeMap<ObjectKey, BTreeSet<ObjectKey>>,
    node_authority: Vec<nodes::Effective>,
    documents: Vec<orch_authz::documents::Listed>,
    can_message: bool,
}

fn groups(
    values: impl IntoIterator<Item = (KeyTargetKind, KeyResolvedTarget)>,
) -> Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)> {
    projection::groups(values)
}

async fn object_facts(
    conn: &mut Conn,
    source: Source,
    objects: &BTreeSet<ObjectKey>,
) -> ApiResult<native::Facts<Source>> {
    let resolved = read_sources::Sources::from_objects(
        objects.iter().map(|&(kind, id)| ObjectRef { kind, id }),
    )
    .identity_targets(conn)
    .await?;
    Ok(native::Facts::collection(
        source,
        groups(
            resolved
                .into_iter()
                .map(|(object, target)| (KeyTargetKind::Object(object.kind), target)),
        ),
    ))
}

fn permitted(
    plan: &OperationPlan,
    caller: &Caller,
    source: Source,
    family: KeyTargetKind,
    target: &KeyResolvedTarget,
) -> ApiResult<bool> {
    Ok(!native::modern(caller)
        || native::permits_usage(
            plan,
            &sources::PROFILE,
            caller,
            &source,
            FacetUsage::ResponseContent,
            family,
            target,
        )?)
}

fn context_slot(plan: &OperationPlan) -> ApiResult<usize> {
    plan.variant()
        .facets
        .iter()
        .position(|facet| {
            facet.target == TargetResolver::ResolvedCollection(sources::PROFILE_CONTEXT_ROLE)
                && facet.usage == FacetUsage::ResponseContent
        })
        .ok_or_else(|| ApiError::internal("the profile context declaration is absent"))
}

pub(crate) fn node_authority_preserved(old: &nodes::Effective, fresh: &nodes::Effective) -> bool {
    old.node == fresh.node
        && old.kind == fresh.kind
        && old.role == fresh.role
        && (!old.owner || fresh.owner)
        && (!old.administers || fresh.administers)
        && old.restricted == fresh.restricted
        && old.archived_node == fresh.archived_node
        && old.held.iter().all(|held| fresh.held.contains(held))
}

fn document_authority_preserved(
    old: &orch_authz::documents::Listed,
    fresh: &orch_authz::documents::Listed,
) -> bool {
    old.document.id == fresh.document.id
        && old.document.kind == fresh.document.kind
        && old.document.node == fresh.document.node
        && fresh.holds(Permission::DocView)
        && old.admin_access == fresh.admin_access
        && old.admin_place == fresh.admin_place
}

impl native::Selected for Profile {
    type Source = Source;
    type Query = ();
    type Value = Selection;
    type Reply = UserProfile;
    const DECLARATION: &'static Declaration<Source> = &sources::PROFILE;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Selection, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let person = identity::user_by_username(conn, ctx.param("username")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let root = native::Facts::one(
                Source::ProfileAccount,
                ACCOUNT,
                targets::account(conn, person.id).await?,
            );
            let plan = native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&root))?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let opened = open_person(conn, &principal, person).await?;
            let stored = profiles::pins(conn, opened.person.id).await?;
            let domain_ids: BTreeSet<_> = opened
                .memberships
                .iter()
                .filter(|member| principal.is_in_tree(member.org_id))
                .map(|member| member.domain_id)
                .chain(opened.person.personal_org_id)
                .collect();
            let ids: Vec<_> = stored
                .iter()
                .filter(|(kind, _)| kind == "node")
                .map(|(_, id)| *id)
                .chain(domain_ids.iter().copied())
                .collect();
            let graph = Graph::load(conn, &ids).await.map_err(ApiError::internal)?;
            let mut pins = BTreeMap::new();
            for (_, id) in stored.iter().filter(|(kind, _)| kind == "node") {
                if let Some(target) =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                    && permitted(
                        &plan,
                        caller,
                        Source::ProfilePins,
                        target.object.kind,
                        &target,
                    )?
                {
                    pins.insert(PinKey::Node(*id), target);
                }
            }
            let doc_ids: Vec<_> = stored
                .iter()
                .filter(|(kind, _)| kind == "document")
                .map(|(_, id)| *id)
                .collect();
            let rows: Vec<_> = documents::get_many(conn, &doc_ids)
                .await?
                .into_iter()
                .filter(|row| row.deleted_at.is_none())
                .collect();
            let document_targets =
                read_sources::Sources::from_objects(rows.iter().map(|row| ObjectRef {
                    kind: ObjectKind::Document,
                    id: row.id,
                }))
                .targets(conn)
                .await?;
            let mut docs = Vec::new();
            for row in rows {
                let target = document_targets
                    .get(&ObjectRef {
                        kind: ObjectKind::Document,
                        id: row.id,
                    })
                    .ok_or_else(ApiError::not_found)?;
                if permitted(
                    &plan,
                    caller,
                    Source::ProfilePins,
                    KeyTargetKind::Object(ObjectKind::Document),
                    target,
                )? {
                    pins.insert(PinKey::Document(row.id), target.clone());
                    docs.push(row);
                }
            }
            let mut places = BTreeMap::new();
            for id in &domain_ids {
                if let Some(target) =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                    && permitted(
                        &plan,
                        caller,
                        Source::ProfilePlaces,
                        target.object.kind,
                        &target,
                    )?
                {
                    places.insert(*id, target);
                }
            }
            let known = native::bind_known(
                ctx,
                Self::DECLARATION,
                &[
                    root.clone(),
                    native::Facts::collection(
                        Source::ProfilePins,
                        groups(
                            pins.values()
                                .cloned()
                                .map(|target| (target.object.kind, target)),
                        ),
                    ),
                    native::Facts::collection(
                        Source::ProfilePlaces,
                        groups(
                            places
                                .values()
                                .cloned()
                                .map(|target| (target.object.kind, target)),
                        ),
                    ),
                ],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let node_ids: Vec<_> = stored
                .iter()
                .filter(|(kind, id)| kind == "node" && pins.contains_key(&PinKey::Node(*id)))
                .map(|(_, id)| *id)
                .collect();
            let nodes =
                view::visible(conn, &principal, &node_ids, ctx.now, Policy::default()).await?;
            let projector =
                projection::Projection::response(ctx, caller, known.clone(), context_slot(&known)?);
            let nodes = projector.project(conn, &principal, nodes).await?;
            let node_authority = nodes::effective_many(
                conn,
                &principal,
                &nodes.iter().map(node_id).collect::<Vec<_>>(),
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            let mut context = BTreeSet::new();
            let mut associations = BTreeMap::new();
            let mut nodes_by_id = HashMap::new();
            for node in nodes {
                let id = node_id(&node);
                let own = object_kind(
                    pins.get(&PinKey::Node(id))
                        .ok_or_else(|| ApiError::internal("a native profile pin changed identity"))?
                        .object
                        .kind,
                )?;
                let sources: BTreeSet<_> = read_sources::Sources::of(&node)?
                    .objects()
                    .filter(|source| source.id != id || source.kind != own)
                    .map(|source| (source.kind, source.id))
                    .collect();
                context.extend(sources.iter().copied());
                associations.insert((own, id), sources);
                nodes_by_id.insert(id, node);
            }
            let doc_ids: Vec<_> = docs.iter().map(|row| row.id).collect();
            let mut listed: HashMap<_, _> =
                orch_authz::documents::listed(conn, &principal, &doc_ids, Policy::default())
                    .await
                    .map_err(ApiError::internal)?
                    .into_iter()
                    .filter(|entry| entry.holds(Permission::DocView))
                    .map(|entry| (entry.document.id, entry))
                    .collect();
            let entries: Vec<_> = docs
                .into_iter()
                .filter_map(|row| listed.remove(&row.id).map(|listed| (row, listed)))
                .collect();
            let mut document_candidates =
                ProjectedDocumentRef::candidates_many(conn, &entries).await?;
            let candidates: BTreeSet<_> = document_candidates.values().flatten().copied().collect();
            let name_ids: Vec<_> = candidates
                .iter()
                .filter_map(|source| match source {
                    WrittenLabelSource::Node(id) => Some(*id),
                    _ => None,
                })
                .collect();
            let names = Graph::load(conn, &name_ids)
                .await
                .map_err(ApiError::internal)?;
            let mut allowed = Vec::new();
            for candidate in candidates {
                if let WrittenLabelSource::Node(id) = candidate
                    && let Some(target) =
                        orch_engine::operation_scope::targets::from_node_identity(&names, id)
                    && permitted(
                        &plan,
                        caller,
                        Source::ProfileDocumentNames,
                        target.object.kind,
                        &target,
                    )?
                {
                    allowed.push(candidate);
                }
            }
            // The document-name candidates are complete here; node context and places
            // have separate roles, so a not-yet-selected component is never bound empty.
            let mut document_names = BTreeSet::new();
            for source in &allowed {
                if let WrittenLabelSource::Node(id) = source
                    && let Some(node) = names.get(*id)
                {
                    document_names.insert((ObjectKind::from(node.kind), *id));
                }
            }
            let node_context = object_facts(conn, Source::ProfileContext, &context).await?;
            let document_context =
                object_facts(conn, Source::ProfileDocumentNames, &document_names).await?;
            let known = native::bind_known(
                ctx,
                Self::DECLARATION,
                &[
                    root.clone(),
                    native::Facts::collection(
                        Source::ProfilePins,
                        groups(
                            pins.values()
                                .cloned()
                                .map(|target| (target.object.kind, target)),
                        ),
                    ),
                    native::Facts::collection(
                        Source::ProfilePlaces,
                        groups(
                            places
                                .values()
                                .cloned()
                                .map(|target| (target.object.kind, target)),
                        ),
                    ),
                    node_context,
                    document_context,
                ],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let (doc_refs, disclosed) = if native::modern(caller) {
                ProjectedDocumentRef::capture_many_allowed(
                    conn,
                    &principal,
                    &entries,
                    ctx.now,
                    Policy::default(),
                    &allowed,
                )
                .await?
            } else {
                (
                    crate::routes::documents::build::refs(
                        conn,
                        &principal,
                        &entries,
                        ctx.now,
                        Policy::default(),
                    )
                    .await?,
                    Vec::new(),
                )
            };
            document_names.clear();
            for source in &disclosed {
                if let WrittenLabelSource::Node(id) = source
                    && let Some(node) = names.get(*id)
                {
                    document_names.insert((ObjectKind::from(node.kind), *id));
                }
            }
            let mut docs_by_id: HashMap<_, _> =
                doc_refs.into_iter().map(|doc| (doc.id, doc)).collect();
            for (row, _) in &entries {
                let sources = document_candidates
                    .remove(&row.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|source| match source {
                        WrittenLabelSource::Node(id) => {
                            names.get(id).map(|node| (ObjectKind::from(node.kind), id))
                        }
                        _ => None,
                    })
                    .collect();
                associations.insert((ObjectKind::Document, row.id), sources);
            }
            let place_ids: Vec<_> = places.keys().copied().collect();
            let visible_places: BTreeSet<_> = nodes::holding(
                conn,
                &principal,
                &place_ids,
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|place| place.id)
            .collect();
            let mut output = Vec::new();
            let mut selected = Vec::new();
            for (kind, id) in &stored {
                let pin = if kind == "node" {
                    nodes_by_id.remove(id).map(|node| ProfilePin {
                        node: Some(node),
                        document: None,
                    })
                } else {
                    docs_by_id.remove(id).map(|document| ProfilePin {
                        node: None,
                        document: Some(document),
                    })
                };
                if let Some(pin) = pin {
                    let key = if kind == "node" {
                        PinKey::Node(*id)
                    } else {
                        PinKey::Document(*id)
                    };
                    selected.push(pins[&key].clone());
                    output.push(pin);
                }
            }
            let node_context = object_facts(conn, Source::ProfileContext, &context).await?;
            let document_context =
                object_facts(conn, Source::ProfileDocumentNames, &document_names).await?;
            let facts = vec![
                root,
                native::Facts::collection(
                    Source::ProfilePins,
                    groups(
                        selected
                            .into_iter()
                            .map(|target| (target.object.kind, target)),
                    ),
                ),
                native::Facts::collection(
                    Source::ProfilePlaces,
                    groups(visible_places.iter().map(|id| {
                        let target = places[id].clone();
                        (target.object.kind, target)
                    })),
                ),
                node_context,
                document_context,
            ];
            let can_message = may_message(conn, &principal, &opened).await?;
            Ok((
                Selection {
                    opened,
                    pins: output,
                    places: visible_places,
                    associations,
                    node_authority,
                    documents: entries.into_iter().map(|(_, listed)| listed).collect(),
                    can_message,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the profile reader changed actor"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<UserProfile>> {
        Box::pin(async move {
            shown_parts(
                ctx,
                conn,
                principal,
                &value.opened,
                value.pins.clone(),
                Some(&value.places),
            )
            .await
        })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a (),
        opened: &'a Selection,
        fresh: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.opened.person.id == fresh.opened.person.id
                && (!opened.opened.page.show_time || fresh.opened.page.show_time)
                && (!opened.opened.page.show_activity || fresh.opened.page.show_activity)
                && (!opened.can_message || fresh.can_message)
                && opened.associations.iter().all(|(pin, sources)| {
                    fresh
                        .associations
                        .get(pin)
                        .is_some_and(|current| sources.is_subset(current))
                })
                && opened.node_authority.iter().all(|old| {
                    fresh
                        .node_authority
                        .iter()
                        .any(|current| node_authority_preserved(old, current))
                })
                && opened.documents.iter().all(|old| {
                    fresh
                        .documents
                        .iter()
                        .any(|current| document_authority_preserved(old, current))
                }))
        })
    }
}
