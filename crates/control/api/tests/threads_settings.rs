//! What a thread's agent is set to and could be set to, and a thread exported whole as JSON: each read by whoever may read the thread, and refused to a stranger.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

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
async fn scope_setting_choices_use_reported_modes_and_never_offer_an_unproved_reviewer_or_rewrite_a_stored_mode()
 {
    use orch_api_contract::settings::{
        NodeSettingDelete, NodeSettingDeleteQuery, NodeSettingList, NodeSettingSet,
        NodeSettingSetBody, SettingRegistryGet,
    };
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let mut descriptor = rig::descriptor();
    // Even an obsolete report that still claims Auto cannot bypass the pinned-harness trial.
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
    assert!(
        !defs
            .iter()
            .find(|def| def.key == "approval.mode")
            .unwrap()
            .schema["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("auto"))
    );
    assert!(
        !defs
            .iter()
            .find(|def| def.key == "approval.allowed_modes")
            .unwrap()
            .schema["items"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("auto"))
    );
    let reply = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&thread, &"approval.mode"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!("on_request"),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
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
            value: json!("off"),
            locked: None,
            expected_revision: Some(0),
        })
        .send()
        .await
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
        .set_at(Target::Node(id), "agents.subagents_at_once", json!(64))
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.project.base.id),
            "agents.subagent_depth",
            json!(3),
        )
        .await;
    s.rig
        .world
        .set_at(Target::Node(id), "agents.subagent_depth", json!(5))
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
    let denied = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&id, &"approval.mode"])
        .token(&s.cai.token)
        .body(&NodeSettingSetBody {
            value: json!("on_request"),
            locked: None,
            expected_revision: Some(0),
        })
        .send()
        .await
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
