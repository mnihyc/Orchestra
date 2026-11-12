//! Actual accepted sharing identities on the native writer's own transaction.
use super::*;
use orch_api_contract::{
    common::ReceiptProjection,
    nodes::NodeKind,
    share::{AgentShareReceipt, ShareAgentCreateReceipt, ShareReceiptRecipient},
};

/// The shared header journal records only this native aggregate's accepted identities.
pub const KIND: &str = "agent_shares";

/// The actual native writer supplies the logical source once for its whole accepted aggregate.
pub fn subject(records: &[Record]) -> Result<Subject, EngineError> {
    let subject = records
        .first()
        .map(|record| record.subject)
        .ok_or_else(|| EngineError::unreadable("the accepted share has no native identity"))?;
    if records.iter().any(|record| record.subject != subject) {
        return Err(EngineError::unreadable(
            "the accepted share mixed native sources",
        ));
    }
    Ok(subject)
}

/// Node chats use their actual node permission page for live entries. Reconstruct only
/// that genuine logical Chat reference, preserving the native producer's response style.
pub async fn matches_subject(
    conn: &mut Conn,
    record: &mut Record,
    expected: Subject,
) -> Result<bool, orch_db::DbError> {
    if record.subject == expected {
        return Ok(true);
    }
    if record.mode == ShareMode::Live
        && let (Subject::Node(node), Subject::Chat(chat)) = (record.subject, expected)
        && orch_db::queries::chats::chat(conn, chat)
            .await?
            .is_some_and(|row| row.node_id == Some(node))
    {
        record.subject = expected;
        return Ok(true);
    }
    Ok(false)
}

/// Factual accepted recipients, shared by the native observer and the HTTP acknowledgement.
pub fn receipt(
    records: &[Record],
    recipients: &[(Id, ObjectKind)],
) -> Result<ShareAgentCreateReceipt, EngineError> {
    let mut shares = Vec::with_capacity(records.len());
    for record in records {
        let kind = recipients
            .iter()
            .find(|(id, _)| *id == record.to)
            .map(|(_, kind)| *kind)
            .ok_or_else(|| {
                EngineError::unreadable("an accepted share lost its native recipient")
            })?;
        let kind = match kind {
            ObjectKind::Org => NodeKind::Org,
            ObjectKind::Project => NodeKind::Project,
            ObjectKind::Topic => NodeKind::Topic,
            ObjectKind::Thread => NodeKind::Thread,
            _ => {
                return Err(EngineError::unreadable(
                    "an accepted share has an invalid recipient kind",
                ));
            }
        };
        shares.push(AgentShareReceipt {
            id: record.id,
            to: ShareReceiptRecipient {
                id: record.to,
                kind,
            },
            mode: record.mode,
            created: record.fresh,
            input_id: record.input,
        });
    }
    Ok(ShareAgentCreateReceipt {
        projection: ReceiptProjection::Receipt,
        shares,
    })
}
pub(super) async fn observe(
    conn: &mut Conn,
    caller: &Caller,
    prepared: &PreparedAgentShare,
    records: &[Record],
) -> Result<(), ShareError> {
    if !orch_db::idempotency::creation::captures(Some(caller.user()), KIND) {
        return Ok(());
    }
    let selection = caller
        .admitted_operation()
        .filter(|selection| selection.starts_with("ShareAgentCreate."))
        .ok_or_else(|| EngineError::unreadable("an accepted share lost its creating operation"))?;
    let receipt = receipt(records, &prepared.recipients)?;
    let first = records
        .first()
        .ok_or_else(|| EngineError::unreadable("an accepted share has no native identity"))?;
    let actual_subject = subject(records)?;
    orch_db::idempotency::creation::observe_with_status(
        conn,
        Some(caller.user()),
        KIND,
        first.id,
        json!({"selection":selection,"subject":actual_subject,"shares":receipt.shares}),
        if records.iter().any(|record| record.fresh) {
            201
        } else {
            200
        },
    )
    .await?;
    Ok(())
}
