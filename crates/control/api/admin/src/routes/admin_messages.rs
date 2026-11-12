//! One native administrator-message writer for organization and installation audiences.

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::authorization;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_access_contract::permissions::Permission;

use orch_api_contract::admin_message_sources as sources;

use orch_api_contract::admin_message_sources::Source;

use orch_api_contract::admin_messages::*;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::queries::admin_messages as rows;

use orch_db::queries::admin_messages::MessageRow;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use orch_types::Time;
pub fn mount(registry: &mut Registry) {
    registry
        .creation(OrgMessageCreate, writing::replay_binding::<writing::Org>())
        .creation(
            AdminMessageCreate,
            writing::replay_binding::<writing::System>(),
        )
        .on_with_operation(
            OrgMessageCreate,
            native::effects::prepare::<writing::Sending<writing::Org>>,
            writing::send::<writing::Org>,
        )
        .on_with_operation(
            AdminMessageCreate,
            native::effects::prepare::<writing::Sending<writing::System>>,
            writing::send::<writing::System>,
        )
        .on_snapshot_read_with_optional_operation(
            MessageGet,
            native::selected::<reading::Message>,
            native::selected_read::<reading::Message>,
        )
        .on_snapshot_read_with_optional_operation(
            OrgMessageList,
            native::selected::<reading::Sent<reading::Org>>,
            native::selected_read::<reading::Sent<reading::Org>>,
        )
        .on_snapshot_read_with_optional_operation(
            AdminMessageList,
            native::selected::<reading::Sent<reading::System>>,
            native::selected_read::<reading::Sent<reading::System>>,
        );
}
orch_api_base::__api_bind_796195948a!();

/// The native sender gate is shared by both send registrations and their completed readback.
async fn require_sender(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    org: Option<Id>,
) -> ApiResult<()> {
    match org {
        Some(org) => {
            authorization::require(
                conn,
                principal,
                org,
                authorization::exact(Permission::MembersManage),
                Act::Use,
                Policy::default(),
            )
            .await
        }
        None => {
            ctx.ordinary_system_admin_with(ctx.user()?, principal.caller())?;
            if !principal.has_system_admin_standing() {
                return Err(ApiError::new(
                    orch_api_contract::common::ErrorCode::AuthzDenied,
                    "current system administration is required",
                ));
            }
            Ok(())
        }
    }
}

async fn scope_target(
    conn: &mut Conn,
    org: Option<Id>,
) -> ApiResult<orch_access_contract::key_scope::KeyResolvedTarget> {
    match org {
        Some(org) => targets::node(conn, org).await,
        None => {
            orch_db::queries::transaction_guard::lock_action(conn).await?;
            orch_db::queries::transaction_guard::watch_standing(conn).await?;
            Ok(targets::system())
        }
    }
}

async fn require_reader(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &MessageRow,
) -> ApiResult<()> {
    if rows::is_recipient(conn, row.id, principal.user()).await? {
        return Ok(());
    }
    match require_sender(ctx, conn, principal, row.org_id).await {
        Err(error)
            if matches!(
                error.code(),
                orch_api_contract::common::ErrorCode::AuthzDenied
                    | orch_api_contract::common::ErrorCode::NotFound
            ) =>
        {
            Err(ApiError::not_found())
        }
        result => result,
    }
}

fn view(row: &MessageRow) -> ApiResult<AdminMessage> {
    let org = row.org_id.map(|id| NodeRef {
        id,
        kind: NodeKind::Org,
        name: row.org_name.clone(),
        redacted: false,
        thread_display: None,
    });
    Ok(AdminMessage {
        id: row.id,
        org,
        title: row.title.clone(),
        body: row.body.clone(),
        sender: serde_json::from_value(row.sender.clone()).map_err(ApiError::internal)?,
        created_at: Time::new(row.created_at),
        recipient_count: row.recipient_count,
        delivery_job_id: row.delivery_job_id,
    })
}

#[path = "admin_messages/reading.rs"]
pub mod reading;

#[path = "admin_messages/writing.rs"]
pub mod writing;
