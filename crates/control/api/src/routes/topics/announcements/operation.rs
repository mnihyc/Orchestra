//! One native audience producer, prepared effect and immutable completed result projection.

use super::{problem, request_of};
use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::topic_sources::{self as sources, Source};
use orch_api_contract::topics::{
    AnnouncementCreateBody, AnnouncementReceipt, PlaceAnnouncementCreateReceipt,
    PlaceAnnouncementCreateReply, PlaceAnnouncementCreateResponse,
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, Tx, queries::threads::tools::AnnouncementRow};
use orch_engine::tools::announce::{PlaceSent, PreparedPersonAnnouncement};
use orch_types::Id;
use std::collections::BTreeMap;

pub(super) struct Create;
pub(super) struct Sending {
    native: PreparedPersonAnnouncement,
    destinations: Vec<(ObjectKind, Id)>,
}
pub(super) struct Sent {
    native: PlaceSent,
    destinations: Vec<(ObjectKind, Id)>,
    actor: Id,
}

async fn facts(
    conn: &mut Conn,
    source: Source,
    destinations: &[(ObjectKind, Id)],
) -> ApiResult<native::Facts<Source>> {
    let ids = destinations.iter().map(|(_, id)| *id).collect::<Vec<_>>();
    let graph = orch_authz::engine::Graph::load(conn, &ids)
        .await
        .map_err(ApiError::internal)?;
    let mut groups = BTreeMap::new();
    for (kind, id) in destinations {
        let site = graph.site_of(*id).ok_or_else(ApiError::not_found)?;
        let target = targets::from_site(&graph, &site)?;
        groups
            .entry(KeyTargetKind::Object(*kind))
            .or_insert_with(Vec::new)
            .push(target);
    }
    Ok(native::Facts::collection(
        source,
        groups.into_iter().collect(),
    ))
}

impl native::effects::Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = AnnouncementCreateBody;
    type Value = Sending;
    type Output = Sent;

    fn declaration(_value: &Sending) -> &'static Declaration<Source> {
        &sources::ANNOUNCEMENT
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: AnnouncementCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Sending, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let known = native::bind_known(ctx, &sources::ANNOUNCEMENT, &[])?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let native = match ctx
                .app
                .engine
                .prepare_person_announcement_in(conn, &principal, ctx.id("id")?, &request_of(body))
                .await
            {
                Ok(native) => native,
                Err(error) => return Err(problem(ctx, conn, error).await),
            };
            let destinations = native.destinations().collect::<Vec<_>>();
            let source = facts(conn, Source::Destinations, &destinations).await?;
            Ok((
                Sending {
                    native,
                    destinations,
                },
                vec![source],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Sending,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .require_prepared_person_announcement(conn, principal, &value.native)
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(problem(ctx, conn, error).await),
            }
        })
    }
}

impl native::effects::ApplyOwned for Create {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: Tx,
        principal: &'a Principal,
        value: &'a Sending,
        before: native::effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<native::effects::Outcome<Sent>>> {
        Box::pin(async move {
            let native = match ctx
                .app
                .engine
                .announce_prepared_place_guarded(tx, principal.clone(), &value.native, move || {
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                })
                .await
            {
                Ok(native) => native,
                Err(error) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    return Err(problem(ctx, &mut conn, error).await);
                }
            };
            let changed = !native.sent.is_empty();
            let output = Sent {
                native,
                destinations: value.destinations.clone(),
                actor: principal.user(),
            };
            Ok(if changed {
                native::effects::Outcome::Applied(output)
            } else {
                native::effects::Outcome::Unchanged(output)
            })
        })
    }
}

pub(super) async fn create(
    ctx: Ctx,
    (): (),
    _body: AnnouncementCreateBody,
) -> ApiResult<PlaceAnnouncementCreateResponse> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let done = native::effects::apply_owned::<Create>(&ctx, tx).await?;
    let receipt = PlaceAnnouncementCreateResponse::Receipt(PlaceAnnouncementCreateReceipt {
        projection: ReceiptProjection::Receipt,
        group_id: done.output.native.group_id,
        topics_skipped: done.output.native.skipped,
        announcements: done
            .output
            .native
            .sent
            .iter()
            .map(|sent| AnnouncementReceipt {
                id: sent.row.id,
                topic_id: sent.row.topic_id,
                delivered: sent.delivered,
            })
            .collect(),
    });
    crate::key_operations::publication::receipt(&done.ctx, &receipt)?;
    Ok(
        native::reply::render::<Content>(&done.ctx, &done.accepted, done.output)
            .await?
            .map_or(receipt, PlaceAnnouncementCreateResponse::full),
    )
}

struct Content;
impl native::reply::Projection for Content {
    type Source = Source;
    type Input = Sent;
    type Subject = Vec<AnnouncementRow>;
    type Value = Vec<AnnouncementRow>;
    type Reply = PlaceAnnouncementCreateReply;
    const READ_SOURCE: Source = Source::Content;

    fn declaration(_input: &Sent) -> &'static Declaration<Source> {
        &sources::ANNOUNCEMENT
    }

    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Sent,
    ) -> BoxFuture<'a, ApiResult<(Vec<AnnouncementRow>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let required = facts(conn, Source::Destinations, &input.destinations).await?;
            let mut rows = Vec::with_capacity(input.native.sent.len());
            for expected in &input.native.sent {
                let row = orch_db::queries::threads::tools::announcement(conn, expected.row.id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if row != expected.row
                    || row.group_id != Some(input.native.group_id)
                    || row.from_user_id != Some(input.actor)
                {
                    return Err(ApiError::not_found());
                }
                rows.push(row);
            }
            // The body contains Topic announcements and generated counts. It contains no
            // existing recipient Thread body, so those effect targets do not lend content Read.
            let topics = rows
                .iter()
                .map(|row| (ObjectKind::Topic, row.topic_id))
                .collect::<Vec<_>>();
            let content = facts(conn, Source::Content, &topics).await?;
            Ok((rows, vec![required, content]))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Sent,
        rows: &'a Vec<AnnouncementRow>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            // The native create response returns this person's own exact accepted rows. It
            // never required TopicView in addition to the native act, nor another completed
            // Interact check after commit. The same-operation Read gate still precedes rendering.
            if principal.user() != input.actor
                || rows.iter().any(|row| row.from_user_id != Some(input.actor))
            {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }

    fn prepare<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Sent,
        rows: &'a Vec<AnnouncementRow>,
    ) -> BoxFuture<'a, ApiResult<(Vec<AnnouncementRow>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { Ok((rows.clone(), Vec::new())) })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Sent,
        prepared: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<PlaceAnnouncementCreateReply>> {
        Box::pin(async move {
            if prepared.value.len() != input.native.sent.len() {
                return Err(ApiError::not_found());
            }
            let mut announcements = Vec::with_capacity(prepared.value.len());
            for (row, sent) in prepared.value.iter().zip(&input.native.sent) {
                if row.id != sent.row.id {
                    return Err(ApiError::not_found());
                }
                announcements.push(
                    orch_engine::tools::announce::announcement_view(
                        conn,
                        row,
                        Some(sent.delivered),
                    )
                    .await
                    .map_err(ApiError::engine)?,
                );
            }
            Ok(PlaceAnnouncementCreateReply {
                group_id: input.native.group_id,
                announcements,
                topics_skipped: input.native.skipped,
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Sent,
        opened: &'a native::reply::Prepared<Self>,
        fresh: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.subject == fresh.subject && opened.value == fresh.value) })
    }
}
