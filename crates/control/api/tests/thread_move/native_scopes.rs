//! Review and confirmation keep separate real operations, scopes and current original authority.

use super::*;
use orch_api_core::testing::RawReply;
use orch_types::Json;
use thread_history_scope::{Proof, key, narrow, rule};

const PREVIEW: &str = "ThreadMovePreview";
const MOVE: &str = "ThreadMove";

async fn scene() -> (Scene, Id, Id, Id) {
    let s = Scene::new().await;
    s.rig.engine.stop();
    let main = s.thread().await.base.id;
    let side = retained_side(&s, main).await;
    let to = destination(&s).await;
    (s, main, side, to)
}
fn controls(op: &str, main: Id, side: Id, to: Id) -> Vec<Json> {
    vec![
        rule(op, "thread", main, "self", "manage"),
        rule(op, "thread", side, "self", "manage"),
        rule(op, "topic", to, "self", "write"),
    ]
}
async fn reads(s: &Scene, op: &str) -> Proof<Vec<Json>> {
    let mut rules = vec![
        rule(op, "org", s.tree.org.base.id, "subtree", "read"),
        rule(op, "org", s.ada.person.personal_org, "subtree", "read"),
        rule(op, "environment", s.on.environment, "self", "read"),
        rule(op, "agent", s.on.agent, "self", "read"),
        rule(op, "provider", s.on.provider, "self", "read"),
    ];
    let host = orch_db::queries::resources::hosting::machine_hosts(
        &mut *s.rig.world.conn().await,
        &[s.rig.machine_id],
    )
    .await?
    .into_iter()
    .find(|(machine, _, _)| *machine == s.rig.machine_id)
    .map(|(_, host, _)| host);
    rules.push(rule(
        op,
        if host.is_some() { "host" } else { "machine" },
        host.unwrap_or(s.rig.machine_id),
        "self",
        "read",
    ));
    Ok(rules)
}
async fn review(s: &Scene, main: Id, to: Id, secret: &str) -> RawReply {
    let request = s
        .rig
        .world
        .app
        .call(ThreadMovePreview, &[&main])
        .token(secret)
        .body(&ThreadMovePreviewBody { parent_id: to });
    request.send_bytes().await
}
async fn apply(s: &Scene, main: Id, body: &ThreadMoveBody, secret: &str) -> RawReply {
    let request = s
        .rig
        .world
        .app
        .call(ThreadMove, &[&main])
        .token(secret)
        .body(body);
    request.send_bytes().await
}
fn body(reply: &RawReply) -> Proof<Json> {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    Ok(serde_json::from_slice(&reply.raw)?)
}
fn confirm(value: &Json) -> Proof<ThreadMoveBody> {
    if value.get("projection").is_some() {
        Ok(ThreadMoveBody {
            parent_id: serde_json::from_value(value["parent_id"].clone())?,
            preflight_id: serde_json::from_value(value["preflight_id"].clone())?,
            expected_revision: serde_json::from_value(value["expected_revision"].clone())?,
        })
    } else {
        Ok(confirmation(&serde_json::from_value(value.clone())?))
    }
}
async fn topic(s: &Scene, id: Id) -> Proof<Id> {
    Ok(
        sqlx::query_scalar("SELECT topic_id FROM threads WHERE node_id=$1")
            .bind(id)
            .fetch_one(s.rig.db.pool())
            .await?,
    )
}
async fn marker_count(s: &Scene, main: Id) -> usize {
    sync(s, main)
        .await
        .items
        .iter()
        .filter(|item| {
            matches!(
                &item.payload,
                ItemPayload::Marker(MarkerPayload::ThreadMoved(_))
            )
        })
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn review_requires_its_exact_main_sides_and_business_thread_destination_then_write_only_confirmation_keeps_native_marker_and_paths()
-> Proof {
    let (s, main, side, to) = scene().await;
    let b = key(&s.rig.world, s.ada.id(), controls(MOVE, main, side, to)).await?;
    for rules in [
        vec![
            rule(PREVIEW, "thread", main, "self", "manage"),
            rule(PREVIEW, "topic", to, "self", "write"),
        ],
        vec![
            rule(PREVIEW, "thread", side, "self", "manage"),
            rule(PREVIEW, "topic", to, "self", "write"),
        ],
        vec![
            rule(PREVIEW, "thread", main, "self", "manage"),
            rule(PREVIEW, "thread", side, "self", "manage"),
            rule(PREVIEW, "topic", to, "self", "read"),
        ],
        controls(MOVE, main, side, to),
    ] {
        let bad = key(&s.rig.world, s.ada.id(), rules).await?;
        assert!(!review(&s, main, to, &bad.secret).await.status.is_success());
    }
    let outsider = key(&s.rig.world, s.cai.id(), controls(PREVIEW, main, side, to)).await?;
    assert!(
        !review(&s, main, to, &outsider.secret)
            .await
            .status
            .is_success()
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM preflights WHERE kind='thread_move' AND node_id=$1",
    )
    .bind(main)
    .fetch_one(s.rig.db.pool())
    .await?;
    assert_eq!(count, 0);
    assert_eq!(topic(&s, main).await?, s.tree.topic.base.id);
    let a = key(&s.rig.world, s.ada.id(), controls(PREVIEW, main, side, to)).await?;
    let planned = body(&review(&s, main, to, &a.secret).await)?;
    assert_eq!(planned["projection"], "receipt");
    for field in [
        "settings",
        "access",
        "roles",
        "unchanged",
        "node",
        "origin",
        "sources",
    ] {
        assert!(planned.get(field).is_none());
    }
    let confirmation = confirm(&planned)?;
    let stored: Json = sqlx::query_scalar("SELECT plan FROM preflights WHERE id=$1")
        .bind(confirmation.preflight_id)
        .fetch_one(s.rig.db.pool())
        .await?;
    assert_eq!(stored["origin"]["operation"]["operation"], PREVIEW);
    assert!(serde_json::to_string(&stored["origin"])?.contains(&a.id.to_string()));
    assert_eq!(stored["sources"]["lanes"].as_array().map(Vec::len), Some(2));
    let moved = body(&apply(&s, main, &confirmation, &b.secret).await)?;
    assert_eq!(moved["projection"], "receipt");
    assert_eq!(moved["thread_id"], json!(main));
    assert_eq!(moved["parent_id"], json!(to));
    for field in ["base", "binding", "environment", "directory"] {
        assert!(moved.get(field).is_none());
    }
    assert_eq!(topic(&s, main).await?, to);
    assert_eq!(topic(&s, side).await?, to);
    assert_eq!(marker_count(&s, main).await, 1);
    assert!(
        !apply(&s, main, &confirmation, &b.secret)
            .await
            .status
            .is_success()
    );
    assert_eq!(marker_count(&s, main).await, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn confirmation_has_its_own_operation_and_scope_and_cannot_borrow_the_reviews_grants() -> Proof
{
    let (s, main, side, to) = scene().await;
    let a = key(&s.rig.world, s.ada.id(), controls(PREVIEW, main, side, to)).await?;
    let mut bad = Vec::new();
    for grants in [
        controls(PREVIEW, main, side, to),
        vec![
            rule(MOVE, "thread", main, "self", "manage"),
            rule(MOVE, "topic", to, "self", "write"),
        ],
        vec![
            rule(MOVE, "thread", main, "self", "manage"),
            rule(MOVE, "thread", side, "self", "manage"),
            rule(MOVE, "topic", to, "self", "read"),
        ],
    ] {
        bad.push(key(&s.rig.world, s.ada.id(), grants).await?);
    }
    let good = key(&s.rig.world, s.ada.id(), controls(MOVE, main, side, to)).await?;
    let plan = confirm(&body(&review(&s, main, to, &a.secret).await)?)?;
    for bad in bad {
        assert!(
            !apply(&s, main, &plan, &bad.secret)
                .await
                .status
                .is_success()
        );
        assert_eq!(topic(&s, main).await?, s.tree.topic.base.id);
        assert_eq!(marker_count(&s, main).await, 0);
    }
    body(&apply(&s, main, &plan, &good.secret).await)?;
    assert_eq!(marker_count(&s, main).await, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_fully_scoped_confirming_key_cannot_replace_a_closed_original_preview_key() -> Proof
{
    for mode in ["alive", "expired", "revoked", "namespace"] {
        let (s, main, side, to) = scene().await;
        let a = key(&s.rig.world, s.ada.id(), controls(PREVIEW, main, side, to)).await?;
        let b = key(&s.rig.world, s.ada.id(), controls(MOVE, main, side, to)).await?;
        if mode == "expired" {
            sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                .bind(a.id)
                .bind(s.rig.world.now() + chrono::Duration::seconds(5))
                .execute(s.rig.db.pool())
                .await?;
        }
        let plan = confirm(&body(&review(&s, main, to, &a.secret).await)?)?;
        match mode {
            "expired" => s.rig.world.advance(chrono::Duration::seconds(6)),
            "revoked" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(a.id)
                    .bind(s.rig.world.now())
                    .execute(s.rig.db.pool())
                    .await?;
            }
            "namespace" => narrow(&s.rig.world, &a, controls(MOVE, main, side, to)).await?,
            _ => {}
        }
        let reply = apply(&s, main, &plan, &b.secret).await;
        if mode == "alive" {
            body(&reply)?;
            assert_eq!(topic(&s, main).await?, to);
        } else {
            assert_eq!(
                reply.status.as_u16(),
                403,
                "{mode}: {}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(topic(&s, main).await?, s.tree.topic.base.id);
            assert_eq!(topic(&s, side).await?, s.tree.topic.base.id);
            assert_eq!(marker_count(&s, main).await, 0);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn current_preview_and_confirmed_original_lifetimes_are_rechecked_after_the_actual_tree_lock_wait()
-> Proof {
    for mode in [
        "preview_alive",
        "preview_expired",
        "move_alive",
        "original_expired",
        "plan_expired",
    ] {
        let (s, main, side, to) = scene().await;
        let a = key(&s.rig.world, s.ada.id(), controls(PREVIEW, main, side, to)).await?;
        let b = key(&s.rig.world, s.ada.id(), controls(MOVE, main, side, to)).await?;
        if matches!(mode, "preview_expired" | "original_expired") {
            sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                .bind(a.id)
                .bind(s.rig.world.now() + chrono::Duration::seconds(5))
                .execute(s.rig.db.pool())
                .await?;
        }
        let is_preview = mode.starts_with("preview");
        let plan = if is_preview {
            None
        } else {
            Some(confirm(&body(&review(&s, main, to, &a.secret).await)?)?)
        };
        let mut blocker = s.rig.db.begin().await?;
        orch_db::queries::hierarchy::lock_tree_for_moves(&mut blocker, s.tree.org.base.id).await?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = s.rig.world.app.clone();
        let secret = if is_preview {
            a.secret.clone()
        } else {
            b.secret.clone()
        };
        let pending = tokio::spawn(async move {
            if let Some(plan) = plan {
                let request = app.call(ThreadMove, &[&main]).token(&secret).body(&plan);
                request.send_bytes().await
            } else {
                let request = app
                    .call(ThreadMovePreview, &[&main])
                    .token(&secret)
                    .body(&ThreadMovePreviewBody { parent_id: to });
                request.send_bytes().await
            }
        });
        tokio::time::timeout(rig::PATIENCE, async {
            loop {
                assert!(!pending.is_finished(), "the actual native tree lock must be reached");
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%nodes.move:%')")
                    .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await?;
        if matches!(mode, "preview_expired" | "original_expired") {
            s.rig.world.advance(chrono::Duration::seconds(6));
        }
        if mode == "plan_expired" {
            s.rig.world.advance(chrono::Duration::minutes(16));
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(rig::PATIENCE, pending).await??;
        if matches!(mode, "preview_alive" | "move_alive") {
            body(&reply)?;
        } else {
            assert!(!reply.status.is_success());
            assert_eq!(marker_count(&s, main).await, 0);
        }
        assert_eq!(
            topic(&s, main).await?,
            if mode == "move_alive" {
                to
            } else {
                s.tree.topic.base.id
            }
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn both_native_full_projections_require_their_own_read_instead_of_another_operations_read()
-> Proof {
    for own in [false, true] {
        let (s, main, side, to) = scene().await;
        let mut a_rules = controls(PREVIEW, main, side, to);
        let mut b_rules = controls(MOVE, main, side, to);
        a_rules.extend(reads(&s, if own { PREVIEW } else { "NodeGet.read" }).await?);
        b_rules.extend(reads(&s, if own { MOVE } else { PREVIEW }).await?);
        let a = key(&s.rig.world, s.ada.id(), a_rules).await?;
        let b = key(&s.rig.world, s.ada.id(), b_rules).await?;
        let preview = body(&review(&s, main, to, &a.secret).await)?;
        assert_eq!(preview.get("projection").is_none(), own);
        if own {
            assert!(preview.get("unchanged").is_some());
        }
        let moved = body(&apply(&s, main, &confirm(&preview)?, &b.secret).await)?;
        assert_eq!(moved.get("projection").is_none(), own);
        if own {
            let moved: Thread = serde_json::from_value(moved)?;
            assert_eq!(moved.topic_id, to);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn completed_review_and_move_readback_do_not_re_admit_manage_or_lend_another_current_source_after_publication_wait()
-> Proof {
    for is_preview in [true, false] {
        for mode in ["unchanged", "read", "manage", "association"] {
            let (s, main, side, to) = scene().await;
            let other = binding_support_environment(&s).await?;
            let mut a_rules = controls(PREVIEW, main, side, to);
            let mut b_rules = controls(MOVE, main, side, to);
            let mut a_read = reads(&s, PREVIEW).await?;
            let mut b_read = reads(&s, MOVE).await?;
            a_read.push(rule(PREVIEW, "environment", other, "self", "read"));
            b_read.push(rule(MOVE, "environment", other, "self", "read"));
            a_rules.extend(a_read.clone());
            b_rules.extend(b_read.clone());
            let a = key(&s.rig.world, s.ada.id(), a_rules).await?;
            let b = key(&s.rig.world, s.ada.id(), b_rules).await?;
            let plan = if is_preview {
                None
            } else {
                Some(confirm(&body(&review(&s, main, to, &a.secret).await)?)?)
            };
            let mut blocker = s.rig.db.begin().await?;
            sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
                .execute(&mut *blocker)
                .await?;
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await?;
            let app = s.rig.world.app.clone();
            let secret = if is_preview {
                a.secret.clone()
            } else {
                b.secret.clone()
            };
            let pending = tokio::spawn(async move {
                if let Some(plan) = plan {
                    let request = app.call(ThreadMove, &[&main]).token(&secret).body(&plan);
                    request.send_bytes().await
                } else {
                    let request = app
                        .call(ThreadMovePreview, &[&main])
                        .token(&secret)
                        .body(&ThreadMovePreviewBody { parent_id: to });
                    request.send_bytes().await
                }
            });
            tokio::time::timeout(rig::PATIENCE, async {
                loop {
                    assert!(!pending.is_finished(), "the actual final activity wait must be reached");
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%INSERT INTO api_token_activity%')")
                        .bind(pid).fetch_one(s.rig.db.pool()).await.unwrap();
                    if waiting { break; }
                    tokio::task::yield_now().await;
                }
            }).await?;
            let seq: i64 = sqlx::query_scalar("SELECT next_seq FROM threads WHERE node_id=$1")
                .bind(main)
                .fetch_one(s.rig.db.pool())
                .await?;
            let selected = if is_preview { &a } else { &b };
            match mode {
                "read" => {
                    narrow(
                        &s.rig.world,
                        selected,
                        controls(if is_preview { PREVIEW } else { MOVE }, main, side, to),
                    )
                    .await?
                }
                "manage" => {
                    narrow(
                        &s.rig.world,
                        selected,
                        if is_preview { a_read } else { b_read },
                    )
                    .await?
                }
                "association" => {
                    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                        .bind(main)
                        .bind(other)
                        .execute(s.rig.db.pool())
                        .await?;
                }
                _ => {}
            }
            blocker.commit().await?;
            let reply = body(&tokio::time::timeout(rig::PATIENCE, pending).await??)?;
            assert_eq!(
                reply.get("projection").is_none(),
                matches!(mode, "unchanged" | "manage")
            );
            if matches!(mode, "read" | "association") {
                assert_eq!(reply["projection"], "receipt");
                assert!(reply.get("unchanged").is_none());
            }
            let after: i64 = sqlx::query_scalar("SELECT next_seq FROM threads WHERE node_id=$1")
                .bind(main)
                .fetch_one(s.rig.db.pool())
                .await?;
            assert_eq!(
                after, seq,
                "publication must not repeat a trial or native write"
            );
            assert_eq!(marker_count(&s, main).await, usize::from(!is_preview));
        }
    }
    Ok(())
}

async fn binding_support_environment(s: &Scene) -> Proof<Id> {
    // The actual second readable environment, reusing the native fixture's existing row shape.
    let id = new_id();
    sqlx::query("INSERT INTO environments (id,name,owner_id,org_id,placement,home_node_id,workspace_root,machine_id,generation,status)
        SELECT $2,'Another readable move environment',owner_id,org_id,placement,home_node_id,workspace_root,machine_id,generation,status FROM environments WHERE id=$1")
        .bind(s.on.environment).bind(id).execute(s.rig.db.pool()).await?;
    Ok(id)
}
