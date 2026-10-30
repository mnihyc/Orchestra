//! Identity and committed-effect evidence for one synthetic creation, without changing its guard or latch.

use crate::common::World;
use orch_agent_contract::events::OrchTool;
use orch_engine::tools::CallContext;
use orch_link_contract::commands::{Command, CommandKind, OrchToolResult};
use orch_types::{Hash, Id, Json, new_id};
use serde_json::json;

/// Only records observations inside the producer's existing transaction. Rolled-back effects leave no record.
pub async fn install(world: &World) {
    let sql = r#"
        CREATE TABLE guard_rest_created_probe(
            object_id uuid PRIMARY KEY, root_id uuid NOT NULL, xact text NOT NULL, pid integer NOT NULL,
            inserted_at timestamptz NOT NULL, insert_snapshot text NOT NULL,
            insert_mode text, insert_seen jsonb, insert_advance jsonb,
            commit_reached_at timestamptz, commit_snapshot text, commit_mode text,
            commit_seen jsonb, commit_advance jsonb, commit_epochs jsonb);
        CREATE TABLE guard_rest_withdraw_probe(
            entry_id uuid NOT NULL, xact text NOT NULL, pid integer NOT NULL,
            commit_reached_at timestamptz NOT NULL, commit_snapshot text NOT NULL,
            mode text, seen jsonb, advance jsonb, epochs jsonb, PRIMARY KEY(entry_id,xact));
        CREATE FUNCTION guard_rest_record_created() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            INSERT INTO guard_rest_created_probe(object_id,root_id,xact,pid,inserted_at,insert_snapshot,
                insert_mode,insert_seen,insert_advance)
            VALUES(NEW.id,NEW.org_id,pg_current_xact_id()::text,pg_backend_pid(),clock_timestamp(),pg_current_snapshot()::text,
                current_setting('orch.guard_epoch',true),orch_setting_json('orch.authz_seen','{}'),
                orch_setting_json('orch.authz_advance','[]'));
            RETURN NEW;
        END $$;
        CREATE TRIGGER guard_rest_record_created AFTER INSERT ON documents
            FOR EACH ROW EXECUTE FUNCTION guard_rest_record_created();
        CREATE FUNCTION guard_rest_record_commit() RETURNS trigger LANGUAGE plpgsql AS $$
        DECLARE wanted guard_rest_target; epochs jsonb;
        BEGIN
            SELECT COALESCE(jsonb_object_agg(org_id::text,epoch),'{}') INTO epochs FROM authz_epochs;
            UPDATE guard_rest_created_probe SET commit_reached_at=clock_timestamp(),commit_snapshot=pg_current_snapshot()::text,
                commit_mode=current_setting('orch.guard_epoch',true),commit_seen=orch_setting_json('orch.authz_seen','{}'),
                commit_advance=orch_setting_json('orch.authz_advance','[]'),commit_epochs=epochs
             WHERE xact=OLD.xact::text;
            SELECT * INTO wanted FROM guard_rest_target WHERE singleton;
            IF wanted.kind='grant' AND COALESCE(current_setting('guard_rest.pause_commit',true),'')='on' THEN
                INSERT INTO guard_rest_withdraw_probe(entry_id,xact,pid,commit_reached_at,commit_snapshot,mode,seen,advance,epochs)
                VALUES(wanted.identity,OLD.xact::text,pg_backend_pid(),clock_timestamp(),pg_current_snapshot()::text,
                    current_setting('orch.guard_epoch',true),orch_setting_json('orch.authz_seen','{}'),
                    orch_setting_json('orch.authz_advance','[]'),epochs);
            END IF;
            RETURN OLD;
        END $$;
        -- Alphabetical trigger order puts this observer after guard_rest_pause_commit. It does not release its latch.
        CREATE TRIGGER guard_rest_record_commit BEFORE DELETE ON commit_work
            FOR EACH ROW EXECUTE FUNCTION guard_rest_record_commit();
    "#;
    sqlx::raw_sql(sql).execute(world.pool.pool()).await.unwrap();
}

pub struct Invocation {
    pub call: CallContext,
    pub document: Id,
    topic: Id,
    title: String,
    receipt: String,
    result: Id,
    fingerprint: String,
}

pub fn prepare(
    world: &World,
    agent: Id,
    turn: Id,
    topic: Id,
    directory: &str,
    title: &str,
    text: &str,
) -> Invocation {
    let call_ref = new_id().to_string();
    let call = CallContext {
        machine: world.machine_id,
        thread: agent,
        turn,
        call_ref,
        attempt: None,
        tool: OrchTool::EditDocument,
        arguments: json!({"command":"create","path":format!("{directory}{title}.md"),"markdown":text}),
    };
    let document = orch_engine::ids::derived(
        "document",
        &[agent.as_bytes().as_slice(), call.call_ref.as_bytes()],
    );
    let result = orch_engine::ids::tool_result(agent, &call.call_ref);
    let fingerprint = Hash::of(&serde_json::to_vec(&call.arguments).unwrap()).to_string();
    Invocation {
        call,
        document,
        topic,
        title: title.to_owned(),
        receipt: format!("text-create:{document}"),
        result,
        fingerprint,
    }
}

fn answer_facts(answer: &OrchToolResult) -> Json {
    json!({"thread":answer.thread_id,"call_ref":answer.call_ref,"attempt":answer.attempt,
        "error":answer.error.as_ref().map(|error|error.code.as_str()),
        "document":answer.result.as_ref().map(|result|&result["id"]),
        "created":answer.result.as_ref().map(|result|&result["created"]),
        "fingerprint":Hash::of(&serde_json::to_vec(answer).unwrap()).to_string()})
}

impl Invocation {
    pub fn start(
        &self,
        world: &World,
    ) -> tokio::task::JoinHandle<Result<(), orch_engine::EngineError>> {
        let engine = world.engine();
        let call = self.call.clone();
        tokio::spawn(async move { engine.answer_tool_call(&call).await })
    }

    /// No content or document name is exposed: identities, equality checks and fingerprints only.
    pub async fn evidence(&self, world: &World, grant: Id) -> Json {
        let stored:Json=sqlx::query_scalar("SELECT jsonb_build_object('sampled_at',clock_timestamp(),'snapshot',pg_current_snapshot()::text,
            'document',(SELECT jsonb_build_object('id',d.id,'node',d.node_id,'caller_matches',d.created_by_thread=$2,
                'destination_matches',d.node_id=$5,'title_matches',d.title=$4,'created_at',d.created_at,
                'xmin',d.xmin::text,'fingerprint',md5(to_jsonb(d)::text)) FROM documents d WHERE d.id=$1),
            'receipt',(SELECT jsonb_build_object('operation',o.operation_id,'xmin',o.xmin::text,'created_at',o.created_at,
                'arguments_fingerprint',md5(o.arguments::text),'result_fingerprint',md5(o.result::text),
                'destination_matches',o.arguments->'parent'=jsonb_build_object('kind','node_docs','id',$5::uuid),
                'title_matches',o.arguments->>'title'=$4,'document_matches',o.result->>'document'=$1::text)
                FROM document_tree_operations o WHERE o.actor_kind='agent' AND o.actor_id=$2 AND o.request_ref=$3),
            'creation_grants',(SELECT COALESCE(jsonb_agg(jsonb_build_object('entry',e.id,'object',e.object_id,
                'holder',e.holder_id,'permission',e.permission,'source',e.source,'xmin',e.xmin::text) ORDER BY e.id),'[]')
                FROM permission_entries e WHERE e.object_kind='document' AND e.object_id=$1 AND e.source='creation' AND e.removed_at IS NULL),
            'audit',(SELECT COALESCE(jsonb_agg(jsonb_build_object('id',a.id,'actor',a.actor_id,'at',a.at,
                'xmin',a.xmin::text,'fingerprint',md5(a.details::text)) ORDER BY a.id),'[]')
                FROM audit_log a WHERE a.action='document.created_by_agent' AND a.object_id=$1),
            'producer',(SELECT to_jsonb(p) FROM guard_rest_created_probe p WHERE p.object_id=$1),
            'withdrawal',(SELECT to_jsonb(p) FROM guard_rest_withdraw_probe p WHERE p.entry_id=$6 ORDER BY p.commit_reached_at DESC LIMIT 1),
            'epochs',(SELECT COALESCE(jsonb_object_agg(org_id::text,epoch),'{}') FROM authz_epochs))")
            .bind(self.document).bind(self.call.thread).bind(&self.receipt).bind(&self.title).bind(self.topic).bind(grant)
            .fetch_one(world.pool.pool()).await.unwrap();
        let retained: Vec<_> = world
            .commands
            .issued_commands("orch.tool_result")
            .into_iter()
            .filter(|(_, command)| command.id == self.result)
            .map(|(_, command)| {
                let body: OrchToolResult = serde_json::from_value(command.body).unwrap();
                answer_facts(&body)
            })
            .collect();
        let delivered: Vec<_> = world
            .seen
            .of_kind(CommandKind::OrchToolResult)
            .into_iter()
            .filter_map(|command| match command {
                Command::OrchToolResult(answer)
                    if answer.thread_id == self.call.thread
                        && answer.call_ref == self.call.call_ref =>
                {
                    Some(answer_facts(&answer))
                }
                _ => None,
            })
            .collect();
        json!({"call_ref":self.call.call_ref,"thread":self.call.thread,"turn":self.call.turn,
            "arguments_fingerprint":self.fingerprint,"expected_document":self.document,"result_command":self.result,
            "receipt_ref":self.receipt,"stored":stored,"retained":retained,"delivered":delivered})
    }

    pub async fn assert_fresh(&self, world: &World, grant: Id) -> Json {
        let evidence = self.evidence(world, grant).await;
        assert!(
            evidence["stored"]["document"].is_null()
                && evidence["stored"]["receipt"].is_null()
                && evidence["stored"]["producer"].is_null(),
            "a fresh call cannot reuse an earlier document, receipt or committed producer: {evidence}"
        );
        assert_eq!(
            evidence["stored"]["creation_grants"],
            json!([]),
            "the derived identity has no earlier creation grant"
        );
        assert_eq!(
            evidence["stored"]["audit"],
            json!([]),
            "the derived identity has no earlier creation audit"
        );
        assert_eq!(
            evidence["retained"],
            json!([]),
            "the canonical result is absent before the call"
        );
        assert_eq!(
            evidence["delivered"],
            json!([]),
            "the native answer lookup is empty before the call"
        );
        evidence
    }

    pub fn assert_answer(&self, answer: &OrchToolResult, evidence: &Json) {
        assert_eq!(answer.thread_id, self.call.thread);
        assert_eq!(answer.call_ref, self.call.call_ref);
        assert_eq!(
            evidence["retained"].as_array().unwrap().len(),
            1,
            "one exact canonical result: {evidence}"
        );
        assert_eq!(
            evidence["delivered"].as_array().unwrap().len(),
            1,
            "one exact native answer: {evidence}"
        );
        assert_eq!(evidence["retained"][0], answer_facts(answer));
        assert_eq!(evidence["delivered"][0], answer_facts(answer));
        if let Some(result) = &answer.result {
            assert_eq!(
                result["id"],
                json!(self.document),
                "this answer names this exact call's derived document"
            );
        }
    }
}
