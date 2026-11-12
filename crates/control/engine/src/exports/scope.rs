//! Native export candidates bounded by the consuming operation before aggregation.
use super::*;
use crate::operation_scope::{
    Prepared,
    sources::{self, Facts},
    targets,
};
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    export_sources::Source, operation_sources::Declaration, operations::FacetUsage,
};
use orch_authz::engine::Graph;

type Subject = (KeyTargetKind, Id);
#[derive(Debug, Clone, PartialEq, Eq)]
struct Association {
    node: Id,
    exportable: bool,
}

/// Selected native inputs and their factual associations, with no retained admission.
#[derive(Debug, Clone)]
pub struct ScopedExportPlan {
    /// Native operands, restricted before counts, blockers, layout and bytes.
    pub plan: PreparedExportPlan,
    subjects: BTreeMap<Subject, Association>,
    names: BTreeSet<Id>,
}
impl ScopedExportPlan {
    /// A refreshed preparation retains exactly the opened subjects and their native placement.
    pub fn same_sources(&self, fresh: &Self) -> bool {
        self.plan.selection.place.id == fresh.plan.selection.place.id
            && self.subjects == fresh.subjects
            && self.names.is_subset(&fresh.names)
    }
}
const NAMES: [(ObjectKind, Source); 4] = [
    (ObjectKind::Org, Source::Orgs),
    (ObjectKind::Project, Source::Projects),
    (ObjectKind::Topic, Source::Topics),
    (ObjectKind::Thread, Source::Threads),
];
fn bound(error: impl std::fmt::Display) -> ExportError {
    ExportError::Internal(EngineError::unreadable(error))
}
pub(super) fn permits(
    plan: &Prepared,
    row: &Declaration<Source>,
    caller: &Caller,
    source: Source,
    usage: FacetUsage,
    target: &KeyResolvedTarget,
) -> Result<bool, ExportError> {
    let binding = row
        .bindings
        .iter()
        .find(|binding| binding.source == source && binding.usage == usage)
        .ok_or_else(|| bound("the export source role is not declared"))?;
    for index in sources::indices(plan, binding).map_err(bound)? {
        if !plan
            .permits_binding(
                caller,
                index,
                target.object.kind,
                std::slice::from_ref(target),
            )
            .map_err(bound)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}
struct Selection<'a> {
    declaration: &'static Declaration<Source>,
    plan: &'a Prepared,
    caller: &'a Caller,
    opened: Option<&'a ScopedExportPlan>,
    subjects: BTreeMap<Subject, Association>,
    groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
}
impl Selection<'_> {
    fn keep(
        &mut self,
        target: KeyResolvedTarget,
        node: Id,
        exportable: bool,
    ) -> Result<bool, ExportError> {
        let id = target.object.id.ok_or(ExportError::NotFound)?;
        let subject = (target.object.kind, id);
        if self
            .opened
            .is_some_and(|opened| !opened.subjects.contains_key(&subject))
            || !permits(
                self.plan,
                self.declaration,
                self.caller,
                Source::Contents,
                FacetUsage::Required,
                &target,
            )?
        {
            return Ok(false);
        }
        self.subjects
            .insert(subject, Association { node, exportable });
        self.groups
            .entry(target.object.kind)
            .or_default()
            .push(target);
        Ok(true)
    }
}
fn name_facts(
    plan: &Prepared,
    row: &Declaration<Source>,
    caller: &Caller,
    graph: &Graph,
    ids: &BTreeSet<Id>,
) -> Result<(Vec<Facts<Source>>, BTreeSet<Id>), ExportError> {
    let mut facts = Vec::new();
    let mut names = BTreeSet::new();
    for (kind, source) in NAMES {
        let family = KeyTargetKind::Object(kind);
        let mut selected = Vec::new();
        for id in ids {
            let Some(target) = targets::from_node_identity(graph, *id) else {
                continue;
            };
            if target.object.kind == family
                && permits(
                    plan,
                    row,
                    caller,
                    source,
                    FacetUsage::ResponseContent,
                    &target,
                )?
            {
                names.insert(*id);
                selected.push(target);
            }
        }
        facts.push(Facts::collection(
            source,
            if selected.is_empty() {
                Vec::new()
            } else {
                vec![(family, selected)]
            },
        ));
    }
    Ok((facts, names))
}
impl Engine {
    /// The same native source selector for preview, creation and deferred archive writing.
    pub async fn bound_export_plan(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        mut prepared: PreparedExportPlan,
        plan: &Prepared,
        declaration: &'static Declaration<Source>,
        opened: Option<&ScopedExportPlan>,
    ) -> Result<(ScopedExportPlan, Vec<Facts<Source>>), ExportError> {
        let scoped = caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some());
        let opened = opened.filter(|_| scoped);
        let raw = &prepared.selection;
        let mut graph = Graph::load(
            conn,
            &raw.scope.iter().map(|node| node.id).collect::<Vec<_>>(),
        )
        .await?;
        let mut selected = Selection {
            declaration,
            plan,
            caller,
            opened,
            subjects: BTreeMap::new(),
            groups: BTreeMap::new(),
        };
        let mut bounds = ExportBounds::default();
        for id in raw
            .threads
            .iter()
            .filter(|id| raw.access.reads_conversations.contains(id))
        {
            let target = targets::from_node_identity(&graph, *id).ok_or(ExportError::NotFound)?;
            if selected.keep(target, *id, raw.access.exports_conversations.contains(id))? {
                bounds.threads.insert(*id);
            }
        }
        let ids = raw
            .chats
            .iter()
            .filter(|chat| raw.access.reads_chats.contains(&chat.node_id))
            .map(|chat| chat.chat_id)
            .collect::<Vec<_>>();
        let chats = orch_db::queries::chats::chats(conn, &ids).await?;
        for chat in chats {
            let node = chat.node_id.ok_or(ExportError::NotFound)?;
            if !raw
                .chats
                .iter()
                .any(|row| row.chat_id == chat.id && row.node_id == node)
            {
                return Err(ExportError::NotFound);
            }
            let target = targets::from_chat_graph(&graph, &chat).map_err(bound)?;
            if selected.keep(target, node, raw.access.exports_chats.contains(&node))? {
                bounds.chats.insert(chat.id);
            }
        }
        let ids = raw
            .documents
            .iter()
            .filter(|document| raw.access.reads_documents.contains(&document.id))
            .map(|document| document.id)
            .collect::<Vec<_>>();
        for document in orch_authz::documents::read_into(conn, &mut graph, &ids).await? {
            if document.kind != ObjectKind::Document
                || document.deleted
                || !raw
                    .documents
                    .iter()
                    .any(|row| row.id == document.id && row.node_id == document.node)
            {
                continue;
            }
            let site =
                orch_authz::documents::site_of(&graph, &document).ok_or(ExportError::NotFound)?;
            let target = targets::from_site(&graph, &site).ok_or(ExportError::NotFound)?;
            if selected.keep(
                target,
                document.node,
                raw.access.exports_documents.contains(&document.id),
            )? {
                bounds.documents.insert(document.id);
            }
        }
        if prepared.wanted.package {
            for node in raw.scope.iter().filter(|node| {
                node.kind != "thread"
                    && raw.included.contains(&node.id)
                    && raw.viewable.contains_key(&node.id)
            }) {
                let target =
                    targets::from_node_identity(&graph, node.id).ok_or(ExportError::NotFound)?;
                if selected.keep(target, node.id, true)? {
                    bounds.nodes.insert(node.id);
                }
            }
        }
        let native_names = raw
            .viewable
            .keys()
            .copied()
            .filter(|id| opened.is_none_or(|opened| opened.names.contains(id)))
            .collect::<BTreeSet<_>>();
        let (_, names) = name_facts(plan, declaration, caller, &graph, &native_names)?;
        bounds.nodes.extend(names.iter().copied());
        if scoped {
            prepared
                .selection
                .restrict(&bounds, prepared.wanted.package);
        }
        let needed = prepared
            .selection
            .viewable
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        let (name_facts, names) = name_facts(plan, declaration, caller, &graph, &needed)?;
        let root = targets::node_identity(conn, prepared.selection.place.id)
            .await
            .map_err(bound)?;
        let mut facts = vec![
            Facts::one(Source::Place, root.object.kind, root),
            Facts::collection(Source::Contents, selected.groups.into_iter().collect()),
        ];
        facts.extend(name_facts);
        Ok((
            ScopedExportPlan {
                plan: prepared,
                subjects: if scoped {
                    selected.subjects
                } else {
                    BTreeMap::new()
                },
                names: if scoped { names } else { BTreeSet::new() },
            },
            facts,
        ))
    }
}
