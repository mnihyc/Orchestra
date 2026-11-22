//! What a thread's agent is set to and could be set to, and a thread exported whole as JSON: each read by whoever may read the thread, and refused to a stranger.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;
use support::FullSettingReply;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::settings::{ModelOptionReason, ThreadParameterGet};
use orch_api_contract::threads::{
    ExportDetail, ExportFormat, ItemKind, ThreadExport, ThreadExportJson, ThreadExportQuery,
};
use orch_settings::Target;
use places::missing_permissions;
use rig::{MODEL, Scene};
use serde_json::json;

#[tokio::test]
async fn a_program_without_an_auto_check_keeps_the_mode_dim_in_the_chip_and_its_setting() {
    use orch_api_contract::settings::NodeSettingList;
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let parameters = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let parameters = serde_json::to_value(parameters).unwrap();
    let automatic = parameters["approval_choices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice["mode"] == "auto")
        .expect("Auto stays visible with the actual missing verification");
    assert_eq!(automatic["available"], false);
    assert_eq!(automatic["reason"], "auto_check_unavailable");
    assert!(
        !parameters["approval_modes"]
            .as_array()
            .unwrap()
            .contains(&json!("auto"))
    );
    let rows = s
        .rig
        .world
        .app
        .call(NodeSettingList, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let row =
        serde_json::to_value(rows.iter().find(|row| row.key == "approval.mode").unwrap()).unwrap();
    assert_eq!(row["approval_choices"], parameters["approval_choices"]);
    assert!(
        !row["available_values"]
            .as_array()
            .unwrap()
            .contains(&json!("auto"))
    );
}

/// A thread whose agent program is Codex 1.0.0 at a selected path of a connected machine (with a node of its own or
/// none, and a build host of the system online or none), whose bytes the catalogue build `harness_channel:auto-choice`
/// names once it is kept.
async fn auto_candidate(has_node: bool, build_host: bool) -> (Scene, orch_types::Id) {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    if build_host {
        let mut conn = s.rig.world.conn().await;
        let host = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "host",
            None,
            None,
            "check-host",
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
        )
        .bind(host)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    let mut descriptor = rig::descriptor();
    descriptor["kind"] = json!("codex");
    descriptor["approval_modes"] = json!(["on_request", "auto"]);
    let declared: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(descriptor.clone()).unwrap();
    s.rig.machine.handler.on(
        orch_link_contract::commands::CommandKind::AgentStart,
        move |_| {
            Ok(orch_link_contract::commands::CommandResult::AgentStart(
                orch_link_contract::commands::AgentStartResult {
                    home: None,
                    reused: false,
                    harness_version: "1.0.0".into(),
                    descriptor: declared.clone(),
                },
            ))
        },
    );
    sqlx::query("UPDATE agents SET harness_kind='codex',descriptor=$2 WHERE id=$1")
        .bind(s.on.agent)
        .bind(&descriptor)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE harnesses SET descriptor=$2,harness_version='1.0.0' WHERE agent_id=$1")
        .bind(s.on.agent)
        .bind(&descriptor)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET placement='connected',roles=array_remove(roles,'node') || $3::text[],facts=facts || $2 WHERE id=$1")
        .bind(s.rig.machine_id)
        .bind(json!({"policy":{"agent.harnesses":[{"kind":"codex","path":"/selected/codex"}]},
            "harnesses":[{"kind":"codex","path":"/selected/codex","version":"1.0.0","sha256":"c".repeat(64)}]}))
        .bind(if has_node {vec!["node"]} else {vec![]})
        .execute(s.rig.db.pool()).await.unwrap();
    (s, thread)
}

#[tokio::test]
async fn an_unchecked_or_failed_build_has_the_same_exact_auto_reason_in_the_chip_and_setting() {
    use orch_api_contract::settings::NodeSettingList;
    // A machine a person connected has no runner of its own: its check runs on a build host of the system, and
    // without one online Auto names that instead of a check that cannot run.
    for (has_node, build_host, failed, reason) in [
        (true, false, false, "auto_not_proven"),
        (false, false, false, "auto_check_needs_host"),
        (false, true, false, "auto_not_proven"),
        (false, false, true, "auto_check_failed"),
    ] {
        let (s, thread) = auto_candidate(has_node, build_host).await;
        let check = failed.then(|| {
            json!({"at":s.rig.world.now(),"machine":"test machine","missing":[],
            "off":["approval_modes.auto"],"error":null,"stage":"complete","cases":[{
                "name":"auto_review","feature":"approval_modes.auto","passed":false,
                "error":"The scripted reviewer did not answer","unsupported":null,"elapsed_ms":1}]})
        });
        let mut conn = s.rig.world.conn().await;
        orch_db::queries::resources::management::put_channel_build(&mut conn,"harness_channel:auto-choice", &json!({
            "kind":"codex","version":"1.0.0","os":"linux","arch":"x86_64",
            "url":"https://example.com/program","sha256":"c".repeat(64),"is_default":false,"source":"channel",
            "status":if failed {"failed"} else {"untested"},"check":check
        }),s.rig.world.now()).await.unwrap();
        drop(conn);
        let parameters = serde_json::to_value(
            s.rig
                .world
                .app
                .call(ThreadParameterGet, &[&thread])
                .token(&s.ada.token)
                .send()
                .await
                .ok(),
        )
        .unwrap();
        let automatic = parameters["approval_choices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|choice| choice["mode"] == "auto")
            .unwrap();
        assert_eq!(automatic["available"], false);
        assert_eq!(automatic["reason"], reason);
        assert_eq!(
            automatic["check"],
            if reason == "auto_check_needs_host" {
                serde_json::Value::Null
            } else {
                json!({"kind":"codex","version":"1.0.0","machine_id":s.rig.machine_id,"sha256":"c".repeat(64)})
            },
            "{reason}"
        );
        assert!(
            !parameters["approval_modes"]
                .as_array()
                .unwrap()
                .contains(&json!("auto"))
        );
        let rows = s
            .rig
            .world
            .app
            .call(NodeSettingList, &[&thread])
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        let setting = rows.iter().find(|row| row.key == "approval.mode").unwrap();
        assert_eq!(
            serde_json::to_value(&setting.approval_choices).unwrap(),
            parameters["approval_choices"]
        );
        assert!(
            !setting
                .available_values
                .as_ref()
                .unwrap()
                .iter()
                .any(|mode| mode == "auto")
        );
    }
}

#[tokio::test]
async fn a_running_auto_check_and_why_the_last_one_did_not_pass_come_with_the_choice_in_the_chip_and_setting()
 {
    use orch_api_contract::settings::NodeSettingList;
    // The check of Auto's build runs for minutes on a build host: the choice carries the build as its check left it,
    // so that the person sees one under way and since when, and after it why it did not pass.
    let (s, thread) = auto_candidate(true, false).await;
    let began = serde_json::to_value(orch_types::Time::new(
        s.rig.world.now() - chrono::Duration::minutes(6),
    ))
    .unwrap();
    let refused = "internal error: native check unit: Docker responded with status code 400: invalid mount config for type \"bind\": bind source path does not exist: /opt/orch/bin/orch-node";
    for (status, error, cases, reason) in [
        ("checking", None, json!([]), "auto_not_proven"),
        ("untested", Some(refused), json!([]), "auto_not_proven"),
        (
            "failed",
            None,
            json!([{"name":"auto_review","feature":"approval_modes.auto","passed":false,
                "error":"The scripted reviewer did not answer","unsupported":null,"elapsed_ms":1}]),
            "auto_check_failed",
        ),
    ] {
        let check = json!({"at":began,"machine":"orch-node","missing":[],
            "off":if status == "failed" {json!(["approval_modes.auto"])} else {json!([])},
            "error":error,"stage":if status == "checking" {"conformance"} else {"complete"},"cases":cases});
        let mut conn = s.rig.world.conn().await;
        let key = "harness_channel:auto-choice";
        if status == "checking" {
            orch_db::queries::resources::management::put_channel_build(&mut conn, key, &json!({
                "kind":"codex","version":"1.0.0","os":"linux","arch":"x86_64",
                "url":"https://example.com/program","sha256":"c".repeat(64),"is_default":false,"source":"channel",
                "status":status,"check":check
            }),s.rig.world.now()).await.unwrap();
        } else {
            orch_db::queries::resources::management::set_harness_build_status(
                &mut conn,
                key,
                status,
                Some(&check),
                s.rig.world.now(),
            )
            .await
            .unwrap();
        }
        drop(conn);
        let parameters = serde_json::to_value(
            s.rig
                .world
                .app
                .call(ThreadParameterGet, &[&thread])
                .token(&s.ada.token)
                .send()
                .await
                .ok(),
        )
        .unwrap();
        let automatic = parameters["approval_choices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|choice| choice["mode"] == "auto")
            .unwrap();
        assert_eq!(automatic["reason"], reason, "{status}");
        let build = &automatic["build"];
        assert_eq!(build["status"], status);
        assert_eq!(build["sha256"], "c".repeat(64), "{status}");
        assert_eq!(
            build["check"]["at"], began,
            "{status}: since when, or when it ended"
        );
        assert_eq!(build["check"]["machine"], "orch-node", "{status}");
        assert_eq!(build["check"]["error"], json!(error), "{status}");
        assert_eq!(build["check"]["cases"], cases, "{status}");
        let rows = s
            .rig
            .world
            .app
            .call(NodeSettingList, &[&thread])
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        let setting = rows.iter().find(|row| row.key == "approval.mode").unwrap();
        assert_eq!(
            serde_json::to_value(&setting.approval_choices).unwrap(),
            parameters["approval_choices"],
            "{status}"
        );
    }
}

#[tokio::test]
async fn scope_setting_choices_use_reported_modes_and_never_offer_an_unproved_reviewer_or_rewrite_a_stored_mode()
 {
    use orch_api_contract::settings::{
        NodeSettingDelete, NodeSettingDeleteQuery, NodeSettingList, NodeSettingSet,
        NodeSettingSetBody, SettingRegistryGet,
    };
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let mut descriptor = rig::descriptor();
    // Activating Codex does not give this unproved fake program an Auto reviewer.
    descriptor["approval_modes"] = json!(["on_request", "auto"]);
    descriptor["sandbox_modes"] = json!(["read_only"]);
    {
        let mut conn = s.rig.db.acquire().await.unwrap();
        sqlx::query("UPDATE agents SET descriptor=$2,descriptor_overrides='{}' WHERE id=$1")
            .bind(s.on.agent)
            .bind(&descriptor)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "approval.mode",
            json!("auto"),
        )
        .await;
    for node in [
        s.tree.org.base.id,
        s.tree.project.base.id,
        s.tree.topic.base.id,
        thread,
    ] {
        let rows = s
            .rig
            .world
            .app
            .call(NodeSettingList, &[&node])
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        for (key, values) in [
            ("approval.mode", vec!["on_request"]),
            ("approval.allowed_modes", vec!["on_request"]),
            ("sandbox.mode", vec!["read_only"]),
        ] {
            let expected: Vec<String> = values.into_iter().map(str::to_owned).collect();
            let row = rows.iter().find(|row| row.key == key);
            // A place lists the keys valid at it or below it: the policy of allowed modes ends at the
            // topic, and a thread has none of its own.
            if key == "approval.allowed_modes" && node == thread {
                assert!(row.is_none(), "{key} at {node}");
                continue;
            }
            assert_eq!(
                row.unwrap().available_values.as_ref(),
                Some(&expected),
                "{key} at {node}"
            );
        }
        if node == s.tree.topic.base.id || node == thread {
            assert_eq!(
                rows.iter()
                    .find(|row| row.key == "approval.mode")
                    .unwrap()
                    .value,
                Some(json!("auto")),
                "reading choices must not repair a stored setting silently"
            );
        }
    }
    let defs = s
        .rig
        .world
        .app
        .call(SettingRegistryGet, &[])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let auto_possible = orch_agent_programs::Programs::builtin().auto_possible();
    for (key, pointer) in [("approval.mode", ""), ("approval.allowed_modes", "/items")] {
        let schema = &defs.iter().find(|def| def.key == key).unwrap().schema;
        let choice = schema.pointer(pointer).unwrap();
        assert_eq!(
            choice["enum"].as_array().unwrap().contains(&json!("auto")),
            auto_possible,
            "the offered enum follows declared program support"
        );
        if !auto_possible {
            assert_eq!(
                choice["x-unavailable-choices"],
                json!([{"value":"auto","reason":"program_auto_unavailable"}]),
                "the known unsupported choice remains visible without being offered"
            );
        }
    }
    let reply = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&thread, &"approval.mode"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            model_choice: None,
            value: json!("on_request"),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .full_setting()
        .ok();
    assert_eq!(reply.available_values, Some(vec!["on_request".to_owned()]));
    let reply = s
        .rig
        .world
        .app
        .call(NodeSettingDelete, &[&thread, &"approval.mode"])
        .token(&s.ada.token)
        .query(&NodeSettingDeleteQuery {
            expected_revision: None,
        })
        .send()
        .await
        .full_setting()
        .ok();
    assert_eq!(
        reply.value,
        Some(json!("auto")),
        "clearing restores inheritance without reinterpreting it"
    );
    assert_eq!(reply.available_values, Some(vec!["on_request".to_owned()]));
    assert_eq!(
        s.rig.commands.issued_of("turn.start"),
        0,
        "offering or editing settings creates no input or turn"
    );
}

#[tokio::test]
async fn a_setting_before_the_first_turn_stamps_and_announces_the_thread_row_without_inventing_a_line()
 {
    use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
    use orch_db::sequences::{self, Notice, THREAD_CHANNEL};
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let mut notices = sequences::listen(&s.rig.db, &[THREAD_CHANNEL])
        .await
        .unwrap();
    let mut conn = s.rig.db.acquire().await.unwrap();
    let before = sequences::current(&mut conn, id).await.unwrap().unwrap();
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(turns, 0);
    let items_before: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let written = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&id, &"agent.memory"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            model_choice: None,
            value: json!("off"),
            locked: None,
            expected_revision: Some(0),
        })
        .send()
        .await
        .full_setting()
        .ok();
    assert_eq!(written.value, Some(json!("off")));
    let mut conn = s.rig.db.acquire().await.unwrap();
    let after = sequences::current(&mut conn, id).await.unwrap().unwrap();
    let row = orch_db::queries::threads::thread(&mut conn, id)
        .await
        .unwrap()
        .unwrap();
    assert!(after > before);
    assert_eq!(
        row.seq_state, after,
        "the settings change is an ordinary row on the thread feed"
    );
    // A change of a thread's setting adds its `setting_changed` marker, and nothing that is said.
    let added: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT kind, payload->>'marker' FROM items WHERE thread_id=$1 ORDER BY seq_created OFFSET $2",
    )
    .bind(id)
    .bind(items_before)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        added,
        [("marker".to_owned(), Some("setting_changed".to_owned()))],
        "a setting before the first turn invents no conversation line"
    );
    drop(conn);
    let sync = s
        .rig
        .world
        .app
        .call(orch_api_contract::threads::ThreadSyncGet, &[&id])
        .token(&s.ada.token)
        .query(&orch_api_contract::threads::ThreadSyncGetQuery {
            since: Some(before),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        sync.thread.seq, after,
        "another open reader receives the changed thread row"
    );
    let notice = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        // Setup can still publish its own row. The action's announcement must arrive within
        // the same deadline, wherever it lies among those already in the listener's queue.
        loop {
            let notice = notices.next().await.unwrap();
            if matches!(notice, Notice::Heard { ref channel, ref payload } if channel == THREAD_CHANNEL && payload == &sequences::payload(id, after)) {
                break notice;
            }
        }
    })
        .await
        .unwrap();
    assert!(
        matches!(notice, Notice::Heard { ref channel, ref payload } if channel == THREAD_CHANNEL && payload == &sequences::payload(id, after)),
        "{notice:?}"
    );
    assert_eq!(
        s.rig.commands.issued_of("turn.start"),
        0,
        "changing a setting starts no turn"
    );
}

fn query(format: ExportFormat) -> ThreadExportQuery {
    ThreadExportQuery {
        continuation_id: None,
        projection_revision: None,
        format,
        detail: None::<ExportDetail>,
        from: None,
        to: None,
        include: None,
    }
}

#[tokio::test]
async fn the_parameters_show_eight_subagents_and_one_layer_by_default_and_the_smallest_inherited_limits()
 {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let read = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let limits = read.subagent_limits.unwrap();
    assert_eq!((limits.limits.at_once, limits.limits.depth), (8, 1));
    assert!(
        limits.supported.is_empty(),
        "the fake harness does not claim native controls, and the reader sees that they do not apply"
    );
    let rows = s
        .rig
        .world
        .app
        .call(orch_api_contract::settings::NodeSettingList, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    for key in ["agents.subagents_at_once", "agents.subagent_depth"] {
        assert_eq!(
            rows.iter()
                .find(|row| row.key == key)
                .unwrap()
                .not_applicable_reason
                .as_deref(),
            Some("harness_control_unavailable")
        );
    }
    // These choices already exist when the project and topic adopt tighter ceilings.
    s.rig
        .world
        .set_at(Target::Node(id), "agents.subagents_at_once", json!(64))
        .await;
    s.rig
        .world
        .set_at(Target::Node(id), "agents.subagent_depth", json!(5))
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.project.base.id),
            "agents.subagents_at_once",
            json!(4),
        )
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "agents.subagents_at_once",
            json!(2),
        )
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.project.base.id),
            "agents.subagent_depth",
            json!(3),
        )
        .await;
    let read = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let limits = read.subagent_limits.unwrap();
    assert_eq!((limits.limits.at_once, limits.limits.depth), (2, 3));
    assert_eq!(
        s.rig.commands.issued_of("turn.start"),
        0,
        "reading limits asks no model and starts no work"
    );
}

#[tokio::test]
async fn a_threads_approval_picker_offers_only_declared_allowed_modes_and_a_viewer_cannot_change_them()
 {
    use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let mut descriptor = rig::descriptor();
    descriptor["approval_modes"] = json!(["ask", "on_request", "auto", "bypass"]);
    sqlx::query("UPDATE agents SET descriptor=$2,descriptor_overrides='{}' WHERE id=$1")
        .bind(s.on.agent)
        .bind(&descriptor)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let declared: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(descriptor).unwrap();
    s.rig.machine.handler.on(
        orch_link_contract::commands::CommandKind::AgentStart,
        move |_| {
            Ok(orch_link_contract::commands::CommandResult::AgentStart(
                orch_link_contract::commands::AgentStartResult {
                    home: None,
                    reused: false,
                    harness_version: "1.0.0".into(),
                    descriptor: declared.clone(),
                },
            ))
        },
    );
    let read = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.approval_mode, "on_request");
    let choices = serde_json::to_value(&read).unwrap()["approval_choices"].clone();
    let auto = choices
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice["mode"] == "auto")
        .unwrap();
    assert_eq!(auto["available"], false);
    assert_eq!(auto["reason"], "auto_check_unavailable");
    let bypass = choices
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice["mode"] == "bypass")
        .unwrap();
    assert_eq!(bypass["available"], false);
    assert_eq!(bypass["reason"], "required_isolation_unavailable");
    let offered = read.approval_modes.unwrap();
    assert!(offered.iter().any(|mode| mode == "ask"));
    assert!(offered.iter().any(|mode| mode == "on_request"));
    assert!(
        !offered.iter().any(|mode| mode == "auto"),
        "an unproved reviewer is never offered by the picker"
    );
    assert!(
        !offered.iter().any(|mode| mode == "bypass"),
        "a managed label is not proof of driver confinement"
    );
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.project.base.id),
            "approval.allowed_modes",
            json!(["on_request"]),
        )
        .await;
    let viewer = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&id])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert_eq!(viewer.approval_modes, Some(vec!["on_request".to_owned()]));
    let choices = serde_json::to_value(&viewer).unwrap()["approval_choices"].clone();
    let ask = choices
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice["mode"] == "ask")
        .unwrap();
    assert_eq!(ask["available"], false);
    assert_eq!(ask["reason"], "not_allowed_here");
    let auto = choices
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice["mode"] == "auto")
        .unwrap();
    assert_eq!(
        auto["reason"], "not_allowed_here",
        "the chip names the same first refusal as native admission"
    );
    assert!(
        auto.get("check").is_none(),
        "a check cannot change this place's policy"
    );
    let denied = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&id, &"approval.mode"])
        .token(&s.cai.token)
        .body(&NodeSettingSetBody {
            model_choice: None,
            value: json!("on_request"),
            locked: None,
            expected_revision: Some(0),
        })
        .send()
        .await
        .full_setting()
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let still = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        still.approval_mode, "on_request",
        "reading and a refused write never choose a mode"
    );
    assert_eq!(s.rig.commands.issued_of("turn.start"), 0);
}

#[tokio::test]
async fn bypass_is_offered_only_after_the_current_driver_reports_isolation() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let host = orch_types::new_id();
    sqlx::query("INSERT INTO hosts (id,machine_id,name,drivers) VALUES ($1,$2,'Confined host',$3)")
        .bind(host)
        .bind(s.rig.machine_id)
        .bind(json!({"test":{"confinement":"isolated"}}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtimes (id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state)
         SELECT $1,'environment',$2,'isolated-picker',e.id,'test',e.template_id,$3,'running' FROM environments e WHERE e.id=$4")
        .bind(orch_types::new_id()).bind(host).bind(vec![0_u8;32]).bind(s.on.environment).execute(s.rig.db.pool()).await.unwrap();
    let read = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(
        read.approval_modes
            .unwrap()
            .iter()
            .any(|mode| mode == "bypass")
    );
    assert_eq!(
        read.approval_mode, "on_request",
        "offering Bypass never selects it by default"
    );
}

#[tokio::test]
async fn a_threads_parameters_show_its_model_with_the_models_its_provider_lists_and_who_may_change_them()
 {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    sqlx::query(
        "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
         VALUES ($1, $2, 'fake-2', 'Fake Two', '{}', false, 'manual', $3)",
    )
    .bind(orch_types::new_id())
    .bind(s.on.provider)
    .bind(s.ada.id())
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let read = |who: &places::Who| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadParameterGet, &[&id])
            .token(&who.token);
        async move { call.send().await }
    };
    let owner = read(&s.ada).await.ok();
    assert_eq!(owner.model.value.as_deref(), Some(MODEL));
    assert!(owner.model.writable, "the owner manages the thread");
    let names: Vec<_> = owner
        .model
        .options
        .iter()
        .map(|each| each.model_name.as_str())
        .collect();
    assert_eq!(names, [MODEL, "fake-2"]);
    assert!(owner.model.options[0].allowed);
    assert_eq!(owner.model.options[0].display_name, "Fake One");
    assert_eq!(
        owner.model.options[1].reason,
        Some(ModelOptionReason::Disabled)
    );
    assert_eq!(
        owner.model.options[0].parameters.efforts.as_deref(),
        Some(&["low".to_owned(), "high".to_owned()][..])
    );
    assert_eq!(
        owner.effort.value.as_deref(),
        Some("low"),
        "the model's own default"
    );
    assert!(
        !owner.effort.supported,
        "the tests' harness takes no effort"
    );
    // A viewer reads what it is set to and changes nothing.
    let viewer = read(&s.cai).await.ok();
    assert!(!viewer.model.writable);
    // A policy of the place leaves a model out.
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.org.base.id),
            "models.allowed",
            json!(["other-*"]),
        )
        .await;
    let policy = read(&s.ada).await.ok();
    assert_eq!(
        policy.model.options[0].reason,
        Some(ModelOptionReason::ExcludedByPolicy)
    );
    let refused = read(&s.bea).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["thread.view"]);
}

#[tokio::test]
async fn a_thread_is_exported_whole_as_json_and_its_chat_is_part_of_the_json_when_it_is_asked_for()
{
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "First question", "First answer").await;
    s.exchange(id, 1, "Second question", "Second answer").await;
    let app = &s.rig.world.app;
    let export = |who: &places::Who, query: ThreadExportQuery| {
        let call = app
            .call(ThreadExport, &[&id])
            .token(&who.token)
            .query(&query);
        async move { call.send_bytes().await }
    };
    let whole = export(&s.cai, query(ExportFormat::Json)).await;
    assert_eq!(whole.status, 200);
    assert!(
        whole
            .headers
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"Fix_login")
    );
    let json: ThreadExportJson = serde_json::from_slice(&whole.raw).unwrap();
    assert_eq!(json.thread.base.id, id);
    assert_eq!((json.segments.len(), json.turns.len()), (1, 2));
    assert_eq!(rig::said(&json.items).len(), 4);
    assert!(json.chat_messages.is_none());

    // A range of turns keeps those turns and what was made in them.
    let one = export(
        &s.cai,
        ThreadExportQuery {
            from: Some(2),
            to: Some(2),
            ..query(ExportFormat::Json)
        },
    )
    .await;
    let one: ThreadExportJson = serde_json::from_slice(&one.raw).unwrap();
    assert_eq!(one.turns.len(), 1);
    assert_eq!(
        rig::said(&one.items),
        ["user: Second question", "agent: Second answer"]
    );
    assert!(
        one.items.iter().any(|item| item.kind() == ItemKind::Marker),
        "the history stays"
    );

    // The reasoning of the agent is there only when asked for.
    sqlx::query(
        "INSERT INTO items (id, thread_id, segment_id, stream, kind, status, payload, seq_created, seq_updated)
         SELECT $1, $2, current_segment_id, 'main', 'reasoning', 'completed', '{\"summary\":\"thinking\",\"text_len\":8,\"hidden\":false}', 999, 999
           FROM threads WHERE node_id = $2",
    )
    .bind(orch_types::new_id())
    .bind(id)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let without: ThreadExportJson =
        serde_json::from_slice(&export(&s.cai, query(ExportFormat::Json)).await.raw).unwrap();
    assert!(
        without
            .items
            .iter()
            .all(|item| item.kind() != ItemKind::Reasoning)
    );
    let with: ThreadExportJson = serde_json::from_slice(
        &export(
            &s.cai,
            ThreadExportQuery {
                include: Some("sub_streams,reasoning".to_owned()),
                ..query(ExportFormat::Json)
            },
        )
        .await
        .raw,
    )
    .unwrap();
    assert!(
        with.items
            .iter()
            .any(|item| item.kind() == ItemKind::Reasoning)
    );

    // The written form is served as Markdown; the chat comes with the records when it is asked for.
    let written = export(&s.cai, query(ExportFormat::Markdown)).await;
    assert_eq!(written.status, 200);
    assert!(
        String::from_utf8(written.raw.to_vec())
            .unwrap()
            .contains("Second answer")
    );
    let chat = export(
        &s.cai,
        ThreadExportQuery {
            include: Some("chat".to_owned()),
            ..query(ExportFormat::Json)
        },
    )
    .await;
    assert_eq!(chat.status, 200);
    let with_chat: ThreadExportJson = serde_json::from_slice(&chat.raw).unwrap();
    assert_eq!(
        with_chat.chat_messages.map(|messages| messages.len()),
        Some(0),
        "the thread's chat holds no message yet, and says so"
    );
    let odd = export(
        &s.cai,
        ThreadExportQuery {
            include: Some("everything".to_owned()),
            ..query(ExportFormat::Json)
        },
    )
    .await;
    assert_eq!(odd.problem().code, ErrorCode::RequestInvalid);
    let stranger = export(&s.bea, query(ExportFormat::Json)).await;
    let problem = stranger.problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&problem),
        ["thread.export"],
        "taking a conversation away is its Export row"
    );
}

/// The parameters of an adapter that publishes a speed, an access program and a context, each reading the
/// entry of the thread's model: the three that the fake adapter declares.
fn declared() -> serde_json::Value {
    let scopes = json!(["org", "project", "topic", "thread"]);
    json!([
        {
            "key": "service_tier", "title": "Speed", "description": "A faster way of running a turn.",
            "group": "model", "type": "enum", "options": { "from_model": "service_tiers" },
            "default": { "from_model": "default_service_tier" }, "scopes": scopes, "apply": "next_turn",
            "eligible_when": { "model_has": "service_tiers" }
        },
        {
            "key": "access_program", "title": "Access program", "description": "A program.",
            "group": "safety", "type": "enum", "options": { "from_model": "access_programs" },
            "default": { "from_model": "default_access_program" }, "scopes": scopes, "apply": "next_turn",
            "eligible_when": { "model_has": "access_programs" }
        },
        {
            "key": "context", "title": "Context", "description": "How much of a conversation is kept.",
            "group": "model", "type": "integer",
            "range": { "min": 1000, "max": { "from_model": "context_max" } },
            "default": { "from_model": "default_context" }, "scopes": scopes, "apply": "new_session",
            "eligible_when": { "model_has": "context_max" }
        },
        {
            "key": "verbosity", "title": "Verbosity", "description": "How much it says.", "group": "behaviour",
            "type": "enum", "options": [{ "value": "terse", "label": "Terse" }, { "value": "chatty", "label": "Chatty" }],
            "default": "terse", "scopes": ["thread"], "apply": "next_turn"
        }
    ])
}

#[tokio::test]
async fn a_threads_parameters_list_only_what_exists_for_its_model_and_carry_the_range_the_models_managers_opened()
 {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(descriptor, '{parameters}', $1)")
        .bind(declared())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let entry = |parameters: serde_json::Value| {
        let pool = s.rig.db.pool();
        async move {
            sqlx::query("UPDATE provider_models SET parameters = $1 WHERE model_name = $2")
                .bind(parameters)
                .bind(MODEL)
                .execute(pool)
                .await
                .unwrap();
        }
    };
    let keys = || async {
        s.rig
            .world
            .app
            .call(ThreadParameterGet, &[&id])
            .token(&s.ada.token)
            .send()
            .await
            .ok()
            .parameters
    };

    // A model that offers a speed, a program and a larger context but has none of them open shows none.
    entry(json!({
        "offered": { "service_tiers": [{ "id": "fast", "label": "Fast" }], "access_programs": ["blue"],
                     "context_max": 1_000_000 },
        "context_window": 272_000
    }))
    .await;
    let shown = keys().await;
    assert_eq!(
        shown
            .iter()
            .map(|view| view.key.as_str())
            .collect::<Vec<_>>(),
        ["verbosity"],
        "what is only on offer is not there"
    );

    // Opened by the managers, each is there with what the browser draws it from.
    entry(json!({
        "service_tiers": [{ "id": "fast", "label": "Fast" }], "default_service_tier": "fast",
        "context_window": 272_000, "context_max": 1_000_000, "default_context": 400_000
    }))
    .await;
    s.rig
        .world
        .set_at(
            Target::Node(id),
            "agent.params",
            json!({ "context": 600_000, "verbosity": "chatty" }),
        )
        .await;
    let shown = keys().await;
    let names: Vec<_> = shown.iter().map(|view| view.key.as_str()).collect();
    assert_eq!(names, ["service_tier", "context", "verbosity"]);
    let context = &shown[1];
    assert_eq!(
        context.def["range"],
        json!({ "min": 1000, "max": { "from_model": "context_max" } })
    );
    assert_eq!(
        context.def["default"],
        json!({ "from_model": "default_context" })
    );
    assert_eq!(
        context.value,
        Some(json!(600_000)),
        "what was chosen for the thread"
    );
    assert_eq!(
        shown[0].value, None,
        "nothing was chosen: the model's start applies"
    );
    assert_eq!(
        context.apply,
        orch_api_contract::settings::ParameterApply::NewSession
    );
    assert!(context.writable);
    // The model's entry says what the thread starts with, beside the thread's own choice.
    let options = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
        .model
        .options;
    let entry = options
        .iter()
        .find(|option| option.model_name == MODEL)
        .unwrap();
    assert_eq!(entry.parameters.default_context, Some(400_000));
    assert_eq!(
        entry.parameters.default_service_tier.as_deref(),
        Some("fast")
    );
}

#[tokio::test]
async fn an_access_program_a_manager_enabled_for_the_model_is_offered_in_a_thread_at_shipped_settings()
 {
    use orch_api_contract::settings::WithheldChoice;
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    // The adapter puts its access program behind its own feature, as Codex does.
    let mut parameters = declared();
    parameters[1]["feature"] = json!("codex.cyber_access");
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(jsonb_set(descriptor, '{parameters}', $1), '{features}', $2)")
        .bind(parameters)
        .bind(json!({"codex.cyber_access":true}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE provider_models SET parameters = $1 WHERE model_name = $2")
        .bind(json!({ "access_programs": ["blue"], "default_access_program": "blue" }))
        .bind(MODEL)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let shown = || async {
        s.rig
            .world
            .app
            .call(ThreadParameterGet, &[&id])
            .token(&s.ada.token)
            .send()
            .await
            .ok()
            .parameters
            .into_iter()
            .map(|view| view.key)
            .collect::<Vec<_>>()
    };
    assert!(
        shown().await.contains(&"access_program".to_owned()),
        "nobody switched the feature on, and the program is there"
    );
    let choices = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(
        choices
            .withheld
            .iter()
            .all(|entry| entry.choice != WithheldChoice::Access),
        "the shipped-on program cannot also be explained as switched off"
    );
    // A place that does not want it offered under it says so.
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.org.base.id),
            "agent.features",
            json!({ "codex.cyber_access": false }),
        )
        .await;
    assert!(
        !shown().await.contains(&"access_program".to_owned()),
        "switched off under the org, it is not offered"
    );
}

#[tokio::test]
async fn a_threads_parameters_name_each_choice_the_models_managers_opened_that_it_cannot_use_and_why()
 {
    use orch_api_contract::settings::{WithheldChoice, WithheldParameter, WithheldReason};

    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    // An agent that publishes no speed, and whose access program waits for a feature of agents.
    let mut defs = declared();
    let list = defs.as_array_mut().unwrap();
    list.retain(|def| def["key"] != "service_tier");
    for def in list.iter_mut().filter(|def| def["key"] == "access_program") {
        def["feature"] = json!("codex.cyber_access");
    }
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(descriptor, '{parameters}', $1)")
        .bind(defs)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let entry = |parameters: serde_json::Value| {
        let pool = s.rig.db.pool();
        async move {
            sqlx::query("UPDATE provider_models SET parameters = $1 WHERE model_name = $2")
                .bind(parameters)
                .bind(MODEL)
                .execute(pool)
                .await
                .unwrap();
        }
    };
    let read = || async {
        s.rig
            .world
            .app
            .call(ThreadParameterGet, &[&id])
            .token(&s.ada.token)
            .send()
            .await
            .ok()
    };
    let keys = |read: &orch_api_contract::settings::ThreadParameters| {
        read.parameters
            .iter()
            .map(|view| view.key.clone())
            .collect::<Vec<_>>()
    };

    // A speed, a program and a larger context are open on the model.
    entry(json!({
        "service_tiers": [{ "id": "fast", "label": "Fast" }], "access_programs": ["blue"],
        "context_window": 272_000, "context_max": 1_000_000
    }))
    .await;
    // The feature ships on; here a manager explicitly withholds it.
    s.rig
        .world
        .set_at(
            Target::System,
            "agent.features",
            json!({ "codex.cyber_access": false }),
        )
        .await;
    let first = read().await;
    assert_eq!(
        keys(&first),
        ["context", "verbosity"],
        "the program is behind a feature that is off, and the agent has no speed"
    );
    assert_eq!(
        first.withheld,
        [
            WithheldParameter {
                choice: WithheldChoice::Speed,
                key: None,
                reason: WithheldReason::Agent,
                requirement: Some(orch_api_contract::settings::ParameterRequirement::Agent),
                inapplicable_selected_value: None,
            },
            WithheldParameter {
                choice: WithheldChoice::Access,
                key: Some("access_program".to_owned()),
                reason: WithheldReason::Feature {
                    feature: "codex.cyber_access".to_owned()
                },
                requirement: Some(orch_api_contract::settings::ParameterRequirement::Feature),
                inapplicable_selected_value: None,
            },
        ]
    );

    // Switching the feature on shows the program; the speed is still the agent's to take.
    s.rig
        .world
        .set_at(
            Target::System,
            "agent.features",
            json!({ "codex.cyber_access": true }),
        )
        .await;
    let second = read().await;
    assert_eq!(keys(&second), ["access_program", "context", "verbosity"]);
    assert_eq!(
        second
            .withheld
            .iter()
            .map(|one| one.choice)
            .collect::<Vec<_>>(),
        [WithheldChoice::Speed]
    );

    // A speed and a program that are only on offer are nobody's to ask for: nothing is listed.
    entry(json!({
        "offered": { "service_tiers": [{ "id": "fast", "label": "Fast" }], "access_programs": ["blue"] },
        "context_window": 272_000
    }))
    .await;
    assert!(read().await.withheld.is_empty());
}

#[path = "threads_settings/prompt_cache_ttl.rs"]
mod prompt_cache_ttl;

#[path = "threads_settings/provider_options.rs"]
mod provider_options;

#[tokio::test]
async fn a_place_can_choose_larger_subagent_defaults_without_claiming_unsupported_native_controls()
{
    use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    for (key, count) in [
        ("agents.subagents_at_once", 65),
        ("agents.subagent_depth", 6),
    ] {
        let accepted = s
            .rig
            .world
            .app
            .call(NodeSettingSet, &[&id, &key])
            .token(&s.ada.token)
            .body(&NodeSettingSetBody {
                model_choice: None,
                value: json!(count),
                locked: None,
                expected_revision: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap();
        assert_eq!(accepted.value, Some(json!(count)));
    }
    let read = s
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let limits = read.subagent_limits.unwrap();
    assert_eq!((limits.limits.at_once, limits.limits.depth), (65, 6));
    assert!(limits.supported.is_empty());
    assert_eq!(s.rig.commands.issued_of("turn.start"), 0);
}

#[tokio::test]
async fn a_model_choice_refuses_every_setting_when_either_related_revision_is_stale() {
    use orch_api_contract::settings::{
        ModelChoiceSetting, ModelChoiceSettings, NodeSettingList, NodeSettingSet,
        NodeSettingSetBody,
    };
    for conflicting in ["agent.effort", "agent.params"] {
        let s = Scene::without_supervisor().await;
        let thread = s.thread().await.base.id;
        sqlx::query("INSERT INTO provider_models (id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES ($1,$2,'atomic-next','Next',$3,true,'manual',$4)")
            .bind(orch_types::new_id()).bind(s.on.provider).bind(json!({"efforts":["low","high"]})).bind(s.ada.id())
            .execute(s.rig.db.pool()).await.unwrap();
        s.rig
            .world
            .set_at(Target::Node(thread), "agent.effort", json!("high"))
            .await;
        s.rig
            .world
            .set_at(
                Target::Node(thread),
                "agent.params",
                json!({"verbosity":"terse","service_tier":"fast"}),
            )
            .await;
        let seen = s
            .rig
            .world
            .app
            .call(NodeSettingList, &[&thread])
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        let revision = |key: &str| {
            seen.iter()
                .find(|row| row.key == key)
                .unwrap()
                .own
                .as_ref()
                .map_or(0, |own| own.revision)
        };
        let change = NodeSettingSetBody {
            value: json!("atomic-next"),
            locked: None,
            expected_revision: Some(revision("agent.model")),
            model_choice: Some(ModelChoiceSettings {
                effort: Some(ModelChoiceSetting {
                    value: json!("low"),
                    expected_revision: revision("agent.effort"),
                }),
                params: Some(ModelChoiceSetting {
                    value: json!({"verbosity":"terse"}),
                    expected_revision: revision("agent.params"),
                }),
            }),
        };
        s.rig
            .world
            .set_at(
                Target::Node(thread),
                conflicting,
                if conflicting == "agent.effort" {
                    json!("low")
                } else {
                    json!({"verbosity":"chatty","service_tier":"fast"})
                },
            )
            .await;
        let before = s
            .rig
            .world
            .app
            .call(NodeSettingList, &[&thread])
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        let refused = s
            .rig
            .world
            .app
            .call(NodeSettingSet, &[&thread, &"agent.model"])
            .token(&s.ada.token)
            .body(&change)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
        let after = s
            .rig
            .world
            .app
            .call(NodeSettingList, &[&thread])
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        for key in ["agent.model", "agent.effort", "agent.params"] {
            assert_eq!(
                after.iter().find(|row| row.key == key).unwrap().own,
                before.iter().find(|row| row.key == key).unwrap().own,
                "a refused {conflicting} component does not commit {key}"
            );
        }
    }
}

#[tokio::test]
async fn a_concurrent_input_waits_for_the_whole_reviewed_model_choice_before_admission() {
    use orch_api_contract::settings::{
        ModelChoiceSetting, ModelChoiceSettings, NodeSettingList, NodeSettingSet,
        NodeSettingSetBody,
    };
    use orch_api_contract::threads::{
        ContentPart, Delivery, ThreadInputCreate, ThreadInputCreateBody,
    };
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    sqlx::query("INSERT INTO provider_models (id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES ($1,$2,'atomic-next','Next',$3,true,'manual',$4)")
        .bind(orch_types::new_id()).bind(s.on.provider).bind(json!({"efforts":["low","high"]})).bind(s.ada.id())
        .execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .world
        .set_at(Target::Node(thread), "agent.effort", json!("high"))
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(thread),
            "agent.params",
            json!({"service_tier":"fast"}),
        )
        .await;
    let seen = s
        .rig
        .world
        .app
        .call(NodeSettingList, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let revision = |key: &str| {
        seen.iter()
            .find(|row| row.key == key)
            .unwrap()
            .own
            .as_ref()
            .map_or(0, |own| own.revision)
    };
    let change = NodeSettingSetBody {
        value: json!("atomic-next"),
        locked: None,
        expected_revision: Some(revision("agent.model")),
        model_choice: Some(ModelChoiceSettings {
            effort: Some(ModelChoiceSetting {
                value: json!("low"),
                expected_revision: revision("agent.effort"),
            }),
            params: Some(ModelChoiceSetting {
                value: json!({}),
                expected_revision: revision("agent.params"),
            }),
        }),
    };
    let mut held = s.rig.db.pool().begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    sqlx::query("SELECT node_id FROM settings WHERE node_id=$1 AND key='agent.effort' FOR UPDATE")
        .bind(thread)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let app = s.rig.world.app.clone();
    let token = s.ada.token.clone();
    let setting = tokio::spawn(async move {
        let call = app
            .call(NodeSettingSet, &[&thread, &"agent.model"])
            .token(&token)
            .body(&change);
        call.send().await
    });
    rig::eventually("the model choice reaches the related setting lock", || async {
        assert!(!setting.is_finished());
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM settings%' AND query LIKE '%FOR UPDATE%')")
            .bind(blocker).fetch_one(s.rig.db.pool()).await.unwrap()
    }).await;
    let app = s.rig.world.app.clone();
    let token = s.ada.token.clone();
    let input = tokio::spawn(async move {
        let call =
            app.call(ThreadInputCreate, &[&thread])
                .token(&token)
                .body(&ThreadInputCreateBody {
                    plan: false,
                    content: vec![ContentPart::Text {
                        text: "Use the reviewed model".into(),
                    }],
                    delivery: Delivery::Queue,
                    client_id: "atomic-model-admission".into(),
                    bypass: None,
                    native_action: None,
                });
        call.send().await
    });
    assert!(s.rig.turn_starts(thread).is_empty());
    held.rollback().await.unwrap();
    setting.await.unwrap().ok();
    input.await.unwrap().ok();
    let turn = s.rig.run_turn(thread, 0, "Done").await;
    assert_eq!(turn.model, "atomic-next");
    assert_eq!(turn.effort.as_deref(), Some("low"));
    assert!(!turn.params.contains_key("service_tier"));
}

#[tokio::test]
async fn the_tool_details_read_only_the_accepted_session_and_never_refresh_it() {
    use orch_api_contract::settings::{OrchToolsQuery, ThreadOrchToolsGet};
    let s = Scene::new().await;
    s.rig.tools_of_agents(json!("mcp")).await;
    let thread = s.thread().await.base.id;
    let before = s
        .rig
        .world
        .app
        .call(ThreadOrchToolsGet, &[&thread])
        .token(&s.ada.token)
        .query(&OrchToolsQuery::default())
        .send()
        .await
        .ok();
    assert!(before.summary.is_none());
    s.say(
        &s.ada,
        thread,
        "Read the offered tools",
        orch_api_contract::threads::Delivery::Queue,
        "tool-details",
    )
    .await
    .ok();
    s.rig.run_turn(thread, 0, "Done").await;
    let first = s
        .rig
        .world
        .app
        .call(ThreadOrchToolsGet, &[&thread])
        .token(&s.ada.token)
        .query(&OrchToolsQuery::default())
        .send()
        .await
        .ok();
    let summary = first
        .summary
        .clone()
        .expect("the native session accepted a tool offer");
    assert!(!first.tools.items.is_empty());
    s.rig
        .world
        .set_at(Target::Node(thread), "orch_tools.enabled", json!([]))
        .await;
    let next = s
        .rig
        .world
        .app
        .call(ThreadOrchToolsGet, &[&thread])
        .token(&s.ada.token)
        .query(&OrchToolsQuery {
            session_revision: Some(summary.session_revision.clone()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        first, next,
        "reading facts never proposes or applies a new tool offer"
    );
    let viewer = s
        .rig
        .world
        .app
        .call(ThreadOrchToolsGet, &[&thread])
        .token(&s.cai.token)
        .query(&OrchToolsQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(viewer.summary, next.summary);
    assert!(
        viewer.tools.items.iter().all(|tool| tool.source.is_none()),
        "thread View reads the catalogue without acquiring private setting sources"
    );
    let denied = s
        .rig
        .world
        .app
        .call(ThreadOrchToolsGet, &[&thread])
        .token(&s.bea.token)
        .query(&OrchToolsQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
}
