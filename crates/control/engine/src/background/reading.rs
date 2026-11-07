//! The roster's ordinary Thread read and optional recorded-provider directory disclosure.

use super::{BackgroundError, Capture, sources};
use crate::error::EngineError;
use crate::operation_scope::{BindingError, Prepared, targets};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::background::ThreadBackgroundGet;
use orch_api_contract::routes::Endpoint;
use orch_authz::Caller;
use orch_db::Conn;
use orch_types::Id;
use std::collections::{BTreeSet, HashSet};

pub(super) fn binding(error: BindingError) -> BackgroundError {
    match error {
        BindingError::Refused => BackgroundError::Scope,
        BindingError::Catalogue(error) => EngineError::unreadable(error).into(),
    }
}

fn target(error: targets::TargetError) -> BackgroundError {
    match error {
        targets::TargetError::Missing => BackgroundError::NotFound,
        targets::TargetError::Db(error) => error.into(),
        targets::TargetError::Authz(error) => error.into(),
    }
}

async fn prepare(
    conn: &mut Conn,
    caller: &Caller,
    thread: Id,
    providers: BTreeSet<Id>,
) -> Result<(Prepared, HashSet<Id>), BackgroundError> {
    let kind = KeyTargetKind::Object(ObjectKind::Thread);
    let node = targets::node(conn, thread).await.map_err(target)?;
    if node.object.kind != kind {
        return Err(BackgroundError::NotFound);
    }
    let mut plan = Prepared::new(ThreadBackgroundGet::ROUTE, ThreadBackgroundGet::ROUTE.name)
        .map_err(binding)?;
    plan.bind(0, kind, &[node]).map_err(binding)?;
    // Optional directories never make the ordinary roster require access to every
    // provider. Only actual recorded sources covered by this same read are retained.
    let kind = KeyTargetKind::Object(ObjectKind::Provider);
    let mut directories = Vec::new();
    let mut allowed = HashSet::new();
    for id in providers {
        let provider = match targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id,
            },
        )
        .await
        {
            Ok(provider) => provider,
            Err(targets::TargetError::Missing)
            | Err(targets::TargetError::Authz(orch_authz::Error::NotFound(_))) => continue,
            Err(error) => return Err(target(error)),
        };
        if plan
            .permits_binding(caller, 1, kind, std::slice::from_ref(&provider))
            .map_err(binding)?
        {
            directories.push(provider);
            allowed.insert(id);
        }
    }
    if directories.is_empty() {
        plan.bind_empty(1).map_err(binding)?;
    } else {
        plan.bind(1, kind, &directories).map_err(binding)?;
    }
    Ok((plan, allowed))
}

/// HTTP and socket admission use the exact existing source segments, with no native I/O.
pub async fn read_scope(
    conn: &mut Conn,
    caller: &Caller,
    thread: Id,
) -> Result<Prepared, BackgroundError> {
    let providers = sources(conn, thread)
        .await?
        .into_iter()
        .map(|source| source.provider_id)
        .collect();
    Ok(prepare(conn, caller, thread, providers).await?.0)
}

/// Bind the actual captured sources without restoring a caller or manufacturing a reader.
/// Both transports load their fresh Principal and current second-step hold after this footprint.
pub async fn read_scope_for_capture(
    conn: &mut Conn,
    caller: &Caller,
    thread: Id,
    capture: &Capture,
) -> Result<(Prepared, HashSet<Id>), BackgroundError> {
    let providers = capture
        .sessions
        .iter()
        .map(|(source, _)| source.provider_id)
        .collect();
    prepare(conn, caller, thread, providers).await
}
