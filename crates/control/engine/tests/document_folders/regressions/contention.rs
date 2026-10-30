//! Deliberately held metadata transactions observe real PostgreSQL structural lock waiters.

use super::*;

async fn waits(world: &World, pid: i32) -> TestResult {
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let waiting:bool=sqlx::query_scalar("SELECT COALESCE((SELECT wait_event_type='Lock' FROM pg_stat_activity WHERE pid=$1),false)")
            .bind(pid).fetch_one(world.pool.pool()).await?;
        if waiting {
            return Ok(());
        }
        if tokio::time::Instant::now() >= until {
            return Err(
                "the competing metadata statement never waited for its real structural lock".into(),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn recursive_delete_and_cross_node_move_exclude_late_child_creation_until_the_actual_parent_is_rechecked()
-> TestResult {
    for deleting in [true, false] {
        let area = Area::new(true).await?;
        let parent = area.folder("research/").await?;
        let nested = area.folder("research/notes/").await?;
        let document = area
            .document(
                "research/notes/Plan.md",
                "Keep these identities and exact blocks.",
            )
            .await?;
        let blocks = area.world.blocks_of(document).await;
        let destination = area.world.topic("Another docs area").await;
        share(
            &area.world,
            "node",
            destination,
            "live",
            "doc.edit",
            "any",
            area.agent,
        )
        .await;
        let actor = domain::Actor::Agent { thread: area.agent };
        let options = domain::Options::for_engine(&area.world.engine());
        let now = Time::new(area.world.clock.now());
        let mut holding = area.world.pool.begin().await?;
        let command = if deleting {
            domain::Command::Delete
        } else {
            domain::Command::Move
        };
        let request = request(
            command,
            vec![domain::Item::Container(parent)],
            (!deleting).then_some(domain::Directory::NodeDocs(destination)),
            "held-complete-tree",
        );
        let plan = if deleting {
            Some(domain::plan_delete_in(&mut holding, actor, &request, now, options).await?)
        } else {
            None
        };
        let receipt = domain::execute_in(
            &mut holding,
            actor,
            &request,
            plan.as_ref().map(|plan| &plan.stamp),
            now,
            options,
        )
        .await?;
        assert_eq!(receipt.entries.len(), 3);
        let held_scopes: Vec<(Id, i64)> =
            sqlx::query_as("SELECT node_id,seq FROM document_tree_scopes ORDER BY node_id")
                .fetch_all(&mut *holding)
                .await?;
        let held_nodes: Vec<(Id, i64)> =
            sqlx::query_as("SELECT id,tree_seq FROM nodes WHERE id=ANY($1) ORDER BY id")
                .bind(vec![area.topic, destination])
                .fetch_all(&mut *holding)
                .await?;
        let late = new_id();
        let database = area.world.pool.clone();
        let thread = area.agent;
        let (announce, waiting) = tokio::sync::oneshot::channel();
        let (resolved, observe_refusal) = tokio::sync::oneshot::channel();
        let (release, commit_refusal) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn(async move {
            let mut tx = database.begin().await?;
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await?;
            announce
                .send(pid)
                .map_err(|_| "lock observer disappeared")?;
            let result = domain::create_text_in(
                &mut tx,
                domain::Actor::Agent { thread },
                domain::Directory::Container(nested),
                late,
                "Late child",
                None,
                now,
            )
            .await;
            let outcome = match result {
                Err(domain::Error::NotFound) => "not_found",
                Err(domain::Error::Stale) => "stale",
                Ok(_) => "created",
                Err(error) => return Err(error.into()),
            };
            resolved
                .send(outcome)
                .map_err(|_| "the parent-recheck observer disappeared")?;
            commit_refusal
                .await
                .map_err(|_| "the refused writer was not released to commit")?;
            tx.commit().await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(outcome)
        });
        let pid = waiting.await?;
        waits(&area.world, pid).await?;
        assert!(
            !writer.is_finished(),
            "the late child waits while the complete tree is locked"
        );
        // This held transaction is intentional native concurrency-test staging only. Production
        // domain code waits for no machine, timer, file store or second pooled connection.
        holding.commit().await?;
        let refused = observe_refusal.await?;
        assert_eq!(refused, if deleting { "not_found" } else { "stale" });
        // Tree numbers are assigned by the winner's deferred commit work, not by execute_in.
        // Keep the refused caller uncommitted so this baseline contains only the winner's stamps.
        let committed_nodes: Vec<(Id, i64)> =
            sqlx::query_as("SELECT id,tree_seq FROM nodes WHERE id=ANY($1) ORDER BY id")
                .bind(vec![area.topic, destination])
                .fetch_all(area.world.pool.pool())
                .await?;
        assert_eq!(committed_nodes.len(), held_nodes.len());
        for ((before_id, before), (after_id, after)) in held_nodes.iter().zip(&committed_nodes) {
            assert_eq!(after_id, before_id);
            if deleting && *after_id == destination {
                assert_eq!(
                    after, before,
                    "deletion does not stamp the untouched destination"
                );
            } else {
                assert!(
                    after > before,
                    "the winning act stamps every changed area at commit"
                );
            }
        }
        let committed = state(&area.world, &[area.topic, destination]).await?;
        release
            .send(())
            .map_err(|_| "the refused writer disappeared before commit")?;
        assert_eq!(writer.await??, refused);
        assert!(
            documents::get(&mut *area.world.pool.acquire().await?, late)
                .await?
                .is_none()
        );
        let mut conn = area.world.pool.acquire().await?;
        let scopes: Vec<(Id, i64)> =
            sqlx::query_as("SELECT node_id,seq FROM document_tree_scopes ORDER BY node_id")
                .fetch_all(&mut *conn)
                .await?;
        let nodes: Vec<(Id, i64)> =
            sqlx::query_as("SELECT id,tree_seq FROM nodes WHERE id=ANY($1) ORDER BY id")
                .bind(vec![area.topic, destination])
                .fetch_all(&mut *conn)
                .await?;
        assert_eq!(
            scopes, held_scopes,
            "the stale or deleted-parent waiter changes no structural sequence"
        );
        assert_eq!(
            nodes, committed_nodes,
            "only the complete winning act stamps the old and new areas"
        );
        assert_eq!(
            state(&area.world, &[area.topic, destination]).await?,
            committed,
            "committing the refused waiter changes no material, grant, audit, receipt, pin or sequence"
        );
        for item in [
            domain::Item::Container(parent),
            domain::Item::Container(nested),
            domain::Item::Document(document),
        ] {
            let kind = if matches!(item, domain::Item::Container(_)) {
                "container"
            } else {
                "document"
            };
            let row = documents::containers::entry(&mut conn, kind, item.id())
                .await?
                .ok_or("original member missing")?;
            assert_eq!(row.node_id, if deleting { area.topic } else { destination });
            assert_eq!(row.deleted_at.is_some(), deleting);
            assert_eq!(
                row.deleted_in,
                if deleting {
                    Some(receipt.operation)
                } else {
                    None
                }
            );
        }
        let docs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM documents WHERE node_id=$1 AND deleted_at IS NULL",
        )
        .bind(area.topic)
        .fetch_one(&mut *conn)
        .await?;
        assert_eq!(docs, 0);
        let there: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM documents WHERE node_id=$1 AND deleted_at IS NULL",
        )
        .bind(destination)
        .fetch_one(&mut *conn)
        .await?;
        assert_eq!(there, if deleting { 0 } else { 1 });
        let source_seq: i64 =
            sqlx::query_scalar("SELECT seq FROM document_tree_scopes WHERE node_id=$1")
                .bind(area.topic)
                .fetch_one(&mut *conn)
                .await?;
        drop(conn);
        assert_eq!(area.world.blocks_of(document).await, blocks);
        let stable = state(&area.world, &[area.topic, destination]).await?;
        let mut tx = area.world.pool.begin().await?;
        // A second attempt resolves the actual parent anew. It remains forbidden after deletion;
        // after movement it succeeds only at the new anchor, never under the stale old one.
        let again = domain::create_text_in(
            &mut tx,
            actor,
            domain::Directory::Container(nested),
            late,
            "Late child",
            None,
            now,
        )
        .await;
        if deleting {
            assert!(matches!(again, Err(domain::Error::NotFound)));
            tx.commit().await?;
            assert_eq!(
                state(&area.world, &[area.topic, destination]).await?,
                stable
            );
        } else {
            let made = again?;
            assert_eq!(made.document.node_id, destination);
            assert!(made.created);
            tx.commit().await?;
            let mut conn = area.world.pool.acquire().await?;
            let after: i64 =
                sqlx::query_scalar("SELECT seq FROM document_tree_scopes WHERE node_id=$1")
                    .bind(area.topic)
                    .fetch_one(&mut *conn)
                    .await?;
            assert_eq!(
                after, source_seq,
                "the re-resolved insert does not stamp the old docs area"
            );
        }
    }
    Ok(())
}

#[test]
fn document_name_collision_runs_with_default_worker_stacks_even_if_the_suite_stack_is_larger() {
    crate::common::on_default_stack(
        "regressions::contention::a_waiting_cross_kind_ascii_folded_collision_commits_exactly_one_entry_in_either_order",
    );
}

#[tokio::test]
async fn a_waiting_cross_kind_ascii_folded_collision_commits_exactly_one_entry_in_either_order()
-> TestResult {
    for folder_first in [true, false] {
        let area = Area::new(false).await?;
        let parent = domain::Directory::NodeDocs(area.topic);
        let thread = area.agent;
        let now = Time::new(area.world.clock.now());
        let options = domain::Options::for_engine(&area.world.engine());
        let mut holding = area.world.pool.begin().await?;
        let text = new_id();
        let mut folder = request(
            domain::Command::CreateFolder,
            Vec::new(),
            Some(parent),
            "one-casefolded-folder",
        );
        folder.name = Some("PLAN.md".into());
        if folder_first {
            domain::execute_in(
                &mut holding,
                domain::Actor::Agent { thread },
                &folder,
                None,
                now,
                options,
            )
            .await?;
        } else {
            domain::create_text_in(
                &mut holding,
                domain::Actor::Agent { thread },
                parent,
                text,
                "Plan",
                Some("Plan.md"),
                now,
            )
            .await?;
        }
        let database = area.world.pool.clone();
        let (announce, waiting) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn(async move {
            let mut tx = database.begin().await?;
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await?;
            announce
                .send(pid)
                .map_err(|_| "lock observer disappeared")?;
            let conflict = if folder_first {
                matches!(
                    domain::create_text_in(
                        &mut tx,
                        domain::Actor::Agent { thread },
                        parent,
                        text,
                        "Plan",
                        Some("Plan.md"),
                        now
                    )
                    .await,
                    Err(domain::Error::Conflict)
                )
            } else {
                matches!(
                    domain::execute_in(
                        &mut tx,
                        domain::Actor::Agent { thread },
                        &folder,
                        None,
                        now,
                        options
                    )
                    .await,
                    Err(domain::Error::Conflict)
                )
            };
            // Both public producers own a savepoint, so even a refused SQL INSERT leaves a
            // committable caller transaction and no namespace/scoping side effect.
            tx.commit().await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(conflict)
        });
        waits(&area.world, waiting.await?).await?;
        assert!(!writer.is_finished());
        holding.commit().await?;
        let stable = state(&area.world, &[area.topic]).await?;
        assert!(
            writer.await??,
            "the waiter must see the committed cross-kind collision after the scope lock"
        );
        assert_eq!(state(&area.world, &[area.topic]).await?, stable);
        let rows = documents::containers::entries_in(
            &mut *area.world.pool.acquire().await?,
            area.topic,
            None,
            false,
        )
        .await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].kind,
            if folder_first {
                "container"
            } else {
                "document"
            }
        );
        assert_eq!(rows[0].basename.to_ascii_lowercase(), "plan.md");
    }
    Ok(())
}
