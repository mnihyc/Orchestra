//! A person's favorites, recents, subscriptions, read positions and the state their interface keeps, over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod desk;
mod places;
mod search_world;
mod support;

use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PageChangeRow, PermissionPageUpdate,
};
use orch_api_contract::auth::{AdminModeSetBody, MeAdminModeSet};
use orch_api_contract::common::{ErrorCode, SubscriptionLevel};
use orch_api_contract::directory::{NodeTagCreate, NodeTagCreateBody};
use orch_api_contract::document_tree::{
    DocumentContainerCreate, DocumentContainerCreateBody, DocumentContainerGet,
    DocumentTreeCommand, DocumentTreeDeletePreview, DocumentTreeItem, DocumentTreeList,
    DocumentTreeListQuery, DocumentTreeOperate, DocumentTreeOperationBody, DocumentTreeRead,
};
use orch_api_contract::documents::NodeDocumentCreateBody;
use orch_api_contract::nodes::{NodeSubscriptionSet, NodeSubscriptionSetBody};
use orch_api_contract::personal::{
    Favorite, MeFavoriteDelete, MeFavoriteList, MeFavoriteSet, MeFavoriteSetBody,
    MeItemFavoriteDelete, MeItemFavoriteSet, MeItemFavoriteSetBody, MeReadStateList,
    MeReadStateListQuery, MeRecentCreate, MeRecentCreateBody, MeStateGet, MeStateSet,
    MeSubscriptionList, MeSubscriptionListQuery, MeVisitCreate, PersonalTarget, SubscriptionSource,
};
use orch_api_contract::search::{NameKind, NameSearchList, NameSearchListQuery, NameSearchPage};
use orch_db::queries::hierarchy;
use orch_types::Id;
use places::{Who, join, make_org, make_project, person};
use serde_json::json;
use support::{Statements, World};

async fn favorites(world: &World, who: &Who) -> Vec<Favorite> {
    world
        .app
        .call(MeFavoriteList, &[])
        .token(&who.token)
        .send()
        .await
        .ok()
}

async fn favor(world: &World, who: &Who, node: Id, after: Option<Option<Id>>) -> u16 {
    world
        .app
        .call(MeFavoriteSet, &[&node])
        .token(&who.token)
        .body(&MeFavoriteSetBody {
            after_node_id: after,
            view: None,
        })
        .send()
        .await
        .status
        .as_u16()
}

#[tokio::test]
async fn every_searchable_kind_can_be_starred_and_recent_through_the_same_personal_routes() {
    let desk = desk::Desk::new().await;
    let document = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "Saved notes".into(),
                blocks: None,
                markdown: None,
                access: None,
            },
        )
        .await
        .ok();
    let folder = personal_folder(&desk, None, "Saved folder").await;
    let (thread, _) = search_world::thread(&desk, &desk.ada, desk.topic, "Saved thread").await;
    let chat = chat_world::chat_of(&desk.world, &desk.ada, desk.topic).await;
    let tag = desk
        .world
        .app
        .call(NodeTagCreate, &[&desk.project])
        .token(&desk.ada.token)
        .body(&NodeTagCreateBody {
            name: "Saved label".into(),
            color: None,
        })
        .send()
        .await
        .ok();
    let group = orch_types::new_id();
    let mut conn = desk.world.conn().await;
    orch_db::queries::access::create_group(
        &mut conn,
        group,
        desk.org,
        "Saved group",
        "",
        desk.ada.id(),
        desk.world.now(),
    )
    .await
    .unwrap();
    let mut targets = vec![
        PersonalTarget {
            kind: NameKind::Org,
            id: desk.org,
        },
        PersonalTarget {
            kind: NameKind::Project,
            id: desk.project,
        },
        PersonalTarget {
            kind: NameKind::Topic,
            id: desk.topic,
        },
        PersonalTarget {
            kind: NameKind::Thread,
            id: thread,
        },
        PersonalTarget {
            kind: NameKind::Document,
            id: document.base.id,
        },
        PersonalTarget {
            kind: NameKind::Chat,
            id: chat,
        },
        PersonalTarget {
            kind: NameKind::DocumentContainer,
            id: folder,
        },
        PersonalTarget {
            kind: NameKind::Tag,
            id: tag.id,
        },
        PersonalTarget {
            kind: NameKind::Group,
            id: group,
        },
        PersonalTarget {
            kind: NameKind::User,
            id: desk.ada.id(),
        },
    ];
    for kind in [
        NameKind::Provider,
        NameKind::Agent,
        NameKind::Environment,
        NameKind::Host,
        NameKind::Template,
        NameKind::McpServer,
        NameKind::Mount,
        NameKind::Machine,
    ] {
        let name = if matches!(kind, NameKind::McpServer | NameKind::Mount) {
            format!("saved_{}", kind.as_str())
        } else {
            format!("Saved {}", kind.as_str())
        };
        let id = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            kind.as_str(),
            Some(desk.ada.id()),
            Some(desk.org),
            &name,
        )
        .await
        .unwrap();
        orch_authz::pages::start(
            &mut conn,
            kind.as_str().parse().unwrap(),
            id,
            desk.ada.id(),
            orch_types::Time::new(desk.world.now()),
        )
        .await
        .unwrap();
        targets.push(PersonalTarget { kind, id });
    }
    drop(conn);
    for target in &targets {
        let kind = target.kind.as_str();
        desk.world
            .app
            .call(MeItemFavoriteSet, &[&kind, &target.id])
            .token(&desk.ada.token)
            .body(&MeItemFavoriteSetBody::default())
            .send()
            .await
            .ok();
        desk.world
            .app
            .call(MeVisitCreate, &[])
            .token(&desk.ada.token)
            .body(target)
            .send()
            .await
            .ok();
    }
    let stars = desk
        .world
        .app
        .call(NameSearchList, &[])
        .token(&desk.ada.token)
        .query(&NameSearchListQuery {
            starred: Some(true),
            limit: Some(50),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let recent = desk
        .world
        .app
        .call(NameSearchList, &[])
        .token(&desk.ada.token)
        .query(&NameSearchListQuery {
            limit: Some(50),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let expected: std::collections::HashSet<_> = targets
        .iter()
        .map(|target| (target.kind, target.id))
        .collect();
    assert_eq!(
        stars
            .items
            .iter()
            .map(|hit| (hit.kind, hit.id))
            .collect::<std::collections::HashSet<_>>(),
        expected
    );
    assert_eq!(
        recent
            .items
            .iter()
            .map(|hit| (hit.kind, hit.id))
            .collect::<std::collections::HashSet<_>>(),
        expected
    );
    assert!(stars.items.iter().all(|hit| hit.starred));
    assert!(!stars.partial && !recent.partial);
    // Each saved name carries its actual page, including the two resource kinds the old finder guessed away.
    for hit in &recent.items {
        let expected = match hit.kind {
            NameKind::Org => Some(format!("/org/{}", hit.id)),
            NameKind::Project => Some(format!("/project/{}", hit.id)),
            NameKind::Topic => Some(format!("/topic/{}", hit.id)),
            NameKind::Thread => Some(format!("/thread/{}", hit.id)),
            NameKind::Document => Some(format!("/doc/{}", hit.id)),
            NameKind::DocumentContainer => Some(format!("/document-folder/{}", hit.id)),
            NameKind::Chat => Some(format!("/topic/{}?panel=chat&chat={}", desk.topic, hit.id)),
            NameKind::User => Some(format!("/u/{}", desk.ada.person.username)),
            NameKind::Group => None,
            NameKind::Tag => Some(format!(
                "/project/{}?{}",
                desk.project,
                form_urlencoded::Serializer::new(String::new())
                    .append_pair("view", "list")
                    .append_pair("tag", &serde_json::to_string(&[hit.id]).unwrap())
                    .finish()
            )),
            NameKind::Provider => Some(format!("/resources/providers/{}", hit.id)),
            NameKind::Agent => Some(format!("/resources/agents/{}", hit.id)),
            NameKind::Environment => Some(format!("/resources/environments/{}", hit.id)),
            NameKind::Host => Some(format!("/resources/hosts/{}", hit.id)),
            NameKind::Template => Some(format!("/resources/templates/{}", hit.id)),
            NameKind::McpServer => Some(format!("/resources/mcp/{}", hit.id)),
            NameKind::Mount => Some(format!("/resources/mounts/{}", hit.id)),
            NameKind::Skill => Some(format!("/resources/skills/{}", hit.id)),
            NameKind::Machine => Some(format!("/resources/machines/{}", hit.id)),
        };
        assert_eq!(hit.href, expected, "served address for {:?}", hit.kind);
        if hit.kind == NameKind::McpServer {
            let server = desk
                .world
                .app
                .call(orch_api_contract::resources::McpServerGet, &[&hit.id])
                .token(&desk.ada.token)
                .send()
                .await
                .ok();
            assert_eq!(server.id, hit.id);
        } else if hit.kind == NameKind::Mount {
            let mount = desk
                .world
                .app
                .call(orch_api_contract::mounts::MountGet, &[&hit.id])
                .token(&desk.ada.token)
                .send()
                .await
                .ok();
            assert_eq!(mount.base.id, hit.id);
        }
    }
    // A stale identity of another kind with the same UUID is independent of this project's star.
    let mut conn = desk.world.conn().await;
    sqlx::query("INSERT INTO favorites(user_id,kind,id,position) VALUES($1,'document',$2,999)")
        .bind(desk.ada.id())
        .bind(desk.project)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    desk.world
        .app
        .call(MeItemFavoriteDelete, &[&"document", &desk.project])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let kept = desk
        .world
        .app
        .call(NameSearchList, &[])
        .token(&desk.ada.token)
        .query(&NameSearchListQuery {
            starred: Some(true),
            limit: Some(50),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        kept.items
            .iter()
            .any(|hit| hit.kind == NameKind::Project && hit.id == desk.project)
    );
}

#[tokio::test]
async fn personal_visit_frequency_decays_before_activity_and_a_new_visit_updates_the_same_count() {
    let world = World::new().await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &reader, "Work", "work").await.base.id;
    let old = make_project(&world, &reader, root, "Current work one")
        .await
        .base
        .id;
    let fresh = make_project(&world, &reader, root, "Current work two")
        .await
        .base
        .id;
    world
        .app
        .call(MeRecentCreate, &[])
        .token(&reader.token)
        .body(&MeRecentCreateBody { node_id: old })
        .send()
        .await
        .ok();
    world
        .app
        .call(MeVisitCreate, &[])
        .token(&reader.token)
        .body(&PersonalTarget {
            kind: NameKind::Project,
            id: fresh,
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    // Sixty-four old visits after eight weekly half-lives weigh less than one current visit.
    sqlx::query("UPDATE recents SET visit_count=64,visited_at=$3 WHERE user_id=$1 AND kind='project' AND id=$2")
        .bind(reader.id()).bind(old).bind(world.now()-chrono::Duration::weeks(8)).execute(&mut *conn).await.unwrap();
    drop(conn);
    let query = NameSearchListQuery {
        q: Some("Current work".into()),
        kinds: Some("project".into()),
        ..Default::default()
    };
    let before = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        before.items.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [fresh, old]
    );
    world
        .app
        .call(MeVisitCreate, &[])
        .token(&reader.token)
        .body(&PersonalTarget {
            kind: NameKind::Project,
            id: old,
        })
        .send()
        .await
        .ok();
    let after = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        after.items.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [old, fresh]
    );
}

#[tokio::test]
async fn rejected_personal_saves_leave_no_visit_or_star_to_appear_after_access_is_later_given() {
    let desk = desk::Desk::new().await;
    let reader = desk.member("reader", "viewer").await;
    let document = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "Private notes".into(),
                blocks: None,
                markdown: None,
                access: Some(orch_api_contract::documents::NewDocumentAccess::Private),
            },
        )
        .await
        .ok();
    let target = PersonalTarget {
        kind: NameKind::Document,
        id: document.base.id,
    };
    let refused = desk
        .world
        .app
        .call(MeItemFavoriteSet, &[&"document", &target.id])
        .token(&reader.token)
        .body(&MeItemFavoriteSetBody::default())
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 404);
    let refused = desk
        .world
        .app
        .call(MeVisitCreate, &[])
        .token(&reader.token)
        .body(&target)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 404);
    let mut conn = desk.world.conn().await;
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("document", target.id, "live"),
        "any",
        "doc.view",
        ("user", Some(reader.id()), None),
        None,
        desk.world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    for starred in [false, true] {
        let answer = desk
            .world
            .app
            .call(NameSearchList, &[])
            .token(&reader.token)
            .query(&NameSearchListQuery {
                starred: Some(starred),
                kinds: Some("document".into()),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(
            answer.items.is_empty(),
            "a refused save must not appear when its target becomes readable"
        );
    }
}

#[tokio::test]
async fn favorites_are_kept_in_order_and_a_favorite_put_again_is_moved() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let own = ada.person.personal_org;
    let [one, two, three] = [
        make_project(&world, &ada, own, "One").await.base.id,
        make_project(&world, &ada, own, "Two").await.base.id,
        make_project(&world, &ada, own, "Three").await.base.id,
    ];
    // Left out, a favorite goes last.
    for node in [one, two, three] {
        assert_eq!(favor(&world, &ada, node, None).await, 204);
    }
    let order = |list: &[Favorite]| {
        list.iter()
            .map(|favorite| favorite.node.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&favorites(&world, &ada).await), vec![one, two, three]);
    // `null` puts it first.
    assert_eq!(favor(&world, &ada, three, Some(None)).await, 204);
    assert_eq!(order(&favorites(&world, &ada).await), vec![three, one, two]);
    // After another favorite: between it and the next.
    assert_eq!(favor(&world, &ada, two, Some(Some(three))).await, 204);
    assert_eq!(order(&favorites(&world, &ada).await), vec![three, two, one]);
    // Again and again between the same two, without renumbering the rest.
    for _ in 0..5 {
        let all = favorites(&world, &ada).await;
        let (first, last) = (all[0].node.id, all[2].node.id);
        assert_eq!(favor(&world, &ada, last, Some(Some(first))).await, 204);
        assert_eq!(favor(&world, &ada, last, Some(None)).await, 204);
    }
    // A favorite it would go after has to be one.
    let refused = world
        .app
        .call(MeFavoriteSet, &[&one])
        .token(&ada.token)
        .body(&MeFavoriteSetBody {
            after_node_id: Some(Some(orch_types::new_id())),
            view: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    // Each comes with the way down to it.
    let listed = favorites(&world, &ada).await;
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].path.len(), 1);
    assert_eq!(listed[0].path[0].id, own);
    let gone = world
        .app
        .call(MeFavoriteDelete, &[&one])
        .token(&ada.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    assert_eq!(favorites(&world, &ada).await.len(), 2);
}

#[tokio::test]
async fn a_node_the_caller_may_not_view_is_not_made_a_favorite_and_a_lost_one_is_not_listed() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let dan = person(&world, "dan").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    join(&world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    assert_eq!(favor(&world, &dan, web.base.id, None).await, 204);
    assert_eq!(favorites(&world, &dan).await.len(), 1);
    // Cut loose from the viewers, it drops from his list without a word.
    sqlx::query(
        "INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, inherit)
         VALUES ('node', $1, 'live', 'any', 'project.view', false)",
    )
    .bind(web.base.id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    assert!(favorites(&world, &dan).await.is_empty());
    let eve = person(&world, "eve").await;
    assert_eq!(favor(&world, &eve, acme.base.id, None).await, 403);
}

#[tokio::test]
async fn the_visits_to_a_node_are_noted_only_for_a_node_the_caller_may_view() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let eve = person(&world, "eve").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let noted = world
        .app
        .call(MeRecentCreate, &[])
        .token(&boss.token)
        .body(&MeRecentCreateBody {
            node_id: acme.base.id,
        })
        .send()
        .await;
    assert_eq!(noted.status, 204);
    let refused = world
        .app
        .call(MeRecentCreate, &[])
        .token(&eve.token)
        .body(&MeRecentCreateBody {
            node_id: acme.base.id,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let mut conn = world.conn().await;
    assert_eq!(
        hierarchy::recents_of(&mut conn, boss.id()).await.unwrap(),
        vec![acme.base.id]
    );
    assert!(
        hierarchy::recents_of(&mut conn, eve.id())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn only_the_newest_visits_are_kept() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let own = ada.person.personal_org;
    let mut conn = world.conn().await;
    let mut nodes = Vec::new();
    for index in 0..4 {
        let at = world.now() + chrono::Duration::seconds(index);
        let project = make_project(&world, &ada, own, &format!("P{index}")).await;
        hierarchy::touch_recent(&mut conn, ada.id(), project.base.id, at, 3)
            .await
            .unwrap();
        nodes.push(project.base.id);
    }
    let kept = hierarchy::recents_of(&mut conn, ada.id()).await.unwrap();
    assert_eq!(
        kept,
        vec![nodes[3], nodes[2], nodes[1]],
        "the latest first, the oldest gone"
    );
}

#[tokio::test]
async fn subscriptions_are_listed_with_how_they_came_about_and_by_org() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let own = boss.person.personal_org;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let notes = make_project(&world, &boss, own, "Notes").await;
    for (node, level) in [
        (web.base.id, SubscriptionLevel::All),
        (notes.base.id, SubscriptionLevel::Important),
    ] {
        world
            .app
            .call(NodeSubscriptionSet, &[&node])
            .token(&boss.token)
            .body(&NodeSubscriptionSetBody { level })
            .send()
            .await
            .ok();
    }
    let all = world
        .app
        .call(MeSubscriptionList, &[])
        .token(&boss.token)
        .query(&MeSubscriptionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(all.len(), 2);
    assert!(
        all.iter()
            .all(|row| row.source == SubscriptionSource::Manual)
    );
    let of_acme = world
        .app
        .call(MeSubscriptionList, &[])
        .token(&boss.token)
        .query(&MeSubscriptionListQuery {
            org_id: Some(acme.base.id),
        })
        .send()
        .await
        .ok();
    assert_eq!(of_acme.len(), 1);
    assert_eq!(of_acme[0].node_id, web.base.id);
    assert_eq!(of_acme[0].level, SubscriptionLevel::All);
}

#[tokio::test]
async fn the_read_positions_of_the_callers_threads_are_listed_by_org() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let topic = places::make_topic(&world, &boss, web.base.id, "Login").await;
    let thread = orch_types::new_id();
    {
        let mut conn = world.conn().await;
        hierarchy::testing::insert_node(
            &mut conn,
            thread,
            "thread",
            Some(topic.base.id),
            "Fix",
            boss.id(),
        )
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO thread_reads (user_id, thread_id, last_read_seq) VALUES ($1, $2, 7)")
        .bind(boss.id())
        .bind(thread)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let list = |org| {
        world
            .app
            .call(MeReadStateList, &[])
            .token(&boss.token)
            .query(&MeReadStateListQuery { org_id: org })
            .send()
    };
    let all = list(None).await.ok();
    assert_eq!((all[0].thread_id, all[0].last_read_seq), (thread, 7));
    assert_eq!(list(Some(acme.base.id)).await.ok().len(), 1);
    assert!(list(Some(boss.person.personal_org)).await.ok().is_empty());
}

#[tokio::test]
async fn the_interface_state_is_kept_whole_under_four_keys_and_follows_the_person() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bob = person(&world, "bob").await;
    let set = |who: &Who, key: &str, value: serde_json::Value| {
        world
            .app
            .call(MeStateSet, &[&key])
            .token(&who.token)
            .raw_body(&value)
            .send()
    };
    assert_eq!(
        set(&ada, "help", json!({ "closed": ["tour"] }))
            .await
            .status,
        204
    );
    assert_eq!(
        set(&ada, "layouts", json!({ "wide": { "panes": 3 } }))
            .await
            .status,
        204
    );
    // The whole value is replaced.
    assert_eq!(set(&ada, "help", json!({ "closed": [] })).await.status, 204);
    let state = world
        .app
        .call(MeStateGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(state["help"], json!({ "closed": [] }));
    assert_eq!(state["layouts"]["wide"]["panes"], 3);
    assert_eq!(state.len(), 2);
    // Nobody else's.
    let other = world
        .app
        .call(MeStateGet, &[])
        .token(&bob.token)
        .send()
        .await
        .ok();
    assert!(other.is_empty());
    // No other key, and no value over 64 KiB.
    let unknown = set(&ada, "theme", json!(1)).await.problem();
    assert_eq!(unknown.code, ErrorCode::RequestInvalid);
    let big = set(&ada, "viewers", json!({ "x": "y".repeat(70_000) }))
        .await
        .problem();
    assert_eq!(big.code, ErrorCode::RequestTooLarge);
    assert_eq!(big.extra["limit"], 65_536);
}

#[tokio::test]
async fn a_persons_keys_are_kept_only_for_actions_there_are_and_none_meets_another() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let set = |value: serde_json::Value| {
        world
            .app
            .call(MeStateSet, &[&"keys"])
            .token(&ada.token)
            .raw_body(&value)
            .send()
    };
    let kept = json!({ "single_key": false, "bindings": { "inbox": "G N", "list": null } });
    assert_eq!(set(kept.clone()).await.status, 204);
    // Keys another action has are refused at the action they were chosen for, naming the other.
    let taken = set(json!({ "bindings": { "search": "Mod+K" } }))
        .await
        .problem();
    assert_eq!(taken.code, ErrorCode::RequestInvalid);
    assert_eq!(taken.extra["errors"][0]["path"], json!("bindings.search"));
    assert!(
        taken.extra["errors"][0]["message"]
            .as_str()
            .is_some_and(|why| why.contains("finder"))
    );
    for wrong in [
        json!({ "bindings": { "launch": "Mod+L" } }),
        json!({ "bindings": { "finder": "ArrowUp" } }),
        json!({ "single_key": "no" }),
    ] {
        assert_eq!(set(wrong).await.problem().code, ErrorCode::RequestInvalid);
    }
    // What was refused changed nothing.
    let state = world
        .app
        .call(MeStateGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert_eq!(state["keys"], kept);
}

#[tokio::test]
async fn the_lists_of_a_person_cost_the_same_statements_whatever_their_length() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let own = ada.person.personal_org;
    async fn measure(world: &World, who: &Who) -> (usize, usize) {
        let call = world.app.call(MeFavoriteList, &[]).token(&who.token);
        let before = Statements::now();
        let listed = call.send().await.ok();
        (Statements::now() - before, listed.len())
    }
    let first = make_project(&world, &ada, own, "First").await;
    favor(&world, &ada, first.base.id, None).await;
    let (few, few_count) = measure(&world, &ada).await;
    for index in 0..8 {
        let project = make_project(&world, &ada, own, &format!("P{index}")).await;
        favor(&world, &ada, project.base.id, None).await;
    }
    let (many, many_count) = measure(&world, &ada).await;
    assert_eq!((few_count, many_count), (1, 9));
    assert_eq!(few, many, "{few} statements for 1 favorite, {many} for 9");
}

async fn personal_folder(desk: &desk::Desk, parent: Option<Id>, name: &str) -> Id {
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([42; 32]),
    );
    let made = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: parent,
            name: name.to_owned(),
            client_id: orch_types::new_id().to_string(),
        })
        .send()
        .await
        .ok();
    match made.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => id,
        _ => panic!("the real create route did not return its distinct document folder"),
    }
}

async fn give_folder_read(
    desk: &desk::Desk,
    folder: Id,
    reader: &Who,
    expires_at: Option<orch_types::Time>,
) {
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &folder])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: reader.id() }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
}

async fn folder_names(
    desk: &desk::Desk,
    reader: &Who,
    text: Option<&str>,
    starred: bool,
) -> NameSearchPage {
    desk.world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&NameSearchListQuery {
            q: text.map(str::to_owned),
            starred: Some(starred),
            kinds: Some("document_container".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok()
}

async fn save_folder(desk: &desk::Desk, reader: &Who, id: Id) {
    desk.world
        .app
        .call(MeItemFavoriteSet, &[&"document_container", &id])
        .token(&reader.token)
        .body(&MeItemFavoriteSetBody::default())
        .send()
        .await
        .ok();
    desk.world
        .app
        .call(MeVisitCreate, &[])
        .token(&reader.token)
        .body(&PersonalTarget {
            kind: NameKind::DocumentContainer,
            id,
        })
        .send()
        .await
        .ok();
}

async fn folder_read(desk: &desk::Desk, reader: &Who, id: Id) -> DocumentTreeRead {
    desk.world
        .app
        .call(DocumentContainerGet, &[&id])
        .token(&reader.token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_folder_only_reader_can_visit_star_list_and_unstar_without_gaining_its_parent_or_a_text_editor()
 {
    let desk = desk::Desk::new().await;
    let reader = desk.stranger("folder-reader").await;
    let parent = personal_folder(&desk, None, "Hidden parent").await;
    let folder = personal_folder(&desk, Some(parent), "Saved folder").await;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &parent])
        .token(&desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .ok();
    give_folder_read(&desk, folder, &reader, None).await;
    let native = folder_read(&desk, &reader, folder).await;
    let row = native
        .entries
        .iter()
        .find(|row| row.entry.item == DocumentTreeItem::Container(folder))
        .unwrap();
    assert!(row.standing.read && !row.standing.write && !row.standing.manage);
    assert!(
        row.document.is_none(),
        "a folder has no editable text projection"
    );
    let context = native.directory.as_ref().unwrap();
    assert!(context.node.redacted && context.node.name.is_none());
    assert!(!context.can_create);
    assert!(
        context
            .ancestors
            .iter()
            .find(|entry| entry.id == parent)
            .is_some_and(|entry| entry.title.is_none() && entry.basename.is_none())
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentTreeList, &[&desk.topic])
            .token(&reader.token)
            .query(&DocumentTreeListQuery::default())
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    save_folder(&desk, &reader, folder).await;
    for (text, starred) in [(Some("Saved folder"), false), (None, false), (None, true)] {
        let found = folder_names(&desk, &reader, text, starred).await;
        assert_eq!(
            found
                .items
                .iter()
                .map(|hit| (hit.kind, hit.id))
                .collect::<Vec<_>>(),
            [(NameKind::DocumentContainer, folder)]
        );
        let hit = &found.items[0];
        assert_eq!(hit.href, Some(format!("/document-folder/{folder}")));
        assert!(hit.starred && hit.recent);
        assert!(
            hit.path
                .iter()
                .all(|place| place.redacted && place.name.is_none())
        );
        assert!(
            !serde_json::to_string(&found)
                .unwrap()
                .contains("Hidden parent")
        );
    }
    desk.world
        .app
        .call(MeItemFavoriteDelete, &[&"document_container", &folder])
        .token(&reader.token)
        .send()
        .await
        .ok();
    assert!(
        folder_names(&desk, &reader, None, true)
            .await
            .items
            .is_empty()
    );
    let recent = folder_names(&desk, &reader, None, false).await;
    assert_eq!(recent.items.len(), 1);
    assert!(recent.items[0].recent && !recent.items[0].starred);
    let mut conn = desk.world.conn().await;
    let not_a_node:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM nodes WHERE id=$1) AND NOT EXISTS(SELECT 1 FROM documents WHERE id=$1)")
        .bind(folder).fetch_one(&mut *conn).await.unwrap();
    assert!(
        not_a_node,
        "personal state must not disguise a folder as a hierarchy node or text document"
    );
}

#[tokio::test]
async fn revoked_expired_private_or_deleted_folders_never_leak_from_names_recents_or_stars() {
    for lost in ["revoked", "expired", "private", "deleted"] {
        let desk = desk::Desk::new().await;
        let reader = if lost == "private" {
            desk.member("folder-reader", "viewer").await
        } else {
            desk.stranger("folder-reader").await
        };
        let title = format!("Saved folder {lost}");
        let folder = personal_folder(&desk, None, &title).await;
        if lost != "private" {
            let expires = (lost == "expired")
                .then(|| orch_types::Time::new(desk.world.now() + chrono::Duration::seconds(1)));
            give_folder_read(&desk, folder, &reader, expires).await;
        }
        assert!(
            folder_read(&desk, &reader, folder)
                .await
                .entries
                .iter()
                .any(|row| row.entry.item == DocumentTreeItem::Container(folder)
                    && row.standing.read)
        );
        save_folder(&desk, &reader, folder).await;
        assert_eq!(
            folder_names(&desk, &reader, None, true).await.items.len(),
            1
        );
        match lost {
            "revoked" => {
                let ids:Vec<Id>=sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='document_container' AND object_id=$1 AND holder_kind='user' AND holder_id=$2 AND permission='doc.view' AND removed_at IS NULL")
                    .bind(folder).bind(reader.id()).fetch_all(&desk.world.db.pool).await.unwrap();
                assert!(!ids.is_empty());
                desk.world
                    .app
                    .call(PermissionPageUpdate, &[&"document-containers", &folder])
                    .token(&desk.ada.token)
                    .body(&PageChange {
                        rows: Some(vec![PageChangeRow {
                            permission: "doc.view".to_owned(),
                            profile: None,
                            inherit: None,
                            locked: None,
                            add: None,
                            update: None,
                            remove: Some(ids),
                        }]),
                        ..Default::default()
                    })
                    .send()
                    .await
                    .ok();
            }
            "expired" => desk.world.advance(chrono::Duration::seconds(2)),
            "private" => {
                desk.world
                    .app
                    .call(PermissionPageUpdate, &[&"document-containers", &folder])
                    .token(&desk.ada.token)
                    .raw_body(&json!({"whole":"private"}))
                    .send()
                    .await
                    .ok();
            }
            "deleted" => {
                let mut request = DocumentTreeOperationBody {
                    command: DocumentTreeCommand::Delete,
                    items: vec![DocumentTreeItem::Container(folder)],
                    to: None,
                    name: None,
                    client_id: orch_types::new_id().to_string(),
                    version: None,
                    deletion_plan: None,
                };
                request.deletion_plan = Some(
                    desk.world
                        .app
                        .call(DocumentTreeDeletePreview, &[])
                        .token(&desk.ada.token)
                        .body(&request)
                        .send()
                        .await
                        .ok()
                        .stamp,
                );
                desk.world
                    .app
                    .call(DocumentTreeOperate, &[])
                    .token(&desk.ada.token)
                    .body(&request)
                    .send()
                    .await
                    .ok();
            }
            _ => unreachable!(),
        }
        for (text, starred) in [
            (Some(title.as_str()), false),
            (None, false),
            (None, true),
            (Some(title.as_str()), true),
        ] {
            let found = folder_names(&desk, &reader, text, starred).await;
            assert!(
                found.items.is_empty(),
                "{lost}: current native Read must filter saved rows"
            );
            assert!(
                !serde_json::to_string(&found)
                    .unwrap()
                    .contains(title.as_str())
            );
        }
        assert_eq!(
            desk.world
                .app
                .call(MeItemFavoriteSet, &[&"document_container", &folder])
                .token(&reader.token)
                .body(&MeItemFavoriteSetBody::default())
                .send()
                .await
                .problem()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            desk.world
                .app
                .call(MeVisitCreate, &[])
                .token(&reader.token)
                .body(&PersonalTarget {
                    kind: NameKind::DocumentContainer,
                    id: folder
                })
                .send()
                .await
                .problem()
                .code,
            ErrorCode::NotFound
        );
        let kept:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM favorites WHERE user_id=$1 AND kind='document_container' AND id=$2),(SELECT count(*) FROM recents WHERE user_id=$1 AND kind='document_container' AND id=$2)")
            .bind(reader.id()).bind(folder).fetch_one(&desk.world.db.pool).await.unwrap();
        assert_eq!(
            kept,
            (1, 1),
            "visibility must come from current native authority, not deleting the personal rows"
        );
        desk.world
            .app
            .call(MeItemFavoriteDelete, &[&"document_container", &folder])
            .token(&reader.token)
            .send()
            .await
            .ok();
    }
}

#[tokio::test]
async fn personal_folder_identity_never_aliases_a_document_with_the_same_uuid() {
    let desk = desk::Desk::new().await;
    let reader = desk.stranger("folder-reader").await;
    let folder = personal_folder(&desk, None, "Saved folder collision").await;
    give_folder_read(&desk, folder, &reader, None).await;
    save_folder(&desk, &reader, folder).await;
    assert_eq!(
        desk.world
            .app
            .call(MeItemFavoriteSet, &[&"document", &folder])
            .token(&reader.token)
            .body(&MeItemFavoriteSetBody::default())
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        desk.world
            .app
            .call(MeVisitCreate, &[])
            .token(&reader.token)
            .body(&PersonalTarget {
                kind: NameKind::Document,
                id: folder
            })
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let mut conn = desk.world.conn().await;
    sqlx::query("INSERT INTO favorites(user_id,kind,id,position) VALUES($1,'document',$2,999)")
        .bind(reader.id())
        .bind(folder)
        .execute(&mut *conn)
        .await
        .unwrap();
    hierarchy::touch_item_recent(
        &mut conn,
        reader.id(),
        "document",
        folder,
        desk.world.now(),
        i64::from(desk.world.app.app.config.limits.recents_per_user),
    )
    .await
    .unwrap();
    drop(conn);
    for starred in [false, true] {
        let wrong = desk
            .world
            .app
            .call(NameSearchList, &[])
            .token(&reader.token)
            .query(&NameSearchListQuery {
                kinds: Some("document".to_owned()),
                starred: Some(starred),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(
            wrong.items.is_empty(),
            "a stale wrong-kind saved row grants no folder or document Read"
        );
    }
    desk.world
        .app
        .call(MeItemFavoriteDelete, &[&"document", &folder])
        .token(&reader.token)
        .send()
        .await
        .ok();
    let kept = folder_names(&desk, &reader, None, true).await;
    assert_eq!(
        kept.items
            .iter()
            .map(|hit| (hit.kind, hit.id))
            .collect::<Vec<_>>(),
        [(NameKind::DocumentContainer, folder)]
    );
    assert!(kept.items[0].starred);
}

#[tokio::test]
async fn folder_personal_state_uses_actual_admin_read_and_loses_visibility_when_that_mode_is_off() {
    let desk = desk::Desk::new().await;
    let admin = places::admin(&desk.world, "folder-admin").await;
    let folder = personal_folder(&desk, None, "Saved private folder").await;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &folder])
        .token(&desk.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .ok();
    let native = folder_read(&desk, &admin, folder).await;
    assert!(
        native
            .entries
            .iter()
            .any(|row| row.entry.item == DocumentTreeItem::Container(folder) && row.standing.read)
    );
    save_folder(&desk, &admin, folder).await;
    let saved = folder_names(&desk, &admin, None, true).await;
    assert_eq!(saved.items.len(), 1);
    assert_eq!(
        (saved.items[0].kind, saved.items[0].id),
        (NameKind::DocumentContainer, folder)
    );
    assert!(
        saved.items[0]
            .admin_access
            .as_ref()
            .is_some_and(|access| access.private)
    );
    assert!(
        !saved.items[0].participating,
        "administrative Read does not invent an assignment"
    );
    desk.world
        .app
        .call(MeAdminModeSet, &[])
        .token(&admin.token)
        .body(&AdminModeSetBody { as_admin: false })
        .send()
        .await
        .ok();
    assert!(
        folder_names(&desk, &admin, None, true)
            .await
            .items
            .is_empty()
    );
    assert!(
        folder_names(&desk, &admin, None, false)
            .await
            .items
            .is_empty()
    );
    assert!(
        folder_names(&desk, &admin, Some("Saved private folder"), false)
            .await
            .items
            .is_empty()
    );
    assert_eq!(
        desk.world
            .app
            .call(MeVisitCreate, &[])
            .token(&admin.token)
            .body(&PersonalTarget {
                kind: NameKind::DocumentContainer,
                id: folder
            })
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    desk.world
        .app
        .call(MeItemFavoriteDelete, &[&"document_container", &folder])
        .token(&admin.token)
        .send()
        .await
        .ok();
}
