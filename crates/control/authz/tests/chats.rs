//! Who reads, writes in, moderates and inspects a chat: a node's by its page, a direct or a group chat by membership of its root org or by guest status, and both by whoever holds `directs.view`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::Role;
use orch_authz::chats::{self, Cannot, ChatPolicy, Reading, Standing};
use orch_authz::{Caller, Principal};
use orch_db::queries::access::testing::{insert_member, suspend_in};
use orch_db::queries::chats::{self as rows, ChatRow, MemberRow, NewChat, NewMember};
use orch_types::{Id, new_id};
use support::World;

async fn node_chat(w: &World, node: Id) -> ChatRow {
    let mut conn = w.conn().await;
    rows::chat_of_node(&mut conn, node).await.unwrap().unwrap()
}

/// A direct chat of two people, or a group of several, in the root org; the
/// ones named in `guests` are in it as guests.
async fn people_chat(w: &World, kind: &str, people: &[Id], guests: &[Id], maker: Id) -> ChatRow {
    let mut conn = w.conn().await;
    let mut key = None;
    if kind == rows::KIND_DIRECT {
        let (low, high) = (people[0].min(people[1]), people[0].max(people[1]));
        key = Some(format!("{low}:{high}"));
    }
    let chat = rows::insert_chat(
        &mut conn,
        &NewChat {
            id: new_id(),
            kind,
            org_id: w.root,
            direct_key: key.as_deref(),
            name: None,
            created_by: maker,
            at: w.now.get(),
        },
    )
    .await
    .unwrap()
    .unwrap();
    for person in people.iter().chain(guests) {
        rows::add_member(
            &mut conn,
            &NewMember {
                expires_at: None,
                chat_id: chat.id,
                user_id: *person,
                added_by: Some(maker),
                guest: guests.contains(person),
                seq: 0,
                at: w.now.get(),
            },
        )
        .await
        .unwrap();
    }
    chat
}

async fn members(w: &World, chat: &ChatRow) -> Vec<MemberRow> {
    let mut conn = w.conn().await;
    rows::members(&mut conn, chat.id).await.unwrap()
}

async fn standing(w: &World, user: Id, chat: &ChatRow, policy: ChatPolicy) -> Standing {
    let principal = w.principal(Caller::session(user)).await;
    let members = members(w, chat).await;
    let mut conn = w.conn().await;
    chats::standing(&mut conn, &principal, chat, &members, policy)
        .await
        .unwrap()
}

fn open() -> ChatPolicy {
    ChatPolicy::default()
}

// ---------------------------------------------------------------- the chat of a node

#[tokio::test]
async fn a_viewer_reads_and_writes_in_the_chat_of_a_project_as_shipped() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    w.give(w.root, viewer, Role::Viewer).await;
    let chat = node_chat(&w, w.project).await;
    let standing = standing(&w, viewer, &chat, open()).await;
    assert_eq!(standing.reading, Some(Reading::Page));
    assert_eq!(standing.sending, Ok(()));
    assert!(
        !standing.moderate,
        "deleting what others wrote is for those who hold chat.manage"
    );
}

#[tokio::test]
async fn somebody_who_holds_nothing_on_a_node_does_not_read_its_chat_or_know_its_name() {
    let w = World::new().await;
    let stranger = w.user("sam").await;
    let chat = node_chat(&w, w.topic).await;
    let standing = standing(&w, stranger, &chat, open()).await;
    assert!(!standing.reads());
    assert!(!standing.names_node);
    assert!(standing.sending.is_err() && !standing.moderate);
}

#[tokio::test]
async fn a_chat_whose_page_row_was_cut_loose_is_read_by_those_it_names_only() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    let named = w.member("nora").await;
    w.give(w.root, viewer, Role::Viewer).await;
    w.give(w.root, named, Role::Viewer).await;
    // Holding the right to write gives the right to read, so both rows are cut loose.
    w.row(w.topic, Permission::ChatView, false).await;
    w.row(w.topic, Permission::ChatInteract, false).await;
    w.name(w.topic, Permission::ChatView, Holder::User { id: named })
        .await;
    let chat = node_chat(&w, w.topic).await;
    assert!(!standing(&w, viewer, &chat, open()).await.reads());
    assert_eq!(
        standing(&w, named, &chat, open()).await.reading,
        Some(Reading::Page)
    );
}

#[tokio::test]
async fn a_viewer_who_is_given_no_chat_interact_reads_the_chat_and_is_told_why_nothing_can_be_written()
 {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    w.give(w.root, viewer, Role::Viewer).await;
    w.row(w.topic, Permission::ChatInteract, false).await;
    let chat = node_chat(&w, w.topic).await;
    let standing = standing(&w, viewer, &chat, open()).await;
    assert!(standing.reads());
    assert_eq!(standing.sending, Err(Cannot::NoPermission));
}

#[tokio::test]
async fn a_chat_under_an_archived_node_is_read_only_and_names_the_node_that_is_archived() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    w.give(w.root, viewer, Role::Viewer).await;
    {
        let mut conn = w.conn().await;
        orch_db::queries::hierarchy::archive(&mut conn, w.project, w.boss, w.now.get())
            .await
            .unwrap();
    }
    let chat = node_chat(&w, w.topic).await;
    let standing = standing(&w, viewer, &chat, open()).await;
    assert!(standing.reads(), "the history stays");
    assert_eq!(standing.sending, Err(Cannot::Archived { node: w.project }));
    assert!(
        !standing.moderate,
        "nothing is deleted from an archived chat"
    );
}

#[tokio::test]
async fn where_chats_are_off_the_history_is_read_and_nothing_is_written() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    w.give(w.root, viewer, Role::Viewer).await;
    let chat = node_chat(&w, w.topic).await;
    let off = ChatPolicy {
        enabled: false,
        direct: true,
    };
    let standing = standing(&w, viewer, &chat, off).await;
    assert!(standing.reads());
    assert_eq!(standing.sending, Err(Cannot::Disabled));
}

#[tokio::test]
async fn somebody_named_in_the_chat_rows_alone_reads_and_writes_the_chat_and_knows_the_nodes_name_and_holds_nothing_else()
 {
    let w = World::new().await;
    let guest = w.user("gwen").await;
    for permission in [Permission::ChatView, Permission::ChatInteract] {
        w.name(w.topic, permission, Holder::User { id: guest })
            .await;
    }
    let chat = node_chat(&w, w.topic).await;
    let standing = standing(&w, guest, &chat, open()).await;
    assert_eq!(standing.reading, Some(Reading::Page));
    assert_eq!(standing.sending, Ok(()));
    assert!(standing.names_node, "they know the name of the topic");
    // and nothing else of it: not the topic, nor the thread in it, nor the project above.
    assert!(!w.can(guest, w.topic, Permission::TopicView).await);
    assert!(!w.can(guest, w.thread, Permission::ThreadView).await);
    assert!(!w.can(guest, w.project, Permission::ProjectView).await);
    assert!(!w.can(guest, w.project, Permission::ChatView).await);
}

#[tokio::test]
async fn only_those_who_hold_chat_manage_delete_the_messages_of_others() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    let developer = w.member("dave").await;
    w.give(w.root, viewer, Role::Viewer).await;
    w.give(w.root, developer, Role::Developer).await;
    let chat = node_chat(&w, w.topic).await;
    assert!(!standing(&w, viewer, &chat, open()).await.moderate);
    assert!(standing(&w, developer, &chat, open()).await.moderate);
    assert!(standing(&w, w.boss, &chat, open()).await.moderate);
}

#[tokio::test]
async fn a_person_suspended_in_the_org_is_refused_its_chats() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    w.give(w.root, viewer, Role::Viewer).await;
    {
        let mut conn = w.conn().await;
        suspend_in(&mut conn, w.root, viewer, w.boss).await.unwrap();
    }
    let chat = node_chat(&w, w.topic).await;
    let principal = w.principal(Caller::session(viewer)).await;
    let members = members(&w, &chat).await;
    let mut conn = w.conn().await;
    let refused = chats::standing(&mut conn, &principal, &chat, &members, open()).await;
    assert!(matches!(
        refused,
        Err(orch_authz::Error::Suspended { org: Some(org) }) if org == w.root
    ));
}

// ---------------------------------------------------------------- direct and group chats

#[tokio::test]
async fn the_two_people_of_a_direct_chat_read_and_write_in_it_and_another_member_reads_nothing() {
    let w = World::new().await;
    let (ann, bea, cat) = (
        w.member("ann").await,
        w.member("bea").await,
        w.member("cat").await,
    );
    let chat = people_chat(&w, rows::KIND_DIRECT, &[ann, bea], &[], ann).await;
    for person in [ann, bea] {
        let standing = standing(&w, person, &chat, open()).await;
        assert_eq!(standing.reading, Some(Reading::Member));
        assert_eq!(standing.sending, Ok(()));
        assert!(standing.in_chat);
        assert!(!standing.moderate, "nobody deletes what another wrote");
    }
    assert!(!standing(&w, cat, &chat, open()).await.reads());
}

#[tokio::test]
async fn an_admin_of_the_root_org_reads_a_direct_chat_and_cannot_write_in_it() {
    let w = World::new().await;
    let (ann, bea) = (w.member("ann").await, w.member("bea").await);
    let chat = people_chat(&w, rows::KIND_DIRECT, &[ann, bea], &[], ann).await;
    let standing = standing(&w, w.boss, &chat, open()).await;
    assert_eq!(
        standing.reading,
        Some(Reading::Inspector { org: w.root }),
        "boss owns the root org and so is its admin"
    );
    assert_eq!(standing.sending, Err(Cannot::Inspecting));
    assert!(!standing.in_chat && !standing.moderate);
}

#[tokio::test]
async fn an_admin_of_a_department_reads_the_chats_among_its_own_people_and_no_chat_somebody_of_another_department_is_in()
 {
    let w = World::new().await;
    let sales = w.node("org", w.root, "Sales").await;
    let engineering = w.node("org", w.root, "Engineering").await;
    let (sal, sam, eng) = (
        w.user("sal").await,
        w.user("sam").await,
        w.user("eng").await,
    );
    {
        let mut conn = w.conn().await;
        insert_member(&mut conn, w.root, sal, sales).await.unwrap();
        insert_member(&mut conn, w.root, sam, sales).await.unwrap();
        insert_member(&mut conn, w.root, eng, engineering)
            .await
            .unwrap();
    }
    let lead = w.user("lead").await;
    {
        let mut conn = w.conn().await;
        insert_member(&mut conn, w.root, lead, sales).await.unwrap();
    }
    w.give(sales, lead, Role::Admin).await;
    let within = people_chat(&w, rows::KIND_DIRECT, &[sal, sam], &[], sal).await;
    let across = people_chat(&w, rows::KIND_DIRECT, &[sal, eng], &[], sal).await;
    let guests = {
        let guest = w.user("gus").await;
        people_chat(&w, rows::KIND_GROUP, &[sal, sam], &[guest], sal).await
    };
    assert_eq!(
        standing(&w, lead, &within, open()).await.reading,
        Some(Reading::Inspector { org: sales })
    );
    assert!(
        !standing(&w, lead, &across, open()).await.reads(),
        "somebody of another department is in it"
    );
    assert!(
        !standing(&w, lead, &guests, open()).await.reads(),
        "a guest is in it"
    );
    // The root org's admin reads all three.
    for chat in [&within, &across, &guests] {
        assert!(standing(&w, w.boss, chat, open()).await.inspects());
    }
}

#[tokio::test]
async fn a_guest_of_a_group_reads_and_writes_it_and_is_no_inspector_of_anything() {
    let w = World::new().await;
    let (ann, bea) = (w.member("ann").await, w.member("bea").await);
    let guest = w.user("gus").await;
    let chat = people_chat(&w, rows::KIND_GROUP, &[ann, bea], &[guest], ann).await;
    let standing = standing(&w, guest, &chat, open()).await;
    assert_eq!(standing.reading, Some(Reading::Guest));
    assert_eq!(standing.sending, Ok(()));
    // They see nothing else of the tree.
    assert!(!w.can(guest, w.root, Permission::OrgView).await);
    assert!(!w.can(guest, w.topic, Permission::ChatView).await);
}

#[tokio::test]
async fn somebody_who_left_the_org_no_longer_reads_its_chats_and_a_direct_chat_with_them_takes_no_message()
 {
    let w = World::new().await;
    let (ann, bea) = (w.member("ann").await, w.member("bea").await);
    let chat = people_chat(&w, rows::KIND_DIRECT, &[ann, bea], &[], ann).await;
    {
        let mut conn = w.conn().await;
        orch_db::queries::access::testing::remove_member(&mut conn, w.root, bea)
            .await
            .unwrap();
    }
    assert!(
        !standing(&w, bea, &chat, open()).await.reads(),
        "they stay in the history, and no longer read"
    );
    let ann_sees = standing(&w, ann, &chat, open()).await;
    assert!(ann_sees.reads());
    assert_eq!(ann_sees.sending, Err(Cannot::Closed));
}

#[tokio::test]
async fn a_direct_chat_with_somebody_the_system_suspended_takes_no_message() {
    let w = World::new().await;
    let (ann, bea) = (w.member("ann").await, w.member("bea").await);
    let chat = people_chat(&w, rows::KIND_DIRECT, &[ann, bea], &[], ann).await;
    {
        let mut conn = w.conn().await;
        orch_db::queries::identity::testing::suspend(&mut conn, bea)
            .await
            .unwrap();
    }
    assert_eq!(
        standing(&w, ann, &chat, open()).await.sending,
        Err(Cannot::Closed)
    );
}

#[tokio::test]
async fn a_group_whose_last_member_has_left_stays_readable_to_the_root_orgs_admins_alone() {
    let w = World::new().await;
    let sales = w.node("org", w.root, "Sales").await;
    let (ann, lead) = (w.member("ann").await, w.user("lead").await);
    {
        let mut conn = w.conn().await;
        insert_member(&mut conn, w.root, lead, sales).await.unwrap();
    }
    w.give(sales, lead, Role::Admin).await;
    let chat = people_chat(&w, rows::KIND_GROUP, &[ann], &[], ann).await;
    {
        let mut conn = w.conn().await;
        rows::remove_member(&mut conn, chat.id, ann, 1, w.now.get())
            .await
            .unwrap();
    }
    assert!(standing(&w, w.boss, &chat, open()).await.inspects());
    assert!(!standing(&w, lead, &chat, open()).await.reads());
    assert!(
        !standing(&w, ann, &chat, open()).await.reads(),
        "and the one who left reads it no more"
    );
}

#[tokio::test]
async fn direct_and_group_chats_are_read_only_where_chat_direct_is_off_and_where_the_root_org_is_archived()
 {
    let w = World::new().await;
    let (ann, bea) = (w.member("ann").await, w.member("bea").await);
    let chat = people_chat(&w, rows::KIND_GROUP, &[ann, bea], &[], ann).await;
    let off = ChatPolicy {
        enabled: true,
        direct: false,
    };
    let held = standing(&w, ann, &chat, off).await;
    assert!(held.reads());
    assert_eq!(held.sending, Err(Cannot::DirectOff));
    {
        let mut conn = w.conn().await;
        orch_db::queries::hierarchy::archive(&mut conn, w.root, w.boss, w.now.get())
            .await
            .unwrap();
    }
    let archived = standing(&w, ann, &chat, open()).await;
    assert!(archived.reads());
    assert_eq!(archived.sending, Err(Cannot::Archived { node: w.root }));
}

// ---------------------------------------------------------------- who is named

#[tokio::test]
async fn a_mention_names_only_people_who_can_read_the_chat() {
    let w = World::new().await;
    let reader = w.member("rae").await;
    let stranger = w.user("sam").await;
    w.give(w.root, reader, Role::Viewer).await;
    let node = node_chat(&w, w.topic).await;
    let mut conn = w.conn().await;
    let read = chats::readers_among(&mut conn, &node, &[], &[reader, stranger], w.now)
        .await
        .unwrap();
    assert_eq!(read, vec![reader]);

    let (ann, bea) = (w.member("ann").await, w.member("bea").await);
    let outsider = w.user("olga").await;
    let chat = people_chat(&w, rows::KIND_GROUP, &[ann], &[], ann).await;
    let members = members(&w, &chat).await;
    let read = chats::readers_among(&mut conn, &chat, &members, &[ann, bea, outsider], w.now)
        .await
        .unwrap();
    assert_eq!(
        read,
        vec![ann],
        "bea is a member of the org and not in the group"
    );
}

#[tokio::test]
async fn a_developer_may_bring_an_outsider_into_a_group_and_a_viewer_may_not_unless_they_made_it_where_the_policy_lets()
 {
    let w = World::new().await;
    let developer = w.member("dave").await;
    let viewer = w.member("vera").await;
    w.give(w.root, developer, Role::Developer).await;
    w.give(w.root, viewer, Role::Viewer).await;
    let chat = people_chat(&w, rows::KIND_GROUP, &[developer, viewer], &[], viewer).await;
    let ask = |user: Id, lets: bool| {
        let (w, chat) = (&w, &chat);
        async move {
            let principal: Principal = w.principal(Caller::session(user)).await;
            let mut conn = w.conn().await;
            chats::may_bring_in_outsider(&mut conn, &principal, chat, lets)
                .await
                .unwrap()
        }
    };
    assert!(ask(developer, false).await.allowed());
    let refused = ask(viewer, false).await;
    assert!(!refused.allowed());
    assert!(
        !refused.ask.is_empty(),
        "the option is shown disabled, with who can do it"
    );
    assert!(
        ask(viewer, true).await.allowed(),
        "the one who made the group, where sharing.allow_external lets"
    );
}

// ---------------------------------------------------------------- tokens

#[tokio::test]
async fn a_token_reads_and_writes_in_the_chats_among_people_only_within_its_scopes_and_outside_any_subtree()
 {
    use orch_access_contract::decisions::Scopes;
    use orch_access_contract::permissions::Permission as P;
    let w = World::new().await;
    let (ann, bea) = (w.member("ann").await, w.member("bea").await);
    let chat = people_chat(&w, rows::KIND_DIRECT, &[ann, bea], &[], ann).await;
    let members = members(&w, &chat).await;
    let with = |scopes: &[P], subtree: Option<Id>| {
        Caller::token(
            ann,
            orch_authz::TokenGrant::new(Scopes::Only(scopes.iter().copied().collect()), subtree),
        )
    };
    let standing_of = |caller: Caller| {
        let (w, chat, members) = (&w, &chat, &members);
        async move {
            let principal = w.principal(caller).await;
            let mut conn = w.conn().await;
            chats::standing(&mut conn, &principal, chat, members, open())
                .await
                .unwrap()
        }
    };
    let both = standing_of(with(&[P::ChatView, P::ChatInteract], None)).await;
    assert_eq!(both.reading, Some(Reading::Member));
    assert_eq!(both.sending, Ok(()));
    let reads = standing_of(with(&[P::ChatView], None)).await;
    assert_eq!(reads.reading, Some(Reading::Member));
    assert_eq!(
        reads.sending,
        Err(Cannot::NoPermission),
        "it carries no right to write"
    );
    let nothing = standing_of(with(&[P::ChatInteract], None)).await;
    assert!(
        !nothing.reads(),
        "holding a level does not put the one below it into the scopes"
    );
    let kept = standing_of(with(&[P::ChatView, P::ChatInteract], Some(w.project))).await;
    assert!(
        !kept.reads(),
        "the chats among people lie in no subtree of the org"
    );
}
