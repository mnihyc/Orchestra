//! Native notification subjects, shared with the stored-subject read adapter.

use super::person;

use super::person::Target;

use crate::key_operations::native;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use futures::future::BoxFuture;

use orch_api_contract::notification_sources as sources;

use orch_api_contract::notification_sources::Source;

use orch_api_contract::notifications::PersonNotifyContext;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Principal;

use orch_db::Conn;
pub(super) struct Subjects;

impl native::Stored for Subjects {
    type Source = Source;
    type Value = Target;
    const ROWS: &'static [Declaration<Source>] = &[sources::PERSON_CONTEXT, sources::PERSON_CREATE];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Target, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let subject = person::target(conn, ctx.param("object")?, ctx.id("id")?).await?;
            let facts = match orch_worker_core::person_notices::facts(conn, subject.source()?).await
            {
                Ok(facts) => facts,
                Err(orch_authz::key_targets::TargetError::Missing) => {
                    return Err(ApiError::not_found());
                }
                Err(orch_authz::key_targets::TargetError::Db(error)) => return Err(error.into()),
                Err(orch_authz::key_targets::TargetError::Authz(error)) => {
                    return Err(
                        crate::middleware::authorization::refusals::problem(conn, error).await,
                    );
                }
            };
            Ok((subject, facts))
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
