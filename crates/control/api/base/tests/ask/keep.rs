//! Keeping captured words does not hold the conversation while its copy is published.
use super::*;

#[tokio::test]
async fn keeping_a_conversation_allows_another_question_while_its_copy_is_publishing() {
    let s = Scene::new().await;
    configured_provider(&s).await;
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, |_| Ok(simple_answer()));
    let made = create(&s, &s.ada, None).await;
    let source = made.conversation_id;
    let initial = "The question captured by Keep";
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&source])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "before-keep".into(),
            content: vec![ContentPart::Text {
                text: initial.into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the captured conversation is ready", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(source)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;

    // This exact publication latch replaces the cost of any number of later copy pages.
    // Source questions never take it; only a copied answer in another thread does.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION pause_kept_answer() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
          IF NEW.thread_id<>'{source}'::uuid AND NEW.kind='agent_message' THEN
            PERFORM pg_advisory_xact_lock(1702,1);
          END IF;
          RETURN NEW;
        END $$;
        CREATE TRIGGER pause_kept_answer AFTER INSERT ON items
          FOR EACH ROW EXECUTE FUNCTION pause_kept_answer();"
    )))
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let mut latch = s.rig.db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(1702,1)")
        .execute(&mut *latch)
        .await
        .unwrap();
    let mut keeping = Box::pin(
        s.rig
            .world
            .app
            .call(AskKeep, &[&source])
            .token(&s.ada.token)
            .body(&AskKeepBody {
                topic_id: Some(s.tree.topic.base.id),
                ..Default::default()
            })
            .send(),
    );
    let publishing = eventually("Keep reaches the held publication", || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_locks
          WHERE locktype='advisory' AND classid=1702::oid AND objid=1::oid AND objsubid=2
            AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND NOT granted)")
            .fetch_one(s.rig.db.pool()).await.unwrap()
    });
    tokio::pin!(publishing);
    tokio::select! {
        () = &mut publishing => {},
        reply = &mut keeping => panic!("Keep returned before its publication was released: {}",reply.status),
    }
    let later = "This later question belongs only to the source";
    let sending = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&source])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "during-keep".into(),
            content: vec![ContentPart::Text { text: later.into() }],
            context: None,
            model: None,
        })
        .send();
    tokio::pin!(sending);
    let accepted = tokio::time::timeout(rig::PATIENCE, async {
        tokio::select! {
            reply = &mut sending => reply.ok(),
            reply = &mut keeping => panic!("Keep escaped its publication latch: {}",reply.status),
        }
    })
    .await
    .expect("a real source question completes while publication stays parked");
    assert_eq!(accepted.conversation_id, source);
    assert!(accepted.turn_id.is_some());
    latch.rollback().await.unwrap();
    let kept = tokio::time::timeout(rig::PATIENCE, keeping)
        .await
        .expect("the released copy finishes")
        .ok();
    let state: String = sqlx::query_scalar("SELECT ask_state FROM threads WHERE node_id=$1")
        .bind(source)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_ne!(
        state, "kept",
        "Keep cannot hide work accepted after its capture"
    );
    let copied: Vec<String> = sqlx::query_scalar(
        "SELECT payload::text FROM items
        WHERE thread_id=$1 AND kind='user_message' ORDER BY seq_created",
    )
    .bind(kept.thread_id)
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap();
    assert!(copied.iter().any(|payload| payload.contains(initial)));
    assert!(
        !copied.iter().any(|payload| payload.contains(later)),
        "the copy retains the accepted capture, not the source's later question"
    );
}

#[tokio::test]
async fn cancelling_keep_releases_its_capture_and_leaves_source_questions_available_during_cleanup()
{
    let s = Scene::new().await;
    configured_provider(&s).await;
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, |_| Ok(simple_answer()));
    let made = create(&s, &s.ada, None).await;
    let source = made.conversation_id;
    let initial = "The question captured by Keep";
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&source])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "before-keep".into(),
            content: vec![ContentPart::Text {
                text: initial.into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the captured conversation is ready", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(source)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;

    // This exact publication latch replaces the cost of any number of later copy pages.
    // Source questions never take it; only a copied answer in another thread does.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION pause_kept_answer() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
          IF NEW.thread_id<>'{source}'::uuid AND NEW.kind='agent_message' THEN
            PERFORM pg_advisory_xact_lock(1702,1);
          END IF;
          RETURN NEW;
        END $$;
        CREATE TRIGGER pause_kept_answer AFTER INSERT ON items
          FOR EACH ROW EXECUTE FUNCTION pause_kept_answer();"
    )))
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let mut latch = s.rig.db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(1702,1)")
        .execute(&mut *latch)
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION pause_capture_cleanup() RETURNS trigger LANGUAGE plpgsql AS $$
      BEGIN PERFORM pg_advisory_xact_lock(1703,1); RETURN OLD; END $$;
      CREATE TRIGGER pause_capture_cleanup BEFORE DELETE ON thread_history_cut_rows
        FOR EACH ROW EXECUTE FUNCTION pause_capture_cleanup();",
    )
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let mut cleanup_latch = s.rig.db.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(1703,1)")
        .execute(&mut *cleanup_latch)
        .await
        .unwrap();
    let mut keeping = Box::pin(
        s.rig
            .world
            .app
            .call(AskKeep, &[&source])
            .token(&s.ada.token)
            .body(&AskKeepBody {
                topic_id: Some(s.tree.topic.base.id),
                ..Default::default()
            })
            .send(),
    );
    let publishing = eventually("the cancelled Keep reaches its publication", || async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM pg_locks
          WHERE locktype='advisory' AND classid=1702::oid AND objid=1::oid AND objsubid=2
            AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND NOT granted)")
            .fetch_one(s.rig.db.pool()).await.unwrap()
    });
    tokio::pin!(publishing);
    tokio::select! {
        ()=&mut publishing=>{},
        reply=&mut keeping=>panic!("Keep answered before its held publication: {}",reply.status),
    }
    let captures: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_history_cuts WHERE thread_id=$1")
            .bind(source)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        captures, 1,
        "capture is durable before copied publication starts"
    );
    drop(keeping);
    latch.rollback().await.unwrap();
    eventually("cancelled Keep reaches its owned cleanup",||async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM pg_locks
          WHERE locktype='advisory' AND classid=1703::oid AND objid=1::oid AND objsubid=2
            AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND NOT granted)")
            .fetch_one(s.rig.db.pool()).await.unwrap()
    }).await;
    tokio::time::timeout(rig::PATIENCE,
        sqlx::query("UPDATE items SET payload=jsonb_set(payload,'{parts,0,text}',to_jsonb('Edited while old capture cleanup is parked'::text))
          WHERE thread_id=$1 AND kind='agent_message'")
            .bind(source).execute(s.rig.db.pool()))
        .await.expect("a retired capture does not delay an old-body writer").unwrap();
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&source])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "during-capture-cleanup".into(),
            content: vec![ContentPart::Text {
                text: "A question while capture cleanup is parked".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    cleanup_latch.rollback().await.unwrap();
    eventually(
        "cancelled Keep needs no later request to release its capture",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM thread_history_cuts WHERE thread_id=$1)
          AND NOT EXISTS(SELECT 1 FROM idempotency_keys WHERE path LIKE '/internal/history/%')",
            )
            .bind(source)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
        },
    )
    .await;
    let targets: i64 =
        sqlx::query_scalar("SELECT count(*) FROM threads WHERE program_root_org_id=$1")
            .bind(s.tree.org.base.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        targets, 0,
        "cancelled publication exposes no partial target thread"
    );
}
