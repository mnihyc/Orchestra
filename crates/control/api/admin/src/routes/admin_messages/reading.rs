//! Received messages and bounded sent lists keep their actual scope and native reader gate.

use super::*;

use base64::Engine as _;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use futures::future::BoxFuture;

use orch_api_contract::common::Page;

use orch_api_contract::common::PageQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use serde::Deserialize;

use serde::Serialize;

use std::marker::PhantomData;
async fn facts(conn: &mut Conn, org: Option<Id>) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = scope_target(conn, org).await?;
    Ok(vec![native::Facts::one(
        Source::Scope,
        target.object.kind,
        target,
    )])
}

async fn message(ctx: &Ctx, conn: &mut Conn) -> ApiResult<MessageRow> {
    rows::get(conn, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub(super) struct Message;

impl native::Selected for Message {
    type Source = Source;
    type Query = ();
    type Value = MessageRow;
    type Reply = AdminMessage;
    const MOVES: native::Moves = native::Moves::Repeat;
    const DECLARATION: &'static Declaration<Source> = &sources::GET;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(MessageRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = message(ctx, conn).await?;
            let facts = facts(conn, row.org_id).await?;
            Ok((row, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let row = message(ctx, conn).await?;
            // The accepted audience keeps this message even after membership changes.
            // A person outside it must still hold the native sending authority today.
            require_reader(ctx, conn, principal, &row).await
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        row: &'a MessageRow,
    ) -> BoxFuture<'a, ApiResult<AdminMessage>> {
        Box::pin(async move { view(row) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a (),
        opened: &'a MessageRow,
        fresh: &'a MessageRow,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}

pub(super) trait Scope: Send + Sync + 'static {
    const ROW: &'static Declaration<Source>;
    fn org(ctx: &Ctx) -> ApiResult<Option<Id>>;
}

pub(super) struct Org;

impl Scope for Org {
    const ROW: &'static Declaration<Source> = &sources::ORG_LIST;
    fn org(ctx: &Ctx) -> ApiResult<Option<Id>> {
        Ok(Some(ctx.id("id")?))
    }
}

pub(super) struct System;

impl Scope for System {
    const ROW: &'static Declaration<Source> = &sources::SYSTEM_LIST;
    fn org(_: &Ctx) -> ApiResult<Option<Id>> {
        Ok(None)
    }
}

pub(super) struct Sent<S>(PhantomData<S>);

pub(super) struct SentPage {
    org: Option<Id>,
    rows: Vec<rows::SummaryRow>,
    next_cursor: Option<String>,
    audience_count: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    at: i64,
    id: Id,
}

fn audience(ctx: &Ctx, org: Option<Id>) -> ApiResult<Vec<u8>> {
    let mut purpose = b"administrator-messages/sent/v1/".to_vec();
    purpose.extend_from_slice(ctx.user()?.id().as_bytes());
    purpose.push(u8::from(org.is_some()));
    if let Some(org) = org {
        purpose.extend_from_slice(org.as_bytes());
    }
    Ok(purpose)
}

fn open(ctx: &Ctx, org: Option<Id>, text: &str) -> ApiResult<(chrono::DateTime<chrono::Utc>, Id)> {
    let bad = || ApiError::invalid("cursor", "is not a cursor of this sent list");
    if text.len() > orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES {
        return Err(bad());
    }
    let sealed = URL_SAFE_NO_PAD.decode(text).map_err(|_| bad())?;
    let plain = crate::routes::resources::sealing::installed_master_key()?
        .open(&sealed, &audience(ctx, org)?)
        .map_err(|_| bad())?;
    let pos: Position = serde_json::from_slice(&plain).map_err(|_| bad())?;
    Ok((
        chrono::DateTime::from_timestamp_micros(pos.at).ok_or_else(bad)?,
        pos.id,
    ))
}

fn seal(ctx: &Ctx, org: Option<Id>, row: &rows::SummaryRow) -> ApiResult<String> {
    let plain = serde_json::to_vec(&Position {
        at: row.created_at.timestamp_micros(),
        id: row.id,
    })
    .map_err(ApiError::internal)?;
    let sealed = crate::routes::resources::sealing::installed_master_key()?
        .seal(&plain, &audience(ctx, org)?)
        .map_err(ApiError::internal)?;
    Ok(URL_SAFE_NO_PAD.encode(sealed))
}

fn summary(row: &rows::SummaryRow) -> ApiResult<AdminMessageSummary> {
    Ok(AdminMessageSummary {
        id: row.id,
        org: row.org_id.map(|id| NodeRef {
            id,
            kind: NodeKind::Org,
            name: row.org_name.clone(),
            redacted: false,
            thread_display: None,
        }),
        title: row.title.clone(),
        sender: serde_json::from_value(row.sender.clone()).map_err(ApiError::internal)?,
        created_at: Time::new(row.created_at),
        recipient_count: row.recipient_count,
        delivery_job_id: row.delivery_job_id,
    })
}

impl<S: Scope> native::Selected for Sent<S> {
    type Source = Source;
    type Query = PageQuery;
    type Value = SentPage;
    type Reply = AdminMessageListResponse;
    const MOVES: native::Moves = native::Moves::Repeat;
    const DECLARATION: &'static Declaration<Source> = S::ROW;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(SentPage, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let org = S::org(ctx)?;
            let after = query
                .cursor
                .as_deref()
                .map(|text| open(ctx, org, text))
                .transpose()?;
            let limit = query.limit() as usize;
            let mut selected =
                rows::sent_page(conn, org, after, i64::from(query.limit()) + 1).await?;
            let more = selected.len() > limit;
            selected.truncate(limit);
            let next_cursor = if more {
                selected.last().map(|row| seal(ctx, org, row)).transpose()?
            } else {
                None
            };
            let audience_count = rows::audience_count(conn, org, ctx.user()?.id()).await?;
            Ok((
                SentPage {
                    org,
                    rows: selected,
                    next_cursor,
                    audience_count,
                },
                facts(conn, org).await?,
            ))
        })
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        _: &'a PageQuery,
        opened: &'a SentPage,
    ) -> BoxFuture<'a, ApiResult<(SentPage, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let org = S::org(ctx)?;
            let ids = opened.rows.iter().map(|row| row.id).collect::<Vec<_>>();
            let rows = rows::sent_by_ids(conn, org, &ids).await?;
            Ok((
                SentPage {
                    org,
                    rows,
                    next_cursor: opened.next_cursor.clone(),
                    audience_count: opened.audience_count,
                },
                facts(conn, org).await?,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require_sender(ctx, conn, principal, S::org(ctx)?).await })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a SentPage,
    ) -> BoxFuture<'a, ApiResult<AdminMessageListResponse>> {
        Box::pin(async move {
            Ok(AdminMessageListResponse {
                page: Page {
                    items: value.rows.iter().map(summary).collect::<ApiResult<_>>()?,
                    next_cursor: value.next_cursor.clone(),
                },
                audience_count: value.audience_count,
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a PageQuery,
        opened: &'a SentPage,
        fresh: &'a SentPage,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.org == fresh.org && opened.rows == fresh.rows) })
    }
}
