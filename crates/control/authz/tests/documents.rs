//! A document's page as shipped and as changed: who reads, comments, writes and manages it, what private means, what a lock above and an archive do, and who gains and loses by a move.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::pages::{Holder, PageState};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_access_contract::roles::{PlaceRef, Role};
use orch_authz::documents::{self, State};
use orch_authz::pages::{self, Target};
use orch_authz::{Act, Caller, Error, Policy, Principal, TokenGrant};
use orch_db::queries::documents as rows;
use orch_types::{Id, Time, new_id};
use serde_json::json;
use support::{World, user_holder};

/// A document beside `node`, made by `maker`, who owns it.
async fn make(w: &World, node: Id, maker: Id) -> Id {
    let mut conn = w.conn().await;
    let id = new_id();
    let org = w.root;
    // These are permission fixtures, including several documents with the same human title.
    // Give each an explicit historical basename rather than creating a new namespace collision.
    let basename = format!("Handbook~{}.md", id.simple());
    rows::containers::insert_document(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: node,
            org_id: org,
            owner_id: maker,
            created_by_thread: None,
            created_by_agent: None,
            title: "Handbook",
            now: w.now.get(),
        },
        None,
        &basename,
    )
    .await
    .unwrap();
    documents::start(&mut conn, id, maker, w.now).await.unwrap();
    id
}

fn page_of(document: Id) -> Target {
    Target::Resource(ObjectRef {
        kind: ObjectKind::Document,
        id: document,
    })
}

async fn holds(w: &World, user: Id, document: Id, permission: Permission) -> bool {
    let principal = w.principal(Caller::session(user)).await;
    let mut conn = w.conn().await;
    match documents::check(
        &mut conn,
        &principal,
        document,
        permission,
        Policy::default(),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => true,
        Ok(Decision::Refused(_)) => false,
        Err(error) => panic!("the question was not answered: {error}"),
    }
}

async fn held(w: &World, user: Id, document: Id) -> Vec<Permission> {
    let principal = w.principal(Caller::session(user)).await;
    let mut conn = w.conn().await;
    documents::effective(&mut conn, &principal, document, Policy::default())
        .await
        .unwrap()
        .held
        .into_iter()
        .map(|held| held.permission)
        .collect()
}

#[tokio::test]
async fn a_document_is_read_by_a_viewer_and_commented_on_by_them_written_by_an_interactor_and_managed_by_a_developer()
 {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    let interactor = w.member("ines").await;
    let developer = w.member("dev").await;
    w.give(w.project, viewer, Role::Viewer).await;
    w.give(w.project, interactor, Role::Interactor).await;
    w.give(w.project, developer, Role::Developer).await;
    let document = make(&w, w.topic, w.boss).await;

    assert!(holds(&w, viewer, document, Permission::DocView).await);
    assert!(
        holds(&w, viewer, document, Permission::DocComment).await,
        "a viewer may ask about a page"
    );
    assert!(
        !holds(&w, viewer, document, Permission::DocEdit).await,
        "but changes none of it"
    );
    assert!(!holds(&w, viewer, document, Permission::DocManage).await);

    assert!(holds(&w, interactor, document, Permission::DocEdit).await);
    assert!(!holds(&w, interactor, document, Permission::DocManage).await);

    assert!(holds(&w, developer, document, Permission::DocManage).await);
    assert!(holds(&w, developer, document, Permission::DocEdit).await);
}

#[tokio::test]
async fn somebody_who_is_no_member_of_the_place_holds_nothing_on_its_documents() {
    let w = World::new().await;
    let stranger = w.user("stranger").await;
    let document = make(&w, w.topic, w.boss).await;
    assert!(held(&w, stranger, document).await.is_empty());
    assert!(!holds(&w, stranger, document, Permission::DocView).await);
}

#[tokio::test]
async fn a_viewer_who_makes_a_document_owns_it_and_writes_and_manages_it_whatever_the_rows_say() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    w.give(w.project, viewer, Role::Viewer).await;
    let theirs = make(&w, w.topic, viewer).await;
    let others = make(&w, w.topic, w.boss).await;
    for permission in [
        Permission::DocView,
        Permission::DocEdit,
        Permission::DocManage,
        Permission::AccessManage,
    ] {
        assert!(
            holds(&w, viewer, theirs, permission).await,
            "{permission:?} on their own"
        );
    }
    assert!(
        !holds(&w, viewer, others, Permission::DocEdit).await,
        "and none of anybody else's"
    );
    let principal = w.principal(Caller::session(viewer)).await;
    let mut conn = w.conn().await;
    let own = documents::effective(&mut conn, &principal, theirs, Policy::default())
        .await
        .unwrap();
    assert!(own.owner);
    let elsewhere = documents::effective(&mut conn, &principal, others, Policy::default())
        .await
        .unwrap();
    assert!(!elsewhere.owner);
}

#[tokio::test]
async fn making_a_document_takes_edit_on_the_node_it_stands_beside_and_a_viewer_is_let_make_one_as_shipped()
 {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    let outsider = w.user("outsider").await;
    w.give(w.project, viewer, Role::Viewer).await;
    let can_make = |user: Id| {
        let w = &w;
        async move {
            let principal = w.principal(Caller::session(user)).await;
            let mut conn = w.conn().await;
            matches!(
                orch_authz::nodes::check(
                    &mut conn,
                    &principal,
                    w.topic,
                    Permission::DocCreate,
                    Act::Use,
                    Policy::default()
                )
                .await
                .unwrap(),
                Decision::Allowed { .. }
            )
        }
    };
    assert!(
        can_make(viewer).await,
        "doc.create is with the Viewers as shipped, so that a page of their own is theirs to write"
    );
    assert!(!can_make(outsider).await);
}

#[tokio::test]
async fn a_private_document_is_its_owners_alone_until_they_share_it() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    let friend = w.member("fay").await;
    w.give(w.project, viewer, Role::Viewer).await;
    w.give(w.project, friend, Role::Viewer).await;
    let owner = w.member("owen").await;
    w.give(w.project, owner, Role::Viewer).await;
    let document = make(&w, w.topic, owner).await;
    assert!(holds(&w, viewer, document, Permission::DocView).await);

    w.change(owner, page_of(document), json!({ "whole": "private" }))
        .await
        .unwrap();

    assert!(
        !holds(&w, viewer, document, Permission::DocView).await,
        "nobody else reads it any more"
    );
    assert!(!holds(&w, viewer, document, Permission::DocComment).await);
    assert!(holds(&w, owner, document, Permission::DocView).await);
    assert!(holds(&w, owner, document, Permission::DocEdit).await);
    let principal = w.principal(Caller::session(owner)).await;
    let mut conn = w.conn().await;
    let page = documents::effective(&mut conn, &principal, document, Policy::default())
        .await
        .unwrap();
    assert_eq!(page.state, PageState::Restricted);
    drop(conn);

    // Sharing is the page again: a person at Can edit.
    w.change(
        owner,
        page_of(document),
        json!({ "name": [{ "holders": [user_holder(friend)], "choice": "full_interact" }] }),
    )
    .await
    .unwrap();
    assert!(holds(&w, friend, document, Permission::DocEdit).await);
    assert!(
        !holds(&w, viewer, document, Permission::DocView).await,
        "still not the others"
    );

    // And private no more when they reset it.
    w.change(owner, page_of(document), json!({ "whole": "reset" }))
        .await
        .unwrap();
    assert!(holds(&w, viewer, document, Permission::DocView).await);
    let mut conn = w.conn().await;
    let principal = w.principal(Caller::session(owner)).await;
    let page = documents::effective(&mut conn, &principal, document, Policy::default())
        .await
        .unwrap();
    assert_eq!(
        page.state,
        PageState::Follows,
        "the Owners row is not a change"
    );
}

#[tokio::test]
async fn explicit_administration_reads_and_manages_a_private_document_without_self_naming() {
    let w = World::new().await;
    let owner = w.member("owen").await;
    w.give(w.project, owner, Role::Viewer).await;
    let document = make(&w, w.topic, owner).await;
    w.change(owner, page_of(document), json!({ "whole": "private" }))
        .await
        .unwrap();
    // Owning the topic does not enable the mode on the ordinary session.
    assert!(!holds(&w, w.boss, document, Permission::AccessManage).await);
    assert!(
        !holds(&w, w.boss, document, Permission::DocView).await,
        "but do not read it by being an admin"
    );
    let principal = w
        .principal(Caller::session(w.boss).with_admin_mode(true))
        .await;
    let mut conn = w.conn().await;
    for permission in [
        Permission::DocView,
        Permission::DocManage,
        Permission::AccessManage,
    ] {
        assert!(
            documents::check(
                &mut conn,
                &principal,
                document,
                permission,
                Policy::default()
            )
            .await
            .unwrap()
            .is_allowed()
        );
    }
    drop(conn);
    let outcome = w
        .change_as_admin(
            w.boss,
            page_of(document),
            json!({ "name": [{ "holders": [user_holder(w.boss)], "choice": "full_view" }] }),
        )
        .await
        .unwrap();
    assert!(
        outcome.changed && !outcome.r#override,
        "native View is not an advanced override"
    );
    assert!(holds(&w, w.boss, document, Permission::DocView).await);
}

#[tokio::test]
async fn beside_a_thread_whose_page_is_closed_a_document_is_as_closed_as_the_thread() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    let reader = w.member("rae").await;
    w.give(w.project, viewer, Role::Viewer).await;
    w.give(w.project, reader, Role::Viewer).await;
    let document = make(&w, w.thread, w.maker).await;
    assert!(holds(&w, viewer, document, Permission::DocView).await);
    // The maker closes the thread's Documents rows to those they name.
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "whole": "private" }),
    )
    .await
    .unwrap();
    assert!(!holds(&w, viewer, document, Permission::DocView).await);
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "name": [{ "holders": [user_holder(reader)], "choice": "full_view" }] }),
    )
    .await
    .unwrap();
    assert!(
        holds(&w, reader, document, Permission::DocView).await,
        "named on the thread, and so on what follows it"
    );
    assert!(!holds(&w, viewer, document, Permission::DocView).await);
}

#[tokio::test]
async fn a_row_of_the_node_that_is_locked_above_decides_the_document_whatever_it_stored() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    let owner = w.member("owen").await;
    w.give(w.project, viewer, Role::Viewer).await;
    w.give(w.project, owner, Role::Viewer).await;
    let document = make(&w, w.topic, owner).await;
    w.change(owner, page_of(document), json!({ "whole": "private" }))
        .await
        .unwrap();
    assert!(!holds(&w, viewer, document, Permission::DocView).await);
    // The project locks its Documents View row to the Viewers: what is below no longer decides.
    w.default_row(w.project, Permission::DocView, "any", true, true)
        .await;
    w.name_default(
        Some(w.project),
        Permission::DocView,
        "any",
        Holder::Role {
            role: Role::Viewer,
            at: None,
        },
    )
    .await;
    assert!(
        holds(&w, viewer, document, Permission::DocView).await,
        "a document under such a lock cannot be made private"
    );
}

#[tokio::test]
async fn under_an_archived_node_a_document_is_read_only() {
    let w = World::new().await;
    let interactor = w.member("dev").await;
    w.give(w.project, interactor, Role::Developer).await;
    let document = make(&w, w.topic, w.boss).await;
    {
        let mut conn = w.conn().await;
        sqlx::query("UPDATE nodes SET archived_at = $2 WHERE id = $1")
            .bind(w.topic)
            .bind(w.now.get())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    assert!(
        holds(&w, interactor, document, Permission::DocView).await,
        "reading stays"
    );
    let principal = w.principal(Caller::session(interactor)).await;
    let mut conn = w.conn().await;
    for permission in [
        Permission::DocEdit,
        Permission::DocComment,
        Permission::DocManage,
    ] {
        let refused = documents::check(
            &mut conn,
            &principal,
            document,
            permission,
            Policy::default(),
        )
        .await;
        assert!(
            matches!(refused, Err(Error::Archived { node }) if node == w.topic),
            "{permission:?} is withheld under an archived node"
        );
    }
    let effective = documents::effective(&mut conn, &principal, document, Policy::default())
        .await
        .unwrap();
    assert!(
        effective
            .held
            .iter()
            .all(|held| matches!(held.permission, Permission::DocView | Permission::DocExport)),
        "reading stays, and so does taking it away as a file, which only reads it"
    );
}

#[tokio::test]
async fn a_document_goes_with_its_node_when_it_is_deleted_and_comes_back_with_it() {
    let w = World::new().await;
    let document = make(&w, w.topic, w.boss).await;
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    sqlx::query("UPDATE nodes SET deleted_at = $2 WHERE id = $1")
        .bind(w.project)
        .bind(w.now.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    let gone = documents::check(
        &mut conn,
        &principal,
        document,
        Permission::DocView,
        Policy::default(),
    )
    .await;
    assert!(matches!(gone, Err(Error::NotFound(_))));
    sqlx::query("UPDATE nodes SET deleted_at = NULL WHERE id = $1")
        .bind(w.project)
        .execute(&mut *conn)
        .await
        .unwrap();
    let back = documents::check(
        &mut conn,
        &principal,
        document,
        Permission::DocView,
        Policy::default(),
    )
    .await;
    assert!(matches!(back, Ok(Decision::Allowed { .. })));
}

#[tokio::test]
async fn a_deleted_document_is_not_found_and_is_asked_about_as_it_was_by_whoever_would_restore_it()
{
    let w = World::new().await;
    let developer = w.member("dev").await;
    let viewer = w.member("vera").await;
    w.give(w.project, developer, Role::Developer).await;
    w.give(w.project, viewer, Role::Viewer).await;
    let document = make(&w, w.topic, w.boss).await;
    {
        let mut conn = w.conn().await;
        rows::soft_delete(&mut conn, document, w.boss, w.now.get())
            .await
            .unwrap();
    }
    let ask = |user: Id, state: State| {
        let w = &w;
        async move {
            let principal = w.principal(Caller::session(user)).await;
            let mut conn = w.conn().await;
            documents::check_all(
                &mut conn,
                &principal,
                document,
                &[Permission::DocManage],
                Act::Use,
                state,
                Policy::default(),
            )
            .await
        }
    };
    assert!(matches!(
        ask(developer, State::Live).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        ask(developer, State::AsItWas).await,
        Ok(Decision::Allowed { .. })
    ));
    assert!(matches!(
        ask(viewer, State::AsItWas).await,
        Ok(Decision::Refused(_))
    ));
}

#[tokio::test]
async fn moving_a_document_to_another_place_says_who_gains_and_who_loses_what() {
    let w = World::new().await;
    let other_project = w.node("project", w.root, "Docs").await;
    let other_topic = w.node("topic", other_project, "Handbook").await;
    let here = w.member("hera").await;
    let there = w.member("theo").await;
    let both = w.member("bobby").await;
    w.give(w.project, here, Role::Viewer).await;
    w.give(other_project, there, Role::Interactor).await;
    w.give(w.project, both, Role::Viewer).await;
    w.give(other_project, both, Role::Viewer).await;
    let document = make(&w, w.topic, w.boss).await;
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let plan = documents::plan_move(
        &mut conn,
        &principal,
        document,
        other_topic,
        Policy::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        plan.losing.get(&here),
        Some(&vec![Permission::DocView, Permission::DocComment])
    );
    assert_eq!(
        plan.gaining.get(&there),
        Some(&vec![
            Permission::DocView,
            Permission::DocComment,
            Permission::DocEdit
        ])
    );
    assert!(
        !plan.gaining.contains_key(&both) && !plan.losing.contains_key(&both),
        "they read in both"
    );
    assert!(
        !plan.losing.contains_key(&w.boss),
        "an owner loses nothing by it"
    );
}

#[tokio::test]
async fn a_token_holds_a_document_only_with_the_scope_and_the_subtree_it_was_made_with() {
    let w = World::new().await;
    let owner = w.member("owen").await;
    w.give(w.project, owner, Role::Viewer).await;
    let document = make(&w, w.topic, owner).await;
    let everything = w
        .principal(Caller::token(owner, TokenGrant::everything()))
        .await;
    let mut conn = w.conn().await;
    assert!(matches!(
        documents::check(
            &mut conn,
            &everything,
            document,
            Permission::DocView,
            Policy::default()
        )
        .await
        .unwrap(),
        Decision::Allowed { .. }
    ));
    let elsewhere = w.node("project", w.root, "Elsewhere").await;
    let kept_to_another_place =
        TokenGrant::from_stored(&["*".to_owned()], Some(elsewhere)).unwrap();
    let narrow = w
        .principal(Caller::token(owner, kept_to_another_place))
        .await;
    assert!(matches!(
        documents::check(
            &mut conn,
            &narrow,
            document,
            Permission::DocView,
            Policy::default()
        )
        .await
        .unwrap(),
        Decision::Refused(_)
    ));
}

#[tokio::test]
async fn a_list_of_documents_is_read_in_the_statements_one_takes_and_says_what_is_held_on_each() {
    let w = World::new().await;
    let viewer = w.member("vera").await;
    w.give(w.project, viewer, Role::Viewer).await;
    let mut ids = Vec::new();
    for _ in 0..6 {
        ids.push(make(&w, w.topic, w.boss).await);
    }
    ids.push(new_id());
    let principal = w.principal(Caller::session(viewer)).await;
    let mut conn = w.conn().await;
    let list = documents::listed(&mut conn, &principal, &ids, Policy::default())
        .await
        .unwrap();
    assert_eq!(list.len(), 6, "what is not there is left out");
    assert!(
        list.iter()
            .all(|each| each.holds(Permission::DocView) && !each.holds(Permission::DocEdit))
    );
    assert!(
        list.iter()
            .all(|each| each.state == PageState::Follows && !each.owner)
    );
    let _ = (
        Time::new(w.now.get()),
        PlaceRef::System,
        pages::read,
        Principal::user,
    );
}
