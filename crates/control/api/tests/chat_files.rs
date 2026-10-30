//! Files in a chat over HTTP: a file stored for an attachment, pictures shown and every other file a download under its own type, files found only in the chat that carries them and read only by those who read it (also after a message was forwarded), and what is refused to those who may not send.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::{Room, text};
use http::StatusCode;
use orch_api_contract::chats::{
    ChatAttachmentBlobBody, ChatAttachmentCreate, ChatAttachmentCreateReply, ChatBlobGet,
    ChatBlobGetQuery, ChatMessage, ChatMessageDelete, ChatMessageUpdate, ChatMessageUpdateBody,
    ChatPart, ForwardCreate, ForwardCreateBody, ForwardDestination, ForwardDestinationChat,
    ForwardSource, ForwardSourceChat,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeArchive;
use orch_api_contract::threads::{
    FILENAME_HEADER, ImageMediaType, UploadCreate, UploadSessionComplete, UploadSessionCreate,
    UploadSessionCreateBody, UploadSessionPut, UploadSessionPutQuery,
};
use orch_api_core::testing::{RawReply, Reply};
use orch_settings::Target;
use orch_types::{Hash, Id, new_id};
use places::Who;
use serde_json::json;

#[tokio::test]
async fn discarding_duplicate_upload_bytes_keeps_a_saved_private_chat_draft_authorized() {
    use orch_api_contract::chats::{ChatDraftSet, ChatDraftSetBody};
    use orch_api_contract::threads::{UploadBlobDelete, UploadBlobDeleteQuery};
    let room = Room::new().await;
    let (blob, size) = room.store(b"private draft bytes").await;
    let parts = vec![ChatPart::Attachment {
        blob,
        name: "draft.txt".to_owned(),
        size,
        media_type: "text/plain".to_owned(),
    }];
    room.world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatDraftSetBody {
            parts: parts.clone(),
            reply_to: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(room.pins(&blob).await, 1);
    room.world
        .app
        .call(UploadBlobDelete, &[&blob])
        .token(&room.bea.token)
        .query(&UploadBlobDeleteQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(room.pins(&blob).await, 1);
    room.send(&room.bea, room.chat, parts, "kept-private-draft")
        .await
        .ok();
}

/// The start of a PDF, which is what tells it from a file called what it likes.
fn pdf(len: usize) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    bytes.extend((0..len).map(|at| u8::try_from(at % 251).unwrap()));
    bytes
}

/// The start of a PNG.
fn png() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(b"not really a picture, but it begins as one");
    bytes
}

/// `by` stores a file for an attachment in `chat`, sending its bytes as the body.
async fn upload(
    room: &Room,
    by: &Who,
    chat: Id,
    name: &str,
    bytes: &[u8],
) -> Reply<ChatAttachmentCreateReply> {
    room.world
        .app
        .call(ChatAttachmentCreate, &[&chat])
        .token(&by.token)
        .header(FILENAME_HEADER, name)
        .bytes(bytes)
        .send()
        .await
}

/// A message by `by` in `chat` that carries the file as an attachment.
async fn send_file(
    room: &Room,
    by: &Who,
    chat: Id,
    file: &ChatAttachmentCreateReply,
) -> ChatMessage {
    room.send(
        by,
        chat,
        vec![
            text("here it is"),
            ChatPart::Attachment {
                blob: file.blob,
                name: file.name.clone(),
                size: file.size,
                media_type: file.media_type.clone(),
            },
        ],
        &new_id().to_string(),
    )
    .await
    .ok()
    .message
}

/// `by` asks `chat` for the blob `hash`.
async fn fetch(room: &Room, by: &Who, chat: Id, hash: &Hash, download: Option<bool>) -> RawReply {
    room.world
        .app
        .call(ChatBlobGet, &[&chat, hash])
        .token(&by.token)
        .query(&ChatBlobGetQuery {
            download,
            ..Default::default()
        })
        .send_bytes()
        .await
}

fn header<'a>(reply: &'a RawReply, name: &str) -> &'a str {
    reply
        .headers
        .get(name)
        .unwrap_or_else(|| panic!("no header {name}"))
        .to_str()
        .unwrap()
}

#[tokio::test]
async fn inbox_file_previews_require_current_chat_reading_and_a_live_message() {
    use orch_api_contract::notifications::{NotificationList, NotificationListQuery};
    use orch_db::queries::notifications::people::{NewNotification, announce};
    let room = Room::new().await;
    let stored = upload(&room, &room.bea, room.chat, "report.pdf", &pdf(128))
        .await
        .ok();
    let message = send_file(&room, &room.bea, room.chat, &stored).await;
    for person in [&room.cai, &room.eve] {
        let mut conn = room.world.conn().await;
        announce(&mut conn, &NewNotification {
            user_id: person.id(), kind: "chat.message", node_id: Some(room.topic), thread_id: None,
            approval_id: None, chat_id: Some(room.chat), title: "A file arrived", body: "",
            data: &json!({"message_id":message.id,"files":[{"name":"a stale private filename"}]}),
            dedupe_key: "file-preview", at: room.world.now(),
        }).await.unwrap();
    }
    let query = NotificationListQuery::default();
    let read = room
        .world
        .app
        .call(NotificationList, &[])
        .token(&room.cai.token)
        .query(&query)
        .send()
        .await
        .ok();
    let notice = read
        .items
        .iter()
        .find(|row| row.data["message_id"] == json!(message.id))
        .unwrap();
    assert_eq!(notice.data["files"][0]["name"], "report.pdf");
    assert_eq!(notice.data["files"][0]["blob"], json!(stored.blob));
    let hidden = room
        .world
        .app
        .call(NotificationList, &[])
        .token(&room.eve.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(
        !serde_json::to_string(&hidden)
            .unwrap()
            .contains("report.pdf")
    );
    assert!(
        !serde_json::to_string(&hidden)
            .unwrap()
            .contains("a stale private filename")
    );
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &message.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    let gone = room
        .world
        .app
        .call(NotificationList, &[])
        .token(&room.cai.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(gone.items.iter().all(|row| row.data["files"].is_null()));
}

// ---------------------------------------------------------------- a file stored, and read back

#[tokio::test]
async fn a_file_is_stored_for_an_attachment_and_read_back_by_those_who_read_the_chat() {
    let room = Room::new().await;
    let bytes = pdf(3_000);
    let stored = upload(&room, &room.bea, room.chat, "Report Q3.pdf", &bytes)
        .await
        .ok();
    assert_eq!(stored.blob, Hash::of(&bytes));
    assert_eq!(stored.size, i64::try_from(bytes.len()).unwrap());
    assert_eq!(stored.name, "Report Q3.pdf");
    assert_eq!(stored.media_type, "application/pdf");
    // Nobody is told of a file until a message names it, and the store holds it.
    let held = room
        .world
        .app
        .app
        .blobs
        .read_whole(&stored.blob, 1 << 20)
        .await
        .unwrap();
    assert_eq!(held.as_ref(), bytes.as_slice());
    assert_eq!(room.pins(&stored.blob).await, 0);
    assert_eq!(
        fetch(&room, &room.cai, room.chat, &stored.blob, None)
            .await
            .status,
        StatusCode::NOT_FOUND,
        "a file no message carries is nobody's yet"
    );

    let message = send_file(&room, &room.bea, room.chat, &stored).await;
    assert_eq!(
        room.pins(&stored.blob).await,
        1,
        "a message pins what it carries"
    );
    let read = fetch(&room, &room.cai, room.chat, &stored.blob, None).await;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(read.raw.as_ref(), bytes.as_slice());
    assert_eq!(header(&read, "content-type"), "application/pdf");
    assert!(header(&read, "content-disposition").starts_with("attachment;"));
    assert!(header(&read, "content-disposition").contains("Report Q3.pdf"));
    assert_eq!(header(&read, "x-content-type-options"), "nosniff");
    assert_eq!(header(&read, "etag"), format!("\"{}\"", stored.blob));
    assert!(header(&read, "content-security-policy").contains("sandbox"));
    // The message shows the part it was sent with.
    assert!(matches!(
        &message.parts[1],
        ChatPart::Attachment { blob, .. } if *blob == stored.blob
    ));
}

#[tokio::test]
async fn a_file_that_is_asked_for_again_with_its_tag_is_not_sent_again() {
    let room = Room::new().await;
    let stored = upload(&room, &room.bea, room.chat, "notes.txt", b"plain words")
        .await
        .ok();
    send_file(&room, &room.bea, room.chat, &stored).await;
    let tag = format!("\"{}\"", stored.blob);
    let again = room
        .world
        .app
        .call(ChatBlobGet, &[&room.chat, &stored.blob])
        .token(&room.cai.token)
        .query(&ChatBlobGetQuery::default())
        .header("if-none-match", &tag)
        .send_bytes()
        .await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);
    assert!(again.raw.is_empty());
}

#[tokio::test]
async fn a_range_of_a_file_is_served_as_a_partial_answer() {
    let room = Room::new().await;
    let bytes = pdf(1_000);
    let stored = upload(&room, &room.bea, room.chat, "big.pdf", &bytes)
        .await
        .ok();
    send_file(&room, &room.bea, room.chat, &stored).await;
    let part = room
        .world
        .app
        .call(ChatBlobGet, &[&room.chat, &stored.blob])
        .token(&room.cai.token)
        .query(&ChatBlobGetQuery::default())
        .header("range", "bytes=10-19")
        .send_bytes()
        .await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.raw.as_ref(), &bytes[10..20]);
    assert_eq!(
        header(&part, "content-range"),
        format!("bytes 10-19/{}", bytes.len())
    );
}

// ---------------------------------------------------------------- what is shown, and what is a download

#[tokio::test]
async fn a_picture_is_shown_inline_and_every_other_file_is_a_download_under_its_own_type() {
    let room = Room::new().await;
    let image = png();
    let sent = room
        .world
        .app
        .call(UploadCreate, &[])
        .token(&room.bea.token)
        .header("content-type", "image/png")
        .header(FILENAME_HEADER, "shot.png")
        .bytes(&image)
        .send()
        .await
        .ok();
    room.send(
        &room.bea,
        room.chat,
        vec![ChatPart::Image {
            blob: sent.blob,
            media_type: ImageMediaType::Png,
            name: "shot.png".to_owned(),
        }],
        "picture",
    )
    .await
    .ok();
    let shown = fetch(&room, &room.cai, room.chat, &sent.blob, None).await;
    assert_eq!(shown.status, StatusCode::OK);
    assert_eq!(header(&shown, "content-type"), "image/png");
    assert!(header(&shown, "content-disposition").starts_with("inline;"));
    assert_eq!(shown.raw.as_ref(), image.as_slice());
    let asked = fetch(&room, &room.cai, room.chat, &sent.blob, Some(true)).await;
    assert!(header(&asked, "content-disposition").starts_with("attachment;"));
    assert_eq!(header(&asked, "content-type"), "image/png");

    // A picture sent as a file is a picture; a film and a sound are downloads that say what they are.
    for (name, bytes, kind, inline) in [
        ("photo.jpg", b"jpeg bytes".to_vec(), "image/jpeg", true),
        ("clip.mp4", b"film".to_vec(), "video/mp4", false),
        ("song.mp3", b"sound".to_vec(), "audio/mpeg", false),
        ("data.csv", b"a,b\n1,2\n".to_vec(), "text/csv", false),
    ] {
        let stored = upload(&room, &room.bea, room.chat, name, &bytes).await.ok();
        assert_eq!(stored.media_type, kind);
        send_file(&room, &room.bea, room.chat, &stored).await;
        let read = fetch(&room, &room.cai, room.chat, &stored.blob, None).await;
        assert_eq!(header(&read, "content-type"), kind, "{name}");
        let disposition = header(&read, "content-disposition");
        assert_eq!(
            disposition.starts_with("inline;"),
            inline,
            "{name}: {disposition}"
        );
        assert_eq!(read.raw.as_ref(), bytes.as_slice());
    }
}

#[tokio::test]
async fn the_type_a_sender_claims_for_a_file_is_not_the_type_it_has() {
    let room = Room::new().await;
    let stored = upload(
        &room,
        &room.bea,
        room.chat,
        "page.html",
        b"<script>1</script>",
    )
    .await
    .ok();
    assert_eq!(stored.media_type, "text/html");
    // A message may say what it likes about its attachment; the answer says only what is safe to say.
    let claims = |name: &str, media: &str, file: &ChatAttachmentCreateReply| ChatPart::Attachment {
        blob: file.blob,
        name: name.to_owned(),
        size: file.size,
        media_type: media.to_owned(),
    };
    room.send(
        &room.bea,
        room.chat,
        vec![claims("page.html", "image/svg+xml", &stored)],
        "svg",
    )
    .await
    .ok();
    let read = fetch(&room, &room.cai, room.chat, &stored.blob, None).await;
    let disposition = header(&read, "content-disposition");
    assert_eq!(header(&read, "content-type"), "application/octet-stream");
    assert!(
        disposition.starts_with("attachment;"),
        "active content is never shown inline: {disposition}"
    );
    assert_eq!(header(&read, "x-content-type-options"), "nosniff");

    let other = upload(&room, &room.bea, room.chat, "notes.txt", b"words")
        .await
        .ok();
    room.send(
        &room.bea,
        room.chat,
        vec![claims("notes.txt", "text/html; charset=utf-7", &other)],
        "odd",
    )
    .await
    .ok();
    let read = fetch(&room, &room.cai, room.chat, &other.blob, None).await;
    assert_eq!(
        header(&read, "content-type"),
        "text/plain",
        "a claim with parameters is not taken for the type"
    );
}

// ---------------------------------------------------------------- where a file is found

#[tokio::test]
async fn a_file_is_found_only_in_the_chat_that_carries_it() {
    let room = Room::new().await;
    let stored = upload(&room, &room.bea, room.chat, "plan.pdf", &pdf(100))
        .await
        .ok();
    send_file(&room, &room.bea, room.chat, &stored).await;
    let elsewhere = room.chat_in_another_topic().await;
    let missing = fetch(&room, &room.ada, elsewhere, &stored.blob, None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.problem().code, ErrorCode::NotFound);
    // Another hash the store holds is no more found than one it never saw.
    let (other, _) = room.store(b"something else the store holds").await;
    for hash in [other, Hash::of(b"never seen")] {
        assert_eq!(
            fetch(&room, &room.ada, room.chat, &hash, None).await.status,
            StatusCode::NOT_FOUND
        );
    }
    // A word that is no hash names nothing.
    let nonsense = room
        .world
        .app
        .call(ChatBlobGet, &[&room.chat, &"not-a-hash"])
        .token(&room.ada.token)
        .query(&ChatBlobGetQuery::default())
        .send_bytes()
        .await;
    assert_eq!(nonsense.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_those_who_read_a_chat_fetch_its_files() {
    let room = Room::new().await;
    // A node's chat: a stranger to the org lacks the right to read it, and is told which.
    let stored = upload(&room, &room.bea, room.chat, "plan.pdf", &pdf(100))
        .await
        .ok();
    send_file(&room, &room.bea, room.chat, &stored).await;
    let refused = fetch(&room, &room.eve, room.chat, &stored.blob, None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(
        places::missing_permissions(&refused.problem()),
        ["chat.view"]
    );
    // Nobody who is not signed in has one either.
    let anonymous = room
        .world
        .app
        .call(ChatBlobGet, &[&room.chat, &stored.blob])
        .query(&ChatBlobGetQuery::default())
        .send_bytes()
        .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    // A direct chat does not exist for anybody who is not in it.
    let direct = room.direct(&room.bea, &room.cai).await;
    let private = upload(&room, &room.bea, direct.base.id, "secret.pdf", &pdf(50))
        .await
        .ok();
    send_file(&room, &room.bea, direct.base.id, &private).await;
    assert_eq!(
        fetch(&room, &room.cai, direct.base.id, &private.blob, None)
            .await
            .status,
        StatusCode::OK
    );
    for outsider in [&room.gus, &room.eve] {
        let refused = fetch(&room, outsider, direct.base.id, &private.blob, None).await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", outsider.id());
    }
    // And the same file, asked for through the node's chat, which does not carry it, is not found.
    assert_eq!(
        fetch(&room, &room.bea, room.chat, &private.blob, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_admin_who_reads_the_chats_of_others_may_fetch_their_files_and_it_is_noted() {
    let room = Room::new().await;
    let direct = room.direct(&room.bea, &room.cai).await;
    let stored = upload(&room, &room.bea, direct.base.id, "secret.pdf", &pdf(50))
        .await
        .ok();
    send_file(&room, &room.bea, direct.base.id, &stored).await;
    // Dee is an admin of the org, who holds directs.view on it.
    let read = fetch(&room, &room.dee, direct.base.id, &stored.blob, None).await;
    assert_eq!(read.status, StatusCode::OK);
    assert!(
        room.audit_of(direct.base.id)
            .await
            .contains(&"chat.inspected".to_owned()),
        "reading a chat as an outsider leaves its one trace"
    );
    // Reading is all that an inspector does: they store no file there.
    let refused = upload(&room, &room.dee, direct.base.id, "mine.pdf", &pdf(10))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn a_file_forwarded_to_another_chat_is_served_there_to_those_who_read_only_that_chat() {
    let room = Room::new().await;
    let from = room.direct(&room.bea, &room.gus).await;
    let to = room.direct(&room.bea, &room.cai).await;
    let stored = upload(&room, &room.gus, from.base.id, "contract.pdf", &pdf(200))
        .await
        .ok();
    let original = send_file(&room, &room.gus, from.base.id, &stored).await;
    // Cai is in the second chat only: the file is not theirs through the first.
    assert_eq!(
        fetch(&room, &room.cai, from.base.id, &stored.blob, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fetch(&room, &room.cai, to.base.id, &stored.blob, None)
            .await
            .status,
        StatusCode::NOT_FOUND,
        "nothing has been forwarded yet"
    );
    room.world
        .app
        .call(ForwardCreate, &[])
        .token(&room.bea.token)
        .body(&ForwardCreateBody {
            from: ForwardSource::Chat(ForwardSourceChat {
                chat_id: from.base.id,
                message_ids: vec![original.id],
            }),
            to: vec![ForwardDestination::Chat(ForwardDestinationChat {
                chat_id: to.base.id,
            })],
            comment: None,
            client_id: "forward-1".to_owned(),
        })
        .send()
        .await
        .ok();
    let there = fetch(&room, &room.cai, to.base.id, &stored.blob, None).await;
    assert_eq!(there.status, StatusCode::OK);
    assert_eq!(there.raw.len(), 9 + 200);
    assert_eq!(header(&there, "content-type"), "application/pdf");
    assert!(header(&there, "content-disposition").contains("contract.pdf"));
    assert_eq!(
        fetch(&room, &room.cai, from.base.id, &stored.blob, None)
            .await
            .status,
        StatusCode::NOT_FOUND,
        "the first chat is still closed to Cai"
    );
    // A copy does not change when its original is deleted.
    room.world
        .app
        .call(ChatMessageDelete, &[&from.base.id, &original.id])
        .token(&room.gus.token)
        .send()
        .await
        .ok();
    assert_eq!(
        fetch(&room, &room.gus, from.base.id, &stored.blob, None)
            .await
            .status,
        StatusCode::NOT_FOUND,
        "the deleted message holds nothing"
    );
    assert_eq!(
        fetch(&room, &room.cai, to.base.id, &stored.blob, None)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_file_a_message_no_longer_carries_is_no_longer_served() {
    let room = Room::new().await;
    let first = upload(&room, &room.bea, room.chat, "one.pdf", &pdf(10))
        .await
        .ok();
    let second = upload(&room, &room.bea, room.chat, "two.pdf", &pdf(20))
        .await
        .ok();
    let message = send_file(&room, &room.bea, room.chat, &first).await;
    assert_eq!(
        fetch(&room, &room.cai, room.chat, &first.blob, None)
            .await
            .status,
        StatusCode::OK
    );
    // The message is edited to carry the other file.
    room.world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &message.id])
        .token(&room.bea.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![ChatPart::Attachment {
                blob: second.blob,
                name: second.name.clone(),
                size: second.size,
                media_type: second.media_type.clone(),
            }],
        })
        .send()
        .await
        .ok();
    assert_eq!(
        fetch(&room, &room.cai, room.chat, &first.blob, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fetch(&room, &room.cai, room.chat, &second.blob, None)
            .await
            .status,
        StatusCode::OK
    );
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &message.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(
        fetch(&room, &room.cai, room.chat, &second.blob, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

// ---------------------------------------------------------------- who may store a file

#[tokio::test]
async fn only_those_who_may_send_in_a_chat_store_a_file_for_it() {
    let room = Room::new().await;
    // A stranger to the org may not take part in the node's chat, and is told what lacks.
    let refused = upload(&room, &room.eve, room.chat, "a.pdf", &pdf(5))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(places::missing_permissions(&refused), ["chat.view"]);
    // Nobody who is not signed in.
    let anonymous = room
        .world
        .app
        .call(ChatAttachmentCreate, &[&room.chat])
        .header(FILENAME_HEADER, "a.pdf")
        .bytes(&pdf(5))
        .send()
        .await
        .problem();
    assert_eq!(anonymous.code, ErrorCode::AuthRequired);
    // A direct chat is not there for the one who is not in it.
    let direct = room.direct(&room.bea, &room.cai).await;
    let missing = upload(&room, &room.gus, direct.base.id, "a.pdf", &pdf(5))
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
    // Nothing a refused request carried was kept.
    assert!(
        room.world
            .app
            .app
            .blobs
            .store()
            .exists(&Hash::of(&pdf(5)))
            .await
            .map(|held| !held)
            .unwrap()
    );
}

#[tokio::test]
async fn a_chat_under_an_archived_node_takes_no_file() {
    let room = Room::new().await;
    room.world
        .app
        .call(NodeArchive, &[&room.project])
        .token(&room.ada.token)
        .body(&places::archive_body(&room.world, &room.ada.token, room.project, true).await)
        .send()
        .await
        .ok();
    let refused = upload(&room, &room.bea, room.chat, "late.pdf", &pdf(5))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeArchived);
}

// ---------------------------------------------------------------- how large, and how it arrives

#[tokio::test]
async fn a_file_larger_than_the_chat_allows_is_refused_with_the_limit_and_leaves_nothing() {
    let room = Room::new().await;
    room.world
        .set_at(
            Target::Node(room.root),
            "chat.attachment_max_bytes",
            json!(1_000),
        )
        .await;
    let small = pdf(900);
    upload(&room, &room.bea, room.chat, "small.pdf", &small)
        .await
        .ok();
    let large = pdf(1_500);
    let refused = upload(&room, &room.bea, room.chat, "large.pdf", &large).await;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
    let problem = refused.problem();
    assert_eq!(problem.code, ErrorCode::RequestTooLarge);
    assert_eq!(problem.extra["limit"], 1_000);
    assert!(
        !room
            .world
            .app
            .app
            .blobs
            .store()
            .exists(&Hash::of(&large))
            .await
            .unwrap(),
        "a body that is too large leaves nothing in the store"
    );
}

#[tokio::test]
async fn a_body_larger_than_a_request_carries_is_told_to_go_in_pieces() {
    let room = Room::new().await;
    let limit = i64::try_from(orch_api_contract::common::MAX_BODY_BYTES).unwrap();
    let bytes = vec![7u8; usize::try_from(limit).unwrap() + 1];
    let refused = upload(&room, &room.bea, room.chat, "huge.bin", &bytes)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestTooLarge);
    // The limit is a request's own, not the chat's: that is what tells a client to send the file in
    // pieces. The sentence is the catalogue's for the code.
    assert_eq!(refused.extra["limit"], limit);
    assert_eq!(
        refused.detail,
        orch_api_contract::messages::for_code(ErrorCode::RequestTooLarge)
    );
}

/// `by` sends `bytes` as an upload in pieces and ends it; the blob.
async fn upload_in_pieces(room: &Room, by: &Who, name: &str, bytes: &[u8]) -> Hash {
    let app = &room.world.app;
    let session = app
        .call(UploadSessionCreate, &[])
        .token(&by.token)
        .body(&UploadSessionCreateBody {
            size: i64::try_from(bytes.len()).unwrap(),
            name: name.to_owned(),
            media_type: None,
        })
        .send()
        .await
        .ok();
    let piece = 700;
    let mut offset = 0;
    for chunk in bytes.chunks(piece) {
        app.call(UploadSessionPut, &[&session.upload_id])
            .token(&by.token)
            .query(&UploadSessionPutQuery {
                offset: i64::try_from(offset).unwrap(),
            })
            .bytes(chunk)
            .send()
            .await
            .ok();
        offset += chunk.len();
    }
    app.call(UploadSessionComplete, &[&session.upload_id])
        .token(&by.token)
        .send()
        .await
        .ok()
        .blob
}

#[tokio::test]
async fn a_finished_upload_in_pieces_is_attached_by_its_hash_and_a_name() {
    let room = Room::new().await;
    let bytes = pdf(5_000);
    let blob = upload_in_pieces(&room, &room.bea, "long.pdf", &bytes).await;
    let attached = room
        .world
        .app
        .call(ChatAttachmentCreate, &[&room.chat])
        .token(&room.bea.token)
        .raw_body(&json!({ "blob": blob, "name": "long.pdf" }))
        .send()
        .await
        .ok();
    assert_eq!(attached.blob, blob);
    assert_eq!(attached.size, i64::try_from(bytes.len()).unwrap());
    assert_eq!(attached.media_type, "application/pdf");
    send_file(&room, &room.bea, room.chat, &attached).await;
    assert_eq!(
        fetch(&room, &room.cai, room.chat, &blob, None)
            .await
            .raw
            .as_ref(),
        bytes.as_slice()
    );

    // A hash the store does not hold names nothing.
    let unknown = room
        .world
        .app
        .call(ChatAttachmentCreate, &[&room.chat])
        .token(&room.bea.token)
        .raw_body(
            &serde_json::to_value(ChatAttachmentBlobBody {
                blob: Hash::of(b"never uploaded"),
                name: "x.pdf".to_owned(),
            })
            .unwrap(),
        )
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::NotFound);

    // An upload that is larger than the chat allows is refused when it is attached.
    room.world
        .set_at(
            Target::Node(room.root),
            "chat.attachment_max_bytes",
            json!(4_000),
        )
        .await;
    let refused = room
        .world
        .app
        .call(ChatAttachmentCreate, &[&room.chat])
        .token(&room.bea.token)
        .raw_body(&json!({ "blob": blob, "name": "long.pdf" }))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestTooLarge);
    assert_eq!(refused.extra["limit"], 4_000);
}

#[tokio::test]
async fn a_file_needs_a_name_either_in_the_header_or_with_the_hash_of_an_upload() {
    let room = Room::new().await;
    for body in [
        room.world
            .app
            .call(ChatAttachmentCreate, &[&room.chat])
            .token(&room.bea.token)
            .bytes(b"no name for me")
            .send()
            .await,
        room.world
            .app
            .call(ChatAttachmentCreate, &[&room.chat])
            .token(&room.bea.token)
            .raw_body(&json!({ "name": "x.pdf" }))
            .send()
            .await,
        room.world
            .app
            .call(ChatAttachmentCreate, &[&room.chat])
            .token(&room.bea.token)
            .header(FILENAME_HEADER, "  ")
            .bytes(b"x")
            .send()
            .await,
    ] {
        assert_eq!(body.problem().code, ErrorCode::RequestInvalid);
    }
}

#[tokio::test]
async fn a_name_that_is_not_plain_ascii_arrives_as_the_browser_escaped_it() {
    let room = Room::new().await;
    let stored = upload(&room, &room.bea, room.chat, "caf%C3%A9 menu.pdf", &pdf(5))
        .await
        .ok();
    assert_eq!(stored.name, "café menu.pdf");
    send_file(&room, &room.bea, room.chat, &stored).await;
    let read = fetch(&room, &room.cai, room.chat, &stored.blob, None).await;
    assert!(
        header(&read, "content-disposition").contains("filename*=UTF-8''caf%C3%A9%20menu.pdf"),
        "{}",
        header(&read, "content-disposition")
    );
}

#[test]
fn every_request_about_a_chat_is_bound() {
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let bound: std::collections::HashSet<&str> =
        registry.bound().iter().map(|route| route.name).collect();
    for route in orch_api_contract::chats::ROUTES {
        assert!(bound.contains(route.name), "{} is not bound", route.name);
    }
}
