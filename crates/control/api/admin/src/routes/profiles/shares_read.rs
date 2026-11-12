//! Direct shares keep native cursor order and both ordinary readers before the scoped page window.

use crate::key_operations::native;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::routes::documents::build::ProjectedDocumentRef;

use crate::routes::documents::build::WrittenLabelSource;

use crate::routes::nodes::invalid;

use crate::routes::nodes::projection;

use crate::routes::nodes::read_sources;

use crate::routes::nodes::view;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::NodePermission;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::PermissionName;

use orch_api_contract::account_sources as sources;

use orch_api_contract::account_sources::Source;

use orch_api_contract::common::Page;

use orch_api_contract::common::PageQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::users::UserShare;

use orch_authz::engine::Graph;

use orch_authz::Act;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::nodes;

use orch_db::Conn;

use orch_db::queries::documents;

use orch_db::queries::identity;

use orch_db::queries::profiles;

use orch_types::Id;

use orch_types::Time;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::collections::HashMap;

use std::collections::HashSet;
type RowKey = (String, Id);

type ObjectKey = (ObjectKind, Id);

pub(super) struct Shares;

pub(super) struct Selection {
    other: Id,
    page: Page<UserShare>,
    associations: BTreeMap<RowKey, BTreeSet<ObjectKey>>,
    documents: Vec<orch_authz::documents::Listed>,
}

fn cursor(query: &PageQuery) -> ApiResult<Option<RowKey>> {
    query
        .cursor
        .as_deref()
        .map(|text| {
            let bad = || invalid("cursor", "is not a cursor of this list");
            let (kind, id) = text.split_once('|').ok_or_else(bad)?;
            if !matches!(kind, "node" | "document" | "document_container") {
                return Err(bad());
            }
            Ok((kind.to_owned(), id.parse().map_err(|_| bad())?))
        })
        .transpose()
}

fn groups(
    targets: impl IntoIterator<Item = KeyResolvedTarget>,
) -> Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)> {
    projection::groups(
        targets
            .into_iter()
            .map(|target| (target.object.kind, target)),
    )
}

impl native::Selected for Shares {
    type Source = Source;
    type Query = PageQuery;
    type Value = Selection;
    type Reply = Page<UserShare>;
    const DECLARATION: &'static Declaration<Source> = &sources::USER_SHARES;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(Selection, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            if !native::modern(caller) {
                native::preparation_principal(ctx, conn, caller, &plan).await?;
            }
            let person = identity::user_by_id(conn, ctx.id("id")?)
                .await?
                .filter(|person| person.kind == "human")
                .ok_or_else(ApiError::not_found)?;
            // This principal is observational only. It never acts, scopes the request or renders actor access.
            let comparison = Principal::load(conn, Caller::session(person.id), Time::new(ctx.now))
                .await
                .map_err(ApiError::internal)?;
            let mut after = cursor(query)?;
            let limit = usize::try_from(query.limit()).map_err(ApiError::internal)?;
            let batch = i64::from(query.limit()) + 1;
            let mut found = Vec::new();
            let mut returned_documents = BTreeMap::new();
            if caller.user() != person.id {
                while found.len() <= limit {
                    let candidates = profiles::direct_shares(
                        conn,
                        caller.user(),
                        person.id,
                        ctx.now,
                        after.as_ref().map(|(kind, id)| (kind.as_str(), *id)),
                        batch,
                    )
                    .await?;
                    let at_end = candidates.len() <= limit;
                    if candidates.is_empty() {
                        break;
                    }
                    after = candidates.last().cloned();
                    let node_ids: Vec<_> = candidates
                        .iter()
                        .filter(|(kind, _)| kind == "node")
                        .map(|(_, id)| *id)
                        .collect();
                    let graph = Graph::load(conn, &node_ids)
                        .await
                        .map_err(ApiError::internal)?;
                    let doc_ids: Vec<_> = candidates
                        .iter()
                        .filter(|(kind, _)| kind != "node")
                        .map(|(_, id)| *id)
                        .collect();
                    let doc_rows: Vec<_> = documents::get_many(conn, &doc_ids)
                        .await?
                        .into_iter()
                        .filter(|row| row.deleted_at.is_none())
                        .collect();
                    let folder_rows: Vec<_> = documents::containers::get_many(conn, &doc_ids)
                        .await?
                        .into_iter()
                        .filter(|row| row.deleted_at.is_none())
                        .collect();
                    let objects = read_sources::Sources::from_objects(
                        doc_rows
                            .iter()
                            .map(|row| ObjectRef {
                                kind: ObjectKind::Document,
                                id: row.id,
                            })
                            .chain(folder_rows.iter().map(|row| ObjectRef {
                                kind: ObjectKind::DocumentContainer,
                                id: row.id,
                            })),
                    );
                    let document_targets = objects.targets(conn).await?;
                    let mut targets_by_row = BTreeMap::new();
                    for (kind, id) in &candidates {
                        let target = match kind.as_str() {
                            "node" => orch_engine::operation_scope::targets::from_node_identity(
                                &graph, *id,
                            ),
                            "document" => document_targets
                                .get(&ObjectRef {
                                    kind: ObjectKind::Document,
                                    id: *id,
                                })
                                .cloned(),
                            "document_container" => document_targets
                                .get(&ObjectRef {
                                    kind: ObjectKind::DocumentContainer,
                                    id: *id,
                                })
                                .cloned(),
                            _ => {
                                return Err(ApiError::internal(
                                    "a native direct share has no typed source",
                                ));
                            }
                        };
                        if let Some(target) = target
                            && (!native::modern(caller)
                                || native::permits(
                                    &plan,
                                    Self::DECLARATION,
                                    caller,
                                    &Source::SharedResults,
                                    target.object.kind,
                                    &target,
                                )?)
                        {
                            targets_by_row.insert((kind.clone(), *id), target);
                        }
                    }
                    // No candidate proof is called empty while later native windows may still contain a match.
                    if targets_by_row.is_empty() {
                        if at_end {
                            break;
                        } else {
                            continue;
                        }
                    }
                    let known = native::bind_known(
                        ctx,
                        Self::DECLARATION,
                        &[native::Facts::collection(
                            Source::SharedResults,
                            groups(targets_by_row.values().cloned()),
                        )],
                    )?;
                    let principal =
                        native::preparation_principal(ctx, conn, caller, &known).await?;
                    let ids: Vec<_> = node_ids
                        .into_iter()
                        .filter(|id| targets_by_row.contains_key(&("node".to_owned(), *id)))
                        .collect();
                    let theirs: HashSet<_> = nodes::holding(
                        conn,
                        &comparison,
                        &ids,
                        PermissionName::OfNode(NodePermission::View),
                        Act::Use,
                        Policy::default(),
                    )
                    .await
                    .map_err(ApiError::internal)?
                    .into_iter()
                    .map(|row| row.id)
                    .collect();
                    let visible: Vec<_> = nodes::holding(
                        conn,
                        &principal,
                        &ids,
                        PermissionName::OfNode(NodePermission::View),
                        Act::Use,
                        Policy::default(),
                    )
                    .await
                    .map_err(ApiError::internal)?
                    .into_iter()
                    .filter(|row| theirs.contains(&row.id))
                    .map(|row| row.id)
                    .collect();
                    let mut node_refs =
                        view::refs(conn, &principal, &visible, Policy::default()).await?;
                    let ids: Vec<_> = doc_ids
                        .into_iter()
                        .filter(|id| {
                            targets_by_row.contains_key(&("document".to_owned(), *id))
                                || targets_by_row
                                    .contains_key(&("document_container".to_owned(), *id))
                        })
                        .collect();
                    let theirs: HashSet<_> =
                        orch_authz::documents::listed(conn, &comparison, &ids, Policy::default())
                            .await
                            .map_err(ApiError::internal)?
                            .into_iter()
                            .filter(|row| row.holds(Permission::DocView))
                            .map(|row| (row.document.kind, row.document.id))
                            .collect();
                    let mut held: HashMap<_, _> =
                        orch_authz::documents::listed(conn, &principal, &ids, Policy::default())
                            .await
                            .map_err(ApiError::internal)?
                            .into_iter()
                            .filter(|row| {
                                theirs.contains(&(row.document.kind, row.document.id))
                                    && row.holds(Permission::DocView)
                            })
                            .map(|row| ((row.document.kind, row.document.id), row))
                            .collect();
                    let entries: Vec<_> = doc_rows
                        .into_iter()
                        .filter_map(|row| {
                            held.remove(&(ObjectKind::Document, row.id))
                                .map(|listed| (row, listed))
                        })
                        .collect();
                    let mut folders: HashMap<_, _> = folder_rows.into_iter().filter(|row| held.contains_key(&(ObjectKind::DocumentContainer, row.id)))
                        .map(|row| (row.id, orch_api_contract::document_tree::DocumentContainerRef { id: row.id, kind: orch_api_contract::document_tree::DocumentContainerMark::DocumentContainer, name: Some(row.name), redacted: false })).collect();
                    let source_names =
                        ProjectedDocumentRef::candidates_many(conn, &entries).await?;
                    let node_names: BTreeSet<_> = source_names
                        .values()
                        .flatten()
                        .filter_map(|source| match source {
                            WrittenLabelSource::Node(id) => Some(*id),
                            _ => None,
                        })
                        .collect();
                    let name_graph =
                        Graph::load(conn, &node_names.iter().copied().collect::<Vec<_>>())
                            .await
                            .map_err(ApiError::internal)?;
                    let mut allowed_names = Vec::new();
                    let mut name_targets = Vec::new();
                    for id in &node_names {
                        if let Some(target) =
                            orch_engine::operation_scope::targets::from_node_identity(
                                &name_graph,
                                *id,
                            )
                            && (!native::modern(caller)
                                || native::permits_usage(
                                    &plan,
                                    Self::DECLARATION,
                                    caller,
                                    &Source::SharedDocumentNames,
                                    FacetUsage::ResponseContent,
                                    target.object.kind,
                                    &target,
                                )?)
                        {
                            allowed_names.push(WrittenLabelSource::Node(*id));
                            name_targets.push(target);
                        }
                    }
                    let known = native::bind_known(
                        ctx,
                        Self::DECLARATION,
                        &[
                            native::Facts::collection(
                                Source::SharedResults,
                                groups(targets_by_row.values().cloned()),
                            ),
                            native::Facts::collection(
                                Source::SharedDocumentNames,
                                groups(name_targets),
                            ),
                        ],
                    )?;
                    let principal =
                        native::preparation_principal(ctx, conn, caller, &known).await?;
                    let (refs, disclosed) = if native::modern(caller) {
                        ProjectedDocumentRef::capture_many_allowed(
                            conn,
                            &principal,
                            &entries,
                            ctx.now,
                            Policy::default(),
                            &allowed_names,
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
                    let disclosed: BTreeSet<_> = disclosed.into_iter().collect();
                    let mut docs: HashMap<_, _> =
                        refs.into_iter().map(|doc| (doc.id, doc)).collect();
                    for (row, listed) in &entries {
                        returned_documents.insert(row.id, listed.clone());
                    }
                    for (kind, id) in &candidates {
                        let item = match kind.as_str() {
                            "node" => node_refs.remove(id).map(|node| UserShare::Node { node }),
                            "document_container" => folders
                                .remove(id)
                                .map(|folder| UserShare::DocumentContainer { folder }),
                            _ => docs.remove(id).map(|document| UserShare::Document {
                                document: Box::new(document),
                            }),
                        };
                        if let Some(item) = item {
                            let sources: BTreeSet<_> = source_names
                                .get(id)
                                .filter(|_| kind == "document")
                                .into_iter()
                                .flatten()
                                .filter_map(|source| match source {
                                    WrittenLabelSource::Node(id) => name_graph
                                        .get(*id)
                                        .map(|node| (ObjectKind::from(node.kind), *id)),
                                    _ => None,
                                })
                                .collect();
                            let emitted: BTreeSet<_> = if kind == "document" {
                                sources
                                    .iter()
                                    .copied()
                                    .filter(|(_, id)| {
                                        disclosed.contains(&WrittenLabelSource::Node(*id))
                                    })
                                    .collect()
                            } else {
                                BTreeSet::new()
                            };
                            found.push((
                                (kind.clone(), *id),
                                item,
                                targets_by_row[&(kind.clone(), *id)].clone(),
                                sources,
                                emitted,
                            ));
                        }
                    }
                    if at_end {
                        break;
                    }
                }
            }
            let more = found.len() > limit;
            let lookahead = found.get(limit).map(|(_, _, target, _, _)| target.clone());
            found.truncate(limit);
            let next_cursor = more
                .then(|| {
                    found
                        .last()
                        .map(|((kind, id), _, _, _, _)| format!("{kind}|{id}"))
                })
                .flatten();
            // Names and metadata of the unreturned look-ahead are not disclosure sources.
            let returned_ids: BTreeSet<_> = found
                .iter()
                .filter(|((kind, _), _, _, _, _)| kind == "document")
                .map(|((_, id), _, _, _, _)| *id)
                .collect();
            let document_entries: Vec<_> = returned_documents
                .into_iter()
                .filter(|(id, _)| returned_ids.contains(id))
                .map(|(_, listed)| listed)
                .collect();
            let named: BTreeSet<_> = found
                .iter()
                .flat_map(|(_, _, _, _, emitted)| emitted.iter().copied())
                .collect();
            let resolved_names = read_sources::Sources::from_objects(
                named.iter().map(|&(kind, id)| ObjectRef { kind, id }),
            )
            .identity_targets(conn)
            .await?;
            let mut selected: Vec<_> = found
                .iter()
                .map(|(_, _, target, _, _)| target.clone())
                .collect();
            selected.extend(lookahead);
            let facts = vec![
                native::Facts::collection(Source::SharedResults, groups(selected)),
                native::Facts::collection(
                    Source::SharedDocumentNames,
                    projection::groups(
                        resolved_names
                            .into_iter()
                            .map(|(object, target)| (KeyTargetKind::Object(object.kind), target)),
                    ),
                ),
            ];
            let associations = found
                .iter()
                .map(|(key, _, _, names, _)| (key.clone(), names.clone()))
                .collect();
            Ok((
                Selection {
                    other: person.id,
                    page: Page {
                        items: found.into_iter().map(|(_, item, _, _, _)| item).collect(),
                        next_cursor,
                    },
                    associations,
                    documents: document_entries,
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
                return Err(ApiError::internal(
                    "the comparison principal cannot become the request actor",
                ));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<Page<UserShare>>> {
        Box::pin(async move { Ok(value.page.clone()) })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a PageQuery,
        opened: &'a Selection,
        fresh: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.other == fresh.other
                && opened.associations.iter().all(|(key, names)| {
                    fresh
                        .associations
                        .get(key)
                        .is_some_and(|current| names.is_subset(current))
                })
                && opened.documents.iter().all(|old| {
                    fresh.documents.iter().any(|current| {
                        old.document.id == current.document.id
                            && old.document.kind == current.document.kind
                            && old.document.node == current.document.node
                            && current.holds(Permission::DocView)
                            && old.admin_access == current.admin_access
                            && old.admin_place == current.admin_place
                    })
                }))
        })
    }
}
