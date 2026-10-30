//! Finder names over HTTP: ranking, paths, visibility, directory cards and typing-scale measurements.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{NodeTagCreate, NodeTagCreateBody};
use orch_api_contract::everything::{EverythingList, EverythingListQuery};
use orch_api_contract::nodes::{
    NewNodeAccess, OrgTreeGet, OrgTreeGetQuery, ProjectChildCreate, ProjectCreate,
    ProjectCreateBody,
};
use orch_api_contract::search::{NameHit, NameKind, NameSearchList, NameSearchListQuery};
use orch_api_contract::settings::SettingRegistryGet;
use orch_types::{Id, new_id};
use places::{Who, join, make_department, make_org, make_project, make_topic, person};
use support::World;

fn query(text: &str, kinds: &str) -> NameSearchListQuery {
    NameSearchListQuery {
        q: Some(text.to_owned()),
        kinds: Some(kinds.to_owned()),
        ..Default::default()
    }
}

async fn found(world: &World, by: &Who, text: &str, kinds: &str) -> Vec<NameHit> {
    world
        .app
        .call(NameSearchList, &[])
        .token(&by.token)
        .query(&query(text, kinds))
        .send()
        .await
        .ok()
        .items
}

#[tokio::test]
async fn named_orgs_and_chats_have_the_same_stable_addresses_as_the_inventory() {
    let room = chat_world::Room::new().await;
    let direct = room.direct(&room.ada, &room.bea).await;
    room.say(&room.ada, room.chat, "The destination is here.")
        .await;
    let roots = found(&room.world, &room.ada, "acme", "org").await;
    let root = roots.iter().find(|hit| hit.id == room.root).unwrap();
    assert_eq!(root.href, Some(format!("/org/{}", room.root)));
    assert_ne!(
        root.href,
        root.address
            .as_ref()
            .map(|address| format!("/org/{address}"))
    );
    for (text, id, href) in [
        (
            "bea",
            direct.base.id,
            format!("/org/{}/messages?chat={}", room.root, direct.base.id),
        ),
        (
            "login",
            room.chat,
            format!("/topic/{}?panel=chat&chat={}", room.topic, room.chat),
        ),
    ] {
        let quick = found(&room.world, &room.ada, text, "chat").await;
        let named = quick.iter().find(|hit| hit.id == id).unwrap();
        assert_eq!(named.href.as_deref(), Some(href.as_str()));
        let inventory = room
            .world
            .app
            .call(EverythingList, &[])
            .token(&room.ada.token)
            .query(&EverythingListQuery {
                q: Some(text.to_owned()),
                kinds: Some("chat".into()),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        let listed = inventory
            .items
            .iter()
            .find(|hit| hit.id == id.to_string())
            .unwrap();
        assert_eq!(
            listed.href, named.href,
            "both routes use the same builder and the chat's own org"
        );
    }
}

#[tokio::test]
async fn same_named_threads_carry_their_own_state_and_last_activity_over_the_real_route() {
    let world = World::new().await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &reader, "Work", "work").await.base.id;
    let project = make_project(&world, &reader, root, "Project").await.base.id;
    let topic = make_topic(&world, &reader, project, "Topic").await.base.id;
    let older = world.now() - chrono::Duration::hours(3);
    let newer = world.now() - chrono::Duration::minutes(5);
    let mut conn = world.conn().await;
    let mut expected = std::collections::HashMap::new();
    for (state, at) in [("idle", older), ("running", newer)] {
        let id = new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            id,
            "thread",
            Some(topic),
            "New thread",
            reader.id(),
        )
        .await
        .unwrap();
        orch_authz::pages::start(
            &mut conn,
            orch_access_contract::pages::StoredKind::Node,
            id,
            reader.id(),
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE nodes SET created_at=$2,updated_at=$2,activity_at=$2 WHERE id=$1")
            .bind(id)
            .bind(at)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE threads SET state=$2,last_activity_at=$3 WHERE node_id=$1")
            .bind(id)
            .bind(state)
            .bind(at)
            .execute(&mut *conn)
            .await
            .unwrap();
        expected.insert(id, (state.to_owned(), orch_types::Time::new(at)));
    }
    drop(conn);
    let names = found(&world, &reader, "new thread", "thread").await;
    assert_eq!(names.len(), 2);
    assert_eq!(names[0].last_activity, Some(orch_types::Time::new(newer)));
    for hit in names {
        let (state, at) = &expected[&hit.id];
        assert_eq!(hit.state.as_ref(), Some(state));
        assert_eq!(hit.last_activity.as_ref(), Some(at));
        assert_eq!(hit.name, "New thread");
    }
}

#[tokio::test]
async fn a_full_quick_name_group_says_there_is_more_instead_of_claiming_twenty_are_everything() {
    let world = World::new().await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &reader, "Work", "work").await.base.id;
    for index in 0..30 {
        make_project(&world, &reader, root, &format!("Quarterly work {index:02}")).await;
    }
    let page = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&NameSearchListQuery {
            limit: Some(20),
            ..query("quarterly work", "project")
        })
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 20);
    assert!(page.partial);
    let all = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&NameSearchListQuery {
            limit: Some(50),
            ..query("quarterly work", "project")
        })
        .send()
        .await
        .ok();
    assert_eq!(all.items.len(), 30);
    assert!(!all.partial);
}

#[tokio::test]
async fn one_character_names_are_refused_but_an_empty_recent_lookup_is_allowed() {
    let world = World::new().await;
    let reader = person(&world, "reader").await;
    for text in ["a", "字"] {
        let reply = world
            .app
            .call(NameSearchList, &[])
            .token(&reader.token)
            .query(&query(text, "project"))
            .send()
            .await;
        assert_eq!(reply.problem().code, ErrorCode::RequestInvalid);
    }
    let recent = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&query("", "project"))
        .send()
        .await
        .ok();
    assert!(recent.items.is_empty());
    assert!(!recent.partial);
}

#[tokio::test]
async fn a_reader_with_a_small_reach_gets_an_explicit_partial_answer_instead_of_an_unbounded_scan()
{
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &owner, "Reach", "reach").await.base.id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    // More private matches than four twenty-name lookup batches can consume.
    let hidden: Vec<Id> = (0..300).map(|_| new_id()).collect();
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by) SELECT id,'project',$2,$2,ARRAY[$2,id],'budget hidden ' || ord,$3,$3 FROM unnest($1::uuid[]) WITH ORDINALITY x(id,ord)")
        .bind(&hidden).bind(root).bind(owner.id()).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO permission_rows (object_kind,object_id,page,profile,permission,inherit) SELECT 'node',id,'live','any','project.view',false FROM unnest($1::uuid[]) x(id)")
        .bind(&hidden).execute(&mut *conn).await.unwrap();
    drop(conn);
    let answer = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&query("budget", "project"))
        .send()
        .await
        .ok();
    assert!(
        answer.items.is_empty(),
        "hidden candidates never become names or paths"
    );
    assert!(
        answer.partial,
        "a bounded visibility scan must say there may be more"
    );
}

#[tokio::test]
async fn a_direct_live_grant_reaches_its_root_without_directory_membership() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &owner, "Separate", "separate")
        .await
        .base
        .id;
    let project = make_project(&world, &owner, root, "Granted destination")
        .await
        .base
        .id;
    let outside = make_org(&world, &owner, "Unreachable", "unreachable")
        .await
        .base
        .id;
    let outside_ids: Vec<Id> = (0..300).map(|_| new_id()).collect();
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by) SELECT id,'project',$2,$2,ARRAY[$2,id],'Granted destination ' || ord,$3,$3 FROM unnest($1::uuid[]) WITH ORDINALITY x(id,ord)")
        .bind(&outside_ids).bind(outside).bind(owner.id()).execute(&mut *conn).await.unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", project, "live"),
        "any",
        "project.view",
        ("user", Some(reader.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let answer = world
        .app
        .call(NameSearchList, &[])
        .token(&reader.token)
        .query(&query("Granted destination", "project"))
        .send()
        .await
        .ok();
    assert_eq!(
        answer.items.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [project]
    );
    assert!(
        !answer.partial,
        "unreachable roots must not consume the bounded candidate scan"
    );
}

#[tokio::test]
async fn name_words_are_unordered_quoted_phrases_are_literal_and_exact_names_win() {
    let world = World::new().await;
    let reader = person(&world, "reader").await;
    let root = make_org(&world, &reader, "Search", "search").await.base.id;
    let exact = make_project(&world, &reader, root, "login").await.base.id;
    let prefix = make_project(&world, &reader, root, "login redirect")
        .await
        .base
        .id;
    let word = make_project(&world, &reader, root, "Fix login redirect")
        .await
        .base
        .id;
    let inside = make_project(&world, &reader, root, "prelogin")
        .await
        .base
        .id;
    assert_eq!(
        found(&world, &reader, "login", "project")
            .await
            .iter()
            .map(|hit| hit.id)
            .collect::<Vec<_>>(),
        [exact, prefix, word, inside]
    );
    let unordered = found(&world, &reader, "redirect fix", "project").await;
    assert_eq!(
        unordered.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [word]
    );
    assert!(
        found(&world, &reader, "\"redirect fix\"", "project")
            .await
            .is_empty()
    );
    assert_eq!(
        found(&world, &reader, "redirect \"fix login\"", "project")
            .await
            .iter()
            .map(|hit| hit.id)
            .collect::<Vec<_>>(),
        [word]
    );
    let literal = make_project(&world, &reader, root, "100% safe")
        .await
        .base
        .id;
    make_project(&world, &reader, root, "100x safe").await;
    assert_eq!(
        found(&world, &reader, "\"100% safe\"", "project")
            .await
            .iter()
            .map(|hit| hit.id)
            .collect::<Vec<_>>(),
        [literal]
    );
}

#[tokio::test]
async fn empty_place_chats_do_not_displace_a_real_chat_before_the_names_limit() {
    let room = chat_world::Room::new().await;
    let real = make_topic(&room.world, &room.ada, room.project, "Inside crowded")
        .await
        .base
        .id;
    let chat = chat_world::chat_of(&room.world, &room.ada, real).await;
    room.say(&room.ada, chat, "Actual work to read").await;
    for n in 0..12 {
        make_topic(
            &room.world,
            &room.ada,
            room.project,
            &format!("crowded empty {n}"),
        )
        .await;
    }
    let mut ask = query("crowded", "chat");
    ask.limit = Some(1);
    let hits = room
        .world
        .app
        .call(NameSearchList, &[])
        .token(&room.ada.token)
        .query(&ask)
        .send()
        .await
        .ok();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].id, chat,
        "an empty prefix match must be excluded before ranking and LIMIT"
    );
}

#[tokio::test]
async fn a_tag_inherited_by_a_readable_topic_keeps_its_private_defining_place_redacted() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
    join(&world, org, org, &bea, "viewer", &ada).await;
    let private = world
        .app
        .call(ProjectCreate, &[&org])
        .token(&ada.token)
        .body(&ProjectCreateBody {
            name: "Hidden definition".into(),
            description: None,
            access: Some(NewNodeAccess::Private),
        })
        .send()
        .await
        .ok()
        .base
        .id;
    let topic = make_topic(&world, &ada, private, "Readable work")
        .await
        .base
        .id;
    let tag = world
        .app
        .call(NodeTagCreate, &[&private])
        .token(&ada.token)
        .body(&NodeTagCreateBody {
            name: "handover".into(),
            color: None,
        })
        .send()
        .await
        .ok();
    assert!(found(&world, &bea, "handover", "tag").await.is_empty());
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", topic, "live"),
        "any",
        "topic.view",
        ("user", Some(bea.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let hits = found(&world, &bea, "handover", "tag").await;
    assert_eq!(hits.len(), 1);
    let shown = hits[0].tag.as_ref().unwrap();
    assert_eq!(shown.id, tag.id);
    assert!(shown.node.redacted);
    assert!(shown.node.name.is_none());
    assert!(!format!("{hits:?}").contains("Hidden definition"));
}

#[tokio::test]
async fn name_relevance_precedes_visits_and_an_empty_query_lists_only_recent_visits() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
    let prefix = make_project(&world, &ada, org, "Build tools").await.base.id;
    let middle = make_project(&world, &ada, org, "Rebuild service")
        .await
        .base
        .id;
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO recents (user_id,kind,id,visited_at) VALUES ($1,'project',$2,$3)")
        .bind(ada.id())
        .bind(middle)
        .bind(world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let hits = found(&world, &ada, "BUILD", "project").await;
    assert_eq!(
        hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [prefix, middle]
    );
    assert!(!hits[0].recent && hits[1].recent);
    let recent = found(&world, &ada, "", "project").await;
    assert_eq!(
        recent.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [middle]
    );
}

#[tokio::test]
async fn folder_paths_and_department_addresses_are_complete() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
    let dept = make_department(&world, &ada, org, "Research", "research")
        .await
        .base
        .id;
    let project = make_project(&world, &ada, dept, "Web").await.base.id;
    let folder = world
        .app
        .call(ProjectChildCreate, &[&project])
        .token(&ada.token)
        .body(&ProjectCreateBody {
            name: "Checkout".to_owned(),
            description: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id;
    let topic = make_topic(&world, &ada, folder, "Payment").await.base.id;
    let hits = found(&world, &ada, "PAY", "topic").await;
    assert_eq!(hits[0].id, topic);
    assert_eq!(
        hits[0].path.iter().map(|node| node.id).collect::<Vec<_>>(),
        [org, dept, project, folder, topic]
    );
    let tree = world
        .app
        .call(OrgTreeGet, &[&org])
        .token(&ada.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        tree.readable()
            .into_iter()
            .find(|node| node.id == dept)
            .unwrap()
            .address
            .as_deref(),
        Some("acme.research")
    );
    assert_eq!(
        tree.readable()
            .into_iter()
            .find(|node| node.id == folder)
            .unwrap()
            .address,
        None
    );
}

#[tokio::test]
async fn a_person_found_by_their_address_carries_their_username_and_avatar() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let outsider = person(&world, "eve").await;
    let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
    let dept = make_department(&world, &ada, org, "Research", "research")
        .await
        .base
        .id;
    join(&world, org, dept, &bea, "viewer", &ada).await;
    let hits = found(&world, &ada, "ACME.RESEARCH.BEA", "user").await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, NameKind::User);
    assert_eq!(hits[0].username.as_deref(), Some("bea"));
    assert!(hits[0].avatar.is_some());
    assert!(hits[0].path.is_empty());
    assert!(
        found(&world, &outsider, "acme.research.bea", "user")
            .await
            .is_empty()
    );
    let definitions = world
        .app
        .call(SettingRegistryGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    let idle = definitions
        .iter()
        .find(|def| def.key == "env.idle_stop_minutes")
        .unwrap();
    assert_eq!(idle.group, "environment");
    assert!(!idle.title.is_empty() && !idle.description.is_empty());
}

#[tokio::test]
async fn private_names_never_fill_a_page_or_reveal_their_ancestor_names() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
    join(&world, org, org, &bea, "viewer", &ada).await;
    let secret = world
        .app
        .call(ProjectCreate, &[&org])
        .token(&ada.token)
        .body(&ProjectCreateBody {
            name: "Private Vault".to_owned(),
            description: None,
            access: Some(NewNodeAccess::Private),
        })
        .send()
        .await
        .ok()
        .base
        .id;
    let topic = make_topic(&world, &ada, secret, "Vault topic")
        .await
        .base
        .id;
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", topic, "live"),
        "any",
        "topic.view",
        ("user", Some(bea.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    assert!(!found(&world, &ada, "vault", "project").await.is_empty());
    assert!(found(&world, &bea, "vault", "project").await.is_empty());
    let visible = found(&world, &bea, "vault", "topic").await;
    assert_eq!(visible[0].id, topic);
    let hidden = visible[0]
        .path
        .iter()
        .find(|node| node.id == secret)
        .unwrap();
    assert!(hidden.redacted && hidden.name.is_none());
}

#[tokio::test]
async fn wildcard_characters_are_literal_and_unknown_kinds_and_bad_limits_are_refused() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
    let percent = make_project(&world, &ada, org, "100% ready").await.base.id;
    let underscore = make_project(&world, &ada, org, "code_value").await.base.id;
    make_project(&world, &ada, org, "100x ready").await;
    make_project(&world, &ada, org, "codeXvalue").await;
    make_project(&world, &ada, org, "Other").await;
    assert_eq!(
        found(&world, &ada, "100%", "project")
            .await
            .iter()
            .map(|hit| hit.id)
            .collect::<Vec<_>>(),
        [percent]
    );
    assert_eq!(
        found(&world, &ada, "code_", "project")
            .await
            .iter()
            .map(|hit| hit.id)
            .collect::<Vec<_>>(),
        [underscore],
        "SQL wildcard syntax stays literal within an otherwise valid-length query"
    );
    for query in [
        query("", "bogus"),
        NameSearchListQuery {
            limit: Some(0),
            ..Default::default()
        },
        NameSearchListQuery {
            limit: Some(51),
            ..Default::default()
        },
    ] {
        assert_eq!(
            world
                .app
                .call(NameSearchList, &[])
                .token(&ada.token)
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
async fn ten_thousand_names_are_measured_through_the_authorized_request() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
    let ids: Vec<Id> = (0..10_000).map(|_| new_id()).collect();
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by) SELECT id,'project',$2,$2,ARRAY[$2,id],'Scale name ' || ord,$3,$3 FROM unnest($1::uuid[]) WITH ORDINALITY x(id,ord)")
        .bind(&ids).bind(org).bind(ada.id()).execute(&mut *conn).await.unwrap();
    drop(conn);
    let mut millis = Vec::new();
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let hits = found(&world, &ada, "scale", "project").await;
        millis.push(start.elapsed().as_millis());
        assert_eq!(hits.len(), 20);
    }
    millis.sort_unstable();
    println!("NAMES_10000 samples_ms={millis:?} median_ms={}", millis[2]);
}

#[tokio::test]
async fn every_resource_kind_and_a_machine_are_named_only_for_a_person_who_can_open_them() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Private tree", "private-tree")
        .await
        .base
        .id;
    let mut conn = world.conn().await;
    let mut expected = std::collections::HashSet::new();
    for kind in [
        "provider",
        "agent",
        "environment",
        "host",
        "template",
        "mcp_server",
        "mount",
        "machine",
    ] {
        let id = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            kind,
            Some(ada.id()),
            Some(org),
            &format!("vault-{kind}"),
        )
        .await
        .unwrap();
        orch_authz::pages::start(
            &mut conn,
            kind.parse().unwrap(),
            id,
            ada.id(),
            orch_types::Time::new(world.now()),
        )
        .await
        .unwrap();
        expected.insert((kind.to_owned(), id));
        if kind == "host" {
            // The host's supporting transport is not the independently owned machine below.
            // It remains authorized normally, but must not accidentally match this name probe.
            sqlx::query("UPDATE machines SET name='fixture host transport' WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)")
                .bind(id).execute(&mut *conn).await.unwrap();
        }
    }
    drop(conn);
    let kinds = "provider,agent,environment,host,template,mcp_server,mount,machine";
    let hits = found(&world, &ada, "vault", kinds).await;
    assert_eq!(hits.len(), 8);
    assert_eq!(
        hits.iter()
            .map(|hit| (hit.kind.as_str().to_owned(), hit.id))
            .collect::<std::collections::HashSet<_>>(),
        expected,
        "each fixture resource and the actual owned machine must be the returned identities"
    );
    assert!(hits.iter().all(
        |hit| hit.owner.as_ref().is_some_and(|owner| owner.id == ada.id()) && hit.path.is_empty()
    ));
    assert!(found(&world, &bea, "vault", kinds).await.is_empty());
}

#[tokio::test]
async fn org_project_and_topic_names_and_directory_groups_do_not_cross_a_private_tree() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Vault org", "vault-org")
        .await
        .base
        .id;
    let project = make_project(&world, &ada, org, "Vault project")
        .await
        .base
        .id;
    make_topic(&world, &ada, project, "Vault topic").await;
    let mut conn = world.conn().await;
    orch_db::queries::access::create_group(
        &mut conn,
        new_id(),
        org,
        "Vault group",
        "",
        ada.id(),
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        found(&world, &ada, "vault", "org,project,topic,group")
            .await
            .len(),
        4
    );
    assert!(
        found(&world, &bea, "vault", "org,project,topic,group")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_visible_name_is_found_even_after_multiple_batches_of_hidden_prefixes() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let org = make_org(&world, &ada, "Acme", "acme").await.base.id;
    join(&world, org, org, &bea, "viewer", &ada).await;
    let visible = make_project(&world, &ada, org, "Accessible vault")
        .await
        .base
        .id;
    let ids: Vec<Id> = (0..120).map(|_| new_id()).collect();
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by) SELECT id,'project',$2,$2,ARRAY[$2,id],'Vault hidden ' || ord,$3,$3 FROM unnest($1::uuid[]) WITH ORDINALITY x(id,ord)")
        .bind(&ids).bind(org).bind(ada.id()).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO permission_rows (object_kind,object_id,page,profile,permission,inherit) SELECT 'node',id,'live','any','project.view',false FROM unnest($1::uuid[]) x(id)").bind(&ids).execute(&mut *conn).await.unwrap();
    drop(conn);
    let answer = world
        .app
        .call(NameSearchList, &[])
        .token(&bea.token)
        .query(&NameSearchListQuery {
            limit: Some(1),
            ..query("vault", "project")
        })
        .send()
        .await
        .ok();
    assert_eq!(
        answer.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        [visible]
    );
}

#[tokio::test]
async fn a_participant_chat_in_an_archived_org_keeps_its_readable_name_and_archive_mark() {
    let room = chat_world::Room::new().await;
    let chat = room
        .group(&room.ada, &[&room.bea], Some("Frozen group"))
        .await
        .base
        .id;
    let before = found(&room.world, &room.ada, "frozen", "chat").await;
    assert_eq!(before.len(), 1);
    assert!(!before[0].archived);
    let mut conn = room.world.conn().await;
    sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
        .bind(room.root)
        .bind(room.world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    for participant in [&room.ada, &room.bea] {
        let names = found(&room.world, participant, "frozen", "chat").await;
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].id, chat);
        assert!(names[0].archived);
    }
    assert!(
        found(&room.world, &room.eve, "frozen", "chat")
            .await
            .is_empty()
    );
}
