//! Transport bindings of shared read operations, not a separate key privilege rule.

use super::{Fail, Inner, server::Connection, watches};
use crate::key_operations::admission::Prepared;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations;
use orch_api_contract::routes::Endpoint;
use orch_realtime_contract::feeds::Feed;

/// Shared thread, topic and native observation bindings. Other key feed bindings are
/// closed until their mixed per-row/event operation admission is installed; sessions keep
/// their existing producers. Every later delivery resolves the current stored target again.
pub(crate) async fn admit(
    inner: &Inner,
    connection: &Connection,
    feed: &Feed,
) -> Result<Option<orch_authz::Caller>, Fail> {
    if watches::delivery_guard(inner, connection).await?.is_none() {
        return Ok(None);
    }
    let caller = connection.credential().current;
    if caller.grant().is_none() {
        return Ok(Some(caller));
    }
    let mut conn = inner.db.acquire().await?;
    if let Feed::Doc(document) = feed {
        let plan =
            match crate::routes::documents::read_operation::root_plan(&mut conn, *document).await {
                Ok(plan) => plan,
                Err(error) if matches!(error.problem.status, 403 | 404 | 410) => return Ok(None),
                Err(error) => return Err(Fail::other(error)),
            };
        return match plan.admit(&caller) {
            Ok(admission) => Ok(Some(admission.caller)),
            Err(error) if matches!(error.problem.status, 403 | 404 | 410) => Ok(None),
            Err(error) => Err(Fail::other(error)),
        };
    }
    if let Feed::Chat(chat) = feed {
        let plan = match crate::routes::chats::read_operation::root_plan(&mut conn, *chat).await {
            Ok(plan) => plan,
            Err(error) if matches!(error.problem.status, 403 | 404 | 410) => return Ok(None),
            Err(error) => return Err(Fail::other(error)),
        };
        return match plan.admit(&caller) {
            Ok(admission) => Ok(Some(admission.caller)),
            Err(error) if matches!(error.problem.status, 403 | 404 | 410) => Ok(None),
            Err(error) => Err(Fail::other(error)),
        };
    }
    if let Feed::Topic(topic) = feed {
        let plan = match crate::routes::topics::sync::root_plan(&mut conn, *topic).await {
            Ok(plan) => plan,
            Err(error) if matches!(error.problem.status, 403 | 404 | 410) => return Ok(None),
            Err(error) => return Err(Fail::other(error)),
        };
        // Subscription admission binds only the actual topic. Every read rebuilds its
        // complete current row footprint through the same HTTP TopicSyncGet producer.
        return match plan.admit(&caller) {
            Ok(admission) => Ok(Some(admission.caller)),
            Err(error) if matches!(error.problem.status, 403 | 404 | 410) => Ok(None),
            Err(error) => Err(Fail::other(error)),
        };
    }
    if let Feed::ThreadBackground(thread) = feed {
        let result = orch_engine::background::read_scope(&mut conn, &caller, *thread).await;
        return match result {
            Ok(plan) => match plan.admit(&caller, true) {
                Ok(caller) => Ok(Some(caller)),
                Err(orch_engine::operation_scope::BindingError::Refused) => Ok(None),
                Err(error) => Err(Fail::other(error)),
            },
            Err(
                orch_engine::background::BackgroundError::Scope
                | orch_engine::background::BackgroundError::NotFound,
            ) => Ok(None),
            Err(error) => Err(Fail::other(error)),
        };
    }
    let (thread, route, selection) = match feed {
        Feed::Thread(thread) => (
            thread,
            orch_api_contract::threads::ThreadSyncGet::ROUTE,
            "read",
        ),
        Feed::ThreadRaw(thread) => (thread, orch_api_contract::threads::ThreadRawList::ROUTE, ""),
        _ => return Ok(None),
    };
    let operation = operations::for_route(&route).map_err(Fail::other)?;
    let variant = operation
        .variants
        .iter()
        .find(|variant| variant.name == selection)
        .ok_or_else(|| Fail::other("the shared thread read variant is absent"))?;
    let target = match crate::key_operations::targets::node(&mut conn, *thread).await {
        Ok(target) => target,
        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => return Ok(None),
        Err(error) => return Err(Fail::other(error)),
    };
    let kind = KeyTargetKind::Object(ObjectKind::Thread);
    if target.object.kind != kind {
        return Ok(None);
    }
    let mut prepared = Prepared::new(route, &variant.operation).map_err(Fail::other)?;
    for index in 0..prepared.variant().facets.len() {
        prepared
            .bind(index, kind, std::slice::from_ref(&target))
            .map_err(Fail::other)?;
    }
    match prepared.admit(&caller) {
        Ok(admission) => Ok(Some(admission.caller)),
        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => Ok(None),
        Err(error) => Err(Fail::other(error)),
    }
}
