//! Completed join questions read the same real batch without another request or membership act.

use super::*;

use crate::key_operations::admission::ResponseProjection;

use crate::key_operations::publication;

use crate::middleware::idempotency::Creation;

use crate::middleware::transaction_guard::HttpIdentityMode;

use crate::middleware::transaction_guard::current_http_identity;

use orch_api_contract::operations::FacetUsage;

use orch_db::idempotency::creation::Created;
pub(crate) fn join_creation() -> Creation<(), ChatJoinRequestBody, AccessRequestWriteResponse> {
    Creation {
        kind: "chat_join_request",
        replay: |ctx, (), _, created| Box::pin(join(ctx, created)),
        identity: |value| {
            Ok(Created {
                kind: "chat_join_request".into(),
                id: match value {
                    AccessRequestWriteResponse::Full(value) => value.batch_id,
                    AccessRequestWriteResponse::Receipt(value) => value.batch_id,
                },
                data: serde_json::Value::Null,
            })
        },
    }
}

pub(super) struct Identity {
    batch: Id,
    /// The chat a join request names; none for a batch replayed through its creation key.
    chat: Option<Id>,
    approvals: Vec<Id>,
}

impl Identity {
    /// A request batch replayed through its creation key, whatever it asked for.
    pub(super) fn created(batch: Id, approvals: Vec<Id>) -> Self {
        Self {
            batch,
            chat: None,
            approvals,
        }
    }
    pub(super) fn matches(&self, rows: &[ApprovalRow]) -> bool {
        let mut actual = rows.iter().map(|row| row.id).collect::<Vec<_>>();
        actual.sort_unstable();
        let mut expected = self.approvals.clone();
        expected.sort_unstable();
        !rows.is_empty()
            && actual == expected
            && rows.iter().all(|row| {
                row.batch_id == Some(self.batch)
                    && self.chat.is_none_or(|chat| {
                        orch_engine::membership_requests::is_join(row)
                            && row.object_id == Some(chat)
                    })
            })
    }
}

async fn join(ctx: Ctx, created: Created) -> ApiResult<AccessRequestWriteResponse> {
    let chat: Id =
        serde_json::from_value(created.data["chat"].clone()).map_err(ApiError::internal)?;
    let ids: Vec<Id> =
        serde_json::from_value(created.data["approvals"].clone()).map_err(ApiError::internal)?;
    if ctx.id("id")? != chat {
        return Err(ApiError::not_found());
    }
    let receipt = AccessRequestWriteResponse::Receipt(AccessRequestReceipt {
        projection: ReceiptProjection::Receipt,
        batch_id: created.id,
        approval_ids: ids.clone(),
    });
    publication::receipt(&ctx, &receipt)?;
    let accepted = {
        let mut conn = ctx.app.db.acquire().await?;
        let current = current_http_identity(&ctx, &mut conn, HttpIdentityMode::Read).await?;
        current.check_deadline(&ctx.app)?;
        ctx.publication.retain_ceiling(&current.caller)?;
        current.caller
    };
    let full = crate::key_operations::completed_projection::run(
        &ctx,
        created_projection(
            ctx.clone(),
            accepted,
            created.id,
            Some(Identity {
                batch: created.id,
                chat: Some(chat),
                approvals: ids,
            }),
        ),
    )
    .await;
    match full {
        Ok(Some(value)) => Ok(AccessRequestWriteResponse::full(value)),
        Ok(None) => Ok(receipt),
        Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
            Ok(receipt)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn projection(plan: &OperationPlan) -> ResponseProjection {
    ResponseProjection::Replay(
        plan.variant()
            .facets
            .iter()
            .enumerate()
            .filter_map(|(index, facet)| {
                (facet.usage == FacetUsage::ResponseContent).then_some(index)
            })
            .collect(),
    )
}

pub(super) struct ReadSource {
    identity: Identity,
    rows: Vec<ApprovalRow>,
    sources: approvals::CardSources,
    pending: Vec<bool>,
}

impl ReadSource {
    pub(super) fn new(
        identity: Identity,
        rows: Vec<ApprovalRow>,
        sources: approvals::CardSources,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        let pending = rows
            .iter()
            .map(|row| row.state == "pending" && row.expires_at.is_none_or(|end| end > at))
            .collect();
        Self {
            identity,
            rows,
            sources,
            pending,
        }
    }
}

pub(super) fn retain(ctx: &Ctx, accepted: &Caller, source: ReadSource) -> ApiResult<()> {
    publication::retain(ctx, accepted, source, |ctx, conn, principal, source| {
        Box::pin(check(ctx, conn, principal, source))
    })
}

async fn check(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    source: &ReadSource,
) -> ApiResult<()> {
    let ctx = ctx.for_completed_replay_projection();
    let actual = batch(conn, source.identity.batch, principal.user()).await?;
    if actual != source.rows
        || !source.identity.matches(&actual)
        || actual
            .iter()
            .map(|row| {
                row.state == "pending" && row.expires_at.is_none_or(|end| end > ctx.app.clock.now())
            })
            .collect::<Vec<_>>()
            != source.pending
    {
        return Err(ApiError::not_found());
    }
    let selected = governors(conn, &actual).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    bind(&mut plan, "request_batch", APPROVAL, &selected)?;
    let Some((current, reader, identity)) =
        reader(&ctx, conn, principal.caller(), &plan, &selected).await?
    else {
        return Err(denied());
    };
    approvals::bind_prepared_card_sources(conn, &mut plan, &source.sources).await?;
    let purpose = projection(&plan);
    let checking = current.clone();
    let node_ids = source
        .sources
        .objects()
        .map(|(kind, id)| {
            if kind.node().is_some() {
                Ok(id)
            } else {
                Err(ApiError::internal(
                    "the native join card has a non-node name",
                ))
            }
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let chats = source.sources.chats().collect::<Vec<_>>();
    plan.read_response_projection(
        &current,
        conn,
        reader.caller(),
        purpose,
        move |conn, reader| {
            Box::pin(async move {
                let visible = crate::routes::nodes::view::reference_visibility(
                    conn,
                    reader,
                    &node_ids,
                    orch_authz::Policy::default(),
                )
                .await?;
                if node_ids.iter().any(|id| !visible.contains(id)) {
                    return Err(ApiError::not_found());
                }
                for chat in chats {
                    let origin = orch_db::audit::Origin::default();
                    let acting = orch_chat::Acting {
                        principal: reader,
                        now: reader.now().get(),
                        origin: &origin,
                    };
                    orch_chat::chats::load(conn, &acting, chat, orch_chat::chats::Mode::Read)
                        .await
                        .map_err(|error| match error {
                            orch_chat::Error::Db(error) => ApiError::internal(error),
                            _ => ApiError::not_found(),
                        })?;
                }
                checking.publication.retain_ceiling(reader.caller())?;
                Ok(())
            })
        },
    )
    .await?
    .ok_or_else(denied)?;
    identity.check_deadline(&ctx.app)
}
