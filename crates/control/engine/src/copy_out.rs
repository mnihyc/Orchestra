//! One source-governance boundary for native duplication and copy-then-delete.
use crate::EngineError;
use orch_access_contract::{
    decisions::{Decision, ObjectRef, Refusal},
    permissions::{ObjectKind, Permission},
};
use orch_authz::{Act, Policy, Principal};
use orch_db::{
    Conn,
    queries::{chats, hierarchy, resources},
};
use orch_types::Id;

/// An actual native subject, never a caller-supplied permission or ancestry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// Stored resource, Thread, document or document-only folder.
    Object(ObjectRef),
    /// A real group conversation; direct chats have no copy/move owner.
    Chat(Id),
}
/// Actual source and destination namespaces observed at this native checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Boundary {
    pub subject: Subject,
    pub source_home: Option<Id>,
    pub source_org: Option<Id>,
    pub destination_org: Option<Id>,
    roots: Vec<MaterialRoot>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MaterialRoot {
    home: Option<Id>,
    org: Option<Id>,
    knowledge: bool,
}
impl Boundary {
    /// Actual declared knowledge roots whose material leaves, deduplicated against storage.
    pub fn knowledge_roots(&self) -> impl Iterator<Item = Id> + '_ {
        self.roots
            .iter()
            .filter(|root| root.knowledge)
            .filter_map(|root| root.org)
    }
    fn stored_exits(&self) -> bool {
        self.source_org != self.destination_org
    }
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the copy subject or destination is not available")]
    NotFound,
    #[error("this place keeps its content inside its root organization")]
    Policy {
        at: Option<Id>,
        locked: bool,
        root: Option<Id>,
    },
    #[error("the source Export permission is required")]
    Export(Box<Refusal>),
    #[error("the source chat Export permission is required")]
    ChatExport,
    #[error("the knowledge root's Export row does not permit this copy")]
    KnowledgeExport { root: Id, permission: Permission },
    #[error(transparent)]
    Native(#[from] EngineError),
}
impl From<orch_db::DbError> for Error {
    fn from(error: orch_db::DbError) -> Self {
        Self::Native(error.into())
    }
}
impl From<orch_authz::Error> for Error {
    fn from(error: orch_authz::Error) -> Self {
        Self::Native(error.into())
    }
}
impl From<orch_settings::Error> for Error {
    fn from(error: orch_settings::Error) -> Self {
        Self::Native(error.into())
    }
}

/// Resolve current native source placement, including an environment-owned server's actual parent.
pub async fn source_place(
    conn: &mut Conn,
    subject: Subject,
) -> Result<(Option<Id>, Option<Id>), Error> {
    match subject {
        Subject::Chat(id) => {
            let row = chats::chat(conn, id)
                .await?
                .filter(|row| row.kind == "group")
                .ok_or(Error::NotFound)?;
            Ok((Some(row.home()), Some(row.org_id)))
        }
        Subject::Object(object) if object.kind == ObjectKind::Thread => {
            let row = hierarchy::node(conn, object.id)
                .await?
                .filter(|row| row.kind == "thread")
                .ok_or(Error::NotFound)?;
            Ok((Some(row.id), Some(row.org_id)))
        }
        Subject::Object(object) if object.kind.is_document() => {
            let (_, document, site) = orch_authz::documents::site(conn, object.id).await?;
            if document.kind != object.kind || document.deleted {
                return Err(Error::NotFound);
            }
            Ok((site.beside, site.root))
        }
        Subject::Object(mut object) => {
            if object.kind == ObjectKind::Machine
                && let Some(id) = resources::hosting::host_of_machine(conn, object.id).await?
            {
                object = ObjectRef {
                    kind: ObjectKind::Host,
                    id,
                };
            }
            if object.kind == ObjectKind::McpServer {
                let row = resources::mcp::subject(conn, object.id)
                    .await?
                    .ok_or(Error::NotFound)?;
                if let Some(environment) = row.environment_id {
                    let env = resources::resource_facts(conn, &[environment])
                        .await?
                        .into_iter()
                        .find(|row| row.kind == "environment" && !row.deleted)
                        .ok_or(Error::NotFound)?;
                    return Ok((env.home_id, env.org_id));
                }
            }
            let row = resources::resource_facts(conn, &[object.id])
                .await?
                .into_iter()
                .find(|row| row.kind == object.kind.as_str() && !row.deleted)
                .ok_or(Error::NotFound)?;
            Ok((row.home_id, row.org_id))
        }
    }
}

/// An actual source setting, using the existing hierarchy resolver and lock semantics.
/// Same-root work is unchanged. No namespace fact or permission is admitted here.
pub async fn inspect(
    conn: &mut Conn,
    subject: Subject,
    destination: Option<Id>,
) -> Result<Option<Boundary>, Error> {
    let boundary = material_boundary(conn, subject, destination).await?;
    if let Some(boundary) = &boundary {
        for root in &boundary.roots {
            require_policy(conn, root).await?;
        }
    }
    Ok(boundary)
}

/// Current factual storage and declared material roots. This supplies no permission or admission.
pub async fn material_boundary(
    conn: &mut Conn,
    subject: Subject,
    destination: Option<Id>,
) -> Result<Option<Boundary>, Error> {
    let (source_home, source_org) = source_place(conn, subject).await?;
    let destination_org = match destination {
        Some(node) => Some(
            hierarchy::node(conn, node)
                .await?
                .ok_or(Error::NotFound)?
                .org_id,
        ),
        None => None,
    };
    let mut roots = vec![MaterialRoot {
        home: source_home,
        org: source_org,
        knowledge: false,
    }];
    let node = match subject {
        Subject::Object(object)
            if object.kind == ObjectKind::Thread || object.kind.is_document() =>
        {
            source_home
        }
        Subject::Chat(_) => source_home,
        _ => None,
    };
    if let Some(node) = node
        && let Some(id) = orch_db::queries::ask::program_root(conn, node).await?
    {
        let row = hierarchy::node(conn, id)
            .await?
            .filter(|row| row.kind == "org" && row.org_id == row.id)
            .ok_or(Error::NotFound)?;
        if let Some(root) = roots.iter_mut().find(|root| root.org == Some(row.id)) {
            root.knowledge = true;
        } else {
            roots.push(MaterialRoot {
                home: Some(row.id),
                org: Some(row.id),
                knowledge: true,
            });
        }
    }
    roots.retain(|root| root.org != destination_org);
    if roots.is_empty() {
        return Ok(None);
    }
    Ok(Some(Boundary {
        subject,
        source_home,
        source_org,
        destination_org,
        roots,
    }))
}

async fn require_policy(conn: &mut Conn, root: &MaterialRoot) -> Result<(), Error> {
    let place = root
        .home
        .map_or(orch_settings::Place::System, orch_settings::Place::Node);
    let setting = orch_settings::resolver::resolve(
        conn,
        place,
        orch_settings_contract::registry::keys::SHARING_ALLOW_COPY_OUT,
    )
    .await?;
    let permitted = match setting.value() {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| EngineError::unreadable("The copy-out policy is not a Boolean."))?,
        None => true,
    };
    if !permitted {
        let locked = setting.resolved.locked_by.is_some();
        let at = setting
            .resolved
            .locked_by
            .as_ref()
            .map(|row| row.node)
            .unwrap_or(setting.resolved.source.node);
        return Err(Error::Policy {
            at,
            locked,
            root: root.org,
        });
    }
    Ok(())
}

/// The person's existing native Export door is conjunctive with source View and destination Make.
pub async fn authorize(
    conn: &mut Conn,
    principal: &Principal,
    subject: Subject,
    destination: Option<Id>,
) -> Result<Option<Boundary>, Error> {
    let Some(boundary) = inspect(conn, subject, destination).await? else {
        return Ok(None);
    };
    require_knowledge_exports(conn, principal, &boundary).await?;
    if !boundary.stored_exits() {
        return Ok(Some(boundary));
    }
    let decision = match subject {
        Subject::Object(object) if object.kind == ObjectKind::Thread => Some(
            orch_authz::nodes::check(
                conn,
                principal,
                object.id,
                Permission::ThreadExport,
                Act::Use,
                Policy::default(),
            )
            .await?,
        ),
        Subject::Object(object) if object.kind.is_document() => Some(
            orch_authz::documents::check(
                conn,
                principal,
                object.id,
                Permission::DocExport,
                Policy::default(),
            )
            .await?,
        ),
        Subject::Chat(id) => {
            let row = chats::chat(conn, id).await?.ok_or(Error::NotFound)?;
            let members = chats::members(conn, id).await?;
            let enabled = orch_settings::get::<bool>(
                conn,
                orch_settings::Place::Node(row.home()),
                orch_settings_contract::registry::keys::CHAT_ENABLED,
            )
            .await?
            .unwrap_or(true);
            let direct = orch_settings::get::<String>(
                conn,
                orch_settings::Place::Node(row.home()),
                orch_settings_contract::registry::keys::CHAT_DIRECT,
            )
            .await?
            .is_none_or(|word| word != "off");
            if !orch_authz::chats::standing(
                conn,
                principal,
                &row,
                &members,
                orch_authz::chats::ChatPolicy { enabled, direct },
            )
            .await?
            .export
            {
                return Err(Error::ChatExport);
            }
            Some(
                orch_authz::nodes::check(
                    conn,
                    principal,
                    row.home(),
                    Permission::ChatExport,
                    Act::Use,
                    Policy::default(),
                )
                .await?,
            )
        }
        _ => None,
    };
    if let Some(Decision::Refused(refusal)) = decision {
        return Err(Error::Export(Box::new(refusal)));
    }
    Ok(Some(boundary))
}

/// Additional defaults control the same admitted native subject; owning the personal copy is
/// never substituted for the governing material's Export row.
pub async fn require_knowledge_exports(
    conn: &mut Conn,
    principal: &Principal,
    boundary: &Boundary,
) -> Result<(), Error> {
    if boundary.knowledge_roots().next().is_none() {
        return Ok(());
    }
    let permission = match boundary.subject {
        Subject::Chat(_) => Permission::ChatExport,
        Subject::Object(object) if object.kind == ObjectKind::Thread => Permission::ThreadExport,
        Subject::Object(object) if object.kind.is_document() => Permission::DocExport,
        _ => return Ok(()),
    };
    let profile = match boundary.subject {
        Subject::Object(object) if object.kind == ObjectKind::Thread => {
            let graph = orch_authz::engine::Graph::load(conn, &[object.id]).await?;
            let site = graph.site_of(object.id).ok_or(Error::NotFound)?;
            graph.profile(&site, permission)
        }
        _ => orch_access_contract::permissions::Profile::Any,
    };
    for root in boundary.knowledge_roots() {
        if !orch_authz::defaults::names_caller(conn, principal, root, permission, profile).await? {
            return Err(Error::KnowledgeExport { root, permission });
        }
    }
    Ok(())
}

/// Actual Owners standing; a group uses its active native manager flags rather than attribution.
pub async fn owns(conn: &mut Conn, principal: &Principal, subject: Subject) -> Result<bool, Error> {
    match subject {
        Subject::Chat(id) => {
            let row = chats::chat(conn, id).await?.ok_or(Error::NotFound)?;
            if row.kind != "group" {
                return Ok(false);
            }
            Ok(chats::is_manager(conn, id, principal.user(), principal.now().get()).await?)
        }
        Subject::Object(object) if object.kind == ObjectKind::Thread => Ok(
            orch_authz::nodes::effective(conn, principal, object.id, Policy::default())
                .await?
                .owner,
        ),
        Subject::Object(object) if object.kind.is_document() => Ok(
            orch_authz::documents::effective(conn, principal, object.id, Policy::default())
                .await?
                .owner,
        ),
        Subject::Object(mut object) => {
            if object.kind == ObjectKind::Machine
                && let Some(id) = resources::hosting::host_of_machine(conn, object.id).await?
            {
                object = ObjectRef {
                    kind: ObjectKind::Host,
                    id,
                };
            }
            if object.kind == ObjectKind::McpServer {
                let row = resources::mcp::subject(conn, object.id)
                    .await?
                    .ok_or(Error::NotFound)?;
                if let Some(id) = row.environment_id {
                    object = ObjectRef {
                        kind: ObjectKind::Environment,
                        id,
                    };
                }
            }
            Ok(
                orch_authz::resources::effective(conn, principal, object, Policy::default())
                    .await?
                    .owner,
            )
        }
    }
}

pub(crate) async fn require_continuing(
    conn: &mut Conn,
    principal: &Principal,
    subject: Subject,
    destination: Option<Id>,
) -> Result<(), crate::operation_scope::continuing::Error> {
    authorize(conn, principal, subject, destination)
        .await
        .map(|_| ())
        .map_err(|error| match error {
            Error::Native(error) => crate::operation_scope::continuing::Error::Native(error),
            _ => crate::operation_scope::continuing::Error::Refused,
        })
}

/// The source records one export in the same accepted creation transaction.
pub async fn record(
    conn: &mut Conn,
    boundary: Boundary,
    actor: orch_db::audit::Actor,
    origin: &orch_db::audit::Origin,
    created: Id,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let (kind, id) = match boundary.subject {
        Subject::Object(object) => (object.kind.as_str(), object.id),
        Subject::Chat(id) => ("chat", id),
    };
    for root in boundary.roots {
        let mut entry = orch_db::audit::Entry::new(
            actor,
            orch_access_contract::audit::AuditAction::ContentCopiedOut.as_str(),
            kind,
            Some(id),
            orch_access_contract::audit::AuditAction::ContentCopiedOut.as_str(),
        );
        entry.node_id = root.home;
        entry.org_id = root.org;
        entry.details =
            serde_json::json!({"destination_org_id":boundary.destination_org,"created_id":created});
        orch_db::audit::append(conn, &entry, origin, at).await?;
    }
    Ok(())
}
