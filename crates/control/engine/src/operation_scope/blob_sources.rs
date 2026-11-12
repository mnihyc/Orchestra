//! Existing stored-byte sources, selected once under the operation's ordinary authority and key bounds.

use super::{BindingError, Prepared as OperationPlan, targets};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::Principal;
use orch_chat::following::BlobOrigin;
use orch_db::Conn;
use orch_types::{Hash, Id};

/// Source selection failures retain their origin for the ordinary transport/worker adapter.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The common catalogue/scope binder refused or found invalid metadata.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// An authoritative source identity could not be resolved.
    #[error(transparent)]
    Target(#[from] targets::TargetError),
    /// The existing ordinary source producer failed.
    #[error(transparent)]
    Source(#[from] orch_chat::Error),
    /// No allowed readable witness exists for requested stored bytes.
    #[error("the stored bytes have no allowed readable source")]
    Missing,
    /// The operation declaration omitted a required typed source role.
    #[error("{0}")]
    Declaration(&'static str),
}

/// The existing families that can supply retained bytes or copied content.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A currently readable chat.
    Chat,
    /// A currently readable thread.
    Thread,
    /// A currently readable document.
    Document,
    /// Currently readable files in an environment.
    Environment,
    /// The acting account's retained upload custody.
    RetainedUpload,
}
/// The exact declared collection families, including empty resolved collections.
pub const SOURCE_KINDS: &[SourceKind] = &[
    SourceKind::Chat,
    SourceKind::Thread,
    SourceKind::Document,
    SourceKind::Environment,
    SourceKind::RetainedUpload,
];
impl SourceKind {
    /// The canonical typed collection role reused by message/input producers.
    #[must_use]
    pub fn role(self) -> &'static str {
        match self {
            Self::Chat => "message_source_chats",
            Self::Thread => "message_source_threads",
            Self::Document => "message_source_documents",
            Self::Environment => "message_source_environments",
            Self::RetainedUpload => "message_retained_uploads",
        }
    }
    /// The business family; retained upload targets the account without inventing an upload ID.
    #[must_use]
    pub fn kind(self) -> KeyTargetKind {
        match self {
            Self::Chat => KeyTargetKind::Identity(KeyIdentityKind::Chat),
            Self::Thread => KeyTargetKind::Object(ObjectKind::Thread),
            Self::Document => KeyTargetKind::Object(ObjectKind::Document),
            Self::Environment => KeyTargetKind::Object(ObjectKind::Environment),
            Self::RetainedUpload => KeyTargetKind::Identity(KeyIdentityKind::Upload),
        }
    }
}
/// One current authoritative source, never client-supplied ancestry.
#[derive(Clone)]
pub struct Binding {
    /// The data family this source supplies.
    pub source: SourceKind,
    /// Current stored placement.
    pub target: KeyResolvedTarget,
}
/// The selected complete source witnesses for one origin at this checkpoint.
pub struct Prepared {
    /// Required copied sources and actual selected blob sources.
    pub bindings: Vec<Binding>,
    /// One complete allowed source for each requested immutable hash.
    pub selected: Vec<(Hash, BlobOrigin)>,
}
fn index(plan: &OperationPlan, source: SourceKind) -> Result<usize, Error> {
    plan.variant()
        .facets
        .iter()
        .position(|facet| {
            facet.usage == FacetUsage::Required
                && facet.action == KeyAction::Read
                && facet.target == TargetResolver::ResolvedCollection(source.role())
        })
        .ok_or_else(|| Error::Declaration("the content source collection is not declared"))
}
/// Every collection is resolved, including an explicitly empty collection. Unresolved slots are never skipped.
pub fn bind(plan: &mut OperationPlan, bindings: &[Binding]) -> Result<(), Error> {
    for source in SOURCE_KINDS {
        let mut targets = Vec::new();
        for binding in bindings.iter().filter(|binding| binding.source == *source) {
            if !targets
                .iter()
                .any(|target: &KeyResolvedTarget| target.object == binding.target.object)
            {
                targets.push(binding.target.clone());
            }
        }
        let index = index(plan, *source)?;
        if targets.is_empty() {
            plan.bind_empty(index)?;
        } else {
            plan.bind(index, source.kind(), &targets)?;
        }
    }
    Ok(())
}
async fn binding(conn: &mut Conn, user: Id, origin: BlobOrigin) -> Result<Binding, Error> {
    let (source, target) = match origin {
        BlobOrigin::RetainedByCaller => (
            SourceKind::RetainedUpload,
            targets::account(conn, user).await?,
        ),
        BlobOrigin::Chat(id) => (SourceKind::Chat, targets::chat(conn, id).await?),
        BlobOrigin::Thread(id) => (
            SourceKind::Thread,
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id,
                },
            )
            .await?,
        ),
        BlobOrigin::Document(id) => (
            SourceKind::Document,
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Document,
                    id,
                },
            )
            .await?,
        ),
        BlobOrigin::Environment(id) => (
            SourceKind::Environment,
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?,
        ),
    };
    Ok(Binding { source, target })
}
/// `base` has the operation's non-source effects/disclosure already bound. Scope-filtered candidate
/// facts prepare the same caller; the ordinary blob producer then picks one currently readable source.
/// The completed operation binds only that selected source, never the union of unrelated alternatives.
pub async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    base: &OperationPlan,
    hashes: &[Hash],
    required: &[Binding],
) -> Result<Prepared, Error> {
    prepare_in(conn, principal, base, hashes, required, None).await
}

/// Recheck only previously selected witnesses under current targets, ordinary
/// visibility and the same operation's current ceilings. No alternate source is adopted.
pub async fn prepare_selected(
    conn: &mut Conn,
    principal: &Principal,
    base: &OperationPlan,
    hashes: &[Hash],
    required: &[Binding],
    selected: &[(Hash, BlobOrigin)],
) -> Result<Prepared, Error> {
    prepare_in(conn, principal, base, hashes, required, Some(selected)).await
}

async fn prepare_in(
    conn: &mut Conn,
    principal: &Principal,
    base: &OperationPlan,
    hashes: &[Hash],
    required: &[Binding],
    selected: Option<&[(Hash, BlobOrigin)]>,
) -> Result<Prepared, Error> {
    let caller = principal.caller();
    let mut candidates = Vec::new();
    let mut proposed = required.to_vec();
    for hash in hashes {
        let origins = orch_chat::following::blob_origins(conn, caller.user(), hash).await?;
        let mut allowed = Vec::new();
        for origin in origins {
            if selected.is_some_and(|selected| !selected.contains(&(*hash, origin))) {
                continue;
            }
            let resolved = match binding(conn, caller.user(), origin).await {
                Ok(binding) => binding,
                Err(Error::Target(targets::TargetError::Missing))
                | Err(Error::Target(targets::TargetError::Authz(
                    orch_authz::Error::NotFound(_)
                    | orch_authz::Error::Archived { .. }
                    | orch_authz::Error::Suspended { .. },
                ))) => continue,
                Err(error) => return Err(error),
            };
            if base.permits_binding(
                caller,
                index(base, resolved.source)?,
                resolved.source.kind(),
                std::slice::from_ref(&resolved.target),
            )? {
                proposed.push(resolved.clone());
                allowed.push((origin, resolved));
            }
        }
        if allowed.is_empty() {
            return Err(Error::Missing);
        }
        candidates.push((*hash, allowed));
    }
    let mut preparation = base.clone();
    bind(&mut preparation, &proposed)?;
    // Source scope and ordinary account visibility stay separate. The supplied Principal already
    // carries the caller's normal assurance/held-place state; do not reload it and lose that bound.
    let _ = preparation.preparation_caller(caller)?;
    let mut selected = Vec::new();
    let mut bindings = required.to_vec();
    for (hash, candidates) in candidates {
        let allowed: Vec<_> = candidates.iter().map(|(origin, _)| *origin).collect();
        let origin = orch_chat::following::blob_origin_in(conn, principal, &hash, &allowed)
            .await?
            .ok_or(Error::Missing)?;
        let binding = candidates
            .into_iter()
            .find_map(|(candidate, binding)| (candidate == origin).then_some(binding))
            .ok_or_else(|| Error::Declaration("a selected blob source was not prepared"))?;
        bindings.push(binding);
        selected.push((hash, origin));
    }
    Ok(Prepared { bindings, selected })
}
