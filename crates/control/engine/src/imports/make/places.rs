//! Making places and document folders: a project or a topic with its owner and its page, and a document folder with its own; or, where the plan says the same thing is there, taking it over so that what lies in it goes in it.

use super::{Batch, Made, MakeError, free_name, note_origin, refuse, settle_name};
use crate::document_tree::{Actor as TreeActor, Directory};
use crate::imports::model::Object;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::pages::StoredKind;
use orch_api_contract::imports::ImportAction;
use orch_api_contract::package::PackageKind;
use orch_db::audit::{Actor, Entry as AuditEntry, append};
use orch_db::queries::documents::containers;
use orch_db::queries::hierarchy;
use orch_db::{DbError, Tx};
use orch_types::{Id, Time};
use serde_json::json;

/// The longest name a place may have.
const PLACE_NAME_MAX: usize = 120;
/// The longest description a place may have.
const PLACE_DESCRIPTION_MAX: usize = 4000;
/// The longest name of a document folder.
const FOLDER_NAME_MAX: usize = 200;
/// What a place is called whose name the package could not give.
const UNNAMED_PLACE: &str = "Imported place";

fn cut(text: &str, most: usize) -> String {
    text.chars()
        .take(most)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn named(object: &Object, most: usize) -> String {
    let name = cut(&object.object.name, most);
    if name.is_empty() {
        UNNAMED_PLACE.to_owned()
    } else {
        name
    }
}

/// Makes a project or a topic, or takes over the one the plan says is there.
pub(super) async fn make_place(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
) -> Result<Made, MakeError> {
    let importer = batch.principal.user();
    if object.action == ImportAction::Replace {
        // What lies in it goes in it; its name and what it says of itself are its own.
        note_origin(
            tx,
            batch,
            object,
            object.target,
            Some(object.lies_in),
            false,
        )
        .await?;
        return Ok(Made::done(object, object.target));
    }
    let kind = if object.made_as == PackageKind::Topic {
        "topic"
    } else {
        "project"
    };
    let mut name = named(object, PLACE_NAME_MAX);
    if object.action == ImportAction::KeepBoth {
        name = free_name(
            tx,
            object.made_as,
            object.lies_in,
            None,
            &name,
            PLACE_NAME_MAX,
        )
        .await?;
    }
    if kind == "project" {
        // A project in a project lies within as many projects as the system allows.
        let parent_kind: Option<String> =
            sqlx::query_scalar("SELECT kind FROM nodes WHERE id = $1")
                .bind(object.lies_in)
                .fetch_optional(&mut **tx)
                .await
                .map_err(DbError::from)?;
        if parent_kind.as_deref() == Some("project") {
            let held = hierarchy::projects_on_the_way(tx, object.lies_in).await?;
            let limit = batch.engine.inner.config.project_depth;
            if !orch_authz::orgs::fits_project_depth(held + 1, limit) {
                return refuse(format!(
                    "projects may lie in one another {limit} deep at most"
                ));
            }
        }
    }
    let owner = orch_authz::ownership::owner_of_new(tx, importer, Some(object.lies_in)).await?;
    let description = cut(&object.object.description, PLACE_DESCRIPTION_MAX);
    let at = object
        .object
        .created_at
        .map_or(batch.now, |created| created.get());
    hierarchy::create(
        tx,
        &hierarchy::NewNode {
            id: object.target,
            kind,
            parent: Some(object.lies_in),
            name: &name,
            name_by_person: true,
            description: &description,
            description_by_person: !description.is_empty(),
            owner,
            org: None,
            at,
        },
    )
    .await?;
    orch_authz::pages::start(
        tx,
        StoredKind::Node,
        object.target,
        importer,
        Time::new(batch.now),
    )
    .await?;
    // The times it had there are the times it has here; that it was imported, and when, is the origin's.
    if let Some(changed) = object.object.updated_at {
        sqlx::query("UPDATE nodes SET updated_at = $2 WHERE id = $1")
            .bind(object.target)
            .bind(changed.get())
            .execute(&mut **tx)
            .await
            .map_err(DbError::from)?;
    }
    let mut audit = AuditEntry::new(
        Actor::User(importer),
        orch_db::audit::action!(AuditAction::NodeCreated).as_str(),
        "node",
        Some(object.target),
        "A place was made by an import.",
    );
    audit.node_id = Some(object.target);
    audit.org_id = Some(batch.plan.destination.org);
    audit.details =
        json!({ "kind": kind, "name": name, "parent": object.lies_in, "import": batch.import });
    append(tx, &audit, &orch_db::audit::Origin::default(), batch.now)
        .await
        .map_err(DbError::from)?;
    note_origin(tx, batch, object, object.target, Some(object.lies_in), true).await?;
    let mut made = Made::done(object, object.target);
    super::access::restore(tx, batch, object, object.target, &mut made).await?;
    Ok(made)
}

/// Makes a document folder, or takes over the one the plan says is there.
pub(super) async fn make_folder(
    tx: &mut Tx,
    batch: &Batch<'_>,
    object: &Object,
) -> Result<Made, MakeError> {
    if object.action == ImportAction::Replace {
        // What lies in the folder goes in it: the person has to be let add to it now, and a folder they cannot see is not one they find.
        let allowed = matches!(
            orch_authz::documents::check(
                tx,
                batch.principal,
                object.target,
                orch_access_contract::permissions::Permission::DocCreate,
                orch_authz::Policy::default(),
            )
            .await,
            Ok(orch_access_contract::decisions::Decision::Allowed { .. })
        );
        if !allowed {
            return refuse("you may not add to the folder that is there");
        }
        note_origin(
            tx,
            batch,
            object,
            object.target,
            Some(object.lies_in),
            false,
        )
        .await?;
        return Ok(Made::done(object, object.target));
    }
    containers::lock_scopes(tx, &[object.lies_in]).await?;
    let mut name = cut(&object.object.name, FOLDER_NAME_MAX);
    if name.is_empty() {
        name = "Folder".to_owned();
    }
    // The names the folders of the product keep for themselves.
    let lower = name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "docs" | "messages" | "conversation.md" | "chat.md"
    ) || lower.ends_with(".files")
        || lower.ends_with(".history")
        || name.contains(['/', '\\'])
        || matches!(name.as_str(), "." | "..")
    {
        name = name.replace(['/', '\\'], "-");
        name.push_str(" folder");
    }
    if object.action == ImportAction::KeepBoth || !object.parent_made {
        name = settle_name(
            tx,
            PackageKind::Folder,
            object.lies_in,
            object.container,
            &name,
            FOLDER_NAME_MAX,
        )
        .await?;
    }
    let at = object
        .object
        .created_at
        .map_or(batch.now, |created| created.get());
    // By the ordinary rule of making a folder: the person needs Create where it goes at this moment (a private folder, or a right taken
    // away since the plan, is a refusal), and the documents of the place have to be on.
    let parent = match object.container {
        Some(container) => Directory::Container(container),
        None => Directory::NodeDocs(object.lies_in),
    };
    let made_now = crate::document_tree::create_folder_in(
        tx,
        TreeActor::Person(batch.principal),
        parent,
        object.target,
        &name,
        Time::new(at),
        Time::new(batch.now),
    )
    .await?;
    if !made_now {
        // A retry of what was made: it stands.
        note_origin(tx, batch, object, object.target, Some(object.lies_in), true).await?;
        return Ok(Made::done(object, object.target));
    }
    note_origin(tx, batch, object, object.target, Some(object.lies_in), true).await?;
    Ok(Made::done(object, object.target))
}

/// The identifier of what an object lies in, for the origin.
#[must_use]
pub fn _lies_in(object: &Object) -> Id {
    object.container.unwrap_or(object.lies_in)
}
