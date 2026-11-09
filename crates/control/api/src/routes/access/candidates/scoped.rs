//! Actual holder candidates and their native address associations on the shared read adapter.

use super::*;
use crate::key_operations::{native, targets};
use crate::registry::OperationPlan;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use std::collections::{BTreeMap, BTreeSet};

const GROUP: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Group);
const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);
const PEOPLE_WINDOW: i64 = 30; // The native holder search's existing account window.
const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::OrgNames),
    (ObjectKind::Project, Source::ProjectNames),
    (ObjectKind::Topic, Source::TopicNames),
    (ObjectKind::Thread, Source::ThreadNames),
];
type Key = (KeyTargetKind, Id);

/// Associations come from native place/group/membership rows before candidate formatting.
#[derive(Clone, PartialEq, Eq)]
struct Association {
    paths: Vec<Vec<Id>>,
    named: BTreeSet<Id>,
}

pub(crate) struct Selected {
    pub(super) reply: HolderSearchReply,
    pub(super) witness: Witness,
}
pub(super) struct Witness(BTreeMap<Key, Association>);

pub(crate) struct Holders;

impl native::Selected for Holders {
    type Source = Source;
    type Query = HolderSearchQuery;
    type Value = Selected;
    type Reply = HolderSearchReply;
    const DECLARATION: &'static Declaration<Source> = &sources::HOLDER_SEARCH;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(super::select(ctx, conn, caller, query))
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the holder search changed actor"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<HolderSearchReply>> {
        Box::pin(async move { Ok(value.reply.clone()) })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a HolderSearchQuery,
        opened: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.witness.0.iter().all(|(key, old)| {
                fresh.witness.0.get(key).is_some_and(|current| {
                    current.paths == old.paths && old.named.is_subset(&current.named)
                })
            }))
        })
    }
}

pub(super) struct Frame {
    limited: bool,
    targets: BTreeMap<Key, KeyResolvedTarget>,
    nodes: HashMap<Id, KeyTargetKind>,
    graph: orch_authz::engine::Graph,
    names: BTreeSet<Id>,
    pub(super) groups: Vec<access::GroupFull>,
    pub(super) people: Vec<identity::UserSearchRow>,
    used: BTreeMap<Key, Association>,
}

fn grouped(
    targets: impl IntoIterator<Item = (Key, KeyResolvedTarget)>,
) -> Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)> {
    let mut groups = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
    for ((kind, _), target) in targets {
        groups.entry(kind).or_default().push(target);
    }
    groups.into_iter().collect()
}

impl Frame {
    pub(super) async fn load(
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
        query: &HolderSearchQuery,
        words: &Words,
        places: &[access::HolderPlaceRow],
    ) -> ApiResult<Self> {
        let limited = native::modern(caller);
        let plan = OperationPlan::new(ctx.route, sources::HOLDER_SEARCH.operation)?;
        let ids: Vec<_> = places.iter().map(|place| place.id).collect();
        let graph = orch_authz::engine::Graph::load(conn, &ids)
            .await
            .map_err(ApiError::internal)?;
        let mut targets = BTreeMap::new();
        let mut nodes = HashMap::new();
        if limited {
            for id in &ids {
                if let Some(target) =
                    orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                {
                    let family = target.object.kind;
                    if native::permits(
                        &plan,
                        &sources::HOLDER_SEARCH,
                        caller,
                        &Source::Holders,
                        family,
                        &target,
                    )? {
                        nodes.insert(*id, family);
                        targets.insert((family, *id), target);
                    }
                }
            }
        }
        let orgs = places
            .iter()
            .filter(|place| place.kind == "org")
            .map(|place| place.id)
            .collect::<Vec<_>>();
        let mut groups = access::groups_in_orgs(conn, &orgs).await?;
        if limited {
            let identities = super::super::operations::group_targets(
                conn,
                &groups.iter().map(|group| group.id).collect::<Vec<_>>(),
            )
            .await?;
            let mut permitted = HashSet::new();
            for target in identities {
                let id = target.object.id.ok_or_else(ApiError::not_found)?;
                if native::permits(
                    &plan,
                    &sources::HOLDER_SEARCH,
                    caller,
                    &Source::Holders,
                    GROUP,
                    &target,
                )? {
                    permitted.insert(id);
                    targets.insert((GROUP, id), target);
                }
            }
            groups.retain(|group| permitted.contains(&group.id));
        }
        let mut people = Vec::new();
        let (path, tail) = words.parts();
        if query.list_places != Some(true)
            && !words.members
            && !words.every
            && tail.is_empty()
            && !path.is_empty()
            && role_word(path).is_none()
        {
            let is_mail = path.contains('@') && !path.starts_with('@');
            let username = if is_mail {
                path
            } else {
                path.rsplit('.')
                    .next()
                    .unwrap_or(path)
                    .trim_start_matches('@')
            };
            let mut after: Option<(String, Id)> = None;
            loop {
                let rows = identity::search_user_rows(
                    conn,
                    caller.user(),
                    username,
                    None,
                    PEOPLE_WINDOW,
                    after.as_ref().map(|(name, id)| (name.as_str(), *id)),
                )
                .await?;
                let exhausted =
                    rows.len() < usize::try_from(PEOPLE_WINDOW).map_err(ApiError::internal)?;
                let next = rows
                    .last()
                    .map(|row| (row.user.username.clone(), row.user.id));
                let exact =
                    path.starts_with('@') && rows.iter().any(|row| row.user.username == username);
                let rows = rows
                    .into_iter()
                    .filter(|row| !exact || row.user.username == username)
                    .collect::<Vec<_>>();
                if !limited {
                    people = rows;
                    break;
                }
                let identities = targets::account_identities(
                    conn,
                    &rows
                        .iter()
                        .map(|row| (row.user.id, row.user.kind.as_str(), row.service_org_id))
                        .collect::<Vec<_>>(),
                )
                .await?;
                for row in rows {
                    let target = identities
                        .get(&row.user.id)
                        .ok_or_else(ApiError::not_found)?;
                    if native::permits(
                        &plan,
                        &sources::HOLDER_SEARCH,
                        caller,
                        &Source::Holders,
                        ACCOUNT,
                        target,
                    )? {
                        targets.insert((ACCOUNT, row.user.id), target.clone());
                        people.push(row);
                        if people.len()
                            == usize::try_from(PEOPLE_WINDOW).map_err(ApiError::internal)?
                        {
                            break;
                        }
                    }
                }
                if people.len() == usize::try_from(PEOPLE_WINDOW).map_err(ApiError::internal)?
                    || exhausted
                    || exact
                {
                    break;
                }
                after = next;
            }
        }
        let names = if limited {
            native::node_name_facts(
                &plan,
                &sources::HOLDER_SEARCH,
                caller,
                &graph,
                &ids.into_iter().collect(),
                NAMES,
            )?
            .1
        } else {
            BTreeSet::new()
        };
        Ok(Self {
            limited,
            targets,
            nodes,
            graph,
            names,
            groups,
            people,
            used: BTreeMap::new(),
        })
    }

    pub(super) async fn principal(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
    ) -> ApiResult<Principal> {
        if !self.limited {
            let plan = OperationPlan::new(ctx.route, sources::HOLDER_SEARCH.operation)?;
            return native::preparation_principal(ctx, conn, caller, &plan).await;
        }
        let facts = native::Facts::collection(
            Source::Holders,
            grouped(
                self.targets
                    .iter()
                    .map(|(key, target)| (*key, target.clone())),
            ),
        );
        let known = native::bind_known(ctx, &sources::HOLDER_SEARCH, &[facts])?;
        native::preparation_principal(ctx, conn, caller, &known).await
    }

    pub(super) fn allows_name(&self, id: Id) -> bool {
        !self.limited || self.names.contains(&id)
    }
    pub(super) fn allows_node(&self, id: Id) -> bool {
        !self.limited || self.nodes.contains_key(&id)
    }

    fn record(&mut self, key: Key, paths: Vec<Vec<Id>>, known: &HashSet<Id>) {
        if !self.limited {
            return;
        }
        let named = paths
            .iter()
            .flatten()
            .filter(|id| known.contains(id))
            .copied()
            .collect();
        // Native input deduplication keeps the first locator for each holder identity.
        self.used.entry(key).or_insert(Association { paths, named });
    }

    pub(super) fn record_node(&mut self, place: &access::HolderPlaceRow, known: &HashSet<Id>) {
        if let Some(kind) = self.nodes.get(&place.id).copied() {
            self.record((kind, place.id), vec![place.path.clone()], known);
        }
    }

    pub(super) fn record_group(
        &mut self,
        id: Id,
        org: Id,
        context: &[Id],
        places: &HashMap<Id, &access::HolderPlaceRow>,
        known: &HashSet<Id>,
    ) {
        let keeper = places
            .get(&org)
            .map_or_else(Vec::new, |place| place.path.clone());
        self.record((GROUP, id), vec![context.to_vec(), keeper], known);
    }

    pub(super) fn record_account(
        &mut self,
        id: Id,
        domain: Option<Id>,
        places: &HashMap<Id, &access::HolderPlaceRow>,
        known: &HashSet<Id>,
    ) {
        let path = domain
            .and_then(|id| places.get(&id))
            .map_or_else(Vec::new, |place| place.path.clone());
        self.record((ACCOUNT, id), vec![path], known);
    }

    pub(super) fn finish(
        self,
        ctx: &Ctx,
        caller: &Caller,
    ) -> ApiResult<(Witness, Vec<native::Facts<Source>>)> {
        if !self.limited {
            return Ok((Witness(BTreeMap::new()), Vec::new()));
        }
        let selected = self
            .used
            .keys()
            .map(|key| {
                self.targets
                    .get(key)
                    .cloned()
                    .map(|target| (*key, target))
                    .ok_or_else(|| ApiError::internal("a native holder source is missing"))
            })
            .collect::<ApiResult<Vec<_>>>()?;
        let mut facts = vec![native::Facts::collection(
            Source::Holders,
            grouped(selected),
        )];
        let names = self
            .used
            .values()
            .flat_map(|value| value.named.iter().copied())
            .collect();
        let plan = OperationPlan::new(ctx.route, sources::HOLDER_SEARCH.operation)?;
        facts.extend(
            native::node_name_facts(
                &plan,
                &sources::HOLDER_SEARCH,
                caller,
                &self.graph,
                &names,
                NAMES,
            )?
            .0,
        );
        Ok((Witness(self.used), facts))
    }
}
