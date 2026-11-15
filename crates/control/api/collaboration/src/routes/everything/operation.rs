//! One own-operation predicate over the native inventory's stored subjects.

use crate::key_operations::admission;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::registry::OptionalOperationFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyEmptyFacet;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyPlaceExclusion;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operations::TargetResolver;

use orch_authz::engine::Graph;

use orch_authz::engine::Site;

use orch_authz::everything::Inventory;

use orch_authz::everything::Projection;

use orch_authz::everything::scope::Boundary;

use orch_authz::Caller;

use orch_authz::Error;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::chats;

use orch_db::queries::everything as rows;

use orch_engine::operation_scope::targets;

use orch_types::Id;

use orch_types::Time;

use std::collections::BTreeMap;

use std::collections::HashSet;
type TargetIndex = BTreeMap<(KeyTargetKind, Id), KeyResolvedTarget>;

const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);

fn chat_target(graph: &Graph, chat: &chats::ChatRow) -> Result<KeyResolvedTarget, Error> {
    targets::from_chat_graph(graph, chat).map_err(|error| match error {
        targets::TargetError::Missing => Error::NotFound(chat.id),
        targets::TargetError::Db(error) => Error::Db(error),
        targets::TargetError::Authz(error) => error,
    })
}

pub(super) struct Predicate {
    plan: OperationPlan,
    caller: Caller,
    results: usize,
}

fn native_error(error: ApiError) -> Error {
    Error::Stored(format!(
        "the inventory operation boundary is unavailable: {}",
        error.problem.code
    ))
}

impl Predicate {
    pub(super) fn plan(&self) -> OperationPlan {
        self.plan.clone()
    }

    /// An origin label uses the same own-operation predicate as every other returned Machine.
    pub(super) async fn origin_principal(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        machine: Id,
    ) -> ApiResult<Option<Principal>> {
        let family = KeyTargetKind::Object(ObjectKind::Machine);
        let target = match targets::machine_subject(conn, machine).await {
            Ok(target) => target,
            Err(targets::TargetError::Missing) => return Ok(None),
            Err(error) => return Err(ApiError::internal(error)),
        };
        if !self
            .permits(family, &target)
            .map_err(authorization::restoration_error)?
        {
            return Ok(None);
        }
        self.preparation_principal(ctx, conn, Some((family, target)))
            .await
            .map(Some)
    }

    /// A real candidate or established factual exhaustion primes only effect-free preparation.
    /// The final selected collection is bound separately after the native SQL window is known.
    pub(super) async fn preparation_principal(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        candidate: Option<(KeyTargetKind, KeyResolvedTarget)>,
    ) -> ApiResult<orch_authz::Principal> {
        let mut plan = self.plan();
        match candidate {
            Some((kind, target)) => plan.bind_collection(self.results, &[(kind, vec![target])])?,
            None => plan.bind_empty(self.results)?,
        }
        plan.preparation_principal(ctx, conn, &self.caller).await
    }

    pub(super) fn bind_results(
        &self,
        plan: &mut OperationPlan,
        groups: &[(KeyTargetKind, Vec<KeyResolvedTarget>)],
    ) -> ApiResult<()> {
        if groups.is_empty() {
            plan.bind_empty(self.results)
        } else {
            plan.bind_collection(self.results, groups)
        }
    }

    pub(super) fn new(ctx: &Ctx, caller: &Caller, selection: &str) -> ApiResult<Self> {
        let plan = OperationPlan::new(ctx.route, selection)?;
        plan.shared()
            .compatible(caller)
            .map_err(admission::binding_error)?;
        let results = plan
            .variant()
            .facets
            .iter()
            .position(|facet| facet.target == TargetResolver::ResolvedCollection("results"))
            .ok_or_else(|| {
                ApiError::internal("the native inventory results collection is undeclared")
            })?;
        let facet = &plan.variant().facets[results];
        // This is only a necessary action/family eligibility check. It does not
        // bind emptiness, replace target matching, or grant native preparation.
        if !caller.permits_empty_operation(
            selection,
            &[KeyEmptyFacet {
                action: facet.action,
                resources: facet.resources.clone(),
            }],
        ) {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the key does not allow this inventory action",
            ));
        }
        Ok(Self {
            plan,
            caller: caller.clone(),
            results,
        })
    }

    fn declares(&self, family: KeyTargetKind) -> bool {
        self.plan.variant().facets[self.results]
            .resources
            .contains(&family)
    }

    fn family_possible(&self, family: KeyTargetKind) -> bool {
        self.declares(family)
            && self.caller.permits_empty_operation(
                &self.plan.variant().operation,
                &[KeyEmptyFacet {
                    action: self.plan.variant().facets[self.results].action,
                    resources: vec![family],
                }],
            )
    }

    fn permits(&self, family: KeyTargetKind, target: &KeyResolvedTarget) -> Result<bool, Error> {
        if !self.declares(family) {
            return Ok(false);
        }
        self.plan
            .permits_binding(
                &self.caller,
                self.results,
                family,
                std::slice::from_ref(target),
            )
            .map_err(native_error)
    }

    /// Preliminary family eligibility only: this does not bind or assert an empty result.
    pub(super) fn needs_thread_candidates(&self) -> bool {
        if !self.declares(KeyTargetKind::Object(ObjectKind::Thread)) && !self.declares(CHAT) {
            return false;
        }
        self.caller.permits_empty_operation(
            &self.plan.variant().operation,
            &[KeyEmptyFacet {
                action: KeyAction::Read,
                resources: vec![KeyTargetKind::Object(ObjectKind::Thread), CHAT],
            }],
        )
    }

    /// Anchors discover stored identities only. Every rule layer and retained place cut stays live.
    pub(super) fn anchors(&self) -> (Vec<Id>, Vec<Id>) {
        let mut nodes = Vec::new();
        let mut chats = Vec::new();
        if let Some(grant) = self.caller.grant() {
            nodes.extend(grant.subtrees());
            if let Some(scope) = grant.operation_scope() {
                for rule in scope.layers().flatten() {
                    match (rule.target.kind, rule.target.id) {
                        (KeyTargetKind::Object(kind), Some(id)) if kind.node().is_some() => {
                            nodes.push(id)
                        }
                        (CHAT, Some(id)) => chats.push(id),
                        _ => {}
                    }
                }
                for excluded in &scope.excluded_places {
                    let place = match excluded {
                        KeyPlaceExclusion::Refused { place }
                        | KeyPlaceExclusion::Lifetime { place, .. } => place,
                    };
                    nodes.extend(place.node());
                }
            }
        }
        nodes.sort_unstable();
        nodes.dedup();
        chats.sort_unstable();
        chats.dedup();
        (nodes, chats)
    }

    /// Actual native identities and canonical facade targets, without permission materialization.
    fn target_index(&self, inventory: &Inventory) -> Result<TargetIndex, Error> {
        let (graph, sites) = inventory.sites();
        let mut found = BTreeMap::new();
        for site in sites {
            let family = KeyTargetKind::Object(site.kind);
            if self.declares(family) {
                let target = targets::from_site(graph, site).ok_or(Error::NotFound(site.id))?;
                found.insert((family, site.id), target);
            }
            if let Some(chat) = inventory
                .node_chat(site.id)
                .filter(|_| site.kind.node().is_some() && self.declares(CHAT))
            {
                found.insert((CHAT, chat.id), chat_target(graph, chat)?);
            }
        }
        if self.declares(KeyTargetKind::Object(ObjectKind::Machine)) {
            for resource in inventory.resources().iter().filter(|resource| {
                resource.kind == orch_access_contract::permissions::ResourceKind::Host
            }) {
                let Some(machine) = resource.machine else {
                    continue;
                };
                let Some(site) = orch_authz::resources::site_of(graph, resource) else {
                    continue;
                };
                let target =
                    targets::from_site(graph, &site).ok_or(Error::NotFound(resource.id))?;
                found.insert(
                    (KeyTargetKind::Object(ObjectKind::Machine), machine),
                    target,
                );
            }
        }
        if self.declares(CHAT) {
            for chat in inventory.direct_chats() {
                found.insert((CHAT, chat.id), chat_target(graph, chat)?);
            }
        }
        Ok(found)
    }

    /// The full factual partition is supplied, so absence means actual scope exhaustion.
    fn first_candidate(
        &self,
        identities: &TargetIndex,
    ) -> Result<Option<(KeyTargetKind, KeyResolvedTarget)>, Error> {
        for ((family, _id), target) in identities {
            if self.permits(*family, target)? {
                return Ok(Some((*family, target.clone())));
            }
        }
        Ok(None)
    }
}

impl Boundary for Predicate {
    fn object(&self, graph: &Graph, site: &Site, business: ObjectKind) -> Result<bool, Error> {
        let family = KeyTargetKind::Object(business);
        if !self.declares(family) {
            return Ok(false);
        }
        let target = targets::from_site(graph, site).ok_or(Error::NotFound(site.id))?;
        self.permits(family, &target)
    }

    fn chat(&self, graph: &Graph, chat: &chats::ChatRow) -> Result<bool, Error> {
        if !self.declares(CHAT) {
            return Ok(false);
        }
        let target = chat_target(graph, chat)?;
        self.permits(CHAT, &target)
    }

    fn thread_profile(&self, graph: &Graph, topic: &Site, chat: bool) -> Result<bool, Error> {
        let family = if chat {
            CHAT
        } else {
            KeyTargetKind::Object(ObjectKind::Thread)
        };
        if !self.declares(family) {
            return Ok(false);
        }
        let mut hint = targets::from_site(graph, topic).ok_or(Error::NotFound(topic.id))?;
        // Unknown child identities cannot match any valid explicit Self rule. Only the
        // complete actual Topic ancestry can make this conservative candidate hint true.
        // This value never escapes the predicate or becomes an admitted result target.
        hint.object = KeyObjectRef {
            kind: family,
            id: Some(Id::nil()),
        };
        self.permits(family, &hint)
    }
}

pub(super) fn scoped(caller: &Caller) -> bool {
    caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}

struct Native {
    predicate: Predicate,
    principal: Principal,
    projection: Projection,
    identities: TargetIndex,
}

async fn native(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    selection: &str,
    parent: Option<Id>,
    system: bool,
) -> ApiResult<Native> {
    let predicate = Predicate::new(ctx, caller, selection)?;
    let raw =
        authorization::load_principal(conn, caller.clone(), Time::new(ctx.app.clock.now())).await?;
    super::system_scope(&raw, system)?;
    let (nodes, chats) = predicate.anchors();
    let inventory = orch_authz::everything::prepare_scoped(
        conn,
        &raw,
        parent,
        &nodes,
        &chats,
        predicate.needs_thread_candidates(),
    )
    .await
    .map_err(authorization::restoration_error)?;
    let identities = predicate
        .target_index(&inventory)
        .map_err(authorization::restoration_error)?;
    let candidate = predicate
        .first_candidate(&identities)
        .map_err(authorization::restoration_error)?;
    let principal = predicate
        .preparation_principal(ctx, conn, candidate)
        .await?;
    let projection = inventory
        .project_scoped(conn, &principal, &predicate)
        .await
        .map_err(authorization::restoration_error)?;
    Ok(Native {
        predicate,
        principal,
        projection,
        identities,
    })
}

/// Native factual discovery and its existing scoped ordinary projection. This carries no
/// final search-result admission; each result window binds its actual sources separately.
pub(crate) struct ScopeFrame {
    pub(crate) principal: Principal,
    pub(crate) projection: Projection,
    pub(crate) exhausted: bool,
    /// A factual interest witness, separate from a later selected result window.
    pub(crate) candidate: Option<(KeyTargetKind, KeyResolvedTarget)>,
}

/// Reuse the same actual identity partition, first candidate and scoped projection as
/// inventory readers. Exhaustion is established by that partition, never a missing hint.
pub(crate) async fn scope_frame(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    selection: &str,
    parent: Option<Id>,
) -> ApiResult<ScopeFrame> {
    let native = native(ctx, conn, caller, selection, parent, false).await?;
    let candidate = native
        .predicate
        .first_candidate(&native.identities)
        .map_err(authorization::restoration_error)?;
    Ok(ScopeFrame {
        principal: native.principal,
        projection: native.projection,
        exhausted: candidate.is_none(),
        candidate,
    })
}

struct Ready<T> {
    response: OperationPlan,
    output: T,
}

pub(super) fn summary<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let native = native(ctx, conn, caller, ctx.route.name, None, false).await?;
        let counted = rows::resource_counts_with_ids(conn, &native.projection.grants).await?;
        let mut groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
        for row in &counted {
            if usize::try_from(row.count).ok() != Some(row.ids.len()) {
                return Err(ApiError::internal(
                    "the native resource count identities are incomplete",
                ));
            }
            let kind = row
                .kind
                .parse::<orch_access_contract::permissions::ResourceKind>()
                .map_err(ApiError::internal)?;
            let family = KeyTargetKind::Object(kind.into());
            for id in &row.ids {
                let target = native
                    .identities
                    .get(&(family, *id))
                    .cloned()
                    .ok_or_else(ApiError::not_found)?;
                groups.entry(family).or_default().push(target);
            }
        }
        let mut plan = native.predicate.plan();
        native
            .predicate
            .bind_results(&mut plan, &groups.into_iter().collect::<Vec<_>>())?;
        let output = super::summary_counts(counted.into_iter().map(|row| (row.kind, row.count)));
        let response = plan.clone();
        Ok(Some(plan.with_data(Ready { response, output })))
    })
}

pub(super) async fn read<T: Clone + Send + Sync + 'static>(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<T> {
    let ready = ctx.operation_data::<Ready<T>>()?;
    let output = ready.output.clone();
    ready
        .response
        .clone()
        .read_response(ctx, conn, principal.caller(), move |_conn, _reader| {
            Box::pin(async move { Ok(output) })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the inventory response has not been admitted",
            )
        })
}

async fn final_targets(
    conn: &mut Conn,
    index: &mut TargetIndex,
    facts: &[(KeyTargetKind, Id)],
) -> ApiResult<()> {
    let mut objects = Vec::new();
    let mut machines = Vec::new();
    let mut chat_ids = Vec::new();
    for (family, id) in facts.iter().filter(|fact| !index.contains_key(*fact)) {
        match family {
            KeyTargetKind::Object(ObjectKind::Machine) => machines.push(*id),
            KeyTargetKind::Object(kind) => objects.push(ObjectRef {
                kind: *kind,
                id: *id,
            }),
            KeyTargetKind::Identity(KeyIdentityKind::Chat) => chat_ids.push(*id),
            _ => {
                return Err(ApiError::internal(
                    "the native list has an undeclared identity source",
                ));
            }
        }
    }
    let sources = crate::routes::nodes::read_sources::Sources::from_objects(objects);
    for (object, target) in sources.targets(conn).await? {
        index.insert((KeyTargetKind::Object(object.kind), object.id), target);
    }
    if !machines.is_empty() {
        for (id, target) in
            crate::key_operations::targets::machine_subjects(conn, &machines).await?
        {
            index.insert((KeyTargetKind::Object(ObjectKind::Machine), id), target);
        }
    }
    if !chat_ids.is_empty() {
        let rows = chats::chats(conn, &chat_ids).await?;
        let places: Vec<_> = rows.iter().map(|chat| chat.home()).collect();
        let graph = Graph::load(conn, &places)
            .await
            .map_err(authorization::restoration_error)?;
        for chat in rows {
            let target = chat_target(&graph, &chat).map_err(authorization::restoration_error)?;
            index.insert((CHAT, chat.id), target);
        }
    }
    if facts.iter().any(|fact| !index.contains_key(fact)) {
        return Err(ApiError::not_found());
    }
    Ok(())
}

struct ListReady {
    response: OperationPlan,
    rows: super::ListRows,
    projection: Projection,
    resource: Option<super::resources::scoped::Prepared>,
    library: Option<super::library::Prepared>,
    sharing: std::collections::HashMap<Id, super::resources::PreparedSharing>,
}

pub(super) fn list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: orch_api_contract::everything::EverythingListQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let selected = super::list_selection(&query)?;
        let selection = if selected.system {
            "EverythingList.system"
        } else {
            "EverythingList.mine"
        };
        let mut native = native(
            ctx,
            conn,
            caller,
            selection,
            selected.access_parent,
            selected.system,
        )
        .await?;
        let mut read_ctx = ctx.clone();
        read_ctx.now = native.principal.now().get();
        let mut rows = super::list_rows(
            &read_ctx,
            conn,
            &native.principal,
            &query,
            &selected,
            &mut native.projection,
        )
        .await?;
        rows.grouping = super::grouping::prepare(
            &read_ctx,
            conn,
            &native.principal,
            &rows.found,
            &rows.grants,
            Some(&native.predicate),
        )
        .await?;
        rows.facts.extend(
            rows.grouping
                .returned_origins()
                .map(|machine| (KeyTargetKind::Object(ObjectKind::Machine), machine)),
        );
        let mut resource = if rows.details {
            super::resources::scoped::prepare(
                &read_ctx,
                conn,
                &native.principal,
                super::resources::scoped::Selection {
                    kind: &rows.kinds[0],
                    rows: &rows.found,
                    grants: &rows.grants,
                    projection: &native.projection,
                },
            )
            .await?
        } else {
            None
        };
        let mut library = if rows.details {
            Some(
                super::library::prepare_page(
                    conn,
                    &native.principal,
                    &rows.found,
                    &rows.grants,
                    Time::new(read_ctx.now),
                )
                .await?,
            )
        } else {
            None
        };
        if let Some(prepared) = library.as_mut() {
            let sources = crate::routes::nodes::read_sources::Sources::from_objects(
                prepared.name_sources().filter(|object| {
                    native
                        .predicate
                        .family_possible(KeyTargetKind::Object(object.kind))
                }),
            );
            let mut allowed = HashSet::new();
            for (object, target) in sources.identity_targets(conn).await? {
                let family = KeyTargetKind::Object(object.kind);
                if native
                    .predicate
                    .permits(family, &target)
                    .map_err(authorization::restoration_error)?
                {
                    allowed.insert((object.kind, object.id));
                    rows.facts.push((family, object.id));
                    native.identities.insert((family, object.id), target);
                }
            }
            prepared.bound_sources(&allowed);
        }
        if let Some(prepared) = resource.as_ref() {
            // These are the native producer's distinct content roles, not a walk over Ref types.
            let sources = crate::routes::nodes::read_sources::Sources::from_objects(
                prepared.foreign_sources().iter().copied(),
            );
            for (object, target) in sources.identity_targets(conn).await? {
                native
                    .identities
                    .insert((KeyTargetKind::Object(object.kind), object.id), target);
            }
        }
        let mut plan = native.predicate.plan();
        let mut sharing = std::collections::HashMap::new();
        let mut sharing_names = crate::routes::access::page::NameSources::default();
        if rows.rows {
            let homes: std::collections::BTreeSet<_> = rows.grouping.homes().collect();
            let slot = plan
                .variant()
                .facets
                .iter()
                .position(|facet| {
                    matches!(facet.target,
                TargetResolver::ResolvedCollection(name) if name == "inventory_groups")
                })
                .ok_or_else(|| ApiError::internal("the inventory's Group source is absent"))?;
            for home in homes {
                let target = crate::key_operations::targets::node(conn, home).await?;
                let family = target.object.kind;
                let role = super::resources::scoped::NAMES
                    .nodes
                    .iter()
                    .find_map(|(kind, role)| {
                        (family == KeyTargetKind::Object(*kind)).then_some(*role)
                    });
                let Some(role) = role else {
                    continue;
                };
                let home_slot = plan
                    .variant()
                    .facets
                    .iter()
                    .position(|facet| {
                        matches!(facet.target,
                    TargetResolver::ResolvedCollection(name) if name == role)
                    })
                    .ok_or_else(|| {
                        ApiError::internal("the sharing home's Read source is absent")
                    })?;
                if !plan.permits_binding(
                    caller,
                    home_slot,
                    family,
                    std::slice::from_ref(&target),
                )? {
                    continue;
                }
                // Home content is an optional Read source even for a system inventory's Manage selection.
                let mut home_plan = plan.clone();
                match native
                    .predicate
                    .first_candidate(&native.identities)
                    .map_err(authorization::restoration_error)?
                {
                    Some((kind, candidate)) => native
                        .predicate
                        .bind_results(&mut home_plan, &[(kind, vec![candidate])])?,
                    None => native.predicate.bind_results(&mut home_plan, &[])?,
                }
                home_plan.bind(home_slot, family, std::slice::from_ref(&target))?;
                let reader = home_plan
                    .preparation_principal(&read_ctx, conn, caller)
                    .await?;
                let prepared =
                    match super::resources::prepare_sharing(conn, &reader, home, &rows.kinds[0])
                        .await
                    {
                        Ok(prepared) => prepared,
                        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => continue,
                        Err(error) => return Err(error),
                    };
                let names = prepared.name_sources();
                let groups = crate::routes::access::operations::group_targets(
                    conn,
                    &names.groups.iter().copied().collect::<Vec<_>>(),
                )
                .await?;
                if !groups.is_empty()
                    && !plan.permits_binding(
                        caller,
                        slot,
                        KeyTargetKind::Identity(KeyIdentityKind::Group),
                        &groups,
                    )?
                {
                    continue;
                }
                sharing_names.users.extend(names.users);
                sharing_names.groups.extend(names.groups);
                sharing_names.nodes.extend(names.nodes);
                sharing_names.containers.extend(names.containers);
                sharing_names.nodes.insert(home);
                sharing.insert(home, prepared);
            }
        }
        let mut names = resource
            .as_ref()
            .map(|prepared| prepared.name_sources().clone())
            .unwrap_or_default();
        names.users.extend(sharing_names.users);
        names.groups.extend(sharing_names.groups);
        names.nodes.extend(sharing_names.nodes);
        names.containers.extend(sharing_names.containers);
        let bounds = crate::routes::access::operations::projection_names(
            conn,
            caller,
            &mut plan,
            names,
            &super::resources::scoped::NAMES,
        )
        .await?;
        if let Some(prepared) = resource.as_mut() {
            prepared.bound_names(bounds.clone());
            rows.facts.extend(
                prepared
                    .foreign_sources()
                    .iter()
                    .map(|object| (KeyTargetKind::Object(object.kind), object.id)),
            );
        }
        for prepared in sharing.values_mut() {
            prepared.names = bounds.clone();
        }
        rows.facts.sort_unstable();
        rows.facts.dedup();
        final_targets(conn, &mut native.identities, &rows.facts).await?;
        let mut groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
        for fact in &rows.facts {
            let target = native
                .identities
                .get(fact)
                .cloned()
                .ok_or_else(ApiError::not_found)?;
            groups.entry(fact.0).or_default().push(target);
        }
        native
            .predicate
            .bind_results(&mut plan, &groups.into_iter().collect::<Vec<_>>())?;
        let response = plan.clone();
        Ok(Some(plan.with_data(ListReady {
            response,
            rows,
            projection: native.projection,
            resource,
            library,
            sharing,
        })))
    })
}

pub(super) async fn list_read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<orch_api_contract::common::Page<orch_api_contract::everything::EverythingRow>> {
    let ready = ctx.operation_data::<ListReady>()?;
    let response = ready.response.clone();
    let mut view_ctx = ctx.clone();
    view_ctx.now = principal.now().get();
    response
        .read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                super::render_rows(
                    &view_ctx,
                    conn,
                    reader,
                    ready.rows.clone(),
                    &ready.projection,
                    super::PreparedViews {
                        resources: ready.resource.as_ref(),
                        library: ready.library.as_ref(),
                        sharing: Some(&ready.sharing),
                    },
                )
                .await
            })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the inventory response has not been admitted",
            )
        })
}
