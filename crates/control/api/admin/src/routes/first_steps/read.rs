//! Native stored setup selection and declaration-bound observations.

use super::*;

use crate::key_operations::native;

use futures::future::BoxFuture;

use orch_api_contract::account_sources as sources;

use orch_api_contract::account_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

pub use orch_api_base::routes::first_steps::read::NativeTarget;

pub use orch_api_base::routes::first_steps::read::Scope;

pub use orch_api_base::routes::first_steps::read::start_document;

pub use orch_api_base::routes::first_steps::read::observe;

pub use orch_api_base::routes::first_steps::read::for_overview;
pub(super) struct Setup;

pub(super) struct Selection {
    reply: FirstSteps,
    authority: Option<orch_authz::nodes::Effective>,
    document: Option<orch_authz::documents::Listed>,
}

impl native::Selected for Setup {
    type Source = Source;
    type Query = FirstStepsGetQuery;
    type Value = Selection;
    type Reply = FirstSteps;
    const DECLARATION: &'static Declaration<Source> = &sources::FIRST_STEPS;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a FirstStepsGetQuery,
    ) -> BoxFuture<'a, ApiResult<(Selection, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let root = crate::routes::auth::account_read::account(ctx, conn, caller).await?;
            let plan = native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&root))?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let mut scope = Scope {
                ctx,
                caller,
                plan: Some(plan),
                overview: None,
                initial: None,
                outside: None,
                facts: vec![root],
            };
            let reply = observe(ctx, conn, &principal, query.clone(), &mut scope).await?;
            let observed_principal = scope.principal(conn).await?;
            let authority = if let Some(org) = &reply.org {
                Some(
                    orch_authz::nodes::effective(
                        conn,
                        &observed_principal,
                        org.id,
                        Policy::default(),
                    )
                    .await
                    .map_err(ApiError::internal)?,
                )
            } else {
                None
            };
            let document = if let Some(document) = &reply.start_here {
                orch_authz::documents::listed(
                    conn,
                    &observed_principal,
                    &[document.id],
                    Policy::default(),
                )
                .await
                .map_err(ApiError::internal)?
                .pop()
            } else {
                None
            };
            Ok((
                Selection {
                    reply,
                    authority,
                    document,
                },
                scope.facts,
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
                return Err(ApiError::internal("the setup observation changed actor"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<FirstSteps>> {
        Box::pin(async move { Ok(value.reply.clone()) })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a FirstStepsGetQuery,
        old: &'a Selection,
        fresh: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let org = match (&old.authority, &fresh.authority) {
                (Some(old), Some(fresh)) => {
                    crate::routes::profiles::node_authority_preserved(old, fresh)
                }
                (None, _) => true,
                _ => false,
            };
            let doc = match (&old.document, &fresh.document) {
                (Some(old), Some(fresh)) => {
                    old.document.id == fresh.document.id
                        && old.document.node == fresh.document.node
                        && fresh.holds(Permission::DocView)
                        && old.admin_place == fresh.admin_place
                        && old.admin_access == fresh.admin_access
                }
                (None, _) => true,
                _ => false,
            };
            Ok(org
                && doc
                && old.reply.org == fresh.reply.org
                && old.reply.work_mode == fresh.reply.work_mode
                && old.reply.steps == fresh.reply.steps
                && old.reply.invitation_roles == fresh.reply.invitation_roles)
        })
    }
}
