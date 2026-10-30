//! The user feed: the rows of a person's Inbox by its sequence, and the news that is no row.
//!
//! The feed is the caller's own, so anybody who is signed in may follow it. Its
//! sequence is the number their Inbox has reached; a subscriber is sent the
//! notifications numbered after the one they hold. What is not a row (a
//! favourite changed on another of their devices, an approval that waits for
//! them) arrives as `event` frames, which are not kept: after every connect the
//! client loads that state again.
//!
//! The two counters of the Inbox travel with the rows: how many notifications
//! are unread, and how many approvals the person can decide wait. The second
//! is decided by who may decide each approval, so it is asked of the access
//! engine as the request for the summary asks it. A change that is no row of
//! the Inbox (an approval was decided by somebody else) moves the second
//! counter without a row of its own: the `approvals` event carries the native
//! summary projected for this browser session after the decision commits.

use crate::realtime::subscriptions::principal_of;
use crate::realtime::sync::{Frame, Head, Rows};
use crate::realtime::{Fail, Inner};
use crate::routes::nodes::view;
use crate::routes::notifications::inbox;
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::notifications::{Notification, NotificationKind};
use orch_api_contract::users::UserRef;
use orch_authz::Policy;
use orch_db::queries::feeds::{self, InboxRow};
use orch_realtime_contract::feeds::{SyncData, UserData, UserDataSummary};
use orch_realtime_contract::foreign;
use orch_realtime_contract::sync::FeedHead;
use orch_types::{Id, Json, Seq, Time};
use serde_json::json;
use std::collections::HashMap;

/// The most Inbox rows a person is sent from a number. A person who was away
/// while more arrived loads their Inbox again: it is paged, and a page is
/// about as many as this.
const MOST_ROWS: i64 = 200;

/// Where the person's feed stands, and how many rows lie above `since`.
pub(crate) async fn head(
    inner: &Inner,
    person: Id,
    since: Option<Seq>,
) -> Result<Option<Head>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some((_, seq)) = feeds::inbox_seqs(&mut conn, &[person])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let rows_after = match since {
        Some(since) if since <= seq => feeds::inbox_after_count(&mut conn, person, since).await?,
        _ => 0,
    };
    Ok(Some(Head {
        feed: FeedHead {
            seq,
            purged_seq: 0,
            incremental_limit: MOST_ROWS,
        },
        rows_after,
    }))
}

/// The in-app path that opens a notification: the approval it is about, unless
/// its kind named a more particular path in its data; else that path; else its
/// thread; else the project, topic or thread it happened at; else the Inbox.
/// The messages sent by mail and push name the same paths.
#[must_use]
pub fn open_path(row: &InboxRow, node: Option<&NodeRef>) -> String {
    let supplied = row
        .data
        .get("path")
        .and_then(Json::as_str)
        .filter(|path| path.starts_with('/') && !path.starts_with("//"));
    if let Some(approval) = row.approval_id
        && supplied.is_none_or(|path| matches!(path, "/approvals" | "/approvals/"))
    {
        return format!("/approvals?approval={approval}");
    }
    if let Some(path) = supplied {
        return path.to_owned();
    }
    if let Some(thread) = row.thread_id {
        return format!("/thread/{thread}");
    }
    if let Some(node) = node {
        match node.kind {
            NodeKind::Project => return format!("/project/{}", node.id),
            NodeKind::Topic => return format!("/topic/{}", node.id),
            NodeKind::Thread => return format!("/thread/{}", node.id),
            NodeKind::Org => {}
        }
    }
    "/inbox".to_owned()
}

/// A stored Inbox row as the API shows it: `paths` names its current thread and places
/// as the reader may know them, without depending on which navigation trees a browser loaded.
#[must_use]
pub fn notification_view(
    row: &InboxRow,
    paths: &HashMap<Id, Vec<NodeRef>>,
    actors: &HashMap<Id, UserRef>,
) -> Option<Notification> {
    let kind: NotificationKind = serde_json::from_value(Json::String(row.kind.clone())).ok()?;
    let reference = |id: Id| {
        paths
            .get(&id)
            .and_then(|path| path.last())
            .filter(|node| node.id == id)
            .cloned()
    };
    let node = row.node_id.and_then(reference);
    let thread = row
        .thread_id
        .and_then(reference)
        .filter(|node| node.kind == NodeKind::Thread);
    let hide_thread_content = row.thread_id.is_some()
        && orch_authz::documents::notice_contains_thread_content(&row.kind)
        && thread.as_ref().is_none_or(|node| node.redacted);
    let path = row
        .thread_id
        .and_then(|id| paths.get(&id))
        .or_else(|| row.node_id.and_then(|id| paths.get(&id)))
        .cloned()
        .unwrap_or_default();
    let url = if hide_thread_content {
        "/inbox".to_owned()
    } else {
        open_path(row, node.as_ref())
    };
    let data = if hide_thread_content {
        orch_db::queries::notifications::people::notification_data(&row.kind, &json!({}))
    } else {
        row.data.clone()
    };
    Some(Notification {
        id: row.id,
        kind,
        title: if hide_thread_content {
            String::new()
        } else {
            row.title.clone()
        },
        body: if hide_thread_content
            || orch_db::queries::notifications::people::machine_notice_has_unscoped_thread_text(
                &row.kind, &row.data,
            ) {
            String::new()
        } else if kind == NotificationKind::JobFinished
            && row.data.get("state").and_then(Json::as_str) == Some("failed")
        {
            orch_api_contract::messages::for_failure(
                row.data
                    .get("error_code")
                    .and_then(Json::as_str)
                    .unwrap_or("internal"),
            )
        } else {
            row.body.clone()
        },
        actor: if hide_thread_content {
            None
        } else {
            actors.get(&row.id).cloned()
        },
        node,
        thread_id: row.thread_id,
        thread,
        path,
        approval_id: if hide_thread_content {
            None
        } else {
            row.approval_id
        },
        chat_id: row.chat_id,
        url,
        data: data.clone(),
        details: data
            .get(orch_db::queries::notifications::people::NOTIFICATION_DETAILS_MEMBER)
            .and_then(|details| {
                serde_json::from_value::<
                        orch_api_contract::notifications::NotificationActionDetails,
                    >(details.clone())
                    .ok()
            })
            .filter(|details| {
                details.v == orch_db::queries::notifications::people::NOTIFICATION_DETAILS_VERSION
                    && details.action == kind
            }),
        created_at: Time::new(row.created_at),
        read_at: row.read_at.map(Time::new),
        inbox_seq: row.inbox_seq,
        // The list by place gives the group, and the lists say what waits for the reader.
        group: None,
        waiting_for_me: false,
    })
}

/// Discards stored copies before current file metadata is projected for a reader.
fn discard_file_projection(data: &mut Json) {
    if let Some(data) = data.as_object_mut() {
        data.remove("files");
        if let Some(parameters) = data
            .get_mut(orch_db::queries::notifications::people::NOTIFICATION_DETAILS_MEMBER)
            .and_then(|details| details.get_mut("parameters"))
            .and_then(Json::as_object_mut)
        {
            parameters.remove("files");
        }
    }
}

/// Adds current readable chat attachments to a whole Inbox page without fetching any original bytes.
pub async fn notification_files(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    rows: &mut [&mut InboxRow],
) -> crate::middleware::requests::ApiResult<()> {
    use crate::middleware::requests::ApiError;
    use orch_db::queries::chats;
    // File names are a current permission projection, never trusted notification payload data.
    for row in rows.iter_mut() {
        discard_file_projection(&mut row.data);
    }
    let chats: Vec<Id> = rows.iter().filter_map(|row| row.chat_id).collect();
    if chats.is_empty() {
        return Ok(());
    }
    let chats = chats::chats(conn, &chats).await?;
    let readable = orch_authz::chats::listed_access(conn, principal, &chats)
        .await
        .map_err(ApiError::internal)?;
    let messages: Vec<Id> = rows
        .iter()
        .filter_map(|row| {
            row.chat_id.filter(|id| readable.contains_key(id))?;
            row.data.get("message_id")?.as_str()?.parse().ok()
        })
        .collect();
    let messages: Vec<chats::MessageRow> =
        sqlx::query_as("SELECT * FROM chat_messages WHERE id=ANY($1) AND deleted_at IS NULL")
            .bind(messages)
            .fetch_all(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    let messages: HashMap<_, _> = messages
        .into_iter()
        .map(|message| (message.id, message))
        .collect();
    for row in rows {
        let files = row.chat_id.filter(|chat| readable.contains_key(chat)).and_then(|chat| {
            let id: Id = row.data.get("message_id")?.as_str()?.parse().ok()?;
            let message = messages.get(&id).filter(|message| message.chat_id == chat)?;
            let parts = serde_json::from_value::<Vec<orch_api_contract::chats::ChatPart>>(message.content.clone()).ok()?;
            Some(crate::routes::documents::references::chat_files(parts).into_iter().map(|(blob, name, size, media_type)| {
                serde_json::json!({ "blob": blob, "name": name, "size": size, "media_type": media_type })
            }).collect::<Vec<_>>())
        }).unwrap_or_default();
        if let Some(data) = row.data.as_object_mut()
            && !files.is_empty()
        {
            data.insert("files".to_owned(), Json::Array(files));
        }
    }
    Ok(())
}

/// Current approval state and actor for these already-authorized notices, in one bounded batch.
/// Enrichment is in memory only; the settlement producer already stamps the same durable row.
pub(crate) async fn notification_people(
    conn: &mut orch_db::Conn,
    rows: &mut [&mut InboxRow],
) -> crate::middleware::requests::ApiResult<HashMap<Id, UserRef>> {
    fn id(value: &Json) -> Option<Id> {
        value
            .as_str()
            .or_else(|| value.get("id").and_then(Json::as_str))
            .or_else(|| {
                value
                    .get("user")
                    .and_then(|user| user.get("id"))
                    .and_then(Json::as_str)
            })
            .and_then(|id| id.parse().ok())
    }
    let given = |row: &InboxRow| {
        ["actor", "author", "requester", "by"]
            .iter()
            .find_map(|key| row.data.get(*key).and_then(id))
    };
    let mut approvals: Vec<Id> = rows.iter().filter_map(|row| row.approval_id).collect();
    approvals.sort_unstable();
    approvals.dedup();
    let current: HashMap<Id, _> =
        orch_db::queries::notifications::people::approval_actors(conn, &approvals)
            .await?
            .into_iter()
            .map(|approval| (approval.id, approval))
            .collect();
    for row in rows.iter_mut() {
        let Some(approval) = row.approval_id.and_then(|approval| current.get(&approval)) else {
            continue;
        };
        if approval.state == "pending"
            || !matches!(
                row.kind.as_str(),
                "approval.requested" | "capability.requested" | "access.requested"
            )
        {
            continue;
        }
        let mut data = row.data.as_object().cloned().unwrap_or_else(|| {
            let mut data = serde_json::Map::new();
            data.insert("_orch_legacy".to_owned(), row.data.clone());
            data
        });
        let action = if approval.kind == "input" && approval.state == "approved" {
            Some("run")
        } else if approval.kind == "plan" {
            approval
                .decision
                .as_ref()
                .and_then(|decision| decision.get("option_id"))
                .and_then(Json::as_str)
        } else {
            None
        };
        if let Some(action) = action {
            data.insert("request_action".to_owned(), json!(action));
        }
        data.insert("request_decision".to_owned(),json!({
            "expires_at":approval.decision.as_ref().and_then(|decision|decision.get("expires_at")),
            "note":approval.decision.as_ref().and_then(|decision|decision.get("note")),
        }));
        data.insert(
            "decided_at".to_owned(),
            json!(approval.decided_at.map(Time::new)),
        );
        data.insert(
            "approval_state".to_owned(),
            Json::String(approval.state.clone()),
        );
        data.insert(
            "decided_by".to_owned(),
            serde_json::json!(approval.decided_by),
        );
        let version = row
            .data
            .get(orch_db::queries::notifications::people::NOTIFICATION_DETAILS_MEMBER)
            .and_then(|details| details.get("v"))
            .and_then(Json::as_u64);
        row.data = if version.is_none_or(|version| {
            version
                == u64::from(orch_db::queries::notifications::people::NOTIFICATION_DETAILS_VERSION)
        }) {
            orch_db::queries::notifications::people::notification_data(
                &row.kind,
                &Json::Object(data),
            )
        } else {
            Json::Object(data)
        };
    }
    let actors: Vec<(Id, Id)> = rows
        .iter()
        .filter_map(|row| {
            let approval = row.approval_id.and_then(|approval| current.get(&approval));
            let actor = if let Some(approval) = approval.filter(|approval| {
                approval.state != "pending"
                    && matches!(
                        row.kind.as_str(),
                        "approval.requested" | "capability.requested" | "access.requested"
                    )
            }) {
                approval.decided_by
            } else {
                given(row).or_else(|| {
                    approval.and_then(|approval| {
                        if row.kind == "access.decided" {
                            approval.decided_by
                        } else {
                            approval.requester_user_id
                        }
                    })
                })
            };
            actor.map(|actor| (row.id, actor))
        })
        .collect();
    let mut ids: Vec<Id> = actors.iter().map(|(_, actor)| *actor).collect();
    ids.sort_unstable();
    ids.dedup();
    let people = crate::middleware::authorization::refusals::user_refs(conn, &ids).await?;
    Ok(actors
        .into_iter()
        .filter_map(|(row, actor)| people.get(&actor).cloned().map(|person| (row, person)))
        .collect())
}

/// The rows of the person's Inbox numbered in `(after, upto]`.
pub(crate) async fn read(inner: &Inner, person: Id, after: Seq, upto: Seq) -> Result<Rows, Fail> {
    let mut conn = inner.db.acquire().await?;
    let mut rows = feeds::inbox_between(&mut conn, person, after, upto, MOST_ROWS + 1).await?;
    if i64::try_from(rows.len()).unwrap_or(i64::MAX) > MOST_ROWS {
        return Ok(Rows::TooMany);
    }
    let Some(highest) = rows.last().map(|row| row.inbox_seq) else {
        return Ok(Rows::Nothing);
    };
    let mut places: Vec<Id> = rows
        .iter()
        .flat_map(|row| [row.node_id, row.thread_id])
        .flatten()
        .collect();
    places.sort_unstable();
    places.dedup();
    let principal = principal_of(inner, &mut conn, person).await?;
    let nodes = match &principal {
        Some(principal) if !places.is_empty() => {
            view::ways(&mut conn, principal, &places, Policy::default())
                .await
                .map_err(Fail::other)?
        }
        _ => HashMap::new(),
    };
    let actors = if principal.is_some() {
        notification_people(&mut conn, &mut rows.iter_mut().collect::<Vec<_>>())
            .await
            .map_err(Fail::other)?
    } else {
        HashMap::new()
    };
    if let Some(principal) = &principal {
        notification_files(
            &mut conn,
            principal,
            &mut rows.iter_mut().collect::<Vec<_>>(),
        )
        .await
        .map_err(Fail::other)?;
    } else {
        for row in &mut rows {
            discard_file_projection(&mut row.data);
        }
    }
    let summary = match &principal {
        Some(principal) => {
            let counted = inbox::summary(&mut conn, principal, inner.engine()?, inner.clock.now())
                .await
                .map_err(Fail::other)?;
            Some(UserDataSummary {
                unread: counted.unread,
                approvals_pending: counted.approvals_pending,
            })
        }
        None => None,
    };
    let mut notifications: Vec<foreign::Notification> = Vec::new();
    let waiting = match &principal {
        Some(principal) => match inner.engine() {
            Ok(engine) => crate::routes::notifications::grouping::waiting_for(
                engine, &mut conn, principal, &rows,
            )
            .await
            .map_err(Fail::other)?,
            Err(_) => std::collections::HashSet::new(),
        },
        None => std::collections::HashSet::new(),
    };
    for row in &rows {
        if let Some(principal) = &principal
            && !orch_authz::documents::notice_readable(
                &mut conn,
                principal,
                &row.kind,
                &row.data,
                row.chat_id,
                row.thread_id,
            )
            .await
            .map_err(Fail::other)?
        {
            continue;
        }
        // A row of a kind this server does not know is skipped, not a reason
        // to send nothing; it is still among the numbers that were read.
        let Some(mut view) = notification_view(row, &nodes, &actors) else {
            continue;
        };
        view.waiting_for_me = waiting.contains(&row.id);
        notifications.push(view);
    }
    Ok(Rows::Frame(Box::new(Frame {
        data: SyncData::User(UserData {
            notifications: Some(notifications),
            summary,
        }),
        highest,
        held: Vec::new(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use orch_types::new_id;
    use serde_json::json;

    #[test]
    fn stored_file_metadata_is_removed_from_both_the_notice_and_its_action_copy() {
        let mut data = orch_db::queries::notifications::people::notification_data(
            "chat.message",
            &json!({"message_id":"message", "files":[{"name":"a stale private filename"}]}),
        );
        discard_file_projection(&mut data);
        assert!(data.get("files").is_none());
        assert!(data.pointer("/_orch_action/parameters/files").is_none());
        assert_eq!(data["message_id"], "message");
        assert_eq!(data["_orch_action"]["parameters"]["message_id"], "message");
    }

    fn row(kind: &str) -> InboxRow {
        InboxRow {
            id: new_id(),
            kind: kind.to_owned(),
            node_id: None,
            thread_id: None,
            approval_id: None,
            chat_id: None,
            title: "Done".to_owned(),
            body: String::new(),
            data: json!({}),
            inbox_seq: 7,
            created_at: Utc
                .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
                .single()
                .unwrap(),
            read_at: None,
        }
    }

    fn place(kind: NodeKind) -> NodeRef {
        NodeRef {
            thread_display: None,
            id: new_id(),
            kind,
            name: Some("Somewhere".to_owned()),
            redacted: false,
        }
    }

    #[test]
    fn a_notification_opens_the_path_its_kind_named_else_its_thread_else_the_approvals_else_its_place_else_the_inbox()
     {
        let mut named = row("milestone");
        named.data = json!({ "path": "/resources/machines/abc" });
        named.thread_id = Some(new_id());
        assert_eq!(open_path(&named, None), "/resources/machines/abc");

        let mut threaded = row("turn.completed");
        let thread = new_id();
        threaded.thread_id = Some(thread);
        assert_eq!(open_path(&threaded, None), format!("/thread/{thread}"));

        let mut approval = row("approval.requested");
        let asked = new_id();
        approval.approval_id = Some(asked);
        assert_eq!(
            open_path(&approval, None),
            format!("/approvals?approval={asked}")
        );
        approval.data = json!({"path":"/approvals"});
        assert_eq!(
            open_path(&approval, None),
            format!("/approvals?approval={asked}")
        );

        let topic = place(NodeKind::Topic);
        assert_eq!(
            open_path(&row("milestone"), Some(&topic)),
            format!("/topic/{}", topic.id)
        );
        assert_eq!(
            open_path(&row("milestone"), Some(&place(NodeKind::Org))),
            "/inbox"
        );
        assert_eq!(open_path(&row("milestone"), None), "/inbox");
    }

    #[test]
    fn a_path_that_leaves_the_application_is_not_followed() {
        for path in [
            "//elsewhere.example/phish",
            "https://elsewhere.example/",
            "relative",
        ] {
            let mut tricky = row("milestone");
            tricky.data = json!({ "path": path });
            assert_eq!(open_path(&tricky, None), "/inbox", "{path}");
        }
    }

    #[test]
    fn a_row_of_a_kind_nobody_knows_is_no_notification() {
        let nodes = HashMap::new();
        assert!(notification_view(&row("milestone"), &nodes, &HashMap::new()).is_some());
        assert!(notification_view(&row("something.new"), &nodes, &HashMap::new()).is_none());
    }

    #[test]
    fn the_place_a_row_names_is_shown_as_the_reader_may_know_it() {
        let topic = place(NodeKind::Topic);
        let mut at = row("milestone");
        at.node_id = Some(topic.id);
        let nodes = HashMap::from([(topic.id, vec![topic.clone()])]);
        let shown = notification_view(&at, &nodes, &HashMap::new()).unwrap();
        assert_eq!(shown.node, Some(topic));
        assert_eq!(shown.inbox_seq, 7);
        // A place the reader may not know is not named.
        assert_eq!(
            notification_view(&at, &HashMap::new(), &HashMap::new())
                .unwrap()
                .node,
            None
        );
    }
}
