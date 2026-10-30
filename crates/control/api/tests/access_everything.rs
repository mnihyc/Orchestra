//! The requests that are written once for every kind of thing, over HTTP: the page's kind, what was done to a thing, what a person holds on it, and the people behind a holder of its page, for a node, a resource, a document and a chat.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access::{
    AccessChoice, AccessThrough, EffectiveAccess, EffectiveAccessGet, EffectiveAccessGetQuery,
    HistoryLine, HistoryList, HistoryListQuery, Holder, HolderInput, NodeDefaultsUpdate,
    PageChange, PageChangeAdd, PageChangeName, PageChangeRow, PermissionPage, PermissionPageGet,
    PermissionPageUpdate, PermissionPeopleList, PermissionPeopleListQuery,
    PermissionPeopleListReply, Role,
};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::nodes::NodeOrSystem;
use orch_api_contract::resources::{CreateProvider, ProviderCreate, Upstream};
use orch_api_contract::users::{UserSearch, UserSearchQuery};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_db::secrets::MasterKey;
use orch_types::{Id, Time};
use places::Who;
use serde_json::json;

/// Ada's office, with a provider of hers in the org.
struct Office {
    desk: Desk,
    provider: Id,
}

async fn office() -> Office {
    install_master_key(MasterKey::new([7; 32]));
    let desk = Desk::new().await;
    let provider = desk
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&desk.ada.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Team key".to_owned(),
            home_id: desk.org,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://api.example.com/v1".to_owned(),
                options: json!({}),
            }],
            api_key: Some("sk-test".to_owned()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider
        .base
        .id;
    Office { desk, provider }
}

fn naming(person: Id, choice: AccessChoice) -> PageChange {
    PageChange {
        name: Some(vec![PageChangeName {
            holders: vec![HolderInput::User { id: person }],
            choice,
            export: None,
            options: None,
            expires_at: None,
            note: None,
        }]),
        ..PageChange::default()
    }
}

async fn page_of(office: &Office, by: &Who, plural: &str, id: Id) -> PermissionPage {
    office
        .desk
        .world
        .app
        .call(PermissionPageGet, &[&plural, &id])
        .token(&by.token)
        .send()
        .await
        .ok()
}

async fn name_on(
    office: &Office,
    by: &Who,
    plural: &str,
    id: Id,
    person: Id,
    choice: AccessChoice,
) {
    office
        .desk
        .world
        .app
        .call(PermissionPageUpdate, &[&plural, &id])
        .token(&by.token)
        .body(&naming(person, choice))
        .send()
        .await
        .ok();
}

async fn history_of(
    office: &Office,
    by: &Who,
    plural: &str,
    id: Id,
    query: &HistoryListQuery,
) -> orch_api_core::testing::Reply<Page<HistoryLine>> {
    office
        .desk
        .world
        .app
        .call(HistoryList, &[&plural, &id])
        .token(&by.token)
        .query(query)
        .send()
        .await
}

async fn holds(
    office: &Office,
    by: &Who,
    plural: &str,
    id: Id,
    person: Id,
) -> orch_api_core::testing::Reply<EffectiveAccess> {
    office
        .desk
        .world
        .app
        .call(EffectiveAccessGet, &[&plural, &id])
        .token(&by.token)
        .query(&EffectiveAccessGetQuery { user_id: person })
        .send()
        .await
}

fn actions(lines: &Page<HistoryLine>) -> Vec<&str> {
    lines
        .items
        .iter()
        .map(|line| line.action.as_str())
        .collect()
}

#[tokio::test]
async fn the_page_of_a_resource_and_of_a_document_say_what_kind_of_thing_they_are_of() {
    let office = office().await;
    let ada = &office.desk.ada;
    let provider = page_of(&office, ada, "providers", office.provider).await;
    assert_eq!(provider.object.kind, "provider");
    assert_eq!(provider.object.id, office.provider);
    assert_eq!(provider.object.name, "Team key");
    let document = office.desk.make(ada, "Plan").await;
    let page = page_of(&office, ada, "documents", document.base.id).await;
    assert_eq!(page.object.kind, "document");
    let project = page_of(&office, ada, "nodes", office.desk.project).await;
    assert_eq!(project.object.kind, "project");
}

#[tokio::test]
async fn the_history_of_a_provider_is_read_by_whoever_may_view_it_and_by_nobody_else() {
    let office = office().await;
    let (ada, id) = (&office.desk.ada, office.provider);
    let reader = office.desk.member("rae", "viewer").await;
    let stranger = office.desk.stranger("sam").await;
    name_on(
        &office,
        ada,
        "providers",
        id,
        reader.id(),
        AccessChoice::FullInteract,
    )
    .await;
    let lines = history_of(&office, ada, "providers", id, &HistoryListQuery::default())
        .await
        .ok();
    assert!(
        actions(&lines).contains(&"permission.added"),
        "naming somebody on its page is in its history: {:?}",
        actions(&lines)
    );
    let named = lines
        .items
        .iter()
        .find(|line| line.action == "permission.added")
        .unwrap();
    assert!(named.text.contains("ada"), "{}", named.text);
    let read_by_the_named = history_of(
        &office,
        &reader,
        "providers",
        id,
        &HistoryListQuery::default(),
    )
    .await
    .ok();
    assert_eq!(
        actions(&read_by_the_named),
        actions(&lines),
        "whoever may view the thing reads the same lines"
    );
    let refused = history_of(
        &office,
        &stranger,
        "providers",
        id,
        &HistoryListQuery::default(),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn the_history_of_a_document_has_its_own_lines_and_is_paged_newest_first() {
    let office = office().await;
    let ada = &office.desk.ada;
    let document = office.desk.make(ada, "Plan").await.base.id;
    for person in ["rae", "ben", "cal"] {
        let who = office.desk.member(person, "viewer").await;
        name_on(
            &office,
            ada,
            "documents",
            document,
            who.id(),
            AccessChoice::FullView,
        )
        .await;
    }
    let all = history_of(
        &office,
        ada,
        "documents",
        document,
        &HistoryListQuery::default(),
    )
    .await
    .ok();
    assert!(
        actions(&all).contains(&"document.created"),
        "{:?}",
        actions(&all)
    );
    assert!(all.items.windows(2).all(|pair| pair[0].at >= pair[1].at));
    let first = history_of(
        &office,
        ada,
        "documents",
        document,
        &HistoryListQuery {
            limit: Some(2),
            ..HistoryListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(first.items.len(), 2);
    let cursor = first.next_cursor.clone().expect("there is more");
    let second = history_of(
        &office,
        ada,
        "documents",
        document,
        &HistoryListQuery {
            limit: Some(2),
            cursor: Some(cursor),
            ..HistoryListQuery::default()
        },
    )
    .await
    .ok();
    let mut joined: Vec<Id> = first.items.iter().map(|line| line.id).collect();
    joined.extend(second.items.iter().map(|line| line.id));
    let in_one: Vec<Id> = all
        .items
        .iter()
        .take(joined.len())
        .map(|line| line.id)
        .collect();
    assert_eq!(joined, in_one, "the pages join into the one list");
    let only_ada = history_of(
        &office,
        ada,
        "documents",
        document,
        &HistoryListQuery {
            actor: Some(ada.id()),
            ..HistoryListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(only_ada.items.len(), all.items.len());
    let nobody = history_of(
        &office,
        ada,
        "documents",
        document,
        &HistoryListQuery {
            actor: Some(Id::nil()),
            ..HistoryListQuery::default()
        },
    )
    .await
    .ok();
    assert!(nobody.items.is_empty());
}

#[tokio::test]
async fn a_thing_s_history_does_not_carry_the_lines_of_another_thing() {
    let office = office().await;
    let ada = &office.desk.ada;
    let first = office.desk.make(ada, "First").await.base.id;
    let second = office.desk.make(ada, "Second").await.base.id;
    let rae = office.desk.member("rae", "viewer").await;
    name_on(
        &office,
        ada,
        "documents",
        second,
        rae.id(),
        AccessChoice::FullView,
    )
    .await;
    let lines = history_of(
        &office,
        ada,
        "documents",
        first,
        &HistoryListQuery::default(),
    )
    .await
    .ok();
    assert!(
        !actions(&lines).contains(&"permission.added"),
        "{:?}",
        actions(&lines)
    );
}

#[tokio::test]
async fn what_a_person_holds_on_a_resource_and_a_document_is_told_with_what_gives_it() {
    let office = office().await;
    let (ada, id) = (&office.desk.ada, office.provider);
    let rae = office.desk.member("rae", "viewer").await;
    name_on(
        &office,
        ada,
        "providers",
        id,
        rae.id(),
        AccessChoice::FullInteract,
    )
    .await;
    let mine = holds(&office, ada, "providers", id, ada.id()).await.ok();
    assert!(
        mine.permissions
            .iter()
            .any(|held| held.permission == "provider.use")
    );
    assert!(
        mine.permissions.iter().all(|held| held
            .through
            .iter()
            .any(|way| matches!(way, AccessThrough::Owner))),
        "the owner holds everything as the owner"
    );
    let hers = holds(&office, ada, "providers", id, rae.id()).await.ok();
    let using = hers
        .permissions
        .iter()
        .find(|held| held.permission == "provider.use")
        .expect("named in the Use row");
    assert!(
        using
            .through
            .iter()
            .any(|way| matches!(way, AccessThrough::Entry { .. })),
        "{using:?}"
    );
    let theirs = holds(&office, &rae, "providers", id, ada.id())
        .await
        .problem();
    assert_eq!(
        theirs.code,
        ErrorCode::AuthzDenied,
        "what another holds is for whoever manages the page"
    );
    let own = holds(&office, &rae, "providers", id, rae.id()).await.ok();
    assert_eq!(own.user.id, rae.id(), "a person reads their own");
    let document = office.desk.make(ada, "Plan").await.base.id;
    name_on(
        &office,
        ada,
        "documents",
        document,
        rae.id(),
        AccessChoice::FullView,
    )
    .await;
    let on_document = holds(&office, ada, "documents", document, rae.id())
        .await
        .ok();
    assert!(
        on_document
            .permissions
            .iter()
            .any(|held| held.permission == "doc.view")
    );
    assert!(!on_document.administers);
}

#[tokio::test]
async fn the_people_behind_a_holder_of_a_resource_s_page_are_listed_only_for_a_holder_the_page_shows()
 {
    let office = office().await;
    let (ada, id) = (&office.desk.ada, office.provider);
    let rae = office.desk.member("rae", "viewer").await;
    let ben = office.desk.member("ben", "developer").await;
    let org = office.desk.org;
    office
        .desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"providers", &id])
        .token(&ada.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "provider.use".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::Role {
                        role: Role::Viewer,
                        at: None,
                    },
                    options: None,
                    expires_at: None,
                    note: None,
                }]),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let people: PermissionPeopleListReply = office
        .desk
        .world
        .app
        .call(PermissionPeopleList, &[&"providers", &id])
        .token(&ada.token)
        .query(&PermissionPeopleListQuery {
            holder: format!("role:{org}:viewer"),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    let names: Vec<&str> = people
        .items
        .iter()
        .map(|user| user.username.as_str())
        .collect();
    for expected in ["rae", "ben", "ada"] {
        assert!(
            names.contains(&expected),
            "{expected} is a viewer or more: {names:?}"
        );
    }
    let hidden = office
        .desk
        .world
        .app
        .call(PermissionPeopleList, &[&"providers", &id])
        .token(&ada.token)
        .query(&PermissionPeopleListQuery {
            holder: format!("group:{}", orch_types::new_id()),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        hidden.code,
        ErrorCode::NotFound,
        "only a chip that the page shows is answered"
    );
    let refused = office
        .desk
        .world
        .app
        .call(PermissionPeopleList, &[&"providers", &id])
        .token(&office.desk.stranger("sam").await.token)
        .query(&PermissionPeopleListQuery {
            holder: format!("role:{org}:viewer"),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let _ = (rae, ben);
}

#[tokio::test]
async fn a_node_is_served_as_before_and_a_chat_or_a_made_up_kind_is_answered_plainly() {
    let office = office().await;
    let ada = &office.desk.ada;
    let project = office.desk.project;
    let lines = history_of(&office, ada, "nodes", project, &HistoryListQuery::default())
        .await
        .ok();
    assert!(
        actions(&lines).contains(&"node.created"),
        "{:?}",
        actions(&lines)
    );
    let chat = places::read(&office.desk.world, ada, project).await;
    let chat_id = match chat {
        orch_api_contract::nodes::Node::Project(project) => project.base.chat_id,
        other => panic!("{other:?}"),
    };
    for plural in ["chats", "gadgets"] {
        let id = if plural == "chats" { chat_id } else { project };
        let history = history_of(&office, ada, plural, id, &HistoryListQuery::default())
            .await
            .problem();
        assert_eq!(history.code, ErrorCode::NotFound, "{plural}: {history:?}");
        let effective = holds(&office, ada, plural, id, ada.id()).await.problem();
        assert_eq!(effective.code, ErrorCode::NotFound, "{plural}");
        let people = office
            .desk
            .world
            .app
            .call(PermissionPeopleList, &[&plural, &id])
            .token(&ada.token)
            .query(&PermissionPeopleListQuery {
                holder: "admins".to_owned(),
                limit: None,
                cursor: None,
            })
            .send()
            .await
            .problem();
        assert_eq!(people.code, ErrorCode::NotFound, "{plural}");
        let change = office
            .desk
            .world
            .app
            .call(PermissionPageUpdate, &[&plural, &id])
            .token(&ada.token)
            .body(&PageChange::default())
            .send()
            .await
            .problem();
        assert_eq!(change.code, ErrorCode::NotFound, "{plural}");
    }
    let missing = history_of(
        &office,
        ada,
        "providers",
        orch_types::new_id(),
        &HistoryListQuery::default(),
    )
    .await
    .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_history_of_an_environment_holds_what_was_done_in_it_told_by_kind_and_leaves_out_what_the_reader_may_not_view()
 {
    let office = office().await;
    let (ada, world) = (&office.desk.ada, &office.desk.world);
    let rae = office.desk.member("rae", "viewer").await;
    let nothing_to_view = orch_types::new_id();
    let env = {
        let mut conn = world.conn().await;
        let id = fixtures::insert_environment(
            &mut conn,
            ada.id(),
            Some(office.desk.project),
            "Main",
            "managed",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id,
            },
            ada.id(),
            NewAccess::Defaults,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        for (action, summary, details) in [
            ("terminal.opened", "ada opened a shell", json!({})),
            (
                "workspace.file_written",
                "ada wrote a file",
                json!({ "path": "docs/plan.md" }),
            ),
            ("environment.started", "ada started it", json!({})),
            (
                "workspace.changed_by_agent",
                "an agent changed files",
                json!({ "thread_id": nothing_to_view, "paths": ["docs/a.md"] }),
            ),
        ] {
            let mut entry = Entry::new(
                Actor::User(ada.id()),
                action,
                "environment",
                Some(id),
                summary,
            );
            entry.history = true;
            entry.environment_id = Some(id);
            entry.details = details;
            audit::append(&mut conn, &entry, &Origin::default(), world.now())
                .await
                .unwrap();
        }
        id
    };
    name_on(
        &office,
        ada,
        "environments",
        env,
        rae.id(),
        AccessChoice::FullView,
    )
    .await;
    let query = |kinds: &str, path: Option<&str>| HistoryListQuery {
        kinds: (!kinds.is_empty()).then(|| kinds.to_owned()),
        path: path.map(str::to_owned),
        ..HistoryListQuery::default()
    };
    let all = history_of(&office, ada, "environments", env, &query("", None))
        .await
        .ok();
    for expected in [
        "terminal.opened",
        "workspace.file_written",
        "environment.started",
    ] {
        assert!(
            actions(&all).contains(&expected),
            "{expected}: {:?}",
            actions(&all)
        );
    }
    // What an agent did to a thread nobody here may view is in the line of nobody's reading.
    assert!(!actions(&all).contains(&"workspace.changed_by_agent"));
    let shells = history_of(&office, ada, "environments", env, &query("shells", None))
        .await
        .ok();
    assert_eq!(actions(&shells), ["terminal.opened"]);
    let two = history_of(
        &office,
        ada,
        "environments",
        env,
        &query("shells,lifecycle", None),
    )
    .await
    .ok();
    assert_eq!(two.items.len(), 2);
    let under = history_of(
        &office,
        ada,
        "environments",
        env,
        &query("files", Some("docs")),
    )
    .await
    .ok();
    assert!(
        actions(&under).contains(&"workspace.file_written"),
        "{:?}",
        actions(&under)
    );
    let elsewhere = history_of(
        &office,
        ada,
        "environments",
        env,
        &query("files", Some("src")),
    )
    .await
    .ok();
    assert!(elsewhere.items.is_empty());
    let bogus = history_of(&office, ada, "environments", env, &query("dancing", None))
        .await
        .problem();
    assert_eq!(bogus.code, ErrorCode::RequestInvalid);
    let by_a_viewer = history_of(&office, &rae, "environments", env, &query("", None))
        .await
        .ok();
    assert!(!actions(&by_a_viewer).contains(&"workspace.changed_by_agent"));
    assert!(actions(&by_a_viewer).contains(&"terminal.opened"));
}

#[tokio::test]
async fn a_person_of_another_org_is_found_by_an_exact_username_or_address_and_a_colleague_by_a_part_of_the_name()
 {
    let office = office().await;
    let world = &office.desk.world;
    office.desk.member("rae-reader", "viewer").await;
    places::person(world, "olga-outside").await;
    let search = |who: &Who, q: &str, org: Option<Id>| {
        let call = world
            .app
            .call(UserSearch, &[])
            .token(&who.token)
            .query(&UserSearchQuery {
                q: q.to_owned(),
                org_id: org,
            });
        async move {
            call.send()
                .await
                .ok()
                .into_iter()
                .map(|found| found.username)
                .collect::<Vec<_>>()
        }
    };
    let ada = &office.desk.ada;
    assert_eq!(
        search(ada, "rae-r", None).await,
        ["rae-reader"],
        "a colleague by a part"
    );
    assert!(
        search(ada, "olga-out", None).await.is_empty(),
        "somebody of another root org is not found by a part of their name"
    );
    assert_eq!(
        search(ada, "olga-outside", None).await,
        ["olga-outside"],
        "but by the username they gave in full"
    );
    assert!(
        search(ada, "olga-outside", Some(office.desk.org))
            .await
            .is_empty(),
        "and among the members of an org only those who are members"
    );
}

#[tokio::test]
async fn the_people_behind_a_holder_of_a_document_s_page_are_listed_for_whoever_may_read_it() {
    let office = office().await;
    let ada = &office.desk.ada;
    let document = office.desk.make(ada, "Plan").await.base.id;
    let ben = office.desk.member("ben", "viewer").await;
    let stranger = office.desk.stranger("sam").await;
    let people = |who: &Who, holder: &str| {
        let call = office
            .desk
            .world
            .app
            .call(PermissionPeopleList, &[&"documents", &document])
            .token(&who.token)
            .query(&PermissionPeopleListQuery {
                holder: holder.to_owned(),
                limit: None,
                cursor: None,
            });
        async move { call.send().await }
    };
    let admins = people(ada, "admins").await.ok();
    let names: Vec<&str> = admins
        .items
        .iter()
        .map(|user| user.username.as_str())
        .collect();
    assert!(
        names.contains(&"ada"),
        "the owner of the org administers what lies in it: {names:?}"
    );
    // A viewer of the node reads the document's page, and so asks who stands behind it.
    assert!(people(&ben, "admins").await.body.is_ok());
    assert_eq!(
        people(&stranger, "admins").await.problem().code,
        ErrorCode::AuthzDenied
    );
    let nobody = office
        .desk
        .world
        .app
        .call(PermissionPeopleList, &[&"documents", &document])
        .query(&PermissionPeopleListQuery {
            holder: "admins".to_owned(),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn each_resource_kind_answers_the_same_permission_readers_and_refuses_a_stranger() {
    let office = office().await;
    let member = office.desk.member("reader-of-resources", "viewer").await;
    let (ada, world) = (&office.desk.ada, &office.desk.world);
    let stranger = office.desk.stranger("sam").await;
    let kinds = [
        ("machines", "machine", ObjectKind::Machine),
        ("providers", "provider", ObjectKind::Provider),
        ("agents", "agent", ObjectKind::Agent),
        ("environments", "environment", ObjectKind::Environment),
        ("hosts", "host", ObjectKind::Host),
        ("templates", "template", ObjectKind::Template),
        ("mcp-servers", "mcp_server", ObjectKind::McpServer),
        ("mounts", "mount", ObjectKind::Mount),
    ];
    for (plural, word, kind) in kinds {
        let id = {
            let mut conn = world.conn().await;
            // An environment lies in a node; every other kind has the org for its home.
            let home = if kind == ObjectKind::Environment {
                office.desk.project
            } else {
                office.desk.org
            };
            let id = fixtures::insert_resource(&mut conn, word, Some(ada.id()), Some(home), word)
                .await
                .unwrap();
            start_resource(
                &mut conn,
                ObjectRef { kind, id },
                ada.id(),
                NewAccess::Defaults,
                Time::new(world.now()),
            )
            .await
            .unwrap();
            id
        };
        let history = history_of(&office, ada, plural, id, &HistoryListQuery::default()).await;
        assert!(
            history.body.is_ok(),
            "the history of a {word}: {:?}",
            history.body.as_ref().err()
        );
        let held = holds(&office, ada, plural, id, ada.id()).await;
        assert!(
            held.body.is_ok(),
            "what its owner holds on a {word}: {:?}",
            held.body.as_ref().err()
        );
        name_on(
            &office,
            ada,
            plural,
            id,
            member.id(),
            AccessChoice::FullView,
        )
        .await;
        let member_access = holds(&office, ada, plural, id, member.id()).await.ok();
        assert_eq!(
            member_access.user.id,
            member.id(),
            "{word}: inspect the selected person"
        );
        assert!(
            !member_access.permissions.is_empty(),
            "{word}: the selected person holds its served view permission"
        );
        assert_eq!(
            holds(&office, &member, plural, id, ada.id())
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied,
            "{word}: a reader cannot inspect the owner's rights",
        );
        let people = world
            .app
            .call(PermissionPeopleList, &[&plural, &id])
            .token(&ada.token)
            .query(&PermissionPeopleListQuery {
                holder: "admins".to_owned(),
                limit: None,
                cursor: None,
            })
            .send()
            .await;
        assert!(
            people.body.is_ok(),
            "the admins behind a {word}: {:?}",
            people.body.as_ref().err()
        );
        for refused in [
            history_of(&office, &stranger, plural, id, &HistoryListQuery::default())
                .await
                .problem(),
            holds(&office, &stranger, plural, id, stranger.id())
                .await
                .problem(),
        ] {
            assert_eq!(refused.code, ErrorCode::AuthzDenied, "{word}: {refused:?}");
        }
    }
}

#[tokio::test]
async fn what_a_role_held_above_gives_inside_a_place_is_told_as_that_role_with_the_place_whose_defaults_hold_it()
 {
    let office = office().await;
    let (ada, world) = (&office.desk.ada, &office.desk.world);
    let rae = office.desk.member("rae", "viewer").await;
    let project = office.desk.project;
    let topic = places::make_topic(world, ada, project, "Plans")
        .await
        .base
        .id;
    // The defaults of the project let the Viewers manage every topic that follows them.
    world
        .app
        .call(NodeDefaultsUpdate, &[&project])
        .token(&ada.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "topic.manage".to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::Role {
                        role: Role::Viewer,
                        at: None,
                    },
                    options: None,
                    expires_at: None,
                    note: None,
                }]),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let held = holds(&office, ada, "nodes", topic, rae.id()).await.ok();
    assert_eq!(held.role, Some(Role::Viewer), "she holds the role above");
    assert!(!held.administers);
    let manage = held
        .permissions
        .iter()
        .find(|each| each.permission == "topic.manage")
        .expect("the role gives it inside the topic");
    let (entry, from) = manage
        .through
        .iter()
        .find_map(|way| match way {
            AccessThrough::Entry { entry, from, .. } => Some((entry, from)),
            _ => None,
        })
        .expect("an entry gives it");
    assert!(
        matches!(
            entry.holder,
            Holder::Role {
                role: Role::Viewer,
                ..
            }
        ),
        "told as the role, not as a share of the topic: {entry:?}"
    );
    assert!(
        matches!(from, Some(NodeOrSystem::Node(place)) if place.id == project),
        "with the place whose defaults hold the entry: {from:?}"
    );
    assert!(
        !manage
            .through
            .iter()
            .any(|way| matches!(way, AccessThrough::Owner)),
        "she owns nothing there"
    );
}
