//! Agent gift choices for the exact pages being created, separate from the human audience summary.

use super::*;
use orch_api_contract::binding::NodeBindingOptionsGetQuery;
use orch_api_contract::nodes::{
    AgentAccessAvailability, AgentAccessChoice, AgentAccessWhere, NewNodeAccess,
};
use orch_api_contract::share::{ShareDocumentsLevel, ShareLevel, ShareParts};

fn parts(permissions: &[Permission]) -> ShareParts {
    let level = |read, write, manage| {
        if permissions.contains(&manage) {
            Some(ShareLevel::Manage)
        } else if permissions.contains(&write) {
            Some(ShareLevel::Write)
        } else if permissions.contains(&read) {
            Some(ShareLevel::Read)
        } else {
            None
        }
    };
    ShareParts {
        timeline: level(
            Permission::ThreadView,
            Permission::ThreadInteract,
            Permission::ThreadManage,
        ),
        chat: level(
            Permission::ChatView,
            Permission::ChatInteract,
            Permission::ChatManage,
        ),
        thread_fork: permissions
            .contains(&Permission::ThreadFork)
            .then_some(true),
        thread_archive: permissions
            .contains(&Permission::ThreadArchive)
            .then_some(true),
        documents: if permissions.contains(&Permission::DocManage) {
            Some(ShareDocumentsLevel::Manage)
        } else if permissions.contains(&Permission::DocEdit) {
            Some(ShareDocumentsLevel::Edit)
        } else if permissions.contains(&Permission::DocView) {
            Some(ShareDocumentsLevel::Read)
        } else {
            None
        },
    }
}

pub(super) async fn environment_readers(
    conn: &mut Conn,
    principal: &Principal,
    parent: Id,
    context: &NodeBindingOptionsGetQuery,
    environment: Option<&environments::EnvironmentRow>,
) -> ApiResult<Option<orch_api_contract::binding::EnvironmentReaderOffer>> {
    let Some(environment) = environment.filter(|row| !row.id.is_nil()) else {
        return Ok(None);
    };
    let topic_change = match context.topic_access {
        None => None,
        Some(NewNodeAccess::Defaults) => Some(orch_access_contract::pages::PageChange::default()),
        Some(NewNodeAccess::Private) => Some(orch_access_contract::pages::PageChange {
            whole: Some(orch_access_contract::pages::Whole::Private),
            ..Default::default()
        }),
    };
    let thread_change = super::super::make::page_of(context.thread_access.as_ref())?;
    let policies = orch_engine::access::resource_sharing_policies(conn, environment.id)
        .await
        .map_err(ApiError::engine)?;
    let result = orch_authz::creation::environment_reader_change(
        conn,
        principal,
        parent,
        topic_change.as_ref(),
        thread_change.as_ref(),
        environment.id,
        &policies,
    )
    .await;
    match result {
        Ok(_) => Ok(Some(orch_api_contract::binding::EnvironmentReaderOffer {
            allowed: true,
            reason: None,
        })),
        Err(
            orch_authz::pages::ChangeError::Denied(_)
            | orch_authz::pages::ChangeError::Outside { .. }
            | orch_authz::pages::ChangeError::Locked { .. }
            | orch_authz::pages::ChangeError::ExternalLifetime { .. },
        ) => Ok(Some(orch_api_contract::binding::EnvironmentReaderOffer {
            allowed: false,
            reason: Some(
                "The environment's page does not allow giving its View doors to these readers"
                    .into(),
            ),
        })),
        Err(error) => Err(refusals::change_failed(conn, error).await),
    }
}

pub(super) async fn availability(
    conn: &mut Conn,
    principal: &Principal,
    parent: Id,
    context: &NodeBindingOptionsGetQuery,
    environment: Option<&environments::EnvironmentRow>,
) -> ApiResult<AgentAccessAvailability> {
    let preset =
        setting::<AgentAccessChoice>(conn, Place::Node(parent), keys::THREADS_AGENT_ACCESS)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::internal("the thread creation preset is unset"))?;
    let topic_change = match context.topic_access {
        None => None,
        Some(NewNodeAccess::Defaults) => Some(orch_access_contract::pages::PageChange::default()),
        Some(NewNodeAccess::Private) => Some(orch_access_contract::pages::PageChange {
            whole: Some(orch_access_contract::pages::Whole::Private),
            ..Default::default()
        }),
    };
    let thread_change = super::super::make::page_of(context.thread_access.as_ref())?;
    let placement = environment
        .map(|environment| {
            environment
                .placement
                .parse::<orch_access_contract::permissions::Placement>()
        })
        .transpose()
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    let actual_environment = environment
        .map(|environment| environment.id)
        .filter(|id| !id.is_nil());
    let projected = orch_authz::creation::agent_permissions(
        conn,
        principal,
        parent,
        topic_change.as_ref(),
        thread_change.as_ref(),
        placement,
        actual_environment,
    )
    .await;
    let projected = match projected {
        Ok(projected) => projected,
        Err(error) => return Err(refusals::change_failed(conn, error).await),
    };
    Ok(offer(preset, projected))
}

/// An already-admitted fork/adoption preview uses its selected topic and prospective
/// page directly. It never enters the separate NodeBindingOptions admission door.
pub(crate) async fn existing_topic(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    page: Option<&orch_access_contract::pages::PageChange>,
    copy_from: Option<Id>,
    environment: Option<&environments::EnvironmentRow>,
) -> ApiResult<AgentAccessAvailability> {
    let preset = setting::<AgentAccessChoice>(conn, Place::Node(topic), keys::THREADS_AGENT_ACCESS)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("the thread creation preset is unset"))?;
    let placement = environment
        .map(|row| {
            row.placement
                .parse::<orch_access_contract::permissions::Placement>()
        })
        .transpose()
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    let environment = environment.map(|row| row.id).filter(|id| !id.is_nil());
    let projected = orch_authz::creation::fork_agent_permissions(
        conn,
        principal,
        topic,
        page,
        copy_from,
        placement,
        environment,
    )
    .await;
    match projected {
        Ok(projected) => Ok(offer(preset, projected)),
        Err(error) => Err(refusals::change_failed(conn, error).await),
    }
}

fn offer(
    preset: AgentAccessChoice,
    [thread, topic, project]: [Vec<Permission>; 3],
) -> AgentAccessAvailability {
    AgentAccessAvailability {
        preset,
        choices: vec![
            AgentAccessChoice {
                where_: AgentAccessWhere::Nothing,
                parts: ShareParts::default(),
            },
            AgentAccessChoice {
                where_: AgentAccessWhere::Thread,
                parts: parts(&thread),
            },
            AgentAccessChoice {
                where_: AgentAccessWhere::Topic,
                parts: parts(&topic),
            },
            AgentAccessChoice {
                where_: AgentAccessWhere::Project,
                parts: parts(&project),
            },
        ],
    }
}
