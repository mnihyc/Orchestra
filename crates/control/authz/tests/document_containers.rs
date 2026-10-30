//! A distinct document folder follows native document pages, locks and creator lifetime rules.

mod support;

use orch_access_contract::{
    decisions::{Decision, ObjectRef},
    pages::{Holder, StoredKind},
    permissions::{ObjectKind, Permission},
    roles::Role,
};
use orch_authz::{
    Caller, Policy, documents,
    pages::{self, Target},
};
use orch_db::queries::documents::{self as rows, containers};
use orch_types::{Id, new_id};
use serde_json::json;
use support::{World, user_holder};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn page(id: Id) -> Target {
    Target::Resource(ObjectRef {
        kind: ObjectKind::DocumentContainer,
        id,
    })
}

async fn folder(w: &World, parent: Option<Id>, name: &str) -> TestResult<Id> {
    let id = new_id();
    let mut conn = w.conn().await;
    containers::insert(
        &mut conn,
        &containers::NewContainer {
            id,
            node: w.topic,
            root: w.root,
            parent,
            owner: w.boss,
            thread: None,
            agent: None,
            name,
            now: w.now.get(),
        },
    )
    .await?;
    pages::start(&mut conn, StoredKind::DocumentContainer, id, w.boss, w.now).await?;
    Ok(id)
}

async fn text(w: &World, parent: Id, owner: Id) -> TestResult<Id> {
    let id = new_id();
    let mut conn = w.conn().await;
    containers::insert_document(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: w.topic,
            org_id: w.root,
            owner_id: owner,
            created_by_thread: None,
            created_by_agent: None,
            title: "Plan",
            now: w.now.get(),
        },
        Some(parent),
        "Plan.md",
    )
    .await?;
    documents::start(&mut conn, id, owner, w.now).await?;
    Ok(id)
}

async fn holds(w: &World, person: Id, id: Id, permission: Permission) -> TestResult<bool> {
    let principal = w.principal(Caller::session(person)).await;
    Ok(matches!(
        documents::check(
            &mut *w.conn().await,
            &principal,
            id,
            permission,
            Policy::default()
        )
        .await?,
        Decision::Allowed { .. }
    ))
}

/// Private retains people explicitly named on the page; revoke those entries through the real owner change.
async fn remove_named(w: &World, folder: Id, person: Id) -> TestResult {
    let entries = orch_db::queries::access::entries_of(
        &mut *w.conn().await,
        &orch_db::queries::access::PageKey::new("document_container", folder, "live"),
    )
    .await?;
    let mut by_permission = std::collections::BTreeMap::<String, Vec<Id>>::new();
    for entry in entries {
        if entry.holder_kind == "user" && entry.holder_id == Some(person) {
            assert_ne!(
                entry.permission, "access.own",
                "the removable fixture entries are not ownership"
            );
            by_permission
                .entry(entry.permission)
                .or_default()
                .push(entry.id);
        }
    }
    assert!(
        !by_permission.is_empty(),
        "the person must really have explicit entries to revoke"
    );
    let changes: Vec<serde_json::Value> = by_permission
        .into_iter()
        .map(|(permission, remove)| json!({"permission":permission,"remove":remove}))
        .collect();
    w.change(w.boss, page(folder), json!({"rows":changes}))
        .await?;
    Ok(())
}

#[tokio::test]
async fn document_folders_inherit_and_restrict_real_document_rows_and_cannot_override_a_locked_default()
-> TestResult {
    let w = World::new().await;
    let viewer = w.member("reader").await;
    w.give(w.project, viewer, Role::Viewer).await;
    let parent = folder(&w, None, "Research").await?;
    let child = folder(&w, Some(parent), "Notes").await?;
    let document = text(&w, child, w.boss).await?;
    assert!(holds(&w, viewer, parent, Permission::DocView).await?);
    assert!(holds(&w, viewer, document, Permission::DocView).await?);
    w.change(w.boss, page(parent), json!({"whole":"private"}))
        .await?;
    assert!(!holds(&w, viewer, child, Permission::DocView).await?);
    assert!(!holds(&w, viewer, document, Permission::DocView).await?);
    w.change(
        w.boss,
        page(parent),
        json!({"name":[{"holders":[user_holder(viewer)],"choice":"full_view"}]}),
    )
    .await?;
    assert!(holds(&w, viewer, document, Permission::DocView).await?);
    assert!(!holds(&w, viewer, document, Permission::DocManage).await?);
    w.change(w.boss, page(parent), json!({"whole":"private"}))
        .await?;
    assert!(
        holds(&w, viewer, document, Permission::DocView).await?,
        "Private keeps the viewer explicitly named on the parent"
    );
    remove_named(&w, parent, viewer).await?;
    assert!(!holds(&w, viewer, child, Permission::DocView).await?);
    assert!(
        !holds(&w, viewer, document, Permission::DocView).await?,
        "the lock case must not retain a personal View grant"
    );
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
    assert!(holds(&w, viewer, child, Permission::DocView).await?);
    assert!(
        holds(&w, viewer, document, Permission::DocView).await?,
        "a folder cannot cut a locked default below the place"
    );
    assert!(holds(&w, w.boss, parent, Permission::DocCreate).await?);
    assert!(holds(&w, w.boss, document, Permission::DocEdit).await?);
    let mut conn = w.conn().await;
    sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
        .bind(w.project)
        .bind(w.now.get())
        .execute(&mut *conn)
        .await?;
    drop(conn);
    assert!(holds(&w, viewer, document, Permission::DocView).await?);
    let principal = w.principal(Caller::session(w.boss)).await;
    for (item, permission) in [
        (parent, Permission::DocCreate),
        (document, Permission::DocEdit),
    ] {
        let decision = documents::check(
            &mut *w.conn().await,
            &principal,
            item,
            permission,
            Policy::default(),
        )
        .await;
        assert!(
            matches!(&decision,Err(orch_authz::Error::Archived {node}) if *node==w.project),
            "archive, rather than a missing permission or an override, closes the write: {decision:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_outside_creators_extra_management_ends_with_access_to_the_actual_parent_folder()
-> TestResult {
    let w = World::new().await;
    let outsider = w.user("guest").await;
    let parent = folder(&w, None, "Research").await?;
    w.change(w.boss,page(parent),json!({"whole":"private","name":[{"holders":[user_holder(outsider)],"choice":"full_interact"}]})).await?;
    assert!(holds(&w, outsider, parent, Permission::DocCreate).await?);
    let document = text(&w, parent, outsider).await?;
    let row = rows::get(&mut *w.conn().await, document)
        .await?
        .ok_or("created text absent")?;
    assert_eq!(
        row.owner_id, w.boss,
        "the accountable root member owns an outside creation"
    );
    assert_eq!(
        row.created_by,
        Some(outsider),
        "the outside person remains the actual author"
    );
    assert!(holds(&w, outsider, document, Permission::DocManage).await?);
    w.change(w.boss, page(parent), json!({"whole":"private"}))
        .await?;
    assert!(
        holds(&w, outsider, parent, Permission::DocView).await?,
        "Private does not silently remove a named guest"
    );
    remove_named(&w, parent, outsider).await?;
    assert!(!holds(&w, outsider, parent, Permission::DocView).await?);
    assert!(
        !holds(&w, outsider, document, Permission::DocManage).await?,
        "the anchor node is not a substitute for the creation's actual parent folder"
    );
    let creation:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='document' AND object_id=$1
        AND permission='doc.manage' AND holder_kind='user' AND holder_id=$2 AND source='creation' AND removed_at IS NULL")
        .bind(document).bind(outsider).fetch_one(&w.db.pool).await?;
    assert_eq!(
        creation, 1,
        "current parent reach, not deleting the creation entry, ends this extra Manage"
    );
    Ok(())
}
