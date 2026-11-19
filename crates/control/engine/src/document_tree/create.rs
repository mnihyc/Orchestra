//! Guarded text headers and ordinary destination ownership without agent Manage grants.

use super::{Actor, Directory, Error, TextCreation, access};
use orch_access_contract::pages::StoredKind;
use orch_db::{
    Conn, Tx,
    queries::{
        access::{self as grants, NewEntry, PageKey},
        documents::{self as rows, containers},
    },
};
use orch_types::{Id, Time, new_id};
use serde_json::json;

pub(super) fn validate_name(name: &str, folder: bool) -> Result<(), Error> {
    // Existing canonical names can carry a full UUID disambiguator after a maximum-length title.
    const UUID_NAME_SUFFIX_CHARS: usize = 33;
    let title_max = usize::try_from(orch_api_contract::documents::DOCUMENT_TITLE_MAX_CHARS)
        .unwrap_or(usize::MAX);
    let maximum = if folder {
        title_max
    } else {
        title_max
            .saturating_add(UUID_NAME_SUFFIX_CHARS)
            .saturating_add(".md".len())
    };
    if name.is_empty()
        || name != name.trim()
        || name.chars().count() > maximum
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
    {
        return Err(Error::Invalid(
            "give one valid basename within the document title and canonical suffix bounds",
        ));
    }
    let folded = name.to_ascii_lowercase();
    if folder
        && (matches!(
            folded.as_str(),
            "docs" | "messages" | "conversation.md" | "chat.md"
        ) || folded.ends_with(".files")
            || folded.ends_with(".history"))
    {
        return Err(Error::Invalid(
            "a built-in document directory cannot be created or managed",
        ));
    }
    Ok(())
}

pub(super) async fn make_private(
    conn: &mut Conn,
    actor: Actor<'_>,
    id: Id,
    node: Id,
) -> Result<(), Error> {
    let Actor::Person(principal) = actor else {
        return Err(Error::Denied);
    };
    let external = orch_settings::get::<bool>(
        conn,
        orch_settings::Place::Node(node),
        orch_settings_contract::registry::keys::SHARING_ALLOW_EXTERNAL,
    )
    .await
    .map_err(crate::EngineError::from)?
    .unwrap_or(true);
    let policy = orch_authz::usable::ResourcePolicy {
        owners_name_outsiders: external,
        shared_subscriptions: None,
    };
    let change = orch_access_contract::pages::PageChange {
        whole: Some(orch_access_contract::pages::Whole::Private),
        ..Default::default()
    };
    let object = orch_access_contract::decisions::ObjectRef {
        kind: orch_access_contract::permissions::ObjectKind::Document,
        id,
    };
    orch_authz::pages::change_resource(
        conn,
        principal,
        object,
        &change,
        &policy,
        &orch_db::audit::current_origin(),
    )
    .await
    .map_err(|error| match error {
        orch_authz::pages::ChangeError::Engine(error) => Error::Authorization(error),
        _ => Error::Denied,
    })?;
    Ok(())
}

pub(super) fn basename(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\') {
                '-'
            } else {
                ch
            }
        })
        .collect();
    let name = name.trim();
    if matches!(name, "" | "." | "..") {
        format!("_{name}_.md")
    } else {
        format!("{name}.md")
    }
}

pub(super) async fn creation_grant(
    conn: &mut Conn,
    id: Id,
    folder: bool,
    identity: &access::Identity,
    now: Time,
) -> Result<(), Error> {
    let kind = if folder {
        StoredKind::DocumentContainer
    } else {
        StoredKind::Document
    };
    orch_authz::pages::start(conn, kind, id, identity.owner, now).await?;
    if let Some(thread) = identity.thread {
        grants::insert_entry(
            conn,
            &NewEntry {
                id: new_id(),
                page: &PageKey::new(kind.as_str(), id, "live"),
                profile: "any",
                permission: "doc.edit",
                holder_kind: "agents",
                holder_id: Some(thread),
                holder_role: None,
                options: &json!({}),
                expires_at: None,
                note: "",
                source: "creation",
                created_by: Some(identity.owner),
                at: now.get(),
            },
        )
        .await?;
    }
    Ok(())
}

/// Inserts an ordinary text header and its destination page, leaving current-content batching to the caller.
pub async fn create_text_in(
    tx: &mut Tx,
    actor: Actor<'_>,
    parent: Directory,
    id: Id,
    title: &str,
    name: Option<&str>,
    now: Time,
) -> Result<TextCreation, Error> {
    sqlx::query("SAVEPOINT document_text_create")
        .execute(&mut **tx)
        .await?;
    let result = create_text(&mut *tx, actor, parent, id, title, name, now).await;
    if result.is_err() {
        sqlx::query("ROLLBACK TO SAVEPOINT document_text_create")
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("RELEASE SAVEPOINT document_text_create")
        .execute(&mut **tx)
        .await?;
    result
}

/// Makes the folder `id` named `name` under `parent`, as the actor makes one by hand: they need Create there at this moment, and the
/// documents of the place have to be switched on. The folder takes `created_at` as the moment it was made. Returns whether it was made;
/// `false` is a folder that is there already under that identifier (a retry), which stands.
pub async fn create_folder_in(
    tx: &mut Tx,
    actor: Actor<'_>,
    parent: Directory,
    id: Id,
    name: &str,
    created_at: Time,
    now: Time,
) -> Result<bool, Error> {
    sqlx::query("SAVEPOINT document_folder_create")
        .execute(&mut **tx)
        .await?;
    let result = create_folder(&mut *tx, actor, parent, id, name, created_at, now).await;
    if result.is_err() {
        sqlx::query("ROLLBACK TO SAVEPOINT document_folder_create")
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("RELEASE SAVEPOINT document_folder_create")
        .execute(&mut **tx)
        .await?;
    result
}

async fn create_folder(
    conn: &mut Conn,
    actor: Actor<'_>,
    parent: Directory,
    id: Id,
    name: &str,
    created_at: Time,
    now: Time,
) -> Result<bool, Error> {
    validate_name(name, true)?;
    // The directory is locked as a tree act locks it, so that nothing is inserted into it as it is moved or deleted.
    let initial = access::directory(conn, parent).await?;
    containers::lock_scopes(conn, &[initial.0]).await?;
    let (node, root, container) = access::require_create(conn, actor, parent, now).await?;
    if node != initial.0 {
        return Err(Error::Stale);
    }
    if containers::get(conn, id).await?.is_some() {
        return Ok(false);
    }
    let identity = access::identity(conn, actor).await?;
    containers::insert(
        conn,
        &containers::NewContainer {
            id,
            node,
            root,
            parent: container,
            owner: identity.owner,
            thread: identity.thread,
            agent: identity.agent.as_deref(),
            name,
            now: created_at.get(),
        },
    )
    .await?;
    creation_grant(conn, id, true, &identity, now).await?;
    Ok(true)
}

struct TextPlan {
    parent: Directory,
    id: Id,
    title: String,
    name: String,
    identity: access::Identity,
    node: Id,
    root: Id,
    container: Option<Id>,
    request_ref: String,
    arguments: orch_types::Json,
    existing: Option<rows::DocumentRow>,
}

/// Native text-header operands under their original directory serialization lock.
/// The plan is consumed once on the exact transaction that prepared it.
pub struct TextCreationPlan {
    packet: TextPlan,
    transaction: String,
}

impl TextCreationPlan {
    /// The actual document identity chosen before native header preparation.
    #[must_use]
    pub fn id(&self) -> Id {
        self.packet.id
    }

    /// The actual existing directory, not the newly generated document.
    #[must_use]
    pub fn destination(&self) -> Directory {
        self.packet.parent
    }

    /// The factual hierarchy anchor after taking the native directory lock.
    #[must_use]
    pub fn node(&self) -> Id {
        self.packet.node
    }

    /// Whether the same stable identity already has its native document.
    #[must_use]
    pub fn is_replay(&self) -> bool {
        self.packet.existing.is_some()
    }
}

async fn transaction_id(conn: &mut Conn) -> Result<String, Error> {
    Ok(sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(conn)
        .await?)
}

/// Runs the unchanged native title, directory, namespace and stable-id replay preparation.
/// Preparation outside an explicit transaction remains provisional and cannot later apply.
pub async fn prepare_text_creation_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    parent: Directory,
    id: Id,
    title: &str,
    name: Option<&str>,
    now: Time,
) -> Result<TextCreationPlan, Error> {
    let packet = prepare_text(conn, actor, parent, id, title, name, now).await?;
    Ok(TextCreationPlan {
        packet,
        transaction: transaction_id(conn).await?,
    })
}

/// Repeats only this native header plan's ordinary destination and replay gates.
pub async fn require_text_creation_in(
    conn: &mut Conn,
    actor: Actor<'_>,
    prepared: &TextCreationPlan,
    now: Time,
) -> Result<(), Error> {
    let placement = access::require_create(conn, actor, prepared.packet.parent, now).await?;
    if placement
        != (
            prepared.packet.node,
            prepared.packet.root,
            prepared.packet.container,
        )
    {
        return Err(Error::Stale);
    }
    if prepared.packet.existing.is_some() {
        let entry = containers::entry(conn, "document", prepared.packet.id)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(Error::NotFound)?;
        access::require_entries(
            conn,
            actor,
            &[entry],
            orch_access_contract::permissions::Permission::DocView,
            now,
            false,
            orch_authz::Act::Use,
        )
        .await?;
    }
    Ok(())
}

/// Applies the original native header/page/receipt writer after one opaque first-effect check.
/// Durable native replay returns without invoking that callback or repeating material.
pub async fn apply_text_creation_in<E, F>(
    tx: &mut Tx,
    actor: Actor<'_>,
    prepared: TextCreationPlan,
    now: Time,
    before: F,
) -> Result<Result<TextCreation, Error>, E>
where
    F: FnOnce() -> Result<(), E>,
{
    let checked: Result<(), Error> = async {
        if transaction_id(&mut *tx).await? != prepared.transaction {
            return Err(Error::Stale);
        }
        let current = access::identity(&mut *tx, actor).await?;
        let earlier = &prepared.packet.identity;
        if current.kind != earlier.kind
            || current.id != earlier.id
            || current.owner != earlier.owner
            || current.thread != earlier.thread
        {
            return Err(Error::Denied);
        }
        Ok(())
    }
    .await;
    if let Err(error) = checked {
        return Ok(Err(error));
    }
    if prepared.packet.existing.is_some() {
        return Ok(finish_text(&mut *tx, &prepared.packet, now).await);
    }
    if let Err(error) = sqlx::query("SAVEPOINT document_text_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    if let Err(error) = before() {
        if let Err(native) = sqlx::query("RELEASE SAVEPOINT document_text_prepared")
            .execute(&mut **tx)
            .await
        {
            return Ok(Err(native.into()));
        }
        return Err(error);
    }
    let result = finish_text(&mut *tx, &prepared.packet, now).await;
    if result.is_err()
        && let Err(error) = sqlx::query("ROLLBACK TO SAVEPOINT document_text_prepared")
            .execute(&mut **tx)
            .await
    {
        return Ok(Err(error.into()));
    }
    if let Err(error) = sqlx::query("RELEASE SAVEPOINT document_text_prepared")
        .execute(&mut **tx)
        .await
    {
        return Ok(Err(error.into()));
    }
    Ok(result)
}

pub(super) async fn create_text(
    conn: &mut Conn,
    actor: Actor<'_>,
    parent: Directory,
    id: Id,
    title: &str,
    name: Option<&str>,
    now: Time,
) -> Result<TextCreation, Error> {
    let prepared = prepare_text(conn, actor, parent, id, title, name, now).await?;
    finish_text(conn, &prepared, now).await
}

async fn prepare_text(
    conn: &mut Conn,
    actor: Actor<'_>,
    parent: Directory,
    id: Id,
    title: &str,
    name: Option<&str>,
    now: Time,
) -> Result<TextPlan, Error> {
    let title = title.trim();
    if title.is_empty()
        || title.chars().count()
            > usize::try_from(orch_api_contract::documents::DOCUMENT_TITLE_MAX_CHARS)
                .unwrap_or(usize::MAX)
    {
        return Err(Error::Invalid(
            "a document title has one to two hundred characters",
        ));
    }
    let requested_name = name;
    let guessed = basename(title);
    let name = requested_name.unwrap_or(&guessed);
    validate_name(name, false)?;
    let identity = access::identity(conn, actor).await?;
    let request_ref = format!("text-create:{id}");
    let arguments = json!({"parent":parent,"title":title,"basename":requested_name});
    let initial = access::directory(conn, parent).await?;
    containers::lock_scopes(conn, &[initial.0]).await?;
    let (node, root, container) = access::require_create(conn, actor, parent, now).await?;
    if node != initial.0 {
        return Err(Error::Stale);
    }
    let saved = containers::operation(conn, identity.kind, identity.id, &request_ref).await?;
    if saved
        .as_ref()
        .is_some_and(|saved| saved.arguments != arguments)
    {
        return Err(Error::Conflict);
    }
    if let Some(document) = rows::get(conn, id).await? {
        let same = match actor {
            Actor::Person(principal) => document.created_by == Some(principal.user()),
            Actor::Agent { thread } => document.created_by_thread == Some(thread),
        };
        if !same || document.deleted_at.is_some() {
            return Err(Error::Conflict);
        }
        let entry = containers::entry(conn, "document", id)
            .await?
            .ok_or(Error::NotFound)?;
        if saved.is_none()
            && (access::project(&entry).parent != parent
                || entry.title != title
                || requested_name.is_some_and(|name| entry.basename != name))
        {
            return Err(Error::Conflict);
        }
        access::require_entries(
            conn,
            actor,
            &[entry],
            orch_access_contract::permissions::Permission::DocView,
            now,
            false,
            orch_authz::Act::Use,
        )
        .await?;
        return Ok(TextPlan {
            parent,
            id,
            title: title.to_owned(),
            name: name.to_owned(),
            identity,
            node,
            root,
            container,
            request_ref,
            arguments,
            existing: Some(document),
        });
    }
    if saved.is_some() {
        return Err(Error::NotFound);
    }
    // A title is a display value. Only an explicitly supplied path claims that
    // exact basename; ordinary unnamed pages may coexist without overwriting.
    // The existing scope lock owns allocation. The new identity provides one
    // stable suffix, without scanning siblings or retrying an unbounded series.
    let allocated = if requested_name.is_none()
        && containers::name_taken(conn, node, container, name, None).await?
    {
        Some(format!(
            "{}-{}.md",
            guessed.strip_suffix(".md").unwrap_or(&guessed),
            id.simple()
        ))
    } else {
        None
    };
    let name = allocated.as_deref().unwrap_or(name);
    Ok(TextPlan {
        parent,
        id,
        title: title.to_owned(),
        name: name.to_owned(),
        identity,
        node,
        root,
        container,
        request_ref,
        arguments,
        existing: None,
    })
}

async fn finish_text(
    conn: &mut Conn,
    prepared: &TextPlan,
    now: Time,
) -> Result<TextCreation, Error> {
    if let Some(document) = &prepared.existing {
        return Ok(TextCreation {
            document: document.clone(),
            created: false,
        });
    }
    let id = prepared.id;
    containers::insert_document(
        conn,
        &rows::NewDocument {
            id,
            node_id: prepared.node,
            org_id: prepared.root,
            owner_id: prepared.identity.owner,
            created_by_thread: prepared.identity.thread,
            created_by_agent: prepared.identity.agent.as_deref(),
            title: &prepared.title,
            now: now.get(),
        },
        prepared.container,
        &prepared.name,
    )
    .await?;
    creation_grant(conn, id, false, &prepared.identity, now).await?;
    containers::remember(
        conn,
        prepared.identity.kind,
        prepared.identity.id,
        &prepared.request_ref,
        &containers::Operation {
            arguments: prepared.arguments.clone(),
            operation_id: id,
            result: json!({"document":id}),
        },
        now.get(),
    )
    .await?;
    orch_authz::epoch::bump(conn).await?;
    let document = rows::get(conn, id).await?.ok_or(Error::NotFound)?;
    Ok(TextCreation {
        document,
        created: true,
    })
}
