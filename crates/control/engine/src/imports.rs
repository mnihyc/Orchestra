//! Importing into a place: what a person brings from an Orch package or from plain files, planned before anything is made, asked for as a job, kept as a report and taken back as a whole.
//!
//! **A plan, then a job.** [`Engine::plan_import`] opens what the person uploaded, reads what is here, and says what each object of
//! the source would become, who the people are, what does not come back and what stands in the way; it writes nothing.
//! [`Engine::make_import_in`] makes the row of the import and the job (`jobs::import`), which makes the same plan again as the
//! person who asked and brings the objects in batches that ask the access engine again for each kind they make. What is made is
//! made by the rights of the person at that moment; what they may not make is a row of the report and nothing of it is made.
//!
//! **Identity.** Everything an import makes has the identifier derived from who imports, where, which package and the identifier
//! it had ([`identity`]), so a retry makes nothing twice, and each thing is recorded in `import_origins` by the place imported into
//! and its source identifier, so that a later import of the same thing finds it.

mod identity;
pub mod make;
mod model;
mod open;
mod people;
pub mod plain;
mod plan;
pub mod records;
mod remap;
mod report;
mod source;
pub mod undo;

pub use model::{Destination, Existing, Object, Opened, Plan, Wants};
pub use open::{OpenError, open};
pub use people::{Credit, Credits, IMPORTED_PERSON, Matched, match_people};
pub use plan::{another_name, build, origin_kind};
pub use report::{
    Counts, csv_head, csv_of, csv_row, kind_word, outcome_word, row_of, stored as stored_row,
};
pub use source::{Entries, EntryFact, SourceError, Spooled, clean_path};

use crate::engine::Engine;
use crate::error::EngineError;
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::{NodePermission, PermissionName};
use orch_api_contract::imports::{
    Import, ImportActionCounts, ImportBlocker, ImportCounts, ImportExisting, ImportFailure,
    ImportLine, ImportLoss, ImportPlan, ImportPlanObject, ImportPlanPerson, ImportPreflightRequest,
    ImportRequest, ImportSourceFacts, ImportState,
};
use orch_api_contract::nodes::NodeRef;
use orch_api_contract::package::PackageHeader;
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::queries::imports::{self as rows, ImportRow};
use orch_db::queries::jobs::{self as job_rows, JobRow, NewJob, kind};
use orch_db::queries::{shares as places, storage};
use orch_db::{Conn, DbError, Tx};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

/// Why an import could not be planned, asked for, read or taken back.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// There is no such place or import, or the caller may not know of it.
    #[error("there is no such thing")]
    NotFound,
    /// The request does not fit.
    #[error("{field}: {message}")]
    Invalid {
        /// Where in the request.
        field: String,
        /// What is wrong.
        message: String,
    },
    /// What was uploaded is not a package or files that can be imported.
    #[error("{0}")]
    Source(String),
    /// The import cannot be made as it is asked for.
    #[error("the import cannot be made as asked: {message}")]
    Blocked {
        /// The code of the refusal.
        code: &'static str,
        /// The limit that was passed, where one was.
        limit: i64,
        /// What was asked.
        asked: i64,
        /// In a sentence.
        message: String,
    },
    /// The person has as many imports running as they may.
    #[error("as many imports are running as one person may have: {limit}")]
    TooManyAtOnce {
        /// How many.
        limit: u32,
    },
    /// The import was taken back already, or is being, or is not over.
    #[error("{0}")]
    Undone(String),
    /// The caller is suspended where this is.
    #[error("the caller is suspended")]
    Suspended {
        /// The org; none for altogether.
        org: Option<Id>,
    },
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<DbError> for ImportError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for ImportError {
    fn from(error: orch_authz::Error) -> Self {
        match error {
            orch_authz::Error::NotFound(_) => Self::NotFound,
            orch_authz::Error::Suspended { org } => Self::Suspended { org },
            other => Self::Internal(other.into()),
        }
    }
}

impl From<crate::exports::ExportError> for ImportError {
    fn from(error: crate::exports::ExportError) -> Self {
        match error {
            crate::exports::ExportError::NotFound => Self::NotFound,
            crate::exports::ExportError::Suspended { org } => Self::Suspended { org },
            crate::exports::ExportError::Internal(error) => Self::Internal(error),
            other => Self::Internal(EngineError::unreadable(other.to_string())),
        }
    }
}

impl From<OpenError> for ImportError {
    fn from(error: OpenError) -> Self {
        match error {
            OpenError::Invalid(message) => Self::Source(message),
            OpenError::Failed(message) => Self::Internal(EngineError::unreadable(message)),
        }
    }
}

/// How many objects of a plan one answer may hold when the view does not say: what a list of anything shows at a time.
const PAGE_DEFAULT: usize = 50;
/// The most objects of a plan one answer holds: what a list of anything pages by at the most.
const PAGE_MOST: usize = 200;

/// The response of a request that asks for an import: the row and the job.
#[derive(Debug, Clone)]
pub struct Made {
    /// The import.
    pub import: ImportRow,
    /// The job that brings it.
    pub job: JobRow,
}

/// An inspected request, kept apart from the transaction that records it under current action authority.
#[derive(Debug)]
pub struct PreparedImport {
    user: Id,
    node: Id,
    request: ImportRequest,
    package: Option<Json>,
}

fn time(at: DateTime<Utc>) -> Time {
    Time::new(at)
}

impl Engine {
    /// Whether the caller may view the place an import is asked of; it is not there for them otherwise.
    async fn import_place_seen(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        place: Id,
    ) -> Result<(), ImportError> {
        match nodes::check_named(
            conn,
            principal,
            place,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(orch_access_contract::decisions::Decision::Allowed { .. }) => Ok(()),
            Ok(orch_access_contract::decisions::Decision::Refused(_)) => Err(ImportError::NotFound),
            Err(orch_authz::Error::Archived { .. }) => Ok(()),
            Err(other) => Err(other.into()),
        }
    }

    /// Opens a request's source and makes the plan of the import into `node` as `principal`.
    pub async fn plan_of_import(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        node: Id,
        request: &ImportRequest,
    ) -> Result<Plan, ImportError> {
        let limits = self.import_limits(conn, node).await?;
        let opened = open(self, &request.source, &limits).await?;
        build(self, conn, principal, node, request, Arc::new(opened)).await
    }

    /// What an import into `node` would do as far as `caller` may bring it, and what stands in its way. Nothing is made.
    pub async fn plan_import(
        &self,
        caller: &Caller,
        node: Id,
        ask: &ImportPreflightRequest,
    ) -> Result<ImportPlan, ImportError> {
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        let principal = Principal::load(&mut conn, caller.clone(), Time::new(now)).await?;
        self.import_place_seen(&mut conn, &principal, node).await?;
        let plan = self
            .plan_of_import(&mut conn, &principal, node, &ask.request)
            .await?;
        self.plan_view(&mut conn, &plan, &ask.view).await
    }

    /// What stands in the way of an import as it was planned, in the order its refusal names it.
    #[must_use]
    pub fn import_blockers(plan: &Plan) -> Vec<(&'static str, i64, i64, String)> {
        let mut out = Vec::new();
        if plan.items == 0 {
            out.push((
                "import.nothing",
                0,
                0,
                "there is nothing in it that you may bring here".to_owned(),
            ));
            return out;
        }
        if plan.items > plan.limits.max_items {
            out.push((
                "import.too_many",
                plan.limits.max_items,
                plan.items,
                format!(
                    "it would hold {} things, and the place allows {}",
                    plan.items, plan.limits.max_items
                ),
            ));
        }
        if plan.opened.bytes > plan.limits.max_bytes {
            out.push((
                "import.too_large",
                plan.limits.max_bytes,
                plan.opened.bytes,
                format!(
                    "it comes to about {} bytes, and the place allows {}",
                    plan.opened.bytes, plan.limits.max_bytes
                ),
            ));
        }
        out
    }

    /// The plan as the API shows it: the part of the objects the view asks for, the people, the losses and the blockers.
    #[allow(
        clippy::too_many_lines,
        reason = "one reading of the plan into the shapes the screens take"
    )]
    pub async fn plan_view(
        &self,
        conn: &mut Conn,
        plan: &Plan,
        view: &orch_api_contract::imports::ImportView,
    ) -> Result<ImportPlan, ImportError> {
        let conflicts_only = view.conflicts_only.unwrap_or(false);
        let limit = usize::try_from(view.limit.unwrap_or(0))
            .ok()
            .filter(|each| *each > 0)
            .unwrap_or(PAGE_DEFAULT)
            .min(PAGE_MOST);
        let start = view
            .cursor
            .as_deref()
            .map(|cursor| cursor.parse::<usize>())
            .transpose()
            .map_err(|_| ImportError::Invalid {
                field: "view.cursor".to_owned(),
                message: "is not one".to_owned(),
            })?
            .unwrap_or(0);
        let shown: Vec<&Object> = plan
            .objects
            .iter()
            .filter(|each| view.kind.is_none_or(|kind| kind == each.object.kind))
            .filter(|each| !conflicts_only || each.existing.is_some() || each.blocked.is_some())
            .collect();
        let page: Vec<ImportPlanObject> = shown
            .iter()
            .skip(start)
            .take(limit)
            .map(|each| ImportPlanObject {
                source_id: each.object.id,
                kind: each.object.kind,
                name: each.object.name.clone(),
                parent: each.object.parent,
                path: each.path.clone(),
                bytes: each.object.bytes,
                action: each.action,
                allowed: each.allowed.clone(),
                existing: each.existing.as_ref().map(|found| ImportExisting {
                    id: found.id,
                    kind: each.object.kind,
                    name: found.name.clone(),
                    found_by: found.found_by,
                    updated_at: found.updated_at.map(time),
                }),
                blocked: each.blocked.clone(),
                creator: each.object.creator,
                created_at: each.object.created_at,
            })
            .collect();
        let next_cursor = (start + limit < shown.len()).then(|| (start + limit).to_string());

        // The people, with the accounts they are taken for as the screens name a person.
        let accounts: Vec<Id> = plan.people.iter().filter_map(|each| each.user).collect();
        let mut named: HashMap<Id, orch_api_contract::users::UserRef> = HashMap::new();
        for account in accounts {
            if let Ok(value) = crate::views::user_ref_json(conn, account).await
                && let Ok(user) = serde_json::from_value(value)
            {
                named.insert(account, user);
            }
        }
        let mut made_by: HashMap<Id, i64> = HashMap::new();
        for each in plan.objects.iter().filter(|each| each.goes()) {
            if let Some(who) = each.object.creator {
                *made_by.entry(who).or_default() += 1;
            }
        }
        let people: Vec<ImportPlanPerson> = plan
            .people
            .iter()
            .map(|each| ImportPlanPerson {
                principal: each.principal.id,
                username: each.principal.username.clone(),
                display_name: each.principal.display_name.clone(),
                matched: each.how,
                user: each.user.and_then(|account| named.get(&account).cloned()),
                objects: made_by.get(&each.principal.id).copied().unwrap_or(0),
            })
            .collect();

        let mut counts = ImportCounts::default();
        let mut actions = ImportActionCounts::default();
        let mut conflicts = 0_i64;
        for each in &plan.objects {
            if each.existing.is_some() {
                conflicts += 1;
            }
            match each.action {
                orch_api_contract::imports::ImportAction::Create => actions.create += 1,
                orch_api_contract::imports::ImportAction::KeepBoth => actions.keep_both += 1,
                orch_api_contract::imports::ImportAction::Replace => actions.replace += 1,
                orch_api_contract::imports::ImportAction::Skip => actions.skip += 1,
            }
            if !each.goes() {
                continue;
            }
            use orch_api_contract::package::PackageKind as K;
            match each.object.kind {
                K::Org | K::Project | K::Topic => counts.places += 1,
                K::Folder => counts.folders += 1,
                K::Document => {
                    counts.documents += 1;
                    counts.comments += each.object.comments.unwrap_or(0);
                    counts.versions += i64::try_from(each.object.versions.len()).unwrap_or(0);
                }
                K::Chat => {
                    counts.chats += 1;
                    counts.messages += each.object.messages.unwrap_or(0);
                }
                K::Thread => {
                    counts.threads += 1;
                    counts.turns += each.object.turns.unwrap_or(0);
                }
            }
            counts.files += i64::try_from(each.object.files.len()).unwrap_or(0);
        }
        let blockers: Vec<ImportBlocker> = Self::import_blockers(plan)
            .into_iter()
            .map(|(code, limit, asked, message)| ImportBlocker {
                code: code.to_owned(),
                message,
                params: Some(json!({ "limit": limit, "asked": asked })),
            })
            .collect();
        let place = places::places(conn, &[plan.destination.id])
            .await?
            .into_iter()
            .next()
            .ok_or(ImportError::NotFound)?;
        Ok(ImportPlan {
            node: NodeRef {
                thread_display: None,
                id: place.id,
                kind: crate::exports::node_kind(&place.kind),
                name: Some(place.name),
                redacted: false,
            },
            source: ImportSourceFacts {
                mode: plan.opened.mode,
                entries: plan.opened.entry_count,
                bytes: plan.opened.bytes,
                package: plan.opened.header.clone(),
            },
            counts,
            actions,
            conflicts,
            bytes: plan.bytes,
            limits: plan.limits,
            authors: plan.authors,
            access: plan.access,
            objects: orch_api_contract::common::Page {
                items: page,
                next_cursor,
            },
            people,
            losses: plan.losses.clone(),
            blockers,
        })
    }
}

// ---------------------------------------------------------------- making one

/// What a caller was reading with, kept with the job so that it reads the same way when it writes.
fn credential_of(caller: &Caller) -> Json {
    match caller.grant() {
        None => Json::Null,
        Some(grant) => json!({ "scopes": grant.scopes().to_strings(), "subtree": grant.subtree() }),
    }
}

/// The names of the files a request is made from, for the person's list.
fn file_names(request: &ImportRequest) -> Vec<String> {
    request
        .source
        .files
        .iter()
        .map(|file| file.name.clone())
        .collect()
}

impl Engine {
    /// Inspects the source before the action transaction, refusing an invalid source or a blocked plan without writing.
    pub async fn prepare_import(
        &self,
        caller: &Caller,
        node: Id,
        request: ImportRequest,
    ) -> Result<PreparedImport, ImportError> {
        let client = request.client_id.trim();
        if client.is_empty() || client.chars().count() > 200 {
            return Err(ImportError::Invalid {
                field: "client_id".to_owned(),
                message: "is one to two hundred characters".to_owned(),
            });
        }
        let now = self.inner.clock.now();
        let user = caller.user();
        // The plan is made outside the transaction that records it: opening an archive reads a good deal.
        let plan = {
            let mut conn = self.inner.db.acquire().await?;
            let principal = Principal::load(&mut conn, caller.clone(), Time::new(now)).await?;
            self.import_place_seen(&mut conn, &principal, node).await?;
            self.plan_of_import(&mut conn, &principal, node, &request)
                .await?
        };
        if let Some((code, limit, asked, message)) = Self::import_blockers(&plan).into_iter().next()
        {
            return Err(ImportError::Blocked {
                code,
                limit,
                asked,
                message,
            });
        }
        let package = plan
            .opened
            .header
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(EngineError::from)?;
        Ok(PreparedImport {
            user,
            node,
            request,
            package,
        })
    }

    /// Records an inspected import and its job in the caller's guarded action transaction. The caller commits and wakes jobs.
    pub async fn make_import_in(
        &self,
        tx: &mut Tx,
        caller: &Caller,
        prepared: PreparedImport,
    ) -> Result<Made, ImportError> {
        let PreparedImport {
            user,
            node,
            request,
            package: header,
        } = prepared;
        if user != caller.user() {
            return Err(ImportError::NotFound);
        }
        let now = self.inner.clock.now();
        let client = request.client_id.trim();
        let hashes: Vec<Hash> = request.source.files.iter().map(|file| file.blob).collect();
        rows::lock_person(&mut *tx, user).await?;
        if let Some(existing) = rows::by_client(&mut *tx, user, client).await? {
            let job = match existing.job_id {
                Some(job) => job_rows::job(&mut *tx, job).await?,
                None => None,
            };
            if let Some(job) = job {
                return Ok(Made {
                    import: existing,
                    job,
                });
            }
        }
        let running = rows::running(&mut *tx, user).await?;
        let most = i64::from(self.inner.config.imports_per_user);
        if running >= most {
            return Err(ImportError::TooManyAtOnce {
                limit: self.inner.config.imports_per_user,
            });
        }
        // The uploads stay until the import ends, whatever the sweep of unused uploads would have done.
        let pinned = storage::pin_blobs(&mut *tx, &hashes).await?;
        let distinct: std::collections::HashSet<Hash> = hashes.iter().copied().collect();
        if usize::try_from(pinned).unwrap_or(0) != distinct.len() {
            return Err(ImportError::Invalid {
                field: "source.files".to_owned(),
                message: "name an upload that is not there any more".to_owned(),
            });
        }
        let id = new_id();
        let stored = serde_json::to_value(&request).map_err(EngineError::from)?;
        let blobs: Vec<String> = distinct.iter().map(ToString::to_string).collect();
        let files = file_names(&request);
        let Some(import) = rows::insert(
            &mut *tx,
            &rows::NewImport {
                id,
                user,
                node,
                client_id: client,
                request: &stored,
                files: &files,
                source_blobs: &blobs,
                package: header.as_ref(),
                at: now,
            },
        )
        .await?
        else {
            return Err(ImportError::Undone(
                "this request was made already".to_owned(),
            ));
        };
        let progress = serde_json::to_value(crate::jobs::runner::initial_progress(
            &crate::jobs::import::PHASES,
        ))
        .map_err(EngineError::from)?;
        let job_plan = json!({
            "import": id,
            "user": user,
            "node": node,
            "credential": credential_of(caller),
            "undo": false,
        });
        let job = job_rows::insert(
            &mut *tx,
            &NewJob {
                id: new_id(),
                kind: kind::IMPORT,
                node_id: None,
                environment_id: None,
                provider_id: None,
                plan: &job_plan,
                progress: &progress,
                created_by: Some(user),
            },
            now,
        )
        .await?;
        rows::attach_job(&mut *tx, id, job.id).await?;
        let import = ImportRow {
            job_id: Some(job.id),
            ..import
        };
        Ok(Made { import, job })
    }
}

// ---------------------------------------------------------------- reading them

fn count_value<T: serde::de::DeserializeOwned>(raw: Option<&Json>) -> Option<T> {
    raw.and_then(|each| serde_json::from_value(each.clone()).ok())
}

fn state_of(word: &str) -> ImportState {
    match word {
        rows::state::DONE => ImportState::Done,
        rows::state::FAILED => ImportState::Failed,
        rows::state::CANCELLED => ImportState::Cancelled,
        rows::state::UNDOING => ImportState::Undoing,
        rows::state::UNDONE => ImportState::Undone,
        _ => ImportState::Pending,
    }
}

impl Engine {
    /// The imports as the API shows them, each with the place it went into by name where its person may still see it.
    pub async fn import_views(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        list: &[ImportRow],
    ) -> Result<Vec<Import>, ImportError> {
        let ids: Vec<Id> = list.iter().filter_map(|row| row.node_id).collect();
        let refs = place_refs(conn, principal, &ids).await?;
        let mut out = Vec::with_capacity(list.len());
        for row in list {
            let request: ImportRequest =
                serde_json::from_value(row.request.clone()).map_err(EngineError::from)?;
            out.push(Import {
                id: row.id,
                node: row.node_id.map(|node| refs.of(node)),
                request,
                state: state_of(&row.state),
                job_id: row.job_id,
                undo_job_id: row.undo_job_id,
                files: row.files.clone(),
                package: count_value::<PackageHeader>(row.package.as_ref()),
                counts: count_value::<ImportCounts>(row.counts.as_ref()),
                actions: count_value::<ImportActionCounts>(row.actions.as_ref()),
                losses: serde_json::from_value::<Vec<ImportLoss>>(row.losses.clone())
                    .unwrap_or_default(),
                failure: count_value::<ImportFailure>(row.failure.as_ref()),
                created_at: time(row.created_at),
                finished_at: row.finished_at.map(time),
                undone_at: row.undone_at.map(time),
            });
        }
        Ok(out)
    }

    /// An import of `user`, if it is theirs: another person's is none for them.
    pub async fn import_of(
        &self,
        conn: &mut Conn,
        user: Id,
        id: Id,
    ) -> Result<ImportRow, ImportError> {
        rows::get(conn, id)
            .await?
            .filter(|row| row.user_id == Some(user))
            .ok_or(ImportError::NotFound)
    }

    /// The imports of `user` as their list shows them, newest first: a page after the cursor of the last one shown.
    pub async fn import_lines_of(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        user: Id,
        after: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<ImportLine>, Option<String>), ImportError> {
        let after = match after {
            None => None,
            Some(cursor) => Some(read_cursor(cursor).ok_or_else(|| ImportError::Invalid {
                field: "cursor".to_owned(),
                message: "is not one".to_owned(),
            })?),
        };
        let mut list = rows::lines_of_user(conn, user, after, limit + 1).await?;
        let more = i64::try_from(list.len()).unwrap_or(i64::MAX) > limit;
        if more {
            list.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        let next = if more {
            list.last().map(|row| write_cursor(row.created_at, row.id))
        } else {
            None
        };
        let ids: Vec<Id> = list.iter().filter_map(|row| row.node_id).collect();
        let refs = place_refs(conn, principal, &ids).await?;
        let lines = list
            .into_iter()
            .map(|row| ImportLine {
                id: row.id,
                node: row.node_id.map(|node| refs.of(node)),
                state: state_of(&row.state),
                job_id: row.job_id,
                undo_job_id: row.undo_job_id,
                first_file: row.first_file,
                file_count: row.file_count,
                actions: count_value::<ImportActionCounts>(row.actions.as_ref()),
                created_at: time(row.created_at),
                finished_at: row.finished_at.map(time),
                undone_at: row.undone_at.map(time),
            })
            .collect();
        Ok((lines, next))
    }

    /// The names of the files an import of `user` was made from, in their order: a page after the cursor of the
    /// last one shown. Another person's import is none for them.
    pub async fn import_file_names(
        &self,
        conn: &mut Conn,
        user: Id,
        id: Id,
        after: Option<&str>,
        limit: i64,
    ) -> Result<(Vec<String>, Option<String>), ImportError> {
        let after = match after {
            None => 0,
            Some(cursor) => cursor
                .parse::<i64>()
                .ok()
                .filter(|place| *place >= 0)
                .ok_or_else(|| ImportError::Invalid {
                    field: "cursor".to_owned(),
                    message: "is not one".to_owned(),
                })?,
        };
        let mut names = rows::file_names(conn, user, id, after, limit + 1)
            .await?
            .ok_or(ImportError::NotFound)?;
        let more = i64::try_from(names.len()).unwrap_or(i64::MAX) > limit;
        if more {
            names.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        let next = if more {
            names.last().map(|(place, _)| place.to_string())
        } else {
            None
        };
        Ok((names.into_iter().map(|(_, name)| name).collect(), next))
    }
}

/// The places imports went into, as the person who asks may see them: a place they may not view is named by its kind
/// alone.
struct PlaceRefs {
    kinds: HashMap<Id, String>,
    viewable: HashMap<Id, String>,
}

impl PlaceRefs {
    fn of(&self, node: Id) -> NodeRef {
        NodeRef {
            thread_display: None,
            id: node,
            kind: crate::exports::node_kind(self.kinds.get(&node).map_or("org", String::as_str)),
            name: self.viewable.get(&node).cloned(),
            redacted: !self.viewable.contains_key(&node),
        }
    }
}

async fn place_refs(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> Result<PlaceRefs, ImportError> {
    if ids.is_empty() {
        return Ok(PlaceRefs {
            kinds: HashMap::new(),
            viewable: HashMap::new(),
        });
    }
    let found = places::places(conn, ids).await?;
    let viewable: HashMap<Id, String> = nodes::holding(
        conn,
        principal,
        ids,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await?
    .into_iter()
    .map(|listed| (listed.id, listed.name))
    .collect();
    Ok(PlaceRefs {
        kinds: found.into_iter().map(|node| (node.id, node.kind)).collect(),
        viewable,
    })
}

fn write_cursor(at: DateTime<Utc>, id: Id) -> String {
    format!("{}:{}", at.timestamp_micros(), id)
}

fn read_cursor(text: &str) -> Option<(DateTime<Utc>, Id)> {
    let (micros, id) = text.split_once(':')?;
    Some((
        DateTime::from_timestamp_micros(micros.parse().ok()?)?,
        id.parse().ok()?,
    ))
}

// ---------------------------------------------------------------- looking at one item

impl Engine {
    /// An item of the source as its text reads, for the review: its reading copy where a package has one, the text of a plain
    /// file, or what a place says of itself. At most [`orch_api_contract::imports::IMPORT_PREVIEW_MAX_BYTES`] bytes of it.
    pub async fn preview_import(
        &self,
        caller: &Caller,
        node: Id,
        ask: &orch_api_contract::imports::ImportPreviewRequest,
    ) -> Result<orch_api_contract::imports::ImportPreview, ImportError> {
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        let principal = Principal::load(&mut conn, caller.clone(), Time::new(now)).await?;
        self.import_place_seen(&mut conn, &principal, node).await?;
        let limits = self.import_limits(&mut conn, node).await?;
        drop(conn);
        let opened = open(self, &ask.source, &limits).await?;
        let Some(object) = opened.objects.iter().find(|each| each.id == ask.object) else {
            return Err(ImportError::NotFound);
        };
        let most = u64::try_from(orch_api_contract::imports::IMPORT_PREVIEW_MAX_BYTES).unwrap_or(0);
        // The reading copy, or the plain file itself, or what a place says of itself.
        let entry = object
            .renderings
            .first()
            .map(|reading| reading.path.clone())
            .or_else(|| {
                opened
                    .plain
                    .get(&object.id)
                    .map(|plain| plain.entry.clone())
            });
        let (text, truncated) = match entry {
            Some(path) => {
                let size = opened
                    .entries
                    .facts()
                    .iter()
                    .find(|fact| fact.name == path)
                    .map_or(0, |fact| fact.size);
                // One byte more than is shown says that there was more.
                let bytes = match opened
                    .entries
                    .read(&path, size.clamp(1, RECORD_PREVIEW_LIMIT))
                    .await
                {
                    Ok(bytes) => bytes,
                    Err(SourceError::TooLarge { .. }) => opened
                        .entries
                        .read(&path, RECORD_PREVIEW_LIMIT)
                        .await
                        .unwrap_or_default(),
                    Err(error) => return Err(ImportError::Source(error.to_string())),
                };
                let cut = usize::try_from(most).unwrap_or(usize::MAX);
                let truncated = bytes.len() > cut || size > most;
                let mut end = bytes.len().min(cut);
                let text = loop {
                    if let Ok(text) = std::str::from_utf8(&bytes[..end]) {
                        break text.to_owned();
                    }
                    if end == 0 {
                        break String::new();
                    }
                    end -= 1;
                };
                (text, truncated)
            }
            None => (object.description.clone(), false),
        };
        Ok(orch_api_contract::imports::ImportPreview {
            kind: object.kind,
            name: object.name.clone(),
            text,
            truncated,
        })
    }
}

/// How much of a reading copy is read to show a preview of it: far more than is shown, so that a long text is cut where it is,
/// and short enough to come back at once.
const RECORD_PREVIEW_LIMIT: u64 = 1024 * 1024;

// ---------------------------------------------------------------- taking one back

impl Engine {
    /// Records the Undo job in the caller's guarded transaction; the caller commits and wakes jobs.
    pub async fn make_undo_in(
        &self,
        tx: &mut Tx,
        caller: &Caller,
        import: Id,
    ) -> Result<Made, ImportError> {
        let user = caller.user();
        let now = self.inner.clock.now();
        rows::lock_person(&mut *tx, user).await?;
        let row = rows::lock(&mut *tx, import)
            .await?
            .filter(|row| row.user_id == Some(user))
            .ok_or(ImportError::NotFound)?;
        if row.state == rows::state::UNDOING
            && let Some(job) = row.undo_job_id
            && let Some(job) = job_rows::job(&mut *tx, job).await?
        {
            return Ok(Made { import: row, job });
        }
        if !matches!(
            row.state.as_str(),
            rows::state::DONE | rows::state::FAILED | rows::state::CANCELLED
        ) {
            return Err(ImportError::Undone(
                "the import was taken back already, or is not over".to_owned(),
            ));
        }
        let node = row.node_id.ok_or(ImportError::NotFound)?;
        let progress = serde_json::to_value(crate::jobs::runner::initial_progress(
            &crate::jobs::import::UNDO_PHASES,
        ))
        .map_err(EngineError::from)?;
        let plan = json!({
            "import": import,
            "user": user,
            "node": node,
            "credential": credential_of(caller),
            "undo": true,
            "undo_retention_secs": self.inner.config.soft_delete.as_secs(),
        });
        let job = job_rows::insert(
            &mut *tx,
            &NewJob {
                id: new_id(),
                kind: kind::IMPORT,
                node_id: None,
                environment_id: None,
                provider_id: None,
                plan: &plan,
                progress: &progress,
                created_by: Some(user),
            },
            now,
        )
        .await?;
        rows::begin_undo(&mut *tx, import, job.id).await?;
        let row = ImportRow {
            undo_job_id: Some(job.id),
            state: rows::state::UNDOING.to_owned(),
            ..row
        };
        Ok(Made { import: row, job })
    }
}

// ---------------------------------------------------------------- the report

/// How many rows of a report one fetch of the whole report reads at a time.
const REPORT_PAGE: i64 = 1_000;

impl Engine {
    /// A page of the report of an import of `user`, in the order it was written: the rows that came to what the query asks.
    pub async fn report_page(
        &self,
        conn: &mut Conn,
        user: Id,
        import: Id,
        query: &orch_api_contract::imports::ImportReportListQuery,
    ) -> Result<
        (
            Vec<orch_api_contract::imports::ImportReportRow>,
            Option<String>,
        ),
        ImportError,
    > {
        self.import_of(conn, user, import).await?;
        let limit = i64::from(
            orch_api_contract::common::PageQuery {
                limit: query.limit,
                cursor: None,
            }
            .limit(),
        );
        let after = query
            .cursor
            .as_deref()
            .map(str::parse::<i64>)
            .transpose()
            .map_err(|_| ImportError::Invalid {
                field: "cursor".to_owned(),
                message: "is not one".to_owned(),
            })?;
        let outcome = query.outcome.map(outcome_word);
        let kind = query.kind.map(kind_word);
        let filter = rows::ReportFilter {
            outcome,
            kind,
            warned: query.warned.unwrap_or(false),
        };
        let mut found = rows::report_rows(conn, import, &filter, after, limit + 1).await?;
        let more = i64::try_from(found.len()).unwrap_or(i64::MAX) > limit;
        if more {
            found.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        }
        let next = if more {
            found.last().map(|(position, _)| position.to_string())
        } else {
            None
        };
        let items = found
            .into_iter()
            .filter_map(|(_, row)| serde_json::from_value(row).ok())
            .collect();
        Ok((items, next))
    }

    /// The whole report of one of the caller's imports as a file, written as it is read: the pieces come a page of the rows at a time, so
    /// that a report of a million rows costs a page of memory. Returns the type of the file, its name and the pieces.
    pub async fn report_stream(
        &self,
        conn: &mut Conn,
        user: Id,
        import: Id,
        format: orch_api_contract::imports::ImportReportFormat,
    ) -> Result<
        (
            &'static str,
            String,
            futures::stream::BoxStream<'static, Result<bytes::Bytes, std::io::Error>>,
        ),
        ImportError,
    > {
        use futures::StreamExt as _;
        use orch_api_contract::imports::ImportReportFormat as Format;
        enum Piece {
            Head(String),
            Rows { after: Option<i64>, first: bool },
            Done,
        }
        let row = self.import_of(conn, user, import).await?;
        let day = row.finished_at.unwrap_or(row.created_at).format("%Y-%m-%d");
        let (media_type, name, head) = match format {
            Format::Csv => (
                "text/csv; charset=utf-8",
                format!("import-report-{day}.csv"),
                csv_head().to_owned(),
            ),
            Format::Json => {
                let principal = Principal::load(
                    conn,
                    Caller::session(user),
                    Time::new(self.inner.clock.now()),
                )
                .await?;
                let views = self
                    .import_views(conn, &principal, std::slice::from_ref(&row))
                    .await?;
                let head = serde_json::to_string(&json!({ "import": views.first() }))
                    .map_err(EngineError::from)?;
                // `{"import":…}` opened again for the rows: the closing brace is taken off and the member is added.
                let head = format!("{},\"rows\":[", head.strip_suffix('}').unwrap_or(&head));
                (
                    "application/json",
                    format!("import-report-{day}.json"),
                    head,
                )
            }
        };
        let engine = self.clone();
        let stream = futures::stream::unfold(Piece::Head(head), move |piece| {
            let engine = engine.clone();
            async move {
                let broken = |error: String| Some((Err(std::io::Error::other(error)), Piece::Done));
                match piece {
                    Piece::Head(text) => Some((
                        Ok(bytes::Bytes::from(text)),
                        Piece::Rows {
                            after: None,
                            first: true,
                        },
                    )),
                    Piece::Rows { after, first } => {
                        let mut conn = match engine.inner.db.acquire().await {
                            Ok(conn) => conn,
                            Err(error) => return broken(error.to_string()),
                        };
                        let page = match rows::report_rows(
                            &mut conn,
                            import,
                            &rows::ReportFilter::default(),
                            after,
                            REPORT_PAGE,
                        )
                        .await
                        {
                            Ok(page) => page,
                            Err(error) => return broken(error.to_string()),
                        };
                        drop(conn);
                        let Some((last, _)) = page.last() else {
                            return match format {
                                Format::Json => {
                                    Some((Ok(bytes::Bytes::from_static(b"]}")), Piece::Done))
                                }
                                Format::Csv => None,
                            };
                        };
                        let last = *last;
                        let mut out = String::new();
                        let mut first = first;
                        for (_, row) in page {
                            let Ok(row) = serde_json::from_value::<
                                orch_api_contract::imports::ImportReportRow,
                            >(row) else {
                                continue;
                            };
                            match format {
                                Format::Json => {
                                    if !first {
                                        out.push(',');
                                    }
                                    first = false;
                                    out.push_str(&serde_json::to_string(&row).unwrap_or_default());
                                }
                                Format::Csv => out.push_str(&csv_row(&row)),
                            }
                        }
                        Some((
                            Ok(bytes::Bytes::from(out)),
                            Piece::Rows {
                                after: Some(last),
                                first,
                            },
                        ))
                    }
                    Piece::Done => None,
                }
            }
        })
        .boxed();
        Ok((media_type, name, stream))
    }
}
