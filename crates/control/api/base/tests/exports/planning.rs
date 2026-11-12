//! Actual native export candidates are bounded before plans expose counts, sizes or labels.
use super::*;
use orch_api_contract::exports::{ExportPlan, NodeExportPreflight};
const OP: &str = "NodeExportPreflight";
fn root(o: &Office) -> Value {
    rule(OP, "org", o.org(), "self", &["org"], &["read"])
}
fn all(o: &Office) -> Value {
    rule(
        OP,
        "org",
        o.org(),
        "subtree",
        &[
            "org",
            "project",
            "topic",
            "thread",
            "document",
            "document_container",
            "chat",
        ],
        &["read"],
    )
}
async fn take(o: &Office, secret: &str, request: &ExportRequest) -> RawReply {
    let call =
        o.s.rig
            .world
            .app
            .call(NodeExportPreflight, &[&o.org()])
            .token(secret)
            .body(request);
    call.send_bytes().await
}
fn plan(reply: RawReply) -> ExportPlan {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn preflight_reads_its_actual_root_and_content_under_its_own_operation_and_preserves_native_plans()
 {
    let o = Office::new().await;
    for rules in [
        vec![rule(OP, "org", o.org(), "self", &["org"], &["manage"])],
        vec![rule(
            "NodeExportCreate",
            "org",
            o.org(),
            "subtree",
            &["org", "project", "topic", "thread", "document", "chat"],
            &["read"],
        )],
        vec![rule(OP, "project", o.org(), "self", &["org"], &["read"])],
    ] {
        let k = key(&o, &o.s.ada, rules).await;
        assert_eq!(
            take(&o, &k.secret, &everything()).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let broad = key(&o, &o.s.ada, vec![all(&o)]).await;
    for package in [false, true] {
        let mut request = everything();
        request.package = Some(package);
        let ordinary = o.plan(&o.s.ada, o.org(), &request).await.ok();
        let scoped = plan(take(&o, &broad.secret, &request).await);
        assert_eq!(
            scoped, ordinary,
            "the native planner retains request defaults, byte estimates, layout and blockers"
        );
    }
}

#[tokio::test]
async fn preflight_scopes_real_candidates_before_totals_and_keeps_a_genuine_empty_plan() {
    let o = Office::new().await;
    let doc = key(
        &o,
        &o.s.ada,
        vec![
            root(&o),
            rule(
                OP,
                "document",
                o.document.base.id,
                "self",
                &["document"],
                &["read"],
            ),
        ],
    )
    .await;
    let scoped = plan(take(&o, &doc.secret, &everything()).await);
    assert_eq!(
        (
            scoped.counts.conversations,
            scoped.counts.chats,
            scoped.counts.documents
        ),
        (0, 0, 1)
    );
    let mut chosen = everything();
    chosen.what = ExportWhat::Chosen(ExportChosen {
        documents: Some(vec![o.document.base.id]),
        ..Default::default()
    });
    let ordinary = o.plan(&o.s.ada, o.org(), &chosen).await.ok();
    assert_eq!(
        scoped, ordinary,
        "excluded conversations/chats contribute neither counts nor bytes"
    );
    let empty = key(&o, &o.s.ada, vec![root(&o)]).await;
    let empty = plan(take(&o, &empty.secret, &everything()).await);
    assert_eq!(
        (
            empty.counts.conversations,
            empty.counts.chats,
            empty.counts.documents,
            empty.bytes
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(empty.blockers[0].code, "export.nothing");
    assert!(empty.left_out.is_empty());
    let chat = key(
        &o,
        &o.s.ada,
        vec![
            root(&o),
            rule(OP, "chat", o.topic_chat, "self", &["chat"], &["read"]),
        ],
    )
    .await;
    let chat = plan(take(&o, &chat.secret, &everything()).await);
    assert_eq!(
        (
            chat.counts.conversations,
            chat.counts.chats,
            chat.counts.documents
        ),
        (0, 1, 0)
    );
}

#[tokio::test]
async fn preflight_counts_readable_but_not_exportable_entries_without_borrowing_another_read_endpoint()
 {
    let o = Office::new().await;
    let mut request = everything();
    request.what = ExportWhat::Chosen(ExportChosen {
        documents: Some(vec![o.document.base.id]),
        ..Default::default()
    });
    use crate::support::FullPermissionPageReply;
    use orch_api_contract::access::{
        AccessChoice, HolderInput, PageChange, PageChangeName, PageChangeRow, PermissionPageUpdate,
    };
    o.s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &o.document.base.id])
        .token(&o.s.ada.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "doc.export".into(),
                profile: None,
                inherit: Some(false),
                locked: None,
                add: None,
                update: None,
                remove: None,
            }]),
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: o.s.cai.id() }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .full_permission_page()
        .ok();
    let ordinary = o.plan(&o.s.cai, o.org(), &request).await.ok();
    assert_eq!(ordinary.counts.documents, 0);
    assert_eq!(
        ordinary.left_out.iter().map(|row| row.count).sum::<i64>(),
        1
    );
    let base = vec![
        root(&o),
        rule(
            OP,
            "document",
            o.document.base.id,
            "self",
            &["document"],
            &["read"],
        ),
    ];
    let mut borrowed = base.clone();
    borrowed.push(rule(
        "NodeGet",
        "topic",
        o.topic(),
        "self",
        &["topic"],
        &["read"],
    ));
    let k = key(&o, &o.s.cai, borrowed).await;
    let hidden = plan(take(&o, &k.secret, &request).await);
    assert_eq!(hidden.counts.documents, 0);
    assert_eq!(hidden.left_out[0].count, 1);
    let name = hidden.left_out[0].place.as_ref().unwrap();
    assert_eq!(name.id, o.topic());
    assert!(name.name.is_none() && name.redacted);
    let mut own = base;
    own.push(rule(OP, "topic", o.topic(), "self", &["topic"], &["read"]));
    let k = key(&o, &o.s.cai, own).await;
    assert_eq!(plan(take(&o, &k.secret, &request).await), ordinary);
}

#[tokio::test]
async fn preflight_subtree_content_follows_the_documents_real_current_home() {
    let o = Office::new().await;
    let k = key(
        &o,
        &o.s.ada,
        vec![
            root(&o),
            rule(
                OP,
                "topic",
                o.topic(),
                "subtree",
                &["topic", "thread", "document", "chat"],
                &["read"],
            ),
        ],
    )
    .await;
    assert_eq!(
        plan(take(&o, &k.secret, &everything()).await)
            .counts
            .documents,
        1
    );
    let other = places::make_topic(
        &o.s.rig.world,
        &o.s.ada,
        o.s.tree.project.base.id,
        "Outside granted export",
    )
    .await;
    sqlx::query("UPDATE documents SET node_id=$2 WHERE id=$1")
        .bind(o.document.base.id)
        .bind(other.base.id)
        .execute(&o.s.rig.world.db.pool)
        .await
        .unwrap();
    let after = plan(take(&o, &k.secret, &everything()).await);
    assert_eq!(after.counts.documents, 0);
    assert_eq!((after.counts.conversations, after.counts.chats), (1, 2));
}

#[tokio::test]
async fn preflight_rechecks_after_real_activity_wait_without_recounting_a_replacement_window() {
    for mode in ["unchanged", "append", "scope", "expired"] {
        let o = Office::new().await;
        let world = &o.s.rig.world;
        let k = key(&o, &o.s.ada, vec![all(&o)]).await;
        let original = o.plan(&o.s.ada, o.org(), &everything()).await.ok();
        let mut locked = world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *locked)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *locked)
            .await
            .unwrap();
        let app = world.app.clone();
        let secret = k.secret;
        let root = o.org();
        let pending = tokio::spawn(async move {
            let call = app
                .call(NodeExportPreflight, &[&root])
                .token(&secret)
                .body(&everything());
            call.send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE,async{loop{assert!(!pending.is_finished(),"the native plan must reach its actual publication wait");let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(blocker).fetch_one(&world.db.pool).await.unwrap();if waiting{break;}tokio::task::yield_now().await;}}).await.expect("actual final activity INSERT wait");
        match mode {
            "append" => {
                Office::write_document(
                    &o.s,
                    &o.s.ada,
                    "Later export entry",
                    "A new source after the plan snapshot",
                )
                .await;
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(k.id)
                    .bind(json!({"rules":[]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        locked.rollback().await.unwrap();
        let reply = pending.await.unwrap();
        if mode == "scope" || mode == "expired" {
            assert!(!reply.status.is_success());
            assert!(serde_json::from_slice::<ExportPlan>(&reply.raw).is_err());
        } else {
            assert_eq!(
                plan(reply),
                original,
                "the producing snapshot is retained; no newer source replaces its old plan"
            );
        }
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='export'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            jobs, 0,
            "a plan neither queues an export nor replays a writer"
        );
    }
}
