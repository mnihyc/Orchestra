//! Private carrier compatibility controls; serialization is not an account/ACL proof.

use super::Outgoing;
use crate::operation_scope::Prepared;
use crate::provider_setup::LoginCommandAuthority;
use orch_access_contract::key_scope::{KeyObjectRef, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_authz::{Caller, CapturedOperation};
use orch_link_contract::commands::{ProviderLoginCancel, PtyClose, SerialKey};
use orch_types::{Id, new_id};
use serde_json::{Value, json};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn terminal_fixture() -> Proof<(Value, Id, Id)> {
    let (user, environment, terminal, machine) = (new_id(), new_id(), new_id(), new_id());
    // These opaque IDs exercise the private data format only. No verifier or ordinary
    // account authorization is bypassed or represented as having succeeded here.
    let kind = KeyTargetKind::Object(ObjectKind::Environment);
    let target = KeyResolvedTarget {
        object: KeyObjectRef {
            kind,
            id: Some(environment),
        },
        node_ancestors: Vec::new(),
        document_ancestors: Vec::new(),
        credential_account: None,
    };
    let mut plan = Prepared::for_selection("TerminalClose")?;
    plan.bind(0, kind, std::slice::from_ref(&target))?;
    plan.bind(1, kind, &[target])?;
    let caller = plan.admit(&Caller::session(user), false)?;
    let origin = CapturedOperation::capture(&caller)?;
    let id = crate::ids::derived("pty.close", &[terminal.as_bytes().as_slice()]);
    let outgoing = Outgoing::new(
        id,
        machine,
        SerialKey::None,
        &PtyClose {
            terminal_id: terminal,
        },
    )?;
    let mut value = serde_json::to_value(outgoing)?;
    value["login_authority"] = json!({"version":1,"kind":"terminal_close","authority":{
        "origin":origin,"terminal_id":terminal,"environment_id":environment,
        "machine_id":machine,"environment_generation":7
    }});
    Ok((value, user, environment))
}

#[test]
fn existing_login_private_json_and_native_body_remain_exact() -> Proof {
    let (provider, agent, machine) = (new_id(), new_id(), new_id());
    let authority = LoginCommandAuthority::cancel(provider, None);
    let old = serde_json::to_value(&authority)?;
    let outgoing = Outgoing::new(
        new_id(),
        machine,
        SerialKey::Harness {
            provider_id: provider,
            agent_id: agent,
        },
        &ProviderLoginCancel {
            provider_id: provider,
            agent_id: agent,
        },
    )?
    .with_login_authority(authority);
    let value = serde_json::to_value(&outgoing)?;
    assert_eq!(value["login_authority"], old);
    assert_eq!(
        value["body"],
        json!({"provider_id":provider,"agent_id":agent})
    );
    let decoded: Outgoing = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, value);
    Ok(())
}

#[test]
fn legacy_missing_private_metadata_stays_absent_not_a_new_origin() -> Proof {
    let outgoing = Outgoing::new(
        new_id(),
        new_id(),
        SerialKey::None,
        &PtyClose {
            terminal_id: new_id(),
        },
    )?;
    let value = serde_json::to_value(&outgoing)?;
    assert!(value.get("login_authority").is_none());
    let decoded: Outgoing = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, value);
    Ok(())
}

#[test]
fn disjoint_terminal_private_metadata_roundtrips_without_touching_native_body() -> Proof {
    let (value, _, _) = terminal_fixture()?;
    let decoded: Outgoing = serde_json::from_value(value.clone())?;
    assert_eq!(decoded.body, value["body"]);
    assert!(decoded.body.get("origin").is_none());
    assert!(decoded.body.get("authority").is_none());
    assert_eq!(serde_json::to_value(decoded)?, value);
    Ok(())
}

#[test]
fn new_private_authority_debug_never_prints_origin_or_environment() -> Proof {
    let (value, user, environment) = terminal_fixture()?;
    let decoded: Outgoing = serde_json::from_value(value)?;
    let shown = format!("{decoded:?}");
    assert!(!shown.contains(&user.to_string()));
    assert!(!shown.contains(&environment.to_string()));
    assert!(!shown.contains("TerminalClose"));
    Ok(())
}

#[test]
fn malformed_unknown_and_mixed_private_metadata_never_becomes_legacy_none() -> Proof {
    let (value, _, _) = terminal_fixture()?;
    let mut changed = value.clone();
    changed["login_authority"]["version"] = json!(2);
    assert!(serde_json::from_value::<Outgoing>(changed).is_err());
    let mut changed = value.clone();
    changed["login_authority"]["kind"] = json!("future_operation");
    assert!(serde_json::from_value::<Outgoing>(changed).is_err());
    let mut changed = value.clone();
    changed["login_authority"]["provider"] = json!(new_id());
    assert!(serde_json::from_value::<Outgoing>(changed).is_err());
    let mut changed = value.clone();
    changed["login_authority"]["authority"]["origin"] = json!({"operation":"TerminalClose"});
    assert!(serde_json::from_value::<Outgoing>(changed).is_err());
    let mut changed = value;
    changed["login_authority"] = json!({"version":1,"kind":"snapshot_read","authority":{}});
    assert!(serde_json::from_value::<Outgoing>(changed).is_err());
    let mut changed = terminal_fixture()?.0;
    changed["login_authority"] = Value::Null;
    assert!(
        serde_json::from_value::<Outgoing>(changed).is_err(),
        "present null metadata is malformed, not absent legacy authority"
    );
    Ok(())
}
