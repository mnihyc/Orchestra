//! Prompt text and blobs retain their actual document/chat/source governors under every reader.
use super::*;
use crate::{key_operations::native, registry::Ctx};
use futures::future::BoxFuture;
use orch_access_contract::{key_scope::KeyResolvedTarget, permissions::NodePermission};
use orch_api_contract::{
    common::Page,
    model_inputs::ModelInputListQuery,
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver},
};
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn,
    queries::model_inputs::{self as rows, ModelInputFilter},
};
use orch_engine::{
    operation_scope::{Prepared, targets},
    summaries::sources,
};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Node,
    Inputs,
}
const BINDINGS: &[Binding<Role>] = &[
    Binding {
        source: Role::Node,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    },
    Binding {
        source: Role::Inputs,
        target: TargetResolver::ResolvedCollection("model_input_sources"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::Collection,
    },
];
const LIST: Declaration<Role> = Declaration {
    operation: "NodeModelInputList",
    bindings: BINDINGS,
};
const BLOB: Declaration<Role> = Declaration {
    operation: "NodeModelInputBlobGet",
    bindings: BINDINGS,
};
fn source_error(_: impl std::fmt::Display) -> ApiError {
    ApiError::not_found()
}
fn facts(node: KeyResolvedTarget, inputs: Vec<KeyResolvedTarget>) -> Vec<native::Facts<Role>> {
    let mut groups = BTreeMap::new();
    for input in inputs {
        groups
            .entry(input.object.kind)
            .or_insert_with(Vec::new)
            .push(input);
    }
    vec![
        native::Facts::one(Role::Node, node.object.kind, node),
        native::Facts::collection(Role::Inputs, groups.into_iter().collect()),
    ]
}
async fn require_node(conn: &mut Conn, principal: &Principal, node: Id) -> ApiResult<()> {
    let target = targets::node(conn, node).await.map_err(source_error)?;
    let orch_access_contract::key_scope::KeyTargetKind::Object(kind) = target.object.kind else {
        return Err(ApiError::not_found());
    };
    let permission = kind
        .node()
        .and_then(|kind| NodePermission::View.on(kind))
        .ok_or_else(ApiError::not_found)?;
    if !orch_authz::nodes::check(
        conn,
        principal,
        node,
        permission,
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
    )
    .await
    .map_err(source_error)?
    .is_allowed()
    {
        return Err(ApiError::not_found());
    }
    Ok(())
}
async fn marker_facts(
    conn: &mut Conn,
    principal: &Principal,
    marker: &rows::Governors,
    plan: Option<&Prepared>,
) -> ApiResult<Vec<KeyResolvedTarget>> {
    if marker
        .viewer_id
        .is_some_and(|viewer| viewer != principal.caller().user())
    {
        return Err(ApiError::not_found());
    }
    let refs: Vec<orch_api_contract::summaries::SummaryTarget> =
        serde_json::from_value(marker.source_refs.clone()).map_err(ApiError::internal)?;
    if refs.is_empty() {
        return Err(ApiError::not_found());
    }
    let mut inputs = Vec::new();
    for target in refs {
        let identity = sources::identify(conn, target)
            .await
            .map_err(source_error)?;
        if plan.is_some_and(|plan| {
            !plan
                .permits_binding(
                    principal.caller(),
                    1,
                    identity.fact().object.kind,
                    std::slice::from_ref(identity.fact()),
                )
                .unwrap_or(false)
        }) || !sources::viewable(conn, principal, target)
            .await
            .map_err(source_error)?
        {
            return Err(ApiError::not_found());
        }
        inputs.push(identity.fact().clone());
    }
    Ok(inputs)
}
pub(super) async fn require_record(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    record: Id,
) -> ApiResult<()> {
    if let Some(marker) = rows::governors(conn, node)
        .await?
        .into_iter()
        .find(|marker| marker.model_input_id == record)
    {
        marker_facts(conn, principal, &marker, None).await?;
    }
    Ok(())
}

pub(super) async fn note_record_opened(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    record: Id,
) -> ApiResult<()> {
    if let Some(marker) = rows::governors(conn, node)
        .await?
        .into_iter()
        .find(|marker| marker.model_input_id == record)
    {
        let inputs = marker_facts(conn, principal, &marker, None).await?;
        crate::routes::summaries::note_chat_openings(conn, principal, &inputs).await?;
    }
    Ok(())
}

pub(super) struct Read {
    rows: Vec<ModelInputRow>,
    next_cursor: Option<String>,
    node: KeyResolvedTarget,
    inputs: Vec<KeyResolvedTarget>,
}
pub(super) struct Reader;
impl native::Selected for Reader {
    type Source = Role;
    type Query = ModelInputListQuery;
    type Value = Read;
    type Reply = Page<ModelInput>;
    const DECLARATION: &'static Declaration<Role> = &LIST;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a ModelInputListQuery,
    ) -> BoxFuture<'a, ApiResult<(Read, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let node = ctx.id("id")?;
            let limit = match query.limit {
                None => PAGE_DEFAULT,
                Some(n) if n < 1 => return Err(ApiError::invalid("limit", "must be at least 1")),
                Some(n) => n.min(PAGE_MAX),
            };
            let plan = Prepared::new(ctx.route, LIST.operation).map_err(source_error)?;
            let principal = Principal::load(
                conn,
                plan.preparation_caller(caller).map_err(source_error)?,
                ctx.app.clock.time(),
            )
            .await
            .map_err(ApiError::internal)?;
            require_node(conn, &principal, node).await?;
            let node = targets::node(conn, node).await.map_err(source_error)?;
            let mut readable = Vec::new();
            let mut governors = BTreeMap::new();
            for marker in
                rows::governors(conn, node.object.id.ok_or_else(ApiError::not_found)?).await?
            {
                match marker_facts(conn, &principal, &marker, Some(&plan)).await {
                    Ok(inputs) => {
                        readable.push(marker.model_input_id);
                        governors.insert(marker.model_input_id, inputs);
                    }
                    Err(error)
                        if error.code() == orch_api_contract::common::ErrorCode::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            let mut found = rows::list_for_sources(
                conn,
                node.object.id.ok_or_else(ApiError::not_found)?,
                &ModelInputFilter {
                    id: query.id,
                    turn: query.turn_id,
                    before: query.before,
                    after: query.after,
                    limit: limit + 1,
                },
                &readable,
            )
            .await?;
            let page = usize::try_from(limit).unwrap_or(usize::MAX);
            let more = found.len() > page;
            found.truncate(page);
            let next_cursor = if more {
                found.last().map(|row| row.seq_created.to_string())
            } else {
                None
            };
            let mut seen = BTreeSet::new();
            let mut inputs = Vec::new();
            for row in &found {
                if let Some(refs) = governors.remove(&row.id) {
                    for input in refs {
                        if seen.insert(input.object) {
                            inputs.push(input);
                        }
                    }
                }
            }
            let facts = facts(node.clone(), inputs.clone());
            Ok((
                Read {
                    rows: found,
                    next_cursor,
                    node,
                    inputs,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require_node(conn, principal, ctx.id("id")?).await })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Read,
    ) -> BoxFuture<'a, ApiResult<Page<ModelInput>>> {
        Box::pin(async move {
            Ok(Page {
                items: value
                    .rows
                    .iter()
                    .cloned()
                    .map(record)
                    .collect::<ApiResult<_>>()?,
                next_cursor: value.next_cursor.clone(),
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a ModelInputListQuery,
        old: &'a Read,
        fresh: &'a Read,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.rows == fresh.rows
                && old.next_cursor == fresh.next_cursor
                && old.node == fresh.node
                && old.inputs == fresh.inputs)
        })
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        _: &'a Caller,
        _: &'a ModelInputListQuery,
        value: &'a Read,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let (mut conn, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
            crate::routes::summaries::note_chat_openings(
                &mut conn,
                guard.principal(),
                &value.inputs,
            )
            .await?;
            conn.commit().await.map_err(orch_db::DbError::from)?;
            Ok(())
        })
    }
}

pub(super) struct BlobValue {
    node: Id,
    record: Id,
    marker: Option<rows::Governors>,
}
pub(super) struct Blob;
impl native::Stored for Blob {
    type Source = Role;
    type Value = BlobValue;
    const ROWS: &'static [Declaration<Role>] = &[BLOB];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'static Declaration<Role>,
    ) -> BoxFuture<'a, ApiResult<(BlobValue, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let node = ctx.id("id")?;
            let record = ctx.id("model_input_id")?;
            let marker = rows::governors(conn, node)
                .await?
                .into_iter()
                .find(|marker| marker.model_input_id == record);
            let mut inputs = Vec::new();
            if let Some(marker) = &marker {
                let refs: Vec<orch_api_contract::summaries::SummaryTarget> =
                    serde_json::from_value(marker.source_refs.clone())
                        .map_err(ApiError::internal)?;
                for target in refs {
                    inputs.push(
                        sources::identify(conn, target)
                            .await
                            .map_err(source_error)?
                            .fact()
                            .clone(),
                    );
                }
            }
            let node_fact = targets::node(conn, node).await.map_err(source_error)?;
            Ok((
                BlobValue {
                    node,
                    record,
                    marker,
                },
                facts(node_fact, inputs),
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a BlobValue,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_node(conn, principal, value.node).await?;
            if let Some(marker) = &value.marker {
                marker_facts(conn, principal, marker, None).await?;
            }
            require_record(conn, principal, value.node, value.record).await
        })
    }
}
