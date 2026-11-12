//! Native original-Thread deletion inside the existing full-history fork mechanism.
use crate::{Engine, EngineError, archive::deletion};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    access::{AccessPreset, ThreadAccess},
    duplicates::{
        CopySubject, CopySubjectKind, DuplicateDestination, MoveOutDeleteEffect, MoveOutDeletePlan,
    },
    forks::{ForkHistory, ForkRequest, ForkResult, HistoryMode},
};
use orch_authz::{Act, Policy, Principal};
use orch_db::{
    Conn,
    queries::{hierarchy, threads},
};
use orch_types::{Hash, Id, Time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct Refusal(&'static str);

pub(super) fn refused(reason: &'static str) -> EngineError {
    EngineError::BeforeEffect(Box::new(Refusal(reason)))
}

fn deletion_error(error: crate::archive::RemovalScheduleError) -> EngineError {
    match error {
        crate::archive::RemovalScheduleError::Internal(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}

/// Actual source and native deletion operands, never a retained authority or a lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    source: Id,
    destination: Id,
    revision: i64,
    stamp: Hash,
    resources: Vec<ObjectRef>,
    environments: Vec<Id>,
    #[serde(default)]
    open_sides: Vec<Id>,
}
impl Review {
    #[must_use]
    pub fn destination(&self) -> Id {
        self.destination
    }
    #[must_use]
    pub fn public(&self) -> MoveOutDeletePlan {
        MoveOutDeletePlan {
            subject: CopySubject {
                kind: CopySubjectKind::Thread,
                id: self.source,
            },
            destination: DuplicateDestination::Node {
                id: self.destination,
            },
            revision: self.revision,
            stamp: self.stamp.to_string(),
            effects: vec![
                MoveOutDeleteEffect::StopBoundWork,
                MoveOutDeleteEffect::RemoveSource,
            ],
        }
    }
    pub(crate) fn target_survives(&self, environment: Option<Id>) -> bool {
        environment.is_none_or(|id| !self.environments.contains(&id))
    }
}
/// Move carries the full native conversation and starts every access row from destination defaults.
pub fn request(mut request: ForkRequest) -> Result<ForkRequest, EngineError> {
    if request.result != ForkResult::New
        || request
            .history
            .as_ref()
            .is_some_and(|history| history.mode != HistoryMode::All)
    {
        return Err(refused(
            "a Move copies the whole conversation into a new thread",
        ));
    }
    request.access = Some(ThreadAccess::Preset(AccessPreset::Defaults));
    request.agent_access = None;
    request.history = Some(ForkHistory {
        mode: HistoryMode::All,
        from_turn: None,
        to_turn: None,
    });
    Ok(request)
}
/// Pure original Delete selection plus the indexed current conversation revision.
pub async fn capture(conn: &mut Conn, source: Id, destination: Id) -> Result<Review, EngineError> {
    let native = deletion::inspect(conn, source)
        .await
        .map_err(deletion_error)?;
    if native.node().kind != "thread" || native.side_parent().is_some() {
        return Err(refused(
            "only a main conversation can move into another organization",
        ));
    }
    let thread = threads::thread(conn, source)
        .await?
        .ok_or_else(|| refused("the moving thread disappeared"))?;
    let target = hierarchy::node(conn, destination)
        .await?
        .filter(|node| {
            node.kind == "topic" && node.deleted_at.is_none() && node.archived_at.is_none()
        })
        .ok_or_else(|| refused("choose a live destination topic"))?;
    if target.org_id == thread.org_id {
        return Err(refused("use the existing Move within this organization"));
    }
    let content: Option<i64> = sqlx::query_scalar(
        "SELECT max(seq_updated) FROM items WHERE thread_id=$1 AND continuation_id=$2",
    )
    .bind(source)
    .bind(thread.active_continuation_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let resources = native
        .resources()
        .iter()
        .map(|row| {
            row.kind
                .parse()
                .map(|kind| ObjectRef { kind, id: row.id })
                .map_err(EngineError::unreadable)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let open_sides = threads::forking::side_threads_of(conn, source, false).await?;
    let associations:Vec<_>=native.resources().iter().map(|row|serde_json::json!({"kind":row.kind,"id":row.id,"home":row.home_id,"org":row.org_id,"machine":row.machine_id})).collect();
    let stamp = crate::jobs::command_identity::hash(
        &serde_json::json!({"source":source,"destination":destination,"set":native.set(),"resources":associations,"environments":native.environments(),"view":thread.active_continuation_id,"execution":thread.execution_revision,"segment":thread.current_segment_id,"turn":thread.current_turn_id,"content":content,"directory":thread.workspace_dir,"name":thread.name,"home":thread.topic_id}),
    )?;
    Ok(Review {
        source,
        destination,
        revision: native.node().revision,
        stamp,
        resources,
        environments: native.environments().to_vec(),
        open_sides,
    })
}
/// The exact real resource cascade used by NodeDelete, with explicit checked absence.
pub async fn resources(
    conn: &mut Conn,
    review: &Review,
) -> Result<Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>, EngineError> {
    let mut groups = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
    for object in &review.resources {
        let target = crate::operation_scope::targets::resource_identity(conn, *object)
            .await
            .map_err(EngineError::unreadable)?;
        groups.entry(target.object.kind).or_default().push(target);
    }
    Ok(groups.into_iter().collect())
}
/// Current owner and original Delete/Export/creation-root checks remain native and conjunctive.
pub async fn require<I: super::operation::Intent>(
    conn: &mut Conn,
    principal: &Principal,
    review: &Review,
) -> Result<(), EngineError> {
    if capture(conn, review.source, review.destination).await? != *review {
        return Err(refused("the reviewed original Thread deletion changed"));
    }
    if I::KIND == super::operation::Kind::MoveConfirm && !review.open_sides.is_empty() {
        return Err(refused(
            "close or keep every listed side thread before moving its main conversation",
        ));
    }
    let subject = crate::copy_out::Subject::Object(ObjectRef {
        kind: ObjectKind::Thread,
        id: review.source,
    });
    if !crate::copy_out::owns(conn, principal, subject)
        .await
        .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?
        || !orch_authz::nodes::check(
            conn,
            principal,
            review.source,
            Permission::ThreadManage,
            Act::Delete,
            Policy::default(),
        )
        .await?
        .is_allowed()
    {
        return Err(refused(
            "only a current owner admitted by original Delete may move this thread",
        ));
    }
    if crate::copy_out::authorize(conn, principal, subject, Some(review.destination))
        .await
        .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?
        .is_none()
    {
        return Err(refused(
            "the thread no longer leaves its original organization",
        ));
    }
    Ok(())
}
/// Hold the original native cascade before the fork writes any of its own source markers.
pub(crate) async fn prepare_ending(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    review: &Review,
) -> Result<deletion::Prepared, EngineError> {
    let prepared = engine
        .prepare_home_deletion_in(conn, review.source, None)
        .await
        .map_err(deletion_error)?;
    require::<super::operation::MoveConfirm>(conn, principal, review).await?;
    Ok(prepared)
}

/// Native full-history forks carry this thread, not its independent open side conversations.
/// Their existing Close or Keep paths must settle them before the Move can be confirmed.
pub async fn review_sides(
    conn: &mut Conn,
    review: &Review,
    draft: &mut super::Draft,
) -> Result<Vec<orch_api_contract::nodes::NodeRef>, EngineError> {
    let mut sides = Vec::new();
    for id in &review.open_sides {
        let side = threads::thread(conn, *id)
            .await?
            .filter(|side| side.side_of == Some(review.source) && side.deleted_at.is_none())
            .ok_or_else(|| refused("the reviewed open side threads changed"))?;
        draft.plan.blockers.push(super::blocker(
            "open_side_thread",
            format!(
                "Close or keep “{}” before moving this conversation.",
                side.name
            ),
        ));
        draft.disclosure.push(ObjectRef {
            kind: ObjectKind::Thread,
            id: side.node_id,
        });
        sides.push(super::thread_ref(&side));
    }
    Ok(sides)
}
/// Consume that held native Delete after the independent conversation is complete, in the same commit.
pub(crate) async fn finish(
    engine: &Engine,
    conn: &mut Conn,
    prepared: &deletion::Prepared,
    actor: Id,
    now: Time,
) -> Result<(), EngineError> {
    let removed = engine
        .delete_prepared_thread_before(conn, prepared, Some(actor), now, || Ok(()))
        .await
        .map_err(deletion_error)?;
    let action = orch_access_contract::audit::AuditAction::NodeDeleted;
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(actor),
        action.as_str(),
        "thread",
        Some(prepared.node().id),
        "deleted the original conversation after its native fork completed",
    );
    entry.history = action.is_history_line();
    entry.node_id = Some(prepared.node().id);
    entry.org_id = Some(prepared.node().org_id);
    entry.details = serde_json::json!({"nodes":removed.gone,"archived_first":removed.archived_first,"threads":removed.settled.threads,"turns":removed.settled.turns,"inputs":removed.settled.inputs,"approvals":removed.settled.approvals,"environments":removed.environments.len(),"resources_before":[],"deleted_nodes":removed.deleted_nodes});
    orch_db::audit::append(conn, &entry, &orch_db::audit::Origin::default(), now.get()).await?;
    Ok(())
}

/// The bounded native explicit-notice audience, read once with the preflight's original source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audience {
    pub people: Vec<Id>,
    pub count: u64,
}
/// Existing explicit notices admit at most two hundred current eligible people in one synchronous act.
pub const NOTICE_LIMIT: u32 = 200;
/// Select actual authors of the copied active lineage, then the existing personal preferences and Source View.
/// This runs once outside snapshot retries, never in source/command rechecks.
pub async fn audience(
    conn: &mut Conn,
    source: Id,
    by: Id,
    now: Time,
) -> Result<Audience, EngineError> {
    let thread = threads::thread(conn, source)
        .await?
        .ok_or_else(|| EngineError::unreadable("the original conversation disappeared"))?;
    let mut cursor = None;
    let mut people = Vec::new();
    let mut count = 0_u64;
    loop {
        let page:Vec<Id>=sqlx::query_scalar("WITH owners AS(SELECT $2::uuid AS owner,0::bigint AS first_seq,9223372036854775807::bigint AS last_seq UNION ALL SELECT owner_continuation_id,first_seq,last_seq FROM thread_continuation_spans WHERE continuation_id=$2) SELECT DISTINCT i.author_id FROM owners o JOIN items i ON i.thread_id=$1 AND i.continuation_id=o.owner AND i.seq_created BETWEEN o.first_seq AND o.last_seq JOIN users u ON u.id=i.author_id WHERE u.kind='human' AND u.status='active' AND i.author_id<>$3 AND i.kind NOT IN ('marker','announcement') AND ($4::uuid IS NULL OR i.author_id>$4) ORDER BY i.author_id LIMIT $5").bind(source).bind(thread.active_continuation_id).bind(by).bind(cursor).bind(i64::from(NOTICE_LIMIT)).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let Some(last) = page.last().copied() else {
            break;
        };
        cursor = Some(last);
        let eligible =
            orch_db::queries::notifications::operations::receivers(conn, &page, "person.notify")
                .await?;
        for user in eligible {
            let reader = match Principal::load(
                conn,
                orch_authz::Caller::session(user).with_admin_mode(false),
                now,
            )
            .await
            {
                Ok(reader) => reader,
                Err(
                    orch_authz::Error::NoSuchUser(_)
                    | orch_authz::Error::NotFound(_)
                    | orch_authz::Error::Suspended { .. }
                    | orch_authz::Error::Archived { .. },
                ) => continue,
                Err(error) => return Err(error.into()),
            };
            if orch_authz::nodes::check(
                conn,
                &reader,
                source,
                Permission::ThreadView,
                Act::Use,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                count = count
                    .checked_add(1)
                    .ok_or_else(|| EngineError::unreadable("the native notice count overflowed"))?;
                if people.len() < NOTICE_LIMIT as usize {
                    people.push(user);
                }
            }
        }
        if page.len() < NOTICE_LIMIT as usize {
            break;
        }
    }
    Ok(Audience { people, count })
}
/// A confirmed choice is retained on the existing native fork plan, never as a second job/lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub notify: bool,
    pub people: Vec<Id>,
}
