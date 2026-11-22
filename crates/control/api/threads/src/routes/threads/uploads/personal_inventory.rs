//! A person's actual unattached upload custody, with the list's own bounded name sources.

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::upload_inventory;

use crate::upload_inventory::Entry;

use crate::upload_inventory::Projection;

use chrono::DateTime;

use chrono::Utc;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::common::Page;

use orch_api_contract::common::CollectionQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::threads::MeUploadList;

use orch_api_contract::threads::UploadInventoryItem;

use orch_api_contract::upload_sources as sources;

use orch_api_contract::upload_sources::Source;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::uploads::inventory;

use orch_types::Id;

use std::collections::HashSet;

use std::sync::Arc;
pub(super) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        MeUploadList,
        native::selected::<Uploads>,
        native::selected_read::<Uploads>,
    );
}

fn decode(query: &CollectionQuery, owner: Id) -> ApiResult<Option<(DateTime<Utc>, String)>> {
    crate::collection::after(query, &format!("personal-uploads:{owner}"))
}
fn encode(owner: Id, entry: &Entry, query: &CollectionQuery) -> ApiResult<String> {
    crate::collection::cursor(
        query,
        &format!("personal-uploads:{owner}"),
        (entry.row.created_at, entry.row.key.clone()),
    )
}

pub(super) struct Uploads;

pub(super) struct Selected {
    owner: Id,
    entries: Vec<Entry>,
    next: Option<String>,
    owner_name: bool,
}

impl native::Selected for Uploads {
    type Source = Source;
    type Query = CollectionQuery;
    type Value = Arc<Selected>;
    type Reply = Page<UploadInventoryItem>;
    const MOVES: native::Moves = native::Moves::Repeat;
    const DECLARATION: &'static Declaration<Source> = &sources::OWN_LIST;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a CollectionQuery,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let owner = ctx.user()?.id();
            if caller.user() != owner {
                return Err(ApiError::internal("the upload list changed account"));
            }
            let target = targets::account(conn, owner).await?;
            let root = native::Facts::one(
                Source::InventoryNamespace,
                KeyTargetKind::Identity(KeyIdentityKind::Upload),
                target.clone(),
            );
            let base = native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&root))?;
            let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
            Self::require(ctx, conn, &principal).await?;
            let owner_name = !native::modern(caller)
                || native::permits_usage(
                    &base,
                    Self::DECLARATION,
                    caller,
                    &Source::InventoryNames,
                    FacetUsage::ResponseContent,
                    KeyTargetKind::Identity(KeyIdentityKind::Account),
                    &target,
                )?;
            let mut after = decode(query, owner)?;
            let page_size = crate::collection::limit(query)?;
            let limit = usize::try_from(page_size).map_err(ApiError::internal)?;
            let mut entries = Vec::new();
            loop {
                let rows = inventory::list_named(
                    conn,
                    Some(owner),
                    after.clone(),
                    Some(i64::from(page_size) + 1),
                    query.q.as_deref().unwrap_or(""),
                )
                .await?;
                let exhausted = rows.len() < limit + 1;
                if let Some(last) = rows.last() {
                    after = Some((last.created_at, last.key.clone()));
                }
                // Native machine transfers belong to System Storage, not a private browser list.
                let rows = rows
                    .into_iter()
                    .filter(|row| row.kind != "machine")
                    .collect();
                entries
                    .extend(upload_inventory::prepare(conn, rows, Some(caller), page_size).await?);
                if entries.len() > limit || exhausted {
                    break;
                }
            }
            let more = entries.len() > limit;
            entries.truncate(limit);
            let next = if more {
                entries
                    .last()
                    .map(|entry| encode(owner, entry, query))
                    .transpose()?
            } else {
                None
            };
            let names = native::Facts::collection(
                Source::InventoryNames,
                if owner_name {
                    vec![(
                        KeyTargetKind::Identity(KeyIdentityKind::Account),
                        vec![target],
                    )]
                } else {
                    Vec::new()
                },
            );
            Ok((
                Arc::new(Selected {
                    owner,
                    entries,
                    next,
                    owner_name,
                }),
                vec![root, names],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the upload list changed account"));
            }
            Ok(())
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let owners = if value.owner_name {
                HashSet::from([value.owner])
            } else {
                HashSet::new()
            };
            Ok(Page {
                items: upload_inventory::project(
                    ctx,
                    conn,
                    &value.entries,
                    Projection {
                        owners,
                        machines: HashSet::new(),
                        administrative: false,
                        at: principal.now(),
                    },
                )
                .await?,
                next_cursor: value.next.clone(),
            })
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a CollectionQuery,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.owner == fresh.owner
                && opened.entries == fresh.entries
                && opened.owner_name == fresh.owner_name
                && opened.next == fresh.next)
        })
    }
}
