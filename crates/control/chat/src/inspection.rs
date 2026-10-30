//! Read-only access to the direct and group chats among an org's people, for whoever holds that row on the org: audited, and announced to nobody.
//!
//! An admin of an org reads the direct and group chats among that org's people
//! as an outsider to them. Nobody is told, and nothing is shown in the chat:
//! no notification, no message, no presence. The one trace is an audit row,
//! once per logical opening, shared by HTTP and its live subscription. An outsider
//! has no state in the chat, no draft and no read position, and cannot write.

use crate::chats::Loaded;
use crate::{Acting, Error};
use orch_authz::chats::Reading;
use orch_db::Conn;
use orch_db::audit;
use serde_json::json;

/// Notes that the caller opened a chat they are not in, if they did: an audit
/// row `chat.inspected`, once for this opening. Says
/// whether a row was written. A caller who is in the chat, or who reads the
/// chat of a node by its page, leaves nothing.
pub async fn note_opened(
    conn: &mut Conn,
    act: &Acting<'_>,
    loaded: &Loaded,
) -> Result<bool, Error> {
    let place = match loaded.standing.admin_access.map(|reach| reach.place) {
        Some(orch_access_contract::roles::PlaceRef::System) => json!("system"),
        Some(orch_access_contract::roles::PlaceRef::Node(id)) => json!({"id":id}),
        None => match loaded.standing.reading {
            Some(Reading::Inspector { org }) => json!({"id":org}),
            _ => return Ok(false),
        },
    };
    let (me, chat) = (act.user(), &loaded.row);
    let session = audit::opening_session().unwrap_or(me);
    let node = place
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| id.parse().ok());
    Ok(audit::opened(
        conn,
        audit::Opening {
            user: me,
            session,
            nonce: audit::current_opening(),
            kind: "chat",
            object: chat.id,
            place,
            node,
        },
        act.origin,
        act.now,
    )
    .await?)
}
