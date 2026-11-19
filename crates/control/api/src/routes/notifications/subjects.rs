//! Native notification subjects, shared with the stored-subject read adapter.

use super::person::{self, Target};
use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::notification_sources::{self as sources, Source};
use orch_api_contract::notifications::PersonNotifyContext;
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Principal;
use orch_db::Conn;

pub(super) struct Subjects;

impl native::Stored for Subjects {
    type Source = Source;
    type Value = Target;
    const ROWS: &'static [Declaration<Source>] = &[sources::PERSON_CONTEXT];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Target, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let subject = person::target(conn, ctx.param("object")?, ctx.id("id")?).await?;
            let (family, target) = match &subject {
                Target::Node(row) => {
                    let target = targets::node(conn, row.id).await?;
                    (target.object.kind, target)
                }
                Target::Document(row) => (
                    KeyTargetKind::Object(ObjectKind::Document),
                    targets::page_object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Document,
                            id: row.id,
                        },
                    )
                    .await?,
                ),
                Target::Chat(row) => (
                    KeyTargetKind::Identity(KeyIdentityKind::Chat),
                    targets::from_chat(conn, row).await?,
                ),
                Target::Resource { row, .. } => {
                    let object = row.object();
                    // Business family comes from the native row, independent of its governor.
                    (
                        KeyTargetKind::Object(object.kind),
                        targets::page_object(conn, object).await?,
                    )
                }
            };
            Ok((
                subject,
                vec![native::Facts::one(Source::Subject, family, target)],
            ))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Target,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if !person::reads(conn, principal, value).await? {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
}

impl native::StoredRead for Subjects {
    type Reply = PersonNotifyContext;

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Target,
    ) -> BoxFuture<'a, ApiResult<PersonNotifyContext>> {
        Box::pin(person::context_for(ctx, conn, principal, value))
    }
}

impl native::StoredQuery for Subjects {
    type Query = ();

    fn load_query<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        declaration: &'static Declaration<Source>,
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Target, Vec<native::Facts<Source>>)>> {
        <Self as native::Stored>::load(ctx, conn, declaration)
    }
}
