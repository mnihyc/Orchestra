//! Creating retries acknowledge native identities without repeating their spent effects.

use super::*;

use crate::middleware::idempotency::Creation;

use orch_db::idempotency::creation::Created;
const CONVERSATION: &str = "ask_conversation";

const KEEP: &str = "ask_kept_thread";

pub(super) fn conversation() -> Creation<(), api::AskCreateBody, api::AskReceipt> {
    Creation {
        kind: CONVERSATION,
        identity: |value| {
            Ok(Created {
                kind: CONVERSATION.into(),
                id: value.conversation_id,
                data: json!({"seq":value.seq}),
            })
        },
        replay: |ctx, (), _body, created| {
            Box::pin(async move {
                let receipt = api::AskReceipt {
                    conversation_id: created.id,
                    turn_id: None,
                    seq: created
                        .data
                        .get("seq")
                        .and_then(Json::as_i64)
                        .ok_or_else(|| {
                            ApiError::internal("The conversation receipt has no native sequence")
                        })?,
                };
                crate::key_operations::publication::receipt(&ctx, &receipt)?;
                Ok(receipt)
            })
        },
    }
}

pub(super) fn keep() -> Creation<(), api::AskKeepBody, api::AskKeepReply> {
    Creation {
        kind: KEEP,
        identity: |value| {
            Ok(Created {
                kind: KEEP.into(),
                id: value.thread_id,
                data: json!({"source":value.conversation_id,"topic":value.topic_id}),
            })
        },
        replay: |ctx, (), _body, created| {
            Box::pin(async move {
                let id = |field: &str| {
                    created
                        .data
                        .get(field)
                        .and_then(Json::as_str)
                        .and_then(|id| id.parse::<Id>().ok())
                        .ok_or_else(|| {
                            ApiError::internal("The kept-thread receipt has no native identity")
                        })
                };
                let receipt = api::AskKeepReply {
                    conversation_id: id("source")?,
                    thread_id: created.id,
                    topic_id: id("topic")?,
                };
                crate::key_operations::publication::receipt(&ctx, &receipt)?;
                Ok(receipt)
            })
        },
    }
}

pub(super) async fn observe_conversation(
    conn: &mut Conn,
    person: Id,
    conversation: Id,
    seq: Seq,
) -> ApiResult<()> {
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(person),
        CONVERSATION,
        conversation,
        json!({"seq":seq}),
    )
    .await?;
    Ok(())
}

pub(super) async fn observe_keep(
    conn: &mut Conn,
    person: Id,
    source: Id,
    thread: Id,
    topic: Id,
) -> ApiResult<()> {
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(person),
        KEEP,
        thread,
        json!({"source":source,"topic":topic}),
    )
    .await?;
    Ok(())
}
