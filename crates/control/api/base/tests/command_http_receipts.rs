//! A real native offering/key/queue proves receipt classification before its success ACK.
mod places;
mod resources;
mod support;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{environment::*, environment_sources::MCP_IMPORT, routes::Endpoint};
use orch_api_core::{dispatch::StoredCommand, middleware::authentication};
use orch_authz::{Caller, CapturedOperation, Principal, TokenGrant};
use orch_db::queries::{identity, link};
use orch_engine::{
    commands::{CommandAuthority, Outgoing},
    environment_mcp::{
        self, CopyCommand,
        importing::{Input, Offering},
    },
    operation_scope::{
        continuing::Source, http_origin::HttpOrigin, request_origin::Credential, sources,
    },
};
use orch_link_contract::policy::LocalPolicy;
use orch_types::{Id, Time, new_id};
use serde_json::json;
use support::World;
type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn wrapped_import_ack_checks_the_actual_native_frame_and_sealed_budget_without_restoring_its_ended_key()
-> Proof {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let mut conn = world.conn().await;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut conn,
        company.admin.id(),
        "receipt-source",
    )
    .await?;
    let policy = LocalPolicy {
        environment_roots: vec!["/srv/mcp".into()],
        exec_offers: vec!["plain".into()],
        mcp_add: true,
        ..Default::default()
    };
    sqlx::query(
        "UPDATE machines SET online=true,facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1",
    )
    .bind(machine)
    .bind(json!(policy))
    .execute(&mut *conn)
    .await?;
    drop(conn);
    let environment = world
        .app
        .call(MachineEnvironmentCreate, &[&machine])
        .token(&company.admin.token)
        .body(&MachineEnvironmentCreateBody {
            data_sources: None,
            home_id: Some(company.acme),
            name: "Receipt environment".into(),
            root: "/srv/mcp/work".into(),
            home_node_id: None,
            remote_write: None,
            terminal: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .map_err(|_| "native registration returned a receipt")?
        .base
        .id;
    let mut conn = world.conn().await;
    sqlx::query("UPDATE environments SET facts=jsonb_set(facts,'{mcp_found}',$2::jsonb) WHERE id=$1").bind(environment).bind(json!([{"source":"codex","scope":"user","name":"search","directory":null,"transport":"stdio","digest":"one","tools":2}])).execute(&mut *conn).await?;
    let key = new_id();
    let bearer = authentication::new_token(authentication::API_TOKEN_PREFIX);
    let hash = authentication::token_hash(&bearer);
    let scope: KeyScope = serde_json::from_value(
        json!({"rules":[{"target":{"kind":"environment","id":environment,"reach":"self"},"resources":["environment","mcp_server"],"allow":[{"operation":MCP_IMPORT.operation,"actions":["manage"]}]}]}),
    )?;
    let end = Time::new(world.now() + chrono::Duration::hours(1));
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id: key,
            user_id: company.admin.id(),
            name: "Native receipt origin",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(end.get()),
        },
        Some(&json!(scope)),
    )
    .await?;
    let caller = Caller::token(
        company.admin.id(),
        TokenGrant::from_stored_scope(&[], None, Some(scope), Some(end))?.for_token(key),
    );
    let selected = environment_mcp::select_import(
        &mut conn,
        environment,
        "codex",
        json!("user"),
        None,
        "search",
    )
    .await?;
    let input = Input::capture(&mut conn, &selected, new_id()).await?;
    let (value, facts) =
        Offering::prepare(&mut conn, &caller, &input, Time::new(world.now())).await?;
    let bound = sources::bind_required(EnvironmentMcpImportCreate::ROUTE, &MCP_IMPORT, &facts)?;
    let caller = bound.admit(&caller, false)?;
    let principal = Principal::load(&mut conn, caller.clone(), Time::new(world.now())).await?;
    Offering::require(&mut conn, &principal, &input, &value).await?;
    let command = Outgoing::new(new_id(), machine, input.serial(), &input.command())?;
    let native =
        CommandAuthority::mcp_import(CopyCommand::capture(&caller, None, input, &command)?);
    let raw = HttpOrigin::capture(
        CapturedOperation::capture(&caller)?,
        Credential::Token { id: key, hash },
        false,
        false,
        Some(end),
    )?;
    let wrapped = raw.command(&command, native.clone())?;
    drop(conn);
    world
        .app
        .app
        .link
        .dispatcher()
        .issue_stored_before(
            StoredCommand {
                id: Some(command.id),
                machine,
                kind: command.kind.as_str().into(),
                key: command.key.wire(),
                deadline: command.deadline,
                body: command.body.clone(),
                authority: Some(wrapped),
            },
            Box::new(|| Ok(())),
        )
        .await?;
    let mut conn = world.conn().await;
    let stored = link::command_with_authority(&mut conn, command.id)
        .await?
        .ok_or("real queued command missing")?;
    let private = stored.authority.clone().ok_or("private wrapper missing")?;
    let authority: CommandAuthority = serde_json::from_value(private.clone())?;
    assert!(private["authority"]["sealed_deadline"].is_string());
    assert!(authority.mcp_import_ack(&stored.command)?.is_none());
    assert!(
        !authority.matches_native_receipt(&native, &stored.command)?,
        "an ACK view is not a completed effect"
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key)
        .bind(world.now())
        .execute(&mut *conn)
        .await?;
    assert!(authority.mcp_import_ack(&stored.command)?.is_none());
    for change in ["id", "machine", "kind", "body", "key", "deadline"] {
        let mut other = stored.command.clone();
        match change {
            "id" => other.id = new_id(),
            "machine" => other.machine_id = new_id(),
            "kind" => other.kind = "mcp.forget".into(),
            "body" => other.body["name"] = json!("different"),
            "key" => other.serial_key = Some("machine".into()),
            _ => other.deadline_at += chrono::Duration::seconds(1),
        }
        assert!(authority.mcp_import_ack(&other).is_err(), "{change}");
    }
    let mut absent = private.clone();
    absent["authority"]
        .as_object_mut()
        .ok_or("wrapper not object")?
        .remove("sealed_deadline");
    let absent: CommandAuthority = serde_json::from_value(absent)?;
    assert!(absent.mcp_import_ack(&stored.command).is_err());
    let mut nested = private.clone();
    nested["authority"]["native"] = json!({"kind":"channel","evidence":private.clone()});
    let nested: CommandAuthority = serde_json::from_value(nested)?;
    assert!(nested.mcp_import_ack(&stored.command).is_err());
    assert_eq!(
        link::command_with_authority(&mut conn, command.id)
            .await?
            .ok_or("queue changed")?
            .authority,
        Some(private)
    );
    let filed: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_servers WHERE id=$1")
        .bind(serde_json::from_value::<Id>(
            command.body["server_id"].clone(),
        )?)
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(filed, 0);
    Ok(())
}
