//! Native upload facts shared by the personal list and the existing Storage report.

use crate::key_operations::targets;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::access::AdminAccess;

use orch_api_contract::nodes::NodeOrSystem;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::threads::UploadAttachment;

use orch_api_contract::threads::UploadBlobDelete;

use orch_api_contract::threads::UploadInventoryItem;

use orch_api_contract::threads::UploadInventoryKind;

use orch_api_contract::threads::UploadInventoryState;

use orch_api_contract::threads::UploadSessionDelete;

use orch_api_contract::upload_sources as sources;

use orch_api_contract::upload_sources::Source;

use orch_authz::Caller;

use orch_db::Conn;

use orch_db::queries::storage;

use orch_db::queries::uploads::inventory;

use orch_engine::operation_scope::sources as binder;

use orch_engine::operation_scope::sources::Facts;

use orch_types::Id;

use orch_types::Time;

use std::collections::HashSet;
/// Stored operands used by publication witnesses, before any response formatting.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub row: inventory::Row,
    pub attachments: Vec<(String, Id)>,
    pub attachment_count: i64,
    pub can_delete: bool,
}

/// A display control checks the other operation's complete canonical facts. It supplies no
/// admitted caller, capture, route substitution or effect to this list's request.
pub(crate) async fn deletable(
    conn: &mut Conn,
    caller: &Caller,
    row: &inventory::Row,
) -> ApiResult<bool> {
    if row.user_id != Some(caller.user()) {
        return Ok(false);
    }
    let upload = KeyTargetKind::Identity(KeyIdentityKind::Upload);
    let plan = match row.kind.as_str() {
        "completed" => binder::bind(
            UploadBlobDelete::ROUTE,
            &sources::RELEASE,
            &[Facts::one(
                Source::CustodyAccount,
                upload,
                targets::account(conn, caller.user()).await?,
            )],
        ),
        "session" => {
            let Some(session) = row.session_id else {
                return Err(ApiError::internal("an upload session has no identity"));
            };
            let Some(stored) = storage::session(conn, session).await? else {
                return Ok(false);
            };
            binder::bind(
                UploadSessionDelete::ROUTE,
                &sources::SESSION_DELETE,
                &[Facts::one(
                    Source::Session,
                    upload,
                    targets::from_upload_session(conn, &stored).await?,
                )],
            )
        }
        _ => return Ok(false),
    }
    .map_err(ApiError::internal)?;
    Ok(plan.admit(caller, false).is_ok())
}

/// Attachment discovery uses recorded authorship for the personal list, and actual global
/// references only for a current native administrative reader.
pub async fn prepare(
    conn: &mut Conn,
    rows: Vec<inventory::Row>,
    personal: Option<&Caller>,
    preview: u32,
) -> ApiResult<Vec<Entry>> {
    let mut entries = Vec::new();
    for row in rows {
        let (attachment_count, attachments) = if let Some(hash) = row.blob()? {
            inventory::attachments(conn, &hash, personal.map(Caller::user), i64::from(preview))
                .await?
        } else {
            (0, Vec::new())
        };
        if personal.is_some() && attachment_count > 0 {
            continue;
        }
        let can_delete = match personal {
            Some(caller) => deletable(conn, caller, &row).await?,
            None => false,
        };
        entries.push(Entry {
            row,
            attachments,
            attachment_count,
            can_delete,
        });
    }
    Ok(entries)
}

pub struct Projection {
    pub owners: HashSet<Id>,
    pub machines: HashSet<Id>,
    pub administrative: bool,
    pub at: Time,
}

/// Names are explicitly selected by the consuming operation, independently of upload content.
pub async fn project(
    ctx: &Ctx,
    conn: &mut Conn,
    entries: &[Entry],
    projection: Projection,
) -> ApiResult<Vec<UploadInventoryItem>> {
    let owners =
        refusals::user_refs(conn, &projection.owners.iter().copied().collect::<Vec<_>>()).await?;
    let machines = crate::routes::resources::view::machine_refs(
        &ctx.app,
        conn,
        &projection.machines.iter().copied().collect::<Vec<_>>(),
    )
    .await?;
    entries
        .iter()
        .map(|entry| {
            let row = &entry.row;
            let kind = match row.kind.as_str() {
                "completed" => UploadInventoryKind::Completed,
                "session" => UploadInventoryKind::Session,
                "machine" => UploadInventoryKind::Machine,
                _ => return Err(ApiError::internal("an unknown upload inventory source")),
            };
            let state = if kind == UploadInventoryKind::Completed {
                UploadInventoryState::Complete
            } else if row
                .expires_at
                .is_some_and(|deadline| deadline < projection.at.get())
            {
                UploadInventoryState::Expired
            } else {
                UploadInventoryState::Partial
            };
            Ok(UploadInventoryItem {
                id: row.key.clone(),
                kind,
                state,
                content_id: row.content_id,
                session_id: row.session_id,
                blob: row.blob()?,
                uploader: row.user_id.and_then(|id| owners.get(&id).cloned()),
                machine: row.machine_id.and_then(|id| machines.get(&id).cloned()),
                name: row.name.clone(),
                media_type: row.media_type.clone(),
                size: row.size,
                received: row.received,
                created_at: Time::new(row.created_at),
                updated_at: Time::new(row.updated_at),
                expires_at: row.expires_at.map(Time::new),
                attached_to: entry
                    .attachments
                    .iter()
                    .map(|(kind, id)| UploadAttachment {
                        kind: kind.clone(),
                        id: *id,
                    })
                    .collect(),
                attachment_count: entry.attachment_count,
                can_open: projection.administrative && row.content_id.is_some(),
                can_delete: entry.can_delete,
                admin_access: projection.administrative.then_some(AdminAccess {
                    place: NodeOrSystem::SYSTEM,
                    private: true,
                }),
            })
        })
        .collect()
}
