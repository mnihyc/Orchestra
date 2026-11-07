//! Real import custody and native distinct undo subjects before counts are rendered.

use super::*;
use crate::key_operations::{native, targets};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    import_undo_sources::{PREFLIGHT, Source},
    operation_sources::Declaration,
};
use orch_authz::{Caller, Principal, engine::Graph};
use orch_engine::imports::undo::{Prepared, Subject};
use orch_types::Id;
use std::collections::BTreeMap;

pub(super) struct Preview;
pub(super) struct Ready {
    native: Prepared,
    targets: BTreeMap<(KeyTargetKind, Id), KeyResolvedTarget>,
}

async fn facts(
    conn: &mut Conn,
    native: &Prepared,
) -> ApiResult<(
    Vec<native::Facts<Source>>,
    BTreeMap<(KeyTargetKind, Id), KeyResolvedTarget>,
)> {
    let import = targets::from_import(conn, &native.import).await?;
    let mut found = BTreeMap::new();
    found.insert((import.object.kind, native.import.id), import.clone());
    let mut facts = vec![native::Facts::one(
        Source::Import,
        import.object.kind,
        import,
    )];
    let nodes = native
        .subjects
        .iter()
        .filter_map(|subject| match subject {
            Subject::Node(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut graph = Graph::load(conn, &nodes)
        .await
        .map_err(ApiError::internal)?;
    let ids = native
        .subjects
        .iter()
        .filter_map(|subject| match subject {
            Subject::Document(id) | Subject::Folder(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut documents = BTreeMap::new();
    for document in orch_authz::documents::read_into(conn, &mut graph, &ids)
        .await
        .map_err(ApiError::internal)?
    {
        let site =
            orch_authz::documents::site_of(&graph, &document).ok_or_else(ApiError::not_found)?;
        documents.insert(
            (document.kind, document.id),
            targets::from_site(&graph, &site)?,
        );
    }
    let ids = native
        .subjects
        .iter()
        .filter_map(|subject| match subject {
            Subject::Chat(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rows = orch_db::queries::chats::chats(conn, &ids).await?;
    let chats = targets::from_chat_rows(conn, &rows).await?;
    let mut groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
    for subject in &native.subjects {
        let target = match subject {
            Subject::Node(id) => {
                orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                    .ok_or_else(ApiError::not_found)?
            }
            Subject::Document(id) => documents
                .get(&(ObjectKind::Document, *id))
                .cloned()
                .ok_or_else(ApiError::not_found)?,
            Subject::Folder(id) => documents
                .get(&(ObjectKind::DocumentContainer, *id))
                .cloned()
                .ok_or_else(ApiError::not_found)?,
            Subject::Chat(id) => chats.get(id).cloned().ok_or_else(ApiError::not_found)?,
        };
        let id = target
            .object
            .id
            .ok_or_else(|| ApiError::internal("a native undo subject has no identity"))?;
        found.insert((target.object.kind, id), target.clone());
        groups.entry(target.object.kind).or_default().push(target);
    }
    facts.push(native::Facts::collection(
        Source::Subjects,
        groups.into_iter().collect(),
    ));
    Ok((facts, found))
}

impl native::Selected for Preview {
    type Source = Source;
    type Query = ();
    type Value = Ready;
    type Reply = ImportUndoPlan;
    const DECLARATION: &'static Declaration<Source> = &PREFLIGHT;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let native = if native::modern(caller) {
                match ctx
                    .app
                    .engine
                    .prepare_undo(conn, caller.user(), ctx.id("id")?)
                    .await
                {
                    Ok(value) => value,
                    Err(error) => return Err(failed(conn, error).await),
                }
            } else {
                let import = match ctx
                    .app
                    .engine
                    .import_of(conn, caller.user(), ctx.id("id")?)
                    .await
                {
                    Ok(value) => value,
                    Err(error) => return Err(failed(conn, error).await),
                };
                Prepared {
                    import,
                    subjects: Vec::new(),
                }
            };
            let (facts, targets) = if native::modern(caller) {
                facts(conn, &native).await?
            } else {
                (Vec::new(), BTreeMap::new())
            };
            Ok((Ready { native, targets }, facts))
        })
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
        opened: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let import = match ctx
                .app
                .engine
                .import_of(conn, caller.user(), ctx.id("id")?)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let native = Prepared {
                import,
                subjects: opened.native.subjects.clone(),
            };
            let (facts, targets) = facts(conn, &native).await?;
            Ok((Ready { native, targets }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(super::read_sources::require_own(ctx, conn, principal))
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<ImportUndoPlan>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .plan_undo(conn, principal.user(), value.native.import.id)
                .await
            {
                Ok(value) => Ok(value),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a (),
        opened: &'a Ready,
        fresh: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.targets == fresh.targets
                && super::read_sources::preserves(&opened.native.import, &fresh.native.import)
                && opened.native.import.state == fresh.native.import.state
                && opened.native.import.undo_job_id == fresh.native.import.undo_job_id)
        })
    }
}
