//! Taking an import back: a plan of what would be deleted, and the deleting, which is the ordinary soft deletion that can be undone for as long as deletions can.
//!
//! What an import made is what its origins say it made (`created_by_import`): places and threads, document folders and documents, and the
//! messages, turns and items it added to things that were there. Only that is taken back, and only by a person who may delete it now. What
//! people added since stays where it is: a document somebody else changed, a thing written in a place of the import's, and so the places and
//! folders that hold it. The plan counts what stays (`added_since`, `edited_since`) so that the person knows before they confirm.

use super::ImportError;

pub mod operation;
use super::make::{Merged, Taken, merged_parents, take_back_finish};
use super::model::node_word;
use crate::engine::Engine;
use crate::jobs::import::Importing;
use crate::jobs::runner::{Done, Failure, INTERNAL, Job, Stop};
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::imports::{ImportBlocker, ImportCounts, ImportUndoPlan};
use orch_api_contract::package::PackageKind;
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::audit::{Actor, Entry, Origin, append};
use orch_db::queries::hierarchy;
use orch_db::queries::imports as rows;
use orch_db::{Conn, DbError};
use orch_types::{Id, Time, new_id};
use serde_json::json;
use std::collections::HashSet;

/// The real stored subjects represented by native undo counts and operations.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(
    tag = "kind",
    content = "subject",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Subject {
    /// A stored place or thread, whose actual kind is read from the node table.
    Node(Id),
    /// A document, including a parent of imported comments or versions.
    Document(Id),
    /// A document folder made by the import.
    Folder(Id),
    /// The actual chat into which imported messages were merged.
    Chat(Id),
    /// An actual resource selected by the existing native place-deletion producer.
    Resource {
        kind: orch_access_contract::permissions::ObjectKind,
        id: Id,
    },
}

/// The native undo source footprint before an aggregate or a job is produced.
#[derive(Clone)]
pub struct Prepared {
    /// The native creator-filtered stored Import.
    pub import: rows::ImportRow,
    /// Distinct real subjects; a million history rows can share one thread.
    pub subjects: Vec<Subject>,
}

impl Engine {
    /// Resolve actual undo inputs without rendering, recounting or opening source archives.
    pub async fn prepare_undo(
        &self,
        conn: &mut Conn,
        user: Id,
        import: Id,
    ) -> Result<Prepared, ImportError> {
        let row = self.import_of(conn, user, import).await?;
        let subjects = rows::undo_subjects(conn, import)
            .await?
            .into_iter()
            .map(|(kind, id)| {
                Ok(match kind.as_str() {
                    "node" => Subject::Node(id),
                    "document" => Subject::Document(id),
                    "folder" => Subject::Folder(id),
                    "chat" => Subject::Chat(id),
                    _ => {
                        return Err(ImportError::Internal(
                            crate::error::EngineError::unreadable(
                                "an undo source has an unknown native table",
                            ),
                        ));
                    }
                })
            })
            .collect::<Result<Vec<_>, ImportError>>()?;
        Ok(Prepared {
            import: row,
            subjects,
        })
    }

    /// The same original imported subjects plus the existing place deletion's actual resources.
    pub async fn prepare_undo_effects(
        &self,
        conn: &mut Conn,
        user: Id,
        import: Id,
    ) -> Result<Prepared, ImportError> {
        let mut prepared = self.prepare_undo(conn, user, import).await?;
        let candidates = prepared
            .subjects
            .iter()
            .filter_map(|subject| match subject {
                Subject::Node(id) => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>();
        let ids = rows::undo_created_nodes(conn, import, &candidates)
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        let nodes =
            hierarchy::with_ancestors(conn, &ids.iter().copied().collect::<Vec<_>>()).await?;
        for root in nodes.iter().filter(|row| {
            ids.contains(&row.id)
                && !row
                    .path
                    .iter()
                    .any(|ancestor| *ancestor != row.id && ids.contains(ancestor))
        }) {
            let deletion = self
                .inspect_home_deletion_in(conn, root.id)
                .await
                .map_err(|error| {
                    ImportError::Internal(crate::error::EngineError::unreadable(error.to_string()))
                })?;
            for resource in deletion.resources() {
                prepared.subjects.push(Subject::Resource {
                    kind: resource
                        .kind
                        .parse()
                        .map_err(crate::error::EngineError::unreadable)?,
                    id: resource.id,
                });
            }
        }
        prepared.subjects.sort_unstable();
        prepared.subjects.dedup();
        Ok(prepared)
    }
}

impl Engine {
    /// What taking the import back would remove: counted by a few statements over the rows that say what the import made, never by loading
    /// them, so that it costs the same for a thousand things as for a million.
    pub async fn plan_undo(
        &self,
        conn: &mut Conn,
        user: Id,
        import: Id,
    ) -> Result<ImportUndoPlan, ImportError> {
        let row = self.import_of(conn, user, import).await?;
        let blockers = match row.state.as_str() {
            rows::state::DONE | rows::state::FAILED | rows::state::CANCELLED => Vec::new(),
            _ => vec![ImportBlocker {
                code: "import.undone".to_owned(),
                message: "the import was taken back already, is being, or is not over".to_owned(),
                params: Some(json!({ "state": row.state })),
            }],
        };
        let mut counts = ImportCounts::default();
        let mut merged = ImportCounts::default();
        for (kind, count) in rows::counts_made(conn, import).await? {
            match kind.as_str() {
                "thread" => counts.threads += count,
                "org" | "project" | "topic" => counts.places += count,
                "folder" => counts.folders += count,
                "document" => counts.documents += count,
                "message" => merged.messages += count,
                "turn" | "item" => merged.turns += count,
                "comment" => merged.comments += count,
                _ => merged.versions += count,
            }
        }
        let added_since = rows::added_since(conn, import, row.created_at).await?;
        let edited_since = rows::edited_since(conn, import, user).await?;
        let keep = chrono::Duration::from_std(self.inner.config.soft_delete).unwrap_or_default();
        Ok(ImportUndoPlan {
            import,
            counts,
            merged,
            added_since,
            edited_since,
            restore_until: Time::new(self.inner.clock.now() + keep),
            blockers,
        })
    }
}

struct Deleting<'a> {
    id: Id,
    name: &'a str,
    now: DateTime<Utc>,
    since: DateTime<Utc>,
}

/// Deletes the place `id` and what lies in it, as the retention of deletions does for a place that is old enough: archived first where
/// it is not, its work settled, its subtree marked deleted as one group.
async fn delete_place(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    deleting: Deleting<'_>,
    mut before: impl FnMut() -> Result<(), Stop>,
) -> Result<bool, Stop> {
    let Deleting {
        id,
        name,
        now,
        since,
    } = deleting;
    let allowed = matches!(
        nodes::check_named(
            conn,
            principal,
            id,
            PermissionName::OfNode(NodePermission::Manage),
            Act::Delete,
            Policy::default(),
        )
        .await,
        Ok(orch_access_contract::decisions::Decision::Allowed { .. })
    );
    if !allowed {
        return Ok(false);
    }
    let user = principal.user();
    let (kind, org): (String, Id) = sqlx::query_as("SELECT kind, org_id FROM nodes WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
    engine
        .lock_home_in(conn, id)
        .await
        .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
    hierarchy::lock(conn, id).await?;
    orch_db::queries::documents::containers::lock_scopes(conn, &[id]).await?;
    if !rows::node_is_clear(conn, id, since).await? {
        return Ok(false);
    }
    let deleted: Vec<Id> = std::iter::once(id)
        .chain(
            hierarchy::descendants(conn, id)
                .await?
                .into_iter()
                .map(|row| row.id),
        )
        .collect();
    before()?;
    hierarchy::keep_archive_states(conn, id).await?;
    before()?;
    let marked = hierarchy::archive_by(conn, id, Some(user), now).await?;
    before()?;
    engine
        .archive_as_in(conn, id, Some(user), Time::new(now))
        .await
        .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
    before()?;
    engine
        .remove_home_as_in(conn, id, Some(user), Time::new(now), None)
        .await
        .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
    before()?;
    hierarchy::delete_subtree_by(conn, id, Some(user), now).await?;
    hierarchy::include_settled_in_deletion(conn, id, &deleted).await?;
    orch_authz::epoch::bump(conn)
        .await
        .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
    hierarchy::stamp_subtree(conn, id).await?;
    let summary = format!("An import was taken back: {name} was deleted");
    let mut entry = Entry::new(
        Actor::User(user),
        orch_db::audit::action!(AuditAction::NodeDeleted).as_str(),
        &kind,
        Some(id),
        &summary,
    );
    entry.history = true;
    entry.node_id = Some(id);
    entry.org_id = Some(org);
    entry.details =
        json!({ "archived_first": marked, "deleted_nodes": deleted, "import_undo": true });
    append(conn, &entry, &Origin::default(), now).await?;
    Ok(true)
}

fn source_stop(error: crate::operation_scope::continuing::Error) -> Stop {
    Failure::new("access.denied", error.to_string()).into()
}

/// How many places or threads, and how many documents or folders, are taken back in one page.
const NODES_PAGE: i64 = 50;
const DOCUMENTS_PAGE: i64 = 500;

/// Takes back what the import of the job made, and only that, in pages: the documents it made (those nobody else has changed since, and
/// that the person may still delete), the folders that hold nothing now, the places and threads it made that hold nothing that people
/// added since (the deepest first, each with its subtree in a transaction of its own), and what it added to things that were there. What
/// people added since is left where it is, and so is what holds it. Nothing is held but one page, so the work is the same for a million
/// things, and a job that stops goes on from what is still there.
pub async fn remove(importing: &Importing, job: &Job) -> Result<Done, Stop> {
    let engine = importing.engine();
    let import = importing.import();
    let (total, since) = {
        let mut conn = engine.inner.db.acquire().await?;
        let counts = rows::counts_made(&mut conn, import).await?;
        let began = rows::get(&mut conn, import)
            .await?
            .map_or_else(|| engine.inner.clock.now(), |row| row.created_at);
        (
            usize::try_from(counts.iter().map(|(_, count)| count).sum::<i64>())
                .unwrap_or(usize::MAX)
                .max(1),
            began,
        )
    };
    let mut done = 0_usize;
    let mut stays = 0_usize;
    // Documents first, a page at a time: what nobody else changed, and what the person may delete.
    let mut after: Option<Id> = None;
    loop {
        job.check()?;
        let page = {
            let mut conn = engine.inner.db.acquire().await?;
            rows::made_page(&mut conn, import, "document", after, DOCUMENTS_PAGE).await?
        };
        let Some(last) = page.last() else {
            break;
        };
        after = Some(*last);
        let now = engine.inner.clock.now();
        let cohort = new_id();
        let mut tx = engine.inner.db.begin().await?;
        let mut cut = importing
            .undo_cut(&mut tx, operation::Work::Documents(page.clone()))
            .await?;
        let principal = match &cut {
            Some(cut) => cut.principal().clone(),
            None => importing.principal_for(&mut tx).await?,
        };
        let user = principal.user();
        let edited: HashSet<Id> = rows::edited_by_others(&mut tx, import, &page, user)
            .await?
            .into_iter()
            .collect();
        let allowed: HashSet<Id> =
            orch_authz::documents::listed(&mut tx, &principal, &page, Policy::default())
                .await
                .map_err(|error| Failure::new(INTERNAL, error.to_string()))?
                .into_iter()
                .filter(|listed| listed.holds(Permission::DocManage))
                .map(|listed| listed.document.id)
                .collect();
        for document in &page {
            if allowed.contains(document) && !edited.contains(document) {
                orch_db::queries::documents::containers::lock_document_scope(&mut tx, *document)
                    .await?;
                if let Some(cut) = &mut cut {
                    cut.before(engine.inner.clock.as_ref())
                        .map_err(source_stop)?;
                }
                sqlx::query(
                    "UPDATE documents SET deleted_at = $2, deleted_by = $3, deleted_in = $4 WHERE id = $1 AND deleted_at IS NULL",
                )
                .bind(document)
                .bind(now)
                .bind(user)
                .bind(cohort)
                .execute(&mut *tx)
                .await
                .map_err(DbError::from)?;
                done += 1;
            } else {
                stays += 1;
            }
        }
        if let Some(cut) = &cut {
            cut.deadline(engine.inner.clock.as_ref())
                .map_err(source_stop)?;
        }
        tx.commit().await.map_err(DbError::from)?;
        job.progress(
            (done as f64 / total as f64).min(1.0),
            Some(&format!("{done} of {total} things")),
        )
        .await?;
    }
    // Folders that hold nothing now, again and again until none is left to take: a folder in a folder is empty once the one in it is gone.
    loop {
        job.check()?;
        let mut taken = 0_usize;
        let mut after: Option<Id> = None;
        loop {
            let page = {
                let mut conn = engine.inner.db.acquire().await?;
                rows::made_page(&mut conn, import, "folder", after, NODES_PAGE).await?
            };
            let Some(last) = page.last() else {
                break;
            };
            after = Some(*last);
            let now = engine.inner.clock.now();
            let cohort = new_id();
            let mut tx = engine.inner.db.begin().await?;
            let mut cut = importing
                .undo_cut(&mut tx, operation::Work::Folders(page.clone()))
                .await?;
            let principal = match &cut {
                Some(cut) => cut.principal().clone(),
                None => importing.principal_for(&mut tx).await?,
            };
            let user = principal.user();
            let empty = rows::empty_folders(&mut tx, &page).await?;
            for folder in &empty {
                let may = matches!(
                    orch_authz::documents::check(
                        &mut tx,
                        &principal,
                        *folder,
                        Permission::DocManage,
                        Policy::default(),
                    )
                    .await,
                    Ok(orch_access_contract::decisions::Decision::Allowed { .. })
                );
                if !may {
                    continue;
                }
                if let Some(cut) = &mut cut {
                    cut.before(engine.inner.clock.as_ref())
                        .map_err(source_stop)?;
                }
                sqlx::query(
                    "UPDATE document_containers SET deleted_at = $2, deleted_by = $3, deleted_in = $4
                      WHERE id = $1 AND deleted_at IS NULL",
                )
                .bind(folder)
                .bind(now)
                .bind(user)
                .bind(cohort)
                .execute(&mut *tx)
                .await
                .map_err(DbError::from)?;
                taken += 1;
            }
            if let Some(cut) = &cut {
                cut.deadline(engine.inner.clock.as_ref())
                    .map_err(source_stop)?;
            }
            tx.commit().await.map_err(DbError::from)?;
        }
        done += taken;
        if taken == 0 {
            break;
        }
    }
    // Places and threads, the deepest first: one that holds something people added since stays, and so does what it lies in.
    let mut after: Option<(i32, Id)> = None;
    loop {
        job.check()?;
        let page = {
            let mut conn = engine.inner.db.acquire().await?;
            rows::nodes_page(&mut conn, import, after, NODES_PAGE).await?
        };
        let Some((last, _, depth)) = page.last() else {
            break;
        };
        after = Some((*depth, *last));
        for (id, name, _) in &page {
            job.check()?;
            let now = engine.inner.clock.now();
            let mut tx = engine.inner.db.begin().await?;
            if !rows::node_is_clear(&mut tx, *id, since).await? {
                tx.rollback().await.map_err(DbError::from)?;
                stays += 1;
                continue;
            }
            let mut cut = importing
                .undo_cut(&mut tx, operation::Work::Node(*id))
                .await?;
            let principal = match &cut {
                Some(cut) => cut.principal().clone(),
                None => importing.principal_for(&mut tx).await?,
            };
            if delete_place(
                engine,
                &mut tx,
                &principal,
                Deleting {
                    id: *id,
                    name: name.as_deref().unwrap_or_default(),
                    now,
                    since,
                },
                || match &mut cut {
                    Some(cut) => cut.before(engine.inner.clock.as_ref()).map_err(source_stop),
                    None => Ok(()),
                },
            )
            .await?
            {
                if let Some(cut) = &cut {
                    cut.deadline(engine.inner.clock.as_ref())
                        .map_err(source_stop)?;
                }
                tx.commit().await.map_err(DbError::from)?;
                done += 1;
            } else {
                tx.rollback().await.map_err(DbError::from)?;
                stays += 1;
            }
            job.progress(
                (done as f64 / total as f64).min(1.0),
                Some(&format!("{done} of {total} things")),
            )
            .await?;
        }
    }
    // What the import added to chats and threads that were there: a chat or a thread at a time, and a batch of its rows at a time until
    // there is none, each batch in a transaction of its own and under what the person may do in that chat or thread now.
    for which in [Merged::Chat, Merged::Thread] {
        let mut after: Option<Id> = None;
        loop {
            job.check()?;
            let parents = {
                let mut conn = engine.inner.db.acquire().await?;
                merged_parents(&mut conn, import, which, after, NODES_PAGE).await?
            };
            let Some(last) = parents.last() else {
                break;
            };
            after = Some(*last);
            for parent in &parents {
                loop {
                    job.check()?;
                    let now = engine.inner.clock.now();
                    let mut tx = engine.inner.db.begin().await?;
                    // The last accepted batch can consume the final origin. That is the native end,
                    // rather than a missing future source to bind or an expired-key session fallback.
                    let present = match which {
                        Merged::Chat => {
                            rows::undo_subjects_present(&mut tx, import, &[], &[], &[], &[*parent])
                                .await?
                        }
                        Merged::Thread => {
                            rows::undo_subjects_present(&mut tx, import, &[*parent], &[], &[], &[])
                                .await?
                        }
                    };
                    if !present {
                        tx.rollback().await.map_err(DbError::from)?;
                        break;
                    }
                    let work = match which {
                        Merged::Chat => operation::Work::Chat(*parent),
                        Merged::Thread => operation::Work::Thread(*parent),
                    };
                    let mut cut = importing.undo_cut(&mut tx, work).await?;
                    let principal = match &cut {
                        Some(cut) => cut.principal().clone(),
                        None => importing.principal_for(&mut tx).await?,
                    };
                    let taken = super::make::take_back_before(
                        &mut tx,
                        import,
                        which,
                        *parent,
                        &principal,
                        now,
                        || match &mut cut {
                            Some(cut) => cut.before(engine.inner.clock.as_ref()).map_err(|error| {
                                crate::error::EngineError::unreadable(error.to_string()).into()
                            }),
                            None => Ok(()),
                        },
                    )
                    .await
                    .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
                    if let Some(cut) = &cut {
                        cut.deadline(engine.inner.clock.as_ref())
                            .map_err(source_stop)?;
                    }
                    tx.commit().await.map_err(DbError::from)?;
                    match taken {
                        Taken::Some(count) => {
                            done += usize::try_from(count).unwrap_or(0);
                            job.progress(
                                (done as f64 / total as f64).min(1.0),
                                Some(&format!("{done} of {total} things")),
                            )
                            .await?;
                        }
                        Taken::Nothing => break,
                        Taken::Refused => {
                            stays += 1;
                            break;
                        }
                    }
                }
            }
        }
    }
    {
        let mut tx = engine.inner.db.begin().await?;
        take_back_finish(&mut tx, import)
            .await
            .map_err(|error| Failure::new(INTERNAL, error.to_string()))?;
        tx.commit().await.map_err(DbError::from)?;
    }
    if stays == 0 {
        Ok(Done::Finished)
    } else {
        Ok(Done::Skipped(format!(
            "{stays} things stay: people added to them or changed them since, or you may not delete them"
        )))
    }
}

/// Which kind of node a package kind is, as the plan words it.
#[must_use]
pub fn node_of(kind: PackageKind) -> Option<&'static str> {
    node_word(kind)
}

/// The caller as the job holds them, for the person who asks to undo.
#[must_use]
pub fn undo_caller(caller: &Caller) -> Caller {
    caller.clone()
}
