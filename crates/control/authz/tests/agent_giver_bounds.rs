//! Agent loans retain each giver and follow that giver's current, ordinary ability to share.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::Role;
use orch_authz::documents::{self, Document};
use orch_authz::pages::{ChangeError, Target};
use orch_authz::reach::{Agent, AgentReach, DocumentPolicy, Reach, Things, Via};
use orch_authz::{Caller, Policy};
use orch_db::queries::{access, documents as rows};
use orch_types::{Id, new_id};
use serde_json::{Value, json};
use support::{World, stored, user_holder};

type Rights = (Option<Via>, Option<Via>, Option<Via>);
type Contribution = (Id, Option<Id>, i64);

async fn document(w: &World, private: bool) -> Document {
    let id = new_id();
    let beside = w
        .node("topic", w.project, &format!("Shared documents {id}"))
        .await;
    {
        let mut conn = w.conn().await;
        rows::insert(
            &mut conn,
            &rows::NewDocument {
                id,
                node_id: beside,
                org_id: w.root,
                owner_id: w.boss,
                created_by_thread: None,
                created_by_agent: None,
                title: "Independent loans",
                now: w.now.get(),
            },
        )
        .await
        .unwrap();
        documents::start(&mut conn, id, w.boss, w.now)
            .await
            .unwrap();
    }
    let doc = {
        let mut conn = w.conn().await;
        Document::of_row(&rows::get(&mut conn, id).await.unwrap().unwrap())
    };
    if private {
        change(
            w,
            w.boss,
            &doc,
            json!({
                "rows": (["doc.view", "doc.edit", "doc.manage"].map(|permission|
                    json!({"permission": permission, "inherit": false})))
            }),
            true,
        )
        .await
        .unwrap();
    }
    doc
}

async fn driver(w: &World, name: &str) -> Id {
    let person = w.member(name).await;
    w.change_as(
        Caller::session(w.maker).with_admin_mode(false),
        Target::Live(w.thread),
        json!({"rows": [{
            "permission": "thread.interact",
            "add": [{"holder": user_holder(person)}]
        }]}),
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(w.can(person, w.thread, P::ThreadView).await);
    assert!(w.can(person, w.thread, P::ThreadInteract).await);
    person
}

async fn change(
    w: &World,
    by: Id,
    doc: &Document,
    body: Value,
    holders_share: bool,
) -> Result<(), ChangeError> {
    w.change_as(
        Caller::session(by).with_admin_mode(false),
        Target::Resource(doc.object()),
        body,
        Policy {
            holders_share,
            ..Policy::default()
        },
    )
    .await
    .map(|_| ())
}

async fn can(w: &World, person: Id, doc: &Document, permission: P) -> bool {
    let principal = w
        .principal(Caller::session(person).with_admin_mode(false))
        .await;
    let mut conn = w.conn().await;
    documents::check(&mut conn, &principal, doc.id, permission, Policy::default())
        .await
        .unwrap()
        .is_allowed()
}

async fn give(w: &World, by: Id, doc: &Document, permission: P, holder: Holder) {
    change(
        w,
        by,
        doc,
        json!({"rows": [{
            "permission": permission.as_str(),
            "add": [{"holder": holder}]
        }]}),
        true,
    )
    .await
    .unwrap();
}

async fn revoke_user(w: &World, doc: &Document, permission: P, person: Id) {
    let entry: Id = sqlx::query_scalar(
        "SELECT id FROM permission_entries
          WHERE object_kind='document' AND object_id=$1 AND permission=$2
            AND holder_kind='user' AND holder_id=$3 AND removed_at IS NULL",
    )
    .bind(doc.id)
    .bind(permission.as_str())
    .bind(person)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    change(
        w,
        w.boss,
        doc,
        json!({"rows": [{"permission": permission.as_str(), "remove": [entry]}]}),
        true,
    )
    .await
    .unwrap();
}

async fn contributions(w: &World, doc: &Document) -> Vec<Contribution> {
    sqlx::query_as(
        "SELECT id, created_by, revision FROM permission_entries
          WHERE object_kind='document' AND object_id=$1 AND permission='doc.manage'
            AND holder_kind='agents' AND holder_id=$2 AND removed_at IS NULL
          ORDER BY id",
    )
    .bind(doc.id)
    .bind(w.thread)
    .fetch_all(&w.db.pool)
    .await
    .unwrap()
}

async fn rights(w: &World, doc: &Document) -> Rights {
    let mut conn = w.conn().await;
    let agent = Agent::of(&mut conn, w.thread).await.unwrap().unwrap();
    let reach = Reach::load_current(
        &mut conn,
        agent,
        w.now,
        &Things {
            nodes: vec![doc.node],
            documents: vec![doc.clone()],
            people_chats: Vec::new(),
        },
    )
    .await
    .unwrap();
    let held = reach.document(doc);
    (held.read, held.write, held.manage)
}

#[tokio::test]
async fn two_people_lend_the_same_agent_right_without_merging_their_contributions() {
    let w = World::new().await;
    let doc = document(&w, true).await;
    let alice = driver(&w, "alice").await;
    let bob = driver(&w, "bob").await;
    let human = w.member("human").await;
    for giver in [alice, bob] {
        give(&w, w.boss, &doc, P::DocManage, Holder::User { id: giver }).await;
        assert!(can(&w, giver, &doc, P::DocManage).await);
    }
    give(&w, alice, &doc, P::DocEdit, Holder::User { id: human }).await;
    for giver in [alice, bob] {
        give(
            &w,
            giver,
            &doc,
            P::DocManage,
            Holder::Agents { id: w.thread },
        )
        .await;
    }
    let original = contributions(&w, &doc).await;
    assert_eq!(
        original.len(),
        2,
        "the two successful page producers must retain independent givers"
    );
    assert!(original.iter().any(|(_, by, _)| *by == Some(alice)));
    assert!(original.iter().any(|(_, by, _)| *by == Some(bob)));
    assert_eq!(
        rights(&w, &doc).await,
        (Some(Via::Share), Some(Via::Share), Some(Via::Share))
    );

    revoke_user(&w, &doc, P::DocManage, alice).await;
    assert!(!can(&w, alice, &doc, P::DocManage).await);
    assert!(can(&w, bob, &doc, P::DocManage).await);
    assert!(can(&w, human, &doc, P::DocEdit).await);
    assert_eq!(
        rights(&w, &doc).await,
        (Some(Via::Share), Some(Via::Share), Some(Via::Share))
    );
    assert_eq!(contributions(&w, &doc).await, original);

    revoke_user(&w, &doc, P::DocManage, bob).await;
    assert_eq!(rights(&w, &doc).await, (None, None, None));
    assert!(can(&w, human, &doc, P::DocEdit).await);
    assert_eq!(contributions(&w, &doc).await, original);
    give(&w, w.boss, &doc, P::DocManage, Holder::User { id: bob }).await;
    assert_eq!(
        rights(&w, &doc).await,
        (Some(Via::Share), Some(Via::Share), Some(Via::Share))
    );
    assert_eq!(contributions(&w, &doc).await, original);
}

#[tokio::test]
async fn a_manage_loan_keeps_only_the_lower_permissions_its_giver_can_still_share() {
    let w = World::new().await;
    let doc = document(&w, true).await;
    let giver = driver(&w, "giver").await;
    give(&w, w.boss, &doc, P::DocManage, Holder::User { id: giver }).await;
    give(
        &w,
        giver,
        &doc,
        P::DocManage,
        Holder::Agents { id: w.thread },
    )
    .await;
    let original = contributions(&w, &doc).await;
    assert_eq!(original.len(), 1);
    assert_eq!(
        rights(&w, &doc).await,
        (Some(Via::Share), Some(Via::Share), Some(Via::Share))
    );

    revoke_user(&w, &doc, P::DocManage, giver).await;
    give(&w, w.boss, &doc, P::DocView, Holder::User { id: giver }).await;
    assert!(can(&w, giver, &doc, P::DocView).await);
    assert!(!can(&w, giver, &doc, P::DocEdit).await);
    assert!(!can(&w, giver, &doc, P::DocManage).await);
    assert_eq!(
        rights(&w, &doc).await,
        (Some(Via::Share), None, None),
        "a management ceiling must not retain lost rights or discard the still-shareable read"
    );
    give(&w, w.boss, &doc, P::DocEdit, Holder::User { id: giver }).await;
    assert_eq!(
        rights(&w, &doc).await,
        (Some(Via::Share), Some(Via::Share), None)
    );
    revoke_user(&w, &doc, P::DocEdit, giver).await;
    revoke_user(&w, &doc, P::DocView, giver).await;
    assert_eq!(rights(&w, &doc).await, (None, None, None));
    assert_eq!(contributions(&w, &doc).await, original);
    give(&w, w.boss, &doc, P::DocManage, Holder::User { id: giver }).await;
    assert_eq!(
        rights(&w, &doc).await,
        (Some(Via::Share), Some(Via::Share), Some(Via::Share))
    );
    assert_eq!(contributions(&w, &doc).await, original);
}

#[tokio::test]
async fn an_agent_loan_tracks_current_giveability_without_revoking_human_grants() {
    let w = World::new().await;
    let doc = document(&w, true).await;
    let giver = driver(&w, "giver").await;
    let human = w.member("human").await;
    give(&w, w.boss, &doc, P::DocManage, Holder::User { id: giver }).await;
    give(
        &w,
        giver,
        &doc,
        P::DocManage,
        Holder::Agents { id: w.thread },
    )
    .await;
    give(&w, giver, &doc, P::DocEdit, Holder::User { id: human }).await;
    let original = contributions(&w, &doc).await;
    assert_eq!(
        rights(&w, &doc).await,
        (Some(Via::Share), Some(Via::Share), Some(Via::Share))
    );
    let second = w.node("thread", w.topic, "Another recipient").await;
    w.change(
        w.maker,
        Target::Live(second),
        json!({"rows": [{"permission": "thread.interact", "add": [{"holder": user_holder(giver)}]}]}),
    )
    .await
    .unwrap();
    assert!(w.can(giver, second, P::ThreadInteract).await);
    sqlx::query("INSERT INTO settings(node_id,key,value) VALUES($1,'sharing.allow_holders','false') ON CONFLICT(node_id,key) DO UPDATE SET value=EXCLUDED.value")
        .bind(doc.node).execute(&w.db.pool).await.unwrap();
    assert!(can(&w, giver, &doc, P::DocManage).await);
    assert!(matches!(
        change(
            &w,
            giver,
            &doc,
            json!({"rows": [{"permission": "doc.manage", "add": [{"holder": {"kind": "agents", "id": second}}]}]}),
            false,
        )
        .await,
        Err(ChangeError::Denied(_))
    ));
    assert_eq!(rights(&w, &doc).await, (None, None, None));
    assert!(can(&w, human, &doc, P::DocEdit).await);
    assert_eq!(contributions(&w, &doc).await, original);
    sqlx::query(
        "UPDATE settings SET value='true' WHERE node_id=$1 AND key='sharing.allow_holders'",
    )
    .bind(doc.node)
    .execute(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(
        rights(&w, &doc).await,
        (Some(Via::Share), Some(Via::Share), Some(Via::Share))
    );
    assert_eq!(contributions(&w, &doc).await, original);
}

#[tokio::test]
async fn an_agent_loan_follows_role_loss_and_regain_without_rewriting_it() {
    let w = World::new().await;
    let doc = document(&w, false).await;
    let giver = driver(&w, "giver").await;
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(giver)
        .execute(&w.db.pool)
        .await
        .unwrap();
    let role = w.give(doc.node, giver, Role::Developer).await;
    assert!(can(&w, giver, &doc, P::DocManage).await);
    give(
        &w,
        giver,
        &doc,
        P::DocManage,
        Holder::Agents { id: w.thread },
    )
    .await;
    let original = contributions(&w, &doc).await;
    assert_eq!(rights(&w, &doc).await.2, Some(Via::Share));
    {
        let mut conn = w.conn().await;
        assert!(
            access::remove_role(&mut conn, role, w.boss, w.now.get())
                .await
                .unwrap()
        );
    }
    assert!(!can(&w, giver, &doc, P::DocManage).await);
    let elevated = w
        .principal(Caller::session(giver).with_admin_mode(true))
        .await;
    {
        let mut conn = w.conn().await;
        assert!(
            documents::check(
                &mut conn,
                &elevated,
                doc.id,
                P::DocManage,
                Policy::default(),
            )
            .await
            .unwrap()
            .is_allowed()
        );
    }
    let origin = orch_db::audit::Origin {
        as_admin: Some(giver),
        ..Default::default()
    };
    assert_eq!(
        orch_db::audit::scope_origin(origin, rights(&w, &doc))
            .await
            .2,
        None,
        "the giver's lost ordinary right cannot be replaced by ambient As-admin"
    );
    assert_eq!(contributions(&w, &doc).await, original);
    w.give(doc.node, giver, Role::Developer).await;
    assert!(can(&w, giver, &doc, P::DocManage).await);
    assert_eq!(rights(&w, &doc).await.2, Some(Via::Share));
    assert_eq!(contributions(&w, &doc).await, original);
}

#[tokio::test]
async fn an_unknown_giver_cannot_supply_a_right_or_hide_an_independent_valid_loan() {
    let w = World::new().await;
    let unknown = document(&w, true).await;
    let valid = document(&w, true).await;
    let giver = driver(&w, "giver").await;
    give(&w, w.boss, &valid, P::DocManage, Holder::User { id: giver }).await;
    give(
        &w,
        giver,
        &valid,
        P::DocManage,
        Holder::Agents { id: w.thread },
    )
    .await;
    {
        let mut conn = w.conn().await;
        access::testing::add_entry(
            &mut conn,
            &access::PageKey::new("document", unknown.id, "live"),
            "any",
            "doc.manage",
            stored(&Holder::Agents { id: w.thread }),
            None,
            w.now.get(),
        )
        .await
        .unwrap();
    }
    let rows = contributions(&w, &unknown).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, None);
    assert_eq!(
        rights(&w, &valid).await,
        (Some(Via::Share), Some(Via::Share), Some(Via::Share))
    );
    assert_eq!(rights(&w, &unknown).await, (None, None, None));
}

#[tokio::test]
async fn legacy_reach_document_and_message_choices_supply_no_rights_without_entries() {
    let w = World::new().await;
    let sibling = w.node("thread", w.topic, "Sibling").await;
    let id = new_id();
    let mut conn = w.conn().await;
    rows::insert(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: w.topic,
            org_id: w.root,
            owner_id: w.boss,
            created_by_thread: None,
            created_by_agent: None,
            title: "No implicit loan",
            now: w.now.get(),
        },
    )
    .await
    .unwrap();
    documents::start(&mut conn, id, w.boss, w.now)
        .await
        .unwrap();
    let doc = Document::of_row(&rows::get(&mut conn, id).await.unwrap().unwrap());
    for (key, value) in [
        ("sharing.agent_reach", json!("project")),
        ("sharing.agent_documents", json!("write")),
        ("sharing.agent_messages", json!(true)),
    ] {
        sqlx::query("INSERT INTO settings(node_id,key,value) VALUES($1,$2,$3) ON CONFLICT(node_id,key) DO UPDATE SET value=EXCLUDED.value")
            .bind(w.topic).bind(key).bind(value).execute(&mut *conn).await.unwrap();
    }
    let agent = Agent::of(&mut conn, w.thread).await.unwrap().unwrap();
    let reach = Reach::load(
        &mut conn,
        agent,
        AgentReach::Project,
        w.now,
        &Things {
            nodes: vec![w.topic, sibling],
            documents: vec![doc.clone()],
            people_chats: Vec::new(),
        },
    )
    .await
    .unwrap()
    .with_document_policy(DocumentPolicy::Write)
    .with_topic_messages(true);
    assert!(reach.conversation(w.thread, false).reads());
    assert_eq!(
        reach.conversation(sibling, false),
        Default::default(),
        "legacy reach and messaging choices are not a contribution"
    );
    assert_eq!(
        reach.document(&doc),
        Default::default(),
        "the former document default is not a hidden giver"
    );
    assert_eq!(reach.documents_of(w.topic), Default::default());
    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM settings WHERE node_id=$1 AND key LIKE 'sharing.agent_%'",
    )
    .bind(w.topic)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        stored, 3,
        "ignoring old values does not destructively delete them"
    );
}

async fn administrator_without_document_rights(w: &World, doc: &Document) -> Id {
    let giver = driver(w, "inspector").await;
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(giver)
        .execute(&w.db.pool)
        .await
        .unwrap();
    assert!(!can(w, giver, doc, P::DocManage).await);
    giver
}

#[tokio::test]
async fn as_admin_page_projects_human_giving_but_no_agent_loan_without_ordinary_rights() {
    let w = World::new().await;
    let doc = document(&w, true).await;
    let giver = administrator_without_document_rights(&w, &doc).await;
    let page = w
        .read_as(
            Caller::session(giver).with_admin_mode(true),
            Target::Resource(doc.object()),
        )
        .await;
    assert!(page.writable);
    assert!(
        !page.gives.choices.is_empty(),
        "ordinary administration of human grants remains available"
    );
    assert!(
        page.gives.agents.is_empty(),
        "As-admin must not be advertised as lendable agent authority"
    );
}

#[tokio::test]
async fn as_admin_page_cannot_create_a_dormant_agent_loan_from_unheld_rights() {
    let w = World::new().await;
    let doc = document(&w, true).await;
    let giver = administrator_without_document_rights(&w, &doc).await;
    let result = w.change_as(
        Caller::session(giver).with_admin_mode(true),
        Target::Resource(doc.object()),
        json!({"rows":[{"permission":"doc.manage","add":[{"holder":Holder::Agents{id:w.thread}}]}]}),
        Policy::default(),
    ).await;
    assert!(
        result.is_err(),
        "an ordinary-rights refusal must precede writing an agent contribution: {result:?}"
    );
    assert!(contributions(&w, &doc).await.is_empty());
    let reader = w.member("human_recipient").await;
    w.change_as(
        Caller::session(giver).with_admin_mode(true),
        Target::Resource(doc.object()),
        json!({"rows":[{"permission":"doc.manage","add":[{"holder":Holder::User{id:reader}}]}]}),
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(
        can(&w, reader, &doc, P::DocManage).await,
        "human administration keeps its existing rules"
    );
}

#[tokio::test]
async fn agent_giving_exposes_documents_only_and_preserves_token_area_limits() {
    let w = World::new().await;
    let source = w.node("topic", w.project, "Documents only").await;
    let giver = w.member("document_editor").await;
    w.change_as(
        Caller::session(w.boss).with_admin_mode(false),
        Target::Live(source),
        json!({"rows":(["topic.view","doc.edit"].map(|permission|
            json!({"permission":permission,"add":[{"holder":Holder::User{id:giver}}]})))}),
        Policy::default(),
    )
    .await
    .unwrap();
    let principal = w
        .principal(Caller::session(giver).with_admin_mode(false))
        .await;
    let page = orch_authz::pages::read(
        &mut *w.conn().await,
        &principal,
        Target::Live(source),
        Policy {
            holders_share: true,
            ..Policy::default()
        },
    )
    .await
    .unwrap();
    assert!(
        page.gives.holders_share,
        "this case explicitly enables ordinary holder sharing"
    );
    assert!(
        page.gives.agents.is_empty(),
        "this giver cannot give every area of a whole preset"
    );
    assert!(page.gives.agent_permissions.contains(&P::DocView));
    assert!(page.gives.agent_permissions.contains(&P::DocEdit));
    assert!(!page.gives.agent_permissions.contains(&P::DocManage));
    assert!(!page.gives.agent_permissions.contains(&P::ChatView));
    assert!(!page.gives.agent_permissions.contains(&P::ThreadView));

    let token = orch_authz::TokenGrant::from_stored(
        &["topic.view".into(), "doc.view".into()],
        Some(source),
    )
    .unwrap();
    let limited = w
        .read_as(Caller::token(w.boss, token), Target::Live(source))
        .await;
    assert_eq!(limited.gives.agent_permissions, vec![P::DocView]);
    assert!(
        limited.gives.agents.is_empty(),
        "the owner's token cannot advertise other conversation areas"
    );
}
