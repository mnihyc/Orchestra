//! Direct and group chats over HTTP: opened within a root org, who is in them and who may read them, guests from outside, the lists of an org, and what an admin who reads them leaves behind.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::{Room, text};
use chrono::Duration;
use orch_api_contract::chats::{
    Chat, ChatCanReason, ChatGet, ChatKind, ChatLevel, ChatListScope, ChatMemberAdd,
    ChatMemberRemove, ChatMessageKind, ChatMessageList, ChatMessageListQuery, ChatRead,
    ChatReadBody, ChatStateSet, ChatStateSetBody, ChatSystemEvent, ChatUpdate, ChatUpdateBody,
    OrgChatCreate, OrgChatCreateBody, OrgChatList, OrgChatListQuery,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeGet;
use orch_db::queries::access::testing::remove_member;
use places::Who;
use serde_json::json;

async fn members_of(chat: &Chat) -> Vec<String> {
    chat.members
        .as_ref()
        .unwrap()
        .iter()
        .map(|user| user.username.clone())
        .collect()
}

async fn get(room: &Room, by: &Who, chat: orch_types::Id) -> orch_api_core::testing::Reply<Chat> {
    room.world
        .app
        .call(ChatGet, &[&chat])
        .token(&by.token)
        .send()
        .await
}

async fn list(
    room: &Room,
    by: &Who,
    query: OrgChatListQuery,
) -> orch_api_core::testing::Reply<orch_api_contract::common::Page<Chat>> {
    room.world
        .app
        .call(OrgChatList, &[&room.root])
        .token(&by.token)
        .query(&query)
        .send()
        .await
}

fn scope(scope: ChatListScope) -> OrgChatListQuery {
    OrgChatListQuery {
        scope: Some(scope),
        ..OrgChatListQuery::default()
    }
}

// ---------------------------------------------------------------- direct chats

#[tokio::test]
async fn two_people_have_one_direct_chat_in_an_org_wherever_they_open_it_from() {
    let room = Room::new().await;
    let opened = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.bea.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.cai.id()],
            name: None,
        })
        .send()
        .await;
    assert_eq!(opened.status, 201);
    let chat = opened.ok();
    assert_eq!(chat.base.kind, ChatKind::Direct);
    assert_eq!(chat.base.org_id, room.root);
    assert_eq!(members_of(&chat).await, ["bea", "cai"]);
    assert!(chat.base.node.is_none() && chat.base.name.is_none());
    assert!(chat.can.send && !chat.can.moderate && chat.can.reason.is_none());
    let mine = chat.my.clone().unwrap();
    assert_eq!(
        mine.effective_level,
        ChatLevel::All,
        "the people of a direct chat follow it"
    );
    assert!(!mine.engaged);

    let again = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.cai.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.bea.id()],
            name: None,
        })
        .send()
        .await;
    assert_eq!(again.status, 200, "it exists already");
    assert_eq!(again.ok().base.id, chat.base.id);
    assert_eq!(
        room.direct(&room.bea, &room.cai).await.base.id,
        chat.base.id
    );
}

#[tokio::test]
async fn a_direct_chat_is_read_and_written_by_its_two_people_and_does_not_exist_for_another_member()
{
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.dee).await.base.id;
    let said = room.say(&room.bea, chat, "psst").await;
    assert_eq!(
        Room::words(&room.say(&room.dee, chat, "yes?").await),
        "yes?"
    );
    assert_eq!(room.sync(&room.dee, chat).await.messages.len(), 2);
    for bystander in [&room.cai, &room.eve] {
        let refused = get(&room, bystander, chat).await.problem();
        assert_eq!(refused.code, ErrorCode::NotFound);
        let post = room
            .send(bystander, chat, vec![text("me too")], "x")
            .await
            .problem();
        assert_eq!(post.code, ErrorCode::NotFound);
    }
    // Nobody deletes what another wrote in it.
    let deleted = room
        .world
        .app
        .call(
            orch_api_contract::chats::ChatMessageDelete,
            &[&chat, &said.id],
        )
        .token(&room.dee.token)
        .send()
        .await
        .problem();
    assert_eq!(deleted.code, ErrorCode::AuthzDenied);
    // A person is not their own other person.
    let alone = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.bea.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.bea.id()],
            name: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(alone.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_direct_message_is_told_to_the_other_person_at_their_default_and_not_the_inspector() {
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.bea, chat, "lunch?").await;
    let told = room.inbox(&room.cai, "chat.message").await;
    assert_eq!(
        told.len(),
        1,
        "a direct chat is followed at `all` by default"
    );
    assert!(told[0].0.contains("bea"), "{:?}", told[0]);
    assert!(room.inbox(&room.bea, "chat.message").await.is_empty());
    let open = room.chat(&room.dee, chat).await;
    assert!(open.can.reason == Some(ChatCanReason::Inspecting));
    assert!(room.inbox(&room.dee, "chat.message").await.is_empty());
    assert!(room.inbox(&room.ada, "chat.message").await.is_empty());
}

// ---------------------------------------------------------------- what an admin leaves behind

#[tokio::test]
async fn an_admin_reads_the_chats_among_the_orgs_people_and_cannot_write_in_them() {
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.bea, chat, "private matter").await;
    for admin in [&room.dee, &room.ada] {
        let seen = get(&room, admin, chat).await.ok();
        assert!(!seen.can.send && !seen.can.moderate);
        assert_eq!(seen.can.reason, Some(ChatCanReason::Inspecting));
        assert!(seen.my.is_none(), "they have no state there");
        assert_eq!(members_of(&seen).await, ["bea", "cai"]);
        assert_eq!(room.sync(admin, chat).await.messages.len(), 1);
    }
    let post = room
        .send(&room.dee, chat, vec![text("hi")], "x")
        .await
        .problem();
    assert_eq!(post.code, ErrorCode::AuthzDenied);
    for refused in [
        room.world
            .app
            .call(ChatRead, &[&chat])
            .token(&room.dee.token)
            .body(&ChatReadBody {
                seq: 1,
                read_revision: 0,
                unread_from: None,
            })
            .send()
            .await
            .problem(),
        room.world
            .app
            .call(ChatStateSet, &[&chat])
            .token(&room.dee.token)
            .body(&ChatStateSetBody {
                level: Some(Some(ChatLevel::All)),
                hidden: None,
            })
            .send()
            .await
            .problem(),
    ] {
        assert_eq!(
            refused.code,
            ErrorCode::AuthzDenied,
            "an admin has no read position there"
        );
    }
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chat_states WHERE user_id = $1 AND chat_id = $2")
            .bind(room.dee.id())
            .bind(chat)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn opening_a_chat_one_is_not_in_leaves_one_audit_row_for_each_opening_and_nothing_else() {
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.bea, chat, "private matter").await;
    let before = (
        room.inbox(&room.bea, "chat.message").await.len(),
        room.inbox(&room.cai, "chat.message").await.len(),
    );
    // One opening of the chat is one trace, however many reads the screen makes for it: its
    // page, its sync and older messages all carry the opening's identifier, as a browser sends it.
    let opening = orch_types::new_id().to_string();
    for _ in 0..3 {
        room.world
            .app
            .call(ChatGet, &[&chat])
            .token(&room.dee.token)
            .header("x-orch-opening", &opening)
            .send()
            .await
            .ok();
        room.world
            .app
            .call(orch_api_contract::chats::ChatSyncGet, &[&chat])
            .token(&room.dee.token)
            .header("x-orch-opening", &opening)
            .query(&orch_api_contract::chats::ChatSyncGetQuery::default())
            .send()
            .await
            .ok();
        room.world
            .app
            .call(ChatMessageList, &[&chat])
            .token(&room.dee.token)
            .header("x-orch-opening", &opening)
            .query(&ChatMessageListQuery {
                before: Some(99),
                ..ChatMessageListQuery::default()
            })
            .send()
            .await
            .ok();
    }
    let inspected = || async {
        sqlx::query_as::<_, (String, serde_json::Value)>(
            "SELECT actor_id::text, details FROM audit_log WHERE action = 'chat.inspected' AND object_id = $1 ORDER BY at, id",
        )
        .bind(chat)
        .fetch_all(&room.world.db.pool)
        .await
        .unwrap()
    };
    let rows = inspected().await;
    assert_eq!(rows.len(), 1, "once for the opening");
    assert_eq!(rows[0].0, room.dee.id().to_string());
    // Another admin's opening is another row, and so is a later opening (a read without the
    // header is an opening of its own); being in the chat is none.
    get(&room, &room.ada, chat).await.ok();
    assert_eq!(inspected().await.len(), 2);
    room.world.advance(Duration::days(1));
    get(&room, &room.dee, chat).await.ok();
    assert_eq!(inspected().await.len(), 3);
    get(&room, &room.bea, chat).await.ok();
    assert_eq!(inspected().await.len(), 3);
    // And nobody was told anything.
    assert_eq!(
        (
            room.inbox(&room.bea, "chat.message").await.len(),
            room.inbox(&room.cai, "chat.message").await.len(),
        ),
        before
    );
    let messages: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id = $1")
        .bind(chat)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(messages, 1, "no message either");
}

#[tokio::test]
async fn an_admin_of_a_department_reads_the_chats_among_its_own_people_and_no_other() {
    let room = Room::new().await;
    let sales = places::make_department(&room.world, &room.ada, room.root, "Sales", "sales").await;
    let sales = sales.base.id;
    let lead = places::person(&room.world, "lea").await;
    let sal = places::person(&room.world, "sal").await;
    let sam = places::person(&room.world, "sam").await;
    for (who, role) in [(&lead, "admin"), (&sal, "viewer"), (&sam, "viewer")] {
        places::join(&room.world, room.root, sales, who, role, &room.ada).await;
    }
    let within = room.direct(&sal, &sam).await.base.id;
    let across = room.direct(&sal, &room.cai).await.base.id;
    assert_eq!(
        get(&room, &lead, within).await.ok().can.reason,
        Some(ChatCanReason::Inspecting)
    );
    assert_eq!(
        get(&room, &lead, across).await.problem().code,
        ErrorCode::NotFound
    );

    let others = list(
        &room,
        &lead,
        OrgChatListQuery {
            scope: Some(ChatListScope::Others),
            of: Some(sales),
            ..OrgChatListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(
        others
            .items
            .iter()
            .map(|chat| chat.base.id)
            .collect::<Vec<_>>(),
        [within]
    );
    // Without `of`, the highest org of the tree on which they hold it.
    let default = list(&room, &lead, scope(ChatListScope::Others)).await.ok();
    assert_eq!(default.items.len(), 1);
    // The root org's admin lists both.
    let all = list(&room, &room.ada, scope(ChatListScope::Others))
        .await
        .ok();
    let mut found: Vec<_> = all.items.iter().map(|chat| chat.base.id).collect();
    found.sort();
    let mut both = vec![within, across];
    both.sort();
    assert_eq!(found, both);
    // Somebody who holds directs.view nowhere is shown none of them: every member sees the section,
    // and it lists what the reader may read.
    let none = list(&room, &room.bea, scope(ChatListScope::Others))
        .await
        .ok();
    assert!(none.items.is_empty(), "{:?}", none.items);
    // An org inside a root org has no chats of its own: they are the tree's.
    let inside = room
        .world
        .app
        .call(OrgChatList, &[&sales])
        .token(&lead.token)
        .query(&OrgChatListQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(inside.code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------- lists

#[tokio::test]
async fn the_chats_of_an_org_are_listed_newest_activity_first_a_page_at_a_time_and_a_hidden_chat_returns_with_its_next_message()
 {
    let room = Room::new().await;
    let with_cai = room.direct(&room.bea, &room.cai).await.base.id;
    let with_dee = room.direct(&room.bea, &room.dee).await.base.id;
    let group = room
        .group(&room.bea, &[&room.cai, &room.dee], Some("Release"))
        .await
        .base
        .id;
    // The clock of a test stands still until it is moved.
    room.say(&room.bea, with_cai, "first").await;
    room.world.advance(Duration::seconds(1));
    room.say(&room.bea, group, "second").await;
    room.world.advance(Duration::seconds(1));
    room.say(&room.bea, with_dee, "third").await;
    room.world.advance(Duration::seconds(1));
    let ids = |page: &orch_api_contract::common::Page<Chat>| {
        page.items
            .iter()
            .map(|chat| chat.base.id)
            .collect::<Vec<_>>()
    };
    let all = list(&room, &room.bea, OrgChatListQuery::default())
        .await
        .ok();
    assert_eq!(ids(&all), [with_dee, group, with_cai]);
    assert!(all.next_cursor.is_none());

    let first = list(
        &room,
        &room.bea,
        OrgChatListQuery {
            limit: Some(2),
            ..OrgChatListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(ids(&first), [with_dee, group]);
    let rest = list(
        &room,
        &room.bea,
        OrgChatListQuery {
            limit: Some(2),
            cursor: first.next_cursor.clone(),
            ..OrgChatListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(ids(&rest), [with_cai]);
    assert!(rest.next_cursor.is_none());

    // Taking one off the list keeps its history, and the next message brings it back.
    room.world
        .app
        .call(ChatStateSet, &[&group])
        .token(&room.bea.token)
        .body(&ChatStateSetBody {
            level: None,
            hidden: Some(true),
        })
        .send()
        .await
        .ok();
    let shorter = list(&room, &room.bea, OrgChatListQuery::default())
        .await
        .ok();
    assert_eq!(ids(&shorter), [with_dee, with_cai]);
    assert_eq!(
        room.sync(&room.bea, group).await.messages.len(),
        2,
        "its history is there"
    );
    assert!(room.chat(&room.bea, group).await.my.unwrap().hidden);
    room.say(&room.cai, group, "back again").await;
    let back = list(&room, &room.bea, OrgChatListQuery::default())
        .await
        .ok();
    assert_eq!(ids(&back)[0], group);
    assert!(!room.chat(&room.bea, group).await.my.unwrap().hidden);

    // The other scopes: not with `of`, and the nodes' chats that have messages.
    let bad = list(
        &room,
        &room.bea,
        OrgChatListQuery {
            of: Some(room.root),
            ..OrgChatListQuery::default()
        },
    )
    .await
    .problem();
    assert_eq!(bad.code, ErrorCode::RequestInvalid);
    assert!(
        list(&room, &room.bea, scope(ChatListScope::Nodes))
            .await
            .ok()
            .items
            .is_empty()
    );
    let topic_chat = room.say(&room.ada, room.chat, "on the topic").await.chat_id;
    let nodes = list(&room, &room.bea, scope(ChatListScope::Nodes))
        .await
        .ok();
    assert_eq!(ids(&nodes), [topic_chat]);
    assert_eq!(nodes.items[0].base.kind, ChatKind::Node);
    // Somebody of no part of the tree reads none of its places' chats, and is shown none.
    assert!(
        list(&room, &room.eve, scope(ChatListScope::Nodes))
            .await
            .ok()
            .items
            .is_empty()
    );
}

// ---------------------------------------------------------------- groups

#[tokio::test]
async fn a_member_makes_a_group_for_the_people_they_name_and_it_has_at_most_the_limit() {
    let room = Room::new().await;
    let group = room
        .group(&room.bea, &[&room.cai, &room.dee], Some("Release"))
        .await;
    assert_eq!(group.base.kind, ChatKind::Group);
    assert_eq!(group.base.name.as_deref(), Some("Release"));
    assert_eq!(group.created_by.as_ref().unwrap().username, "bea");
    let mut names = members_of(&group).await;
    names.sort();
    assert_eq!(names, ["bea", "cai", "dee"]);
    assert!(group.guests.is_empty());
    let sync = room.sync(&room.cai, group.base.id).await;
    assert_eq!(sync.messages.len(), 1);
    let line = &sync.messages[0];
    assert_eq!(line.kind, ChatMessageKind::System);
    assert_eq!(
        line.system.as_ref().unwrap().event,
        ChatSystemEvent::Created
    );
    assert!(line.parts.is_empty() && line.author.is_none());
    assert_eq!(room.audit_of(group.base.id).await, ["chat.created"]);
    assert_eq!(
        room.chat(&room.cai, group.base.id).await.last_message_seq,
        0,
        "a line of the system is no message: nothing becomes unread"
    );

    // A group without a name is named after its members; one name fits 80 characters.
    let unnamed = room.group(&room.bea, &[&room.cai, &room.dee], None).await;
    assert!(unnamed.base.name.is_none());
    for name in ["", "   ", &"x".repeat(81)] {
        let bad = room
            .world
            .app
            .call(OrgChatCreate, &[&room.root])
            .token(&room.bea.token)
            .body(&OrgChatCreateBody {
                user_ids: vec![room.cai.id()],
                name: Some(name.to_owned()),
            })
            .send()
            .await
            .problem();
        assert_eq!(bad.code, ErrorCode::RequestInvalid, "{name:?}");
    }

    // Too many people.
    room.world.set("chat.group_max_members", json!(3)).await;
    let full = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.bea.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.cai.id(), room.dee.id(), room.ada.id()],
            name: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(full.code, ErrorCode::ChatNotAllowed);
    assert_eq!(full.extra["reason"], "full");
    // Somebody who is no member of the org.
    let stranger = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.bea.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.cai.id(), room.eve.id()],
            name: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::ChatNotAllowed);
    assert_eq!(stranger.extra["reason"], "not_member");
    // Nobody, and a stranger asking.
    let nobody = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.bea.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![],
            name: Some("Empty".to_owned()),
        })
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::RequestInvalid);
    let outsider_asks = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.eve.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.bea.id()],
            name: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(outsider_asks.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn where_direct_chats_are_off_nothing_new_is_started_and_what_exists_is_read_only() {
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.bea, chat, "before").await;
    room.world
        .set_at(
            orch_settings::Target::Node(room.root),
            "chat.direct",
            json!("off"),
        )
        .await;
    let started = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.bea.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.dee.id()],
            name: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(started.code, ErrorCode::ChatNotAllowed);
    assert_eq!(started.extra["reason"], "direct_off");
    let post = room
        .send(&room.bea, chat, vec![text("after")], "x")
        .await
        .problem();
    assert_eq!(post.extra["reason"], "direct_off");
    let still = room.chat(&room.cai, chat).await;
    assert!(!still.can.send);
    assert_eq!(still.can.reason, Some(ChatCanReason::Disabled));
    assert_eq!(
        room.sync(&room.cai, chat).await.messages.len(),
        1,
        "what exists stays readable"
    );
}

#[tokio::test]
async fn any_member_adds_another_member_a_member_may_leave_and_the_maker_removes_others() {
    let room = Room::new().await;
    let group = room
        .group(&room.bea, &[&room.cai], Some("Release"))
        .await
        .base
        .id;
    let add = |by: &Who, user: &Who| {
        let call = room
            .world
            .app
            .call(ChatMemberAdd, &[&group, &user.id()])
            .token(&by.token);
        async move { call.send().await }
    };
    // Cai, who did not make it, adds Gus.
    let added = add(&room.cai, &room.gus).await.ok();
    assert_eq!(added.members.as_ref().unwrap().len(), 3);
    let again = add(&room.cai, &room.gus).await.ok();
    assert_eq!(
        again.seq, added.seq,
        "adding somebody who is in changes nothing"
    );
    // Somebody who is not in the group adds nobody and does not know it exists; an admin who reads it
    // as an outsider is not in it either.
    assert_eq!(
        add(&room.eve, &room.eve).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        add(&room.ada, &room.ada).await.problem().code,
        ErrorCode::NotFound
    );

    let lines: Vec<(ChatSystemEvent, Option<String>)> = room
        .sync(&room.bea, group)
        .await
        .messages
        .iter()
        .filter_map(|m| m.system.as_ref())
        .map(|s| (s.event, s.user.as_ref().map(|u| u.username.clone())))
        .collect();
    assert_eq!(
        lines,
        [
            (ChatSystemEvent::Created, None),
            (ChatSystemEvent::MemberAdded, Some("gus".to_owned()))
        ]
    );

    let remove = |by: &Who, user: &Who| {
        let call = room
            .world
            .app
            .call(ChatMemberRemove, &[&group, &user.id()])
            .token(&by.token);
        async move { call.send().await }
    };
    // Only the one who made the group takes another out.
    assert_eq!(
        remove(&room.cai, &room.gus).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(remove(&room.bea, &room.gus).await.status, 204);
    assert_eq!(
        get(&room, &room.gus, group).await.problem().code,
        ErrorCode::NotFound,
        "no longer theirs to read"
    );
    // A member leaves by themself; they can be added again.
    assert_eq!(remove(&room.cai, &room.cai).await.status, 204);
    assert_eq!(
        members_of(&get(&room, &room.bea, group).await.ok()).await,
        ["bea"]
    );
    assert_eq!(
        add(&room.bea, &room.cai).await.ok().members.unwrap().len(),
        2
    );
    let actions = room.audit_of(group).await;
    assert_eq!(
        actions,
        [
            "chat.created",
            "chat.member_added",
            "chat.member_removed",
            "chat.member_removed",
            "chat.member_added"
        ]
    );
    // A direct chat has no people to add, nor a name.
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    let refused = room
        .world
        .app
        .call(ChatMemberAdd, &[&direct, &room.gus.id()])
        .token(&room.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_group_is_renamed_by_a_member_and_null_names_it_after_its_members_again() {
    let room = Room::new().await;
    let group = room
        .group(&room.bea, &[&room.cai], Some("Release"))
        .await
        .base
        .id;
    let rename = |by: &Who, name: Option<&str>| {
        let call = room
            .world
            .app
            .call(ChatUpdate, &[&group])
            .token(&by.token)
            .body(&ChatUpdateBody {
                name: name.map(str::to_owned),
            });
        async move { call.send().await }
    };
    let renamed = rename(&room.cai, Some("Launch")).await.ok();
    assert_eq!(renamed.base.name.as_deref(), Some("Launch"));
    let cleared = rename(&room.bea, None).await.ok();
    assert!(cleared.base.name.is_none());
    assert_eq!(
        rename(&room.dee, Some("Mine")).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        rename(&room.bea, Some("")).await.problem().code,
        ErrorCode::RequestInvalid
    );
    let events: Vec<Option<String>> = room
        .sync(&room.bea, group)
        .await
        .messages
        .iter()
        .filter_map(|m| m.system.as_ref())
        .filter(|s| s.event == ChatSystemEvent::Renamed)
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(events, [Some("Launch".to_owned()), None]);
    let direct = room.direct(&room.bea, &room.dee).await.base.id;
    let nameless = room
        .world
        .app
        .call(ChatUpdate, &[&direct])
        .token(&room.bea.token)
        .body(&ChatUpdateBody {
            name: Some("Us".to_owned()),
        })
        .send()
        .await
        .problem();
    assert_eq!(nameless.code, ErrorCode::RequestInvalid);
}

// ---------------------------------------------------------------- guests and people who left

#[tokio::test]
async fn somebody_from_outside_joins_a_group_as_a_guest_only_when_the_one_who_brings_them_may() {
    let room = Room::new().await;
    let group = room
        .group(&room.cai, &[&room.bea], Some("Release"))
        .await
        .base
        .id;
    let add = |by: &Who, user: &Who| {
        let call = room
            .world
            .app
            .call(ChatMemberAdd, &[&group, &user.id()])
            .token(&by.token);
        async move { call.send().await }
    };
    // A developer holds access.external as shipped.
    let with_guest = add(&room.cai, &room.eve).await.ok();
    assert_eq!(with_guest.guests, [room.eve.id()]);
    assert_eq!(with_guest.members.as_ref().unwrap().len(), 3);

    // The guest reads and writes in the chat, and sees nothing else of the tree.
    room.say(&room.eve, group, "thanks for having me").await;
    assert_eq!(room.sync(&room.eve, group).await.messages.len(), 3);
    for node in [room.root, room.project, room.topic] {
        let refused = room
            .world
            .app
            .call(NodeGet, &[&node])
            .token(&room.eve.token)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
    }
    assert!(
        list(&room, &room.eve, OrgChatListQuery::default())
            .await
            .ok()
            .items
            .len()
            == 1,
        "only the chat they are in"
    );
    // A guest adds nobody, starts no chat, and cannot read the org's other chats.
    assert_eq!(
        add(&room.eve, &room.fay).await.problem().code,
        ErrorCode::NotFound
    );
    let starts = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.eve.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.bea.id()],
            name: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(starts.code, ErrorCode::NotFound);
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    assert_eq!(
        get(&room, &room.eve, direct).await.problem().code,
        ErrorCode::NotFound
    );

    // A viewer holds no right to name an outsider; they are told who does.
    let refused = add(&room.bea, &room.fay).await.problem();
    assert_eq!(refused.code, ErrorCode::SharingExternalForbidden);
    assert!(
        refused.extra["ask"]
            .as_array()
            .is_some_and(|ask| !ask.is_empty())
    );
    // The one who made the group may where the policy lets owners.
    let made = room
        .group(&room.bea, &[&room.cai], Some("Mine"))
        .await
        .base
        .id;
    let by_maker = room
        .world
        .app
        .call(ChatMemberAdd, &[&made, &room.fay.id()])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(by_maker.guests, [room.fay.id()]);
    room.world
        .set_at(
            orch_settings::Target::Node(room.root),
            "sharing.allow_external",
            json!(false),
        )
        .await;
    let other = room
        .group(&room.bea, &[&room.cai], Some("Other"))
        .await
        .base
        .id;
    let policy = room
        .world
        .app
        .call(ChatMemberAdd, &[&other, &room.fay.id()])
        .token(&room.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(policy.code, ErrorCode::SharingExternalForbidden);
    // A guest who leaves is out.
    let left = room
        .world
        .app
        .call(ChatMemberRemove, &[&group, &room.eve.id()])
        .token(&room.eve.token)
        .send()
        .await;
    assert_eq!(left.status, 204);
    assert_eq!(
        get(&room, &room.eve, group).await.problem().code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn somebody_who_left_the_org_no_longer_reads_its_chats_and_the_other_person_of_a_direct_chat_is_told_it_is_closed()
 {
    let room = Room::new().await;
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    let group = room
        .group(&room.bea, &[&room.cai, &room.dee], Some("Release"))
        .await
        .base
        .id;
    room.say(&room.bea, direct, "before").await;
    {
        let mut conn = room.world.conn().await;
        remove_member(&mut conn, room.root, room.bea.id())
            .await
            .unwrap();
    }
    assert_eq!(
        get(&room, &room.bea, direct).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        get(&room, &room.bea, group).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        room.sync(&room.cai, direct).await.messages.len(),
        1,
        "their words stay in the history"
    );
    let closed = room
        .send(&room.cai, direct, vec![text("you there?")], "x")
        .await
        .problem();
    assert_eq!(closed.code, ErrorCode::ChatNotAllowed);
    assert_eq!(closed.extra["reason"], "closed");
    assert_eq!(
        room.chat(&room.cai, direct).await.can.reason,
        Some(ChatCanReason::Closed)
    );
    // The group goes on without them.
    room.say(&room.cai, group, "still here").await;
    // A person cannot be started a chat with who is gone.
    let started = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.cai.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.bea.id()],
            name: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(started.code, ErrorCode::ChatNotAllowed);
}
