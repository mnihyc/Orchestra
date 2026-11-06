//! Requested native Defaults projections under the Node opening's own read footprint.

use super::node_opening::{bind, error, permits};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan};
use crate::routes::access::operations::{group_targets, projection_names};
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::resources::providers;
use crate::routes::settings::readers::{self, PreparedSecrets, PreparedSettings};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::chats::{
    NodeChatOpeningGetQuery, NodeDefaultsOpening, NodeDefaultsProvider,
};
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::nodes::NodeKind;
use orch_authz::{Caller, Policy, Principal};
use orch_db::Conn;
use orch_settings_contract::registry::keys;
use orch_types::Id;
use std::collections::{BTreeSet, HashMap};

const PROVIDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Provider);
const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);
const SYSTEM: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::System);

#[derive(Clone)]
enum Part<T> {
    Ready(T),
    Failed {
        problem: Problem,
        retry_after: Option<u64>,
    },
}

impl<T> Part<T> {
    async fn failed(ctx: &Ctx, conn: &mut Conn, caller: &Caller, failed: ApiError) -> Self {
        let failed = error(ctx, conn, caller, failed).await;
        let (problem, retry_after) = failed.into_parts_for(&ctx.request_id);
        Self::Failed {
            problem,
            retry_after,
        }
    }
    fn failure<U>(&self) -> Option<(Option<U>, Option<Problem>, Option<u64>)> {
        match self {
            Self::Failed {
                problem,
                retry_after,
            } => Some((None, Some(problem.clone()), *retry_after)),
            Self::Ready(_) => None,
        }
    }
}

struct ProviderRead {
    setting: &'static str,
    id: Id,
    prepared: Part<providers::PreparedProjection>,
}

pub(super) struct PreparedDefaults {
    node: Id,
    registry: Part<Vec<orch_api_contract::settings::SettingDef>>,
    settings: Part<PreparedSettings>,
    secrets: Option<Part<PreparedSecrets>>,
    providers: Vec<ProviderRead>,
    topic: Option<crate::routes::environments::reading::TopicSummarySources>,
    readmes: BTreeSet<Id>,
}

fn denied() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the key does not allow this requested read",
    )
}
fn merge(into: &mut NameSources, other: NameSources) {
    into.users.extend(other.users);
    into.groups.extend(other.groups);
    into.nodes.extend(other.nodes);
    into.containers.extend(other.containers);
}
fn empty_provider_names(plan: &mut OperationPlan) -> ApiResult<()> {
    let roles = providers::readable_name_roles();
    if let Some(role) = roles.groups {
        bind(
            plan,
            role,
            KeyTargetKind::Identity(KeyIdentityKind::Group),
            &[],
        )?;
    }
    for (kind, role) in roles.nodes {
        bind(plan, role, KeyTargetKind::Object(kind), &[])?;
    }
    Ok(())
}
pub(super) fn empty(plan: &mut OperationPlan) -> ApiResult<()> {
    bind(plan, "opening_registry", SYSTEM, &[])?;
    bind(plan, "opening_providers", PROVIDER, &[])?;
    empty_provider_names(plan)?;
    bind(plan, "opening_readme_documents", DOCUMENT, &[])?;
    bind(plan, "opening_node_environments", ENVIRONMENT, &[])
}

async fn retain_provider(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    plan: &OperationPlan,
    id: Id,
) -> ApiResult<(KeyResolvedTarget, providers::PreparedProjection)> {
    let target = crate::key_operations::targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id,
        },
    )
    .await?;
    if !permits(plan, principal.caller(), "opening_providers", &target)? {
        return Err(denied());
    }
    let prepared = providers::prepare_readable_projection(ctx, conn, principal, id).await?;
    // Full Group payloads remain mandatory under the existing Provider contract.
    // A missing source declines only this optional Provider part, never another read.
    let sources = prepared.name_sources();
    let groups = group_targets(conn, &sources.groups.into_iter().collect::<Vec<_>>()).await?;
    let group_role = providers::readable_name_roles()
        .groups
        .ok_or_else(|| ApiError::internal("the Provider Group source is not declared"))?;
    for group in groups {
        if !permits(plan, principal.caller(), group_role, &group)? {
            return Err(denied());
        }
    }
    Ok((target, prepared))
}

async fn node_sources(
    conn: &mut Conn,
    principal: &Principal,
    plan: &mut OperationPlan,
    node: Id,
    kind: NodeKind,
    names: &mut NameSources,
) -> ApiResult<(
    Option<crate::routes::environments::reading::TopicSummarySources>,
    BTreeSet<Id>,
)> {
    let mut retained_docs = BTreeSet::new();
    let mut document_targets = Vec::new();
    let rows = orch_db::queries::documents::readmes_of(conn, &[node]).await?;
    let mut eligible = Vec::new();
    for row in rows {
        let target = crate::key_operations::targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Document,
                id: row.id,
            },
        )
        .await?;
        if permits(
            plan,
            principal.caller(),
            "opening_readme_documents",
            &target,
        )? {
            eligible.push((row, target));
        }
    }
    if !eligible.is_empty() {
        let ids: Vec<_> = eligible.iter().map(|(row, _)| row.id).collect();
        let listed: HashMap<_, _> =
            orch_authz::documents::listed(conn, principal, &ids, Policy::default())
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .map(|listed| (listed.document.id, listed))
                .collect();
        for (row, target) in eligible {
            let Some(listed) = listed
                .get(&row.id)
                .filter(|listed| listed.holds(Permission::DocView))
            else {
                continue;
            };
            for source in crate::routes::documents::build::ProjectedDocumentRef::candidates(
                conn, &row, listed,
            )
            .await?
            {
                if let crate::routes::documents::build::WrittenLabelSource::Node(id) = source {
                    names.nodes.insert(id);
                }
            }
            retained_docs.insert(row.id);
            document_targets.push(target);
        }
    }
    bind(
        plan,
        "opening_readme_documents",
        DOCUMENT,
        &document_targets,
    )?;
    let topic = if kind == NodeKind::Topic {
        let mut sources =
            crate::routes::environments::reading::topic_summary_sources(conn, principal, &[node])
                .await?;
        let ids: BTreeSet<_> = sources.bindings.iter().map(|(_, id)| *id).collect();
        let mut permitted = BTreeSet::new();
        let mut targets = Vec::new();
        for id in ids {
            let target = crate::key_operations::targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?;
            if permits(
                plan,
                principal.caller(),
                "opening_node_environments",
                &target,
            )? {
                permitted.insert(id);
                targets.push(target);
            }
        }
        sources.bindings.retain(|(_, id)| permitted.contains(id));
        // The configured ID remains the node's stored setting; only live refs are filtered.
        bind(plan, "opening_node_environments", ENVIRONMENT, &targets)?;
        Some(sources)
    } else {
        bind(plan, "opening_node_environments", ENVIRONMENT, &[])?;
        None
    };
    Ok((topic, retained_docs))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the existing read checkpoint, actual root and requested native parts share one explicit source plan"
)]
pub(super) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    plan: &mut OperationPlan,
    node: Id,
    kind: NodeKind,
    query: &NodeChatOpeningGetQuery,
    names: &mut NameSources,
) -> ApiResult<PreparedDefaults> {
    let registry_target = crate::key_operations::targets::system();
    let (registry, registry_targets) = if permits(
        plan,
        principal.caller(),
        "opening_registry",
        &registry_target,
    )? {
        match crate::routes::settings::registry_definitions() {
            Ok(registry) => (Part::Ready(registry), vec![registry_target]),
            Err(failed) => (
                Part::failed(ctx, conn, principal.caller(), failed).await,
                Vec::new(),
            ),
        }
    } else {
        (
            Part::failed(ctx, conn, principal.caller(), denied()).await,
            Vec::new(),
        )
    };
    bind(plan, "opening_registry", SYSTEM, &registry_targets)?;
    let settings = match readers::prepare_settings(conn, principal, node).await {
        Ok(read) => {
            names.nodes.extend(read.source_nodes());
            Part::Ready(read)
        }
        Err(failed) => Part::failed(ctx, conn, principal.caller(), failed).await,
    };
    let secrets = if query.secret_names {
        Some(
            match readers::prepare_secrets(conn, principal, node).await {
                Ok(read) => {
                    names.nodes.extend(read.source_nodes());
                    Part::Ready(read)
                }
                Err(failed) => Part::failed(ctx, conn, principal.caller(), failed).await,
            },
        )
    } else {
        None
    };
    let mut wanted = Vec::new();
    if let Part::Ready(read) = &settings {
        if query.default_provider_name
            && let Some(id) = read.selected_id(keys::AGENT_DEFAULT_PROVIDER_ID, None)
        {
            wanted.push((keys::AGENT_DEFAULT_PROVIDER_ID, id));
        }
        if query.embeddings_provider_name
            && kind == NodeKind::Org
            && let Some(id) =
                read.selected_id(keys::SEARCH_SEMANTIC_EMBEDDINGS, Some("provider_id"))
        {
            wanted.push((keys::SEARCH_SEMANTIC_EMBEDDINGS, id));
        }
    }
    let mut providers = Vec::<ProviderRead>::new();
    let mut provider_targets = Vec::new();
    let mut provider_names = NameSources::default();
    for (setting, id) in wanted {
        let prepared = if let Some(existing) = providers.iter().find(|read| read.id == id) {
            existing.prepared.clone()
        } else {
            match retain_provider(ctx, conn, principal, plan, id).await {
                Ok((target, read)) => {
                    provider_targets.push(target);
                    merge(&mut provider_names, read.name_sources());
                    Part::Ready(read)
                }
                Err(failed) => Part::failed(ctx, conn, principal.caller(), failed).await,
            }
        };
        providers.push(ProviderRead {
            setting,
            id,
            prepared,
        });
    }
    bind(plan, "opening_providers", PROVIDER, &provider_targets)?;
    let bounds = projection_names(
        conn,
        principal.caller(),
        plan,
        provider_names,
        providers::readable_name_roles(),
    )
    .await?;
    for provider in &mut providers {
        if let Part::Ready(read) = &mut provider.prepared {
            read.bound_names(bounds.clone());
        }
    }
    let (topic, readmes) = node_sources(conn, principal, plan, node, kind, names).await?;
    Ok(PreparedDefaults {
        node,
        registry,
        settings,
        secrets,
        providers,
        topic,
        readmes,
    })
}

async fn rendered<T>(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    result: ApiResult<T>,
) -> (Option<T>, Option<Problem>, Option<u64>) {
    match result {
        Ok(value) => (Some(value), None, None),
        Err(failed) => {
            let failed = error(ctx, conn, caller, failed).await;
            let (problem, retry) = failed.into_parts_for(&ctx.request_id);
            (None, Some(problem), retry)
        }
    }
}

pub(super) async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedDefaults,
    names: &NameBounds,
) -> ApiResult<NodeDefaultsOpening> {
    let node = crate::routes::nodes::view::one_with_read_sources(
        conn,
        principal,
        prepared.node,
        principal.now().get(),
        Policy::default(),
        &names.nodes,
        prepared.topic.as_ref(),
        &prepared.readmes,
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    let (registry, registry_problem, registry_retry_after) = match &prepared.registry {
        Part::Ready(registry) => (Some(registry.clone()), None, None),
        failed => failed
            .failure()
            .ok_or_else(|| ApiError::internal("the registry part has no outcome"))?,
    };
    let (settings, settings_problem, settings_retry_after) = match &prepared.settings {
        Part::Ready(read) => {
            let result = readers::show_settings(ctx, conn, principal, read, names).await;
            rendered(ctx, conn, principal.caller(), result).await
        }
        failed => failed
            .failure()
            .ok_or_else(|| ApiError::internal("the settings part has no outcome"))?,
    };
    let (secrets, secrets_problem, secrets_retry_after) = match &prepared.secrets {
        Some(Part::Ready(read)) => {
            let result = readers::show_secrets(conn, principal, read, names).await;
            rendered(ctx, conn, principal.caller(), result).await
        }
        Some(failed) => failed
            .failure()
            .ok_or_else(|| ApiError::internal("the secrets part has no outcome"))?,
        None => (None, None, None),
    };
    let mut providers = Vec::<NodeDefaultsProvider>::new();
    // A failed settings reader supplies no configured provider selection, as in
    // the native aggregate. Preparation retained IDs, never an unconditional read.
    for read in prepared.providers.iter().filter(|_| settings.is_some()) {
        let (provider, problem, retry_after) =
            if let Some(existing) = providers.iter().find(|existing| existing.id == read.id) {
                (
                    existing.provider.clone(),
                    existing.problem.clone(),
                    existing.retry_after,
                )
            } else {
                match &read.prepared {
                    Part::Ready(provider) => {
                        let result =
                            providers::show_prepared(ctx, conn, principal, provider.clone()).await;
                        rendered(ctx, conn, principal.caller(), result).await
                    }
                    failed => failed
                        .failure()
                        .ok_or_else(|| ApiError::internal("the Provider part has no outcome"))?,
                }
            };
        providers.push(NodeDefaultsProvider {
            setting: read.setting.to_owned(),
            id: read.id,
            provider,
            problem,
            retry_after,
        });
    }
    Ok(NodeDefaultsOpening {
        node,
        registry,
        registry_problem,
        registry_retry_after,
        settings,
        settings_problem,
        settings_retry_after,
        secrets,
        secrets_problem,
        secrets_retry_after,
        providers,
    })
}
