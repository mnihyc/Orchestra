//! Cached vendor quota is an observation, not authority to cancel a new native request.
//! These controls reuse the production Dispatcher, peer and file-backed command journal;
//! the provider endpoint is scripted and makes no external model or credential request.

use super::*;
use orch_link_contract::commands::{
    CompletionPurpose, ProviderComplete, ProviderCompleteResult, ProviderEmbed,
    ProviderEmbedResult, encode_vector,
};
use orch_types::Json;

const MODEL: &str = "owned-quota-model";
const INPUT: &str = "A later ordinary request";
const ANSWER: &str = "The native endpoint accepted the request";
const STOP: &str = "Provider switched off";

pub(super) fn reply(
    context: CommandContext,
    command: Command,
) -> Result<CommandResult, CommandError> {
    if context.key.is_some() {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "provider complete and embed use the catalogue's unkeyed queue",
        ));
    }
    match command {
        Command::ProviderComplete(body) if body.model_name == MODEL && body.input == INPUT => {
            Ok(CommandResult::ProviderComplete(ProviderCompleteResult {
                text: ANSWER.into(),
                usage: Default::default(),
                request_ref: Some(context.id.to_string()),
                timing: Default::default(),
            }))
        }
        Command::ProviderEmbed(body)
            if body.model_name == MODEL && body.inputs == [INPUT] && body.dimensions == Some(2) =>
        {
            Ok(CommandResult::ProviderEmbed(ProviderEmbedResult {
                // Two finite components match the explicitly requested dimension.
                vectors: vec![encode_vector(&[3.0, 4.0])],
                usage: Default::default(),
                request_ref: Some(context.id.to_string()),
                timing: Default::default(),
            }))
        }
        _ => Err(CommandError::new(
            ErrorCode::Conflict,
            "the provider endpoint received a different request",
        )),
    }
}

async fn provider(fixture: &Fixture) -> Proof<Id> {
    let provider = new_id();
    sqlx::query("INSERT INTO providers
        (id,name,owner_id,org_id,home_id,kind,key_machine_id,machine_id,upstreams,protocols,arrangements,status)
        SELECT $1,'Owned quota endpoint',owner_id,org_id,home_node_id,'api',$3,$3,$4,
            ARRAY['openai-responses','openai-embeddings'],ARRAY['key_separated'],'ready'
        FROM environments WHERE id=$2")
        .bind(provider).bind(fixture.environment).bind(fixture.machine)
        .bind(json!([
            {"protocol":"openai-responses","base_url":"https://quota.invalid/v1","options":{"auth":"none"}},
            {"protocol":"openai-embeddings","base_url":"https://quota.invalid/v1","options":{"auth":"none"}}
        ])).execute(&fixture.test.pool).await?;
    sqlx::query(
        "INSERT INTO provider_models
        (id,provider_id,model_name,display_name,enabled,source,created_by)
        SELECT $1,id,$3,'Owned quota model',true,'manual',owner_id FROM providers WHERE id=$2",
    )
    .bind(new_id())
    .bind(provider)
    .bind(MODEL)
    .execute(&fixture.test.pool)
    .await?;
    Ok(provider)
}

fn request(fixture: &Fixture, provider: Id, kind: CommandKind) -> Proof<StoredCommand> {
    let body = match kind {
        CommandKind::ProviderComplete => serde_json::to_value(ProviderComplete {
            model_parameters: None,
            provider_id: provider,
            purpose: CompletionPurpose::Summary,
            protocol: "openai-responses".into(),
            model_name: MODEL.into(),
            params: None,
            system: "Answer this ordinary request".into(),
            input: INPUT.into(),
            // A small ordinary output budget suffices for the scripted sentence.
            max_output_tokens: 32,
        })?,
        CommandKind::ProviderEmbed => serde_json::to_value(ProviderEmbed {
            model_parameters: None,
            provider_id: provider,
            protocol: "openai-embeddings".into(),
            model_name: MODEL.into(),
            params: None,
            inputs: vec![INPUT.into()],
            dimensions: Some(2),
        })?,
        _ => return Err("only complete and embed belong to this provider control".into()),
    };
    Ok(StoredCommand {
        id: Some(new_id()),
        machine: fixture.machine,
        kind: kind.as_str().into(),
        key: SerialKey::None.wire(),
        deadline: Some(Time::new(
            fixture.clock.now() + chrono::Duration::seconds(i64::from(kind.deadline_secs())),
        )),
        body,
        authority: None,
    })
}

async fn cached_exhaustion(fixture: &Fixture, provider: Id) -> Proof<Json> {
    let facts = json!({
        "ordinary_usage_allowed": false,
        // A future reset prevents the test from succeeding by expiry or a timer refresh.
        "limit_reached": {"type":"weekly","resets_at":Time::new(fixture.clock.now()+chrono::Duration::days(7))},
        // Positive vendor credits can coexist with a stale included-plan observation.
        "balances": [{"id":"account","unit":"credits","remaining":"12.50"}],
        "field_times": {
            "ordinary_usage_allowed":fixture.clock.time(),
            "limit_reached":fixture.clock.time(),
            "balances":fixture.clock.time()
        }
    });
    let _: orch_provider_contract::facts::ProviderFacts = serde_json::from_value(facts.clone())?;
    sqlx::query("UPDATE providers SET status='exhausted',status_detail='Cached weekly quota observation',facts=$2,facts_at=$3 WHERE id=$1")
        .bind(provider).bind(&facts).bind(fixture.clock.now()).execute(&fixture.test.pool).await?;
    Ok(facts)
}

async fn succeeds(fixture: &Fixture, original: &StoredCommand) -> Proof {
    let id = original.id.ok_or("the request has no owned id")?;
    let outcome = within(fixture.dispatcher.wait(id)).await??;
    let CommandOutcome::Succeeded(body) = outcome else {
        return Err(
            format!("a normal native request was blocked by cached quota: {outcome:?}").into(),
        );
    };
    let kind: CommandKind = original.kind.parse()?;
    match CommandResult::decode(kind, body.clone())? {
        CommandResult::ProviderComplete(answer) => {
            assert_eq!(answer.text, ANSWER);
            assert_eq!(answer.request_ref, Some(id.to_string()));
        }
        CommandResult::ProviderEmbed(answer) => {
            assert_eq!(answer.vectors, vec![encode_vector(&[3.0, 4.0])]);
            assert_eq!(answer.request_ref, Some(id.to_string()));
        }
        _ => return Err("the native result belongs to a different command".into()),
    }
    let entry = fixture.server.entry(id).ok_or("no native journal entry")?;
    assert!(matches!(entry.step, Step::Done { result } if result.outcome == Ok(body)));
    let wire = fixture
        .native
        .wire
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let sent: Vec<_> = wire.iter().filter(|command| command.id == id).collect();
    assert_eq!(
        sent.len(),
        1,
        "one normal request reached the native endpoint"
    );
    assert_eq!(sent[0].body, original.body);
    assert_eq!(sent[0].key, original.key);
    assert_eq!(Some(sent[0].deadline), original.deadline);
    assert!(
        wire.iter().all(|command| matches!(
            command.kind.as_str(),
            "provider.complete" | "provider.embed" | "machine.probe"
        )),
        "no provider refresh, reset or other preparatory command was sent"
    );
    Ok(())
}

async fn failed_without_native(fixture: &Fixture, original: &StoredCommand) -> Proof {
    let id = original.id.ok_or("the request has no owned id")?;
    let outcome = within(fixture.dispatcher.wait(id)).await??;
    assert!(
        matches!(outcome, CommandOutcome::Failed(ref error)
        if error.code == ErrorCode::Conflict.as_str() && error.detail == STOP),
        "{outcome:?}"
    );
    fixture.completed_probe().await?;
    assert!(
        fixture.server.entry(id).is_none(),
        "a stopped request entered the native journal"
    );
    assert!(
        !fixture
            .native
            .wire
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|command| command.id == id)
    );
    let saved = rows::command(&mut *fixture.test.pool.acquire().await?, id)
        .await?
        .ok_or("the refused durable command disappeared")?;
    assert_eq!(saved.state, "failed");
    assert_eq!(saved.body, original.body);
    assert_eq!(saved.serial_key, original.key);
    assert_eq!(Some(Time::new(saved.deadline_at)), original.deadline);
    Ok(())
}

#[tokio::test]
async fn ready_provider_complete_and_embed_reach_the_native_journal() -> Proof {
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let provider = provider(&fixture).await?;
    for kind in [CommandKind::ProviderComplete, CommandKind::ProviderEmbed] {
        let original = request(&fixture, provider, kind)?;
        fixture.dispatcher.issue_stored(original.clone()).await?;
        succeeds(&fixture, &original).await?;
    }
    fixture.close().await
}

async fn quota_allows_normal_request(kind: CommandKind, before_wire: bool) -> Proof {
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    let provider = provider(&fixture).await?;
    let original = request(&fixture, provider, kind)?;
    let facts = if before_wire {
        // Pause only after production admission committed a queued row, before adoption.
        let (read, release) = gate(original.id.ok_or("no request id")?, Phase::Adoption);
        let dispatcher = fixture.dispatcher.clone();
        let carried = original.clone();
        let issuer = tokio::spawn(async move { dispatcher.issue_stored(carried).await });
        within(read).await??;
        let facts = cached_exhaustion(&fixture, provider).await?;
        release
            .send(())
            .map_err(|_| "the admitted issuer was lost")?;
        within(issuer).await???;
        facts
    } else {
        let facts = cached_exhaustion(&fixture, provider).await?;
        fixture.dispatcher.issue_stored(original.clone()).await?;
        facts
    };
    succeeds(&fixture, &original).await?;
    let saved: (String, Json, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT status,facts,facts_requested_at FROM providers WHERE id=$1")
            .bind(provider)
            .fetch_one(&fixture.test.pool)
            .await?;
    assert_eq!(
        saved,
        ("exhausted".into(), facts, None),
        "dispatch neither refreshes nor rewrites the cached observation"
    );
    fixture.close().await
}

#[tokio::test]
async fn cached_quota_with_credits_does_not_cancel_new_complete_admission() -> Proof {
    quota_allows_normal_request(CommandKind::ProviderComplete, false).await
}

#[tokio::test]
async fn cached_quota_with_credits_does_not_cancel_new_embed_admission() -> Proof {
    quota_allows_normal_request(CommandKind::ProviderEmbed, false).await
}

#[tokio::test]
async fn cached_quota_with_credits_does_not_cancel_complete_before_transmission() -> Proof {
    quota_allows_normal_request(CommandKind::ProviderComplete, true).await
}

#[tokio::test]
async fn cached_quota_with_credits_does_not_cancel_embed_before_transmission() -> Proof {
    quota_allows_normal_request(CommandKind::ProviderEmbed, true).await
}

#[derive(Clone, Copy)]
enum Stop {
    Provider,
    Model,
    Accepted,
}

async fn stop(fixture: &Fixture, provider: Id, original: &StoredCommand, why: Stop) -> Proof {
    match why {
        Stop::Provider => {
            sqlx::query("UPDATE providers SET status='disabled',status_detail=$2 WHERE id=$1")
                .bind(provider)
                .bind(STOP)
                .execute(&fixture.test.pool)
                .await?;
        }
        Stop::Model => {
            sqlx::query("UPDATE provider_models SET enabled=false,pause_note='Model switched off' WHERE provider_id=$1")
                .bind(provider).execute(&fixture.test.pool).await?;
        }
        Stop::Accepted => {
            sqlx::query("INSERT INTO summary_calls(id,node_id,topic_id,provider_id,person_id,model_name,created_at,purpose,cancel_reason)
                SELECT $1,home_id,home_id,id,owner_id,$3,$4,$5,$6 FROM providers WHERE id=$2")
                .bind(original.id.ok_or("no accepted request id")?).bind(provider).bind(MODEL)
                .bind(fixture.clock.now())
                .bind(if original.kind == CommandKind::ProviderEmbed.as_str() { "embeddings" } else { "summary" })
                .bind(STOP).execute(&fixture.test.pool).await?;
        }
    }
    Ok(())
}

async fn lifecycle_still_blocks(why: Stop) -> Proof {
    let mut fixture = Fixture::new(false).await?;
    within(fixture.connect(false)).await??;
    // Both catalogue kinds retain each stop at initial admission and on later transmission.
    for before_wire in [false, true] {
        for kind in [CommandKind::ProviderComplete, CommandKind::ProviderEmbed] {
            let provider = provider(&fixture).await?;
            let original = request(&fixture, provider, kind)?;
            if before_wire {
                let (read, release) = gate(original.id.ok_or("no request id")?, Phase::Adoption);
                let dispatcher = fixture.dispatcher.clone();
                let carried = original.clone();
                let issuer = tokio::spawn(async move { dispatcher.issue_stored(carried).await });
                within(read).await??;
                stop(&fixture, provider, &original, why).await?;
                release
                    .send(())
                    .map_err(|_| "the stopped issuer was lost")?;
                within(issuer).await???;
            } else {
                stop(&fixture, provider, &original, why).await?;
                fixture.dispatcher.issue_stored(original.clone()).await?;
            }
            failed_without_native(&fixture, &original).await?;
            sqlx::query("UPDATE providers SET status='ready' WHERE id=$1")
                .bind(provider)
                .execute(&fixture.test.pool)
                .await?;
            sqlx::query("UPDATE provider_models SET enabled=true WHERE provider_id=$1")
                .bind(provider)
                .execute(&fixture.test.pool)
                .await?;
            fixture.dispatcher.issue_stored(original.clone()).await?;
            failed_without_native(&fixture, &original).await?;
            // Re-enabling permits a new id, while the old accepted/stopped id stays terminal.
            let later = request(&fixture, provider, kind)?;
            fixture.dispatcher.issue_stored(later.clone()).await?;
            succeeds(&fixture, &later).await?;
        }
    }
    fixture.close().await
}

#[tokio::test]
async fn explicit_provider_disable_still_blocks_complete_and_embed() -> Proof {
    lifecycle_still_blocks(Stop::Provider).await
}

#[tokio::test]
async fn explicit_model_disable_still_blocks_complete_and_embed() -> Proof {
    lifecycle_still_blocks(Stop::Model).await
}

#[tokio::test]
async fn an_accepted_stop_still_blocks_complete_and_embed_after_the_provider_is_ready() -> Proof {
    lifecycle_still_blocks(Stop::Accepted).await
}
