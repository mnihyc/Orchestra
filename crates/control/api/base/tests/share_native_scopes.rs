//! Native sharing records preserve source/recipient selection and independent current contributions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod desk;
mod places;
mod rig;
mod support;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::documents::{NewDocumentAccess, NodeDocumentCreate};
use orch_api_contract::share::{
    AgentShareList, AgentShareListQuery, AgentShareSide, ShareAgentCreate, ShareDocumentsLevel,
    ShareParts, ShareTo, ShareToThread, ShareWhat, ShareWhatDocument, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use rig::Scene;
use serde_json::json;
async fn key(s: &Scene, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native sharing records",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
fn rule(kind: &str, id: Id, operation: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn value(reply: orch_api_core::testing::RawReply) -> Json {
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn document(s: &Scene, title: &str) -> Id {
    document_by(s, &s.ada, title).await
}
async fn document_by(s: &Scene, by: &places::Who, title: &str) -> Id {
    if by.id() != s.ada.id() {
        s.rig
            .world
            .app
            .call(
                orch_api_contract::access::PermissionPageUpdate,
                &[&"nodes", &s.tree.topic.base.id],
            )
            .token(&s.ada.token)
            .raw_body(&json!({"rows":[{"permission":"doc.create","add":[{"holder":{"kind":"user","id":by.id()}}]}]}))
            .send()
            .await
            .ok();
    }
    let mut body = desk::body(title);
    body.access = Some(NewDocumentAccess::Private);
    body.blocks = Some(vec![desk::paragraph("Private native source content")]);
    let reply = s
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&s.tree.topic.base.id])
        .token(&by.token)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(reply.status, 201, "{}", String::from_utf8_lossy(&reply.raw));
    let created: Json = serde_json::from_slice(&reply.raw).unwrap();
    serde_json::from_value(created["id"].clone()).unwrap()
}
async fn share(s: &Scene, document: Id, to: Id) -> Id {
    let body = ShareWithAgents {
        link_origin: None,
        read_first: None,
        viewers_read: None,
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: document,
        }),
        inside: None,
        download: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..Default::default()
        }),
        mode: ShareMode::Live,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: to }]),
        comment: None,
        expires_at: None,
        client_id: new_id().to_string(),
    };
    s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .shares[0]
        .id
}
#[tokio::test]
async fn share_list_source_and_recipient_sides_bind_the_native_record_subjects_and_names_separately()
 {
    let s = Scene::new().await;
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{
        AgentAccessChoice, AgentAccessWhere, CreateThread, CreateThreadBinding, ThreadCreate,
    };
    // This control creates exactly two document shares. An omitted birth choice
    // also creates the thread's preset share, which is covered by creation tests.
    let target = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            name: Some("Recipient name sentinel".into()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: s.on.provider,
                arrangement: None,
            }),
            model: Some(rig::MODEL.into()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            directory: Some("web".into()),
            agent_access: Some(AgentAccessChoice {
                where_: AgentAccessWhere::Nothing,
                parts: ShareParts::default(),
            }),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the ordinary recipient creation keeps full content")
        .base
        .id;
    let first = document(&s, "Selected source sentinel").await;
    let second = document(&s, "Unselected source sentinel").await;
    let wanted = share(&s, first, target).await;
    share(&s, second, target).await;
    let of = AgentShareListQuery {
        side: Some(AgentShareSide::Of),
    };
    let to = AgentShareListQuery {
        side: Some(AgentShareSide::To),
    };
    let ordinary = value(
        s.rig
            .world
            .app
            .call(AgentShareList, &[&"nodes", &target])
            .token(&s.ada.token)
            .query(&to)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary.as_array().unwrap().len(), 2);
    let source_only = key(
        &s,
        s.ada.id(),
        vec![rule("document", first, "AgentShareList")],
    )
    .await;
    let source = value(
        s.rig
            .world
            .app
            .call(AgentShareList, &[&"documents", &first])
            .token(&source_only)
            .query(&of)
            .send_bytes()
            .await,
    );
    assert_eq!(source.as_array().unwrap().len(), 1);
    assert_eq!(source[0]["id"], json!(wanted));
    assert_eq!(source[0]["what"]["title"], "Selected source sentinel");
    assert!(source[0]["to"]["name"].is_null());
    assert!(
        !serde_json::to_string(&source)
            .unwrap()
            .contains("Recipient name sentinel")
    );
    let recipient_only = key(
        &s,
        s.ada.id(),
        vec![rule("thread", target, "AgentShareList")],
    )
    .await;
    assert_eq!(
        value(
            s.rig
                .world
                .app
                .call(AgentShareList, &[&"nodes", &target])
                .token(&recipient_only)
                .query(&to)
                .send_bytes()
                .await
        ),
        json!([])
    );
    let both = key(
        &s,
        s.ada.id(),
        vec![
            rule("thread", target, "AgentShareList"),
            rule("document", first, "AgentShareList"),
        ],
    )
    .await;
    let selected = value(
        s.rig
            .world
            .app
            .call(AgentShareList, &[&"nodes", &target])
            .token(&both)
            .query(&to)
            .send_bytes()
            .await,
    );
    assert_eq!(selected.as_array().unwrap().len(), 1);
    assert_eq!(selected[0]["id"], json!(wanted));
    assert_eq!(selected[0]["what"]["id"], json!(first));
    assert_eq!(selected[0]["to"]["name"], "Recipient name sentinel");
    assert!(
        !serde_json::to_string(&selected)
            .unwrap()
            .contains("Unselected source sentinel")
    );
    let unrelated = key(&s, s.ada.id(), vec![rule("document", first, "DocumentGet")]).await;
    assert_eq!(
        s.rig
            .world
            .app
            .call(AgentShareList, &[&"documents", &first])
            .token(&unrelated)
            .query(&of)
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
}
#[tokio::test]
async fn share_list_keeps_future_holder_and_independent_giver_records_without_creating_control_or_viewer_entries()
 {
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document(&s, "Independent source").await;
    let first = share(&s, source, target).await;
    s.rig.world.app.call(orch_api_contract::access::PermissionPageUpdate,&[&"documents",&source]).token(&s.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","add":[{"holder":{"kind":"user","id":s.cai.id()}}]}]})).send().await.ok();
    let body = ShareWithAgents {
        link_origin: None,
        read_first: None,
        viewers_read: None,
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: source,
        }),
        inside: None,
        download: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..Default::default()
        }),
        mode: ShareMode::Live,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: target }]),
        comment: None,
        expires_at: None,
        client_id: new_id().to_string(),
    };
    let second = s
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .shares[0]
        .id;
    assert_ne!(
        second, first,
        "independent givers remain separate native contributions"
    );
    let future = s
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&s.ada.token)
        .body(&ShareWithAgents {
            link_origin: None,
            to: ShareTo::Nodes(vec![orch_api_contract::share::ShareToNode {
                node_id: s.tree.topic.base.id,
            }]),
            client_id: new_id().to_string(),
            ..body
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .shares[0]
        .id;
    let before:Vec<(Id,i64)>=sqlx::query_as("SELECT id,revision FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND removed_at IS NULL ORDER BY id")
        .bind(source).fetch_all(&s.rig.world.db.pool).await.unwrap();
    let source_key = key(
        &s,
        s.ada.id(),
        vec![
            rule("document", source, "AgentShareList"),
            rule("thread", target, "AgentShareList"),
        ],
    )
    .await;
    let ordinary = value(
        s.rig
            .world
            .app
            .call(AgentShareList, &[&"documents", &source])
            .token(&s.ada.token)
            .send_bytes()
            .await,
    );
    let scoped = value(
        s.rig
            .world
            .app
            .call(AgentShareList, &[&"documents", &source])
            .token(&source_key)
            .send_bytes()
            .await,
    );
    assert_eq!(scoped.as_array().unwrap().len(), 3);
    for id in [first, second, future] {
        let actual = scoped
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(id))
            .unwrap();
        let expected = ordinary
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(id))
            .unwrap();
        assert_eq!(actual["current"], expected["current"]);
        assert!(actual.get("viewer_entries").is_none());
    }
    let after:Vec<(Id,i64)>=sqlx::query_as("SELECT id,revision FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND removed_at IS NULL ORDER BY id")
        .bind(source).fetch_all(&s.rig.world.db.pool).await.unwrap();
    assert_eq!(after, before);
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(target)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(inputs, 0);
    let named:Id=sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='user' AND holder_id=$2 AND permission='doc.view' AND removed_at IS NULL")
        .bind(source).bind(s.cai.id()).fetch_one(&s.rig.world.db.pool).await.unwrap();
    s.rig
        .world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"documents", &source],
        )
        .token(&s.ada.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","remove":[named]}]}))
        .send()
        .await
        .ok();
    let now = value(
        s.rig
            .world
            .app
            .call(AgentShareList, &[&"documents", &source])
            .token(&source_key)
            .send_bytes()
            .await,
    );
    assert_eq!(
        now.as_array().unwrap().len(),
        3,
        "loss of a giver's authority does not erase other contributions or history"
    );
    let inactive = now
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(second))
        .unwrap();
    let active = now
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(first))
        .unwrap();
    assert_eq!(inactive["current"]["state"], "inactive");
    assert_eq!(active["current"]["state"], "active");
}
const WAIT_NATIVE: std::time::Duration = std::time::Duration::from_secs(30);
#[tokio::test]
async fn share_list_publication_keeps_the_original_native_contribution_after_activity_wait() {
    for remove in [false, true] {
        let s = Scene::new().await;
        let target = s.thread().await.base.id;
        let source = document(&s, "Held sharing source").await;
        let id = share(&s, source, target).await;
        let scoped = key(
            &s,
            s.ada.id(),
            vec![rule("document", source, "AgentShareList")],
        )
        .await;
        let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let app = s.rig.world.app.clone();
        let action = tokio::spawn(async move {
            let request = app
                .call(AgentShareList, &[&"documents", &source])
                .token(&scoped);
            request.send_bytes().await
        });
        let reached=tokio::time::timeout(WAIT_NATIVE,async {loop {
            if action.is_finished(){break false;}
            let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&s.rig.world.db.pool).await.unwrap();if found{break true;}tokio::task::yield_now().await;
        }}).await.unwrap();
        assert!(
            reached,
            "the actual native successful read must reach bookkeeping"
        );
        if remove {
            sqlx::query("UPDATE permission_entries SET removed_at=$3 WHERE object_kind='document' AND object_id=$1 AND holder_id=$2 AND holder_kind='agents'")
            .bind(source).bind(target).bind(s.rig.world.now()).execute(&s.rig.world.db.pool).await.unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT_NATIVE, action)
            .await
            .unwrap()
            .unwrap();
        if remove {
            assert_eq!(
                reply.problem().code,
                orch_api_contract::common::ErrorCode::AuthzDenied
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Held sharing source"));
        } else {
            assert_eq!(value(reply)[0]["id"], json!(id));
        }
    }
}

fn operation_rule(kind: &str, id: Id, operation: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}
async fn share_by(s: &Scene, by: &places::Who, source: Id, target: Id) -> Id {
    let body = ShareWithAgents {
        link_origin: None,
        read_first: None,
        viewers_read: None,
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: source,
        }),
        inside: None,
        download: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..Default::default()
        }),
        mode: ShareMode::Live,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: target }]),
        comment: None,
        expires_at: None,
        client_id: new_id().to_string(),
    };
    s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&by.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .shares[0]
        .id
}
#[tokio::test]
async fn share_updates_resolve_the_native_stored_source_and_keep_pure_grants_without_recipient_control()
 {
    use orch_api_contract::share::{ShareAgentUpdate, ShareAgentUpdateBody};
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document_by(&s, &s.cai, "Update source sentinel").await;
    let id = share_by(&s, &s.cai, source, target).await;
    let held = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::access::EffectiveAccessGet,
            &[&"nodes", &target],
        )
        .token(&s.ada.token)
        .query(&orch_api_contract::access::EffectiveAccessGetQuery {
            user_id: s.cai.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        held.permissions
            .iter()
            .any(|p| p.permission == "thread.view")
    );
    assert!(
        !held
            .permissions
            .iter()
            .any(|p| p.permission == "thread.interact")
    );
    let manage = key(
        &s,
        s.cai.id(),
        vec![operation_rule(
            "document",
            source,
            "ShareAgentUpdate.grant",
            &["manage"],
        )],
    )
    .await;
    let body = ShareAgentUpdateBody {
        read_first: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Edit),
            ..Default::default()
        }),
        expires_at: None,
    };
    let receipt = value(
        s.rig
            .world
            .app
            .call(ShareAgentUpdate, &[&id])
            .token(&manage)
            .body(&body)
            .send_bytes()
            .await,
    );
    assert_eq!(receipt, json!({"projection":"receipt","id":id}));
    let edit:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents' AND holder_id=$2 AND permission='doc.edit' AND removed_at IS NULL)")
        .bind(source).bind(target).fetch_one(&s.rig.world.db.pool).await.unwrap();
    assert!(edit);
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(target)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(inputs, 0);
    let surrogate = key(
        &s,
        s.cai.id(),
        vec![operation_rule(
            "document",
            id,
            "ShareAgentUpdate.grant",
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        s.rig
            .world
            .app
            .call(ShareAgentUpdate, &[&id])
            .token(&surrogate)
            .body(&body)
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    let readable = key(
        &s,
        s.cai.id(),
        vec![operation_rule(
            "document",
            source,
            "ShareAgentUpdate.grant",
            &["manage", "read"],
        )],
    )
    .await;
    let full = value(
        s.rig
            .world
            .app
            .call(ShareAgentUpdate, &[&id])
            .token(&readable)
            .body(&ShareAgentUpdateBody {
                parts: Some(ShareParts {
                    documents: Some(ShareDocumentsLevel::Read),
                    ..Default::default()
                }),
                ..body
            })
            .send_bytes()
            .await,
    );
    assert_eq!(full["id"], json!(id));
    assert_eq!(full["what"]["title"], "Update source sentinel");
    assert!(full["to"]["name"].is_null());
    assert!(full.get("viewer_entries").is_none());
}
#[tokio::test]
async fn share_updates_select_effective_first_reading_changes_from_the_stored_flag_and_keep_ordinary_control()
 {
    use orch_api_contract::share::{ShareAgentUpdate, ShareAgentUpdateBody};
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document(&s, "First reading source").await;
    let id = share(&s, source, target).await;
    let body = ShareAgentUpdateBody {
        read_first: Some(true),
        parts: None,
        expires_at: None,
    };
    let grant_only = key(
        &s,
        s.ada.id(),
        vec![operation_rule(
            "document",
            source,
            "ShareAgentUpdate.grant",
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        s.rig
            .world
            .app
            .call(ShareAgentUpdate, &[&id])
            .token(&grant_only)
            .body(&body)
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    let control = key(
        &s,
        s.ada.id(),
        vec![
            operation_rule(
                "document",
                source,
                "ShareAgentUpdate.read_first",
                &["manage", "read"],
            ),
            operation_rule("thread", target, "ShareAgentUpdate.read_first", &["write"]),
        ],
    )
    .await;
    let result = value(
        s.rig
            .world
            .app
            .call(ShareAgentUpdate, &[&id])
            .token(&control)
            .body(&body)
            .send_bytes()
            .await,
    );
    assert_eq!(result["read_first"], true);
    assert!(result["to"]["name"].is_null());
    let before:Vec<(Id,i64)>=sqlx::query_as("SELECT id,revision FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents' AND removed_at IS NULL ORDER BY id")
        .bind(source).fetch_all(&s.rig.world.db.pool).await.unwrap();
    let receipt = value(
        s.rig
            .world
            .app
            .call(ShareAgentUpdate, &[&id])
            .token(&grant_only)
            .body(&body)
            .send_bytes()
            .await,
    );
    assert_eq!(
        receipt,
        json!({"projection":"receipt","id":id}),
        "an unchanged true flag is a pure grant variant"
    );
    let after:Vec<(Id,i64)>=sqlx::query_as("SELECT id,revision FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents' AND removed_at IS NULL ORDER BY id")
        .bind(source).fetch_all(&s.rig.world.db.pool).await.unwrap();
    assert_eq!(after, before);
    let no_source_read = key(
        &s,
        s.ada.id(),
        vec![
            operation_rule(
                "document",
                source,
                "ShareAgentUpdate.read_first",
                &["manage"],
            ),
            operation_rule("thread", target, "ShareAgentUpdate.read_first", &["write"]),
        ],
    )
    .await;
    assert_eq!(
        s.rig
            .world
            .app
            .call(ShareAgentUpdate, &[&id])
            .token(&no_source_read)
            .body(&ShareAgentUpdateBody {
                read_first: Some(false),
                ..body.clone()
            })
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    let cleared = value(
        s.rig
            .world
            .app
            .call(ShareAgentUpdate, &[&id])
            .token(&control)
            .body(&ShareAgentUpdateBody {
                read_first: Some(false),
                ..body
            })
            .send_bytes()
            .await,
    );
    assert_eq!(cleared["read_first"], false);
    let source = document_by(&s, &s.cai, "Viewer control source").await;
    let viewer_share = share_by(&s, &s.cai, source, target).await;
    let viewer = key(
        &s,
        s.cai.id(),
        vec![
            operation_rule(
                "document",
                source,
                "ShareAgentUpdate.read_first",
                &["manage", "read"],
            ),
            operation_rule("thread", target, "ShareAgentUpdate.read_first", &["write"]),
        ],
    )
    .await;
    assert_eq!(
        s.rig
            .world
            .app
            .call(ShareAgentUpdate, &[&viewer_share])
            .token(&viewer)
            .body(&ShareAgentUpdateBody {
                read_first: Some(true),
                parts: None,
                expires_at: None
            })
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
}
#[tokio::test]
async fn share_update_checks_the_original_key_after_the_native_contribution_row_wait() {
    use orch_api_contract::share::{ShareAgentUpdate, ShareAgentUpdateBody};
    for expire in [false, true] {
        let s = Scene::new().await;
        let target = s.thread().await.base.id;
        let source = document(&s, "Waited update source").await;
        let id = share(&s, source, target).await;
        let scoped = key(
            &s,
            s.ada.id(),
            vec![operation_rule(
                "document",
                source,
                "ShareAgentUpdate.grant",
                &["manage"],
            )],
        )
        .await;
        let entry:Id=sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_id=$2 AND holder_kind='agents' AND permission='doc.view' AND removed_at IS NULL")
            .bind(source).bind(target).fetch_one(&s.rig.world.db.pool).await.unwrap();
        let before: i64 = sqlx::query_scalar("SELECT revision FROM permission_entries WHERE id=$1")
            .bind(entry)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
        let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM permission_entries WHERE id=$1 FOR KEY SHARE")
            .bind(entry)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let body = ShareAgentUpdateBody {
            read_first: None,
            parts: None,
            expires_at: Some(orch_types::Time::new(
                s.rig.world.now() + chrono::Duration::days(3),
            )),
        };
        let app = s.rig.world.app.clone();
        let action = tokio::spawn(async move {
            let request = app
                .call(ShareAgentUpdate, &[&id])
                .token(&scoped)
                .body(&body);
            request.send_bytes().await
        });
        let reached=tokio::time::timeout(WAIT_NATIVE,async {loop {
            if action.is_finished(){break false;}
            let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM permission_entries WHERE id=ANY($1) ORDER BY id FOR UPDATE%')")
                .bind(pid).fetch_one(&s.rig.world.db.pool).await.unwrap();if found{break true;}tokio::task::yield_now().await;
        }}).await.unwrap();
        assert!(
            reached,
            "the actual native contribution lock must be reached"
        );
        if expire {
            s.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT_NATIVE, action)
            .await
            .unwrap()
            .unwrap();
        let after: i64 = sqlx::query_scalar("SELECT revision FROM permission_entries WHERE id=$1")
            .bind(entry)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
        if expire {
            assert!(matches!(reply.status.as_u16(), 401 | 403));
            assert_eq!(after, before);
        } else {
            assert_eq!(value(reply), json!({"projection":"receipt","id":id}));
            assert!(after > before);
        }
    }
}

#[tokio::test]
async fn share_native_preparation_locks_the_page_subject_before_its_grant_entries() {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ObjectKind;
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document(&s, "Native page ordering").await;
    let id = share(&s, source, target).await;
    let entry: Id = sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents' AND holder_id=$2 AND permission='doc.view' AND removed_at IS NULL")
        .bind(source).bind(target).fetch_one(&s.rig.world.db.pool).await.unwrap();
    let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    orch_authz::pages::lock_change_target(
        &mut blocker,
        orch_authz::pages::Target::Resource(ObjectRef {
            kind: ObjectKind::Document,
            id: source,
        }),
    )
    .await
    .unwrap();
    let mut native_tx = s.rig.world.db.pool.begin().await.unwrap();
    let native_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *native_tx)
        .await
        .unwrap();
    let engine = s.rig.world.app.app.engine.clone();
    let mut action = tokio::spawn(async move {
        let record = engine.lock_agent_share_record_in(&mut native_tx, id).await;
        native_tx.rollback().await.unwrap();
        record
    });
    let reached = tokio::time::timeout(WAIT_NATIVE, async {
        loop {
            if action.is_finished() { break false; }
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock' AND $2=ANY(pg_blocking_pids(pid)) AND (query LIKE '%document_tree_lock%' OR query LIKE '%FROM documents%FOR UPDATE%'))")
                .bind(native_pid).bind(blocker_pid).fetch_one(&s.rig.world.db.pool).await.unwrap();
            if blocked { break true; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap_or(false);
    let entry_lock = if reached {
        sqlx::query_scalar::<_, Id>(
            "SELECT id FROM permission_entries WHERE id=$1 FOR UPDATE NOWAIT",
        )
        .bind(entry)
        .fetch_optional(&mut *blocker)
        .await
    } else {
        Ok(None)
    };
    blocker.rollback().await.unwrap();
    let completed = tokio::time::timeout(WAIT_NATIVE, &mut action).await;
    if completed.is_err() {
        action.abort();
        let _ = action.await;
    }
    let record = completed
        .expect("native preparation must drain after the page lock is released")
        .unwrap()
        .unwrap();
    assert!(reached, "the actual native source lock must be reached");
    assert!(
        entry_lock.is_ok(),
        "the native page writer must not wait on a grant held by the source waiter: {entry_lock:?}"
    );
    assert_eq!(entry_lock.unwrap(), Some(entry));
    assert_eq!(record.id, id);
}

#[tokio::test]
async fn completed_share_update_returns_its_receipt_after_activity_wait_without_replaying_the_delta()
 {
    use orch_api_contract::share::{ShareAgentUpdate, ShareAgentUpdateBody};
    for expire in [false, true] {
        let s = Scene::new().await;
        let target = s.thread().await.base.id;
        let source = document(&s, "Committed update source").await;
        let id = share(&s, source, target).await;
        let scoped = key(
            &s,
            s.ada.id(),
            vec![
                operation_rule(
                    "document",
                    source,
                    "ShareAgentUpdate.grant",
                    &["manage", "read"],
                ),
                operation_rule("thread", target, "ShareAgentUpdate.grant", &["read"]),
            ],
        )
        .await;
        let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let body = ShareAgentUpdateBody {
            read_first: None,
            parts: None,
            expires_at: Some(orch_types::Time::new(
                s.rig.world.now() + chrono::Duration::days(3),
            )),
        };
        let app = s.rig.world.app.clone();
        let action = tokio::spawn(async move {
            let request = app
                .call(ShareAgentUpdate, &[&id])
                .token(&scoped)
                .body(&body);
            request.send_bytes().await
        });
        let reached=tokio::time::timeout(WAIT_NATIVE,async {loop {
            if action.is_finished(){break false;}
            let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&s.rig.world.db.pool).await.unwrap();if found{break true;}tokio::task::yield_now().await;
        }}).await.unwrap();
        assert!(
            reached,
            "the native writer must commit before bookkeeping waits"
        );
        let (entry,before):(Id,String)=sqlx::query_as("SELECT id,xmin::text FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_id=$2 AND holder_kind='agents' AND permission='doc.view' AND removed_at IS NULL")
            .bind(source).bind(target).fetch_one(&s.rig.world.db.pool).await.unwrap();
        let committed: bool =
            sqlx::query_scalar("SELECT expires_at IS NOT NULL FROM permission_entries WHERE id=$1")
                .bind(entry)
                .fetch_one(&s.rig.world.db.pool)
                .await
                .unwrap();
        assert!(committed);
        if expire {
            s.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT_NATIVE, action)
            .await
            .unwrap()
            .unwrap();
        let result = value(reply);
        if expire {
            assert_eq!(result, json!({"projection":"receipt","id":id}));
        } else {
            assert_eq!(result["id"], json!(id));
            assert_eq!(result["what"]["title"], "Committed update source");
        }
        let after: String =
            sqlx::query_scalar("SELECT xmin::text FROM permission_entries WHERE id=$1")
                .bind(entry)
                .fetch_one(&s.rig.world.db.pool)
                .await
                .unwrap();
        assert_eq!(
            after, before,
            "response publication never repeats the committed delta"
        );
    }
}

#[derive(Default)]
struct RemovalParcels {
    withdrawn: std::sync::Mutex<Vec<Id>>,
    hold: bool,
    entered: tokio::sync::Notify,
    released: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl orch_engine::sharing::Parcels for RemovalParcels {
    async fn keep(
        &self,
        _conn: &mut orch_db::Conn,
        _id: Id,
        _path: &str,
        _files: &orch_engine::copies::Files,
    ) -> Result<Option<Vec<u8>>, orch_engine::sharing::ParcelFault> {
        Ok(None)
    }
    async fn withdraw(
        &self,
        _conn: &mut orch_db::Conn,
        copy: &orch_db::queries::shares::CopyRow,
    ) -> Result<(), orch_engine::sharing::ParcelFault> {
        self.withdrawn.lock().unwrap().push(copy.id);
        Ok(())
    }
    async fn withdraw_before(
        &self,
        _conn: &mut orch_db::Conn,
        copy: &orch_db::queries::shares::CopyRow,
        before: orch_engine::sharing::BeforeParcel<'_>,
    ) -> Result<(), orch_engine::sharing::ParcelFailure> {
        if self.hold {
            self.entered.notify_one();
            self.released.notified().await;
        }
        let mut withdrawn = self.withdrawn.lock().unwrap();
        before().map_err(orch_authz::pages::EffectFailure::BeforeEffect)?;
        withdrawn.push(copy.id);
        Ok(())
    }
    async fn write_file(
        &self,
        _conn: &mut orch_db::Conn,
        _thread: Id,
        _path: &str,
        _bytes: &[u8],
    ) -> Result<orch_engine::sharing::FileWritten, orch_engine::sharing::ParcelFault> {
        Ok(orch_engine::sharing::FileWritten::Ready)
    }
}
async fn copy_by(s: &Scene, by: &places::Who, source: Id, target: Id) -> Id {
    s.rig.tools_of_agents(json!("mcp")).await;
    let body = ShareWithAgents {
        link_origin: None,
        read_first: None,
        viewers_read: None,
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: source,
        }),
        inside: None,
        download: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..Default::default()
        }),
        mode: ShareMode::Snapshot,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: target }]),
        comment: None,
        expires_at: None,
        client_id: new_id().to_string(),
    };
    s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&by.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .shares[0]
        .id
}
#[tokio::test]
async fn share_removal_uses_the_stored_source_without_recipient_control_and_keeps_other_contributions()
 {
    use orch_api_contract::share::ShareAgentDelete;
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document_by(&s, &s.cai, "Removal source").await;
    let id = share_by(&s, &s.cai, source, target).await;
    s.rig.world.app.call(orch_api_contract::access::PermissionPageUpdate,&[&"documents",&source]).token(&s.cai.token)
        .raw_body(&json!({"rows":[{"permission":"doc.view","add":[{"holder":{"kind":"user","id":s.ada.id()}}]}]})).send().await.ok();
    let other = share_by(&s, &s.ada, source, target).await;
    assert_ne!(other, id);
    let other_entries:Vec<Id>=sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_id=$2 AND holder_kind='agents' AND created_by=$3 AND removed_at IS NULL ORDER BY id")
        .bind(source).bind(target).bind(s.ada.id()).fetch_all(&s.rig.world.db.pool).await.unwrap();
    assert!(!other_entries.is_empty());

    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(target)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    let wrong = key(
        &s,
        s.cai.id(),
        vec![operation_rule(
            "document",
            id,
            "ShareAgentDelete",
            &["manage"],
        )],
    )
    .await;
    assert_eq!(
        s.rig
            .world
            .app
            .call(ShareAgentDelete, &[&id])
            .token(&wrong)
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    let key = key(
        &s,
        s.cai.id(),
        vec![operation_rule(
            "document",
            source,
            "ShareAgentDelete",
            &["manage"],
        )],
    )
    .await;
    let result = s
        .rig
        .world
        .app
        .call(ShareAgentDelete, &[&id])
        .token(&key)
        .send_bytes()
        .await;
    assert_eq!(result.status, 204);
    assert!(result.raw.is_empty());
    let remaining:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents' AND holder_id=$2 AND removed_at IS NULL")
        .bind(source).bind(target).fetch_one(&s.rig.world.db.pool).await.unwrap();
    assert_eq!(usize::try_from(remaining).unwrap(), other_entries.len());
    let other_after:Vec<Id>=sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_id=$2 AND holder_kind='agents' AND created_by=$3 AND removed_at IS NULL ORDER BY id")
        .bind(source).bind(target).bind(s.ada.id()).fetch_all(&s.rig.world.db.pool).await.unwrap();
    assert_eq!(other_after, other_entries);

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(target)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, before);
    let held = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::access::EffectiveAccessGet,
            &[&"nodes", &target],
        )
        .token(&s.ada.token)
        .query(&orch_api_contract::access::EffectiveAccessGetQuery {
            user_id: s.cai.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        !held
            .permissions
            .iter()
            .any(|p| p.permission == "thread.interact")
    );
}
#[tokio::test]
async fn share_copy_removal_keeps_native_pending_delivered_and_retained_source_cleanup_with_manage_only()
 {
    use orch_api_contract::share::ShareAgentDelete;
    for delivered in [false, true] {
        let s = Scene::new().await;
        let target = s.thread().await.base.id;
        let source = document(&s, "Retained copy source sentinel").await;
        let parcels = std::sync::Arc::new(RemovalParcels::default());
        s.rig.engine.attach_parcels(parcels.clone());
        let id = copy_by(&s, &s.ada, source, target).await;
        if delivered {
            sqlx::query("UPDATE agent_shares SET state='delivered',environment_id=$2,delivered_at=$3 WHERE id=$1").bind(id).bind(s.on.environment).bind(s.rig.world.now()).execute(&s.rig.world.db.pool).await.unwrap();
        }
        sqlx::query("UPDATE documents SET deleted_at=$2 WHERE id=$1")
            .bind(source)
            .bind(s.rig.world.now())
            .execute(&s.rig.world.db.pool)
            .await
            .unwrap();
        let scoped = key(
            &s,
            s.ada.id(),
            vec![operation_rule(
                "document",
                source,
                "ShareAgentDelete",
                &["manage"],
            )],
        )
        .await;
        let reply = s
            .rig
            .world
            .app
            .call(ShareAgentDelete, &[&id])
            .token(&scoped)
            .send_bytes()
            .await;
        assert_eq!(reply.status, 204, "{}", String::from_utf8_lossy(&reply.raw));
        let state: String = sqlx::query_scalar("SELECT state FROM agent_shares WHERE id=$1")
            .bind(id)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
        assert_eq!(state, "revoked");
        assert_eq!(*parcels.withdrawn.lock().unwrap(), vec![id]);
        let notices: Json =
            sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id=$1")
                .bind(target)
                .fetch_one(&s.rig.world.db.pool)
                .await
                .unwrap();
        let last = notices.as_array().unwrap().last().unwrap();
        assert_eq!(last["title"], "No longer shared with you");
        assert!(
            !serde_json::to_string(last)
                .unwrap()
                .contains("Retained copy source sentinel")
        );
    }
}
#[tokio::test]
async fn share_copy_removal_checks_after_native_keeper_wait_and_leaves_the_copy_on_expiry() {
    use orch_api_contract::share::ShareAgentDelete;
    for expire in [false, true] {
        let s = Scene::new().await;
        let target = s.thread().await.base.id;
        let source = document(&s, "Keeper waited source").await;
        let parcels = std::sync::Arc::new(RemovalParcels {
            hold: true,
            ..Default::default()
        });
        s.rig.engine.attach_parcels(parcels.clone());
        let id = copy_by(&s, &s.ada, source, target).await;
        let scoped = key(
            &s,
            s.ada.id(),
            vec![operation_rule(
                "document",
                source,
                "ShareAgentDelete",
                &["manage"],
            )],
        )
        .await;
        let app = s.rig.world.app.clone();
        let mut action = tokio::spawn(async move {
            let request = app.call(ShareAgentDelete, &[&id]).token(&scoped);
            request.send_bytes().await
        });
        let reached = tokio::time::timeout(WAIT_NATIVE, parcels.entered.notified())
            .await
            .is_ok();
        if !reached {
            parcels.released.notify_one();
            action.abort();
        }
        if reached && expire {
            s.rig.world.advance(chrono::Duration::hours(2));
        }
        parcels.released.notify_one();
        let completed = tokio::time::timeout(WAIT_NATIVE, &mut action).await;
        if completed.is_err() {
            action.abort();
            let _ = action.await;
        }
        assert!(
            reached,
            "the actual native keeper boundary must be reached: {completed:?}"
        );
        let reply = completed
            .expect("the native keeper request must drain after release")
            .unwrap();
        let state: String = sqlx::query_scalar("SELECT state FROM agent_shares WHERE id=$1")
            .bind(id)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
        if expire {
            assert!(matches!(reply.status.as_u16(), 401 | 403));
            assert_eq!(state, "pending");
            assert!(parcels.withdrawn.lock().unwrap().is_empty());
        } else {
            assert_eq!(reply.status, 204);
            assert_eq!(state, "revoked");
            assert_eq!(*parcels.withdrawn.lock().unwrap(), vec![id]);
        }
    }
}

#[tokio::test]
async fn share_pending_copy_withdrawal_rechecks_after_the_real_blob_pin_wait() {
    use orch_api_contract::share::ShareAgentDelete;
    for expire in [false, true] {
        let s = Scene::new().await;
        let target = s.thread().await.base.id;
        let source = document(&s, "Pinned workspace copy").await;
        let id = copy_by(&s, &s.ada, source, target).await;
        let manifest = {
            let mut conn = s.rig.world.conn().await;
            orch_db::queries::shares::copy_parcel_of(&mut conn, id)
                .await
                .unwrap()
                .unwrap()
                .manifest
                .unwrap()
        };
        let scoped = key(
            &s,
            s.ada.id(),
            vec![operation_rule(
                "document",
                source,
                "ShareAgentDelete",
                &["manage"],
            )],
        )
        .await;
        let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1 FOR UPDATE")
            .bind(manifest.as_bytes().to_vec())
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        assert!(
            pins > 0,
            "the actual workspace keeper must pin this copy's manifest"
        );
        let app = s.rig.world.app.clone();
        let mut action = tokio::spawn(async move {
            let request = app.call(ShareAgentDelete, &[&id]).token(&scoped);
            request.send_bytes().await
        });
        let reached = tokio::time::timeout(WAIT_NATIVE, async {
            loop {
                if action.is_finished() { break false; }
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT pin_count FROM blobs WHERE hash=ANY($1) ORDER BY hash FOR UPDATE%')")
                    .bind(pid).fetch_one(&s.rig.world.db.pool).await.unwrap();
                if blocked { break true; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap_or(false);
        if reached && expire {
            s.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.rollback().await.unwrap();
        let completed = tokio::time::timeout(WAIT_NATIVE, &mut action).await;
        if completed.is_err() {
            action.abort();
            let _ = action.await;
        }
        let reply = completed
            .expect("the actual workspace unpin request must drain")
            .unwrap();
        assert!(
            reached,
            "the real workspace keeper must reach its actual blob-row preparation"
        );
        let after: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(manifest.as_bytes().to_vec())
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
        let state: String = sqlx::query_scalar("SELECT state FROM agent_shares WHERE id=$1")
            .bind(id)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
        if expire {
            assert!(matches!(reply.status.as_u16(), 401 | 403));
            assert_eq!(after, pins);
            assert_eq!(state, "pending");
        } else {
            assert_eq!(reply.status, 204, "{}", String::from_utf8_lossy(&reply.raw));
            assert_eq!(after, pins - 1);
            assert_eq!(state, "revoked");
        }
    }
}

#[tokio::test]
async fn share_copy_removal_keeps_ordinary_cleanup_after_source_purge_without_guessing_a_scoped_identity()
 {
    use orch_api_contract::share::ShareAgentDelete;
    let s = Scene::new().await;
    let target = s.thread().await.base.id;
    let source = document(&s, "Purged copy source").await;
    let parcels = std::sync::Arc::new(RemovalParcels::default());
    s.rig.engine.attach_parcels(parcels.clone());
    let id = copy_by(&s, &s.ada, source, target).await;
    let scoped = key(
        &s,
        s.ada.id(),
        vec![operation_rule(
            "document",
            source,
            "ShareAgentDelete",
            &["manage"],
        )],
    )
    .await;
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(source)
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        s.rig
            .world
            .app
            .call(ShareAgentDelete, &[&id])
            .token(&scoped)
            .send_bytes()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    assert!(parcels.withdrawn.lock().unwrap().is_empty());
    let state: String = sqlx::query_scalar("SELECT state FROM agent_shares WHERE id=$1")
        .bind(id)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap();
    assert_eq!(state, "pending");
    let ordinary = s
        .rig
        .world
        .app
        .call(ShareAgentDelete, &[&id])
        .token(&s.ada.token)
        .send_bytes()
        .await;
    assert_eq!(
        ordinary.status,
        204,
        "{}",
        String::from_utf8_lossy(&ordinary.raw)
    );
    assert_eq!(*parcels.withdrawn.lock().unwrap(), vec![id]);
}
