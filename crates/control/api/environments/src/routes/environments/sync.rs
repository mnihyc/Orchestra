//! Native environment-feed members prepare typed sources before their one renderer.

use super::reading;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::syncs::Reading;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::environment::EnvironmentSyncGet;

use orch_api_contract::environment::EnvironmentSyncGetQuery;

use orch_api_contract::environment_sources::GetSource as Source;

use orch_api_contract::environment_sources::SYNC;

use orch_authz::Caller;

use orch_db::Conn;

use orch_db::queries::feeds;

use orch_db::queries::jobs as job_rows;

pub use orch_api_base::routes::environments::sync::JOBS_MAX;

pub use orch_api_base::routes::environments::sync::MEMBERS;

pub use orch_api_base::routes::environments::sync::put_together;

pub use orch_api_base::routes::environments::sync::NativeMember;

pub use orch_api_base::routes::environments::sync::Part;

pub use orch_api_base::routes::environments::sync::TheEnvironment;

pub use orch_api_base::routes::environments::sync::Library;

pub use orch_api_base::routes::environments::sync::TheLibrary;

pub use orch_api_base::routes::environments::sync::FeedItem;

pub use orch_api_base::routes::environments::sync::JobRows;

pub use orch_api_base::routes::environments::sync::FeedJobs;

pub use orch_api_base::routes::environments::sync::source_reference;

pub use orch_api_base::routes::environments::sync::job_names;

pub use orch_api_base::routes::environments::sync::TheJobs;

pub use orch_api_base::routes::environments::sync::Feed;

pub use orch_api_base::routes::environments::sync::Prepared;
pub(super) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        EnvironmentSyncGet,
        native::selected::<Feed>,
        native::selected_read::<Feed>,
    );
}

fn same_job(old: &job_rows::JobRow, now: &job_rows::JobRow) -> bool {
    old.id == now.id
        && old.kind == now.kind
        && old.environment_id == now.environment_id
        && old.node_id == now.node_id
        && old.provider_id == now.provider_id
        && old.created_by == now.created_by
        && old.created_at == now.created_at
        && old.batch_id == now.batch_id
}
orch_api_base::__api_bind_60d76202f7!();

fn fact(
    source: Source,
    kind: KeyTargetKind,
    targets: Vec<KeyResolvedTarget>,
) -> native::Facts<Source> {
    native::Facts::collection(
        source,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(kind, targets)]
        },
    )
}
orch_api_base::__api_bind_4a2e170369!();

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &EnvironmentSyncGetQuery,
    opened: Option<&Prepared>,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    if query.since.is_some_and(|since| since < 0) {
        return Err(ApiError::invalid(
            "since",
            "is a number of the feed, from 0",
        ));
    }
    let subject = ctx.id("id")?;
    let root = if native::modern(caller) {
        vec![native::Facts::one(
            Source::Environment,
            KeyTargetKind::Object(ObjectKind::Environment),
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: subject,
                },
            )
            .await?,
        )]
    } else {
        Vec::new()
    };
    let plan = native::bind_known(ctx, &SYNC, &root)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    let progress_only =
        reading::decide(conn, &principal, subject).await? == reading::Reading::ThroughAThread;
    let seq = feeds::environment_seqs(conn, &[subject])
        .await?
        .pop()
        .map(|(_, seq)| seq)
        .ok_or_else(ApiError::not_found)?;
    let ahead = query.since.is_some_and(|since| since > seq);
    let mut reading = Reading {
        ctx,
        conn,
        principal: &principal,
        progress_only,
        subject,
        seq,
        since: if ahead { Some(seq) } else { query.since },
    };
    let mut parts = Vec::new();
    let mut facts = Vec::new();
    for (index, member) in MEMBERS.iter().enumerate() {
        let (part, more) = match opened {
            None => member.prepare(&mut reading, caller, &plan).await?,
            Some(old) => {
                member
                    .refresh(&mut reading, caller, &plan, &old.parts[index])
                    .await?
            }
        };
        parts.push(part);
        facts.extend(more);
    }
    Ok((
        Prepared {
            parts,
            seq,
            since: query.since,
            progress_only,
            complete: !ahead,
        },
        facts,
    ))
}
orch_api_base::__api_bind_4b39fa92cc!();
