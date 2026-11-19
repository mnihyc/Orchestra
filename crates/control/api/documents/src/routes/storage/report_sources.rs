//! Native storage source collection before formatting, aggregation and paging.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::storage_sources as sources;

use orch_api_contract::storage_sources::Source;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use std::collections::BTreeMap;

use std::collections::HashSet;

use std::sync::Arc;
pub(super) struct Report;

pub(super) struct OperatorSnapshot {
    pub report: Option<api::OperatorStorageInventory>,
    pub reason: Option<&'static str>,
    pub(super) origin: CapturedOperation,
}

pub(super) struct Ready {
    records: PreparedRecords,
    query: api::StorageQuery,
    name: String,
    names: HashSet<ObjectRef>,
    accounts: HashSet<Id>,
    witness: BTreeMap<String, Json>,
    origin: Arc<OperatorSnapshot>,
}

pub(super) struct Book {
    declaration: &'static Declaration<Source>,
    plan_names: bool,
    needed_names: HashSet<KeyObjectRef>,
    physical: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
    associations: BTreeMap<String, orch_engine::jobs::cleanup::sources::Reference>,
    machines: HashMap<Id, KeyResolvedTarget>,
    pub(super) plan: crate::registry::OperationPlan,
    caller: Caller,
    root: native::Facts<Source>,
    groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
    named: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
    targets: HashMap<ObjectRef, KeyResolvedTarget>,
    pub names: HashSet<ObjectRef>,
    pub accounts: HashSet<Id>,
    account_targets: HashMap<Id, KeyResolvedTarget>,
    pub witness: BTreeMap<String, Json>,
    original: Option<HashSet<String>>,
}

pub(super) async fn namespace(
    conn: &mut Conn,
    caller: &Caller,
    scope: Scope,
) -> ApiResult<KeyResolvedTarget> {
    match scope {
        Scope::Me => targets::account(conn, caller.user()).await,
        Scope::System => Ok(targets::system()),
        Scope::Node(id) => targets::node_identity(conn, id).await,
        Scope::Host(id) => {
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id,
                },
            )
            .await
        }
        Scope::Resource(object) if object.kind.node().is_some() => {
            targets::node_identity(conn, object.id).await
        }
        Scope::Resource(object) => targets::resource_identity(conn, object).await,
    }
}

pub(super) async fn require_namespace(
    conn: &mut Conn,
    principal: &Principal,
    scope: Scope,
) -> ApiResult<()> {
    match scope {
        Scope::System if !principal.is_system_admin() => return Err(ApiError::not_found()),
        Scope::Node(id) => {
            if orch_authz::nodes::holding(
                conn,
                principal,
                &[id],
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Default::default(),
            )
            .await
            .map_err(ApiError::internal)?
            .is_empty()
            {
                return Err(ApiError::not_found());
            }
        }
        Scope::Host(id)
        | Scope::Resource(ObjectRef {
            kind: ObjectKind::Host,
            id,
        }) => {
            if !resource_facts::authorities(conn, principal, &[id])
                .await
                .map_err(ApiError::internal)?
                .iter()
                .any(|row| {
                    row.resource.object()
                        == (ObjectRef {
                            kind: ObjectKind::Host,
                            id,
                        })
                        && row.manageable
                })
            {
                return Err(ApiError::not_found());
            }
        }
        Scope::Resource(object)
            if !resource_facts::authorities(conn, principal, &[object.id])
                .await
                .map_err(ApiError::internal)?
                .iter()
                .any(|row| row.resource.object() == object && row.manageable) =>
        {
            return Err(ApiError::not_found());
        }
        _ => {}
    }
    Ok(())
}

impl Book {
    async fn new(
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
        scope: Scope,
        original: Option<HashSet<String>>,
    ) -> ApiResult<Self> {
        Self::for_operation(ctx, conn, caller, scope, original, &sources::REPORT, false).await
    }

    pub(super) async fn for_operation(
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
        scope: Scope,
        original: Option<HashSet<String>>,
        declaration: &'static Declaration<Source>,
        plan_names: bool,
    ) -> ApiResult<Self> {
        let target = namespace(conn, caller, scope).await?;
        let named_target = target.clone();
        let root = native::Facts::one(Source::Namespace, target.object.kind, target);
        let plan = native::bind_known(ctx, declaration, std::slice::from_ref(&root))?;
        let mut book = Self {
            declaration,
            plan_names,
            needed_names: HashSet::from([named_target.object]),
            physical: BTreeMap::new(),
            associations: BTreeMap::new(),
            machines: HashMap::new(),
            plan,
            caller: caller.clone(),
            root,
            groups: BTreeMap::new(),
            named: BTreeMap::new(),
            targets: HashMap::new(),
            names: HashSet::new(),
            accounts: HashSet::new(),
            account_targets: HashMap::new(),
            witness: BTreeMap::new(),
            original,
        };
        if plan_names
            || !native::modern(caller)
            || native::permits_usage(
                &book.plan,
                declaration,
                caller,
                &Source::Names,
                FacetUsage::ResponseContent,
                named_target.object.kind,
                &named_target,
            )?
        {
            if let (KeyTargetKind::Object(kind), Some(id)) =
                (named_target.object.kind, named_target.object.id)
            {
                book.names.insert(ObjectRef { kind, id });
            }
            book.named
                .entry(named_target.object.kind)
                .or_default()
                .push(named_target);
        }
        Ok(book)
    }

    pub async fn prime(
        &mut self,
        conn: &mut Conn,
        resources: &[&resource_facts::Authority],
        nodes: &[Id],
        accounts: &[Id],
    ) -> ApiResult<()> {
        let places = resources
            .iter()
            .filter_map(|row| row.resource.home)
            .chain(nodes.iter().copied())
            .collect::<Vec<_>>();
        let graph = orch_authz::engine::Graph::load(conn, &places)
            .await
            .map_err(ApiError::internal)?;
        for row in resources {
            if let Some(target) =
                orch_authz::key_targets::from_resource_identity(&graph, &row.resource)
            {
                self.targets.insert(row.resource.object(), target);
            }
        }
        for id in nodes {
            if let Some(target) = orch_authz::key_targets::from_node_identity(&graph, *id)
                && let KeyTargetKind::Object(kind) = target.object.kind
            {
                self.targets.insert(ObjectRef { kind, id: *id }, target);
            }
        }
        let rows: Vec<(Id, String, Option<Id>)> =
            sqlx::query_as("SELECT id,kind,service_org_id FROM users WHERE id=ANY($1)")
                .bind(accounts)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
        let identities = rows
            .iter()
            .map(|(id, kind, org)| (*id, kind.as_str(), *org))
            .collect::<Vec<_>>();
        self.account_targets
            .extend(targets::account_identities(conn, &identities).await?);
        Ok(())
    }

    pub async fn prime_references(
        &mut self,
        conn: &mut Conn,
        objects: &[ObjectRef],
    ) -> ApiResult<()> {
        let ids = objects
            .iter()
            .filter(|object| object.kind.resource().is_some() && !self.targets.contains_key(object))
            .map(|object| object.id)
            .collect::<Vec<_>>();
        let mut graph = orch_authz::engine::Graph::default();
        let rows = orch_authz::resources::read_into(conn, &mut graph, &ids)
            .await
            .map_err(ApiError::internal)?;
        for row in rows {
            if let Some(target) = orch_authz::key_targets::from_resource_identity(&graph, &row) {
                self.targets.insert(row.object(), target);
            }
        }
        let nodes = objects
            .iter()
            .filter(|object| object.kind.node().is_some())
            .map(|object| object.id)
            .collect::<Vec<_>>();
        let graph = orch_authz::engine::Graph::load(conn, &nodes)
            .await
            .map_err(ApiError::internal)?;
        for id in nodes {
            if let Some(target) = orch_authz::key_targets::from_node_identity(&graph, id)
                && let KeyTargetKind::Object(kind) = target.object.kind
            {
                self.targets.insert(ObjectRef { kind, id }, target);
            }
        }
        Ok(())
    }

    async fn object(&mut self, conn: &mut Conn, object: ObjectRef) -> ApiResult<KeyResolvedTarget> {
        if let Some(target) = self.targets.get(&object) {
            return Ok(target.clone());
        }
        let target = if object.kind.node().is_some() {
            targets::node_identity(conn, object.id).await?
        } else if object.kind.is_document() {
            targets::page_object(conn, object).await?
        } else {
            targets::resource_identity(conn, object).await?
        };
        self.targets.insert(object, target.clone());
        Ok(target)
    }

    pub async fn keep_object(
        &mut self,
        conn: &mut Conn,
        key: String,
        object: ObjectRef,
        witness: Json,
    ) -> ApiResult<bool> {
        if self
            .original
            .as_ref()
            .is_some_and(|ids| !ids.contains(&key))
        {
            return Ok(false);
        }
        let target = self.object(conn, object).await?;
        self.keep(key, target, witness)
    }

    pub fn keep(
        &mut self,
        key: String,
        target: KeyResolvedTarget,
        witness: Json,
    ) -> ApiResult<bool> {
        if self
            .original
            .as_ref()
            .is_some_and(|ids| !ids.contains(&key))
        {
            return Ok(false);
        }
        if !self.plan_names
            && native::modern(&self.caller)
            && !native::permits(
                &self.plan,
                self.declaration,
                &self.caller,
                &Source::Records,
                target.object.kind,
                &target,
            )?
        {
            return Ok(false);
        }
        self.associations.insert(
            key.clone(),
            orch_engine::jobs::cleanup::sources::Reference {
                family: target.object.kind,
                object: target.object,
            },
        );
        self.groups
            .entry(target.object.kind)
            .or_default()
            .push(target);
        self.witness.insert(key, witness);
        Ok(true)
    }

    pub async fn name(&mut self, conn: &mut Conn, object: ObjectRef) -> ApiResult<bool> {
        let target = self.object(conn, object).await?;
        let allowed = self.plan_names
            || !native::modern(&self.caller)
            || native::permits_usage(
                &self.plan,
                self.declaration,
                &self.caller,
                &Source::Names,
                FacetUsage::ResponseContent,
                target.object.kind,
                &target,
            )?;
        if allowed {
            self.names.insert(object);
            self.named
                .entry(target.object.kind)
                .or_default()
                .push(target);
        }
        Ok(allowed)
    }

    pub async fn account_name(&mut self, conn: &mut Conn, id: Id) -> ApiResult<bool> {
        let target = match self.account_targets.get(&id).cloned() {
            Some(target) => target,
            None => {
                let target = targets::account(conn, id).await?;
                self.account_targets.insert(id, target.clone());
                target
            }
        };
        let allowed = self.plan_names
            || !native::modern(&self.caller)
            || native::permits_usage(
                &self.plan,
                self.declaration,
                &self.caller,
                &Source::Names,
                FacetUsage::ResponseContent,
                target.object.kind,
                &target,
            )?;
        if allowed {
            self.accounts.insert(id);
            self.named
                .entry(target.object.kind)
                .or_default()
                .push(target);
        }
        Ok(allowed)
    }

    pub fn system_name(&mut self) -> ApiResult<bool> {
        let target = targets::system();
        let allowed = self.plan_names
            || !native::modern(&self.caller)
            || native::permits_usage(
                &self.plan,
                self.declaration,
                &self.caller,
                &Source::Names,
                FacetUsage::ResponseContent,
                target.object.kind,
                &target,
            )?;
        if allowed {
            self.named
                .entry(target.object.kind)
                .or_default()
                .push(target);
        }
        Ok(allowed)
    }

    pub(super) async fn row_names(
        &mut self,
        conn: &mut Conn,
        host: Option<Id>,
        home: Option<Id>,
        owner: Option<Id>,
    ) -> ApiResult<()> {
        if !self.plan_names {
            return Ok(());
        }
        if let Some(host) = host {
            self.require_object_name(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: host,
                },
            )
            .await?;
        }
        if let Some(home) = home
            && let Some(target) = self
                .targets
                .iter()
                .find(|(object, _)| object.id == home && object.kind.node().is_some())
                .map(|(_, target)| target.clone())
        {
            self.require_name(target.object);
        }
        if let Some(owner) = owner
            && let Some(target) = self.account_targets.get(&owner)
        {
            self.require_name(target.object);
        }
        Ok(())
    }

    pub(super) fn plan_names(&self) -> bool {
        self.plan_names
    }
    pub(super) fn require_name(&mut self, target: KeyObjectRef) {
        self.needed_names.insert(target);
    }
    pub(super) async fn require_object_name(
        &mut self,
        conn: &mut Conn,
        object: ObjectRef,
    ) -> ApiResult<()> {
        let target = self.object(conn, object).await?;
        self.require_name(target.object);
        Ok(())
    }
    pub(super) async fn prepare_machines(
        &mut self,
        conn: &mut Conn,
        machines: &[Id],
    ) -> ApiResult<()> {
        if !self.plan_names {
            return Ok(());
        }
        self.machines = targets::machine_identities(conn, machines).await?;
        Ok(())
    }
    pub(super) async fn destination(
        &mut self,
        conn: &mut Conn,
        host: Option<Id>,
        machine: Id,
    ) -> ApiResult<()> {
        if !self.plan_names {
            return Ok(());
        }
        if let Some(host) = host {
            let target = self
                .object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: host,
                    },
                )
                .await?;
            self.physical
                .entry(target.object.kind)
                .or_default()
                .push(target);
        }
        let target = self
            .machines
            .get(&machine)
            .ok_or_else(ApiError::not_found)?
            .clone();
        self.physical
            .entry(KeyTargetKind::Object(ObjectKind::Machine))
            .or_default()
            .push(target);
        Ok(())
    }

    pub(super) fn snapshot(&self) -> ApiResult<orch_engine::jobs::cleanup::sources::Sources> {
        use orch_engine::jobs::cleanup::sources::{Reference, Sources};
        fn refs(groups: &BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>) -> Vec<Reference> {
            let mut seen = HashSet::new();
            groups
                .iter()
                .flat_map(|(family, targets)| {
                    targets.iter().map(move |target| Reference {
                        family: *family,
                        object: target.object,
                    })
                })
                .filter(|reference| seen.insert(reference.clone()))
                .collect()
        }
        let namespace = self
            .root
            .groups
            .iter()
            .flat_map(|(family, targets)| {
                targets.iter().map(move |target| Reference {
                    family: *family,
                    object: target.object,
                })
            })
            .next()
            .ok_or_else(|| ApiError::internal("the native storage namespace is missing"))?;
        let names = refs(&self.named)
            .into_iter()
            .filter(|reference| self.needed_names.contains(&reference.object))
            .collect();
        let physical = refs(&self.physical);
        let machines = self
            .machines
            .iter()
            .filter(|(_, target)| {
                physical.iter().any(|reference| {
                    reference.family == KeyTargetKind::Object(ObjectKind::Machine)
                        && reference.object == target.object
                })
            })
            .map(|(id, target)| (*id, target.object))
            .collect();
        Ok(Sources {
            namespace,
            records: refs(&self.groups),
            physical,
            names,
            machines,
            associations: self.associations.clone(),
        })
    }

    pub(super) fn facts(self) -> Vec<native::Facts<Source>> {
        fn distinct(
            groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
        ) -> Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)> {
            groups
                .into_iter()
                .map(|(kind, mut targets)| {
                    let mut seen = HashSet::new();
                    targets.retain(|target| seen.insert(target.clone()));
                    (kind, targets)
                })
                .collect()
        }
        let mut content = self.groups.clone();
        for (kind, rows) in &self.physical {
            content
                .entry(*kind)
                .or_default()
                .extend(rows.iter().cloned());
        }
        let named = if self.plan_names {
            self.named
                .into_iter()
                .filter_map(|(kind, rows)| {
                    let rows = rows
                        .into_iter()
                        .filter(|target| self.needed_names.contains(&target.object))
                        .collect::<Vec<_>>();
                    (!rows.is_empty()).then_some((kind, rows))
                })
                .collect()
        } else {
            self.named
        };
        let mut facts = vec![
            self.root,
            native::Facts::collection(Source::Records, distinct(self.groups)),
            native::Facts::collection(Source::Names, distinct(named)),
        ];
        if self.plan_names {
            facts.push(native::Facts::collection(
                Source::Physical,
                distinct(self.physical),
            ));
            facts.push(native::Facts::collection(
                Source::Content,
                distinct(content),
            ));
        }
        facts
    }
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &api::StorageQuery,
    original: Option<HashSet<String>>,
) -> ApiResult<(Ready, Vec<native::Facts<Source>>)> {
    let scope = Scope::parse(query)?;
    let mut book = Book::new(ctx, conn, caller, scope, original).await?;
    let principal = native::preparation_principal(ctx, conn, caller, &book.plan).await?;
    require_namespace(conn, &principal, scope).await?;
    let origin = native::effects::initial_value::<Report>(ctx)?;
    let records = prepare_records(
        ctx,
        conn,
        &principal,
        query,
        Some(origin.as_ref()),
        Some(&mut book),
    )
    .await?;
    let name = match scope {
        Scope::Host(id)
        | Scope::Resource(ObjectRef {
            kind: ObjectKind::Host,
            id,
        }) if book.names.contains(&ObjectRef {
            kind: ObjectKind::Host,
            id,
        }) =>
        {
            sqlx::query_scalar("SELECT name FROM hosts WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?
                .unwrap_or_default()
        }
        Scope::Node(id)
            if book
                .names
                .iter()
                .any(|object| object.id == id && object.kind.node().is_some()) =>
        {
            sqlx::query_scalar("SELECT name FROM nodes WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?
                .unwrap_or_default()
        }
        Scope::Resource(object) if book.names.contains(&object) => {
            resource_facts::authorities(conn, &principal, &[object.id])
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .find(|row| row.resource.object() == object && row.readable)
                .map(|row| row.resource.name)
                .unwrap_or_default()
        }
        _ => String::new(),
    };
    let value = Ready {
        records,
        query: query.clone(),
        name,
        names: book.names.clone(),
        accounts: book.accounts.clone(),
        witness: book.witness.clone(),
        origin,
    };
    Ok((value, book.facts()))
}

impl native::Selected for Report {
    type Source = Source;
    type Query = api::StorageQuery;
    type Value = Ready;
    type Reply = api::StorageReport;
    const MOVES: native::Moves = native::Moves::Repeat;
    const DECLARATION: &'static Declaration<Source> = &sources::REPORT;
    fn origin<'a>(_: &Self::Query, value: &'a Ready) -> Option<&'a CapturedOperation> {
        Some(&value.origin.origin)
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, query, None))
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
        opened: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(
            ctx,
            conn,
            caller,
            query,
            Some(opened.witness.keys().cloned().collect()),
        ))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<api::StorageReport>> {
        Box::pin(async move {
            render_report(
                value.records.render()?,
                &value.query,
                Time::new(ctx.now),
                value.name.clone(),
            )
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        old: &'a Ready,
        now: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old
                .witness
                .iter()
                .all(|(key, value)| now.witness.get(key) == Some(value))
                && old.names.is_subset(&now.names)
                && old.accounts.is_subset(&now.accounts)
                && old.name == now.name)
        })
    }
}

pub(super) async fn fetch_operator(
    ctx: &Ctx,
    caller: &Caller,
    scope: Scope,
) -> ApiResult<OperatorSnapshot> {
    let origin = CapturedOperation::capture(caller).map_err(ApiError::internal)?;
    let (report, reason) = if matches!(scope, Scope::System) {
        operator_inventory(ctx).await?
    } else {
        (None, None)
    };
    Ok(OperatorSnapshot {
        report,
        reason,
        origin,
    })
}

// The shared initial supplier supplies only immutable native IO. All selected rows are
// subsequently produced by Selected; this type adds no writer or synthetic effect.
impl native::effects::Effect for Report {
    type Source = Source;
    type Query = api::StorageQuery;
    type Body = ();
    type Value = Ready;
    type Output = ();
    fn declaration(_: &Ready) -> &'static Declaration<Source> {
        &sources::REPORT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: Self::Query,
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { prepare(ctx, conn, caller, &query, None).await })
    }
    fn require<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
}

impl native::effects::Initial for Report {
    type Input = Scope;
    type Fetched = OperatorSnapshot;
    fn declaration(_: &Scope) -> &'static Declaration<Source> {
        &sources::REPORT
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: api::StorageQuery,
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Scope, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let scope = Scope::parse(&query)?;
            let target = namespace(conn, caller, scope).await?;
            let _ = ctx;
            Ok((
                scope,
                vec![native::Facts::one(
                    Source::Namespace,
                    target.object.kind,
                    target,
                )],
            ))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        scope: &'a Scope,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_namespace(conn, principal, *scope))
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        scope: Scope,
    ) -> BoxFuture<'a, ApiResult<OperatorSnapshot>> {
        Box::pin(fetch_operator(ctx, caller, scope))
    }
}
