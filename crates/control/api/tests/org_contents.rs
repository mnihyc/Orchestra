//! Everything an org holds, a level at a time, for whoever administers it: every kind in one tree with whose a
//! reader may see, and a shared thing's ancestors shown by the same redacted references as the finder.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::chats::{
    ChatGet, ChatLevel, ChatListScope, ChatMessageCreate, ChatMessageCreateBody, ChatPart,
    ChatStateSet, ChatStateSetBody, OrgChatList, OrgChatListQuery,
};
use orch_api_contract::common::{ErrorCode, Page, SubscriptionLevel};
use orch_api_contract::directory::{NodeTagCreate, NodeTagCreateBody};
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::everything::{
    EverythingAccess, EverythingDirection, EverythingKind, EverythingList, EverythingListQuery,
    EverythingRecent, EverythingRow, EverythingScope, EverythingSort,
};
use orch_api_contract::nodes::{
    NewNodeAccess, Node, NodeGet, NodeSubscriptionSet, NodeSubscriptionSetBody, OrgTreeGet,
    OrgTreeGetQuery, ProjectCreate, ProjectCreateBody,
};
use orch_api_contract::search::{NameSearchList, NameSearchListQuery, SearchList, SearchListQuery};
use orch_db::queries::access::{PageKey, testing as access};
use orch_types::Id;
use places::{Who, admin, give_role, join, make_org, make_project, make_topic, person};
use support::World;

async fn listing(world: &World, by: &Who, query: &EverythingListQuery) -> Page<EverythingRow> {
    world
        .app
        .call(EverythingList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

fn branch(parent: Id) -> EverythingListQuery {
    EverythingListQuery {
        parent: Some(parent.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn recent_opened_crosses_levels_by_visit_time_and_binds_its_keyset_to_the_view() {
    let world = World::new().await;
    let owner = person(&world, "recent-owner").await;
    let reader = person(&world, "recent-reader").await;
    let root = make_org(&world, &owner, "Recent team", "recent-team")
        .await
        .base
        .id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let project = make_project(&world, &owner, root, "Zulu").await.base.id;
    let topic = make_topic(&world, &owner, project, "Alpha").await.base.id;
    let other = make_project(&world, &owner, root, "Outside").await.base.id;
    let private = private_project(&world, &owner, root).await;
    let mut conn = world.conn().await;
    for (kind, id, seconds) in [
        ("project", project, 10_i64),
        ("topic", topic, 20),
        ("project", other, 30),
        ("project", private, 40),
    ] {
        sqlx::query("INSERT INTO recents(user_id,kind,id,visited_at) VALUES($1,$2,$3,$4) ON CONFLICT(user_id,kind,id) DO UPDATE SET visited_at=excluded.visited_at")
            .bind(reader.id()).bind(kind).bind(id).bind(world.now()+chrono::Duration::seconds(seconds))
            .execute(&mut *conn).await.unwrap();
    }
    drop(conn);
    let ask = EverythingListQuery {
        recent: Some(EverythingRecent::Opened),
        limit: Some(1),
        ..branch(root)
    };
    let first = listing(&world, &reader, &ask).await;
    assert_eq!(
        first
            .items
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        [other.to_string()]
    );
    assert!(first.items[0].last_opened.is_some());
    let second = listing(
        &world,
        &reader,
        &EverythingListQuery {
            cursor: first.next_cursor.clone(),
            ..ask.clone()
        },
    )
    .await;
    assert_eq!(second.items[0].id, topic.to_string());
    let third = listing(
        &world,
        &reader,
        &EverythingListQuery {
            cursor: second.next_cursor.clone(),
            ..ask.clone()
        },
    )
    .await;
    assert_eq!(third.items[0].id, project.to_string());
    assert!(third.next_cursor.is_none());
    let scoped = listing(
        &world,
        &reader,
        &EverythingListQuery {
            parent: Some(project.to_string()),
            limit: Some(20),
            ..ask.clone()
        },
    )
    .await;
    assert_eq!(
        scoped
            .items
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        [topic.to_string()]
    );
    let names = listing(
        &world,
        &reader,
        &EverythingListQuery {
            sort: Some(EverythingSort::Name),
            direction: Some(EverythingDirection::Asc),
            limit: Some(20),
            ..ask.clone()
        },
    )
    .await;
    assert_eq!(
        names.items[0].id,
        topic.to_string(),
        "an explicit column sort wins over recency"
    );
    let invalid = world
        .app
        .call(EverythingList, &[])
        .token(&reader.token)
        .query(&EverythingListQuery {
            cursor: first.next_cursor,
            recent: Some(EverythingRecent::Active),
            ..ask
        })
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn recent_active_respects_kinds_archived_done_and_access_on_each_page() {
    let world = World::new().await;
    let owner = person(&world, "active-owner").await;
    let reader = person(&world, "active-reader").await;
    let root = make_org(&world, &owner, "Active team", "active-team")
        .await
        .base
        .id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let project = make_project(&world, &owner, root, "Work").await.base.id;
    let topic = make_topic(&world, &owner, project, "Plans").await.base.id;
    let (one, two) = (orch_types::new_id(), orch_types::new_id());
    let mut conn = world.conn().await;
    for (id, name) in [(one, "One"), (two, "Two")] {
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            id,
            "thread",
            Some(topic),
            name,
            owner.id(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE nodes SET created_at=$2,updated_at=$2 WHERE id=$1")
            .bind(id)
            .bind(world.now())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE threads SET last_activity_at=$2 WHERE node_id=$1")
        .bind(two)
        .bind(world.now() + chrono::Duration::seconds(10))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let ask = EverythingListQuery {
        recent: Some(EverythingRecent::Active),
        navigation: Some(true),
        kinds: Some("thread,document".into()),
        show_archived: Some(false),
        limit: Some(1),
        ..branch(project)
    };
    let first = listing(&world, &reader, &ask).await;
    assert_eq!(first.items[0].id, two.to_string());
    assert!(first.items[0].last_opened.is_none());
    let mut conn = world.conn().await;
    access::set_row(
        &mut conn,
        &PageKey::new("node", one, "live"),
        "any",
        "thread.view",
        false,
        false,
    )
    .await
    .unwrap();
    // Chat Interact also grants View; every inherited level must stop for this private locator.
    for permission in ["chat.view", "chat.interact", "chat.manage"] {
        access::set_row(
            &mut conn,
            &PageKey::new("node", one, "live"),
            "any",
            permission,
            false,
            false,
        )
        .await
        .unwrap();
    }
    drop(conn);
    let cut = listing(
        &world,
        &reader,
        &EverythingListQuery {
            cursor: first.next_cursor,
            ..ask.clone()
        },
    )
    .await;
    assert!(
        cut.items.is_empty(),
        "revoking access cuts a continued page as well"
    );
    let mut conn = world.conn().await;
    sqlx::query("UPDATE topics SET status='done' WHERE node_id=$1")
        .bind(topic)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(
        listing(
            &world,
            &reader,
            &EverythingListQuery {
                hide_done: Some(true),
                ..ask.clone()
            }
        )
        .await
        .items
        .is_empty()
    );
    assert_eq!(
        listing(
            &world,
            &reader,
            &EverythingListQuery {
                hide_done: Some(true),
                keep: Some(two),
                ..ask.clone()
            }
        )
        .await
        .items[0]
            .id,
        two.to_string()
    );
    assert!(
        listing(
            &world,
            &reader,
            &EverythingListQuery {
                hide_done: Some(true),
                keep: Some(one),
                ..ask.clone()
            }
        )
        .await
        .items
        .is_empty(),
        "an unreadable current locator cannot disclose its ancestor through the done filter"
    );
    let document = world
        .app
        .call(NodeDocumentCreate, &[&one])
        .token(&owner.token)
        .body(&NodeDocumentCreateBody {
            title: "Shared current notes".into(),
            markdown: None,
            blocks: None,
            access: None,
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    access::set_row(
        &mut conn,
        &PageKey::new("document", document.base.id, "live"),
        "any",
        "doc.view",
        false,
        false,
    )
    .await
    .unwrap();
    access::add_entry(
        &mut conn,
        &PageKey::new("document", document.base.id, "live"),
        "any",
        "doc.view",
        ("user", Some(reader.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    for current in [one, document.base.id] {
        let kept = listing(
            &world,
            &reader,
            &EverythingListQuery {
                hide_done: Some(true),
                keep: Some(current),
                limit: Some(50),
                ..ask.clone()
            },
        )
        .await;
        assert_eq!(kept.items.len(), 2);
        assert!(kept.items.iter().any(|row| row.id == two.to_string()));
        assert!(
            kept.items
                .iter()
                .any(|row| row.id == document.base.id.to_string())
        );
        assert!(kept.items.iter().all(|row| row.id != one.to_string()));
    }
    let mut conn = world.conn().await;
    sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
        .bind(topic)
        .bind(world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(listing(&world, &reader, &ask).await.items.is_empty());
    assert_eq!(
        listing(
            &world,
            &reader,
            &EverythingListQuery {
                show_archived: Some(true),
                ..ask.clone()
            }
        )
        .await
        .items[0]
            .id,
        two.to_string()
    );
    assert!(
        listing(
            &world,
            &reader,
            &EverythingListQuery {
                kinds: Some("document".into()),
                ..ask
            }
        )
        .await
        .items
        .is_empty()
    );
}

#[tokio::test]
async fn a_recent_continuation_omits_a_visit_whose_timed_view_grant_has_ended() {
    let world = World::new().await;
    let owner = person(&world, "expiry-owner").await;
    let reader = person(&world, "expiry-reader").await;
    let root = make_org(&world, &owner, "Expiry team", "expiry-team")
        .await
        .base
        .id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let ordinary = make_project(&world, &owner, root, "Ordinary").await.base.id;
    let private = private_project(&world, &owner, root).await;
    let mut conn = world.conn().await;
    access::add_entry(
        &mut conn,
        &PageKey::new("node", private, "live"),
        "any",
        "project.view",
        ("user", Some(reader.id()), None),
        Some(world.now() + chrono::Duration::seconds(30)),
        world.now(),
    )
    .await
    .unwrap();
    for (id, seconds) in [(ordinary, 20_i64), (private, 10)] {
        sqlx::query("INSERT INTO recents(user_id,kind,id,visited_at) VALUES($1,'project',$2,$3)")
            .bind(reader.id())
            .bind(id)
            .bind(world.now() + chrono::Duration::seconds(seconds))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    drop(conn);
    let ask = EverythingListQuery {
        recent: Some(EverythingRecent::Opened),
        limit: Some(1),
        ..branch(root)
    };
    let first = listing(&world, &reader, &ask).await;
    assert_eq!(first.items[0].id, ordinary.to_string());
    assert!(first.next_cursor.is_some());
    world.advance(chrono::Duration::seconds(31));
    let next = listing(
        &world,
        &reader,
        &EverythingListQuery {
            cursor: first.next_cursor,
            ..ask
        },
    )
    .await;
    assert!(next.items.is_empty());
    assert!(next.next_cursor.is_none());
    assert!(
        !serde_json::to_string(&next)
            .unwrap()
            .contains("Private Vault")
    );
}

#[tokio::test]
async fn recent_documents_keep_their_read_marks_and_the_threads_switch_applies_to_their_homes() {
    let world = World::new().await;
    let owner = person(&world, "document-owner").await;
    let reader = person(&world, "document-reader").await;
    let root = make_org(&world, &owner, "Document team", "document-team")
        .await
        .base
        .id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let project = make_project(&world, &owner, root, "Work").await.base.id;
    let topic = make_topic(&world, &owner, project, "Plans").await.base.id;
    let thread = orch_types::new_id();
    let mut conn = world.conn().await;
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        thread,
        "thread",
        Some(topic),
        "Research",
        owner.id(),
    )
    .await
    .unwrap();
    drop(conn);
    let mut documents = Vec::new();
    for (home, title) in [(project, "Project notes"), (thread, "Thread notes")] {
        let document = world
            .app
            .call(NodeDocumentCreate, &[&home])
            .token(&owner.token)
            .body(&NodeDocumentCreateBody {
                title: title.into(),
                markdown: Some("A recorded change".into()),
                blocks: None,
                access: None,
            })
            .send()
            .await
            .ok();
        documents.push(document.base.id);
    }
    let ask = EverythingListQuery {
        recent: Some(EverythingRecent::Active),
        navigation: Some(true),
        kinds: Some("document".into()),
        ..branch(project)
    };
    let without = listing(&world, &reader, &ask).await;
    assert_eq!(without.items.len(), 1);
    assert_eq!(without.items[0].id, documents[0].to_string());
    assert_eq!(without.items[0].document_unread, Some(true));
    assert_eq!(
        without.items[0].activity_by.as_ref().unwrap().id,
        owner.id()
    );
    let with = listing(
        &world,
        &reader,
        &EverythingListQuery {
            kinds: Some("thread,document".into()),
            ..ask
        },
    )
    .await;
    for id in documents {
        assert!(with.items.iter().any(|row| row.id == id.to_string()));
    }
    assert!(with.items.iter().all(|row| row.last_opened.is_none()));
}

async fn private_project(world: &World, by: &Who, root: Id) -> Id {
    world
        .app
        .call(ProjectCreate, &[&root])
        .token(&by.token)
        .body(&ProjectCreateBody {
            name: "Private Vault".to_owned(),
            description: None,
            access: Some(NewNodeAccess::Private),
        })
        .send()
        .await
        .ok()
        .base
        .id
}

async fn sized_environment(
    world: &World,
    owner: &Who,
    root: Id,
    spec: serde_json::Value,
    config: serde_json::Value,
) -> Id {
    let (template, environment) = (orch_types::new_id(), orch_types::new_id());
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO env_templates (id,org_id,home_id,owner_id,name,driver,spec) VALUES ($1,$2,$2,$3,'Sizing template','docker',$4)")
        .bind(template).bind(root).bind(owner.id()).bind(spec).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO environments (id,name,owner_id,org_id,placement,home_node_id,template_id,status,retained_bytes,config,created_at,updated_at)
      VALUES ($1,'Private environment',$2,$3,'managed',$3,$4,'running',4096,$5,$6,$6)")
        .bind(environment).bind(owner.id()).bind(root).bind(template).bind(config).bind(world.now()).execute(&mut *conn).await.unwrap();
    environment
}

#[tokio::test]
async fn a_member_has_the_complete_view_but_never_a_private_siblings_name_or_metadata() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let reader = person(&world, "reader").await;
    let org = make_org(&world, &owner, "Acme", "acme").await.base.id;
    join(&world, org, org, &reader, "viewer", &owner).await;
    let visible = make_project(&world, &owner, org, "Web").await.base.id;
    let private = private_project(&world, &owner, org).await;
    let top = listing(&world, &reader, &EverythingListQuery::default()).await;
    assert!(top.items.iter().any(|row| row.id == org.to_string()));
    assert!(
        top.items
            .iter()
            .any(|row| row.id == reader.person.personal_org.to_string())
    );
    assert!(top.items.iter().any(|row| row.id == "shared"));
    assert!(top.items.iter().any(|row| row.id == "resources"));
    let level = listing(&world, &reader, &branch(org)).await;
    let web = level
        .items
        .iter()
        .find(|row| row.id == visible.to_string())
        .unwrap();
    assert_eq!(web.my_access, EverythingAccess::View);
    assert!(web.created_at.is_some());
    assert_eq!(web.created_by.as_ref().unwrap().id, owner.id());
    assert!(web.can_share, "holders may share by the shipped setting");
    assert!(!level.items.iter().any(|row| row.id == private.to_string()));
    assert!(
        !serde_json::to_string(&level)
            .unwrap()
            .contains("Private Vault")
    );
    world
        .set_at(
            orch_settings::Target::Node(org),
            orch_settings_contract::registry::keys::SHARING_ALLOW_HOLDERS,
            serde_json::json!(false),
        )
        .await;
    let no_holder_shares = listing(&world, &reader, &branch(org)).await;
    assert!(
        !no_holder_shares
            .items
            .iter()
            .find(|row| row.id == visible.to_string())
            .unwrap()
            .can_share
    );
    assert!(
        listing(&world, &owner, &branch(org))
            .await
            .items
            .iter()
            .find(|row| row.id == visible.to_string())
            .unwrap()
            .can_share
    );
}

#[tokio::test]
async fn sharing_a_leaf_supplies_only_the_path_steps_and_names_that_the_finder_would_reveal() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let guest = person(&world, "guest").await;
    let org = make_org(&world, &owner, "Acme", "acme").await.base.id;
    let private = private_project(&world, &owner, org).await;
    let shared = make_topic(&world, &owner, private, "Shared release plan")
        .await
        .base
        .id;
    let tag = world
        .app
        .call(NodeTagCreate, &[&private])
        .token(&owner.token)
        .body(&NodeTagCreateBody {
            name: "handover".into(),
            color: None,
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO node_tags (node_id,tag_id) VALUES ($1,$2)")
        .bind(shared)
        .bind(tag.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    access::add_entry(
        &mut conn,
        &PageKey::new("node", shared, "live"),
        "any",
        "topic.view",
        ("user", Some(guest.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let found = listing(
        &world,
        &guest,
        &EverythingListQuery {
            q: Some("Shared release".into()),
            ..Default::default()
        },
    )
    .await;
    let hits = world
        .app
        .call(NameSearchList, &[])
        .token(&guest.token)
        .query(&NameSearchListQuery {
            q: Some("Shared release".into()),
            kinds: Some("topic".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(found.items.len(), 1);
    assert_eq!(
        found.items[0].path, hits.items[0].path,
        "the same path rule, including redacted names"
    );
    assert_eq!(found.items[0].tags[0].name, "handover");
    assert!(found.items[0].tags[0].node.redacted && found.items[0].tags[0].node.name.is_none());
    let shared_roots = listing(
        &world,
        &guest,
        &EverythingListQuery {
            parent: Some("shared".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(
        shared_roots
            .items
            .iter()
            .any(|row| row.id == org.to_string())
    );
    let under_root = listing(&world, &guest, &branch(org)).await;
    let step = under_root
        .items
        .iter()
        .find(|row| row.id == private.to_string())
        .unwrap();
    assert_eq!(step.my_access, EverythingAccess::PathOnly);
    assert!(step.name.is_none());
    assert!(step.has_children);
    assert!(step.owner.is_none() && step.created_by.is_none() && step.created_at.is_none());
    assert!(step.last_activity.is_none() && step.size_bytes.is_none() && step.state.is_none());
    assert!(
        step.href.is_none()
            && step.settings_href.is_none()
            && !step.can_share
            && step.tags.is_empty()
    );
    let leaf = listing(&world, &guest, &branch(private)).await;
    assert!(
        leaf.items
            .iter()
            .any(|row| row.id == shared.to_string() && row.href.is_some())
    );
}

#[tokio::test]
async fn the_whole_system_is_an_explicit_admin_scope_and_is_refused_to_a_member() {
    let world = World::new().await;
    let operator = admin(&world, "operator").await;
    let owner = person(&world, "owner").await;
    let reader = person(&world, "reader").await;
    let org = make_org(&world, &owner, "Elsewhere", "elsewhere")
        .await
        .base
        .id;
    let query = EverythingListQuery {
        scope: Some(EverythingScope::System),
        ..Default::default()
    };
    let all = listing(&world, &operator, &query).await;
    assert!(all.items.iter().any(|row| row.id == org.to_string()));
    let refused = world
        .app
        .call(EverythingList, &[])
        .token(&reader.token)
        .query(&query)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn participation_is_explicit_work_below_a_place_and_follows_not_inherited_view() {
    let world = World::new().await;
    let owner = person(&world, "focus-owner").await;
    let reader = person(&world, "focus-reader").await;
    let root = make_org(&world, &owner, "Focus", "focus").await.base.id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let role_project = make_project(&world, &owner, root, "Role work")
        .await
        .base
        .id;
    let group_project = make_project(&world, &owner, root, "Group work")
        .await
        .base
        .id;
    let follow_project = make_project(&world, &owner, root, "Followed work")
        .await
        .base
        .id;
    let other_project = make_project(&world, &owner, root, "Other work")
        .await
        .base
        .id;
    let role_topic = make_topic(&world, &owner, role_project, "Assigned below")
        .await
        .base
        .id;
    let group_topic = make_topic(&world, &owner, group_project, "Shared with group")
        .await
        .base
        .id;
    let follow_topic = make_topic(&world, &owner, follow_project, "Conversation")
        .await
        .base
        .id;
    let initial = listing(&world, &reader, &branch(root)).await;
    assert!(
        initial
            .items
            .iter()
            .filter(
                |row| [role_project, group_project, follow_project, other_project]
                    .iter()
                    .any(|id| row.id == id.to_string())
            )
            .all(|row| !row.participating && row.admin_access.is_none()),
        "ordinary inherited View alone is not participation"
    );
    let root_view = world
        .app
        .call(NodeGet, &[&root])
        .token(&reader.token)
        .send()
        .await
        .ok();
    let Node::Org(root_view) = root_view else {
        panic!("an org was read");
    };
    assert!(
        root_view.base.access.participating,
        "the reader's role was assigned at this root"
    );

    give_role(&world, role_topic, &reader, "developer", &owner).await;
    let followed = orch_types::new_id();
    let group = {
        let mut conn = world.conn().await;
        let group = access::insert_group(&mut conn, root, "Focus group", owner.id())
            .await
            .unwrap();
        access::add_to_group(&mut conn, group, reader.id())
            .await
            .unwrap();
        access::add_entry(
            &mut conn,
            &PageKey::new("node", group_topic, "live"),
            "any",
            "topic.view",
            ("group", Some(group), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            followed,
            "thread",
            Some(follow_topic),
            "Following this",
            owner.id(),
        )
        .await
        .unwrap();
        orch_db::queries::access::bump_epoch(&mut conn)
            .await
            .unwrap();
        group
    };
    world
        .app
        .call(NodeSubscriptionSet, &[&followed])
        .token(&reader.token)
        .body(&NodeSubscriptionSetBody {
            level: SubscriptionLevel::Important,
        })
        .send()
        .await
        .ok();
    let focused = listing(
        &world,
        &reader,
        &EverythingListQuery {
            participating: Some(true),
            ..branch(root)
        },
    )
    .await;
    let ids: std::collections::HashSet<_> =
        focused.items.iter().map(|row| row.id.clone()).collect();
    assert!(
        ids.contains(&role_project.to_string())
            && ids.contains(&group_project.to_string())
            && ids.contains(&follow_project.to_string())
    );
    assert!(!ids.contains(&other_project.to_string()));
    let threads = listing(
        &world,
        &reader,
        &EverythingListQuery {
            participating: Some(true),
            ..branch(follow_topic)
        },
    )
    .await;
    assert!(
        threads
            .items
            .iter()
            .any(|row| row.id == followed.to_string() && row.participating),
        "ordinary followed threads are not expanded as custom pages"
    );
    let tree = world
        .app
        .call(OrgTreeGet, &[&root])
        .token(&reader.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    assert!(
        tree.readable()
            .iter()
            .find(|row| row.id == group_project)
            .unwrap()
            .participating
    );
    assert!(
        !tree
            .readable()
            .iter()
            .find(|row| row.id == other_project)
            .unwrap()
            .participating
    );
    let names = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&NameSearchListQuery {
            q: Some("work".to_owned()),
            kinds: Some("project".to_owned()),
            participating: Some(true),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(names.items.iter().all(|row| row.participating));
    assert!(names.items.iter().any(|row| row.id == group_project));
    assert!(!names.items.iter().any(|row| row.id == other_project));

    let mut conn = world.conn().await;
    access::remove_from_group(&mut conn, group, reader.id())
        .await
        .unwrap();
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let after = listing(
        &world,
        &reader,
        &EverythingListQuery {
            participating: Some(true),
            ..branch(root)
        },
    )
    .await;
    assert!(
        !after
            .items
            .iter()
            .any(|row| row.id == group_project.to_string()),
        "a group grant stops focusing work when membership ends"
    );
}

#[tokio::test]
async fn participation_and_admin_only_are_independent_filters_from_the_same_frame() {
    let world = World::new().await;
    let owner = person(&world, "scope-owner").await;
    let operator = admin(&world, "scope-operator").await;
    let root = make_org(&world, &owner, "Scope", "scope").await.base.id;
    let private = private_project(&world, &owner, root).await;
    let leaf = make_topic(&world, &owner, private, "Named work")
        .await
        .base
        .id;
    let unrelated = make_project(&world, &owner, root, "Other work")
        .await
        .base
        .id;
    {
        let mut conn = world.conn().await;
        access::add_entry(
            &mut conn,
            &PageKey::new("node", leaf, "live"),
            "any",
            "topic.view",
            ("user", Some(operator.id()), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
    }
    let rows = listing(&world, &operator, &branch(root)).await;
    let row = rows
        .items
        .iter()
        .find(|row| row.id == private.to_string())
        .unwrap();
    assert!(
        row.participating && row.admin_access.is_some(),
        "work is assigned below a place whose own content needs native reach"
    );
    assert!(
        !rows
            .items
            .iter()
            .find(|row| row.id == unrelated.to_string())
            .unwrap()
            .participating
    );
    let only_work = listing(
        &world,
        &operator,
        &EverythingListQuery {
            participating: Some(true),
            ..branch(root)
        },
    )
    .await;
    assert!(
        only_work
            .items
            .iter()
            .any(|row| row.id == private.to_string())
    );
    assert!(
        !only_work
            .items
            .iter()
            .any(|row| row.id == unrelated.to_string())
    );
    let ordinary_only = listing(
        &world,
        &operator,
        &EverythingListQuery {
            participating: Some(true),
            admin_access: Some(false),
            ..branch(root)
        },
    )
    .await;
    assert!(
        !ordinary_only
            .items
            .iter()
            .any(|row| row.id == private.to_string()),
        "hiding native-only rows is a separate choice"
    );
}

#[tokio::test]
async fn following_then_muting_a_chat_changes_focus_without_changing_view() {
    let world = World::new().await;
    let owner = person(&world, "chat-owner").await;
    let reader = person(&world, "chat-reader").await;
    let root = make_org(&world, &owner, "Chats", "chats").await.base.id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let project = make_project(&world, &owner, root, "A conversation").await;
    let chat = project.base.chat_id;
    world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&owner.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Text {
                text: "Focus needle".to_owned(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: "focus-message".to_owned(),
        })
        .send()
        .await
        .ok();
    let read = world
        .app
        .call(ChatGet, &[&chat])
        .token(&reader.token)
        .send()
        .await
        .ok();
    assert!(!read.can.participating && read.can.admin_access.is_none());
    for (level, expected) in [(ChatLevel::Mentions, true), (ChatLevel::Off, false)] {
        world
            .app
            .call(ChatStateSet, &[&chat])
            .token(&reader.token)
            .body(&ChatStateSetBody {
                level: Some(Some(level)),
                hidden: None,
            })
            .send()
            .await
            .ok();
        let read = world
            .app
            .call(ChatGet, &[&chat])
            .token(&reader.token)
            .send()
            .await
            .ok();
        assert_eq!(read.can.participating, expected);
        assert!(
            read.can.admin_access.is_none(),
            "a follow is not an access grant or admin marker"
        );
        let focused = listing(
            &world,
            &reader,
            &EverythingListQuery {
                participating: Some(true),
                kinds: Some("chat".to_owned()),
                ..branch(project.base.id)
            },
        )
        .await;
        assert_eq!(
            focused.items.iter().any(|row| row.id == chat.to_string()),
            expected
        );
        let node = world
            .app
            .call(NodeGet, &[&project.base.id])
            .token(&reader.token)
            .send()
            .await
            .ok();
        let Node::Project(node) = node else {
            panic!("a project was read");
        };
        assert_eq!(node.base.access.participating, expected);
        assert!(
            node.base
                .access
                .permissions
                .contains(&"project.view".to_owned())
        );
        let chats = world
            .app
            .call(OrgChatList, &[&root])
            .token(&reader.token)
            .query(&OrgChatListQuery {
                participating: Some(true),
                scope: Some(ChatListScope::Nodes),
                of: None,
                limit: Some(1),
                cursor: None,
            })
            .send()
            .await
            .ok();
        assert_eq!(chats.items.iter().any(|row| row.base.id == chat), expected);
        let search: SearchListQuery = serde_json::from_value(
            serde_json::json!({ "q":"Focus needle", "chat_id":chat, "participating":true }),
        )
        .unwrap();
        let found = world
            .app
            .call(SearchList, &[])
            .token(&reader.token)
            .query(&search)
            .send()
            .await
            .ok();
        assert_eq!(!found.items.is_empty(), expected);
    }
}

#[tokio::test]
async fn inventory_uses_the_shared_words_phrases_and_relevance_order_across_cursor_pages() {
    let world = World::new().await;
    let owner = person(&world, "rank-owner").await;
    let root = make_org(&world, &owner, "Ranked", "ranked").await.base.id;
    for name in [
        "Prerelease train",
        "Train release plans",
        "Release train notes",
        "Release train",
        "Release alone",
    ] {
        make_project(&world, &owner, root, name).await;
    }
    let query = EverythingListQuery {
        parent: Some(root.to_string()),
        q: Some("release train".to_owned()),
        kinds: Some("project".to_owned()),
        limit: Some(1),
        ..Default::default()
    };
    let mut next = None;
    let mut names = Vec::new();
    loop {
        let page = listing(
            &world,
            &owner,
            &EverythingListQuery {
                cursor: next,
                ..query.clone()
            },
        )
        .await;
        names.extend(page.items.into_iter().map(|row| row.name.unwrap()));
        next = page.next_cursor;
        if next.is_none() {
            break;
        }
    }
    assert_eq!(
        names,
        [
            "Release train",
            "Release train notes",
            "Train release plans",
            "Prerelease train"
        ]
    );
    let phrase = listing(
        &world,
        &owner,
        &EverythingListQuery {
            q: Some("\"release train\"".to_owned()),
            limit: Some(200),
            ..query
        },
    )
    .await;
    assert!(
        phrase
            .items
            .iter()
            .all(|row| row.name.as_deref() != Some("Train release plans")),
        "quotes keep adjacent words together"
    );
    assert_eq!(phrase.items.len(), 3);
}

#[tokio::test]
async fn search_ties_use_kind_bound_decaying_visits_then_activity_with_a_stable_page_clock() {
    let world = World::new().await;
    let owner = person(&world, "visits-owner").await;
    let root = make_org(&world, &owner, "Visits", "visits").await.base.id;
    let exact = make_project(&world, &owner, root, "Release note")
        .await
        .base
        .id;
    let current = make_project(&world, &owner, root, "Release note current")
        .await
        .base
        .id;
    let old = make_project(&world, &owner, root, "Release note habit")
        .await
        .base
        .id;
    let older = make_project(&world, &owner, root, "Release note A older")
        .await
        .base
        .id;
    let newer = make_project(&world, &owner, root, "Release note Z newer")
        .await
        .base
        .id;
    {
        let mut conn = world.conn().await;
        sqlx::query("DELETE FROM recents WHERE user_id=$1")
            .bind(owner.id())
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO recents(user_id,kind,id,visit_count,visited_at) VALUES
            ($1,'project',$2,2,$5),($1,'project',$3,100,$5::timestamptz-interval '70 days'),
            ($1,'environment',$4,1000000,$5)",
        )
        .bind(owner.id())
        .bind(current)
        .bind(old)
        .bind(older)
        .bind(world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("UPDATE nodes SET created_at=$2,updated_at=$2 WHERE id=$1")
            .bind(older)
            .bind(world.now() - chrono::Duration::hours(2))
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE nodes SET created_at=$2,updated_at=$2 WHERE id=$1")
            .bind(newer)
            .bind(world.now() - chrono::Duration::hours(1))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let query = EverythingListQuery {
        parent: Some(root.to_string()),
        q: Some("release note".to_owned()),
        kinds: Some("project".to_owned()),
        limit: Some(1),
        ..Default::default()
    };
    let mut ids = Vec::new();
    let mut cursor = None;
    loop {
        let page = listing(
            &world,
            &owner,
            &EverythingListQuery {
                cursor,
                ..query.clone()
            },
        )
        .await;
        assert_eq!(page.items.len(), 1);
        ids.push(page.items[0].id.clone());
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        world.advance(chrono::Duration::hours(1));
    }
    assert_eq!(
        ids,
        [exact, current, old, newer, older].map(|id| id.to_string()),
        "exact names precede habit, old visits decay, another kind's visits never count, and activity precedes alphabetic ties"
    );
}

#[tokio::test]
async fn thread_document_and_chat_activity_reach_the_tree_inventory_and_finder_without_hot_ancestor_writes()
 {
    let world = World::new().await;
    let owner = person(&world, "activity-owner").await;
    let reader = person(&world, "activity-reader").await;
    let root = make_org(&world, &owner, "Activity", "activity")
        .await
        .base
        .id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let work = make_project(&world, &owner, root, "Activity Z current")
        .await
        .base
        .id;
    let quiet = make_project(&world, &owner, root, "Activity A earlier")
        .await
        .base
        .id;
    let work_topic = make_topic(&world, &owner, work, "Current topic").await;
    let quiet_topic = make_topic(&world, &owner, quiet, "Earlier topic").await;
    let thread = orch_types::new_id();
    {
        let mut conn = world.conn().await;
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            thread,
            "thread",
            Some(work_topic.base.id),
            "Working",
            owner.id(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE nodes SET created_at=$2,updated_at=$2 WHERE id=$1")
            .bind(thread)
            .bind(world.now())
            .execute(&mut *conn)
            .await
            .unwrap();
        // A node has its chat from the moment it is made: it is dated with it, on the test's clock.
        sqlx::query("UPDATE chats SET created_at=$2 WHERE node_id=$1")
            .bind(thread)
            .bind(world.now())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let before = world
        .app
        .call(OrgTreeGet, &[&root])
        .token(&reader.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    world.advance(chrono::Duration::seconds(
        orch_db::queries::hierarchy::ACTIVITY_ROLLUP_SECONDS + 1,
    ));
    let active_at = orch_types::Time::new(world.now());
    {
        let mut conn = world.conn().await;
        orch_db::queries::threads::touch(&mut conn, thread, world.now())
            .await
            .unwrap();
    }
    let changed = world
        .app
        .call(OrgTreeGet, &[&root])
        .token(&reader.token)
        .query(&OrgTreeGetQuery {
            since: Some(before.seq),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    for id in [root, work, work_topic.base.id, thread] {
        assert_eq!(
            changed
                .readable()
                .iter()
                .find(|row| row.id == id)
                .unwrap()
                .activity_at,
            Some(active_at)
        );
    }
    assert!(!changed.readable().iter().any(|row| row.id == quiet));
    let active = listing(
        &world,
        &reader,
        &EverythingListQuery {
            active_from: Some(active_at),
            ..branch(root)
        },
    )
    .await;
    assert!(
        active
            .items
            .iter()
            .any(|row| row.id == work.to_string() && row.last_activity == Some(active_at))
    );
    assert!(!active.items.iter().any(|row| row.id == quiet.to_string()));
    let new = listing(
        &world,
        &reader,
        &EverythingListQuery {
            created_from: Some(active_at),
            ..branch(root)
        },
    )
    .await;
    assert!(
        !new.items.iter().any(|row| row.id == work.to_string()),
        "active and created are separate facts"
    );

    world.advance(chrono::Duration::seconds(1));
    {
        let mut conn = world.conn().await;
        orch_db::queries::threads::touch(&mut conn, thread, world.now())
            .await
            .unwrap();
        assert_eq!(
            orch_db::queries::hierarchy::tree_seq(&mut conn, root)
                .await
                .unwrap(),
            changed.seq,
            "a stream inside the five-second window does not rewrite ancestors"
        );
        orch_db::queries::threads::touch(
            &mut conn,
            thread,
            active_at.get() - chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        let last: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT last_activity_at FROM threads WHERE node_id=$1")
                .bind(thread)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(
            last,
            world.now(),
            "delayed work never moves activity backward"
        );
    }
    world.advance(chrono::Duration::seconds(
        orch_db::queries::hierarchy::ACTIVITY_ROLLUP_SECONDS + 1,
    ));
    let document_at = orch_types::Time::new(world.now());
    world
        .app
        .call(NodeDocumentCreate, &[&quiet_topic.base.id])
        .token(&owner.token)
        .body(&NodeDocumentCreateBody {
            title: "Current notes".to_owned(),
            markdown: Some("A real document write".to_owned()),
            blocks: None,
            access: None,
        })
        .send()
        .await
        .ok();
    let after_document = listing(&world, &reader, &branch(root)).await;
    assert_eq!(
        after_document
            .items
            .iter()
            .find(|row| row.id == quiet.to_string())
            .unwrap()
            .last_activity,
        Some(document_at)
    );
    world.advance(chrono::Duration::seconds(
        orch_db::queries::hierarchy::ACTIVITY_ROLLUP_SECONDS + 1,
    ));
    let chat_at = orch_types::Time::new(world.now());
    world
        .app
        .call(ChatMessageCreate, &[&work_topic.base.chat_id])
        .token(&owner.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Text {
                text: "The work is current".to_owned(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: "rollup-message".to_owned(),
        })
        .send()
        .await
        .ok();
    let after_chat = listing(
        &world,
        &reader,
        &EverythingListQuery {
            sort: Some(EverythingSort::LastActivity),
            direction: Some(EverythingDirection::Desc),
            ..branch(root)
        },
    )
    .await;
    assert_eq!(after_chat.items[0].id, work.to_string());
    assert_eq!(after_chat.items[0].last_activity, Some(chat_at));
    let names = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&NameSearchListQuery {
            q: Some("Activity".to_owned()),
            kinds: Some("project".to_owned()),
            org_id: Some(root),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        names.items[0].id, work,
        "finder ties use activity below a place"
    );

    let prior = world
        .app
        .call(OrgTreeGet, &[&root])
        .token(&reader.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    let mut tx = world.db.pool.begin().await.unwrap();
    orch_db::queries::hierarchy::touch_activity(
        &mut tx,
        thread,
        world.now() + chrono::Duration::minutes(1),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let rolled_back = world
        .app
        .call(OrgTreeGet, &[&root])
        .token(&reader.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(rolled_back.seq, prior.seq);
    assert_eq!(
        rolled_back
            .readable()
            .iter()
            .find(|row| row.id == root)
            .unwrap()
            .activity_at,
        Some(chat_at)
    );
}

#[tokio::test]
async fn sort_filters_and_cursors_apply_before_the_page_and_the_cursor_belongs_to_its_view() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Acme", "acme").await.base.id;
    let alpha = make_project(&world, &owner, org, "Alpha").await.base.id;
    let beta = make_project(&world, &owner, org, "Beta").await.base.id;
    let query = EverythingListQuery {
        parent: Some(org.to_string()),
        kinds: Some("project".into()),
        limit: Some(1),
        ..Default::default()
    };
    let first = listing(&world, &owner, &query).await;
    assert_eq!(first.items[0].id, alpha.to_string());
    let second = listing(
        &world,
        &owner,
        &EverythingListQuery {
            cursor: first.next_cursor.clone(),
            ..query.clone()
        },
    )
    .await;
    assert_eq!(second.items[0].id, beta.to_string());
    // A new org has its General project as well: it is the third, and the last.
    let third = listing(
        &world,
        &owner,
        &EverythingListQuery {
            cursor: second.next_cursor.clone(),
            ..query.clone()
        },
    )
    .await;
    assert_eq!(third.items[0].name.as_deref(), Some("General"));
    assert!(third.next_cursor.is_none());
    let backwards = listing(
        &world,
        &owner,
        &EverythingListQuery {
            direction: Some(EverythingDirection::Desc),
            ..query.clone()
        },
    )
    .await;
    assert_eq!(backwards.items[0].name.as_deref(), Some("General"));
    let wrong = world
        .app
        .call(EverythingList, &[])
        .token(&owner.token)
        .query(&EverythingListQuery {
            sort: Some(EverythingSort::Created),
            cursor: first.next_cursor,
            ..query.clone()
        })
        .send()
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::RequestInvalid);
    let no_owner = listing(
        &world,
        &owner,
        &EverythingListQuery {
            owner: Some(Id::nil()),
            ..query.clone()
        },
    )
    .await;
    assert!(no_owner.items.is_empty());
    let future = listing(
        &world,
        &owner,
        &EverythingListQuery {
            created_from: Some(orch_types::Time::new(
                world.now() + chrono::Duration::days(1),
            )),
            ..query
        },
    )
    .await;
    assert!(future.items.is_empty());
}

#[tokio::test]
async fn literal_name_search_finds_every_level_and_page_limits_and_dates_are_validated() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let org = make_org(&world, &owner, "Acme", "acme").await.base.id;
    let project = make_project(&world, &owner, org, "Web").await.base.id;
    let topic = make_topic(&world, &owner, project, "100% ready")
        .await
        .base
        .id;
    let found = listing(
        &world,
        &owner,
        &EverythingListQuery {
            q: Some("%".into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        found
            .items
            .iter()
            .filter(|row| row.kind == EverythingKind::Topic)
            .map(|row| row.id.clone())
            .collect::<Vec<_>>(),
        [topic.to_string()]
    );
    for query in [
        EverythingListQuery {
            limit: Some(201),
            ..Default::default()
        },
        EverythingListQuery {
            created_from: Some(orch_types::Time::new(world.now())),
            created_to: Some(orch_types::Time::new(
                world.now() - chrono::Duration::days(1),
            )),
            ..Default::default()
        },
    ] {
        assert_eq!(
            world
                .app
                .call(EverythingList, &[])
                .token(&owner.token)
                .query(&query)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::RequestInvalid
        );
    }
}

#[tokio::test]
async fn owner_filters_retain_the_path_to_a_match_and_use_the_same_live_and_locked_thread_decisions_as_the_tree()
 {
    use orch_access_contract::permissions::{NodePermission, PermissionName};
    use orch_authz::{Act, Caller, Policy, Principal};
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &owner, "Acme", "acme").await.base.id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let project = make_project(&world, &owner, root, "Work").await.base.id;
    let topic = make_topic(&world, &owner, project, "Plans").await.base.id;
    let (ordinary, mine, private) = (
        orch_types::new_id(),
        orch_types::new_id(),
        orch_types::new_id(),
    );
    let mut conn = world.conn().await;
    for (id, name, first) in [
        (ordinary, "Ordinary", owner.id()),
        (mine, "Mine", reader.id()),
        (private, "Custom", owner.id()),
    ] {
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            id,
            "thread",
            Some(topic),
            name,
            first,
        )
        .await
        .unwrap();
        access::name_owner(&mut conn, id, ("user", first), world.now())
            .await
            .unwrap();
    }
    access::set_row(
        &mut conn,
        &PageKey::new("node", private, "live"),
        "any",
        "thread.view",
        false,
        false,
    )
    .await
    .unwrap();
    drop(conn);
    let chats = listing(
        &world,
        &reader,
        &EverythingListQuery {
            q: Some("Ordinary".into()),
            kinds: Some("chat".into()),
            ..Default::default()
        },
    )
    .await;
    let chat = chats.items.first().unwrap();
    assert_eq!(chat.path.last().unwrap().id, ordinary);
    assert_eq!(
        chat.path.last().unwrap().kind,
        orch_api_contract::nodes::NodeKind::Thread
    );
    assert_eq!(chat.path.last().unwrap().name.as_deref(), Some("Ordinary"));
    assert!(
        chat.href
            .as_deref()
            .unwrap()
            .starts_with(&format!("/thread/{ordinary}?"))
    );
    let owned = EverythingListQuery {
        owner: Some(reader.id()),
        ..Default::default()
    };
    let top = listing(&world, &reader, &owned).await;
    assert!(
        top.items.iter().any(|row| row.id == root.to_string()),
        "a root is retained because its descendant matches"
    );
    let inside = listing(
        &world,
        &reader,
        &EverythingListQuery {
            parent: Some(topic.to_string()),
            ..owned
        },
    )
    .await;
    assert_eq!(
        inside
            .items
            .iter()
            .filter(|row| row.kind == EverythingKind::Thread)
            .map(|row| row.id.clone())
            .collect::<Vec<_>>(),
        [mine.to_string()]
    );
    assert_eq!(
        inside
            .items
            .iter()
            .find(|row| row.id == mine.to_string())
            .unwrap()
            .my_access,
        EverythingAccess::Manage
    );
    for locked in [false, true] {
        let mut conn = world.conn().await;
        if locked {
            access::set_row(
                &mut conn,
                &PageKey::new("node", topic, "defaults"),
                "managed",
                "thread.view",
                false,
                true,
            )
            .await
            .unwrap();
        }
        let principal = Principal::load(
            &mut conn,
            Caller::session(reader.id()),
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
        let expected = orch_authz::nodes::holding(
            &mut conn,
            &principal,
            &[ordinary, mine, private],
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await
        .unwrap();
        drop(conn);
        let shown = listing(&world, &reader, &branch(topic)).await;
        let actual: std::collections::HashSet<_> = shown
            .items
            .iter()
            .filter(|row| {
                row.kind == EverythingKind::Thread && row.my_access != EverythingAccess::PathOnly
            })
            .map(|row| row.id.clone())
            .collect();
        assert_eq!(
            actual,
            expected.iter().map(|row| row.id.to_string()).collect(),
            "locked={locked}"
        );
    }
}

#[tokio::test]
async fn sizes_count_only_readable_documents_and_a_recorded_maker_is_distinct_from_the_current_owner()
 {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &owner, "Acme", "acme").await.base.id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let project = make_project(&world, &owner, root, "Work").await.base.id;
    let topic = make_topic(&world, &owner, project, "Plans").await.base.id;
    let (visible, private) = (orch_types::new_id(), orch_types::new_id());
    let mut conn = world.conn().await;
    for (id, name, bytes) in [
        (visible, "Public notes", 50_i64),
        (private, "Private notes", 900_i64),
    ] {
        sqlx::query("INSERT INTO documents (id,node_id,org_id,owner_id,created_by,title,bytes,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8)")
            .bind(id).bind(topic).bind(root).bind(owner.id()).bind(reader.id()).bind(name).bind(bytes).bind(world.now()).execute(&mut *conn).await.unwrap();
    }
    access::set_row(
        &mut conn,
        &PageKey::new("document", private, "live"),
        "any",
        "doc.view",
        false,
        false,
    )
    .await
    .unwrap();
    drop(conn);
    let documents = listing(&world, &reader, &branch(topic)).await;
    assert!(
        !documents
            .items
            .iter()
            .any(|row| row.id == private.to_string())
    );
    let note = documents
        .items
        .iter()
        .find(|row| row.id == visible.to_string())
        .unwrap();
    assert_eq!(note.size_bytes, Some(50));
    assert_eq!(note.owner.as_ref().unwrap().id, owner.id());
    assert_eq!(note.created_by.as_ref().unwrap().id, reader.id());
    let places = listing(
        &world,
        &reader,
        &EverythingListQuery {
            sort: Some(EverythingSort::Size),
            direction: Some(EverythingDirection::Desc),
            ..branch(project)
        },
    )
    .await;
    assert_eq!(
        places
            .items
            .iter()
            .find(|row| row.id == topic.to_string())
            .unwrap()
            .size_bytes,
        Some(50)
    );
}

#[tokio::test]
async fn knowing_a_private_environments_name_never_reveals_its_state_size_or_requirements() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &owner, "Acme", "acme").await.base.id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let environment = sized_environment(&world,&owner,root,
        serde_json::json!({"image":"alpine:3","resources":{"cpu_millis":4000,"memory_mb":2048,"disk_gb":1}}),serde_json::json!({})).await;
    let mut conn = world.conn().await;
    let page = PageKey::new("environment", environment, "live");
    access::set_row(&mut conn, &page, "any", "environment.view", false, false)
        .await
        .unwrap();
    access::add_entry(
        &mut conn,
        &page,
        "any",
        "access.manage",
        ("user", Some(reader.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    for query in [
        EverythingListQuery {
            parent: Some("resources".into()),
            ..Default::default()
        },
        EverythingListQuery {
            q: Some("Private environment".into()),
            ..Default::default()
        },
    ] {
        let found = listing(&world, &reader, &query).await;
        let row = found
            .items
            .iter()
            .find(|row| row.id == environment.to_string())
            .unwrap();
        assert_eq!(row.name.as_deref(), Some("Private environment"));
        assert_eq!(row.my_access, EverythingAccess::PathOnly);
        assert!(
            row.state.is_none()
                && row.size_bytes.is_none()
                && row.takes.is_none()
                && row.created_at.is_none()
        );
        assert!(row.owner.is_none() && row.created_by.is_none() && row.last_activity.is_none());
        assert!(row.href.is_none() && row.settings_href.is_none() && !row.can_share);
    }
}

#[tokio::test]
async fn resource_requirements_use_the_same_inherited_needs_and_template_limits_as_the_scheduler() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &owner, "Acme", "acme").await.base.id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    world
        .set_at(
            orch_settings::Target::Node(root),
            orch_settings_contract::registry::keys::ENV_NEEDS,
            serde_json::json!({"cpu_millis":1500}),
        )
        .await;
    let environment = sized_environment(&world,&owner,root,
        serde_json::json!({"image":"alpine:3","needs":{"cpu_millis":1000,"memory_mb":256,"disk_gb":1},"resources":{"cpu_millis":2000,"memory_mb":512,"disk_gb":2}}),
        serde_json::json!({"env.needs":{"memory_mb":384}})).await;
    let found = listing(
        &world,
        &reader,
        &EverythingListQuery {
            parent: Some("resources".into()),
            ..Default::default()
        },
    )
    .await;
    let row = found
        .items
        .iter()
        .find(|row| row.id == environment.to_string())
        .unwrap();
    let takes = row.takes.as_ref().unwrap();
    assert_eq!(takes.cpus, Some(1.5));
    assert_eq!(takes.memory_bytes, Some(384 * 1024 * 1024));
    assert_eq!(takes.storage_bytes, Some(1024 * 1024 * 1024));
    assert_eq!(row.state.as_deref(), Some("running"));
}
