//! A private native model step returns ordinary assistant messages and read requests.
use super::*;
use orch_link_contract::{
    commands::AgentStartResult,
    events::{Event, TurnOutcome, UsageUpdated},
};

async fn configure(s: &Scene, schema: bool) -> AskModelIdentity {
    let mut declared = rig::descriptor();
    declared["side_thread_modes"] = json!(["read_only", "no_tools"]);
    declared["x_capability_program"] = json!({"title":"Test dialogue program","access":{"native":{"orch_tools":true,"native":{
        "generation":{"enabled":true,"modes":["native"],"default_mode":"native"},
        "ask":{"enabled":true,"modes":["native"],"default_mode":"native","delegated_mode":"native",
            "structured_output":schema,"execution":[{"input":"ask.step","result":"ask.reply","confined_tools":true,"read_tools":true}]}
    }}}});
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(s.on.agent)
        .bind(&declared)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    s.rig.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            home: None,
            reused: true,
            harness_version: "1.0.0".into(),
            descriptor: serde_json::from_value(declared.clone()).unwrap(),
        }))
    });
    s.rig
        .commands
        .check_turn_starts_on_wire(&s.rig.world.app.app);
    let choice: AskModelIdentity =
        serde_json::from_value(json!({"backend":{"route":"ask.delegation",
        "offering":{"provider":s.on.provider,"offering":MODEL},
        "config":{"environment_id":s.on.environment,"helper_topic_id":s.tree.topic.base.id}}}))
        .unwrap();
    setting(s, "ask.models", json!([choice])).await;
    choice
}

async fn step(s: &Scene, ask: Id, previous: Option<Id>) -> orch_link_contract::commands::TurnStart {
    received_step(s, ask, previous, rig::PATIENCE).await
}

async fn received_step(
    s: &Scene,
    ask: Id,
    previous: Option<Id>,
    patience: std::time::Duration,
) -> orch_link_contract::commands::TurnStart {
    let received=tokio::time::timeout(patience,async {
        loop {
            let helpers:Vec<Id>=sqlx::query_scalar("SELECT w.helper_id FROM ask_calls c JOIN delegated_program_work w ON w.source_ask_call_id=c.id
                WHERE c.thread_id=$1 AND NOT c.settled AND w.helper_id IS DISTINCT FROM $2")
                .bind(ask).bind(previous).fetch_all(s.rig.db.pool()).await.unwrap();
            for helper in helpers {
                if let Some(start)=s.rig.turn_starts(helper).into_iter().next(){return start;}
            }
            tokio::time::sleep(s.rig.engine.config().sweep_every).await;
        }
    }).await;
    match received {
        Ok(turn) => turn,
        Err(_) => {
            let phase:Json=sqlx::query_scalar("SELECT jsonb_build_object('conversation',t.ask_state,'call',c.id IS NOT NULL,'settled',c.settled,
                'record',c.delegated_program IS NOT NULL,'helper',h.state,'step',h.starting_step,'blocked',h.blocked_reason,
                'native_turn',u.state,'reply',c.delegated_result IS NOT NULL) FROM threads t LEFT JOIN ask_calls c ON c.thread_id=t.node_id
                LEFT JOIN delegated_program_work w ON w.source_ask_call_id=c.id LEFT JOIN threads h ON h.node_id=w.helper_id
                LEFT JOIN turns u ON u.id=h.current_turn_id WHERE t.node_id=$1 ORDER BY c.settled,c.created_at DESC LIMIT 1")
                .bind(ask).fetch_one(s.rig.db.pool()).await.unwrap();
            panic!("the native Ask step did not reach the program: {phase}");
        }
    }
}

async fn answer(s: &Scene, start: &orch_link_contract::commands::TurnStart, raw: &str) {
    let usage = orch_api_contract::threads::TokenUsage {
        input_tokens: 7,
        output_tokens: 3,
        total_tokens: 10,
        ..Default::default()
    };
    s.rig
        .say(start.thread_id, rig::turn_began(start.turn_id))
        .await;
    s.rig
        .say(
            start.thread_id,
            Event::UsageUpdated(UsageUpdated {
                turn_id: start.turn_id,
                turn: usage,
                thread: usage,
                context_window: None,
                context_used: None,
                report: None,
            }),
        )
        .await;
    s.rig
        .say(start.thread_id, rig::message_began(start.turn_id, "answer"))
        .await;
    s.rig
        .say(start.thread_id, rig::message_done("answer", raw))
        .await;
    s.rig
        .say(
            start.thread_id,
            rig::turn_ended(start.turn_id, TurnOutcome::Completed),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_ask_uses_the_ordinary_read_loop_and_counts_each_native_turn_once() {
    let s = Scene::new().await;
    let choice = configure(&s, true).await;
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,created_by)
        VALUES($1,$2,'user',$3,'shared','running',NULL,1,ARRAY['ask'],$3)")
        .bind(new_id()).bind(s.tree.org.base.id).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    let listing = s
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
    assert_eq!(listing.default_model, Some(AskTarget::Backend(choice)));
    let ask = create_unseeded(&s, &s.ada).await.conversation_id;
    let receipt = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&ask])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "one-native-dialogue".into(),
            content: vec![ContentPart::Text {
                text: "Where is the help about summaries?".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    let first = step(&s, ask, None).await;
    let output = first
        .native_model
        .as_ref()
        .and_then(|model| model.program_output().ok().flatten())
        .unwrap();
    assert!(output.schema.is_some());
    assert!(first.text.contains("Where is the help about summaries?"));
    assert!(!first.text.contains(&s.ada.token));
    answer(&s,&first,r#"{"message":{"role":"assistant","content":[{"kind":"tool_call","id":"read-a","name":"docs","arguments":{"query":"summary"}},{"kind":"tool_call","id":"read-b","name":"docs","arguments":{"query":"thread"}}]},"stop":"tools"}"#).await;
    let second = step(&s, ask, Some(first.thread_id)).await;
    assert!(second.text.contains("read-a") && second.text.contains("read-b"));
    assert!(!second.text.contains(&s.ada.token));
    answer(&s,&second,r#"{"message":{"role":"assistant","content":[{"kind":"text","text":"Here is the answer."}]},"stop":"complete"}"#).await;
    s.rig.wait_turn(receipt.turn_id.unwrap(), "completed").await;
    assert!(
        read(&s, &s.ada, ask)
            .await
            .messages
            .iter()
            .any(|message| message.content.contains(&ContentPart::Text {
                text: "Here is the answer.".into()
            }))
    );
    eventually(
        "two measured Ask turns belong to the person under the chosen root",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT count(DISTINCT turn_id)=2 AND sum(total_tokens)=20 FROM usage_events
            WHERE provider_id=$1 AND user_id=$2 AND work_node_id=$3 AND purpose='ask' AND counted",
            )
            .bind(s.on.provider)
            .bind(s.ada.id())
            .bind(s.tree.org.base.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let histories = [ask, first.thread_id, second.thread_id];
    let owners: Vec<Id> = sqlx::query_scalar(
        "SELECT DISTINCT c.continuation_id FROM thread_history_turn_counts c
         JOIN thread_continuations o ON o.id=c.continuation_id WHERE o.thread_id=ANY($1)",
    )
    .bind(&histories[..])
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap();
    assert!(owners.len() >= histories.len());
    let lease = new_id();
    sqlx::query(
        "INSERT INTO leases(id,provider_id,thread_id,allowed_models,closed_at,close_reason)
        VALUES($1,$2,$3,$4,$5,'turn_ended')",
    )
    .bind(lease)
    .bind(s.on.provider)
    .bind(first.thread_id)
    .bind(vec![MODEL])
    .bind(s.rig.world.now())
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    s.rig
        .world
        .app
        .call(AskDelete, &[&ask])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let helpers = [first.thread_id, second.thread_id];
    eventually(
        "deleting Ask retires both private model contexts and their physical history",
        || async {
            orch_api_core::testing::recover_ask_turns(&s.rig.world.app.app)
                .await
                .unwrap();
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM nodes WHERE id=ANY($1)) AND
            NOT EXISTS(SELECT 1 FROM delegated_program_work WHERE helper_id=ANY($1)) AND
            NOT EXISTS(SELECT 1 FROM model_inputs WHERE node_id=ANY($1)) AND
            NOT EXISTS(SELECT 1 FROM thread_history_turn_counts WHERE continuation_id=ANY($2)) AND
            NOT EXISTS(SELECT 1 FROM leases WHERE id=$3)",
            )
            .bind(&helpers[..])
            .bind(&owners)
            .bind(lease)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
        },
    )
    .await;
    let counted:i64=sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE provider_id=$1 AND user_id=$2 AND purpose='ask' AND counted")
        .bind(s.on.provider).bind(s.ada.id()).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(counted, 2);
    assert_eq!(s.rig.commands.issued_of("provider.generate"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_program_without_schema_support_gets_one_fresh_repair_and_stop_closes_its_pending_reads()
{
    let s = Scene::new().await;
    configure(&s, false).await;
    let ask = create_unseeded(&s, &s.ada).await.conversation_id;
    let receipt = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&ask])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "bounded-repair-stop".into(),
            content: vec![ContentPart::Text {
                text: "Read our help.".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    let first = step(&s, ask, None).await;
    assert!(
        first
            .native_model
            .as_ref()
            .unwrap()
            .program_output()
            .unwrap()
            .unwrap()
            .schema
            .is_none()
    );
    answer(&s,&first,r#"{"message":{"role":"assistant","content":[{"kind":"tool_call","id":"early","name":"docs","arguments":{"query":"help"}},{"kind":"tool_call","id":"invalid","name":"docs","arguments":{"query":false}}]},"stop":"tools"}"#).await;
    let next = step(&s, ask, Some(first.thread_id)).await;
    assert!(next.text.contains("previous reply"));
    s.rig
        .world
        .app
        .call(AskStop, &[&ask])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    s.rig
        .wait_turn(receipt.turn_id.unwrap(), "interrupted")
        .await;
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM items WHERE thread_id=$1 AND payload ? 'ask_witness'",
    )
    .bind(ask)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(rows, 0);
    eventually("Stop reaches the exact active program turn", || async {
        matches!(
            s.rig.turn_state(next.turn_id).await.as_str(),
            "interrupted" | "failed"
        )
    })
    .await;
    assert_eq!(s.rig.commands.issued_of("provider.generate"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ten_ask_threads_on_one_subscription_keep_three_peoples_prompts_and_usage_separate() {
    use orch_api_contract::threads::{ThreadItemList, ThreadItemListQuery};
    let s = Scene::new().await;
    configure(&s, true).await;
    setting(&s, "providers.subscription_sharing", json!(["fake"])).await;
    for (kind, resource, permission) in [
        ("provider", s.on.provider, "provider.use"),
        ("agent", s.on.agent, "agent.use"),
        ("environment", s.on.environment, "environment.manage"),
    ] {
        s.rig
            .lets_use(kind, resource, permission, &s.dan, json!({}))
            .await;
    }
    let people = [&s.ada, &s.eve, &s.dan];
    let mut questions = Vec::new();
    for index in 0..10 {
        let who = people[index % people.len()];
        let conversation = create_unseeded(&s, who).await.conversation_id;
        questions.push((
            who,
            conversation,
            format!("Private question [{}]", new_id()),
        ));
    }
    let sends = questions.iter().map(|(who, id, text)| {
        let app = &s.rig.world.app;
        async move {
            app.call(AskMessageCreate, &[id])
                .token(&who.token)
                .body(&AskMessageBody {
                    client_id: "private-question".into(),
                    content: vec![ContentPart::Text { text: text.clone() }],
                    context: None,
                    model: None,
                })
                .send()
                .await
                .ok()
                .turn_id
                .unwrap()
        }
    });
    let turns = futures::future::join_all(sends).await;
    let scene = &s;
    let all = &questions;
    let workers=questions.iter().zip(turns).map(|((who,id,text),turn)| async move {
        let start=received_step(scene,*id,None,rig::ARCHIVE_PATIENCE).await;
        assert!(start.text.contains(text));
        for (_,_,other) in all {
            if other!=text {assert!(!start.text.contains(other));}
        }
        let other=if who.id()==scene.ada.id(){&scene.eve}else{&scene.ada};
        let denied=scene.rig.world.app.call(ThreadItemList,&[&start.thread_id])
            .query(&ThreadItemListQuery::default()).token(&other.token).send().await;
        assert!([403,404].contains(&denied.status.as_u16()));
        answer(scene,&start,r#"{"message":{"role":"assistant","content":[{"kind":"text","text":"Answered."}]},"stop":"complete"}"#).await;
        scene.rig.wait_turn(turn,"completed").await;
        start.thread_id
    });
    let helpers: std::collections::HashSet<_> = futures::future::join_all(workers)
        .await
        .into_iter()
        .collect();
    assert_eq!(helpers.len(), questions.len());
    for who in people {
        let count = questions
            .iter()
            .filter(|(person, _, _)| person.id() == who.id())
            .count() as i64;
        eventually("each person's measured usage reaches their own ledger",||async {
            sqlx::query_scalar::<_,bool>("SELECT count(*)=$4 AND count(DISTINCT turn_id)=$4 AND sum(total_tokens)=10*$4 FROM usage_events WHERE provider_id=$1 AND user_id=$2 AND work_node_id=$3 AND purpose='ask' AND counted")
                .bind(s.on.provider).bind(who.id()).bind(s.tree.org.base.id).bind(count).fetch_one(s.rig.db.pool()).await.unwrap_or(false)
        }).await;
    }
    assert_eq!(s.rig.commands.issued_of("provider.generate"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resumed_subscription_rereads_its_sources_and_keeps_the_selected_account_after_use_is_revoked()
 {
    use orch_api_contract::nodes::{NewNodeAccess, TopicCreate, TopicCreateBody, TopicCreateReply};
    use orch_db::queries::access;
    let s = Scene::new().await;
    let native = configure(&s, true).await;
    setting(&s, "providers.subscription_sharing", json!(["fake"])).await;
    let restricted=sqlx::query("UPDATE permission_entries SET options=$3 WHERE object_kind='provider' AND object_id=$1 AND permission='provider.use'
        AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL").bind(s.on.provider).bind(s.eve.id()).bind(json!({"models":[MODEL],"purposes":["ask"]})).execute(s.rig.db.pool()).await.unwrap();
    assert_eq!(restricted.rows_affected(), 1);
    let api = configured_provider(&s).await;
    s.rig
        .lets_use("provider", api, "provider.use", &s.eve, json!({}))
        .await;
    let made = s
        .rig
        .world
        .app
        .call(TopicCreate, &[&s.tree.project.base.id])
        .token(&s.ada.token)
        .body(&TopicCreateBody {
            name: Some("Read before revocation".into()),
            access: Some(NewNodeAccess::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let TopicCreateReply::Topic(source) = made else {
        panic!("one topic was created")
    };
    let mut conn = s.rig.db.acquire().await.unwrap();
    let share = access::testing::add_entry(
        &mut conn,
        &access::PageKey::new("node", source.base.id, "live"),
        "any",
        "topic.view",
        ("user", Some(s.eve.id()), None),
        None,
        s.rig.world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let ask = create_unseeded(&s, &s.eve).await.conversation_id;
    let question = AskMessageBody {
        client_id: "first".into(),
        content: vec![ContentPart::Text {
            text: "Read the source.".into(),
        }],
        context: None,
        model: Some(Some(native.clone().into())),
    };
    let first = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&ask])
        .token(&s.eve.token)
        .body(&question)
        .send()
        .await
        .ok();
    let start = step(&s, ask, None).await;
    let read_request = json!({"message":{"role":"assistant","content":[{"kind":"tool_call","id":"read-source","name":"open","arguments":{"reference":format!("topic:{}",source.base.id)}}]},"stop":"tools"});
    answer(&s, &start, &read_request.to_string()).await;
    let continued = step(&s, ask, Some(start.thread_id)).await;
    assert!(continued.text.contains("Read before revocation"));
    let reply = r#"{"message":{"role":"assistant","content":[{"kind":"text","text":"Finished."}]},"stop":"complete"}"#;
    answer(&s, &continued, reply).await;
    s.rig.wait_turn(first.turn_id.unwrap(), "completed").await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    assert!(
        access::remove_entry(&mut conn, share, s.ada.id(), s.rig.world.now())
            .await
            .unwrap()
    );
    drop(conn);
    let next = AskMessageBody {
        client_id: "second".into(),
        model: None,
        ..question.clone()
    };
    let follow = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&ask])
        .token(&s.eve.token)
        .body(&next)
        .send()
        .await
        .ok();
    let resumed = step(&s, ask, None).await;
    assert!(resumed.text.contains("no longer available to you"));
    assert!(!resumed.text.contains("Read before revocation"));
    answer(&s, &resumed, reply).await;
    s.rig.wait_turn(follow.turn_id.unwrap(), "completed").await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let entry:Id=sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='provider' AND object_id=$1 AND page='live' AND permission='provider.use' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL")
        .bind(s.on.provider).bind(s.eve.id()).fetch_one(&mut *conn).await.unwrap();
    assert!(
        access::remove_entry(&mut conn, entry, s.ada.id(), s.rig.world.now())
            .await
            .unwrap()
    );
    drop(conn);
    let choices = s
        .rig
        .world
        .app
        .call(AskModelsGet, &[])
        .token(&s.eve.token)
        .query(&AskModelsQuery {
            root_org_id: s.tree.org.base.id,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        choices.default_model,
        Some(AskTarget::Backend(AskModelIdentity::api(api, MODEL.into())))
    );
    let refusal = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&ask])
        .token(&s.eve.token)
        .body(&AskMessageBody {
            client_id: "revoked".into(),
            ..next
        })
        .send()
        .await
        .problem();
    assert_eq!(refusal.extra.get("reason"), Some(&json!("no_use")));
    assert_eq!(
        read(&s, &s.eve, ask)
            .await
            .conversation
            .model
            .as_ref()
            .map(|m| concrete_model(m).backend.clone()),
        Some(native.backend)
    );
    assert_eq!(s.rig.commands.issued_of("provider.generate"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paid_native_answer_cannot_publish_after_its_source_access_changes_during_the_final_write()
 {
    let s = Scene::new().await;
    configure(&s, true).await;
    let ask = create_unseeded(&s, &s.ada).await.conversation_id;
    let accepted = s
        .rig
        .world
        .app
        .call(AskMessageCreate, &[&ask])
        .token(&s.ada.token)
        .body(&AskMessageBody {
            client_id: "raced-answer".into(),
            content: vec![ContentPart::Text {
                text: "Report the current source.".into(),
            }],
            context: None,
            model: None,
        })
        .send()
        .await
        .ok();
    let start = step(&s, ask, None).await;
    let mut held = s.rig.db.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    sqlx::query("SELECT i.id FROM items i JOIN ask_calls c ON c.item_id=i.id WHERE c.thread_id=$1 AND NOT c.settled FOR UPDATE OF i")
        .bind(ask).fetch_one(&mut *held).await.unwrap();
    answer(&s,&start,r#"{"message":{"role":"assistant","content":[{"kind":"text","text":"Sensitive late answer"}]},"stop":"complete"}"#).await;
    eventually(
        "the validated reply reaches its canonical item write",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
            AND $1=ANY(pg_blocking_pids(pid)) AND query ILIKE '%UPDATE items SET status%')",
            )
            .bind(pid)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap()
        },
    )
    .await;
    let mut revoke = s.rig.db.begin().await.unwrap();
    orch_db::queries::access::bump_epoch_in_root(&mut revoke, s.tree.org.base.id)
        .await
        .unwrap();
    revoke.commit().await.unwrap();
    held.commit().await.unwrap();
    s.rig.wait_turn(accepted.turn_id.unwrap(), "failed").await;
    let history = read(&s, &s.ada, ask).await;
    assert!(
        !serde_json::to_string(&history)
            .unwrap()
            .contains("Sensitive late answer")
    );
    assert_eq!(s.rig.commands.issued_of("provider.generate"), 0);
}
