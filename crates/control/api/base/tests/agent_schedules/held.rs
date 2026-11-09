//! The managed producer inherits the ordinary tray's stored Stop state and future-only cancellation.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_occurrence_keeps_stops_hold_and_deleting_its_schedule_keeps_the_queued_input() {
    let (scene, thread, turn) = setup(true).await;
    let id = create(
        &scene,
        thread,
        turn,
        "held-future",
        json!({"in_seconds":60,"note":"A requested check after Stop"}),
    )
    .await;
    sqlx::query("UPDATE threads SET queue_held=true,queue_held_by=$2,queue_held_at=$3,queue_revision=queue_revision+1 WHERE node_id=$1")
        .bind(thread).bind(scene.ada.id()).bind(scene.rig.world.now()).execute(scene.rig.db.pool()).await.unwrap();
    finish(&scene, thread, turn).await;
    scene.rig.world.advance(Duration::seconds(61));
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        1
    );
    let (input,held):(Id,bool)=sqlx::query_as("SELECT i.id,i.queue_held FROM thread_inputs i JOIN input_schedule_runs r ON r.input_id=i.id WHERE r.schedule_id=$1")
        .bind(id).fetch_one(scene.rig.db.pool()).await.unwrap();
    assert!(held, "a managed schedule cannot resume Stop-held work");
    assert_eq!(scene.rig.turn_starts(thread).len(), 1);
    scene
        .rig
        .world
        .app
        .call(InputScheduleDelete, &[&id])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let (state, held): (String, bool) =
        sqlx::query_as("SELECT state,queue_held FROM thread_inputs WHERE id=$1")
            .bind(input)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(state, "pending");
    assert!(
        held,
        "deleting future occurrences does not cancel or resume an already queued input"
    );
}
