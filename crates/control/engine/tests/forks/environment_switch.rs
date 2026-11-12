//! Switching uses the native fork planner, saved review and worker for directories and access.

use super::*;
use orch_api_contract::forks::ForkEnvironment;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_directory_switch_keeps_an_omitted_directory_and_commits_the_selected_one() {
    for directory in ["apps/project", ""] {
        let rig = Rig::new(Can::default(), "fake", Can::default()).await;
        let destination = rig.second_environment().await;
        // These are attached machine directories, with no unit template or host allocation.
        let environments = [rig.world.environment, destination];
        let attached = Arc::new(Mutex::new(Vec::new()));
        let recorded = attached.clone();
        rig.world
            .machine
            .handler
            .on(CommandKind::EnvAttach, move |command| {
                let Command::EnvAttach(body) = command else {
                    unreachable!()
                };
                assert!(environments.contains(&body.environment_id));
                let expected_root = if body.environment_id == destination {
                    "/second"
                } else {
                    "/workspace"
                };
                assert_eq!(body.workspace_root.as_deref(), Some(expected_root));
                let mut facts = hosting::facts();
                facts["workspace_root"] = json!(body.workspace_root);
                facts["generation"] = json!(body.data_generation.unwrap());
                recorded.lock().unwrap().push(body.environment_id);
                Ok(CommandResult::EnvAttach(
                    serde_json::from_value(facts).unwrap(),
                ))
            });
        rig.world.machine.handler.on(CommandKind::ExecStart, |_| {
            Ok(CommandResult::ExecStart(
                serde_json::from_value(json!({
                    "service":"mcp/exec", "version":"0.1.0", "tool_sets":["plain"]
                }))
                .unwrap(),
            ))
        });
        sqlx::query(
            "UPDATE environments SET placement='connected',template_id=NULL WHERE id=ANY($1)",
        )
        .bind(environments.as_slice())
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
        for environment in environments {
            let physical = orch_db::queries::environment_facts::of(
                &mut rig.world.pool.acquire().await.unwrap(),
                environment,
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(physical.machine_id, Some(rig.world.machine_id));
        }
        let thread = rig.thread_with_history("Changing directory").await;
        let mut request = ForkRequest {
            to: None,
            environment: Some(ForkEnvironment::Existing {
                environment_id: destination,
                files: None,
            }),
            ..rig.to_other()
        };
        let inherited = rig.plan(thread, &request).await;
        assert!(inherited.blockers.is_empty(), "{:?}", inherited.blockers);
        assert_eq!(inherited.to.directory, "web");

        request.directory = Some(directory.into());
        let plan = rig.plan(thread, &request).await;
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        assert_eq!(plan.to.directory, directory);
        let job = rig.confirm(thread, &plan).await;
        rig.wait_job(job.id, "succeeded").await;
        assert!(
            environments
                .iter()
                .all(|environment| attached.lock().unwrap().contains(environment)),
            "both connected directories must be attached by their native command"
        );
        let moved: Json = sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1 AND kind='marker' AND payload->>'marker'='environment_moved' ORDER BY seq_created DESC LIMIT 1")
            .bind(thread).fetch_one(rig.world.pool.pool()).await.unwrap();
        let moved: orch_api_contract::threads::MarkerPayload =
            serde_json::from_value(moved).unwrap();
        let orch_api_contract::threads::MarkerPayload::EnvironmentMoved(moved) = moved else {
            panic!("environment-only marker")
        };
        assert_eq!(moved.from.id, rig.world.environment);
        assert_eq!(moved.to.id, destination);
        assert_eq!(
            moved.from_machine.as_ref().map(|machine| machine.id),
            Some(rig.world.machine_id)
        );
        assert_eq!(
            moved.to_machine.as_ref().map(|machine| machine.id),
            Some(rig.world.machine_id)
        );
        let actual: (Id, String) =
            sqlx::query_as("SELECT environment_id, workspace_dir FROM threads WHERE node_id=$1")
                .bind(thread)
                .fetch_one(rig.world.pool.pool())
                .await
                .unwrap();
        assert_eq!(actual, (destination, directory.into()));
        let target = if directory.is_empty() {
            "/second".to_owned()
        } else {
            format!("/second/{directory}")
        };
        assert!(rig.files.lock().unwrap().iter().any(|command| {
            matches!(command, Command::FsMaterialize(write)
                if write.environment_id == destination && write.target_root == target)
        }));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_rejects_a_directory_outside_the_destination_workspace_before_planning_io() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let thread = rig.thread_with_history("Stay within the workspace").await;
    let before = rig.files.lock().unwrap().len();
    for directory in ["/outside", "../outside", "project/../../outside"] {
        let request = ForkRequest {
            to: None,
            directory: Some(directory.into()),
            ..rig.to_other()
        };
        let result = rig
            .world
            .engine()
            .draft_fork(&rig.caller(), thread, &request)
            .await;
        assert!(
            matches!(result, Err(PlanError::Invalid { .. })),
            "{result:?}"
        );
    }
    assert_eq!(rig.files.lock().unwrap().len(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_out_of_a_machine_directory_never_promises_to_remove_an_old_unit() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let thread = rig.thread_with_history("Directory stays").await;
    let destination = rig.second_environment().await;
    let mut conn = rig.world.pool.acquire().await.unwrap();
    assert!(
        !orch_db::queries::environment_facts::has_unit(&mut conn, rig.world.environment)
            .await
            .unwrap()
    );
    drop(conn);
    let plan = rig
        .plan(
            thread,
            &ForkRequest {
                to: None,
                environment: Some(ForkEnvironment::Existing {
                    environment_id: destination,
                    files: None,
                }),
                ..rig.to_other()
            },
        )
        .await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let left = plan
        .leaves
        .as_ref()
        .and_then(|left| left.environment.as_ref())
        .unwrap();
    assert_eq!(
        left.action,
        orch_api_contract::forks::LeftAction::NotRemovable
    );
}
