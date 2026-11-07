//! Native stored Machine relabeling and its current optional singleton response.

use super::{
    MachineRow, api, audit_machine, clean_name, live_machine, queries, registered_sources,
    require_machine,
};
use crate::key_operations::native;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use crate::routes::access::operations::NameRoles;
use crate::routes::resources::projection::single;
use futures::future::BoxFuture;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::machine_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::resources::{MachineUpdateReceipt, MachineUpdateResponse, Resource};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError};
use orch_types::Id;
use serde_json::json;

pub(super) struct Machines;

#[derive(Clone)]
pub(super) struct Subject {
    row: MachineRow,
    registered: Vec<ObjectRef>,
}

impl Subject {
    /// The native current selector must still contain every originally returned registration.
    pub(super) fn covers(&self, earlier: &Self) -> bool {
        self.row.id == earlier.row.id
            && earlier.registered.iter().all(|source| {
                self.registered
                    .binary_search_by_key(&(source.kind, source.id), |current| {
                        (current.kind, current.id)
                    })
                    .is_ok()
            })
    }
}

const ROLES: single::Roles = single::Roles {
    names: NameRoles {
        groups: Some(sources::GROUPS),
        nodes: [
            (ObjectKind::Org, sources::NODES[0]),
            (ObjectKind::Project, sources::NODES[1]),
            (ObjectKind::Topic, sources::NODES[2]),
            (ObjectKind::Thread, sources::NODES[3]),
        ],
        containers: None,
    },
    sources: sources::REGISTERED,
};

async fn subject(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Subject> {
    let row = live_machine(conn, ctx.id("id")?).await?;
    let registered = registered_sources(conn, &[row.id]).await?;
    Ok(Subject { row, registered })
}

async fn require(conn: &mut Conn, principal: &Principal, source: &Subject) -> ApiResult<()> {
    require_machine(conn, principal, source.row.id, Permission::MachineView).await?;
    require_machine(conn, principal, source.row.id, Permission::MachineManage).await
}

impl native::Stored for Machines {
    type Source = Source;
    type Value = Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::UPDATE];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = subject(ctx, conn).await?;
            let target = single::subject(
                conn,
                ObjectRef {
                    kind: ObjectKind::Machine,
                    id: value.row.id,
                },
            )
            .await?;
            Ok((
                value,
                vec![native::Facts::one(
                    Source::Governor,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    target,
                )],
            ))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, value))
    }
}

impl native::resource_reply::StoredResource for Machines {
    const READ_SOURCE: Source = Source::Governor;
    const ROLES: &'static single::Roles = &ROLES;

    fn object(value: &Subject) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Machine,
            id: value.row.id,
        }
    }

    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_machine(
            conn,
            principal,
            value.row.id,
            Permission::MachineView,
        ))
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        opened: &'a Subject,
        fresh: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(fresh.covers(opened)) })
    }
}

fn receipt(id: Id) -> MachineUpdateResponse {
    MachineUpdateResponse::Receipt(MachineUpdateReceipt {
        projection: ReceiptProjection::Receipt,
        machine_id: id,
    })
}

pub(super) async fn update(
    ctx: Ctx,
    (): (),
    body: api::MachineUpdateBody,
) -> ApiResult<MachineUpdateResponse> {
    let name = clean_name("name", &body.name)?;
    let id = ctx.id("id")?;
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let original = subject(&ctx, &mut tx).await?;
    require(&mut tx, initial.principal(), &original).await?;
    if !orch_db::queries::resources::lock_resource(&mut tx, "machine", id).await? {
        return Err(ApiError::not_found());
    }
    // The physical row's existing foreign keys serialize a new facade; hold any current facade too.
    sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 FOR SHARE")
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let current = if native::modern(guard.principal().caller()) {
        guard
            .operation_data::<native::Prepared<Subject>>()?
            .value
            .clone()
    } else {
        subject(&ctx, &mut tx).await?
    };
    require(&mut tx, guard.principal(), &current).await?;
    guard.check_deadline(&ctx)?;
    queries::rename_machine(&mut tx, current.row.id, &name).await?;
    audit_machine(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MachineRenamed),
        id,
        &format!(
            "{} renamed the machine {} to {name}",
            crate::routes::nodes::actor_name(&ctx)?,
            current.row.name
        ),
        json!({"machine":id,"from":current.row.name,"to":name}),
    )
    .await?;
    guard.check_deadline(&ctx)?;
    let accepted: Caller = guard.principal().caller().clone();
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&ctx, &receipt(id))?;
    Ok(
        match native::resource_reply::stored::<Machines>(&ctx, &accepted).await? {
            Some(Resource::Machine(machine)) => MachineUpdateResponse::Full(Box::new(machine)),
            None => receipt(id),
            Some(_) => {
                return Err(ApiError::internal(
                    "the native Machine response has another resource kind",
                ));
            }
        },
    )
}
