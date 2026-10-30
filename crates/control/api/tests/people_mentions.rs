//! Real chat-reader choices, guarded broad mentions and frozen accepted recipients.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::{Room, text};
use orch_api_contract::access::{PageChange, PermissionPageUpdate, Role};
use orch_api_contract::chats::{
    ChatLevel, ChatMentionList, ChatMentionListQuery, ChatMentionTarget, ChatMessageUpdate,
    ChatMessageUpdateBody, ChatPart, ChatStateSet, ChatStateSetBody, MentionChoice, OrgChatCreate,
    OrgChatCreateBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_settings::Target;
use orch_types::{Id, new_id};
use places::Who;
use serde_json::json;

#[tokio::test]
async fn the_send_preview_counts_overlapping_human_audiences_once_without_posting() {
    use orch_api_contract::chats::{ChatMentionPreview, ChatMentionPreviewBody};
    let room = Room::new().await;
    let crew = group(&room, "Crew", &[room.bea.id(), room.cai.id()]).await;
    let parts = vec![
        named(ChatMentionTarget::Group { id: crew }),
        named(ChatMentionTarget::Role {
            node_id: room.topic,
            role: Role::Developer,
        }),
    ];
    let preview = room
        .world
        .app
        .call(ChatMentionPreview, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatMentionPreviewBody {
            parts: parts.clone(),
            reply_to: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(preview.count, 3);
    assert!(room.sync(&room.bea, room.chat).await.messages.is_empty());
    let posted = room
        .send(&room.bea, room.chat, parts, "preview-union")
        .await
        .ok()
        .message;
    assert_eq!(i64::try_from(posted.mentions.len()).unwrap(), preview.count);
}

#[tokio::test]
async fn a_typed_target_is_shown_once_inline_and_a_portable_draft_reopens_with_its_current_name() {
    use orch_api_contract::chats::{ChatDraftGet, ChatDraftSet, ChatDraftSetBody};
    let room = Room::new().await;
    let crew = group(&room, "Crew", &[room.bea.id(), room.cai.id()]).await;
    let target = ChatMentionTarget::Group { id: crew };
    let parts = vec![text("Before "), named(target.clone()), text(" after")];
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
    let draft = room
        .world
        .app
        .call(ChatDraftGet, &[&room.chat])
        .token(&room.bea.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(draft.parts, parts);
    assert_eq!(draft.mention_names[0].target, target);
    assert!(draft.mention_names[0].name.contains("Crew"));
    let sent = room
        .send(&room.bea, room.chat, parts, "inline-target")
        .await
        .ok()
        .message;
    assert_eq!(sent.parts.len(), 1);
    let ChatPart::Text { text } = &sent.parts[0] else {
        panic!("the reader gets current human words")
    };
    assert!(text.starts_with("Before @Crew"));
    assert!(text.ends_with(" after"));
    assert_eq!(text.matches("Crew").count(), 1);
}

async fn choices(room: &Room, who: &Who, chat: Id, q: Option<&str>) -> Vec<MentionChoice> {
    let mut found = Vec::new();
    let mut cursor = None;
    loop {
        let page = room
            .world
            .app
            .call(ChatMentionList, &[&chat])
            .token(&who.token)
            .query(&ChatMentionListQuery {
                q: q.map(str::to_owned),
                limit: Some(20),
                cursor,
            })
            .send()
            .await
            .ok();
        found.extend(page.items);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    found
}

async fn group(room: &Room, name: &str, members: &[Id]) -> Id {
    let made = room
        .world
        .app
        .call(OrgGroupCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: name.into(),
            description: None,
        })
        .send()
        .await
        .ok();
    for member in members {
        room.world
            .app
            .call(GroupMemberAdd, &[&made.id, member])
            .token(&room.ada.token)
            .send()
            .await
            .ok();
    }
    made.id
}

fn named(target: ChatMentionTarget) -> ChatPart {
    ChatPart::Mention { target }
}

fn ids(message: &orch_api_contract::chats::ChatMessage) -> Vec<Id> {
    let mut ids: Vec<Id> = message.mentions.iter().map(|user| user.id).collect();
    ids.sort_unstable();
    ids
}

#[tokio::test]
async fn an_empty_picker_lists_current_readers_and_role_or_higher_intersections_before_paging() {
    let room = Room::new().await;
    let found = choices(&room, &room.bea, room.chat, None).await;
    let humans: Vec<Id> = found
        .iter()
        .filter_map(|choice| match choice.target {
            ChatMentionTarget::User { id } => Some(id),
            _ => None,
        })
        .collect();
    assert!(
        humans.contains(&room.ada.id())
            && humans.contains(&room.bea.id())
            && humans.contains(&room.cai.id())
            && humans.contains(&room.dee.id())
            && humans.contains(&room.gus.id())
    );
    assert!(!humans.contains(&room.eve.id()) && !humans.contains(&room.fay.id()));
    let developer = found
        .iter()
        .find(|choice| {
            choice.target
                == ChatMentionTarget::Role {
                    node_id: room.topic,
                    role: Role::Developer,
                }
        })
        .unwrap();
    assert_eq!(
        developer.count, 3,
        "the owner, developer and admin; not the viewer sending"
    );
    let first_broad = found
        .iter()
        .position(|choice| !matches!(choice.target, ChatMentionTarget::User { .. }))
        .unwrap();
    assert!(
        found[..first_broad]
            .iter()
            .all(|choice| matches!(choice.target, ChatMentionTarget::User { .. }))
    );
}

#[tokio::test]
async fn overlapping_role_and_group_targets_freeze_once_and_an_edit_tells_only_newly_named_people()
{
    let room = Room::new().await;
    let crew = group(&room, "Crew", &[room.bea.id(), room.cai.id()]).await;
    let group_target = ChatMentionTarget::Group { id: crew };
    let role_target = ChatMentionTarget::Role {
        node_id: room.topic,
        role: Role::Developer,
    };
    let selected = choices(&room, &room.bea, room.chat, Some("Crew")).await;
    assert_eq!(selected[0].count, 1);
    let parts = vec![
        text("Please review"),
        named(group_target.clone()),
        named(role_target.clone()),
    ];
    let sent = room
        .send(&room.bea, room.chat, parts.clone(), "team-review")
        .await
        .ok()
        .message;
    let mut expected = vec![room.ada.id(), room.cai.id(), room.dee.id()];
    expected.sort_unstable();
    assert_eq!(ids(&sent), expected);
    assert_eq!(
        sent.mention_targets,
        vec![group_target.clone(), role_target.clone()]
    );
    room.world
        .app
        .call(GroupMemberAdd, &[&crew, &room.gus.id()])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let duplicate = room
        .send(&room.bea, room.chat, parts.clone(), "team-review")
        .await;
    assert_eq!(duplicate.status, 200);
    assert_eq!(duplicate.ok().message.id, sent.id);
    assert_eq!(
        ids(&room.sync(&room.bea, room.chat).await.messages[0]),
        expected
    );
    let edited = room
        .world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &sent.id])
        .token(&room.bea.token)
        .body(&ChatMessageUpdateBody { parts })
        .send()
        .await
        .ok();
    expected.push(room.gus.id());
    expected.sort_unstable();
    assert_eq!(ids(&edited), expected);
    let mut conn = room.world.conn().await;
    let notices: Vec<(Id, i64)> = sqlx::query_as("SELECT user_id, count(*) FROM notifications WHERE chat_id = $1 AND kind = 'mention' GROUP BY user_id")
        .bind(room.chat).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(notices.len(), 4);
    assert!(notices.iter().all(|(_, count)| *count == 1));
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT mention_targets FROM chat_messages WHERE id = $1")
            .bind(sent.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(stored, json!([group_target, role_target]));
}

#[tokio::test]
async fn managers_policy_reserves_broad_targets_but_a_group_creator_is_its_manager() {
    let room = Room::new().await;
    let crew = group(&room, "Crew", &[room.cai.id()]).await;
    room.world
        .set_at(
            Target::Node(room.topic),
            "chat.group_mentions",
            json!("managers"),
        )
        .await;
    let found = choices(&room, &room.bea, room.chat, None).await;
    assert!(
        found
            .iter()
            .all(|choice| matches!(choice.target, ChatMentionTarget::User { .. }))
    );
    let refused = room
        .send(
            &room.bea,
            room.chat,
            vec![named(ChatMentionTarget::Group { id: crew })],
            "reserved",
        )
        .await;
    assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    assert!(room.sync(&room.ada, room.chat).await.messages.is_empty());
    room.world
        .set_at(
            Target::Node(room.root),
            "chat.group_mentions",
            json!("managers"),
        )
        .await;
    let chat = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.bea.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.cai.id(), room.gus.id()],
            name: Some("Reviewers".into()),
        })
        .send()
        .await
        .ok();
    let target = ChatMentionTarget::Role {
        node_id: room.root,
        role: Role::Viewer,
    };
    let accepted = room
        .send(
            &room.bea,
            chat.base.id,
            vec![named(target.clone())],
            "creator",
        )
        .await
        .ok();
    assert_eq!(accepted.message.mentions.len(), 2);
    assert_eq!(
        room.send(&room.cai, chat.base.id, vec![named(target)], "not-creator")
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn a_direct_chat_and_an_unrelated_role_or_unreadable_person_refuse_without_writing() {
    let room = Room::new().await;
    let crew = group(&room, "Crew", &[room.cai.id()]).await;
    let direct = room.direct(&room.bea, &room.cai).await;
    assert!(
        choices(&room, &room.bea, direct.base.id, None)
            .await
            .iter()
            .all(|choice| matches!(choice.target, ChatMentionTarget::User { .. }))
    );
    for (chat, target) in [
        (direct.base.id, ChatMentionTarget::Group { id: crew }),
        (
            room.chat,
            ChatMentionTarget::Role {
                node_id: room.root,
                role: Role::Viewer,
            },
        ),
        (room.chat, ChatMentionTarget::User { id: room.eve.id() }),
    ] {
        let refused = room
            .send(&room.bea, chat, vec![named(target)], &new_id().to_string())
            .await;
        assert_eq!(refused.problem().code, ErrorCode::RequestInvalid);
    }
    assert!(room.sync(&room.ada, room.chat).await.messages.is_empty());
}

#[tokio::test]
async fn a_human_all_mentions_only_current_followers_and_a_duplicate_does_not_expand_again() {
    let room = Room::new().await;
    for who in [&room.bea, &room.gus] {
        room.world
            .app
            .call(ChatStateSet, &[&room.chat])
            .token(&who.token)
            .body(&ChatStateSetBody {
                level: Some(Some(ChatLevel::All)),
                hidden: None,
            })
            .send()
            .await
            .ok();
    }
    let followers = choices(&room, &room.ada, room.chat, Some("@all")).await;
    assert_eq!(followers.iter().map(|choice| choice.count).sum::<i64>(), 2);
    let sent = room
        .send(
            &room.ada,
            room.chat,
            vec![text("@all please look")],
            "followers",
        )
        .await
        .ok()
        .message;
    let mut expected = vec![room.bea.id(), room.gus.id()];
    expected.sort_unstable();
    assert_eq!(ids(&sent), expected);
    assert!(sent.mentions_all);
    room.world
        .app
        .call(ChatStateSet, &[&room.chat])
        .token(&room.cai.token)
        .body(&ChatStateSetBody {
            level: Some(Some(ChatLevel::All)),
            hidden: None,
        })
        .send()
        .await
        .ok();
    let repeated = room
        .send(
            &room.ada,
            room.chat,
            vec![text("@all please look")],
            "followers",
        )
        .await
        .ok()
        .message;
    assert_eq!(repeated.id, sent.id);
    assert_eq!(ids(&repeated), expected);
}

#[tokio::test]
async fn a_deliberate_group_audience_is_not_cut_to_a_five_hundred_person_roster_page() {
    let room = Room::new().await;
    let crew = group(&room, "Large crew", &[]).await;
    let mut people = Vec::new();
    let mut conn = room.world.conn().await;
    // One past the former roster bound proves complete expansion, without an unbounded stress run.
    for n in 0..501 {
        let id = new_id();
        people.push(id);
        orch_db::queries::identity::testing::insert_user(&mut conn, id, &format!("reader{n}"))
            .await
            .unwrap();
        orch_db::queries::access::testing::insert_member(&mut conn, room.root, id, room.root)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members (group_id, user_id, added_by) VALUES ($1, $2, $3)")
            .bind(crew)
            .bind(id)
            .bind(room.ada.id())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    drop(conn);
    let change: PageChange = serde_json::from_value(json!({ "rows": [{
        "permission":"chat.view", "add":[{"holder":{"kind":"group","id":crew}}]
    }] }))
    .unwrap();
    room.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &room.topic])
        .token(&room.ada.token)
        .body(&change)
        .send()
        .await
        .ok();
    let found = choices(&room, &room.ada, room.chat, Some("Large crew")).await;
    assert_eq!(found[0].count, 501);
    let sent = room
        .send(
            &room.ada,
            room.chat,
            vec![named(ChatMentionTarget::Group { id: crew })],
            "everyone-in-group",
        )
        .await
        .ok()
        .message;
    people.sort_unstable();
    assert_eq!(ids(&sent), people);
}
