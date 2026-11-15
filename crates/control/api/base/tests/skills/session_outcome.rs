//! Reference pages keep the accepted version and mask names separately for every current reader.
use super::*;

#[tokio::test]
async fn captured_session_pages_keep_their_revision_and_mask_a_skill_the_reader_cannot_view() {
    let world = World::new().await;
    let c = company(&world).await;
    let private = make(&world, &c.boss, c.acme, "private-captured-skill").await;
    let own = make(&world, &c.viewer, c.acme, "readable-captured-skill").await;
    let project = make_project(&world, &c.boss, c.acme, "Project", "project").await;
    let topic = places::make_topic(&world, &c.boss, project.base.id, "Topic")
        .await
        .base
        .id;
    let thread = orch_types::new_id();
    let segment = orch_types::new_id();
    let revision = orch_types::new_id();
    let mut conn = world.conn().await;
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        thread,
        "thread",
        Some(topic),
        "Captured session",
        c.boss.id(),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO threads(node_id,topic_id) VALUES($1,$2)")
        .bind(thread)
        .bind(topic)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO segments(id,thread_id,ordinal,env_generation,arrangement,credential_path,seq_updated,ended_at,import_fidelity) VALUES($1,$2,1,0,'separated','forwarded',1,$3,'lossless')")
        .bind(segment).bind(thread).bind(world.now()).execute(&mut *conn).await.unwrap();
    let selected = json!([
        {"skill_id":private.id,"version_id":private.latest.as_ref().unwrap().id,"number":1,"contributor":c.boss.id(),"from":{"kind":"place","node_id":c.acme}},
        {"skill_id":own.id,"version_id":own.latest.as_ref().unwrap().id,"number":1,"contributor":c.viewer.id(),"from":{"kind":"place","node_id":c.acme}}
    ]);
    sqlx::query("INSERT INTO session_skill_outcomes(revision,thread_id,segment_id,requested_at,applied_at,applied_seq,supported,fingerprint,selected,skipped) VALUES($1,$2,$3,$4,$4,1,true,'captured',$5,'[]')")
        .bind(revision).bind(thread).bind(segment).bind(world.now()).bind(selected).execute(&mut *conn).await.unwrap();
    drop(conn);
    let mut query = ThreadSessionSkillsQuery {
        continuation_id: None,
        projection_revision: None,
        inspect_deleted: None,
        segment_id: segment,
        session_revision: revision,
        cursor: None,
        limit: Some(1),
    };
    let first = world
        .app
        .call(ThreadSessionSkillsGet, &[&thread])
        .token(&c.viewer.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(first.summary.selected, 2);
    assert_eq!(first.entries.items.len(), 1);
    assert!(first.entries.items[0].skill.is_none());
    assert!(matches!(
        first.entries.items[0].decision,
        SessionSkillDecision::Selected { version: None }
    ));
    assert!(
        !serde_json::to_string(&first)
            .unwrap()
            .contains("private-captured-skill")
    );
    let replacement = orch_types::new_id();
    sqlx::query("INSERT INTO session_skill_outcomes(revision,thread_id,segment_id,requested_at,applied_at,applied_seq,supported,fingerprint,selected,skipped) VALUES($1,$2,$3,$4,$4,2,true,'replacement','[]','[]')")
        .bind(replacement).bind(thread).bind(segment).bind(world.now()).execute(&mut *world.conn().await).await.unwrap();
    sqlx::query("UPDATE segments SET session_skills=$2 WHERE id=$1")
        .bind(segment).bind(json!({"session_revision":replacement,"segment_id":segment,"applied_at":world.now(),"supported":true,"selected":0,"skipped":0}))
        .execute(&mut *world.conn().await).await.unwrap();
    query.cursor = first.entries.next_cursor;
    query.continuation_id = Some(first.projection.viewed_continuation_id);
    query.projection_revision = Some(first.projection.projection_revision);
    let next = world
        .app
        .call(ThreadSessionSkillsGet, &[&thread])
        .token(&c.viewer.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(next.summary.session_revision, revision);
    assert_eq!(
        next.entries.items[0].skill.as_ref().unwrap().name,
        "readable-captured-skill"
    );
    assert!(
        matches!(next.entries.items[0].decision,SessionSkillDecision::Selected{version:Some(ref version)} if version.number==1)
    );
    assert!(next.entries.next_cursor.is_none());
    query.cursor = None;
    let visible = world
        .app
        .call(ThreadSessionSkillsGet, &[&thread])
        .token(&c.boss.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        visible.entries.items[0].skill.as_ref().unwrap().name,
        "private-captured-skill"
    );
    assert_eq!(
        world
            .app
            .call(ThreadSessionSkillsGet, &[&thread])
            .token(&c.stranger.token)
            .query(&query)
            .send()
            .await
            .status,
        403
    );
}
