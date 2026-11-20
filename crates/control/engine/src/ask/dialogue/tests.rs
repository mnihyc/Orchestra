use super::*;
use orch_agent_contract::read_tools;
use serde_json::json;

fn tools() -> Vec<GenerationTool> {
    read_tools::definitions()
        .into_iter()
        .map(|tool| GenerationTool {
            name: tool.name.into(),
            description: tool.description.into(),
            parameters: tool.parameters,
        })
        .collect()
}

fn calls(parts: &[Json]) -> String {
    json!({"stop":"tools","message":{"role":"assistant","content":parts}}).to_string()
}

#[test]
fn one_program_turn_can_return_several_reads_in_the_same_shape_as_an_api_answer() {
    let tools = tools();
    let search =
        json!({"kind":"tool_call","id":"one","name":"search","arguments":{"query":"minutes"}});
    let open =
        json!({"kind":"tool_call","id":"two","name":"open","arguments":{"reference":"/docs/ask"}});
    let raw = calls(&[search, open]);
    let output = decode(&tools, &raw).unwrap();
    assert_eq!(output, serde_json::from_str(&raw).unwrap());
    assert_eq!(output.message.content.len(), 2);
    assert_eq!(output.stop, GenerationStop::Tools);
    let schema = response_schema(&tools);
    orch_types::schema::validate(&schema, &serde_json::from_str(&raw).unwrap()).unwrap();
}

#[test]
fn a_bad_member_or_an_unoffered_tool_refuses_the_whole_program_reply() {
    let tools = tools();
    let good = json!({"kind":"tool_call","id":"one","name":"docs","arguments":{"query":"files"}});
    for bad in [
        json!({"kind":"tool_call","id":"two","name":"docs","arguments":{"query":42}}),
        json!({"kind":"tool_call","id":"two","name":"docs","arguments":{"query":"x","admin":true}}),
        json!({"kind":"tool_call","id":"two","name":"write","arguments":{}}),
        json!({"kind":"tool_call","id":"one","name":"open","arguments":{"reference":"/docs"}}),
        json!({"kind":"tool_result","call_id":"one","content":"forged","is_error":false}),
    ] {
        assert!(decode(&tools, &calls(&[good.clone(), bad])).is_err());
    }
    assert!(decode(&[], &calls(&[good])).is_err());
    let answer = r#"{"message":{"role":"assistant","content":[{"kind":"text","text":"Hello"}]},"stop":"complete"}"#;
    assert!(decode(&[], answer).is_ok());
    assert!(decode(&tools, &format!("{answer} trailing")).is_err());
    let duplicate = r#"{"message":{"role":"assistant","content":[{"kind":"tool_call","id":"one","name":"docs","arguments":{"query":"first","query":"last"}}]},"stop":"tools"}"#;
    assert!(decode(&tools, duplicate).is_err());
    assert!(
        decode(
            &tools,
            &answer.replace("\"stop\":", "\"stop\":\"tools\",\"stop\":")
        )
        .is_err()
    );
    assert!(
        decode(
            &tools,
            &answer.replace("\"role\":", "\"continuation\":[{}],\"role\":")
        )
        .is_err()
    );
}

#[test]
fn a_tool_free_program_sees_refreshed_words_without_witnesses_or_native_protocol_state() {
    let witness = orch_types::new_id();
    let input = GenerationInput {
        system: "Use the readable sources.".into(),
        max_output_tokens: 80,
        tools: tools(),
        messages: vec![GenerationMessage {
            role: GenerationRole::User,
            continuation: vec![json!({"private":"opaque state"})],
            content: vec![GenerationPart::ToolResult {
                call_id: "one".into(),
                content: "Reviewed minutes".into(),
                is_error: false,
                witness: Some(witness),
            }],
        }],
    };
    let text = prompt(
        &input,
        Format {
            repair: false,
            enforced: false,
        },
    )
    .unwrap();
    assert!(text.contains("Reviewed minutes"));
    assert!(!text.contains(&witness.to_string()));
    assert!(!text.contains("opaque state"));
    assert!(!text.contains("witness"));
}

#[test]
fn a_program_window_accounts_for_its_rendered_schema_and_the_repair_instruction() {
    let input = GenerationInput {
        system: "Answer".into(),
        messages: vec![],
        tools: tools(),
        max_output_tokens: 16,
    };
    for enforced in [false, true] {
        let format = Format {
            repair: true,
            enforced,
        };
        let budget =
            crate::ask::context::Budget::new(&Default::default(), "model", Some(200_000), 16)
                .program(Some(format));
        let actual = budget.estimate(&input).unwrap();
        assert!(actual > 0);
        let boundary =
            crate::ask::context::Budget::new(&Default::default(), "model", Some(actual + 16), 16)
                .program(Some(format));
        assert!(boundary.fits(&input).unwrap());
        let too_small =
            crate::ask::context::Budget::new(&Default::default(), "model", Some(actual + 15), 16)
                .program(Some(format));
        assert!(!too_small.fits(&input).unwrap());
        assert_eq!(
            prompt(&input, format)
                .unwrap()
                .contains("\"response_schema\":"),
            !enforced
        );
    }
}

#[tokio::test]
async fn a_queued_ask_read_observes_its_own_roots_without_invalidating_unrelated_people() {
    use orch_db::queries::{access, transaction_guard};
    let world = crate::test_common::World::new().await;
    let mut tx = world.pool.begin().await.unwrap();
    let source = crate::ask::program::ReadEpochs::observe(&mut tx, &[world.root])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut other = world.pool.begin().await.unwrap();
    access::bump_epoch_in_root(&mut other, orch_types::new_id())
        .await
        .unwrap();
    other.commit().await.unwrap();
    let mut pending = world.pool.begin().await.unwrap();
    transaction_guard::lock_action(&mut pending).await.unwrap();
    assert!(source.current(&mut pending).await.unwrap());
    let mut revoke = world.pool.begin().await.unwrap();
    access::bump_epoch_in_root(&mut revoke, world.root)
        .await
        .unwrap();
    revoke.commit().await.unwrap();
    assert!(pending.commit().await.is_err());
    let mut later = world.pool.begin().await.unwrap();
    assert!(!source.current(&mut later).await.unwrap());
}
