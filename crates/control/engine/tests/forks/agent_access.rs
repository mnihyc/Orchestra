//! Fork preview freezes creation choices while switches keep existing contributions.

use super::*;
use orch_api_contract::nodes::{AgentAccessChoice, AgentAccessWhere};
use orch_api_contract::share::{ShareDocumentsLevel, ShareParts};
use orch_db::audit::Origin;
use orch_settings_contract::registry::keys;

fn preset() -> AgentAccessChoice {
    AgentAccessChoice {
        where_: AgentAccessWhere::Topic,
        parts: ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..ShareParts::default()
        },
    }
}

fn fresh(rig: &Rig) -> ForkRequest {
    ForkRequest {
        history: Some(ForkHistory {
            mode: HistoryMode::Nothing,
            from_turn: None,
            to_turn: None,
        }),
        access: Some(orch_api_contract::access::ThreadAccess::Preset(
            orch_api_contract::access::AccessPreset::Defaults,
        )),
        ..rig.fork()
    }
}

async fn existing_gifts(rig: &Rig, holder: Id) -> Json {
    rig.world.scalar("SELECT COALESCE(jsonb_agg(jsonb_build_array(id,object_id,page,permission,created_by,revision) ORDER BY id),'[]'::jsonb) FROM permission_entries WHERE holder_kind='agents' AND holder_id=$1 AND removed_at IS NULL", holder).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fork_confirmation_keeps_the_preview_preset_and_explicit_origin_while_switching_keeps_existing_gifts()
 {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    rig.world
        .set(rig.world.topic, keys::THREADS_AGENT_ACCESS, json!(preset()))
        .await;
    let source = rig.world.thread("The source keeps its contribution").await;
    let source_gifts = existing_gifts(&rig, source).await;
    assert_ne!(source_gifts, json!([]));
    for explicit in [false, true] {
        rig.world
            .set(rig.world.topic, keys::THREADS_AGENT_ACCESS, json!(preset()))
            .await;
        let request = ForkRequest {
            agent_access: explicit.then(preset),
            ..fresh(&rig)
        };
        let plan = rig.plan(source, &request).await;
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        // The current parent now says Nothing. Confirmation must use the shown value,
        // preserving whether it was accepted as a preset or supplied exactly by a person.
        rig.world
            .set(
                rig.world.topic,
                keys::THREADS_AGENT_ACCESS,
                json!({"where":"nothing", "parts":{}}),
            )
            .await;
        let job = rig.confirm(source, &plan).await;
        assert_eq!(
            job.plan["agent_access"]["kind"],
            json!(if explicit { "exact" } else { "frozen_preset" })
        );
        assert_eq!(job.plan["agent_access"]["choice"], json!(preset()));
        let thread = rig.new_thread_of(&job);
        rig.wait_job(job.id, "succeeded").await;
        let shared = rig
            .world
            .engine()
            .shares_of_thread(&rig.caller(), &Origin::default(), thread)
            .await
            .unwrap()
            .shares;
        assert_eq!(shared.len(), 1);
        assert_eq!(
            shared[0].subject,
            orch_engine::sharing::Subject::Node(rig.world.topic)
        );
        assert_eq!(shared[0].to, thread);
        assert_eq!(shared[0].by, Some(rig.world.ada));
        assert_eq!(shared[0].parts, preset().parts);
        assert!(
            shared[0]
                .grants
                .iter()
                .all(|row| row.permission == "doc.view")
        );
        assert_eq!(
            rig.world
                .count(
                    "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                    thread
                )
                .await,
            0,
            "a creation gift starts no instruction"
        );
    }
    rig.world
        .set(
            rig.world.topic,
            keys::THREADS_AGENT_ACCESS,
            json!({"where":"project", "parts":{"documents":"manage"}}),
        )
        .await;
    let switch = ForkRequest {
        history: Some(ForkHistory {
            mode: HistoryMode::Nothing,
            from_turn: None,
            to_turn: None,
        }),
        ..rig.to_other()
    };
    let plan = rig.plan(source, &switch).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let job = rig.confirm(source, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    assert_eq!(
        existing_gifts(&rig, source).await,
        source_gifts,
        "a switch is not another creation choice"
    );
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_cannot_apply_a_new_creation_choice() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let source = rig.world.thread("A switch retains access").await;
    let before = existing_gifts(&rig, source).await;
    let result = rig
        .world
        .engine()
        .draft_fork(
            &rig.caller(),
            source,
            &ForkRequest {
                agent_access: Some(preset()),
                ..rig.to_other()
            },
        )
        .await;
    assert!(
        matches!(result, Err(PlanError::Invalid { ref path, .. }) if path == "agent_access"),
        "{result:?}"
    );
    assert_eq!(existing_gifts(&rig, source).await, before);
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forks_explicit_nongiveable_source_is_refused_at_preview_before_any_job() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let source = rig.world.thread("A readable fork source").await;
    let developer = rig.world.person("developer").await;
    rig.world.give(developer, "developer").await;
    let caller = Caller::session(developer);
    let ordinary = rig
        .world
        .engine()
        .draft_fork(&caller, source, &fresh(&rig))
        .await
        .unwrap();
    assert!(ordinary.is_clear(), "{:?}", ordinary.plan.blockers);
    let mut conn = rig.world.pool.acquire().await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        caller.clone(),
        orch_types::Time::new(rig.world.clock.now()),
    )
    .await
    .unwrap();
    assert!(
        !orch_authz::nodes::check(
            &mut conn,
            &principal,
            rig.world.project,
            orch_access_contract::permissions::Permission::DocManage,
            orch_authz::Act::Use,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);
    let result = rig
        .world
        .engine()
        .draft_fork(
            &caller,
            source,
            &ForkRequest {
                agent_access: Some(AgentAccessChoice {
                    where_: AgentAccessWhere::Project,
                    parts: ShareParts {
                        documents: Some(ShareDocumentsLevel::Manage),
                        ..ShareParts::default()
                    },
                }),
                ..fresh(&rig)
            },
        )
        .await;
    assert!(
        matches!(result, Err(PlanError::AgentAccess(_))),
        "{result:?}"
    );
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        jobs, 0,
        "a known source refusal cannot be deferred until a copying job"
    );
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_read_only_side_lane_cannot_turn_creation_or_explicit_gifts_into_writes() {
    side_lane_keeps_its_reading_boundary(orch_link_contract::commands::SideReading::ReadOnly).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_approval_gated_side_lane_cannot_turn_gifts_into_actions() {
    side_lane_keeps_its_reading_boundary(orch_link_contract::commands::SideReading::ApprovalGated)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_no_tools_side_lane_refuses_direct_reads_and_actions() {
    side_lane_keeps_its_reading_boundary(orch_link_contract::commands::SideReading::NoTools).await;
}

async fn side_lane_keeps_its_reading_boundary(mode: orch_link_contract::commands::SideReading) {
    use crate::shared::Places;
    use orch_api_contract::share::{
        ShareTo, ShareToThread, ShareWhat, ShareWhatDocument, ShareWhatNode, ShareWithAgents,
    };
    use orch_api_contract::threads::{ContentPart, ShareMode};
    use orch_authz::reach::{Agent, AgentReach, Reach, Things};
    let can = Can {
        import_neutral: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    // The producer chooses this declared mechanism; native_tools::enable then reports the same
    // descriptor at agent.start. This is a server boundary fixture, not native conformance proof.
    sqlx::query(
        "UPDATE agents SET descriptor=jsonb_set(descriptor,'{side_thread_modes}',$2) WHERE id=$1",
    )
    .bind(rig.world.agent)
    .bind(json!([mode]))
    .execute(rig.world.pool.pool())
    .await
    .unwrap();
    shared::the_agent_takes_tools_again(&rig.world).await;
    rig.world
        .set(
            rig.world.topic,
            keys::THREADS_AGENT_ACCESS,
            json!({"where":"topic","parts":{"documents":"edit"}}),
        )
        .await;
    let parent = rig.thread_with_history("The side's parent").await;
    let recipient = rig.world.thread("A separately controlled thread").await;
    let doc = rig
        .world
        .document_beside(
            rig.world.topic,
            "Read only from the aside",
            &["Original material"],
        )
        .await;
    for (what, parts, client_id) in [
        (
            ShareWhat::Document(ShareWhatDocument { document_id: doc }),
            ShareParts {
                documents: Some(ShareDocumentsLevel::Manage),
                ..ShareParts::default()
            },
            "side-document",
        ),
        (
            ShareWhat::Node(ShareWhatNode { node_id: recipient }),
            ShareParts {
                timeline: Some(orch_api_contract::share::ShareLevel::Write),
                ..ShareParts::default()
            },
            "side-control",
        ),
    ] {
        rig.world
            .engine()
            .share_with_agents(
                &rig.caller(),
                &Origin::default(),
                &ShareWithAgents {
                    link_origin: None,
                    viewers_read: None,
                    what,
                    parts: Some(parts),
                    mode: ShareMode::Live,
                    to: ShareTo::Threads(vec![ShareToThread { thread_id: parent }]),
                    inside: None,
                    read_first: None,
                    download: Some(false),
                    comment: None,
                    expires_at: None,
                    client_id: client_id.into(),
                },
            )
            .await
            .unwrap();
    }
    let side = rig
        .world
        .engine()
        .start_side_thread(
            &rig.caller(),
            parent,
            vec![ContentPart::Text {
                text: "Read this material without acting".into(),
            }],
            "readonly-gifts".into(),
            None,
        )
        .await
        .unwrap()
        .thread;
    let reading: String = rig
        .world
        .scalar("SELECT side_reading FROM threads WHERE node_id=$1", side)
        .await;
    assert_eq!(
        json!(reading),
        json!(mode),
        "the actual side producer selected the harness's declared verified mode"
    );
    let start = rig.world.turn_start(side, 0).await;
    let open = rig
        .opens()
        .into_iter()
        .find(|open| open.thread_id == side)
        .expect("the side thread opened a session before starting its turn");
    assert_eq!(
        open.side_reading,
        Some(mode),
        "the actual session handoff carries the side-specific reading mode"
    );
    rig.world
        .answer(side, &start, "readonly-active", "Reading the material")
        .await;
    rig.world.wait_turn(start.turn_id, "running").await;
    {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        let agent = Agent::of(&mut conn, side).await.unwrap().unwrap();
        let document = orch_db::queries::documents::get(&mut conn, doc)
            .await
            .unwrap()
            .unwrap();
        let document = orch_authz::documents::Document::of_row(&document);
        let reach = Reach::load(
            &mut conn,
            agent,
            AgentReach::None,
            orch_types::Time::new(rig.world.clock.now()),
            &Things {
                nodes: vec![rig.world.topic, recipient],
                documents: vec![document.clone()],
                people_chats: Vec::new(),
            },
        )
        .await
        .unwrap();
        assert!(
            reach.documents_of(rig.world.topic).writes(),
            "the creation contribution is genuinely present"
        );
        assert!(
            reach.document(&document).manages(),
            "the direct document contribution is genuinely present"
        );
        assert!(
            reach.conversation(recipient, false).writes(),
            "the separate control contribution is genuinely present"
        );
    }
    let read = shared::call(
        &rig.world,
        side,
        orch_link_contract::commands::OrchTool::ReadShared,
        json!({"path":format!("id:{doc}")}),
    )
    .await;
    if mode == orch_link_contract::commands::SideReading::NoTools {
        assert!(
            open.orch_tools.is_empty(),
            "no-tools must advertise no Orch tool"
        );
        let (code, message) = shared::refused(&read);
        assert_eq!(code, "invalid");
        assert!(message.contains("not a tool you have here"), "{message}");
    } else {
        assert!(
            open.orch_tools
                .contains(&orch_link_contract::commands::OrchTool::ReadShared)
        );
        let read = shared::ok(read);
        assert!(read["text"].as_str().unwrap().contains("Original material"));
    }
    for tool in [
        orch_link_contract::commands::OrchTool::EditDocument,
        orch_link_contract::commands::OrchTool::SendMessage,
    ] {
        assert!(
            !open.orch_tools.contains(&tool),
            "a side must not advertise {tool:?}"
        );
    }
    let before = rig.world.blocks_of(doc).await;
    let seq: i64 = rig
        .world
        .scalar("SELECT doc_seq FROM documents WHERE id=$1", doc)
        .await;
    let edit_arguments = json!({
        "command":"rewrite", "path":format!("id:{doc}"),
        "markdown":"The ordinary thread can write this", "seq":seq
    });
    let edit = shared::call(
        &rig.world,
        side,
        orch_link_contract::commands::OrchTool::EditDocument,
        edit_arguments.clone(),
    )
    .await;
    assert!(
        edit.error.is_some(),
        "a real read-only side lane cannot write through inherited gifts: {edit:?}"
    );
    let (code, why) = shared::refused(&edit);
    assert_eq!(code, "invalid");
    assert!(why.contains("not a tool you have here"), "{why}");
    assert_eq!(rig.world.blocks_of(doc).await, before);
    let message = shared::call(&rig.world, side, orch_link_contract::commands::OrchTool::SendMessage,
        json!({"target":{"kind":"thread","path":format!("id:{recipient}")},"text":"Must not control another thread"})).await;
    assert!(
        message.error.is_some(),
        "a read-only side lane cannot control a separately shared thread: {message:?}"
    );
    let (code, why) = shared::refused(&message);
    assert_eq!(code, "invalid");
    assert!(why.contains("not a tool you have here"), "{why}");
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                side
            )
            .await,
        0
    );
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                recipient
            )
            .await,
        0
    );
    // Same valid request and same existing giver contributions: the ordinary parent can act.
    // This uses the already-created parent and proves a side refusal is not a schema/ACL failure.
    shared::ok(
        shared::call(
            &rig.world,
            parent,
            orch_link_contract::commands::OrchTool::EditDocument,
            edit_arguments,
        )
        .await,
    );
    assert_ne!(rig.world.blocks_of(doc).await, before);
    rig.world.shutdown().await;
}
