//! Native stored setup selection and declaration-bound observations.

use super::*;
use crate::key_operations::native;
use crate::registry::OperationPlan;
use crate::routes::documents::build::{ProjectedDocumentRef, WrittenLabelSource};
use crate::routes::nodes::read_sources;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::account_sources::{self as sources, Source};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::notifications::OutsideWayState;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_authz::Caller;
use orch_db::queries::hierarchy;
use orch_db::queries::resources::routes as resources;
use orch_types::{Json, Time};
use std::collections::{BTreeSet, HashSet};

pub(super) struct Setup;
pub(super) struct Selection {
    reply: FirstSteps,
    authority: Option<orch_authz::nodes::Effective>,
    document: Option<orch_authz::documents::Listed>,
}

/// Keep the selected native identity beside its scope governor. A System governor has
/// no UUID, while these native organization/provider/thread/document rows always do.
#[derive(Clone)]
struct NativeTarget {
    id: Id,
    family: KeyTargetKind,
    target: KeyResolvedTarget,
}

struct Scope<'a> {
    ctx: &'a Ctx,
    caller: &'a Caller,
    plan: Option<OperationPlan>,
    overview: Option<(
        &'a Principal,
        &'a crate::routes::nodes::overview::Reading<'a>,
    )>,
    initial: Option<orch_api_contract::nodes::Org>,
    outside: Option<orch_api_contract::notifications::OutsideStatus>,
    facts: Vec<native::Facts<Source>>,
}

impl Scope<'_> {
    async fn targets(
        &self,
        conn: &mut Conn,
        source: Source,
        kind: ObjectKind,
        ids: &[Id],
    ) -> ApiResult<Vec<NativeTarget>> {
        let targets =
            read_sources::Sources::from_objects(ids.iter().map(|id| ObjectRef { kind, id: *id }))
                .targets(conn)
                .await?;
        let mut allowed = Vec::new();
        for id in ids {
            let Some(target) = targets.get(&ObjectRef { kind, id: *id }) else {
                continue;
            };
            let family = KeyTargetKind::Object(kind);
            let permitted = match self.overview {
                Some((_, reading)) => reading.source_target(family, target)?,
                None => {
                    !native::modern(self.caller)
                        || native::permits_usage(
                            self.plan.as_ref().ok_or_else(|| {
                                ApiError::internal("the setup declaration is unavailable")
                            })?,
                            &sources::FIRST_STEPS,
                            self.caller,
                            &source,
                            FacetUsage::ResponseContent,
                            family,
                            target,
                        )?
                }
            };
            if permitted {
                allowed.push(NativeTarget {
                    id: *id,
                    family,
                    target: target.clone(),
                });
            }
        }
        Ok(allowed)
    }

    fn set(&mut self, source: Source, targets: Vec<NativeTarget>) {
        self.facts.retain(|fact| fact.source != source);
        self.facts.push(native::Facts::collection(
            source,
            crate::routes::nodes::projection::groups(
                targets
                    .into_iter()
                    .map(|native| (native.family, native.target)),
            ),
        ));
    }

    async fn principal(&self, conn: &mut Conn) -> ApiResult<Principal> {
        if let Some((principal, _)) = self.overview {
            return Ok(principal.clone());
        }
        let known = native::bind_known(self.ctx, &sources::FIRST_STEPS, &self.facts)?;
        native::preparation_principal(self.ctx, conn, self.caller, &known).await
    }

    async fn org(&mut self, conn: &mut Conn, id: Id) -> ApiResult<orch_api_contract::nodes::Org> {
        let targets = self
            .targets(conn, Source::FirstOrg, ObjectKind::Org, &[id])
            .await?;
        if targets.is_empty() {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the selected organization has not been admitted",
            ));
        }
        self.set(Source::FirstOrg, targets);
        let principal = self.principal(conn).await?;
        if let Some((_, reading)) = self.overview {
            if self.initial.as_ref().is_some_and(|org| org.base.id == id) {
                return self.initial.take().ok_or_else(|| {
                    ApiError::internal("the selected setup organization disappeared")
                });
            }
            return match reading.visible(conn, &[id]).await?.pop() {
                Some(Node::Org(org)) => Ok(org),
                _ => Err(ApiError::not_found()),
            };
        }
        readable_org(self.ctx, conn, &principal, id).await
    }

    fn no_org(&mut self) {
        // The actual org selector proved there is no component to observe.
        for binding in sources::FIRST_STEPS
            .bindings
            .iter()
            .filter(|binding| binding.source != Source::CurrentAccount)
        {
            self.set(binding.source, Vec::new());
        }
    }
}

impl native::Selected for Setup {
    type Source = Source;
    type Query = FirstStepsGetQuery;
    type Value = Selection;
    type Reply = FirstSteps;
    const DECLARATION: &'static Declaration<Source> = &sources::FIRST_STEPS;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a FirstStepsGetQuery,
    ) -> BoxFuture<'a, ApiResult<(Selection, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let root = crate::routes::auth::account_read::account(ctx, conn, caller).await?;
            let plan = native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&root))?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let mut scope = Scope {
                ctx,
                caller,
                plan: Some(plan),
                overview: None,
                initial: None,
                outside: None,
                facts: vec![root],
            };
            let reply = observe(ctx, conn, &principal, query.clone(), &mut scope).await?;
            let observed_principal = scope.principal(conn).await?;
            let authority = if let Some(org) = &reply.org {
                Some(
                    orch_authz::nodes::effective(
                        conn,
                        &observed_principal,
                        org.id,
                        Policy::default(),
                    )
                    .await
                    .map_err(ApiError::internal)?,
                )
            } else {
                None
            };
            let document = if let Some(document) = &reply.start_here {
                orch_authz::documents::listed(
                    conn,
                    &observed_principal,
                    &[document.id],
                    Policy::default(),
                )
                .await
                .map_err(ApiError::internal)?
                .pop()
            } else {
                None
            };
            Ok((
                Selection {
                    reply,
                    authority,
                    document,
                },
                scope.facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the setup observation changed actor"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<FirstSteps>> {
        Box::pin(async move { Ok(value.reply.clone()) })
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a FirstStepsGetQuery,
        old: &'a Selection,
        fresh: &'a Selection,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let org = match (&old.authority, &fresh.authority) {
                (Some(old), Some(fresh)) => {
                    crate::routes::profiles::node_authority_preserved(old, fresh)
                }
                (None, _) => true,
                _ => false,
            };
            let doc = match (&old.document, &fresh.document) {
                (Some(old), Some(fresh)) => {
                    old.document.id == fresh.document.id
                        && old.document.node == fresh.document.node
                        && fresh.holds(Permission::DocView)
                        && old.admin_place == fresh.admin_place
                        && old.admin_access == fresh.admin_access
                }
                (None, _) => true,
                _ => false,
            };
            Ok(org
                && doc
                && old.reply.org == fresh.reply.org
                && old.reply.work_mode == fresh.reply.work_mode
                && old.reply.steps == fresh.reply.steps
                && old.reply.invitation_roles == fresh.reply.invitation_roles)
        })
    }
}

async fn start_document(
    ctx: &Ctx,
    conn: &mut Conn,
    scope: &mut Scope<'_>,
    org: Id,
    root: Id,
) -> ApiResult<Option<orch_api_contract::documents::DocumentRef>> {
    let principal = scope.principal(conn).await?;
    let Some(entry) = start_here_row(conn, &principal, org, root).await? else {
        scope.set(Source::FirstDocument, Vec::new());
        scope.set(Source::FirstDocumentNames, Vec::new());
        return Ok(None);
    };
    let targets = scope
        .targets(
            conn,
            Source::FirstDocument,
            ObjectKind::Document,
            &[entry.0.id],
        )
        .await?;
    let readable = !targets.is_empty();
    scope.set(Source::FirstDocument, targets);
    if !readable {
        scope.set(Source::FirstDocumentNames, Vec::new());
        return Ok(None);
    }
    let candidates = ProjectedDocumentRef::candidates(conn, &entry.0, &entry.1).await?;
    let ids: Vec<_> = candidates
        .iter()
        .filter_map(|source| {
            if let WrittenLabelSource::Node(id) = source {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    let graph = orch_authz::engine::Graph::load(conn, &ids)
        .await
        .map_err(ApiError::internal)?;
    let mut targets = Vec::new();
    let mut allowed = Vec::new();
    for id in ids {
        if let Some(target) = orch_engine::operation_scope::targets::from_node_identity(&graph, id)
        {
            let permitted = match scope.overview {
                Some((_, reading)) => reading.source_target(target.object.kind, &target)?,
                None => {
                    !native::modern(scope.caller)
                        || native::permits_usage(
                            scope.plan.as_ref().ok_or_else(|| {
                                ApiError::internal("the setup declaration is unavailable")
                            })?,
                            &sources::FIRST_STEPS,
                            scope.caller,
                            &Source::FirstDocumentNames,
                            FacetUsage::ResponseContent,
                            target.object.kind,
                            &target,
                        )?
                }
            };
            if permitted {
                targets.push(NativeTarget {
                    id,
                    family: target.object.kind,
                    target,
                });
                allowed.push(WrittenLabelSource::Node(id));
            }
        }
    }
    scope.set(Source::FirstDocumentNames, targets.clone());
    let principal = scope.principal(conn).await?;
    let result = if native::modern(scope.caller) {
        let (mut documents, disclosed) = ProjectedDocumentRef::capture_many_allowed(
            conn,
            &principal,
            &[entry],
            ctx.now,
            Policy::default(),
            &allowed,
        )
        .await?;
        let named: BTreeSet<_> = disclosed
            .into_iter()
            .filter_map(|source| {
                if let WrittenLabelSource::Node(id) = source {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();
        scope.set(
            Source::FirstDocumentNames,
            targets
                .into_iter()
                .filter(|native| named.contains(&native.id))
                .collect(),
        );
        documents.pop()
    } else {
        crate::routes::documents::build::refs(
            conn,
            &principal,
            &[entry],
            ctx.now,
            Policy::default(),
        )
        .await?
        .pop()
    };
    Ok(result)
}

async fn observe(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: FirstStepsGetQuery,
    scope: &mut Scope<'_>,
) -> ApiResult<FirstSteps> {
    let saved = state(conn, principal.user()).await?;
    let choice: FirstStepsChoice = saved
        .get("help")
        .and_then(|help| help.get("first_steps"))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let personal = identity::user_by_id(conn, principal.user())
        .await?
        .and_then(|user| user.personal_org_id);
    let last = saved
        .get("sidebar")
        .and_then(|sidebar| sidebar.get("last_org_id"))
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok());
    let wanted = query.org_id.or(choice.org_id).or(last);
    let mut org = None;
    if let Some(id) = wanted {
        match scope.org(conn, id).await {
            Ok(found) => org = Some(found),
            Err(error)
                if query.org_id.is_none()
                    && matches!(
                        error.code(),
                        orch_api_contract::common::ErrorCode::AuthzDenied
                            | orch_api_contract::common::ErrorCode::NotFound
                            | orch_api_contract::common::ErrorCode::OrgSuspended
                    ) => {}
            Err(error) => return Err(error),
        }
    }
    if org.is_none() && query.org_id.is_none() {
        let roots = hierarchy::root_orgs_of(conn, principal.user()).await?;
        let targets = scope
            .targets(conn, Source::FirstOrg, ObjectKind::Org, &roots)
            .await?;
        let scoped_roots: Vec<_> = targets.iter().map(|native| native.id).collect();
        scope.set(Source::FirstOrg, targets);
        let roots_principal = scope.principal(conn).await?;
        let mut teams: Vec<_> = view::visible(
            conn,
            &roots_principal,
            &scoped_roots,
            ctx.now,
            Policy::default(),
        )
        .await?
        .into_iter()
        .filter_map(|node| match node {
            Node::Org(org) if org.org_kind == orch_api_contract::nodes::OrgType::Team => Some(org),
            _ => None,
        })
        .collect();
        if teams.len() == 1 {
            org = teams.pop();
        } else if let Some(id) = personal {
            match scope.org(conn, id).await {
                Ok(found) => org = Some(found),
                Err(error)
                    if native::modern(scope.caller)
                        && matches!(
                            error.code(),
                            orch_api_contract::common::ErrorCode::AuthzDenied
                                | orch_api_contract::common::ErrorCode::NotFound
                                | orch_api_contract::common::ErrorCode::OrgSuspended
                        ) => {}
                Err(error) => return Err(error),
            }
        }
    }
    let Some(org) = org else {
        scope.no_org();
        return Ok(FirstSteps {
            org: None,
            work_mode: None,
            steps: Vec::new(),
            invitation_roles: Vec::new(),
            start_here: None,
            as_of: Time::new(ctx.now),
        });
    };
    let id = org.base.id;
    let targets = scope
        .targets(conn, Source::FirstOrg, ObjectKind::Org, &[id])
        .await?;
    scope.set(Source::FirstOrg, targets);
    let org_principal = scope.principal(conn).await?;
    let principal = &org_principal;
    let org_ref = NodeRef {
        thread_display: None,
        id,
        kind: NodeKind::Org,
        name: Some(org.base.name.clone()),
        redacted: false,
    };
    let mode = if choice.org_id == Some(id) {
        choice.work_mode
    } else {
        None
    };
    let permits = |permission: &str| {
        org.base
            .access
            .permissions
            .iter()
            .any(|held| held == permission)
    };
    let mut provider_ids = rows::providers(conn, id).await?;
    let default = orch_settings::get::<Id>(
        conn,
        orch_settings::Place::Node(id),
        orch_settings_contract::registry::keys::AGENT_DEFAULT_PROVIDER_ID,
    )
    .await
    .map_err(ApiError::internal)?;
    if let Some(default) = default
        && !provider_ids.contains(&default)
    {
        provider_ids.push(default);
    }
    let standing =
        orch_authz::resources::effective_among(conn, principal, &provider_ids, Policy::default())
            .await
            .map_err(ApiError::internal)?;
    let visible: HashSet<Id> = standing
        .iter()
        .filter(|row| {
            row.held
                .iter()
                .any(|held| held.permission == Permission::ProviderView)
        })
        .map(|row| row.object.id)
        .collect();
    let usable: HashSet<Id> = standing
        .iter()
        .filter(|row| {
            row.held
                .iter()
                .any(|held| held.permission == Permission::ProviderUse)
        })
        .map(|row| row.object.id)
        .collect();
    provider_ids.retain(|candidate| {
        visible.contains(candidate) && (Some(*candidate) != default || usable.contains(candidate))
    });
    let scoped_providers = scope
        .targets(
            conn,
            Source::FirstProviders,
            ObjectKind::Provider,
            &provider_ids,
        )
        .await?;
    let provider_unknown = scoped_providers.len() != provider_ids.len();
    let provider_ids: Vec<_> = scoped_providers.iter().map(|native| native.id).collect();
    scope.set(Source::FirstProviders, scoped_providers);
    let providers = resources::providers(conn, &provider_ids).await?;
    let secrets = resources::provider_secret_names(conn, &provider_ids).await?;
    let models = resources::models_of(conn, &provider_ids).await?;
    let configured = providers.iter().find_map(|provider| {
        let yields = provider
            .facts
            .pointer("/credential/yields")
            .and_then(Json::as_str);
        let fact = if secrets
            .iter()
            .any(|(owner, name)| *owner == provider.id && name == "api_key")
            || yields == Some("key")
        {
            FirstStepsFact::KeySaved
        } else if matches!(yields, Some("login" | "token")) {
            FirstStepsFact::SignedIn
        } else if provider.upstreams.as_array().is_some_and(|upstreams| {
            upstreams.iter().any(|endpoint| {
                endpoint.pointer("/options/auth").and_then(Json::as_str) == Some("none")
            })
        }) {
            FirstStepsFact::EndpointConfigured
        } else {
            return None;
        };
        Some((provider, fact))
    });
    let with_models = providers.iter().find(|provider| {
        usable.contains(&provider.id)
            && models
                .iter()
                .any(|model| model.provider_id == provider.id && model.enabled)
    });
    let candidates = rows::answers(conn, id).await?;
    let candidate_ids: Vec<_> = candidates.iter().map(|(thread, _)| *thread).collect();
    let scoped_answers = scope
        .targets(
            conn,
            Source::FirstAnswer,
            ObjectKind::Thread,
            &candidate_ids,
        )
        .await?;
    let answer_unknown = scoped_answers.len() != candidate_ids.len();
    let candidate_ids: Vec<_> = scoped_answers.iter().map(|native| native.id).collect();
    scope.set(Source::FirstAnswer, scoped_answers.clone());
    let answer_principal = scope.principal(conn).await?;
    let answers = view::visible(
        conn,
        &answer_principal,
        &candidate_ids,
        ctx.now,
        Policy::default(),
    )
    .await?;
    let answer = answers.iter().find_map(|node| {
        if let Node::Thread(thread) = node {
            Some(thread.base.id)
        } else {
            None
        }
    });
    scope.set(
        Source::FirstAnswer,
        scoped_answers
            .into_iter()
            .filter(|native| Some(native.id) == answer)
            .collect(),
    );
    let scripted = answer.is_some_and(|answer| {
        candidates
            .iter()
            .any(|(id, scripted)| *id == answer && *scripted)
    });
    let domain_ids = rows::member_domains(conn, id, principal.user()).await?;
    let scoped_domains = scope
        .targets(conn, Source::FirstMembers, ObjectKind::Org, &domain_ids)
        .await?;
    let member_unknown = scoped_domains.len() != domain_ids.len();
    let domain_ids: Vec<_> = scoped_domains.iter().map(|native| native.id).collect();
    scope.set(Source::FirstMembers, scoped_domains.clone());
    let members_principal = scope.principal(conn).await?;
    let joined_domains = view::visible(
        conn,
        &members_principal,
        &domain_ids,
        ctx.now,
        Policy::default(),
    )
    .await?;
    let joined = !joined_domains.is_empty();
    let domain_ids: HashSet<_> = joined_domains
        .iter()
        .map(crate::routes::nodes::node_id)
        .collect();
    scope.set(
        Source::FirstMembers,
        scoped_domains
            .into_iter()
            .filter(|native| domain_ids.contains(&native.id))
            .collect(),
    );
    let waiting = permits("members.manage") && rows::invitation_waiting(conn, id, ctx.now).await?;
    let mut steps = vec![
        step(
            FirstStepsId::Team,
            match mode {
                None => FirstStepsFact::ChooseOrg,
                Some(FirstStepsMode::Solo) => FirstStepsFact::SoloChosen,
                Some(FirstStepsMode::Team) => FirstStepsFact::TeamChosen,
            },
            Some(org_ref.clone()),
            None,
            false,
        ),
        step(
            FirstStepsId::Provider,
            configured.map_or(
                if provider_unknown {
                    FirstStepsFact::Unknown
                } else {
                    FirstStepsFact::NotDone
                },
                |(_, fact)| fact,
            ),
            None,
            action(
                configured.map_or_else(
                    || format!("/resources/providers?org_id={id}"),
                    |(provider, _)| format!("/resources/providers/{}", provider.id),
                ),
                configured.is_some() || permits("settings.manage"),
            ),
            false,
        ),
        step(
            FirstStepsId::Models,
            if with_models.is_some() {
                FirstStepsFact::ModelsEnabled
            } else if provider_unknown {
                FirstStepsFact::Unknown
            } else {
                FirstStepsFact::NotDone
            },
            None,
            action(
                with_models.map_or_else(
                    || format!("/resources/providers?org_id={id}"),
                    |provider| format!("/resources/providers/{}?tab=models", provider.id),
                ),
                with_models.is_some() || permits("settings.manage"),
            ),
            false,
        ),
        step(
            FirstStepsId::Work,
            if answer.is_some() {
                FirstStepsFact::AnsweredOnce
            } else if answer_unknown {
                FirstStepsFact::Unknown
            } else {
                FirstStepsFact::NotDone
            },
            None,
            action(
                answer.map_or_else(
                    || format!("/org/{}", org.address),
                    |thread| format!("/thread/{thread}"),
                ),
                answer.is_some() || permits("org.interact"),
            ),
            scripted,
        ),
    ];
    if answer.is_some() {
        let outside = if scope
            .overview
            .is_none_or(|(_, reading)| reading.delivery_allowed())
        {
            Some(
                crate::routes::notifications::inbox::outside_status(
                    conn,
                    principal,
                    Some(id),
                    ctx.now,
                )
                .await?,
            )
        } else {
            None
        };
        steps.push(step(
            FirstStepsId::Away,
            match &outside {
                None => FirstStepsFact::Unknown,
                Some(status)
                    if status
                        .ways
                        .iter()
                        .any(|way| way.state == OutsideWayState::Ready) =>
                {
                    FirstStepsFact::OutwardEnabled
                }
                Some(_) => FirstStepsFact::NotDone,
            },
            None,
            action("/settings/notifications".into(), true),
            false,
        ));
        scope.outside = outside;
    }
    if mode != Some(FirstStepsMode::Solo) {
        steps.push(step(
            FirstStepsId::Teammate,
            if joined {
                FirstStepsFact::MemberJoined
            } else if waiting {
                FirstStepsFact::InvitationWaiting
            } else if member_unknown || !permits("members.manage") {
                FirstStepsFact::Unknown
            } else {
                FirstStepsFact::NotDone
            },
            Some(org_ref.clone()),
            action(
                format!("/org/{}/settings/members", org.address),
                permits("members.manage"),
            ),
            false,
        ));
    }
    let invitation_roles = role_summaries(conn, principal, id).await?;
    let start_here = start_document(ctx, conn, scope, id, org.base.org_id).await?;
    Ok(FirstSteps {
        org: Some(org_ref),
        work_mode: mode,
        steps,
        invitation_roles,
        start_here,
        as_of: Time::new(ctx.now),
    })
}

/// The same provider/member/answer/document cohort, using the already admitted
/// aggregate principal and explicit native source bounds.
pub(super) async fn for_overview<'a>(
    ctx: &'a Ctx,
    conn: &mut Conn,
    principal: &'a Principal,
    query: FirstStepsGetQuery,
    initial: Option<orch_api_contract::nodes::Org>,
    reading: &'a crate::routes::nodes::overview::Reading<'a>,
) -> ApiResult<ObservedFirstSteps> {
    if initial
        .as_ref()
        .is_some_and(|org| query.org_id.is_some_and(|id| id != org.base.id))
    {
        return Err(ApiError::internal(
            "the setup observation changed its organization",
        ));
    }
    let mut scope = Scope {
        ctx,
        caller: principal.caller(),
        plan: None,
        overview: Some((principal, reading)),
        initial,
        outside: None,
        facts: Vec::new(),
    };
    let facts = observe(ctx, conn, principal, query, &mut scope).await?;
    Ok(ObservedFirstSteps {
        facts,
        outside: scope.outside,
    })
}
