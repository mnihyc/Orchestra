//! Native shell composition with a once-only device report outside replayable reads.

use super::account_read;

use crate::key_operations::native;

use crate::key_operations::native::Selected;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::CurrentHttpIdentity;

use crate::middleware::transaction_guard::HttpIdentityMode;

use crate::registry::Answer;

use crate::registry::Ctx;

use crate::registry::OperationInitialFuture;

use crate::routes::chats;

use crate::routes::notifications::inbox;

use crate::routes::personal;

use crate::sign_in::people;

use futures::future::BoxFuture;

use orch_api_contract::account_sources as sources;

use orch_api_contract::account_sources::Source;

use orch_api_contract::auth::AdminMode;

use orch_api_contract::auth::MeAuthorityReply;

use orch_api_contract::auth::MeBootstrapReply;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::users::Me;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::chats::ChatRow;

use orch_db::queries::hierarchy;

use orch_db::queries::identity;

use orch_types::Id;
/// A report is a native bootstrap auxiliary act, never a retried source or renderer.
pub(super) fn initial<'a>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        let Some(zone) = ctx
            .headers
            .get("x-orch-timezone")
            .and_then(|value| value.to_str().ok())
            .filter(|zone| zone.parse::<chrono_tz::Tz>().is_ok())
        else {
            return Ok(());
        };
        let mut tx = ctx.app.db.begin().await?;
        report_admission(ctx, &mut tx, caller).await?;
        let current = identity::lock_device_timezone(&mut tx, caller.user())
            .await?
            .ok_or_else(ApiError::not_found)?;
        // A native preference writer may have held this row. Read the exact HTTP identity,
        // original/current ceiling, standing, second step and Account gate after that wait.
        let (identity, principal) = report_admission(ctx, &mut tx, caller).await?;
        if current.1 == "device" && current.0 != zone {
            identity.check_deadline(&ctx.app)?;
            if !principal
                .caller()
                .key_deadline_allows(ctx.app.clock.as_ref())
            {
                return Err(ApiError::new(
                    orch_api_contract::common::ErrorCode::AuthInvalidToken,
                    "the device report credential has expired",
                ));
            }
            identity::report_device_timezone(&mut tx, principal.user(), zone, ctx.app.clock.now())
                .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    })
}

async fn report_admission(
    ctx: &Ctx,
    conn: &mut Conn,
    accepted: &Caller,
) -> ApiResult<(CurrentHttpIdentity, Principal)> {
    let identity =
        transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read).await?;
    transaction_guard::guard_user(&identity.user)?;
    let caller = identity
        .caller
        .intersect_restrictions(accepted)
        .map_err(ApiError::internal)?;
    let checkpoint = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
    let root = account_read::account_for(&checkpoint, conn, &caller, Source::Account).await?;
    let known = native::bind_known(&checkpoint, &sources::BOOTSTRAP, &[root])?;
    let principal = native::preparation_principal(&checkpoint, conn, &caller, &known).await?;
    identity.check_deadline(&ctx.app)?;
    Ok((identity, principal))
}

pub(super) struct Shell;

pub(super) struct ShellSelection {
    person: Me,
    state: orch_api_contract::personal::PersonalState,
    orgs: account_read::Organizations,
    admin_available: bool,
    favorites: personal::native::FavoriteRows,
    read_states: Vec<(Id, Id, i64)>,
    subscriptions: Vec<hierarchy::SubscriptionRow>,
    chats: Vec<ChatRow>,
}

impl Selected for Shell {
    type Source = Source;
    type Query = ();
    type Value = ShellSelection;
    type Reply = MeBootstrapReply;
    const DECLARATION: &'static Declaration<Source> = &sources::BOOTSTRAP;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(ShellSelection, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let root = account_read::account_for(ctx, conn, caller, Source::Account).await?;
            let orgs =
                account_read::organizations(ctx, conn, caller, Self::DECLARATION, &root).await?;
            let person = identity::user_by_id(conn, caller.user())
                .await?
                .ok_or_else(ApiError::not_found)?;
            let person = people::me(conn, &person, ctx.now).await?;
            let state = identity::interface_state(conn, caller.user())
                .await?
                .into_iter()
                .collect();
            let admin_available =
                identity::admin_mode_available(conn, caller.user(), ctx.now).await?;
            let (favorites, favorite_facts) =
                personal::native::Favorites::select_for(ctx, conn, caller, &(), Self::DECLARATION)
                    .await?;
            let (read_states, read_facts) = personal::native::ReadStates::select_for(
                ctx,
                conn,
                caller,
                &Default::default(),
                Self::DECLARATION,
            )
            .await?;
            let (subscriptions, subscription_facts) = personal::native::Subscriptions::select_for(
                ctx,
                conn,
                caller,
                &Default::default(),
                Self::DECLARATION,
            )
            .await?;
            let (chats, chat_facts) = chats::state_collection::States::select_for(
                ctx,
                conn,
                caller,
                &(),
                Self::DECLARATION,
            )
            .await?;
            let mut facts = vec![root];
            facts.extend(orgs.facts.iter().cloned());
            facts.extend(
                favorite_facts
                    .into_iter()
                    .chain(read_facts)
                    .chain(subscription_facts)
                    .chain(chat_facts)
                    .filter(|fact| fact.source != Source::Account),
            );
            Ok((
                ShellSelection {
                    person,
                    state,
                    orgs,
                    admin_available,
                    favorites,
                    read_states,
                    subscriptions,
                    chats,
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
                return Err(ApiError::internal("the shell observation changed actor"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a ShellSelection,
    ) -> BoxFuture<'a, ApiResult<MeBootstrapReply>> {
        Box::pin(async move {
            let favorites =
                personal::native::Favorites::render(ctx, conn, principal, &value.favorites).await?;
            let read_states =
                personal::native::ReadStates::render(ctx, conn, principal, &value.read_states)
                    .await?;
            let subscriptions =
                personal::native::Subscriptions::render(ctx, conn, principal, &value.subscriptions)
                    .await?;
            let chat_states =
                chats::state_collection::States::render(ctx, conn, principal, &value.chats).await?;
            let notification_summary =
                inbox::summary_for(conn, principal, &ctx.app.engine, None, ctx.now).await?;
            let meta = crate::routes::meta::on_connection(ctx, conn, Some(ctx.user()?)).await?;
            let notification_preferences = inbox::prefs_in(conn, principal).await.ok();
            let language = match value.person.locale {
                orch_api_contract::users::Locale::En => "en",
                orch_api_contract::users::Locale::ZhCn => "zh-CN",
            };
            Ok(MeBootstrapReply {
                meta,
                person: value.person.clone(),
                state: value.state.clone(),
                orgs: value.orgs.rows.clone(),
                admin_mode: AdminMode {
                    as_admin: ctx.user()?.as_admin,
                    available: value.admin_available,
                },
                favorites,
                chat_states,
                read_states,
                subscriptions,
                notification_summary,
                permission_catalogue: Some(crate::routes::access::catalogue_reply()?),
                notifier_catalogue: Some(inbox::notifier_catalogue(language)),
                notification_preferences,
            })
        })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a (),
        old: &'a ShellSelection,
        fresh: &'a ShellSelection,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.person.user.id == fresh.person.user.id
                && (!old.person.is_system_admin || fresh.person.is_system_admin)
                && (!old.admin_available || fresh.admin_available)
                && old
                    .person
                    .second_step_sources
                    .iter()
                    .all(|source| fresh.person.second_step_sources.contains(source))
                && fresh.orgs.preserves(&old.orgs)
                && fresh.favorites.preserves(&old.favorites)
                && old.read_states.iter().all(|(thread, continuation, _)| {
                    fresh
                        .read_states
                        .iter()
                        .any(|(current_thread, current_continuation, _)| {
                            thread == current_thread && continuation == current_continuation
                        })
                })
                && old.subscriptions.iter().all(|row| {
                    fresh
                        .subscriptions
                        .iter()
                        .any(|current| row.node_id == current.node_id)
                })
                && old.chats.iter().all(|row| {
                    fresh.chats.iter().any(|current| {
                        row.id == current.id
                            && row.kind == current.kind
                            && row.node_id == current.node_id
                    })
                }))
        })
    }
}

pub(super) async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<MeBootstrapReply>> {
    let reply = native::selected_read::<Shell>(ctx, (), ()).await?;
    Ok(Answer::from(reply).header(http::header::CACHE_CONTROL, "no-store"))
}

/// Retake the native authority suppliers after admission, without replaying shell preferences.
pub(super) async fn authority(ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<MeAuthorityReply>> {
    let reply =
        crate::key_operations::native::selected_read::<super::account_read::Authority>(ctx, (), ())
            .await?;
    Ok(Answer::from(reply).header(http::header::CACHE_CONTROL, "no-store"))
}
