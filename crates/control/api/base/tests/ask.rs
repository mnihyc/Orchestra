//! Personal Ask runs without thread-creation authority or an environment.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding case fails by panicking"
)]
#[path = "ask/context.rs"]
mod context;
mod places;
mod rig;
mod support;

use orch_api_contract::{
    ask::*,
    settings::{NodeSettingSet, NodeSettingSetBody},
    threads::ContentPart,
};
use orch_link_contract::commands::{
    Command, CommandKind, CommandResult, Empty, ProviderGenerateResult,
};
use orch_model_contract::generation::{
    GenerationMessage, GenerationOutput, GenerationPart, GenerationRole, GenerationStop,
};
use orch_types::{Id, Json, new_id};
use rig::{MODEL, Scene, eventually};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tracing::instrument::WithSubscriber;

async fn setting(s: &Scene, key: &str, value: Json) {
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&s.tree.org.base.id, &key])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value,
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}
async fn create(s: &Scene, who: &places::Who, days: Option<u32>) -> AskReceipt {
    s.rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&who.token)
        .body(&AskCreateBody {
            root_org_id: Some(s.tree.org.base.id),
            context: Some(format!("/topic/{}", s.tree.topic.base.id)),
            model: None,
            retention_days: days,
        })
        .send()
        .with_subscriber(
            tracing_subscriber::fmt()
                .with_test_writer()
                .with_ansi(false)
                .with_max_level(tracing::Level::ERROR)
                .finish(),
        )
        .await
        .ok()
}
async fn create_unseeded(s: &Scene, who: &places::Who) -> AskReceipt {
    s.rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&who.token)
        .body(&AskCreateBody {
            root_org_id: Some(s.tree.org.base.id),
            context: None,
            model: None,
            retention_days: None,
        })
        .send()
        .with_subscriber(
            tracing_subscriber::fmt()
                .with_test_writer()
                .with_ansi(false)
                .with_max_level(tracing::Level::ERROR)
                .finish(),
        )
        .await
        .ok()
}
async fn read(s: &Scene, who: &places::Who, id: Id) -> AskGetReply {
    s.rig
        .world
        .app
        .call(AskGet, &[&id])
        .token(&who.token)
        .send()
        .await
        .ok()
}
async fn provider(s: &Scene) -> Id {
    let id = new_id();
    sqlx::query("INSERT INTO providers(id,name,owner_id,org_id,home_id,kind,key_machine_id,machine_id,protocols,arrangements,status,upstreams) VALUES ($1,'Question API',$2,$3,$3,'api',$4,$4,ARRAY['openai-responses'],ARRAY['key_separated'],'ready',$5)")
        .bind(id).bind(s.ada.id()).bind(s.tree.org.base.id).bind(s.rig.machine_id)
        .bind(json!([{"protocol":"openai-responses","base_url":"https://model.example.invalid","options":{}}])).execute(s.rig.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES ($1,$2,$3,'Question model','{\"context_window\":200000}',true,'manual',$4)")
        .bind(new_id()).bind(id).bind(MODEL).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .lets_use("provider", id, "access.own", &s.ada, json!({}))
        .await;
    s.rig
        .lets_use("provider", id, "provider.use", &s.cai, json!({}))
        .await;
    s.rig.machine.handler.on(CommandKind::ProviderStart, |_| {
        Ok(CommandResult::ProviderStart(Empty {}))
    });
    id
}

async fn configured_provider(s: &Scene) -> Id {
    let id = provider(s).await;
    let mut models = orch_settings::get::<Vec<AskModelIdentity>>(
        &mut s.rig.db.acquire().await.unwrap(),
        orch_settings::Place::Node(s.tree.org.base.id),
        "ask.models",
    )
    .await
    .unwrap()
    .unwrap_or_default();
    models.push(AskModelIdentity {
        provider_id: id,
        model: MODEL.into(),
    });
    setting(s, "ask.models", json!(models)).await;
    id
}

async fn kept_native_source(s: &Scene) -> (Id, Id) {
    let provider = configured_provider(s).await;
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, |_| Ok(simple_answer()));
    let made = create(s, &s.ada, None).await;
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "before-native".into(),
            content: vec![ContentPart::Text {
                text: "Retain this question and its answer".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually(
        "the source program finishes its actual model turn",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        },
    )
    .await;
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody {
            topic_id: Some(s.tree.topic.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    // The destination's real harness accepts portable history; the source has no native archive.
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,import_neutral}','true') WHERE id=$1")
        .bind(s.on.agent).execute(s.rig.db.pool()).await.unwrap();
    (kept.thread_id, provider)
}

async fn native_program_plan(
    s: &Scene,
    source: Id,
    result: &str,
) -> orch_api_contract::forks::ForkPlan {
    use orch_api_contract::forks::{ForkRequest, ThreadForkPreflight};
    let request: ForkRequest = serde_json::from_value(json!({
        "result":result,"to":{"agent_id":s.on.agent,"provider_id":s.on.provider,"model":MODEL},
        "environment":{"use":s.on.environment},"history":{"mode":"all"}
    }))
    .unwrap();
    s.rig
        .world
        .app
        .call(ThreadForkPreflight, &[&source])
        .token(&s.ada.token)
        .body(&request)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
}

#[tokio::test]
async fn a_builtin_source_forks_to_a_native_profile_without_inventing_source_resources() {
    // EN browser: Ask > Send > Keep as thread > Fork > native profile and existing environment >
    // Confirm; inspect the copied question/answer and the unchanged Orch source.
    // 中文浏览器：问一问 > 发送 > 保留为会话 > 分叉 > 选择原生配置和现有环境 >
    // 确认；副本应保留提问与回答，原 Orch 会话不变。
    use orch_api_contract::forks::{ForkCreateBody, ForkPlanFrom, HistoryMethod, ThreadForkCreate};
    let s = Scene::new().await;
    let (source, provider) = kept_native_source(&s).await;
    let plan = native_program_plan(&s, source, "new").await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        matches!(&plan.from, ForkPlanFrom::Program {provider: actual,model,..}
        if actual.id == provider && model == MODEL)
    );
    assert!(plan.from.agent().is_none());
    assert!(plan.from.environment().is_none());
    assert_eq!(plan.history.method, HistoryMethod::NeutralImport);
    let job = s
        .rig
        .world
        .app
        .call(ThreadForkCreate, &[&source])
        .token(&s.ada.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    eventually("the program source makes its native fork", || async {
        let row = orch_db::queries::jobs::job(&mut s.rig.db.acquire().await.unwrap(), job.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !matches!(row.state.as_str(), "failed" | "cancelled"),
            "{:?}",
            row.error
        );
        row.state == "succeeded"
    })
    .await;
    let row = orch_db::queries::jobs::job(&mut s.rig.db.acquire().await.unwrap(), job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.plan["source_execution"]["kind"], "program");
    assert!(row.plan.get("from_environment").is_none());
    let target: Id = serde_json::from_value(row.plan["new_thread"].clone()).unwrap();
    let binding: (Option<Id>, Option<Id>, Option<String>) =
        sqlx::query_as("SELECT agent_id,environment_id,program FROM threads WHERE node_id=$1")
            .bind(target)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(binding, (Some(s.on.agent), Some(s.on.environment), None));
    let source_binding: (Option<Id>, Option<Id>, Option<String>) =
        sqlx::query_as("SELECT agent_id,environment_id,program FROM threads WHERE node_id=$1")
            .bind(source)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(source_binding, (None, None, Some("orch".into())));
    let copies: i64 =
        sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1 AND history_only")
            .bind(target)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert!(copies > 0);
    assert!(s.rig.seen.of_kind(CommandKind::SessionExport).is_empty());
}

#[tokio::test]
async fn a_builtin_switch_commits_only_after_the_native_target_is_ready() {
    // EN browser: kept Orch thread > Switch > native profile/environment > Confirm;
    // after readiness the current binding changes, with the old program visible in history.
    // 中文浏览器：已保留的 Orch 会话 > 切换 > 原生配置和环境 > 确认；
    // 目标就绪后才改变当前绑定，历史仍显示原内置程序。
    use orch_api_contract::forks::{ForkCreateBody, ThreadForkCreate};
    let s = Scene::new().await;
    let (source, _) = kept_native_source(&s).await;
    let old: Id = sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id=$1")
        .bind(source)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let plan = native_program_plan(&s, source, "replace").await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let job = s
        .rig
        .world
        .app
        .call(ThreadForkCreate, &[&source])
        .token(&s.ada.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    eventually("the native switch installs its target", || async {
        let row = orch_db::queries::jobs::job(&mut s.rig.db.acquire().await.unwrap(), job.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !matches!(row.state.as_str(), "failed" | "cancelled"),
            "{:?}",
            row.error
        );
        row.state == "succeeded"
    })
    .await;
    let binding: (Option<Id>, Option<Id>, Option<String>, Id) = sqlx::query_as(
        "SELECT agent_id,environment_id,program,current_segment_id FROM threads WHERE node_id=$1",
    )
    .bind(source)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        (binding.0, binding.1, binding.2),
        (Some(s.on.agent), Some(s.on.environment), None)
    );
    assert_ne!(binding.3, old);
    let historical: (Option<String>, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT program,ended_at FROM segments WHERE id=$1")
            .bind(old)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(historical.0.as_deref(), Some("orch"));
    assert!(historical.1.is_some());
}

#[tokio::test]
async fn a_failed_native_target_leaves_the_builtin_source_bound_and_readable() {
    // EN browser: kept Orch thread > Switch > native target fails to start;
    // reopen its history: the original program, provider/model and binding remain.
    // 中文浏览器：已保留的 Orch 会话 > 切换 > 原生目标启动失败；
    // 重开历史，原内置程序、提供方、模型和当前绑定必须保留。
    use orch_api_contract::forks::{ForkCreateBody, ThreadForkCreate};
    let s = Scene::new().await;
    let (source, _) = kept_native_source(&s).await;
    let before: (Option<String>, Option<Id>, Option<Id>, Option<Id>) = sqlx::query_as(
        "SELECT program,agent_id,environment_id,current_segment_id FROM threads WHERE node_id=$1",
    )
    .bind(source)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    let plan = native_program_plan(&s, source, "replace").await;
    for kind in [CommandKind::SessionOpen, CommandKind::SessionOpenFenced] {
        s.rig.machine.handler.on(kind, |_| {
            Err(orch_link_contract::commands::CommandError {
                code: "test.native-unavailable".into(),
                detail: "Native startup failed".into(),
                retryable: false,
                details: None,
            })
        });
    }
    let job = s
        .rig
        .world
        .app
        .call(ThreadForkCreate, &[&source])
        .token(&s.ada.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    eventually("the failed native switch is undone", || async {
        let row = orch_db::queries::jobs::job(&mut s.rig.db.acquire().await.unwrap(), job.id)
            .await
            .unwrap()
            .unwrap();
        row.state == "failed"
    })
    .await;
    let after: (Option<String>, Option<Id>, Option<Id>, Option<Id>) = sqlx::query_as(
        "SELECT program,agent_id,environment_id,current_segment_id FROM threads WHERE node_id=$1",
    )
    .bind(source)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(after, before);
    let node = places::read(&s.rig.world, &s.ada, source).await;
    let orch_api_contract::nodes::Node::Thread(thread) = node else {
        panic!("thread")
    };
    assert_eq!(thread.program.as_ref().unwrap().id.as_str(), "orch");
    let ended: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT ended_at FROM segments WHERE id=$1")
            .bind(before.3.unwrap())
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert!(ended.is_none());
}

#[tokio::test]
async fn switching_a_running_builtin_interrupts_its_actual_turn_without_a_native_harness() {
    // EN browser: kept Orch thread > Send > while answering, Switch > Interrupt;
    // the real answer stops and the verified native binding takes over without a forced-harness error.
    // 中文浏览器：已保留的 Orch 会话 > 发送 > 回答时切换 > 中断；
    // 实际回答结束，原生目标就绪后接管，不得出现虚构的原生进程超时错误。
    use orch_api_contract::forks::{
        ForkCreateBody, ForkRequest, ThreadForkCreate, ThreadForkPreflight,
    };
    let s = Scene::new().await;
    let (source, _) = kept_native_source(&s).await;
    s.rig.machine.handler.hold(CommandKind::ProviderGenerate);
    let accepted = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&source])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "busy-native-switch".into(),
            content: vec![ContentPart::Text {
                text: "An answer stopped by a real switch".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    let turn = accepted.turn_id.unwrap();
    eventually("the program has its actual generation command", || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM ask_calls WHERE turn_id=$1)")
            .bind(turn)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let request: ForkRequest = serde_json::from_value(json!({
        "result":"replace","to":{"agent_id":s.on.agent,"provider_id":s.on.provider,"model":MODEL},
        "environment":{"use":s.on.environment},"history":{"mode":"all"},
        "options":{"active_turn":"interrupt"}
    }))
    .unwrap();
    let plan = s
        .rig
        .world
        .app
        .call(ThreadForkPreflight, &[&source])
        .token(&s.ada.token)
        .body(&request)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert!(
        plan.turn_running && plan.blockers.is_empty(),
        "{:?}",
        plan.blockers
    );
    let job = s
        .rig
        .world
        .app
        .call(ThreadForkCreate, &[&source])
        .token(&s.ada.token)
        .body(&ForkCreateBody {
            notify: None,
            preflight_id: plan.preflight_id,
            options: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    eventually(
        "the real built-in turn ends before native handoff",
        || async {
            let row = orch_db::queries::jobs::job(&mut s.rig.db.acquire().await.unwrap(), job.id)
                .await
                .unwrap()
                .unwrap();
            assert!(
                !matches!(row.state.as_str(), "failed" | "cancelled"),
                "{:?}",
                row.error
            );
            row.state == "succeeded"
        },
    )
    .await;
    s.rig.machine.handler.release();
    let ended: (String, Option<String>, Option<Json>) =
        sqlx::query_as("SELECT state,interrupt_cause,error FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        ended,
        (
            "interrupted".into(),
            Some("environment_change".into()),
            None
        )
    );
    let binding: (Option<String>, Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT program,agent_id,environment_id FROM threads WHERE node_id=$1")
            .bind(source)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(binding, (None, Some(s.on.agent), Some(s.on.environment)));
}

async fn scoped_key(s: &Scene, scope: Json) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity;
    let scope: KeyScope = serde_json::from_value(scope).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let encoded = json!(scope);
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: s.ada.id(),
            name: "Ask operation fixture",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        Some(&encoded),
    )
    .await
    .unwrap();
    secret
}

async fn permission_key(s: &Scene, permissions: &[&str]) -> String {
    permission_key_for(s, s.ada.id(), permissions).await
}

async fn permission_key_for(s: &Scene, person: Id, permissions: &[&str]) -> String {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity;
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let scopes: Vec<String> = permissions
        .iter()
        .map(|permission| (*permission).to_owned())
        .collect();
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: person,
            name: "Ask permission ceiling fixture",
            token_hash: &hash,
            scopes: &scopes,
            node_id: None,
            at: s.rig.world.now(),
            expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await
    .unwrap();
    secret
}

fn simple_answer() -> CommandResult {
    CommandResult::ProviderGenerate(ProviderGenerateResult {
        native_result_blob: None,
        output: GenerationOutput {
            native_result: serde_json::Value::Null,
            message: GenerationMessage {
                role: GenerationRole::Assistant,
                content: vec![GenerationPart::Text {
                    text: "A retained answer".into(),
                }],
                continuation: vec![],
            },
            stop: GenerationStop::Complete,
        },
        usage: Default::default(),
        timing: Default::default(),
        request_ref: None,
    })
}

#[tokio::test]
async fn a_named_org_page_resolves_its_real_root_before_ask_birth() {
    // EN: organization page with its named address > Ask > first conversation;
    // the page's actual root governs it, without a fabricated UUID for the name.
    // 中文：使用命名地址打开组织页 > 问一问 > 新对话；必须使用页面的真实根组织，不伪造标识。
    let s = Scene::new().await;
    let made = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.cai.token)
        .body(&AskCreateBody {
            root_org_id: Some(s.tree.org.base.id),
            context: Some(format!("/org/{}", s.tree.org.address.to_uppercase())),
            model: None,
            retention_days: None,
        })
        .send()
        .await
        .ok();
    let got = read(&s, &s.cai, made.conversation_id).await;
    assert_eq!(got.conversation.root_org_id, s.tree.org.base.id);
}

#[tokio::test]
async fn a_viewer_owns_a_hidden_personal_ask_without_an_environment() {
    // EN browser: as the viewer, open a topic > Ask > New; inspect the root and history.
    // 中文浏览器：以查看者打开话题 > 问一问 > 新建；核对所属根组织和历史记录。
    let s = Scene::new().await;
    let made = create(&s, &s.cai, None).await;
    let got = read(&s, &s.cai, made.conversation_id).await;
    assert_eq!(got.conversation.root_org_id, s.tree.org.base.id);
    assert!(!got.conversation.shown);
    assert!(got.conversation.admin_inspectable);
    let stored:(Option<Id>,Option<Id>,String,Id)=sqlx::query_as("SELECT t.agent_id,t.environment_id,t.kind,n.org_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.node_id=$1")
        .bind(made.conversation_id).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(stored.0, None);
    assert_eq!(stored.1, None);
    assert_eq!(stored.2, "ask");
    let native = places::read(&s.rig.world, &s.cai, made.conversation_id).await;
    let orch_api_contract::nodes::Node::Thread(native) = native else {
        panic!("a real thread");
    };
    let program = native.program.unwrap();
    assert!(!program.requires_environment);
    assert!(!program.requires_native_harness);
    assert!(program.read_only);
    assert_eq!(program.root_org_id, s.tree.org.base.id);
    assert!(native.binding.is_none());
    assert!(native.environment.is_none());
    assert!(!native.base.chat_id.is_nil());
    let personal: Option<Id> = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
        .bind(s.cai.id())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(Some(stored.3), personal);
    let denied = s
        .rig
        .world
        .app
        .call(AskGet, &[&made.conversation_id])
        .token(&s.bea.token)
        .send()
        .await;
    assert!(denied.status.is_client_error());
}

#[tokio::test]
async fn a_department_member_can_ask_without_the_root_page_and_loses_it_with_membership() {
    // EN browser: as a department-only viewer, open its topic > Ask; the
    // conversation uses the fixed root without revealing the root's page.
    // 中文浏览器：仅有部门查看权限时，打开部门话题 > 问一问；所属根组织固定，不泄露根组织页面。
    use orch_api_contract::nodes::{NodeGet, OrgChildCreate, OrgChildCreateBody};
    let s = Scene::new().await;
    let department = s
        .rig
        .world
        .app
        .call(OrgChildCreate, &[&s.tree.org.base.id])
        .token(&s.ada.token)
        .body(
            &serde_json::from_value::<OrgChildCreateBody>(
                json!({"name":"Department","slug":"department","general":false}),
            )
            .unwrap(),
        )
        .send()
        .await
        .ok();
    let department = department.into_full().unwrap();
    let project =
        places::make_project(&s.rig.world, &s.ada, department.base.id, "Department work").await;
    let topic = places::make_topic(&s.rig.world, &s.ada, project.base.id, "Questions").await;
    sqlx::query("UPDATE org_members SET domain_id=$3 WHERE org_id=$1 AND user_id=$2")
        .bind(s.tree.org.base.id)
        .bind(s.cai.id())
        .bind(department.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE node_roles SET removed_at=now(),removed_by=$3 WHERE node_id=$1 AND subject_id=$2 AND subject_kind='user'")
        .bind(s.tree.org.base.id).bind(s.cai.id()).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    places::give_role(&s.rig.world, department.base.id, &s.cai, "viewer", &s.ada).await;
    assert!(
        s.rig
            .world
            .app
            .call(NodeGet, &[&s.tree.org.base.id])
            .token(&s.cai.token)
            .send()
            .await
            .status
            .is_client_error()
    );
    let made = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.cai.token)
        .body(&AskCreateBody {
            root_org_id: None,
            context: Some(format!("/topic/{}", topic.base.id)),
            model: None,
            retention_days: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        read(&s, &s.cai, made.conversation_id)
            .await
            .conversation
            .root_org_id,
        s.tree.org.base.id
    );
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_db::queries::access::testing::remove_member(&mut conn, s.tree.org.base.id, s.cai.id())
        .await
        .unwrap();
    drop(conn);
    assert!(
        s.rig
            .world
            .app
            .call(AskGet, &[&made.conversation_id])
            .token(&s.cai.token)
            .send()
            .await
            .status
            .is_client_error()
    );
}

#[tokio::test]
async fn ask_creations_replay_native_receipts_without_repeating_committed_effects() {
    // EN browser: New conversation > lose the creation response > retry the
    // same act: only one conversation exists. Keep as thread > retry after
    // deleting the temporary source: the same committed thread is acknowledged.
    // 中文浏览器：新对话 > 创建响应丢失 > 重试同一操作；只创建一个对话。
    // 保留为会话 > 删除临时来源后重试；应确认同一个已提交的永久线程。
    let s = Scene::new().await;
    let key = new_id().to_string();
    let body = AskCreateBody {
        root_org_id: Some(s.tree.org.base.id),
        context: Some(format!("/topic/{}", s.tree.topic.base.id)),
        model: None,
        retention_days: None,
    };
    let first = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.ada.token)
        .header("Idempotency-Key", &key)
        .body(&body)
        .send()
        .await
        .ok();
    let again = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.ada.token)
        .header("Idempotency-Key", &key)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(again, first);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE ask_root_org_id=$1")
        .bind(s.tree.org.base.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let changed = AskCreateBody {
        retention_days: Some(14),
        ..body.clone()
    };
    let mismatch = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.ada.token)
        .header("Idempotency-Key", &key)
        .body(&changed)
        .send()
        .await
        .problem();
    assert_eq!(
        mismatch.code,
        orch_api_contract::common::ErrorCode::ConflictIdempotencyMismatch
    );
    let keep_key = new_id().to_string();
    let keep_body = AskKeepBody::default();
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&first.conversation_id])
        .token(&s.ada.token)
        .header("Idempotency-Key", &keep_key)
        .body(&keep_body)
        .send()
        .await
        .ok();
    s.rig
        .world
        .app
        .call(AskDelete, &[&first.conversation_id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let replay = s
        .rig
        .world
        .app
        .call(AskKeep, &[&first.conversation_id])
        .token(&s.ada.token)
        .header("Idempotency-Key", &keep_key)
        .body(&keep_body)
        .send()
        .await
        .ok();
    assert_eq!(replay, kept);
    let replay = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.ada.token)
        .header("Idempotency-Key", &key)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(replay, first);
    let threads: Vec<Id> =
        sqlx::query_scalar("SELECT node_id FROM threads WHERE program_root_org_id=$1")
            .bind(s.tree.org.base.id)
            .fetch_all(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(threads, vec![kept.thread_id]);
}

#[tokio::test]
async fn simultaneous_first_asks_share_one_private_storage_container() {
    // EN browser: open Ask in two windows and start both conversations together.
    // 中文浏览器：在两个窗口打开“问一问”，同时创建对话；两者均应成功且互不混淆。
    let s = Scene::new().await;
    let (left, right) = tokio::join!(create(&s, &s.cai, None), create(&s, &s.cai, None));
    assert_ne!(left.conversation_id, right.conversation_id);
    let parents: Vec<Id> = sqlx::query_scalar("SELECT parent_id FROM nodes WHERE id=ANY($1)")
        .bind(vec![left.conversation_id, right.conversation_id])
        .fetch_all(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(parents.len(), 2);
    assert_eq!(parents[0], parents[1]);
    assert_eq!(
        read(&s, &s.cai, left.conversation_id)
            .await
            .conversation
            .user_id,
        s.cai.id()
    );
    assert_eq!(
        read(&s, &s.cai, right.conversation_id)
            .await
            .conversation
            .user_id,
        s.cai.id()
    );
}

#[tokio::test]
async fn hidden_conversation_uses_the_existing_private_composer_draft() {
    // EN browser: Ask > type a draft > close > History > reopen; the words return.
    // 中文浏览器：问一问 > 编辑草稿 > 关闭 > 历史记录 > 重开；草稿文字应恢复。
    use orch_api_contract::threads::{
        Delivery, ThreadDraftDelete, ThreadDraftGet, ThreadDraftSet, ThreadDraftSetBody,
    };
    let s = Scene::new().await;
    let made = create(&s, &s.cai, None).await;
    let draft = ThreadDraftSetBody {
        native_action: None,
        content: vec![ContentPart::Text {
            text: "A question kept for later".into(),
        }],
        delivery: Delivery::Queue,
        review_comments: None,
    };
    s.rig
        .world
        .app
        .call(ThreadDraftSet, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&draft)
        .send()
        .await
        .ok();
    let saved = s
        .rig
        .world
        .app
        .call(ThreadDraftGet, &[&made.conversation_id])
        .token(&s.cai.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(saved.content, draft.content);
    assert!(
        s.rig
            .world
            .app
            .call(ThreadDraftGet, &[&made.conversation_id])
            .token(&s.bea.token)
            .send()
            .await
            .status
            .is_client_error()
    );
    s.rig
        .world
        .app
        .call(ThreadDraftDelete, &[&made.conversation_id])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert!(
        s.rig
            .world
            .app
            .call(ThreadDraftGet, &[&made.conversation_id])
            .token(&s.cai.token)
            .send()
            .await
            .ok()
            .is_none()
    );
}

#[tokio::test]
async fn native_thread_and_draft_doors_keep_the_conversations_root_governance() {
    // EN browser: As admin > inspect the owner's hidden Ask through the normal
    // thread view; history is readable, Send is disabled and drafts remain private.
    // 中文浏览器：以管理员身份通过普通会话查看入口打开隐藏对话；可读历史，不可发送，草稿仍私有。
    use orch_api_contract::threads::{
        Delivery, ThreadDraftGet, ThreadDraftSet, ThreadDraftSetBody, ThreadInputCreate,
        ThreadInputCreateBody, ThreadItemList, ThreadItemListQuery,
    };
    let s = Scene::new().await;
    let made = create(&s, &s.cai, None).await;
    s.rig
        .world
        .app
        .call(ThreadDraftSet, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&ThreadDraftSetBody {
            native_action: None,
            content: vec![ContentPart::Text {
                text: "An unsent private question".into(),
            }],
            delivery: Delivery::Queue,
            review_comments: None,
        })
        .send()
        .await
        .ok();
    assert!(
        s.rig
            .world
            .app
            .call(ThreadItemList, &[&made.conversation_id])
            .query(&ThreadItemListQuery {
                after: Some(0),
                ..Default::default()
            })
            .token(&s.ada.token)
            .send()
            .await
            .status
            .is_client_error()
    );
    places::admin_mode(&s.rig.world, &s.ada, true).await;
    assert!(
        s.rig
            .world
            .app
            .call(ThreadItemList, &[&made.conversation_id])
            .query(&ThreadItemListQuery {
                after: Some(0),
                ..Default::default()
            })
            .token(&s.ada.token)
            .send()
            .await
            .status
            .is_success()
    );
    let writing = s
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&ThreadInputCreateBody {
            content: vec![ContentPart::Text {
                text: "Must not be sent".into(),
            }],
            delivery: Delivery::Queue,
            client_id: "inspect-only".into(),
            bypass: None,
            native_action: None,
            plan: false,
        })
        .send()
        .await;
    assert_eq!(writing.status, http::StatusCode::FORBIDDEN);
    assert!(
        s.rig
            .world
            .app
            .call(ThreadDraftGet, &[&made.conversation_id])
            .token(&s.ada.token)
            .send()
            .await
            .status
            .is_client_error()
    );
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_db::queries::access::testing::remove_member(&mut conn, s.tree.org.base.id, s.cai.id())
        .await
        .unwrap();
    drop(conn);
    assert!(
        s.rig
            .world
            .app
            .call(ThreadDraftGet, &[&made.conversation_id])
            .token(&s.cai.token)
            .send()
            .await
            .status
            .is_client_error()
    );
    assert!(
        s.rig
            .world
            .app
            .call(ThreadItemList, &[&made.conversation_id])
            .query(&ThreadItemListQuery {
                after: Some(0),
                ..Default::default()
            })
            .token(&s.cai.token)
            .send()
            .await
            .status
            .is_client_error()
    );
}

#[tokio::test]
async fn native_drafts_close_at_ask_expiry_before_the_worker_purges_storage() {
    // EN browser: save an Ask draft, advance past retention, reopen the thread;
    // neither the ordinary draft nor its history can be read while purge is pending.
    // 中文浏览器：保存提问草稿并超过保留期限后重开；清理尚未执行时，普通草稿和历史也不可读。
    use orch_api_contract::threads::{
        Delivery, ThreadDraftGet, ThreadDraftSet, ThreadDraftSetBody, ThreadItemList,
    };
    let s = Scene::new().await;
    let made = create(&s, &s.cai, Some(1)).await;
    s.rig
        .world
        .app
        .call(ThreadDraftSet, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&ThreadDraftSetBody {
            native_action: None,
            content: vec![ContentPart::Text {
                text: "A draft that will expire".into(),
            }],
            delivery: Delivery::Queue,
            review_comments: None,
        })
        .send()
        .await
        .ok();
    s.rig.world.advance(chrono::Duration::days(2));
    assert!(
        s.rig
            .world
            .app
            .call(ThreadDraftGet, &[&made.conversation_id])
            .token(&s.cai.token)
            .send()
            .await
            .status
            .is_client_error()
    );
    assert!(
        s.rig
            .world
            .app
            .call(ThreadItemList, &[&made.conversation_id])
            .token(&s.cai.token)
            .send()
            .await
            .status
            .is_client_error()
    );
    let stored: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1)")
        .bind(made.conversation_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        stored,
        "the native boundary applies before physical cleanup"
    );
}

#[tokio::test]
async fn retention_is_clamped_and_expiry_has_no_second_delete_window() {
    // EN browser: Ask > history settings > request one day under a seven-day minimum.
    // 中文浏览器：问一问 > 历史设置 > 在最少七天的策略下选择一天，核对实际保留七天。
    let s = Scene::new().await;
    setting(&s, "ask.retention_min_days", json!(7)).await;
    setting(&s, "ask.retention_max_days", json!(9)).await;
    let made = create(&s, &s.ada, Some(1)).await;
    let got = read(&s, &s.ada, made.conversation_id).await;
    assert_eq!(got.conversation.retention_days, 7);
    let mut tx = s.rig.db.begin().await.unwrap();
    let at = got.conversation.last_message_at.get() + chrono::Duration::days(7);
    assert_eq!(
        orch_db::queries::ask::purge_root(&mut tx, s.tree.org.base.id, 7, 9, at, 200)
            .await
            .unwrap(),
        1
    );
    tx.commit().await.unwrap();
    let present: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1)")
        .bind(made.conversation_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        !present,
        "expiry hard-deletes the node rather than starting soft-delete retention"
    );
}

#[tokio::test]
async fn deleting_a_governing_root_removes_its_ask_nodes_without_touching_personal_asks() {
    // EN browser: keep an Ask about the team and one about your personal org;
    // after the team is permanently deleted, its Ask disappears from history
    // while the personal conversation remains readable.
    // 中文浏览器：分别保留团队与个人组织的提问；团队被永久删除后，其提问应从历史记录消失，
    // 个人对话仍可读取，虽然两者曾存储在同一个人空间。
    let s = Scene::new().await;
    let team = create(&s, &s.cai, None).await;
    let personal: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
        .bind(s.cai.id())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_ne!(personal, s.tree.org.base.id);
    let personal_ask = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.cai.token)
        .body(&AskCreateBody {
            root_org_id: Some(personal),
            context: None,
            model: None,
            retention_days: None,
        })
        .send()
        .await
        .ok();
    let homes: Vec<Id> =
        sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=ANY($1) ORDER BY id")
            .bind(vec![team.conversation_id, personal_ask.conversation_id])
            .fetch_all(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(homes, vec![personal, personal]);
    // Model the final native purge after the root's physical children are due.
    // Its schema trigger also deletes the governed Ask's real node in another home.
    sqlx::query("DELETE FROM nodes WHERE org_id=$1")
        .bind(s.tree.org.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let remains: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1)")
        .bind(team.conversation_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert!(!remains);
    let dangling: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads WHERE node_id=$1)")
            .bind(team.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert!(!dangling);
    assert_eq!(
        read(&s, &s.cai, personal_ask.conversation_id)
            .await
            .conversation
            .root_org_id,
        personal
    );
}

#[tokio::test]
async fn root_admin_inspection_is_explicit_audited_and_read_only() {
    // EN browser: As admin > this root's Ask history > open another person's conversation > Export.
    // 中文浏览器：以管理员身份 > 本根组织的提问历史 > 打开他人的会话 > 导出；不能发送或停止。
    let s = Scene::new().await;
    let made = create(&s, &s.cai, None).await;
    let export_query = orch_api_contract::threads::ThreadExportQuery {
        format: orch_api_contract::threads::ExportFormat::Json,
        continuation_id: None,
        projection_revision: None,
        detail: None,
        from: None,
        to: None,
        include: None,
    };
    let ordinary_export = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::threads::ThreadExport,
            &[&made.conversation_id],
        )
        .token(&s.ada.token)
        .query(&export_query)
        .send_bytes()
        .await;
    assert_eq!(ordinary_export.status.as_u16(), 403);
    assert!(
        s.rig
            .world
            .app
            .call(AskGet, &[&made.conversation_id])
            .token(&s.ada.token)
            .send()
            .await
            .status
            .is_client_error()
    );
    places::admin_mode(&s.rig.world, &s.ada, true).await;
    let got = read(&s, &s.ada, made.conversation_id).await;
    assert!(got.conversation.inspecting);
    assert!(!got.conversation.can.message);
    assert!(!got.conversation.can.stop);
    let listed = s
        .rig
        .world
        .app
        .call(AskList, &[])
        .token(&s.ada.token)
        .query(&AskListQuery {
            inspect: true,
            root_org_id: Some(s.tree.org.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        listed
            .items
            .iter()
            .any(|row| row.id == made.conversation_id)
    );
    let exported = s
        .rig
        .world
        .app
        .call(AskExport, &[&made.conversation_id])
        .token(&s.ada.token)
        .send_bytes()
        .await;
    assert!(exported.status.is_success());
    let body: Json = serde_json::from_slice(&exported.raw).unwrap();
    assert_eq!(body["conversation"]["id"], made.conversation_id.to_string());
    // EN browser: As admin > open the hidden conversation through its ordinary
    // thread inspection > Export; View and Export are offered, never Send.
    // 中文浏览器：以管理员身份 > 从普通线程查看入口打开隐藏对话 > 导出；可查看和导出，不可发送。
    let native = places::read(&s.rig.world, &s.ada, made.conversation_id).await;
    let orch_api_contract::nodes::Node::Thread(native) = native else {
        panic!("thread");
    };
    assert!(
        native
            .base
            .access
            .permissions
            .iter()
            .any(|permission| permission == "thread.export")
    );
    assert!(
        !native
            .base
            .access
            .permissions
            .iter()
            .any(|permission| permission == "thread.interact" || permission == "thread.manage")
    );
    let native = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::threads::ThreadExport,
            &[&made.conversation_id],
        )
        .token(&s.ada.token)
        .query(&export_query)
        .send_bytes()
        .await;
    assert_eq!(native.status.as_u16(), 200);
    let native: orch_api_contract::threads::ThreadExportJson =
        serde_json::from_slice(&native.raw).unwrap();
    assert_eq!(native.thread.base.id, made.conversation_id);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='admin.opened' AND object_id=$1",
    )
    .bind(made.conversation_id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert!(count > 0);
}

#[tokio::test]
async fn root_model_choices_do_not_require_thread_creation_authority() {
    // EN browser: viewer > Ask > model chip; allowed API choices and exclusions load without a thread-start request.
    // 中文浏览器：查看者 > 问一问 > 模型选择；无需新建线程权限即可看到可选模型和排除原因。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,enabled,source,created_by) VALUES ($1,$2,'switched-off','Off',false,'manual',$3)")
        .bind(new_id()).bind(provider).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    setting(&s,"ask.models",json!([{ "provider_id":provider,"model":MODEL },{ "provider_id":provider,"model":"switched-off" }])).await;
    let reply = s
        .rig
        .world
        .app
        .call(AskModelsGet, &[])
        .token(&s.cai.token)
        .query(&AskModelsQuery {
            root_org_id: s.tree.org.base.id,
        })
        .send()
        .await
        .ok();
    let allowed = reply
        .models
        .iter()
        .find(|choice| choice.selection.provider_id == provider && choice.selection.model == MODEL)
        .unwrap();
    assert!(allowed.allowed);
    assert!(allowed.is_default);
    let disabled = reply
        .models
        .iter()
        .find(|choice| choice.selection.model == "switched-off")
        .unwrap();
    assert!(!disabled.allowed);
    assert!(disabled.unavailable_reason.is_some());
}

#[tokio::test]
async fn model_choices_keep_the_original_keys_root_read_admission() {
    // EN browser/API: root-scoped Ask model-list Read > Ask > model chip; the
    // default loads without ProviderGet. A key for another operation stays refused.
    // 中文浏览器/API：仅授予根组织的问一问模型列表读取 > 问一问 > 模型选项；
    // 不读取提供方也能显示默认项；仅获其他操作权限的密钥仍应被拒绝。

    // EN browser/API: open Ask's model picker with a root Read key; allowed
    // choices load. A key for another operation cannot borrow that Read.
    // 中文浏览器/API：使用根组织读取密钥打开模型选择器，应加载可用模型；
    // 只允许另一操作的密钥不能借用该读取权限。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    for operation in ["AskGet", "AskModelsGet"] {
        let secret = scoped_key(
            &s,
            json!({"rules":[{
                "target":{"kind":"org","id":s.tree.org.base.id,"reach":"subtree"},
                "allow":[{"operation":operation,"actions":["read"]}]
            }]}),
        )
        .await;
        let reply = s
            .rig
            .world
            .app
            .call(AskModelsGet, &[])
            .token(&secret)
            .query(&AskModelsQuery {
                root_org_id: s.tree.org.base.id,
            })
            .send()
            .await;
        if operation == "AskModelsGet" {
            let reply = reply.ok();
            assert!(
                reply
                    .models
                    .iter()
                    .any(|choice| choice.selection.provider_id == provider
                        && choice.selection.model == MODEL
                        && choice.allowed
                        && choice.is_default)
            );
        } else {
            assert_eq!(reply.status.as_u16(), 403);
        }
    }
}

#[tokio::test]
async fn ask_model_options_include_native_constraints_without_provider_read_operations() {
    // EN browser: open Ask with only its model-list Read > model chip > Parameters;
    // the declared temperature range loads without a provider/model-list request.
    // 中文浏览器：仅持问一问模型列表读取权限 > 模型选择 > 参数；
    // 应显示声明的温度范围，无需读取提供方或其模型列表。
    use orch_api_contract::resources::{ProviderGet, ProviderModelList};
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    sqlx::query("UPDATE provider_models SET parameters=$2 WHERE provider_id=$1 AND model_name=$3")
        .bind(provider)
        .bind(json!({"request_options":{"openai-responses":{
            "type":"object","properties":{"temperature":{"type":"number","minimum":0,"maximum":0.5}}
        }}}))
        .bind(MODEL)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let secret = scoped_key(
        &s,
        json!({"rules":[{
            "target":{"kind":"org","id":s.tree.org.base.id,"reach":"subtree"},
            "allow":[{"operation":"AskModelsGet","actions":["read"]}]
        }]}),
    )
    .await;
    assert_eq!(
        s.rig
            .world
            .app
            .call(ProviderGet, &[&provider])
            .token(&secret)
            .send()
            .await
            .status
            .as_u16(),
        403
    );
    assert_eq!(
        s.rig
            .world
            .app
            .call(ProviderModelList, &[&provider])
            .token(&secret)
            .send()
            .await
            .status
            .as_u16(),
        403
    );
    let reply = s
        .rig
        .world
        .app
        .call(AskModelsGet, &[])
        .token(&secret)
        .query(&AskModelsQuery {
            root_org_id: s.tree.org.base.id,
        })
        .send()
        .await
        .ok();
    let option = reply
        .models
        .iter()
        .find(|option| option.selection.provider_id == provider)
        .unwrap();
    assert!(option.allowed, "{:?}", option.unavailable_reason);
    let schema = option.request_option_schema.as_ref().unwrap();
    assert_eq!(
        schema,
        &option.model.as_ref().unwrap().option_constraints["openai-responses"]
    );
    assert!(orch_types::schema::validate(schema, &json!({"temperature":0.3})).is_ok());
    assert!(orch_types::schema::validate(schema, &json!({"temperature":0.8})).is_err());
    assert_eq!(schema["properties"]["temperature"]["maximum"], 0.5);
}

#[tokio::test]
async fn ask_model_discovery_batches_queries_without_changing_order_or_exclusions() {
    // EN browser: Ask > Model with two then thirty-two configured providers;
    // the first unavailable choice remains excluded and the next is Default.
    // 中文浏览器：问一问 > 模型，先配置两个、再配置三十二个提供方；
    // 首个不可用项仍被排除，下一个可用项仍为默认，列表加载不逐项发起读取。
    let s = Scene::new().await;
    let disabled = configured_provider(&s).await;
    let default = configured_provider(&s).await;
    sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
        .bind(disabled)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let mut counts = Vec::new();
    let mut stable = None;
    for total in [2, 32] {
        if total > 2 {
            for _ in 2..total {
                configured_provider(&s).await;
            }
        }
        let (reply, count) = support::Statements::measure(
            s.rig
                .world
                .app
                .call(AskModelsGet, &[])
                .token(&s.cai.token)
                .query(&AskModelsQuery {
                    root_org_id: s.tree.org.base.id,
                })
                .send()
                .with_subscriber(support::Statements),
        )
        .await;
        let reply = reply.ok();
        assert_eq!(reply.models.len(), total);
        assert_eq!(reply.models[0].selection.provider_id, disabled);
        assert!(!reply.models[0].allowed);
        assert!(reply.models[0].unavailable_reason.is_some());
        assert_eq!(reply.models[1].selection.provider_id, default);
        assert!(reply.models[1].allowed && reply.models[1].is_default);
        assert_eq!(reply.default_model.as_ref().unwrap().provider_id, default);
        assert!(reply.blocked_reason.is_none());
        let first = reply.models[..2].to_vec();
        match &stable {
            Some(stable) => assert_eq!(stable, &first),
            None => stable = Some(first),
        }
        assert!(count > 0, "the native SQL subscriber must observe the read");
        counts.push(count);
    }
    assert_eq!(
        counts[0], counts[1],
        "provider/model discovery and source facts must use bounded batches"
    );
    let mut conn = s.rig.db.acquire().await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(s.cai.id()),
        orch_types::Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    let ids = [disabled, default];
    let policies = orch_engine::access::resource_policies_among(&mut conn, &ids)
        .await
        .unwrap();
    let batched = orch_authz::usable::usable_among(
        &mut conn,
        &principal,
        &ids,
        Some(s.tree.org.base.id),
        &policies,
    )
    .await
    .unwrap();
    for id in ids {
        let policy = orch_engine::access::resource_policies(&mut conn, id)
            .await
            .unwrap();
        let original = orch_authz::usable::usable(
            &mut conn,
            &principal,
            id,
            Some(s.tree.org.base.id),
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(
            batched.get(&id),
            Some(&original),
            "batching preserves native Use/options and unavailability"
        );
    }
}

#[tokio::test]
async fn ask_models_refuses_outside_choices_and_reports_policy_empty_and_unusable_lists() {
    // EN: configure Ask models > request an unlisted model > refusal; remove
    // all entries by policy, empty the list, then leave one unavailable entry.
    // 中文：配置问一问模型 > 请求列表外模型被拒绝；再用策略排除全部、清空列表，最后仅留不可用项。
    let s = Scene::new().await;
    let listed = configured_provider(&s).await;
    let outside = provider(&s).await;
    let outside_birth = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.ada.token)
        .body(&AskCreateBody {
            root_org_id: Some(s.tree.org.base.id),
            context: None,
            retention_days: None,
            model: Some(AskModel {
                provider_id: outside,
                model: MODEL.into(),
                params: None,
            }),
        })
        .send()
        .await;
    assert_eq!(outside_birth.status.as_u16(), 400);
    let calls = Arc::new(Mutex::new(0));
    let seen = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            *seen.lock().unwrap() += 1;
            Ok(simple_answer())
        });
    let made = create_unseeded(&s, &s.ada).await;
    let question = |client: &str, model| AskMessageBody {
        client_id: client.into(),
        content: vec![ContentPart::Text {
            text: "Use only the configured choices".into(),
        }],
        context: None,
        model,
    };
    let refused = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&question(
            "outside",
            Some(Some(AskModel {
                provider_id: outside,
                model: MODEL.into(),
                params: None,
            })),
        ))
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 400);
    assert_eq!(*calls.lock().unwrap(), 0);
    setting(&s, "models.allowed", json!([])).await;
    let blocked = s
        .rig
        .world
        .app
        .call(AskModelsGet, &[])
        .token(&s.ada.token)
        .query(&AskModelsQuery {
            root_org_id: s.tree.org.base.id,
        })
        .send()
        .await
        .ok();
    assert_eq!(blocked.models.len(), 1);
    assert_eq!(
        blocked.blocked_reason,
        Some(AskModelsBlockedReason::ExcludedByPolicy)
    );
    assert!(blocked.default_model.is_none());
    assert!(blocked.models.iter().all(|entry| !entry.allowed));
    let refused = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&question("policy", None))
        .send()
        .await;
    assert_eq!(
        refused.problem().extra["blocked_reason"],
        "excluded_by_policy"
    );
    setting(&s, "ask.models", json!([])).await;
    let empty = s
        .rig
        .world
        .app
        .call(AskModelsGet, &[])
        .token(&s.ada.token)
        .query(&AskModelsQuery {
            root_org_id: s.tree.org.base.id,
        })
        .send()
        .await
        .ok();
    assert_eq!(empty.blocked_reason, Some(AskModelsBlockedReason::Empty));
    assert!(empty.models.is_empty());
    setting(&s, "models.allowed", json!(["*"])).await;
    setting(
        &s,
        "ask.models",
        json!([{ "provider_id":listed,"model":MODEL }]),
    )
    .await;
    sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
        .bind(listed)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let unusable = s
        .rig
        .world
        .app
        .call(AskModelsGet, &[])
        .token(&s.ada.token)
        .query(&AskModelsQuery {
            root_org_id: s.tree.org.base.id,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        unusable.blocked_reason,
        Some(AskModelsBlockedReason::NoUsableModels)
    );
    assert!(unusable.default_model.is_none());
    assert_eq!(*calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn ask_models_unset_blocks_then_the_first_usable_entry_answers_without_a_pick() {
    // EN: unset Ask models > first question is blocked > configure two entries,
    // disable the first > Send without selecting a model > the second answers.
    // 中文：未配置模型列表时首问被阻止；配置两项并停用第一项，不选模型发送，第二项回答。
    let s = Scene::new().await;
    let first = provider(&s).await;
    let second = provider(&s).await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation")
            };
            seen.lock().unwrap().push(request.provider_id);
            Ok(simple_answer())
        });
    let made = create_unseeded(&s, &s.ada).await;
    let question = AskMessageBody {
        client_id: "first-list-question".into(),
        content: vec![ContentPart::Text {
            text: "Answer without making me choose".into(),
        }],
        context: None,
        model: None,
    };
    let unset = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&question)
        .send()
        .await;
    assert_eq!(unset.status.as_u16(), 400);
    assert_eq!(unset.problem().extra["blocked_reason"], "not_configured");
    assert!(calls.lock().unwrap().is_empty());
    setting(
        &s,
        "ask.models",
        json!([{ "provider_id":first,"model":MODEL },{ "provider_id":second,"model":MODEL }]),
    )
    .await;
    sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
        .bind(first)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let choices = s
        .rig
        .world
        .app
        .call(AskModelsGet, &[])
        .token(&s.ada.token)
        .query(&AskModelsQuery {
            root_org_id: s.tree.org.base.id,
        })
        .send()
        .await
        .ok();
    assert_eq!(choices.models.len(), 2);
    assert!(!choices.models[0].allowed);
    assert!(choices.models[1].allowed);
    assert_eq!(choices.default_model.as_ref().unwrap().provider_id, second);
    assert!(choices.blocked_reason.is_none());
    assert!(choices.writable);
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&question)
        .send()
        .await
        .ok();
    eventually("the configured default answers", || async {
        !calls.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(calls.lock().unwrap().as_slice(), [second]);
}

#[tokio::test]
async fn a_saved_excluded_model_uses_todays_allowed_default_with_a_notice() {
    // EN browser: resume a conversation whose saved model was disabled; Send
    // uses today's allowed default and displays the model-change notice.
    // 中文浏览器：重开已禁用旧模型的对话；“发送”使用当前允许的默认模型并显示切换提示。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let made = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.cai.token)
        .body(&AskCreateBody {
            root_org_id: Some(s.tree.org.base.id),
            context: None,
            retention_days: None,
            model: None,
        })
        .send()
        .await
        .ok();
    // A formerly saved entry is removed from the current configuration.
    sqlx::query("UPDATE threads SET ask_model=$2 WHERE node_id=$1")
        .bind(made.conversation_id)
        .bind(json!({"provider_id":provider,"model":"removed-model"}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let models = Arc::new(Mutex::new(Vec::new()));
    let seen = models.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            seen.lock().unwrap().push(request.model_name.clone());
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![GenerationPart::Text {
                            text: "Answer".into(),
                        }],
                        continuation: vec![],
                    },
                    stop: GenerationStop::Complete,
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "resume".into(),
            content: vec![ContentPart::Text {
                text: "Continue".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the allowed default answers", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(models.lock().unwrap().as_slice(), [MODEL]);
    let changed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ask_events WHERE thread_id=$1 AND body->>'code'='model_changed'",
    )
    .bind(made.conversation_id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(changed, 1);
    assert!(
        s.rig
            .world
            .app
            .call(AskMessageCreate, &[&made.conversation_id])
            .token(&s.cai.token)
            .body(&AskMessageBody {
                client_id: "explicit".into(),
                content: vec![ContentPart::Text {
                    text: "Use this".into()
                }],
                context: None,
                model: Some(Some(AskModel {
                    provider_id: provider,
                    model: "removed-model".into(),
                    params: None
                }))
            })
            .send()
            .await
            .status
            .is_client_error()
    );
    assert_eq!(models.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn default_model_preference_follows_current_settings_and_can_replace_an_explicit_choice() {
    // EN browser: send with Use the current default > change the org's default
    // > send again; choose a specific model > send > choose Use the current
    // default > send. The chip remains a preference and Keep retains all turns.
    // 中文浏览器：使用当前默认模型发送 > 更改组织默认模型 > 再发送；选择指定模型发送后，
    // 再选“使用当前默认模型”发送。模型选择保留为偏好，保留为会话仍应包含所有回合。
    let s = Scene::new().await;
    let first = configured_provider(&s).await;
    let other = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":first,"model":MODEL},{"provider_id":other,"model":MODEL}]),
    )
    .await;
    let providers = Arc::new(Mutex::new(Vec::new()));
    let seen = providers.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            seen.lock().unwrap().push(request.provider_id);
            Ok(simple_answer())
        });
    let made = create(&s, &s.ada, None).await;
    for index in 0..5 {
        if index == 1 {
            setting(
                &s,
                "ask.models",
                json!([{"provider_id":other,"model":MODEL},{"provider_id":first,"model":MODEL}]),
            )
            .await;
        }
        let selection = match index {
            2 => Some(Some(AskModel {
                provider_id: first,
                model: MODEL.into(),
                params: None,
            })),
            4 => Some(None),
            _ => None,
        };
        s.rig
            .world
            .app
            .call(AskMessageCreate, &[&made.conversation_id])
            .token(&s.ada.token)
            .body(&AskMessageBody {
                client_id: format!("default-{index}"),
                content: vec![ContentPart::Text {
                    text: "Which model handles this question?".into(),
                }],
                context: None,
                model: selection,
            })
            .send()
            .await
            .ok();
        eventually("the selected preference answers", || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        })
        .await;
        let preference = read(&s, &s.ada, made.conversation_id)
            .await
            .conversation
            .model;
        if matches!(index, 2 | 3) {
            assert_eq!(preference.unwrap().provider_id, first);
        } else {
            assert!(
                preference.is_none(),
                "default remains a preference, not the last resolved model"
            );
        }
    }
    assert_eq!(
        providers.lock().unwrap().as_slice(),
        [first, other, first, first, other]
    );
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody::default())
        .send()
        .await
        .ok();
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(kept.thread_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        turns, 5,
        "Keep copies actual history even while the model preference is default"
    );
}

#[tokio::test]
async fn changing_the_model_origin_keeps_text_without_replaying_another_interfaces_opaque_state() {
    // EN browser: ask twice with the same API model, then select another
    // provider/interface and Send. Readable answers remain in context, while
    // signed or encrypted continuation stays with its originating model.
    // 中文浏览器：使用同一 API 模型提问两次，再选择另一提供方或接口发送；
    // 可读回答仍在上下文中，签名或加密续接状态不得传给其他模型来源。
    let s = Scene::new().await;
    let first = configured_provider(&s).await;
    let other = configured_provider(&s).await;
    sqlx::query("UPDATE providers SET protocols=ARRAY['anthropic-messages'],upstreams=$2 WHERE id=$1")
        .bind(other).bind(json!([{"protocol":"anthropic-messages","base_url":"https://other-model.example.invalid","options":{}}]))
        .execute(s.rig.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES ($1,$2,'other-model','Other model','{\"context_window\":200000}',true,'manual',$3)")
        .bind(new_id()).bind(other).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":first,"model":MODEL},{"provider_id":other,"model":MODEL},{"provider_id":other,"model":"other-model"}]),
    )
    .await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            seen.lock().unwrap().push(request.clone());
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![GenerationPart::Text {
                            text: "A readable retained answer".into(),
                        }],
                        continuation: vec![json!({"opaque":"synthetic-model-continuation"})],
                    },
                    stop: GenerationStop::Complete,
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let made = create_unseeded(&s, &s.cai).await;
    for index in 0..5 {
        if index == 2 {
            sqlx::query("UPDATE providers SET protocols=ARRAY['anthropic-messages'],upstreams=$2 WHERE id=$1")
                .bind(first).bind(json!([{"protocol":"anthropic-messages","base_url":"https://model.example.invalid","options":{}}]))
                .execute(s.rig.db.pool()).await.unwrap();
        }
        s.rig
            .world
            .app
            .call(AskMessageCreate, &[&made.conversation_id])
            .token(&s.cai.token)
            .body(&AskMessageBody {
                client_id: format!("origin-{index}"),
                content: vec![ContentPart::Text {
                    text: "Continue the conversation".into(),
                }],
                context: None,
                model: (index >= 3)
                    .then(|| AskModel {
                        provider_id: other,
                        model: if index == 4 {
                            "other-model".into()
                        } else {
                            MODEL.into()
                        },
                        params: None,
                    })
                    .map(Some),
            })
            .send()
            .await
            .ok();
        eventually("the selected model origin answers", || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        })
        .await;
    }
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[1].provider_id, first);
    assert!(
        requests[1]
            .input
            .messages
            .iter()
            .any(|message| !message.continuation.is_empty())
    );
    assert_eq!(requests[2].provider_id, first);
    assert_eq!(requests[2].protocol, "anthropic-messages");
    assert_eq!(requests[3].provider_id, other);
    assert_eq!(requests[4].model_name, "other-model");
    for request in &requests[2..] {
        assert!(
            request
                .input
                .messages
                .iter()
                .all(|message| message.continuation.is_empty())
        );
    }
    assert!(requests[2].input.messages.iter().flat_map(|message| &message.content)
        .any(|part| matches!(part, GenerationPart::Text { text } if text == "A readable retained answer")));
}

#[tokio::test]
async fn sourced_opaque_state_stays_inside_its_current_tool_round_trip() {
    // EN browser/API: ask the program to find a read operation > wait for its
    // answer > ask a follow-up. Current tool thinking is preserved, while the
    // next question receives fresh readable results without old hidden state.
    // 中文浏览器/API：让程序查找读取操作 > 等待回答 > 继续提问；本轮工具推理可保留，
    // 下一问题只能取得重新检查的可读结果，不得重放此前的隐藏来源状态。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let requests = seen.clone();
    s.rig.machine.handler.on(CommandKind::ProviderGenerate, move |command| {
        let Command::ProviderGenerate(request) = command else { panic!("generation"); };
        let mut seen = requests.lock().unwrap();
        let first = seen.is_empty();
        seen.push(request.clone());
        Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
            native_result_blob: None,
            output: GenerationOutput {
                native_result: serde_json::Value::Null, message: GenerationMessage { role: GenerationRole::Assistant,
                content: if first { vec![GenerationPart::ToolCall {
                    id: "catalogue".into(), name: "find_operation".into(), arguments: json!({"query":"TopicList"}),
                }] } else { vec![GenerationPart::Text { text: "The visible sourced answer".into() }] },
                continuation: vec![json!({"opaque":if first {"current-tool-state"}else{"sourced-hidden-state"}})],
            }, stop: if first { GenerationStop::Tools } else { GenerationStop::Complete } },
            usage: Default::default(), timing: Default::default(), request_ref: None,
        }))
    });
    let source = create_unseeded(&s, &s.cai).await;
    for index in 0..2 {
        s.rig
            .world
            .app
            .call(AskMessageCreate, &[&source.conversation_id])
            .token(&s.cai.token)
            .body(&AskMessageBody {
                client_id: format!("opaque-sources-{index}"),
                context: None,
                model: None,
                content: vec![ContentPart::Text {
                    text: "Explain the readable catalogue".into(),
                }],
            })
            .send()
            .await
            .ok();
        eventually("the sourced question finishes", || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(source.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        })
        .await;
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert!(
        seen[1]
            .input
            .messages
            .iter()
            .any(|message| !message.continuation.is_empty()),
        "the active tool response keeps its own native thinking state"
    );
    assert!(
        seen[2]
            .input
            .messages
            .iter()
            .all(|message| message.continuation.is_empty()),
        "a later question must not recover hidden earlier source material"
    );
    assert!(
        serde_json::to_string(&seen[2].input)
            .unwrap()
            .contains("The visible sourced answer")
    );
    assert!(
        !seen[2]
            .input
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|part| matches!(
                part,
                GenerationPart::ToolCall { .. } | GenerationPart::ToolResult { .. }
            )),
        "portable text retains source history without requiring missing signatures"
    );
}

#[tokio::test]
async fn repeated_provider_tool_ids_keep_distinct_journal_witnesses_through_keep() {
    // EN browser/API: ask two different reads whose provider reuses one call
    // id > Keep as thread > continue. Each source keeps its own journal witness;
    // another conversation cannot use those witnesses as implicit read authority.
    // 中文浏览器/API：执行两种读取，提供方重用同一调用标识 > 保留为会话 > 继续；
    // 各来源必须绑定自己的日志见证，其他对话不能借这些标识取得读取权限。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let requests = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            let mut seen = requests.lock().unwrap();
            let index = seen.len();
            seen.push(request.clone());
            let tool = match index {
                0 => Some(("docs", "permissions")),
                1 => Some(("find_operation", "TopicList")),
                _ => None,
            };
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: match tool {
                            Some((name, query)) => vec![GenerationPart::ToolCall {
                                id: "reused-provider-id".into(),
                                name: name.into(),
                                arguments: json!({"query":query}),
                            }],
                            None => vec![GenerationPart::Text {
                                text: "The checked sources remain readable".into(),
                            }],
                        },
                        continuation: vec![],
                    },
                    stop: if tool.is_some() {
                        GenerationStop::Tools
                    } else {
                        GenerationStop::Complete
                    },
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let source = create_unseeded(&s, &s.ada).await;
    let body = AskMessageBody {
        client_id: "witness-source".into(),
        context: None,
        model: None,
        content: vec![ContentPart::Text {
            text: "Read the guide and find its read operation".into(),
        }],
    };
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    eventually("the repeated-id reads finish", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(source.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(
        seen.lock().unwrap().len(),
        3,
        "reused provider ids must not collide with the tool journal's unique key"
    );
    let ids: Vec<Id> = sqlx::query_scalar("SELECT (payload->>'ask_witness')::uuid FROM items WHERE thread_id=$1 AND kind='tool_call' ORDER BY seq_created")
        .bind(source.conversation_id).fetch_all(s.rig.db.pool()).await.unwrap();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    let mut conn = s.rig.db.acquire().await.unwrap();
    let witnesses = orch_db::queries::ask::tool_witnesses(&mut conn, source.conversation_id, &ids)
        .await
        .unwrap();
    assert_eq!(witnesses.len(), 2);
    for (id, name) in ids.iter().zip(["docs", "find_operation"]) {
        let identity = id.to_string();
        assert_eq!(
            witnesses
                .iter()
                .find(|row| row.witness == identity)
                .unwrap()
                .name
                .as_deref(),
            Some(name)
        );
    }
    drop(conn);
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody {
            topic_id: Some(s.tree.topic.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let other = create_unseeded(&s, &s.ada).await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    assert_eq!(
        orch_db::queries::ask::tool_witnesses(&mut conn, kept.thread_id, &ids)
            .await
            .unwrap()
            .len(),
        2,
        "the authorized copy preserves logical witnesses even though it has new item ids"
    );
    assert!(
        orch_db::queries::ask::tool_witnesses(&mut conn, other.conversation_id, &ids)
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&kept.thread_id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    eventually("the exact copied sources remain available", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(kept.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 4);
    assert!(
        !serde_json::to_string(&seen[3].input)
            .unwrap()
            .contains("no longer available to you")
    );
}

#[tokio::test]
async fn another_builtin_sender_gets_visible_history_without_the_previous_persons_opaque_state() {
    // EN browser/API: Keep as thread > allow another person View and Interact
    // > continue as them. The answer text remains, without the former sender's
    // opaque model context, even with the same provider and model.
    // 中文浏览器/API：保留为会话 > 授予另一人查看及交互权限 > 由其继续；保留可读回答，
    // 即使提供方和模型相同，也不得重放前一位提问者的隐藏模型上下文。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let requests = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            requests.lock().unwrap().push(request.clone());
            let CommandResult::ProviderGenerate(mut result) = simple_answer() else {
                panic!("generation");
            };
            result.output.message.continuation = vec![json!({"opaque":"first-person-context"})];
            Ok(CommandResult::ProviderGenerate(result))
        });
    let source = create(&s, &s.ada, None).await;
    let body = AskMessageBody {
        client_id: "actor-context".into(),
        context: None,
        model: None,
        content: vec![ContentPart::Text {
            text: "Answer as the current person".into(),
        }],
    };
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    eventually("the first person answers", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(source.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody::default())
        .send()
        .await
        .ok();
    for permission in ["thread.view", "thread.interact"] {
        s.rig
            .lets_use("node", kept.thread_id, permission, &s.cai, json!({}))
            .await;
    }
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&kept.thread_id])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await
        .ok();
    eventually("the next actual sender answers", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(kept.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[1]
            .input
            .messages
            .iter()
            .all(|message| message.continuation.is_empty())
    );
    assert!(
        serde_json::to_string(&seen[1].input)
            .unwrap()
            .contains("A retained answer")
    );
}

#[tokio::test]
async fn conversation_files_are_private_without_an_environment() {
    // EN browser: Ask > Attach files > Choose files; owner opens it, another person is refused.
    // 中文浏览器：问一问 > 添加附件 > 选择文件；所有者可以打开，其他人不能读取。
    let s = Scene::new().await;
    let made = create(&s, &s.cai, None).await;
    let bytes = bytes::Bytes::from_static(b"a private conversation file");
    let meta = s
        .rig
        .world
        .app
        .app
        .blobs
        .put_bytes(bytes.clone())
        .await
        .unwrap();
    let now = s.rig.world.app.app.clock.now();
    let mut conn = s.rig.db.acquire().await.unwrap();
    s.rig
        .world
        .app
        .app
        .blobs
        .register(&mut conn, meta, now)
        .await
        .unwrap();
    orch_db::queries::uploads::remember(&mut conn, s.cai.id(), &meta.hash, now)
        .await
        .unwrap();
    drop(conn);
    let file = s
        .rig
        .world
        .app
        .call(AskAttachmentCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskAttachmentBody {
            blob: meta.hash,
            name: "notes.txt".into(),
        })
        .send()
        .await
        .ok();
    let got = s
        .rig
        .world
        .app
        .call(AskAttachmentGet, &[&made.conversation_id, &file.blob])
        .token(&s.cai.token)
        .send_bytes()
        .await;
    assert!(got.status.is_success());
    assert_eq!(got.raw, bytes);
    let refused = s
        .rig
        .world
        .app
        .call(AskAttachmentGet, &[&made.conversation_id, &file.blob])
        .token(&s.bea.token)
        .send_bytes()
        .await;
    assert!(refused.status.is_client_error());
    places::admin_mode(&s.rig.world, &s.ada, true).await;
    assert!(
        s.rig
            .world
            .app
            .call(AskAttachmentGet, &[&made.conversation_id, &file.blob])
            .token(&s.ada.token)
            .send_bytes()
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn named_read_tools_use_the_catalogue_and_refuse_a_different_actual_variant() {
    // EN: topic > Ask > request this topic and a thread's history; follow the
    // source. A request to inspect deleted data or change the topic is refused.
    // 中文：话题 > 问一问 > 读取话题和会话历史 > 打开来源；检查已删除内容或修改话题
    // 的请求应被拒绝。API assertions decide the actual producer and catalogue boundary.
    let s = Scene::new().await;
    let source = s.thread().await;
    let org = s.tree.org.base.id;
    let project = s.tree.project.base.id;
    let topic = s.tree.topic.base.id;
    let other_root: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
        .bind(s.ada.id())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let tools = [
        ("catalogue", "find_operation", json!({"query":"NodeGet"})),
        (
            "node-read",
            "read",
            json!({"operation":"NodeGet.read","parameters":{"path":{"id":topic}}}),
        ),
        (
            "node-plain",
            "read",
            json!({"operation":"NodeGet","parameters":{"path":{"id":topic}}}),
        ),
        (
            "node-manage",
            "read",
            json!({"operation":"NodeGet.inspect_deleted","parameters":{"path":{"id":topic}}}),
        ),
        (
            "node-switched",
            "read",
            json!({"operation":"NodeGet.read","parameters":{"path":{"id":topic},"query":{"inspect_deleted":true}}}),
        ),
        (
            "history",
            "history",
            json!({"reference":format!("thread:{}",source.base.id)}),
        ),
        (
            "org-reference",
            "history",
            json!({"reference":format!("org:{org}")}),
        ),
        (
            "project-reference",
            "history",
            json!({"reference":format!("project:{project}")}),
        ),
        (
            "topic-reference",
            "history",
            json!({"reference":format!("topic:{topic}")}),
        ),
        (
            "filtered-documents",
            "list",
            json!({"kind":"documents","place":topic,"filter":{"limit":1,"under":true}}),
        ),
        (
            "history-manage",
            "read",
            json!({"operation":"ThreadItemList.inspect_deleted","parameters":{"path":{"id":source.base.id}}}),
        ),
        (
            "write",
            "read",
            json!({"operation":"NodeUpdate","parameters":{"path":{"id":topic},"body":{"name":"A tool cannot rename this"}}}),
        ),
        (
            "other-root",
            "read",
            json!({"operation":"NodeGet.read","parameters":{"path":{"id":other_root}}}),
        ),
    ];
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            let mut seen = recorded.lock().unwrap();
            let first = seen.is_empty();
            seen.push(request.clone());
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: if first {
                            tools
                                .iter()
                                .map(|(id, name, arguments)| GenerationPart::ToolCall {
                                    id: (*id).into(),
                                    name: (*name).into(),
                                    arguments: arguments.clone(),
                                })
                                .collect()
                        } else {
                            vec![GenerationPart::Text {
                                text: "The permitted sources were checked.".into(),
                            }]
                        },
                        continuation: vec![],
                    },
                    stop: if first {
                        GenerationStop::Tools
                    } else {
                        GenerationStop::Complete
                    },
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let made = create_unseeded(&s, &s.ada).await;
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "named-reads".into(),
            content: vec![ContentPart::Text {
                text: "Read the permitted sources".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually(
        "the named reads finish through the native handlers",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        },
    )
    .await;
    let unchanged: bool = sqlx::query_scalar(
        "SELECT name IS DISTINCT FROM 'A tool cannot rename this' FROM nodes WHERE id=$1",
    )
    .bind(topic)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert!(unchanged);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let results: std::collections::HashMap<_, _> = seen[1]
        .input
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|part| match part {
            GenerationPart::ToolResult {
                call_id,
                content,
                is_error,
                ..
            } => Some((call_id.as_str(), (content, *is_error))),
            _ => None,
        })
        .collect();
    assert!(!results["node-read"].1, "a real named Read succeeds");
    assert!(results["node-read"].0.contains(&topic.to_string()));
    let source: Json = serde_json::from_str(results["node-read"].0).unwrap();
    assert!(
        source["citations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|citation| citation["url"] == format!("/topic/{topic}")
                && citation["title"].is_string())
    );
    assert!(
        !results["history"].1,
        "the general history tool derives the same named Read"
    );
    for (reference, id) in [
        ("org-reference", org),
        ("project-reference", project),
        ("topic-reference", topic),
    ] {
        assert!(
            !results[reference].1,
            "{reference} reaches its actual view reader"
        );
        assert!(results[reference].0.contains(&id.to_string()));
    }
    assert!(
        !results["filtered-documents"].1,
        "a session read must use the real complete native list producer"
    );
    let listed: Json = serde_json::from_str(results["filtered-documents"].0).unwrap();
    assert!(
        listed["data"]["items"].as_array().unwrap().len() <= 1,
        "the native filter is applied before delivery"
    );
    for refused in [
        "node-plain",
        "node-manage",
        "node-switched",
        "history-manage",
        "write",
        "other-root",
    ] {
        assert!(
            results[refused].1,
            "{refused} cannot borrow the selected Read"
        );
    }
    assert!(!results["catalogue"].1);
    let catalogue: Json = serde_json::from_str(results["catalogue"].0).unwrap();
    let operations = catalogue["operations"].as_array().unwrap();
    assert!(
        operations
            .iter()
            .any(|entry| entry["operation"] == "NodeGet.read")
    );
    assert!(
        operations
            .iter()
            .all(|entry| entry["operation"] != "NodeGet"
                && entry["operation"] != "NodeGet.inspect_deleted")
    );
    assert!(
        operations
            .iter()
            .all(|entry| entry["schemas"]["query"].is_object())
    );
}

#[tokio::test]
async fn resuming_replaces_a_revoked_preview_before_the_model_sees_it() {
    // EN browser: ask about a privately shared topic, revoke that share, then
    // resume; the old source becomes "no longer available to you" for the model.
    // 中文浏览器：询问私下共享的话题，撤销共享后继续提问；模型应收到“已无法读取”的来源状态。
    use orch_api_contract::nodes::{NewNodeAccess, TopicCreate, TopicCreateBody, TopicCreateReply};
    let s = Scene::new().await;
    let source = s
        .rig
        .world
        .app
        .call(TopicCreate, &[&s.tree.project.base.id])
        .token(&s.ada.token)
        .body(&TopicCreateBody {
            name: Some("Private source marker".into()),
            access: Some(NewNodeAccess::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let TopicCreateReply::Topic(source) = source else {
        panic!("one topic was created");
    };
    let source_id = source.base.id;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let role = orch_db::queries::access::testing::give_role(
        &mut conn,
        source_id,
        "user",
        s.cai.id(),
        "viewer",
        s.ada.id(),
        None,
    )
    .await
    .unwrap();
    drop(conn);
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            let mut seen = recorded.lock().unwrap();
            let first = seen.is_empty();
            seen.push(request.clone());
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: if first {
                            vec![GenerationPart::ToolCall {
                                id: "source".into(),
                                name: "open".into(),
                                arguments: json!({"reference":format!("topic:{source_id}")}),
                            }]
                        } else {
                            vec![GenerationPart::Text {
                                text: "Answer".into(),
                            }]
                        },
                        continuation: vec![],
                    },
                    stop: if first {
                        GenerationStop::Tools
                    } else {
                        GenerationStop::Complete
                    },
                },
                usage: orch_api_contract::threads::TokenUsage::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let made = create(&s, &s.cai, None).await;
    for number in 0..2 {
        s.rig
            .world
            .app
            .call(AskMessageCreate, &[&made.conversation_id])
            .token(&s.cai.token)
            .body(&AskMessageBody {
                client_id: format!("question-{number}"),
                content: vec![ContentPart::Text {
                    text: "Read the shared source".into(),
                }],
                context: None,
                model: None,
            })
            .send()
            .await
            .ok();
        eventually("the Ask turn finishes", || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        })
        .await;
        if number == 0 {
            assert!(
                serde_json::to_string(&seen.lock().unwrap()[1].input)
                    .unwrap()
                    .contains("Private source marker")
            );
            let mut conn = s.rig.db.acquire().await.unwrap();
            orch_db::queries::access::testing::take_role_away(
                &mut conn,
                role,
                s.ada.id(),
                s.rig.world.app.app.clock.now(),
            )
            .await
            .unwrap();
        }
    }
    let seen = seen.lock().unwrap();
    let resumed = serde_json::to_string(&seen.last().unwrap().input).unwrap();
    assert!(resumed.contains("no longer available to you"));
    assert!(!resumed.contains("Private source marker"));
}

#[tokio::test]
async fn personal_storage_does_not_mix_another_ask_knowledge_root_into_a_turn() {
    // EN browser: keep one Ask scoped to the personal org and one to a team.
    // Ask the personal conversation to read the team's Ask; it must refuse.
    // 中文浏览器：分别创建个人组织和团队组织的提问；从个人对话读取团队提问应被拒绝。
    let s = Scene::new().await;
    let foreign = create(&s, &s.cai, None).await;
    assert_eq!(
        read(&s, &s.cai, foreign.conversation_id)
            .await
            .conversation
            .root_org_id,
        s.tree.org.base.id
    );
    let personal: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
        .bind(s.cai.id())
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let provider = configured_provider(&s).await;
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&personal, &"ask.models"])
        .token(&s.cai.token)
        .body(&NodeSettingSetBody {
            value: json!([{ "provider_id":provider,"model":MODEL }]),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let made = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.cai.token)
        .body(&AskCreateBody {
            root_org_id: Some(personal),
            context: None,
            retention_days: None,
            model: Some(AskModel {
                provider_id: provider,
                model: MODEL.into(),
                params: None,
            }),
        })
        .send()
        .await
        .ok();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    s.rig.machine.handler.on(CommandKind::ProviderGenerate, move |command| {
        let Command::ProviderGenerate(request) = command else { panic!("generation"); };
        let mut seen = captured.lock().unwrap(); let first = seen.is_empty(); seen.push(request.clone());
        Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
            native_result_blob: None,
            output: GenerationOutput {
                native_result: serde_json::Value::Null, message: GenerationMessage { role: GenerationRole::Assistant,
                content: if first { vec![GenerationPart::ToolCall { id: "other-conversation".into(), name: "read".into(),
                    arguments: json!({"operation":"AskGet","parameters":{"path":{"id":foreign.conversation_id}}}) }] }
                    else { vec![GenerationPart::Text { text: "That source is outside this conversation.".into() }] },
                continuation: vec![] }, stop: if first { GenerationStop::Tools } else { GenerationStop::Complete } },
            usage: Default::default(), timing: Default::default(), request_ref: None,
        }))
    });
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "question".into(),
            content: vec![ContentPart::Text {
                text: "Read the other conversation".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the root-bound Ask finishes", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[1]
            .input
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|part| matches!(part, GenerationPart::ToolResult { is_error: true, .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_use_revoked_during_startup_prevents_a_paid_ask_request() {
    // EN browser: Send an Ask while its provider starts, revoke its Use entry,
    // then let startup finish; the turn fails before any model request is sent.
    // 中文浏览器：发送提问后，在提供方启动期间撤销使用权限；启动结束后，不得发起付费模型请求。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let entry: Id = sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='provider' AND object_id=$1 AND page='live' AND permission='provider.use' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL")
        .bind(provider).bind(s.cai.id()).fetch_one(s.rig.db.pool()).await.unwrap();
    let db = s.rig.db.clone();
    let clock = s.rig.world.app.app.clock.clone();
    let giver = s.ada.id();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderStart, move |_| {
            // This callback models the owner revoking during the genuine startup
            // await. Its database future gets a runtime worker outside this sync handler.
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let mut conn = db.acquire().await.unwrap();
                    orch_db::queries::access::remove_entry(&mut conn, entry, giver, clock.now())
                        .await
                        .unwrap();
                })
            });
            Ok(CommandResult::ProviderStart(Empty {}))
        });
    let calls = Arc::new(Mutex::new(0));
    let seen = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            *seen.lock().unwrap() += 1;
            panic!("a revoked Use must not reach the paid generation command");
        });
    let made = create(&s, &s.cai, None).await;
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "question".into(),
            content: vec![ContentPart::Text {
                text: "Question".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("startup revocation settles the Ask", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(*calls.lock().unwrap(), 0);
    let ended: String = sqlx::query_scalar(
        "SELECT state FROM turns WHERE thread_id=$1 ORDER BY ordinal DESC LIMIT 1",
    )
    .bind(made.conversation_id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(ended, "failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn losing_the_keys_history_read_during_startup_prevents_paid_generation() {
    // EN browser/API: start a question with a permission key, remove its
    // Thread View during provider startup, then finish startup. No model call
    // may use the old key's history authority.
    // 中文浏览器/API：使用权限密钥开始提问，在提供方启动期间撤销其线程查看权限，再完成启动；
    // 不得使用密钥原先的历史读取权限发起模型调用。
    use orch_api_core::middleware::authentication::token_hash;
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let made = create(&s, &s.ada, None).await;
    let key = permission_key(
        &s,
        &[
            "org.view",
            "thread.view",
            "thread.interact",
            "provider.view",
            "provider.use",
        ],
    )
    .await;
    let token: Id = sqlx::query_scalar("SELECT id FROM api_tokens WHERE token_hash=$1")
        .bind(token_hash(&key))
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let db = s.rig.db.clone();
    s.rig.machine.handler.on(CommandKind::ProviderStart, move |_| {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async {
            sqlx::query("UPDATE api_tokens SET scopes=ARRAY['org.view','thread.interact','provider.view','provider.use'] WHERE id=$1")
                .bind(token).execute(db.pool()).await.unwrap();
        }));
        Ok(CommandResult::ProviderStart(Empty {}))
    });
    let requests = Arc::new(Mutex::new(0));
    let paid = requests.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            *paid.lock().unwrap() += 1;
            panic!("a key without current history Read cannot submit a paid question");
        });
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&key)
        .body(&AskMessageBody {
            client_id: "history-revoked".into(),
            content: vec![ContentPart::Text {
                text: "Question".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the revoked history key ends its accepted turn", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(*requests.lock().unwrap(), 0);
    let ended: String = sqlx::query_scalar(
        "SELECT state FROM turns WHERE thread_id=$1 ORDER BY ordinal DESC LIMIT 1",
    )
    .bind(made.conversation_id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(ended, "failed");
}

#[tokio::test]
async fn owning_an_ask_does_not_bypass_a_permission_keys_native_ceiling() {
    // EN browser/API: read your conversation through a read-only permission
    // key; Send, Stop, preferences, Delete and Export stay outside its grant.
    // A key with the actual Interact permission can still Stop.
    // 中文浏览器/API：通过只读权限密钥读取自己的对话；发送、停止、偏好、删除和导出均不得
    // 越过密钥范围；具有实际交互权限的密钥仍能停止。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let made = create(&s, &s.ada, None).await;
    let boundary_only = permission_key(&s, &["org.view"]).await;
    let no_thread_read = s
        .rig
        .world
        .app
        .call(AskGet, &[&made.conversation_id])
        .token(&boundary_only)
        .send()
        .await;
    assert_eq!(no_thread_read.status.as_u16(), 403);
    let hidden = s
        .rig
        .world
        .app
        .call(AskList, &[])
        .token(&boundary_only)
        .query(&AskListQuery::default())
        .send()
        .await
        .ok();
    assert!(hidden.items.is_empty());
    let reader = permission_key(&s, &["org.view", "thread.view", "provider.use"]).await;
    s.rig
        .world
        .app
        .call(AskGet, &[&made.conversation_id])
        .token(&reader)
        .send()
        .await
        .ok();
    let readable = s
        .rig
        .world
        .app
        .call(AskList, &[])
        .token(&reader)
        .query(&AskListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(readable.items.len(), 1);
    let send = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&reader)
        .body(&AskMessageBody {
            client_id: "not-an-interactor".into(),
            content: vec![ContentPart::Text {
                text: "The owner's key still needs Interact".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await;
    assert_eq!(send.status.as_u16(), 403);
    let stop = s
        .rig
        .world
        .app
        .call(AskStop, &[&made.conversation_id])
        .token(&reader)
        .send()
        .await;
    assert_eq!(stop.status.as_u16(), 403);
    let update = s
        .rig
        .world
        .app
        .call(AskUpdate, &[&made.conversation_id])
        .token(&reader)
        .body(&AskUpdateBody {
            shown: Some(true),
            ..Default::default()
        })
        .send()
        .await;
    assert_eq!(update.status.as_u16(), 403);
    let export = s
        .rig
        .world
        .app
        .call(AskExport, &[&made.conversation_id])
        .token(&reader)
        .send_bytes()
        .await;
    assert_eq!(export.status.as_u16(), 403);
    let delete = s
        .rig
        .world
        .app
        .call(AskDelete, &[&made.conversation_id])
        .token(&reader)
        .send()
        .await;
    assert_eq!(delete.status.as_u16(), 403);
    let interactor = permission_key(&s, &["org.view", "thread.view", "thread.interact"]).await;
    s.rig
        .world
        .app
        .call(AskStop, &[&made.conversation_id])
        .token(&interactor)
        .send()
        .await
        .ok();
    let requests: i64 = sqlx::query_scalar("SELECT count(*) FROM ask_calls WHERE thread_id=$1")
        .bind(made.conversation_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(requests, 0);
    assert!(
        !read(&s, &s.ada, made.conversation_id)
            .await
            .conversation
            .shown
    );
}

#[tokio::test]
async fn builtin_input_needs_the_original_keys_read_of_retained_history() {
    // EN browser/API: with a key allowing only input Write, try Send through
    // Ask and the ordinary thread composer. Both refuse before model use;
    // giving a key Write never gives it the retained conversation's Read.
    // 中文浏览器/API：仅允许输入写入的密钥分别通过问一问和普通线程发送；
    // 两者均应在调用模型前拒绝，写入权限不会授予历史对话的读取权限。
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_contract::threads::{Delivery, ThreadInputCreate, ThreadInputCreateBody};
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity;
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let made = create(&s, &s.ada, None).await;
    let mut accepted_input = None;
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, |_| {
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![GenerationPart::Text {
                            text: "Read and Write were both admitted.".into(),
                        }],
                        continuation: vec![],
                    },
                    stop: GenerationStop::Complete,
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    for (operation, actions) in [
        ("AskMessageCreate", vec!["write"]),
        ("ThreadInputCreate", vec!["write"]),
        ("ThreadInputCreate", vec!["read", "write"]),
    ] {
        let scope: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"thread","id":made.conversation_id,"reach":"self"},
            "allow":[{"operation":operation,"actions":actions}]
        }]}))
        .unwrap();
        assert!(scope.validate().is_empty());
        let secret = new_token(API_TOKEN_PREFIX);
        let hash = token_hash(&secret);
        let encoded = json!(scope);
        identity::create_api_token_scoped(
            &mut *s.rig.world.conn().await,
            &identity::NewApiToken {
                id: new_id(),
                user_id: s.ada.id(),
                name: "Built-in input scope fixture",
                token_hash: &hash,
                scopes: &[],
                node_id: None,
                at: s.rig.world.now(),
                expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
            },
            Some(&encoded),
        )
        .await
        .unwrap();
        let input_body = ThreadInputCreateBody {
            client_id: operation.into(),
            content: vec![ContentPart::Text {
                text: "Do not disclose history".into(),
            }],
            delivery: Delivery::Queue,
            native_action: None,
            bypass: None,
            plan: false,
        };
        let reply = if operation == "AskMessageCreate" {
            s.rig
                .world
                .app
                .call(AskMessageCreate, &[&made.conversation_id])
                .token(&secret)
                .body(&AskMessageBody {
                    client_id: operation.into(),
                    content: vec![ContentPart::Text {
                        text: "Do not disclose history".into(),
                    }],
                    context: None,
                    model: None,
                })
                .send_bytes()
                .await
        } else {
            s.rig
                .world
                .app
                .call(ThreadInputCreate, &[&made.conversation_id])
                .token(&secret)
                .body(&input_body)
                .send_bytes()
                .await
        };
        if actions.contains(&"read") {
            assert_eq!(reply.status.as_u16(), 201);
            let body: orch_api_contract::threads::ThreadInputCreateResponse =
                serde_json::from_slice(&reply.raw).unwrap();
            let first = body.into_full().unwrap();
            let replayed = s
                .rig
                .world
                .app
                .call(ThreadInputCreate, &[&made.conversation_id])
                .token(&secret)
                .body(&input_body)
                .send_bytes()
                .await;
            assert_eq!(replayed.status.as_u16(), 200);
            let replayed: orch_api_contract::threads::ThreadInputCreateResponse =
                serde_json::from_slice(&replayed.raw).unwrap();
            assert_eq!(replayed.into_full().unwrap().input.id, first.input.id);
            accepted_input = Some(first.input.id);
        } else {
            assert_eq!(
                reply.status.as_u16(),
                403,
                "{operation} must keep its required history Read"
            );
            let accepted: i64 =
                sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
                    .bind(made.conversation_id)
                    .fetch_one(s.rig.db.pool())
                    .await
                    .unwrap();
            assert_eq!(accepted, 0);
            let paid: i64 = sqlx::query_scalar("SELECT count(*) FROM ask_calls WHERE thread_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
            assert_eq!(paid, 0);
        }
    }
    assert!(accepted_input.is_some());
    eventually(
        "the same key with required history Read answers",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        },
    )
    .await;
    let requests: i64 = sqlx::query_scalar("SELECT count(*) FROM ask_calls WHERE thread_id=$1")
        .bind(made.conversation_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(requests, 1);
}

#[tokio::test]
async fn a_creation_key_can_make_a_genuine_empty_builtin_thread_without_readback_or_environment() {
    // EN browser/API: create a built-in thread in a topic under a creation-only
    // key; receive its identity, retry with the same act, then open as its owner.
    // 中文浏览器/API：用仅创建权限的密钥在话题中新建内置线程，取得标识后按同一操作重试，
    // 再以拥有者打开；不得要求环境、智能体或额外的读取权限。
    use orch_api_contract::nodes::{
        CreateThread, CreateThreadProgram, ThreadCreate, ThreadCreateResponse,
    };
    use orch_api_contract::threads::BuiltinThreadProgramId;
    let s = Scene::new().await;
    let key = scoped_key(
        &s,
        json!({"rules":[{
            "target":{"kind":"topic","id":s.tree.topic.base.id,"reach":"self"},
            "resources":["thread"],"allow":[{"operation":"ThreadCreate","actions":["write"]}]
        }]}),
    )
    .await;
    let body = CreateThread {
        name: Some("Built-in question".into()),
        program: Some(CreateThreadProgram {
            id: BuiltinThreadProgramId::Orch,
            model: None,
        }),
        ..Default::default()
    };
    let act = new_id().to_string();
    let first = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&key)
        .header("Idempotency-Key", &act)
        .body(&body)
        .send()
        .await
        .ok();
    let ThreadCreateResponse::Receipt(first) = first else {
        panic!("A creation-only key receives a native identity receipt");
    };
    let second = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&key)
        .header("Idempotency-Key", &act)
        .body(&body)
        .send()
        .await
        .ok();
    let ThreadCreateResponse::Receipt(second) = second else {
        panic!("Readback is still outside this key");
    };
    assert_eq!(first, second);
    let node = places::read(&s.rig.world, &s.ada, first.thread_id).await;
    let orch_api_contract::nodes::Node::Thread(thread) = node else {
        panic!("thread");
    };
    assert_eq!(
        thread.program.as_ref().unwrap().root_org_id,
        s.tree.org.base.id
    );
    assert!(thread.environment.is_none());
    assert!(thread.binding.is_none());
    assert!(
        thread.current_segment_id.is_none(),
        "an unused program has no invented actual model segment"
    );
}

#[tokio::test]
async fn a_no_environment_program_refuses_workspace_choices_before_runtime_or_birth_effects() {
    // EN browser/API: choose the built-in read-only program with an explicit
    // workspace. Creation explains the incompatible choice without starting
    // a descriptor/provider or creating a thread, environment or job.
    // 中文浏览器/API：选择内置只读程序并指定工作区；创建应说明选择不兼容，
    // 不启动程序探测或提供方，也不创建线程、环境或任务。
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, CreateThreadProgram, ThreadCreate};
    use orch_api_contract::threads::BuiltinThreadProgramId;
    use orch_link_contract::commands::{CommandError, ErrorCode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let s = Scene::new().await;
    let starts = Arc::new(AtomicUsize::new(0));
    for kind in [
        CommandKind::AgentStart,
        CommandKind::ProviderStart,
        CommandKind::ProviderGenerate,
    ] {
        let started = starts.clone();
        s.rig.machine.handler.on(kind, move |_| {
            started.fetch_add(1, Ordering::SeqCst);
            Err(CommandError::new(
                ErrorCode::Conflict,
                "An incompatible program must not start a runtime",
            ))
        });
    }
    let before: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM nodes),(SELECT count(*) FROM threads),(SELECT count(*) FROM environments),(SELECT count(*) FROM jobs)",
    ).fetch_one(s.rig.db.pool()).await.unwrap();
    let reply = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            program: Some(CreateThreadProgram {
                id: BuiltinThreadProgramId::Orch,
                model: None,
            }),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            ..Default::default()
        })
        .send()
        .await;
    assert_eq!(reply.status.as_u16(), 400);
    assert_eq!(reply.problem().extra["errors"][0]["path"], "environment");
    let after: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM nodes),(SELECT count(*) FROM threads),(SELECT count(*) FROM environments),(SELECT count(*) FROM jobs)",
    ).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(after, before);
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_builtin_birth_commits_its_first_question_before_generation_and_replays_once() {
    // EN browser/API: New thread > built-in program > Private > first question
    // > Create. The first question answers without a workspace. Retry the same
    // creation after a lost result; it remains one thread and one question.
    // 中文浏览器/API：新建线程 > 内置程序 > 私有 > 首个问题 > 创建；无需工作区即可回答。
    // 回答丢失后按同一创建操作重试，仍只有一个线程、一个问题。
    use orch_api_contract::access::{AccessPreset, ThreadAccess};
    use orch_api_contract::nodes::{CreateThread, CreateThreadProgram, FirstInput, ThreadCreate};
    use orch_api_contract::threads::BuiltinThreadProgramId;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let visible = Arc::new(AtomicBool::new(false));
    let (paid, committed) = (calls.clone(), visible.clone());
    let pool = s.rig.db.pool().clone();
    let person = s.ada.id();
    s.rig.machine.handler.on(CommandKind::ProviderGenerate, move |_| {
        paid.fetch_add(1, Ordering::SeqCst);
        let found: bool = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs i JOIN threads t ON t.node_id=i.thread_id WHERE i.client_id='native-born-first' AND i.author_id=$1 AND t.program='orch' AND i.turn_id=t.current_turn_id)")
                .bind(person).fetch_one(&pool).await.unwrap()
        }));
        committed.store(found, Ordering::SeqCst);
        Ok(simple_answer())
    });
    let body = CreateThread {
        name: Some("Built-in thread".into()),
        access: Some(ThreadAccess::Preset(AccessPreset::Private)),
        program: Some(CreateThreadProgram {
            id: BuiltinThreadProgramId::Orch,
            model: None,
        }),
        first_input: Some(FirstInput {
            client_id: "native-born-first".into(),
            content: vec![ContentPart::Text {
                text: "What can I read in this organization?".into(),
            }],
        }),
        ..Default::default()
    };
    let act = new_id().to_string();
    let made = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .header("Idempotency-Key", &act)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    eventually(
        "the native-born program answers its first question",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.base.id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        },
    )
    .await;
    assert!(
        visible.load(Ordering::SeqCst),
        "the model callback sees the committed thread and first input"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(made.environment.is_none());
    assert!(made.binding.is_none());
    assert_eq!(
        made.program.as_ref().unwrap().root_org_id,
        s.tree.org.base.id
    );
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE node_id=$1")
        .bind(made.base.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        jobs, 0,
        "a control-plane program needs no native creation job"
    );
    let replay = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .header("Idempotency-Key", &act)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(replay.base.id, made.base.id);
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(made.base.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(inputs, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let mut invalid = body.clone();
    invalid.first_input.as_mut().unwrap().client_id.clear();
    let refused = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&invalid)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 400);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        after, before,
        "a refused first input rolls back the thread and its discussion"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a refused birth never launches its controller"
    );
}

#[tokio::test]
async fn ask_message_pages_skip_nonmessages_without_losing_the_boundary_message() {
    // EN browser/API: open a long Ask > Load earlier messages until the start;
    // each retained message appears once, even with many intervening tool/marker rows.
    // 中文浏览器/API：打开较长提问记录 > 反复加载更早消息至开头；即使夹有大量工具和标记行，
    // 每条保留的消息也必须恰好出现一次，不能跳过分页边界。
    use orch_db::queries::threads as journal;
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, |_| Ok(simple_answer()));
    let source = create_unseeded(&s, &s.cai).await;
    let accepted = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&source.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "page-source".into(),
            context: None,
            model: None,
            content: vec![ContentPart::Text {
                text: "Start retained history".into(),
            }],
        })
        .send()
        .await
        .ok();
    eventually("the paging fixture has actual recorded history", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(source.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let mut expected: Vec<Id> = read(&s, &s.cai, source.conversation_id)
        .await
        .messages
        .into_iter()
        .map(|message| message.id)
        .collect();
    let segment: Id = sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id=$1")
        .bind(source.conversation_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let mut tx = s.rig.db.pool().begin().await.unwrap();
    // Extend the native journal to isolate cursor behavior, without dozens of paid requests.
    for index in 0..53 {
        for message in [true, false] {
            let id = new_id();
            let seq = orch_db::sequences::next(&mut tx, source.conversation_id)
                .await
                .unwrap()
                .unwrap();
            let text = format!("Retained message {index}");
            let payload = if message {
                json!({"text":text,"text_len":text.chars().count(),"phase":"final"})
            } else {
                json!({"marker":"history_note","data":{}})
            };
            journal::insert_item(
                &mut tx,
                &journal::NewItem {
                    id,
                    thread_id: source.conversation_id,
                    segment_id: segment,
                    turn_id: accepted.turn_id,
                    stream: "main",
                    parent_item_id: None,
                    kind: if message { "agent_message" } else { "marker" },
                    status: "completed",
                    author_id: None,
                    payload: &payload,
                    native_ref: None,
                    search_text: None,
                    at: s.rig.world.now(),
                },
                seq,
            )
            .await
            .unwrap();
            if message {
                expected.push(id);
            }
        }
    }
    tx.commit().await.unwrap();
    let mut cursor = None;
    let mut found = std::collections::HashSet::new();
    for _ in 0..=expected.len() {
        let page = s
            .rig
            .world
            .app
            .call(AskGet, &[&source.conversation_id])
            .token(&s.cai.token)
            .query(&AskGetQuery {
                before: cursor.clone(),
                limit: Some(10),
            })
            .send()
            .await
            .ok();
        assert!(page.messages.len() <= 10);
        assert!(!page.messages.is_empty());
        for message in page.messages {
            assert!(
                found.insert(message.id),
                "a page must not repeat a boundary message"
            );
        }
        match page.previous_cursor {
            Some(next) => {
                assert_ne!(cursor.as_ref(), Some(&next));
                cursor = Some(next);
            }
            None => break,
        }
    }
    assert_eq!(found.len(), expected.len());
    assert!(expected.into_iter().all(|id| found.contains(&id)));
}

#[tokio::test]
async fn ask_snapshot_model_and_export_use_the_native_selected_history() {
    // EN browser/API: answer twice > select a continuation before the second
    // question > read history, continue and Export. The selected prefix remains;
    // the inactive suffix is absent from the panel, model input and export.
    // 中文浏览器/API：回答两次 > 选择第二个问题之前的续接版本 > 读取、继续并导出；
    // 保留所选前缀，面板、模型输入和导出中均不得出现未选中的后缀。
    use orch_db::queries::{thread_continuations as continuations, threads as journal};
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let requests = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            let mut seen = requests.lock().unwrap();
            let index = seen.len();
            seen.push(request.clone());
            let CommandResult::ProviderGenerate(mut result) = simple_answer() else {
                panic!("generation");
            };
            result.output.message.content = vec![GenerationPart::Text {
                text: match index {
                    0 => "Selected prefix answer",
                    1 => "Inactive suffix canary",
                    _ => "New selected answer",
                }
                .into(),
            }];
            Ok(CommandResult::ProviderGenerate(result))
        });
    let source = create_unseeded(&s, &s.cai).await;
    let mut turns = Vec::new();
    for index in 0..2 {
        let result = s
            .rig
            .world
            .app
            .call(AskMessageCreate, &[&source.conversation_id])
            .token(&s.cai.token)
            .body(&AskMessageBody {
                client_id: format!("selected-source-{index}"),
                context: None,
                model: None,
                content: vec![ContentPart::Text {
                    text: if index == 0 {
                        "Selected first question"
                    } else {
                        "Inactive suffix canary"
                    }
                    .into(),
                }],
            })
            .send()
            .await
            .ok();
        turns.push(result.turn_id.unwrap());
        eventually("the selected-history source finishes", || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(source.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        })
        .await;
    }
    let mut tx = s.rig.db.pool().begin().await.unwrap();
    let row = journal::thread(&mut tx, source.conversation_id)
        .await
        .unwrap()
        .unwrap();
    let second = journal::turn(&mut tx, turns[1]).await.unwrap().unwrap();
    let boundary: i64 = sqlx::query_scalar(
        "SELECT min(seq_created) FROM items WHERE turn_id=$1 AND kind='user_message'",
    )
    .bind(second.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let seq = orch_db::sequences::next(&mut tx, row.node_id)
        .await
        .unwrap()
        .unwrap();
    let view = new_id();
    continuations::insert(
        &mut tx,
        &continuations::NewContinuation {
            id: view,
            thread: row.node_id,
            parent: row.active_continuation_id,
            before_input: second.input_id,
            actor: s.cai.id(),
            at: s.rig.world.now(),
            seq,
            queue: continuations::QueueRow {
                held: row.queue_held,
                held_by: row.queue_held_by,
                held_at: row.queue_held_at,
                revision: row.queue_revision,
            },
        },
    )
    .await
    .unwrap();
    continuations::freeze_prefix(
        &mut tx,
        row.node_id,
        row.active_continuation_id,
        view,
        second.ordinal,
        boundary,
        seq,
    )
    .await
    .unwrap();
    // Model a completed native selection without pretending the native harness edit API serves this program.
    sqlx::query("UPDATE thread_continuations SET status='ready' WHERE id=$1")
        .bind(view)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET active_continuation_id=$2,execution_revision=execution_revision+1,current_segment_id=NULL,seq_state=$3 WHERE node_id=$1")
        .bind(row.node_id).bind(view).bind(seq).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let snapshot = read(&s, &s.cai, source.conversation_id).await;
    let snapshot_text = serde_json::to_string(&snapshot.messages).unwrap();
    assert!(snapshot_text.contains("Selected prefix answer"));
    assert!(!snapshot_text.contains("Inactive suffix canary"));
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&source.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "continue-selected".into(),
            context: None,
            model: None,
            content: vec![ContentPart::Text {
                text: "Continue this selected line".into(),
            }],
        })
        .send()
        .await
        .ok();
    eventually("the selected program continues", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(source.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        let model = serde_json::to_string(&seen[2].input).unwrap();
        assert!(model.contains("Selected prefix answer"));
        assert!(!model.contains("Inactive suffix canary"));
    }
    let exported = s
        .rig
        .world
        .app
        .call(AskExport, &[&source.conversation_id])
        .token(&s.cai.token)
        .send_bytes()
        .await;
    assert_eq!(exported.status.as_u16(), 200);
    let exported = String::from_utf8(exported.raw.to_vec()).unwrap();
    assert!(exported.contains("Selected prefix answer"));
    assert!(!exported.contains("Inactive suffix canary"));
}

#[tokio::test]
async fn permanent_builtin_forks_use_native_source_and_destination_rights_and_the_actual_creator() {
    // EN browser/API: Keep as thread > share View and Fork with another person
    // who may create in the destination > fork as that person. The new Private
    // thread is theirs and continues without an environment; the source stays open.
    // 中文浏览器/API：保留为会话 > 向可在目标位置创建的另一人授予查看和派生权限 > 以该人派生；
    // 新私有线程属于实际创建者，无需环境即可继续，来源线程保持可用。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let calls = Arc::new(Mutex::new(0));
    let paid = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            *paid.lock().unwrap() += 1;
            Ok(simple_answer())
        });
    let source = create(&s, &s.ada, None).await;
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "before-fork".into(),
            content: vec![ContentPart::Text {
                text: "Retain this answer".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the fork source answers", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(source.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let read_rule = json!({"target":{"kind":"thread","id":source.conversation_id,"reach":"self"},
        "allow":[{"operation":"AskKeep","actions":["read"]}]});
    let reader = scoped_key(&s, json!({"rules":[read_rule.clone()]})).await;
    let missing_destination = s
        .rig
        .world
        .app
        .call(AskKeep, &[&source.conversation_id])
        .token(&reader)
        .body(&AskKeepBody::default())
        .send()
        .await;
    assert_eq!(missing_destination.status.as_u16(), 403);
    let creator = scoped_key(
        &s,
        json!({"rules":[read_rule,
        {"target":{"kind":"topic","id":s.tree.topic.base.id,"reach":"self"},"resources":["thread"],
            "allow":[{"operation":"AskKeep","actions":["write"]}]}]}),
    )
    .await;
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&source.conversation_id])
        .token(&creator)
        .body(&AskKeepBody::default())
        .send()
        .await
        .ok();
    places::give_role(
        &s.rig.world,
        s.tree.project.base.id,
        &s.cai,
        "developer",
        &s.ada,
    )
    .await;
    for permission in ["thread.view", "thread.fork"] {
        s.rig
            .lets_use("node", kept.thread_id, permission, &s.cai, json!({}))
            .await;
    }
    let fork_key = permission_key_for(
        &s,
        s.cai.id(),
        &[
            "org.view",
            "thread.view",
            "thread.fork",
            "topic.start_threads",
            "project.create_topics",
            "access.manage",
            "access.own",
        ],
    )
    .await;
    let no_driving = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&kept.thread_id])
        .token(&fork_key)
        .body(&AskMessageBody {
            client_id: "fork-key-does-not-drive".into(),
            content: vec![ContentPart::Text {
                text: "No Interact was granted".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await;
    assert_eq!(no_driving.status.as_u16(), 403);
    let fork = s
        .rig
        .world
        .app
        .call(AskKeep, &[&kept.thread_id])
        .token(&fork_key)
        .body(&AskKeepBody::default())
        .send()
        .await
        .ok();
    assert_ne!(fork.thread_id, kept.thread_id);
    assert_eq!(fork.topic_id, kept.topic_id);
    let node = places::read(&s.rig.world, &s.cai, fork.thread_id).await;
    let orch_api_contract::nodes::Node::Thread(node) = node else {
        panic!("thread");
    };
    assert_eq!(node.base.owner.id, s.cai.id());
    assert_eq!(
        node.program.as_ref().unwrap().root_org_id,
        s.tree.org.base.id
    );
    assert!(node.environment.is_none());
    assert!(node.binding.is_none());
    let (state, linked): (String, Option<Id>) =
        sqlx::query_as("SELECT ask_state,ask_kept_thread_id FROM threads WHERE node_id=$1")
            .bind(kept.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(state, "ready");
    assert!(linked.is_none());
    assert_eq!(
        *calls.lock().unwrap(),
        1,
        "copying history makes no model request"
    );
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&fork.thread_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "after-fork".into(),
            content: vec![ContentPart::Text {
                text: "Continue as the new person".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the new owner continues the built-in fork", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(fork.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(*calls.lock().unwrap(), 2);
    let made_topic = s
        .rig
        .world
        .app
        .call(AskKeep, &[&kept.thread_id])
        .token(&s.cai.token)
        .body(&AskKeepBody {
            project_id: Some(s.tree.project.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let topic = places::read(&s.rig.world, &s.cai, made_topic.topic_id).await;
    let orch_api_contract::nodes::Node::Topic(topic) = topic else {
        panic!("topic");
    };
    assert_eq!(topic.base.owner.id, s.cai.id());
    assert!(!topic.base.chat_id.is_nil());
}

#[tokio::test]
async fn builtin_fork_from_here_copies_only_visible_selected_history_and_its_files() {
    // EN browser: Ask > send a question with early.txt > send another with later.txt
    // > Keep as thread > first answer > Fork from here > Fork. The new Private
    // thread contains the first turn and file only; continue it without later words.
    // 中文浏览器：问一问 > 附 early.txt 提问 > 再附 later.txt 提问 > 保留为会话 >
    // 首次回答 > 从这里分叉 > 分叉；新私有线程只保留首轮和其文件，继续时不含后续内容。
    use orch_api_contract::threads::{FILENAME_HEADER, ThreadAttachmentCreate, ThreadBlobGet};
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let seen = prompts.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            seen.lock()
                .unwrap()
                .push(serde_json::to_string(&request.input).unwrap());
            Ok(simple_answer())
        });
    let source = create(&s, &s.ada, None).await;
    let mut files = Vec::new();
    for (index, name) in ["early.txt", "later.txt"].into_iter().enumerate() {
        let attached = s
            .rig
            .world
            .app
            .call(ThreadAttachmentCreate, &[&source.conversation_id])
            .token(&s.ada.token)
            .header(FILENAME_HEADER, name)
            .bytes(name.as_bytes())
            .send()
            .await
            .ok();
        s.rig
            .world
            .app
            .call(AskMessageCreate, &[&source.conversation_id])
            .token(&s.ada.token)
            .body(&AskMessageBody {
                client_id: format!("selected-history-{index}"),
                context: None,
                model: None,
                content: vec![
                    ContentPart::Text {
                        text: format!("Question about {name}"),
                    },
                    ContentPart::File {
                        path: attached.path.clone(),
                        name: attached.name.clone(),
                        size: attached.size,
                        blob: Some(attached.blob),
                    },
                ],
            })
            .send()
            .await
            .ok();
        eventually("the source turn has finished", || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(source.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        })
        .await;
        files.push(attached);
    }
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody::default())
        .send()
        .await
        .ok();
    for through in [0, 3, i64::MAX] {
        let invalid = s
            .rig
            .world
            .app
            .call(AskKeep, &[&kept.thread_id])
            .token(&s.ada.token)
            .body(&AskKeepBody {
                to_turn: Some(through),
                ..Default::default()
            })
            .send()
            .await;
        assert_eq!(
            invalid.status.as_u16(),
            400,
            "the requested turn must exist in the selected history"
        );
    }
    let key = new_id().to_string();
    let body = AskKeepBody {
        to_turn: Some(1),
        ..Default::default()
    };
    let fork = s
        .rig
        .world
        .app
        .call(AskKeep, &[&kept.thread_id])
        .token(&s.ada.token)
        .header("Idempotency-Key", &key)
        .body(&body)
        .send()
        .await
        .ok();
    let retry = s
        .rig
        .world
        .app
        .call(AskKeep, &[&kept.thread_id])
        .token(&s.ada.token)
        .header("Idempotency-Key", &key)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(
        retry, fork,
        "a lost result must not create a second permanent fork"
    );
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(fork.thread_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(turns, 1);
    let retained_files: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ask_files WHERE thread_id=$1")
            .bind(fork.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        retained_files, 1,
        "later or unsent files are not carried into a history prefix"
    );
    let early = s
        .rig
        .world
        .app
        .call(ThreadBlobGet, &[&fork.thread_id, &files[0].blob])
        .token(&s.ada.token)
        .send_bytes()
        .await;
    assert_eq!(early.status.as_u16(), 200);
    let later = s
        .rig
        .world
        .app
        .call(ThreadBlobGet, &[&fork.thread_id, &files[1].blob])
        .token(&s.ada.token)
        .send_bytes()
        .await;
    assert!(later.status.is_client_error());
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&fork.thread_id])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "continue-prefix".into(),
            context: None,
            model: None,
            content: vec![ContentPart::Text {
                text: "Continue only this branch".into(),
            }],
        })
        .send()
        .await
        .ok();
    eventually("the selected fork continues", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(fork.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 3, "copy and replay make no model request");
    assert!(prompts[2].contains("early.txt"));
    assert!(!prompts[2].contains("later.txt"));
}

#[tokio::test]
async fn a_kept_thread_exposes_and_applies_its_own_program_model_choice() {
    // EN browser: Keep as thread > grant another person View and Interact >
    // open as that person > choose an allowed API model > Send > choose the
    // current default > Send. The genuine thread carries the actual preference.
    // 中文浏览器：保留为会话 > 授予另一人查看和交互权限 > 以该人打开 > 选择可用 API 模型发送，
    // 再选择当前默认模型发送；真实线程应携带实际模型偏好。
    let s = Scene::new().await;
    let first = configured_provider(&s).await;
    let other = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":first,"model":MODEL},{"provider_id":other,"model":MODEL}]),
    )
    .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let providers = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            providers.lock().unwrap().push(request.provider_id);
            Ok(simple_answer())
        });
    let source = create(&s, &s.ada, None).await;
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody::default())
        .send()
        .await
        .ok();
    for permission in ["thread.view", "thread.interact"] {
        s.rig
            .lets_use("node", kept.thread_id, permission, &s.cai, json!({}))
            .await;
    }
    for index in 0..2 {
        s.rig
            .world
            .app
            .call(AskMessageCreate, &[&kept.thread_id])
            .token(&s.cai.token)
            .body(&AskMessageBody {
                client_id: format!("kept-model-{index}"),
                content: vec![ContentPart::Text {
                    text: "Use my selected model".into(),
                }],
                context: None,
                model: Some((index == 0).then(|| AskModel {
                    provider_id: other,
                    model: MODEL.into(),
                    params: None,
                })),
            })
            .send()
            .await
            .ok();
        eventually(
            "the current sender's selected program model answers",
            || async {
                sqlx::query_scalar::<_, bool>(
                    "SELECT ask_state='ready' FROM threads WHERE node_id=$1",
                )
                .bind(kept.thread_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
            },
        )
        .await;
        let node = places::read(&s.rig.world, &s.cai, kept.thread_id).await;
        let orch_api_contract::nodes::Node::Thread(thread) = node else {
            panic!("thread");
        };
        assert_eq!(
            thread.program.as_ref().unwrap().root_org_id,
            s.tree.org.base.id
        );
        assert_eq!(thread.base.owner.id, s.ada.id());
        if index == 0 {
            assert_eq!(thread.program_model.as_ref().unwrap().provider_id, other);
        } else {
            assert!(thread.program_model.is_none());
        }
    }
    assert_eq!(seen.lock().unwrap().as_slice(), [other, first]);
    let authors: Vec<Id> = sqlx::query_scalar(
        "SELECT author_id FROM thread_inputs WHERE thread_id=$1 ORDER BY position",
    )
    .bind(kept.thread_id)
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(authors, vec![s.cai.id(), s.cai.id()]);
}

#[tokio::test]
async fn kept_segment_program_survives_the_end_of_its_current_binding() {
    // EN browser: Keep as thread > end the built-in binding > inspect earlier
    // history. Its actual program/provider and no-environment declaration stay
    // readable while a new current binding is chosen.
    // 中文浏览器：保留为会话 > 结束内置程序绑定 > 查看早期历史；实际程序、提供方和无需环境的
    // 声明仍可读取，不受当前正在选择的新绑定影响。
    use orch_api_contract::threads::{ThreadHistoryListQuery, ThreadSegmentList};
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, |_| Ok(simple_answer()));
    let made = s
        .rig
        .world
        .app
        .call(AskCreate, &[])
        .token(&s.ada.token)
        .body(&AskCreateBody {
            root_org_id: Some(s.tree.org.base.id),
            context: None,
            retention_days: None,
            model: Some(AskModel {
                provider_id: provider,
                model: MODEL.into(),
                params: None,
            }),
        })
        .send()
        .await
        .ok();
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "bound-answer".into(),
            content: vec![ContentPart::Text {
                text: "Retain a real answered segment".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the historical program answers", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody {
            topic_id: Some(s.tree.topic.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let before = s
        .rig
        .world
        .app
        .call(ThreadSegmentList, &[&kept.thread_id])
        .token(&s.ada.token)
        .query(&ThreadHistoryListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(before.segments.len(), 1);
    let program = before.segments[0].program.clone().unwrap();
    // Isolate the historical projection from the native switching machinery:
    // the completed old binding is ended and the genuine current state is unbound.
    let now = s.rig.world.now();
    let mut tx = s.rig.db.begin().await.unwrap();
    sqlx::query("UPDATE segments SET ended_at=$2,end_reason='switched' WHERE thread_id=$1 AND ended_at IS NULL")
        .bind(kept.thread_id).bind(now).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE threads SET program=NULL,program_root_org_id=NULL,ask_state=NULL,provider_id=NULL,current_segment_id=NULL,state='blocked',blocked_reason='unbound' WHERE node_id=$1")
        .bind(kept.thread_id).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let after = s
        .rig
        .world
        .app
        .call(ThreadSegmentList, &[&kept.thread_id])
        .token(&s.ada.token)
        .query(&ThreadHistoryListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(after.segments.len(), 1);
    assert_eq!(after.segments[0].program.as_ref(), Some(&program));
    assert_eq!(after.segments[0].provider.as_ref().unwrap().id, provider);
    assert!(after.segments[0].agent.is_none());
    assert!(after.segments[0].environment.is_none());
}

#[tokio::test]
async fn a_native_attachment_reaches_ask_and_its_kept_history_without_an_environment() {
    // EN browser: Ask > Attach > choose diagram.png > Send with a text-only
    // model; its named link stays. Keep as thread > open the copied image >
    // Send a follow-up: the reference points at the kept thread.
    // 中文浏览器：问一问 > 附件 > 选择 diagram.png > 使用纯文本模型发送；保留带名称的链接。
    // 保留为会话 > 打开复制的图片 > 发送后续问题；引用应指向永久线程。
    use base64::Engine as _;
    use orch_api_contract::threads::{
        Delivery, FILENAME_HEADER, ImageMediaType, ThreadAttachmentCreate, ThreadBlobGet,
        ThreadInputCreate, ThreadInputCreateBody,
    };
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    sqlx::query("UPDATE provider_models SET parameters=$2 WHERE provider_id=$1")
        .bind(provider)
        .bind(json!({"input_images":false,"context_window":200000}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let seen = inputs.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            seen.lock()
                .unwrap()
                .push(serde_json::to_value(&request.input).unwrap());
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![GenerationPart::Text {
                            text: "The linked image is retained.".into(),
                        }],
                        continuation: vec![],
                    },
                    stop: GenerationStop::Complete,
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let made = create(&s, &s.ada, None).await;
    let bytes = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6RxkAAAAASUVORK5CYII=").unwrap();
    let attached = s
        .rig
        .world
        .app
        .call(ThreadAttachmentCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .header(FILENAME_HEADER, "diagram.png")
        .bytes(&bytes)
        .send()
        .await
        .ok();
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "image".into(),
            context: None,
            model: None,
            content: vec![ContentPart::Image {
                blob: attached.blob,
                media_type: ImageMediaType::Png,
                name: attached.name,
                path: Some(attached.path),
            }],
        })
        .send()
        .await
        .ok();
    eventually("the named image reference answers", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&made.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody::default())
        .send()
        .await
        .ok();
    let file = s
        .rig
        .world
        .app
        .call(ThreadBlobGet, &[&kept.thread_id, &attached.blob])
        .token(&s.ada.token)
        .send_bytes()
        .await;
    assert_eq!(file.status.as_u16(), 200);
    assert_eq!(file.raw.as_ref(), bytes.as_slice());
    s.rig
        .world
        .app
        .call(ThreadInputCreate, &[&kept.thread_id])
        .token(&s.ada.token)
        .body(&ThreadInputCreateBody {
            content: vec![ContentPart::Text {
                text: "Continue with that image reference".into(),
            }],
            delivery: Delivery::Queue,
            client_id: "image-follow-up".into(),
            bypass: None,
            native_action: None,
            plan: false,
        })
        .send()
        .await
        .ok();
    eventually("the kept image reference answers", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(kept.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let inputs = inputs.lock().unwrap();
    assert_eq!(inputs.len(), 2);
    assert!(inputs[0].to_string().contains("diagram.png"));
    assert!(inputs[0].to_string().contains(&format!(
        "/ask/conversations/{}/attachments/",
        made.conversation_id
    )));
    assert!(
        inputs[1]
            .to_string()
            .contains(&format!("/threads/{}/blobs/", kept.thread_id))
    );
    assert!(!inputs[1].to_string().contains("data:image"));
}

#[tokio::test]
async fn a_kept_builtin_thread_accepts_ordinary_input_as_its_current_sender() {
    // EN browser: Ask > Send > Keep as thread > open the Private thread;
    // grant another person View and Interact, then send as that person.
    // The retained answer is visible and the new usage belongs to its sender.
    // 中文浏览器：问一问 > 发送 > 保留为会话 > 打开私有线程；向另一人授予查看和交互权限，
    // 再以该人发送。历史回答可见，新用量归属实际发送者。
    use orch_api_contract::threads::{
        Delivery, ThreadInputCreate, ThreadInputCreateBody, ThreadInterrupt, ThreadInterruptBody,
    };
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let calls = Arc::new(Mutex::new(0));
    let count = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            *count.lock().unwrap() += 1;
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![GenerationPart::Text {
                            text: "A retained answer".into(),
                        }],
                        continuation: vec![],
                    },
                    stop: GenerationStop::Complete,
                },
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    total_tokens: 12,
                    ..Default::default()
                },
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let source = create(&s, &s.ada, None).await;
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "original".into(),
            content: vec![ContentPart::Text {
                text: "First question".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the source answer finishes", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(source.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody::default())
        .send()
        .await
        .ok();
    let node = places::read(&s.rig.world, &s.ada, kept.thread_id).await;
    let orch_api_contract::nodes::Node::Thread(thread) = node else {
        panic!("thread");
    };
    assert_eq!(
        thread.program.as_ref().unwrap().root_org_id,
        s.tree.org.base.id
    );
    assert!(thread.binding.is_none());
    assert!(thread.environment.is_none());
    // EN browser: the kept thread has the ordinary Discussion and Follow controls.
    // 中文浏览器：保留后的线程具有普通的讨论和关注入口。
    let chats: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chats WHERE kind='node' AND node_id=$1")
            .bind(kept.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(chats, 1);
    let discussion = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::chats::NodeChatOpeningGet,
            &[&kept.thread_id],
        )
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(
        discussion
            .scopes
            .iter()
            .any(|scope| scope.node.id == kept.thread_id)
    );
    let followed: String =
        sqlx::query_scalar("SELECT level FROM subscriptions WHERE node_id=$1 AND user_id=$2")
            .bind(kept.thread_id)
            .bind(s.ada.id())
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(followed, "all");
    for permission in ["thread.view", "thread.interact"] {
        s.rig
            .lets_use("node", kept.thread_id, permission, &s.cai, json!({}))
            .await;
    }
    let question = ThreadInputCreateBody {
        content: vec![ContentPart::Text {
            text: "A question from somebody else".into(),
        }],
        delivery: Delivery::Queue,
        client_id: "continued".into(),
        bypass: None,
        native_action: None,
        plan: false,
    };
    let sent = s
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&kept.thread_id])
        .token(&s.cai.token)
        .body(&question)
        .send()
        .await;
    assert_eq!(sent.status.as_u16(), 201);
    let sent = sent.ok().into_full().unwrap();
    assert_eq!(sent.input.author.as_ref().unwrap().id, s.cai.id());
    eventually("the current sender's answer finishes", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(kept.thread_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let replay = s
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&kept.thread_id])
        .token(&s.cai.token)
        .body(&question)
        .send()
        .await;
    assert_eq!(replay.status.as_u16(), 200);
    assert_eq!(replay.ok().into_full().unwrap().input.id, sent.input.id);
    assert_eq!(*calls.lock().unwrap(), 2);
    let usage: Vec<(Id, i64)> = sqlx::query_as("SELECT user_id,sum(total_tokens)::bigint FROM usage_events WHERE thread_id=$1 AND purpose='ask' GROUP BY user_id")
        .bind(kept.thread_id).fetch_all(s.rig.db.pool()).await.unwrap();
    assert_eq!(usage, vec![(s.cai.id(), 12)]);
    let followed: String =
        sqlx::query_scalar("SELECT level FROM subscriptions WHERE node_id=$1 AND user_id=$2")
            .bind(kept.thread_id)
            .bind(s.cai.id())
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(followed, "all");
    // EN browser: stop the displayed current turn; a stale turn selection
    // must leave it running, and a successful Stop names the actual person.
    // 中文浏览器：停止当前显示的回合；过期选择不能停止别的回合，成功停止应记录实际操作者。
    let turn = sent.input.turn_id.unwrap();
    let now = s.rig.world.app.app.clock.now();
    sqlx::query("UPDATE turns SET state='running',completed_at=NULL,completed_seq=0,end_work=NULL WHERE id=$1")
        .bind(turn).execute(s.rig.db.pool()).await.unwrap();
    sqlx::query("UPDATE threads SET state='running',ask_state='running',current_turn_id=$2,ask_runner_id=$3,ask_heartbeat_at=$4 WHERE node_id=$1")
        .bind(kept.thread_id).bind(turn).bind(new_id()).bind(now).execute(s.rig.db.pool()).await.unwrap();
    let stale = s
        .rig
        .world
        .app
        .call(ThreadInterrupt, &[&kept.thread_id])
        .token(&s.cai.token)
        .body(&ThreadInterruptBody { turn_id: new_id() })
        .send()
        .await;
    assert!(stale.status.is_client_error());
    let state: String = sqlx::query_scalar("SELECT ask_state FROM threads WHERE node_id=$1")
        .bind(kept.thread_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(state, "running");
    let stopped = s
        .rig
        .world
        .app
        .call(ThreadInterrupt, &[&kept.thread_id])
        .token(&s.cai.token)
        .body(&ThreadInterruptBody { turn_id: turn })
        .send()
        .await
        .ok();
    assert_eq!(stopped.id, turn);
    assert_eq!(
        stopped.state,
        orch_api_contract::threads::TurnState::Interrupted
    );
    assert_eq!(stopped.interrupted_by.as_ref().unwrap().id, s.cai.id());
    assert_eq!(*calls.lock().unwrap(), 2);
    let deleted = s
        .rig
        .world
        .app
        .call(AskDelete, &[&kept.thread_id])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(
        deleted.status.as_u16(),
        404,
        "temporary conversation deletion cannot erase a permanent thread"
    );
    assert!(matches!(
        places::read(&s.rig.world, &s.ada, kept.thread_id).await,
        orch_api_contract::nodes::Node::Thread(_)
    ));
}

#[tokio::test]
async fn ordinary_retry_rechecks_and_runs_a_builtin_question_as_the_actual_sender() {
    // EN browser/API: Keep as thread > send a question that fails > another
    // allowed person presses Retry. It runs as that person without an environment;
    // retrying the same client identity sends no second request. Their cap applies.
    // 中文浏览器/API：保留为会话 > 提问失败 > 由另一位获准用户重试；无需环境，按实际发送者
    // 运行，同一客户端标识重试不得重复请求，并受重试者自己的额度限制。
    use orch_api_contract::threads::{
        Delivery, ThreadInputCreate, ThreadInputCreateBody, ThreadTurnRetry, ThreadTurnRetryBody,
    };
    use orch_link_contract::commands::{CommandError, ErrorCode};
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let calls = Arc::new(Mutex::new(0));
    let paid = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            let mut calls = paid.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "Synthetic request failed",
                ));
            }
            let CommandResult::ProviderGenerate(mut result) = simple_answer() else {
                panic!("generation");
            };
            result.usage = orch_api_contract::threads::TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                ..Default::default()
            };
            Ok(CommandResult::ProviderGenerate(result))
        });
    let source = create_unseeded(&s, &s.ada).await;
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&source.conversation_id])
        .token(&s.ada.token)
        .body(&AskKeepBody {
            topic_id: Some(s.tree.topic.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let first = s
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&kept.thread_id])
        .token(&s.ada.token)
        .body(&ThreadInputCreateBody {
            client_id: "failed-program-question".into(),
            content: vec![ContentPart::Text {
                text: "Retry this exact question".into(),
            }],
            delivery: Delivery::Queue,
            bypass: None,
            native_action: None,
            plan: false,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    let turn = first.input.turn_id.unwrap();
    eventually("the original program question fails durably", || async {
        sqlx::query_scalar::<_, bool>("SELECT state='failed' FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    for permission in ["thread.view", "thread.interact"] {
        s.rig
            .lets_use("node", kept.thread_id, permission, &s.cai, json!({}))
            .await;
    }
    let without_read = scoped_key(
        &s,
        json!({"rules":[{
            "target":{"kind":"thread","id":kept.thread_id,"reach":"self"},
            "allow":[{"operation":"ThreadTurnRetry","actions":["write"]}]
        }]}),
    )
    .await;
    let refused = s
        .rig
        .world
        .app
        .call(ThreadTurnRetry, &[&kept.thread_id, &turn])
        .token(&without_read)
        .body(&ThreadTurnRetryBody {
            client_id: "without-original-read".into(),
            bypass: None,
        })
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    let body = ThreadTurnRetryBody {
        client_id: "actual-person-retry".into(),
        bypass: None,
    };
    let retried = s
        .rig
        .world
        .app
        .call(ThreadTurnRetry, &[&kept.thread_id, &turn])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(retried.input.author.as_ref().unwrap().id, s.cai.id());
    assert_ne!(retried.input.id, first.input.id);
    eventually(
        "the native Retry entrance runs the control-plane loop",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(kept.thread_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        },
    )
    .await;
    assert_eq!(*calls.lock().unwrap(), 2);
    let replay = s
        .rig
        .world
        .app
        .call(ThreadTurnRetry, &[&kept.thread_id, &turn])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(replay.status.as_u16(), 200);
    assert_eq!(replay.ok().input.id, retried.input.id);
    assert_eq!(*calls.lock().unwrap(), 2);
    let counted: i64 = sqlx::query_scalar(
        "SELECT sum(total_tokens)::bigint FROM usage_events WHERE user_id=$1 AND purpose='ask'",
    )
    .bind(s.cai.id())
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        counted, 15,
        "the retry is attributed to its actual sender, not the earlier author"
    );
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,uncounted,created_by) VALUES($1,$2,'user',$3,'shared','tokens','day',0,ARRAY['ask'],'allow',$4)")
        .bind(new_id()).bind(s.tree.org.base.id).bind(s.cai.id()).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    let capped = s
        .rig
        .world
        .app
        .call(ThreadTurnRetry, &[&kept.thread_id, &turn])
        .token(&s.cai.token)
        .body(&ThreadTurnRetryBody {
            client_id: "capped-person-retry".into(),
            bypass: None,
        })
        .send()
        .await;
    assert_eq!(capped.status.as_u16(), 403);
    assert_eq!(*calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn an_expired_controller_lease_recovers_without_borrowing_a_new_principal() {
    // EN browser: interrupt the API controller during a streamed Ask, reopen
    // after its lease expires, then Send a follow-up under current authority.
    // 中文浏览器：在流式提问时中断 API 控制进程，租约到期后重开，再以当前权限发送后续问题。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, |_| {
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![GenerationPart::Text {
                            text: "A retained answer".into(),
                        }],
                        continuation: vec![],
                    },
                    stop: GenerationStop::Complete,
                },
                usage: Default::default(),
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let made = create(&s, &s.cai, None).await;
    let accepted = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "first".into(),
            content: vec![ContentPart::Text {
                text: "Question".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the accepted answer is retained", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    let turn = accepted.turn_id.unwrap();
    let count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM usage_events WHERE thread_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    s.rig.world.advance(chrono::Duration::minutes(5));
    let now = s.rig.world.app.app.clock.now();
    // Model the durable running rows left by a dead controller. No user or
    // credential is manufactured to finish them; only the native sweep runs.
    sqlx::query("UPDATE turns SET state='running',completed_at=NULL,completed_seq=0,end_work=NULL WHERE id=$1")
        .bind(turn)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET state='running',ask_state='running',current_turn_id=$2,ask_runner_id=$3,ask_heartbeat_at=$4 WHERE node_id=$1")
        .bind(made.conversation_id).bind(turn).bind(new_id()).bind(now).execute(s.rig.db.pool()).await.unwrap();
    assert_eq!(
        orch_api_core::testing::recover_ask_turns(&s.rig.world.app.app)
            .await
            .unwrap(),
        0,
        "another healthy controller retains its turn"
    );
    sqlx::query("UPDATE threads SET ask_heartbeat_at=$2 WHERE node_id=$1")
        .bind(made.conversation_id)
        .bind(now - chrono::Duration::minutes(5))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        orch_api_core::testing::recover_ask_turns(&s.rig.world.app.app)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        orch_api_core::testing::recover_ask_turns(&s.rig.world.app.app)
            .await
            .unwrap(),
        0
    );
    let got = read(&s, &s.cai, made.conversation_id).await;
    assert_eq!(got.conversation.state, AskState::Ready);
    let state: String = sqlx::query_scalar("SELECT state FROM turns WHERE id=$1")
        .bind(turn)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(state, "interrupted");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM usage_events WHERE thread_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap(),
        count
    );
    // A retained pre-publication call may never have reached the command writer.
    s.rig.engine.cancel_ask_calls(&[new_id()]).await.unwrap();
}

#[tokio::test]
async fn a_late_provider_reply_is_counted_only_from_its_original_recipient_and_once() {
    // EN browser: stop a streaming answer; after the runtime acknowledges Stop,
    // usage still names the asking person, and no late answer appears.
    // 中文浏览器：停止流式回答；运行端确认后，已用额度仍归提问者，旧回答不再出现。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let response = ProviderGenerateResult {
        native_result_blob: None,
        output: GenerationOutput {
            native_result: serde_json::Value::Null,
            message: GenerationMessage {
                role: GenerationRole::Assistant,
                content: vec![GenerationPart::Text {
                    text: "Answer".into(),
                }],
                continuation: vec![],
            },
            stop: GenerationStop::Complete,
        },
        usage: orch_api_contract::threads::TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        },
        timing: Default::default(),
        request_ref: Some("actual-request".into()),
    };
    let generated = response.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            Ok(CommandResult::ProviderGenerate(generated.clone()))
        });
    let made = create(&s, &s.cai, None).await;
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "first".into(),
            content: vec![ContentPart::Text {
                text: "Question".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the first Ask finishes", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    // Retain one additional native issuer row whose controller has ended its
    // turn already. A late receipt must account without writing a new item.
    let late = new_id();
    sqlx::query("INSERT INTO ask_calls(id,thread_id,turn_id,item_id,root_org_id,person_id,provider_id,machine_id,request,created_at) SELECT $1,thread_id,turn_id,item_id,root_org_id,person_id,provider_id,machine_id,request,created_at FROM ask_calls WHERE thread_id=$2 LIMIT 1")
        .bind(late).bind(made.conversation_id).execute(s.rig.db.pool()).await.unwrap();
    let before = read(&s, &s.cai, made.conversation_id).await;
    let reply = Ok(serde_json::to_value(response).unwrap());
    let mut tx = s.rig.db.begin().await.unwrap();
    s.rig
        .engine
        .reconcile_ask_reply(&mut tx, new_id(), late, &reply)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE id=$1")
        .bind(late)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 0);
    for _ in 0..2 {
        s.rig
            .engine
            .reconcile_ask_reply(&mut tx, s.rig.machine_id, late, &reply)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let counted: (i64, i64) = sqlx::query_as("SELECT count(*),sum(total_tokens)::bigint FROM usage_events WHERE id=$1 AND purpose='ask' AND user_id=$2")
        .bind(late).bind(s.cai.id()).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(counted, (1, 15));
    let after = read(&s, &s.cai, made.conversation_id).await;
    assert_eq!(before.messages, after.messages);
}

#[tokio::test]
async fn ask_purpose_and_offering_filters_share_the_owned_model_cap_port() {
    // EN browser: configure a zero ordinary-turn allowance and a service-only
    // allowance; Ask still answers. Reach a personal Ask allowance: its next
    // question is refused, while another person's question still answers.
    // 中文浏览器：配置普通回合零额度和仅服务额度后，问一问仍可回答；达到某人的提问额度后，
    // 只拒绝该人的下一个问题，其他人的问题仍可回答。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    for purpose in ["turn", "summary"] {
        sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,uncounted,created_by) VALUES($1,$2,'user',$3,'shared','tokens','day',0,$4,'allow',$5)")
            .bind(new_id()).bind(s.tree.org.base.id).bind(s.cai.id()).bind(vec![purpose.to_owned()])
            .bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    }
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,services,uncounted,created_by) VALUES($1,$2,'user',$3,'shared','credits','day',0,ARRAY['ask'],ARRAY['search-only'],'allow',$4)")
        .bind(new_id()).bind(s.tree.org.base.id).bind(s.cai.id()).bind(s.ada.id())
        .execute(s.rig.db.pool()).await.unwrap();
    let calls = Arc::new(Mutex::new(0));
    let paid = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            *paid.lock().unwrap() += 1;
            let CommandResult::ProviderGenerate(mut answer) = simple_answer() else {
                panic!("generation");
            };
            answer.usage = orch_api_contract::threads::TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                ..Default::default()
            };
            Ok(CommandResult::ProviderGenerate(answer))
        });
    let made = create(&s, &s.cai, None).await;
    let body = AskMessageBody {
        client_id: "own-purpose".into(),
        content: vec![ContentPart::Text {
            text: "Ask uses its own allowance".into(),
        }],
        context: None,
        model: None,
    };
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await
        .ok();
    eventually(
        "Ask ignores unrelated purposes and service-only offerings",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        },
    )
    .await;
    assert_eq!(*calls.lock().unwrap(), 1);
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,uncounted,created_by) VALUES($1,$2,'user',$3,'shared','tokens','day',15,ARRAY['ask'],'allow',$4)")
        .bind(new_id()).bind(s.tree.org.base.id).bind(s.cai.id()).bind(s.ada.id())
        .execute(s.rig.db.pool()).await.unwrap();
    let next = AskMessageBody {
        client_id: "ask-capped".into(),
        ..body.clone()
    };
    let refused = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&next)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    assert_eq!(*calls.lock().unwrap(), 1);
    let other = create(&s, &s.ada, None).await;
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&other.conversation_id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    eventually("another issuer has independent Ask use", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(other.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(*calls.lock().unwrap(), 2);
    let usage: Vec<(Id, i64)> = sqlx::query_as("SELECT user_id,sum(total_tokens)::bigint FROM usage_events WHERE org_id=$1 AND purpose='ask' GROUP BY user_id ORDER BY user_id")
        .bind(s.tree.org.base.id).fetch_all(s.rig.db.pool()).await.unwrap();
    assert!(usage.contains(&(s.cai.id(), 15)) && usage.contains(&(s.ada.id(), 15)));
}

#[tokio::test]
async fn ask_concurrency_keeps_the_current_turns_reservation_and_its_actual_purpose() {
    // EN browser/API: one Ask occupies its personal running slot. That turn
    // may continue, a second Ask is refused, and a turn-only cap cannot count
    // or stop the Ask slot as ordinary agent work.
    // 中文浏览器/API：一个提问占用个人并发名额后，本轮可以继续，第二个提问被拒绝；
    // 仅普通回合的额度不能将提问名额算作普通代理工作。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    sqlx::query("UPDATE provider_models SET parameters=$2 WHERE provider_id=$1")
        .bind(provider)
        .bind(
            json!({"context_window":200000,"service_tiers":[{"id":"priority","label":"Priority"}]}),
        )
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let calls = Arc::new(Mutex::new(0));
    let paid = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            *paid.lock().unwrap() += 1;
            Ok(simple_answer())
        });
    let made = create(&s, &s.cai, None).await;
    let body = AskMessageBody {
        client_id: "reserved".into(),
        content: vec![ContentPart::Text {
            text: "Question".into(),
        }],
        context: None,
        model: Some(Some(AskModel {
            provider_id: provider,
            model: MODEL.into(),
            params: Some(json!({"service_tier":"priority"})),
        })),
    };
    let accepted = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await
        .ok();
    eventually(
        "the reservation fixture has a real accepted turn",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(made.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        },
    )
    .await;
    let other = create(&s, &s.cai, None).await;
    let cap = new_id();
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,tiers,created_by) VALUES($1,$2,'user',$3,'shared','running',NULL,1,ARRAY['ask'],ARRAY['priority'],$4)")
        .bind(cap).bind(s.tree.org.base.id).bind(s.cai.id()).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    let turn = accepted.turn_id.unwrap();
    let now = s.rig.world.now();
    sqlx::query("UPDATE turns SET state='running',completed_at=NULL,completed_seq=0,end_work=NULL WHERE id=$1")
        .bind(turn).execute(s.rig.db.pool()).await.unwrap();
    sqlx::query("UPDATE threads SET state='running',ask_state='running',current_turn_id=$2,ask_runner_id=$3,ask_heartbeat_at=$4 WHERE node_id=$1")
        .bind(made.conversation_id).bind(turn).bind(new_id()).bind(now).execute(s.rig.db.pool()).await.unwrap();
    let mut conn = s.rig.db.acquire().await.unwrap();
    assert!(
        s.rig
            .engine
            .room_for_ask(
                &mut conn,
                s.tree.org.base.id,
                provider,
                MODEL,
                Some("priority"),
                s.cai.id(),
                Some(turn)
            )
            .await
            .unwrap()
    );
    assert!(
        !s.rig
            .engine
            .room_for_ask(
                &mut conn,
                s.tree.org.base.id,
                provider,
                MODEL,
                Some("priority"),
                s.cai.id(),
                None
            )
            .await
            .unwrap()
    );
    drop(conn);
    let refused = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&other.conversation_id])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    sqlx::query("UPDATE usage_caps SET purposes=ARRAY['turn'],amount=0 WHERE id=$1")
        .bind(cap)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&other.conversation_id])
        .token(&s.cai.token)
        .body(&body)
        .send()
        .await
        .ok();
    eventually(
        "ordinary-turn purpose does not occupy an Ask slot",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
                .bind(other.conversation_id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        },
    )
    .await;
    assert_eq!(*calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn lowering_an_ask_stop_cap_ends_only_its_persons_actual_builtin_turn() {
    // EN browser/API: while Ask answers, lower its person's Ask cap below used
    // tokens. Finish current answer leaves it running; Stop interrupts it,
    // refreshes the panel and preserves history. Another person's answer continues.
    // Repeat after Keep as thread, without supplying an agent or environment.
    // 中文浏览器/API：回答期间将某人的提问额度降至已用量以下；完成本轮模式允许继续，
    // 停止模式应中断、刷新面板并保留历史，其他人的回答不受影响。保留为会话后再次验证，
    // 不提供智能体或环境。
    for permanent in [false, true] {
        let s = Scene::new().await;
        if permanent {
            places::give_role(
                &s.rig.world,
                s.tree.project.base.id,
                &s.cai,
                "developer",
                &s.ada,
            )
            .await;
        }
        let provider = configured_provider(&s).await;
        setting(
            &s,
            "ask.models",
            json!([{"provider_id":provider,"model":MODEL}]),
        )
        .await;
        let calls = Arc::new(Mutex::new(0));
        let paid = calls.clone();
        s.rig
            .machine
            .handler
            .on(CommandKind::ProviderGenerate, move |_| {
                *paid.lock().unwrap() += 1;
                let CommandResult::ProviderGenerate(mut answer) = simple_answer() else {
                    panic!("generation");
                };
                answer.usage = orch_api_contract::threads::TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    total_tokens: 15,
                    ..Default::default()
                };
                Ok(CommandResult::ProviderGenerate(answer))
            });
        let mut active = Vec::new();
        for who in [&s.ada, &s.cai] {
            let source = create(&s, who, None).await;
            s.rig
                .world
                .app
                .call(AskMessageCreate, &[&source.conversation_id])
                .token(&who.token)
                .body(&AskMessageBody {
                    client_id: "lowered-cap".into(),
                    context: None,
                    model: None,
                    content: vec![ContentPart::Text {
                        text: "Answer under current caps".into(),
                    }],
                })
                .send()
                .await
                .ok();
            eventually(
                "the active-cap fixture has real answered history",
                || async {
                    sqlx::query_scalar::<_, bool>(
                        "SELECT ask_state='ready' FROM threads WHERE node_id=$1",
                    )
                    .bind(source.conversation_id)
                    .fetch_one(s.rig.db.pool())
                    .await
                    .unwrap()
                },
            )
            .await;
            let thread = if permanent {
                s.rig
                    .world
                    .app
                    .call(AskKeep, &[&source.conversation_id])
                    .token(&who.token)
                    .body(&AskKeepBody::default())
                    .send()
                    .await
                    .ok()
                    .thread_id
            } else {
                source.conversation_id
            };
            let turn: Id = sqlx::query_scalar(
                "SELECT id FROM turns WHERE thread_id=$1 ORDER BY ordinal DESC LIMIT 1",
            )
            .bind(thread)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
            // An already accepted durable active state isolates cap-change handling,
            // independently of model callback timing and without a second paid request.
            sqlx::query("UPDATE turns SET state='running',completed_at=NULL,completed_seq=0,end_work=NULL WHERE id=$1")
                .bind(turn).execute(s.rig.db.pool()).await.unwrap();
            sqlx::query("UPDATE threads SET state='running',ask_state='running',current_turn_id=$2,ask_runner_id=$3,ask_heartbeat_at=$4 WHERE node_id=$1")
                .bind(thread).bind(turn).bind(new_id()).bind(s.rig.world.now()).execute(s.rig.db.pool()).await.unwrap();
            active.push((thread, turn));
        }
        let cap = new_id();
        sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,at_limit,uncounted,created_by) VALUES($1,$2,'user',$3,'shared','tokens','day',10,ARRAY['ask'],'finish_turn','allow',$3)")
            .bind(cap).bind(s.tree.org.base.id).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
        s.rig.engine.cap_changed(cap).await.unwrap();
        let state: String = sqlx::query_scalar("SELECT state FROM turns WHERE id=$1")
            .bind(active[0].1)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(state, "running", "finish_turn preserves the active answer");
        sqlx::query("UPDATE usage_caps SET at_limit='stop' WHERE id=$1")
            .bind(cap)
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        s.rig.engine.cap_changed(cap).await.unwrap();
        eventually("the exact covered built-in turn is interrupted", || async {
            sqlx::query_scalar::<_, bool>("SELECT state='interrupted' FROM turns WHERE id=$1")
                .bind(active[0].1)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap()
        })
        .await;
        let ended: (String, Option<Id>, Option<Id>, String) = sqlx::query_as(
            "SELECT t.ask_state,t.current_turn_id,t.ask_runner_id,r.error->>'code' FROM threads t JOIN turns r ON r.thread_id=t.node_id WHERE r.id=$1",
        ).bind(active[0].1).fetch_one(s.rig.db.pool()).await.unwrap();
        assert_eq!(ended, ("ready".into(), None, None, "cap_reached".into()));
        let other: String = sqlx::query_scalar("SELECT state FROM turns WHERE id=$1")
            .bind(active[1].1)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(
            other, "running",
            "another person's allowance is independent"
        );
        let reset: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM ask_events WHERE thread_id=$1 AND body->>'kind'='reset')",
        )
        .bind(active[0].0)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
        assert!(
            reset,
            "the panel rereads a current authorized header after interruption"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            2,
            "a cap change makes no generation call"
        );
    }
}

#[tokio::test]
async fn a_personal_ask_cap_allows_finish_turn_and_blocks_the_next_question() {
    // EN browser: reach a personal Ask cap during a sourced answer; finish_turn
    // completes that answer, then the next Send is refused with the cap reason.
    // 中文浏览器：在带来源的回答中达到个人额度；允许完成本轮，下一次“发送”被额度阻止。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,at_limit,uncounted,created_by) VALUES ($1,$2,'user',$3,'shared','tokens','day',100,'finish_turn','allow',$4)")
        .bind(new_id()).bind(s.tree.org.base.id).bind(s.cai.id()).bind(s.ada.id())
        .execute(s.rig.db.pool()).await.unwrap();
    let seen = Arc::new(Mutex::new(0));
    let count = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            let mut calls = count.lock().unwrap();
            *calls += 1;
            let first = *calls == 1;
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: if first {
                            vec![GenerationPart::ToolCall {
                                id: "help".into(),
                                name: "docs".into(),
                                arguments: json!({"query":"permissions"}),
                            }]
                        } else {
                            vec![GenerationPart::Text {
                                text: "The answer is complete.".into(),
                            }]
                        },
                        continuation: vec![],
                    },
                    stop: if first {
                        GenerationStop::Tools
                    } else {
                        GenerationStop::Complete
                    },
                },
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    total_tokens: 120,
                    ..Default::default()
                },
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let made = create(&s, &s.cai, None).await;
    let question = |client_id: &str| AskMessageBody {
        client_id: client_id.into(),
        content: vec![ContentPart::Text {
            text: "Explain permissions".into(),
        }],
        context: None,
        model: None,
    };
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&question("first"))
        .send()
        .await
        .ok();
    eventually("the current Ask turn may finish", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(*seen.lock().unwrap(), 2);
    assert!(
        s.rig
            .world
            .app
            .call(AskMessageCreate, &[&made.conversation_id])
            .token(&s.cai.token)
            .body(&question("next"))
            .send()
            .await
            .status
            .is_client_error()
    );
    assert_eq!(*seen.lock().unwrap(), 2);
    let reached: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_cap_notices WHERE user_id=$1 AND level='reached'",
    )
    .bind(s.cai.id())
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert!(reached > 0);
}

#[tokio::test]
async fn a_stop_cap_prevents_the_next_tool_batch_and_model_step() {
    // EN browser: reach a stop cap on the first streamed request; no source
    // read or second request starts, and the retained answer shows the cap reason.
    // 中文浏览器：首次流式请求达到停止额度后，不再读取来源或发起第二次请求，并显示额度原因。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,at_limit,uncounted,created_by) VALUES ($1,$2,'user',$3,'shared','tokens','day',100,'stop','allow',$4)")
        .bind(new_id()).bind(s.tree.org.base.id).bind(s.cai.id()).bind(s.ada.id())
        .execute(s.rig.db.pool()).await.unwrap();
    let calls = Arc::new(Mutex::new(0));
    let seen = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |_| {
            *seen.lock().unwrap() += 1;
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: vec![GenerationPart::ToolCall {
                            id: "after-cap".into(),
                            name: "docs".into(),
                            arguments: json!({"query":"permissions"}),
                        }],
                        continuation: vec![],
                    },
                    stop: GenerationStop::Tools,
                },
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    total_tokens: 120,
                    ..Default::default()
                },
                timing: Default::default(),
                request_ref: None,
            }))
        });
    let made = create(&s, &s.cai, None).await;
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "question".into(),
            content: vec![ContentPart::Text {
                text: "Explain permissions".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the capped Ask stops", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(*calls.lock().unwrap(), 1);
    let tools: i64 =
        sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1 AND kind='tool_call'")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(tools, 0);
}

#[tokio::test]
async fn ask_runs_a_tool_loop_and_attributes_every_step_to_the_person() {
    // EN browser: Ask > ask a help question > see a cited answer > Keep as thread > verify Private.
    // 中文浏览器：问一问 > 询问帮助问题 > 查看带来源的回答 > 保留为会话 > 核对为私有。
    let s = Scene::new().await;
    let provider = configured_provider(&s).await;
    setting(
        &s,
        "ask.models",
        json!([{"provider_id":provider,"model":MODEL}]),
    )
    .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderGenerate, move |command| {
            let Command::ProviderGenerate(request) = command else {
                panic!("generation");
            };
            let mut seen = recorded.lock().unwrap();
            let first = seen.is_empty();
            seen.push(request.clone());
            Ok(CommandResult::ProviderGenerate(ProviderGenerateResult {
                native_result_blob: None,
                output: GenerationOutput {
                    native_result: serde_json::Value::Null,
                    message: GenerationMessage {
                        role: GenerationRole::Assistant,
                        content: if first {
                            vec![GenerationPart::ToolCall {
                                id: "read-help".into(),
                                name: "docs".into(),
                                arguments: json!({"query":"permissions"}),
                            }]
                        } else {
                            vec![GenerationPart::Text {
                                text: "See [permissions](/help/permissions).".into(),
                            }]
                        },
                        continuation: vec![],
                    },
                    stop: if first {
                        GenerationStop::Tools
                    } else {
                        GenerationStop::Complete
                    },
                },
                usage: orch_api_contract::threads::TokenUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    total_tokens: 120,
                    ..Default::default()
                },
                timing: Default::default(),
                request_ref: Some(format!("question-{}", seen.len())),
            }))
        });
    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM api_tokens")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let made = create(&s, &s.cai, None).await;
    s.rig
        .world
        .app
        .call(AskMessageCreate, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskMessageBody {
            client_id: "question".into(),
            content: vec![ContentPart::Text {
                text: "How do permissions work?".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    eventually("the built-in turn finishes", || async {
        sqlx::query_scalar::<_, bool>("SELECT ask_state='ready' FROM threads WHERE node_id=$1")
            .bind(made.conversation_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(seen.lock().unwrap().len(), 2);
    let rows:(i64,i64)=sqlx::query_as("SELECT count(*),COALESCE(sum(total_tokens),0)::bigint FROM usage_events WHERE thread_id=$1 AND purpose='ask' AND user_id=$2 AND org_id=$3")
        .bind(made.conversation_id).bind(s.cai.id()).bind(s.tree.org.base.id).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(rows, (2, 240));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM api_tokens")
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap(),
        tokens
    );
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskKeepBody {
            topic_id: Some(s.tree.topic.base.id),
            ..Default::default()
        })
        .send()
        .await;
    assert!(
        kept.status.is_client_error(),
        "a viewer still needs actual destination creation authority"
    );
    places::give_role(
        &s.rig.world,
        s.tree.project.base.id,
        &s.cai,
        "developer",
        &s.ada,
    )
    .await;
    let kept = s
        .rig
        .world
        .app
        .call(AskKeep, &[&made.conversation_id])
        .token(&s.cai.token)
        .body(&AskKeepBody::default())
        .send()
        .await
        .ok();
    assert_eq!(kept.topic_id, s.tree.topic.base.id);
    let inherited:i64=sqlx::query_scalar("SELECT count(*) FROM permission_rows WHERE object_id=$1 AND page='live' AND permission='thread.view' AND NOT inherit")
        .bind(kept.thread_id).fetch_one(s.rig.db.pool()).await.unwrap();
    assert!(inherited > 0);
    let history: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1")
        .bind(kept.thread_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        history >= 2,
        "Keep uses the existing conversation copier to preserve recorded messages"
    );
}
