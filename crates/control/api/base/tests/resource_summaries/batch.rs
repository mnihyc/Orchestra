//! The real native batch reads only stored paragraphs under its own current source namespace.
use super::*;
use orch_api_contract::summaries::{SummaryBatchBody, SummaryBatchGet};

#[tokio::test]
async fn an_opted_in_document_sync_carries_only_the_summary_its_reader_may_read() {
    use orch_api_contract::documents::{DocumentSyncGet, DocumentSyncGetQuery};
    let desk = Desk::new().await;
    let reader = desk.member("document-summary-reader", "viewer").await;
    let id = desk
        .make(&desk.ada, "Document with a summary")
        .await
        .base
        .id;
    let source = desk.make(&desk.ada, "Contributing document").await.base.id;
    let target = SummaryTarget::Document { id };
    let input = SummaryTarget::Document { id: source };
    cache(
        &desk,
        target,
        None,
        "Stored document paragraph.",
        json!([target, input]),
    )
    .await;
    let query = DocumentSyncGetQuery {
        summary: Some(true),
        ..Default::default()
    };
    let single = desk
        .world
        .app
        .call(SummaryGet, &[&"document", &id])
        .token(&desk.ada.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(single.text.as_deref(), Some("Stored document paragraph."));
    let opening = desk
        .world
        .app
        .call(DocumentSyncGet, &[&id])
        .token(&desk.ada.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(opening.document.base.id, id);
    assert_eq!(opening.summary, Some(single));
    for without in [
        DocumentSyncGetQuery::default(),
        DocumentSyncGetQuery {
            since: Some(opening.seq),
            ..query.clone()
        },
    ] {
        let reply = desk
            .world
            .app
            .call(DocumentSyncGet, &[&id])
            .token(&desk.ada.token)
            .query(&without)
            .send()
            .await
            .ok();
        assert!(reply.summary.is_none());
    }

    let scope: orch_access_contract::key_scope::KeyScope = serde_json::from_value(json!({"rules":[
        {"target":{"kind":"document","id":id,"reach":"self"},"allow":[{"operation":"DocumentSyncGet.read","actions":["read"]}]},
        {"target":{"kind":"document","id":source,"reach":"self"},"allow":[{"operation":"SummaryGet","actions":["read"]}]}
    ]})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "Document opening summary control",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    access::testing::set_row(
        &mut *desk.world.conn().await,
        &access::PageKey::new("document", source, "live"),
        "any",
        "doc.view",
        false,
        false,
    )
    .await
    .unwrap();
    for credential in [&token, &reader.token] {
        let reply = desk
            .world
            .app
            .call(DocumentSyncGet, &[&id])
            .token(credential)
            .query(&query)
            .send()
            .await
            .ok();
        assert_eq!(reply.document.base.id, id);
        let summary = reply.summary.unwrap();
        assert_eq!(summary.target, target);
        assert!(summary.text.is_none());
        assert!(summary.producer.is_none());
        assert!(summary.updated_at.is_none());
    }
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM summary_calls)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *desk.world.conn().await).await.unwrap();
    assert_eq!(work, 0);
}

#[tokio::test]
async fn node_openings_carry_the_same_checked_topic_and_chat_views_without_paid_work() {
    use orch_api_contract::chats::{NodeChatOpeningGet, NodeChatOpeningGetQuery};
    let desk = Desk::new().await;
    let topic = SummaryTarget::Topic { id: desk.topic };
    cache(
        &desk,
        topic,
        None,
        "Shared topic paragraph.",
        json!([topic]),
    )
    .await;
    cache(
        &desk,
        topic,
        Some(desk.ada.id()),
        "This reader's complete topic paragraph.",
        json!([topic]),
    )
    .await;
    let opening = desk
        .world
        .app
        .call(NodeChatOpeningGet, &[&desk.topic])
        .token(&desk.ada.token)
        .query(&NodeChatOpeningGetQuery::default())
        .send()
        .await
        .ok();
    let chat = opening.opening.as_ref().unwrap().sync.chat.base.id;
    let range = opening.summary_range.unwrap();
    assert_eq!(opening.summaries.len(), 3);
    assert_eq!(
        opening
            .summaries
            .iter()
            .find(|view| view.target == topic)
            .unwrap()
            .text
            .as_deref(),
        Some("This reader's complete topic paragraph.")
    );
    for view in &opening.summaries {
        let query = if view.target == (SummaryTarget::Chat { id: chat }) {
            SummaryReadQuery {
                after: Some(range.after),
                through: Some(range.through),
            }
        } else {
            SummaryReadQuery::default()
        };
        let kind = match view.target {
            SummaryTarget::Chat { .. } => "chat",
            SummaryTarget::ChatTitle { .. } => "chat_title",
            SummaryTarget::Topic { .. } => "topic",
            _ => panic!("only the selected chat and this topic belong in the opening"),
        };
        let single = desk
            .world
            .app
            .call(SummaryGet, &[&kind, &view.target.id()])
            .token(&desk.ada.token)
            .query(&query)
            .send()
            .await
            .ok();
        assert_eq!(*view, single);
    }
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM summary_calls)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *desk.world.conn().await).await.unwrap();
    assert_eq!(work, 0);
}

#[tokio::test]
async fn topic_opening_summaries_keep_private_viewers_and_every_current_contributor() {
    use orch_api_contract::chats::{NodeChatOpeningGet, NodeChatOpeningGetQuery};
    let desk = Desk::new().await;
    let reader = desk.member("topic-summary-reader", "viewer").await;
    let topic = SummaryTarget::Topic { id: desk.topic };
    let doc = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Topic contributor").await.base.id,
    };
    cache(
        &desk,
        topic,
        None,
        "Shared topic paragraph.",
        json!([topic]),
    )
    .await;
    cache(
        &desk,
        topic,
        Some(desk.ada.id()),
        "Private viewer canary.",
        json!([topic]),
    )
    .await;
    for hidden in [false, true] {
        if hidden {
            cache(
                &desk,
                topic,
                None,
                "Hidden contributor canary.",
                json!([topic, doc]),
            )
            .await;
            access::testing::set_row(
                &mut *desk.world.conn().await,
                &access::PageKey::new("document", doc.id(), "live"),
                "any",
                "doc.view",
                false,
                false,
            )
            .await
            .unwrap();
        }
        let opening = desk
            .world
            .app
            .call(NodeChatOpeningGet, &[&desk.topic])
            .token(&reader.token)
            .query(&NodeChatOpeningGetQuery::default())
            .send()
            .await
            .ok();
        let single = desk
            .world
            .app
            .call(SummaryGet, &[&"topic", &desk.topic])
            .token(&reader.token)
            .query(&SummaryReadQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(
            opening.summaries.iter().find(|view| view.target == topic),
            Some(&single)
        );
        assert_eq!(
            single.text.as_deref(),
            (!hidden).then_some("Shared topic paragraph.")
        );
        assert_eq!(
            opening.summaries.len(),
            3,
            "a hidden contributor cannot drop the chat views"
        );
        assert!(!serde_json::to_string(&opening).unwrap().contains("canary"));
    }
}

#[tokio::test]
async fn a_topic_only_opening_key_keeps_its_summary_without_borrowing_chat_or_document_reads() {
    use orch_api_contract::chats::{NodeChatOpeningGet, NodeChatOpeningGetQuery};
    let desk = Desk::new().await;
    let topic = SummaryTarget::Topic { id: desk.topic };
    let doc = SummaryTarget::Document {
        id: desk
            .make(&desk.ada, "Scoped topic contributor")
            .await
            .base
            .id,
    };
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":[{
            "target":{"kind":"topic","id":desk.topic,"reach":"self"},"resources":["topic"],
            "allow":[{"operation":"NodeChatOpeningGet","actions":["read"]}]
        }]}))
        .unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "One topic opening",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    for hidden in [false, true] {
        cache(
            &desk,
            topic,
            None,
            if hidden {
                "Unscoped document canary."
            } else {
                "Admitted topic paragraph."
            },
            if hidden {
                json!([topic, doc])
            } else {
                json!([topic])
            },
        )
        .await;
        let opening = desk
            .world
            .app
            .call(NodeChatOpeningGet, &[&desk.topic])
            .token(&token)
            .query(&NodeChatOpeningGetQuery::default())
            .send()
            .await
            .ok();
        assert!(opening.scopes.is_empty());
        assert!(opening.opening.is_none());
        assert!(opening.summary_range.is_none());
        assert_eq!(opening.summaries.len(), 1);
        assert_eq!(opening.summaries[0].target, topic);
        assert_eq!(
            opening.summaries[0].text.as_deref(),
            (!hidden).then_some("Admitted topic paragraph.")
        );
        assert!(!serde_json::to_string(&opening).unwrap().contains("canary"));
    }
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM summary_calls)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *desk.world.conn().await).await.unwrap();
    assert_eq!(work, 0);
}

/// What the thread's own opening, its singleton reader and its full batch reader each give `token`.
async fn thread_views(
    world: &support::World,
    token: &str,
    thread: Id,
) -> (
    orch_api_contract::chats::NodeChatOpening,
    orch_api_contract::summaries::SummarySnapshot,
    orch_api_contract::summaries::SummaryBatchReply,
) {
    use orch_api_contract::chats::{NodeChatOpeningGet, NodeChatOpeningGetQuery};
    let opening = world
        .app
        .call(NodeChatOpeningGet, &[&thread])
        .token(token)
        .query(&NodeChatOpeningGetQuery::default())
        .send()
        .await
        .ok();
    let single = world
        .app
        .call(SummaryGet, &[&"thread", &thread])
        .token(token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    let batch = world
        .app
        .call(SummaryBatchGet, &[])
        .token(token)
        .body(&SummaryBatchBody {
            targets: vec![SummaryTarget::Thread { id: thread }],
            full: true,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    (opening, single, batch)
}

#[tokio::test]
async fn a_thread_opening_carries_the_view_its_readers_give_beside_the_chat_views() {
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let world = &scene.rig.world;
    let thread = scene.thread().await.base.id;
    let target = SummaryTarget::Thread { id: thread };
    cache_in(
        world,
        target,
        None,
        "Shared thread paragraph.",
        json!([target]),
    )
    .await;
    cache_in(
        world,
        target,
        Some(scene.ada.id()),
        "This reader's complete thread paragraph.",
        json!([target]),
    )
    .await;
    let (opening, single, batch) = thread_views(world, &scene.ada.token, thread).await;
    let chat = opening.opening.as_ref().unwrap().sync.chat.base.id;
    assert_eq!(
        opening.summaries.len(),
        3,
        "the thread's own view comes beside the chat's catch-up and title"
    );
    assert!(opening.summary_range.is_some());
    for chat_view in [
        SummaryTarget::Chat { id: chat },
        SummaryTarget::ChatTitle { id: chat },
    ] {
        assert!(
            opening
                .summaries
                .iter()
                .any(|view| view.target == chat_view)
        );
    }
    let carried = opening
        .summaries
        .iter()
        .find(|view| view.target == target)
        .unwrap();
    assert_eq!(
        carried.text.as_deref(),
        Some("This reader's complete thread paragraph.")
    );
    assert_eq!(carried, &single);
    assert!(batch.unavailable.is_empty());
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].full.as_ref(), Some(carried));
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM summary_calls)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *world.conn().await).await.unwrap();
    assert_eq!(work, 0);
}

#[tokio::test]
async fn thread_opening_summaries_keep_private_viewers_and_every_current_contributor() {
    use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let world = &scene.rig.world;
    let thread = scene.thread().await.base.id;
    let target = SummaryTarget::Thread { id: thread };
    let contributor = SummaryTarget::Document {
        id: world
            .app
            .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
            .token(&scene.ada.token)
            .body(&NodeDocumentCreateBody {
                title: "Thread contributor".into(),
                blocks: None,
                markdown: Some("A document the thread paragraph draws on.".into()),
                access: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap()
            .base
            .id,
    };
    cache_in(
        world,
        target,
        None,
        "Shared thread paragraph.",
        json!([target]),
    )
    .await;
    cache_in(
        world,
        target,
        Some(scene.ada.id()),
        "Private viewer canary.",
        json!([target]),
    )
    .await;
    let mut chat_views = None;
    for hidden in [false, true] {
        if hidden {
            cache_in(
                world,
                target,
                None,
                "Hidden contributor canary.",
                json!([target, contributor]),
            )
            .await;
            access::testing::set_row(
                &mut *world.conn().await,
                &access::PageKey::new("document", contributor.id(), "live"),
                "any",
                "doc.view",
                false,
                false,
            )
            .await
            .unwrap();
        }
        let (opening, single, batch) = thread_views(world, &scene.cai.token, thread).await;
        assert_eq!(
            opening.summaries.iter().find(|view| view.target == target),
            Some(&single)
        );
        assert_eq!(batch.items[0].full.as_ref(), Some(&single));
        assert_eq!(
            single.text.as_deref(),
            (!hidden).then_some("Shared thread paragraph.")
        );
        let kept = opening
            .summaries
            .iter()
            .filter(|view| view.target != target)
            .count();
        assert_eq!(
            *chat_views.get_or_insert(kept),
            kept,
            "a hidden contributor cannot drop the chat views"
        );
        assert!(!serde_json::to_string(&opening).unwrap().contains("canary"));
    }
}

#[tokio::test]
async fn a_stale_or_pending_thread_paragraph_comes_in_the_state_the_batch_reader_reports() {
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let world = &scene.rig.world;
    let thread = scene.thread().await.base.id;
    let target = SummaryTarget::Thread { id: thread };
    let summary = cache_in(
        world,
        target,
        None,
        "Previous readable paragraph.",
        json!([target]),
    )
    .await;
    let (opening, single, batch) = thread_views(world, &scene.ada.token, thread).await;
    let carried = opening
        .summaries
        .iter()
        .find(|view| view.target == target)
        .unwrap();
    assert_eq!(carried, &single);
    assert_eq!(batch.items[0].full.as_ref(), Some(carried));
    assert_eq!(
        carried.text.as_deref(),
        Some("Previous readable paragraph.")
    );
    assert_ne!(
        carried.state,
        SummaryState::Fresh,
        "a paragraph whose captured source is not the current one is never claimed fresh"
    );
    let now = world.now();
    let due = now + chrono::Duration::minutes(15);
    // enqueue accepts only inside the caller's transaction.
    let mut tx = orch_db::Db::from_pool(world.db.pool.clone())
        .begin()
        .await
        .unwrap();
    let result = rows::enqueue(
        &mut tx,
        summary,
        rows::Origin::Automatic {
            writer: scene.ada.id(),
        },
        &json!({"client_id":new_id(),"model":null,"range":null}),
        &json!({"target":target}),
        now,
        due,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let rows::Enqueued::New(pending) = result else {
        panic!("one native queued fixture");
    };
    let (opening, single, batch) = thread_views(world, &scene.ada.token, thread).await;
    let carried = opening
        .summaries
        .iter()
        .find(|view| view.target == target)
        .unwrap();
    assert_eq!(carried.state, SummaryState::Scheduled);
    assert_eq!(
        carried.text.as_deref(),
        Some("Previous readable paragraph.")
    );
    let wait = carried.wait.clone().unwrap();
    assert_eq!(wait.until.map(|until| until.get()), Some(due));
    assert_eq!(carried, &single);
    assert_eq!(batch.items[0].full.as_ref(), Some(carried));
    let mut conn = world.conn().await;
    let kept = rows::request(&mut conn, pending.id).await.unwrap().unwrap();
    assert_eq!(kept.state, "queued", "reading starts and claims no work");
    assert!(kept.lease_token.is_none());
}

#[tokio::test]
async fn a_thread_only_opening_key_keeps_its_summary_without_borrowing_chat_or_document_reads() {
    use orch_api_contract::chats::{NodeChatOpeningGet, NodeChatOpeningGetQuery};
    use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
    let scene = rig::Scene::new().await;
    scene.rig.engine.stop();
    let world = &scene.rig.world;
    let thread = scene.thread().await.base.id;
    let target = SummaryTarget::Thread { id: thread };
    let contributor = SummaryTarget::Document {
        id: world
            .app
            .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
            .token(&scene.ada.token)
            .body(&NodeDocumentCreateBody {
                title: "Scoped thread contributor".into(),
                blocks: None,
                markdown: Some("A document outside the key.".into()),
                access: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap()
            .base
            .id,
    };
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":[{
            "target":{"kind":"thread","id":thread,"reach":"self"},"resources":["thread"],
            "allow":[{"operation":"NodeChatOpeningGet","actions":["read"]}]
        }]}))
        .unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: scene.ada.id(),
            name: "One thread opening",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    for hidden in [false, true] {
        cache_in(
            world,
            target,
            None,
            if hidden {
                "Unscoped document canary."
            } else {
                "Admitted thread paragraph."
            },
            if hidden {
                json!([target, contributor])
            } else {
                json!([target])
            },
        )
        .await;
        let opening = world
            .app
            .call(NodeChatOpeningGet, &[&thread])
            .token(&token)
            .query(&NodeChatOpeningGetQuery::default())
            .send()
            .await
            .ok();
        assert!(opening.scopes.is_empty());
        assert!(opening.opening.is_none());
        assert!(opening.summary_range.is_none());
        assert_eq!(opening.summaries.len(), 1);
        assert_eq!(opening.summaries[0].target, target);
        assert_eq!(
            opening.summaries[0].text.as_deref(),
            (!hidden).then_some("Admitted thread paragraph.")
        );
        assert!(!serde_json::to_string(&opening).unwrap().contains("canary"));
    }
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM summary_calls)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *world.conn().await).await.unwrap();
    assert_eq!(work, 0);
}

#[tokio::test]
async fn full_batches_keep_the_singleton_state_and_refuse_unrelated_chat_ranges() {
    let desk = Desk::new().await;
    let target = SummaryTarget::Document {
        id: desk.make(&desk.ada, "A full summary source").await.base.id,
    };
    cache(
        &desk,
        target,
        None,
        "The actual retained paragraph.",
        json!([target]),
    )
    .await;
    let single = desk
        .world
        .app
        .call(SummaryGet, &[&"document", &target.id()])
        .token(&desk.ada.token)
        .query(&SummaryReadQuery::default())
        .send()
        .await
        .ok();
    let batch = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&desk.ada.token)
        .body(&SummaryBatchBody {
            targets: vec![target],
            full: true,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(batch.unavailable.is_empty());
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].full.as_ref(), Some(&single));
    assert_eq!(batch.items[0].text, single.text);
    let invalid = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&desk.ada.token)
        .body(&SummaryBatchBody {
            targets: vec![target],
            full: true,
            ranges: [(
                target.id(),
                orch_api_contract::summaries::SummaryChatRange {
                    after: 0,
                    through: 7,
                },
            )]
            .into(),
            ..Default::default()
        })
        .send()
        .await;
    assert_eq!(invalid.problem().code, ErrorCode::RequestInvalid);
    let work: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM summary_calls)+(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *desk.world.conn().await).await.unwrap();
    assert_eq!(work, 0, "full screen reads never admit paid work");
}

#[tokio::test]
async fn a_batch_key_cannot_borrow_a_missing_governors_read_from_its_primary_target() {
    let desk = Desk::new().await;
    let doc = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Scoped contributor").await.base.id,
    };
    let topic = SummaryTarget::Topic { id: desk.topic };
    cache(
        &desk,
        topic,
        None,
        "Private derived canary.",
        json!([topic, doc]),
    )
    .await;
    let scope:orch_access_contract::key_scope::KeyScope=serde_json::from_value(json!({"rules":[{
        "target":{"kind":"topic","id":desk.topic,"reach":"self"},"resources":["topic"],"allow":[{"operation":"SummaryBatchGet","actions":["read"]}]
    }]})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "One batch source",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    for full in [false, true] {
        let reply = desk
            .world
            .app
            .call(SummaryBatchGet, &[])
            .token(&token)
            .body(&SummaryBatchBody {
                targets: vec![topic],
                full,
                offset: 0,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(reply.items.len(), 1);
        assert_eq!(reply.items[0].text, None);
        assert!(!serde_json::to_string(&reply).unwrap().contains("canary"));
    }
}

#[tokio::test]
async fn mixed_batch_keeps_private_viewers_and_every_governor_without_paid_work() {
    let desk = Desk::new().await;
    let reader = desk.member("batch-reader", "viewer").await;
    let good = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Readable").await.base.id,
    };
    let derived = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Public container").await.base.id,
    };
    let private = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Private source canary").await.base.id,
    };
    access::testing::set_row(
        &mut *desk.world.conn().await,
        &access::PageKey::new("document", private.id(), "live"),
        "any",
        "doc.view",
        false,
        false,
    )
    .await
    .unwrap();
    cache(&desk, good, None, "Public paragraph.", json!([good])).await;
    cache(
        &desk,
        good,
        Some(desk.ada.id()),
        "Another viewer private paragraph canary.",
        json!([good]),
    )
    .await;
    cache(
        &desk,
        derived,
        None,
        "Hidden derived paragraph canary.",
        json!([derived, private]),
    )
    .await;
    let missing = SummaryTarget::Document { id: new_id() };
    for full in [false, true] {
        let reply = desk
            .world
            .app
            .call(SummaryBatchGet, &[])
            .token(&reader.token)
            .body(&SummaryBatchBody {
                targets: vec![good, derived, private, missing, good],
                full,
                offset: 0,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(reply.items.len(), 2);
        assert!(reply.items.iter().all(|item| item.full.is_some() == full));
        assert_eq!(
            reply
                .items
                .iter()
                .find(|item| item.target == good)
                .and_then(|item| item.text.as_deref()),
            Some("Public paragraph.")
        );
        assert_eq!(
            reply
                .items
                .iter()
                .find(|item| item.target == derived)
                .and_then(|item| item.text.as_deref()),
            None
        );
        assert_eq!(reply.unavailable, vec![private, missing]);
        let encoded = serde_json::to_string(&reply).unwrap();
        assert!(!encoded.contains("canary"));
        let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM resource_summary_requests),(SELECT count(*) FROM summary_calls),(SELECT count(*) FROM usage_events WHERE purpose='summary')")
        .fetch_one(&mut *desk.world.conn().await).await.unwrap();
        assert_eq!(counts, (0, 0, 0));
    }
}

#[tokio::test]
async fn batch_windows_continue_and_bind_only_actual_current_read_sources() {
    let desk = Desk::new().await;
    let one = SummaryTarget::Document {
        id: desk.make(&desk.ada, "First").await.base.id,
    };
    let two = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Second").await.base.id,
    };
    let three = SummaryTarget::Document {
        id: desk.make(&desk.ada, "Third").await.base.id,
    };
    let value = json!(2);
    orch_db::queries::settings::put(
        &mut *desk.world.conn().await,
        None,
        "summary.read_batch_size",
        &orch_db::queries::settings::Stored {
            value: &value,
            locked: false,
            by: Some(desk.ada.id()),
            at: desk.world.now(),
        },
        None,
    )
    .await
    .unwrap();
    let first = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&desk.ada.token)
        .body(&SummaryBatchBody {
            targets: vec![one, two, three],
            offset: 0,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.target)
            .collect::<Vec<_>>(),
        vec![one, two]
    );
    assert_eq!(first.next_offset, Some(2));
    let second = desk
        .world
        .app
        .call(SummaryBatchGet, &[])
        .token(&desk.ada.token)
        .body(&SummaryBatchBody {
            targets: vec![one, two, three],
            offset: 2,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        second
            .items
            .iter()
            .map(|item| item.target)
            .collect::<Vec<_>>(),
        vec![three]
    );
    assert_eq!(second.next_offset, None);
}

#[tokio::test]
async fn a_batch_read_during_which_the_summarys_request_is_taken_up_is_answered_with_the_newer_state()
 {
    let desk = Desk::new().await;
    let doc = desk
        .make(&desk.ada, "A summary that is being written")
        .await
        .base
        .id;
    let target = SummaryTarget::Document { id: doc };
    let summary = cache(
        &desk,
        target,
        None,
        "The earlier paragraph.",
        json!([target]),
    )
    .await;
    let request = queued_request(&desk, summary, target).await;
    let log = Boundaries::new();
    let pool = desk.world.db.pool.clone();
    let moved =
        when_the_read_is_checked_again(&log, "SummaryBatchGet", "selected.settle", move || {
            take_up(pool.clone(), request)
        });
    let batch = follow(log, async {
        desk.world
            .app
            .call(SummaryBatchGet, &[])
            .token(&desk.ada.token)
            .body(&SummaryBatchBody {
                targets: vec![target],
                full: true,
                ..Default::default()
            })
            .send()
            .await
    })
    .await
    .ok();
    assert!(moved.load(Ordering::SeqCst));
    assert!(batch.unavailable.is_empty());
    let full = batch.items[0].full.as_ref().unwrap();
    assert_eq!(full.state, SummaryState::Updating);
    assert!(
        full.wait.is_none(),
        "the batch gives the newer state, in which the request is no longer waiting for its turn"
    );
}

#[tokio::test]
async fn a_document_opening_that_carries_a_summary_is_answered_when_the_summary_moves_while_it_is_read()
 {
    use orch_api_contract::documents::{DocumentSyncGet, DocumentSyncGetQuery};
    let desk = Desk::new().await;
    let id = desk
        .make(&desk.ada, "Document with a summary that moves")
        .await
        .base
        .id;
    let target = SummaryTarget::Document { id };
    let summary = cache(
        &desk,
        target,
        None,
        "The earlier paragraph.",
        json!([target]),
    )
    .await;
    let request = queued_request(&desk, summary, target).await;
    let log = Boundaries::new();
    let pool = desk.world.db.pool.clone();
    let moved =
        when_the_read_is_checked_again(&log, "DocumentSyncGet", "embedded.settle", move || {
            take_up(pool.clone(), request)
        });
    let opening = follow(log, async {
        desk.world
            .app
            .call(DocumentSyncGet, &[&id])
            .token(&desk.ada.token)
            .query(&DocumentSyncGetQuery {
                summary: Some(true),
                ..Default::default()
            })
            .send()
            .await
    })
    .await
    .ok();
    assert!(moved.load(Ordering::SeqCst));
    assert_eq!(opening.document.base.id, id);
    assert_eq!(
        opening
            .summary
            .as_ref()
            .and_then(|summary| summary.text.as_deref()),
        Some("The earlier paragraph.")
    );
}
