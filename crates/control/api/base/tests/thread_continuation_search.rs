//! Content search matches and hydrates the same frozen history, retaining its scope across bounded pages.

mod continuation_read_support;
mod places;
mod rig;
mod support;

use continuation_read_support::{Proof, Retained, retained, select};
use orch_api_contract::{
    common::ErrorCode,
    search::{SearchFound, SearchList, SearchListQuery},
    threads::ItemPayload,
};
use orch_types::{Id, Time};

async fn scene() -> Proof<(rig::Scene, Id, Retained)> {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(
            thread,
            0,
            "Page needle first",
            "Retainedneedle original answer",
        )
        .await;
    scene
        .exchange(
            thread,
            1,
            "Page needle second",
            "Excludedneedle second answer",
        )
        .await;
    rig::eventually(
        "native work finishes before selecting a search fixture history",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1
          AND (end_work IS NOT NULL OR files_end_due OR files_work_pending))",
            )
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let view = retained(
        &scene.rig.world.app.app.db,
        thread,
        scene.ada.id(),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    Ok((scene, thread, view))
}

fn query(thread: Id, view: Option<Id>, text: &str) -> SearchListQuery {
    SearchListQuery {
        q: text.into(),
        scope: Some(thread),
        continuation_id: view,
        kinds: Some("agent_message,user_message".into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn frozen_search_text_body_and_deep_link_scope_agree_without_showing_an_excluded_suffix()
-> Proof {
    let (scene, thread, view) = scene().await?;
    let native: String = sqlx::query_scalar("SELECT native_ref FROM items WHERE id=$1")
        .bind(view.prefix_item)
        .fetch_one(scene.rig.db.pool())
        .await?;
    scene
        .rig
        .say(
            thread,
            rig::message_done(&native, "Newsourcephrase current source answer"),
        )
        .await;
    rig::eventually(
        "the real retained native item gets its later current-body projection",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT payload->>'text'='Newsourcephrase current source answer'
          AND search_text LIKE '%Newsourcephrase%' FROM items WHERE id=$1",
            )
            .bind(view.prefix_item)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let app = &scene.rig.world.app;
    let before = (
        scene.rig.commands.issued_total(),
        scene.rig.turn_starts(thread).len(),
    );
    let child = app
        .call(SearchList, &[])
        .token(&scene.cai.token)
        .query(&query(thread, Some(view.child), "Retainedneedle"))
        .send()
        .await
        .ok();
    assert_eq!(child.items.len(), 1);
    assert!(child.next_cursor.is_none());
    let SearchFound::Item {
        item, projection, ..
    } = &child.items[0].found
    else {
        return Err("the actual retained item was not returned".into());
    };
    assert_eq!(item.id, view.prefix_item);
    assert_eq!(item.continuation_id, Some(view.root));
    assert_eq!(
        projection
            .as_ref()
            .map(|scope| (scope.viewed_continuation_id, scope.follows_active)),
        Some((view.child, false))
    );
    assert!(
        matches!(&item.payload,ItemPayload::AgentMessage(answer) if answer.text=="Retainedneedle original answer")
    );
    assert!(child.items[0].snippet.contains("Retainedneedle"));
    assert!(!child.items[0].snippet.contains("Newsourcephrase"));
    for text in ["Newsourcephrase", "Excludedneedle"] {
        let hidden = app
            .call(SearchList, &[])
            .token(&scene.cai.token)
            .query(&query(thread, Some(view.child), text))
            .send()
            .await
            .ok();
        assert!(hidden.items.is_empty() && hidden.next_cursor.is_none());
    }
    let source = app
        .call(SearchList, &[])
        .token(&scene.cai.token)
        .query(&query(thread, Some(view.root), "Newsourcephrase"))
        .send()
        .await
        .ok();
    assert_eq!(source.items.len(), 1);
    assert!(matches!(&source.items[0].found,SearchFound::Item {item,..}
        if item.id==view.prefix_item && matches!(&item.payload,ItemPayload::AgentMessage(answer) if answer.text=="Newsourcephrase current source answer")));
    let old_source = app
        .call(SearchList, &[])
        .token(&scene.cai.token)
        .query(&query(thread, Some(view.root), "Retainedneedle"))
        .send()
        .await
        .ok();
    assert!(old_source.items.is_empty());
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.child,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let global = app
        .call(SearchList, &[])
        .token(&scene.cai.token)
        .query(&SearchListQuery {
            q: "Excludedneedle".into(),
            kinds: Some("agent_message".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        global.items.is_empty() && global.next_cursor.is_none(),
        "default global search sees selected histories"
    );
    let global = app
        .call(SearchList, &[])
        .token(&scene.cai.token)
        .query(&SearchListQuery {
            q: "Retainedneedle".into(),
            kinds: Some("agent_message".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(global.items.len(), 1);
    assert!(
        matches!(&global.items[0].found,SearchFound::Item {item,projection:Some(scope),..}
        if item.id==view.prefix_item && scope.viewed_continuation_id==view.child)
    );
    let private = app
        .call(SearchList, &[])
        .token(&scene.bea.token)
        .query(&query(thread, Some(view.child), "Retainedneedle"))
        .send()
        .await
        .problem();
    assert_eq!(private.code, ErrorCode::NotFound);
    assert_eq!(
        (
            scene.rig.commands.issued_total(),
            scene.rig.turn_starts(thread).len()
        ),
        before,
        "searching/version deep links never prepare native work or author a turn"
    );
    Ok(())
}

#[tokio::test]
async fn an_opaque_thread_search_cursor_resets_after_away_and_back_but_an_explicit_history_cursor_stays_valid()
-> Proof {
    let (scene, thread, view) = scene().await?;
    let app = &scene.rig.world.app;
    let mut following = query(thread, None, "Page needle");
    following.limit = Some(1);
    let first = app
        .call(SearchList, &[])
        .token(&scene.ada.token)
        .query(&following)
        .send()
        .await
        .ok();
    assert_eq!(first.items.len(), 1);
    following.cursor = Some(
        first
            .next_cursor
            .ok_or("a one-row page must retain its next actual match")?,
    );
    let mut explicit = query(thread, Some(view.root), "Page needle");
    explicit.limit = Some(1);
    let first_explicit = app
        .call(SearchList, &[])
        .token(&scene.ada.token)
        .query(&explicit)
        .send()
        .await
        .ok();
    explicit.cursor = Some(
        first_explicit
            .next_cursor
            .ok_or("the explicit source page has another actual input")?,
    );
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.child,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.root,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let reset = app
        .call(SearchList, &[])
        .token(&scene.ada.token)
        .query(&following)
        .send()
        .await
        .problem();
    assert_eq!(reset.code, ErrorCode::ConflictStaleRevision);
    let next = app
        .call(SearchList, &[])
        .token(&scene.ada.token)
        .query(&explicit)
        .send()
        .await
        .ok();
    assert_eq!(next.items.len(), 1);
    assert_ne!(next.items[0], first_explicit.items[0]);
    assert!(next.next_cursor.is_none());
    assert!(matches!(&next.items[0].found,SearchFound::Item {item,..}
        if matches!(&item.payload,ItemPayload::UserMessage(message) if message.input_id==Some(view.first_input))));
    assert_eq!(scene.rig.turn_starts(thread).len(), 2);
    Ok(())
}
