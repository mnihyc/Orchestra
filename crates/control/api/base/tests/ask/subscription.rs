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
    let received=tokio::time::timeout(rig::PATIENCE,async {
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
    assert_eq!(listing.default_model, Some(choice));
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
