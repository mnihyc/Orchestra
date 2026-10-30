//! Skills: made from an uploaded archive, an environment's directory or a draft begun in Orch; read with their versions and the files of each; switched on and off and deleted; given further versions by upload, by reading their directory again, or by publishing the draft their managers edit.
//!
//! A version is made in two steps so that no transaction waits on the store or on a machine: the
//! caller's right to make it and its source are checked, the files are read, checked and kept as an
//! archive outside any transaction, and the version is then recorded, with the caller's right asked
//! again, in the transaction that writes it.

mod source_url;

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{invalid, placed, policy_at, view as node_view};
use crate::routes::resources::providers::{home_policies, refused_making};
use crate::routes::resources::view::{Holding, accesses, addresses};
use crate::routes::resources::{Home, begin_mutation, record, require};
use crate::routes::workspace::live;
use crate::routes::workspace::paths;
use crate::routes::workspace::problems::Doing;
use axum::body::Body;
use axum::response::Response;
use http::{StatusCode, header};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::common::{ErrorCode, Listed, Page};
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::skills::{
    self as api, Skill, SkillArchive, SkillDraft, SkillDraftFile, SkillDraftInfo, SkillFile,
    SkillOrigin, SkillReimportReply, SkillSource, SkillVersion, SkillVersionInfo,
};
use orch_api_contract::workspace::DirEntryKind;
use orch_authz::ownership::{self, Making, NewAccess};
use orch_authz::{Policy, Principal, resources};
use orch_db::queries::resources::skills::{self as rows, DraftFile, SkillRow, VersionRow};
use orch_db::queries::settings::SecretName;
use orch_db::queries::storage;
use orch_db::{Conn, DbError};
use orch_engine::skills::archive::{self, Limits};
use orch_engine::skills::archive::{File, SKILL_FILE, SkillFault};
use orch_engine::skills::front_matter;
use orch_engine::skills::versions::{self, Made, Prepared, SkillError, Target};
use orch_link_contract::commands::{
    FsFetch, FsFetchResult, FsList, FsListPage, NodeSkillFetch, NodeSkillFetchResult, Secret,
    SecretOrText, SerialKey, SkillFetchSource,
};
use orch_types::{Hash, Id, Time};
use serde_json::json;
use std::collections::HashMap;

/// The bits of a file's mode that make it executable by anybody.
const EXECUTE_BITS: u32 = 0o111;

/// How many versions a page lists when the caller does not say, and at most.
const VERSIONS_PAGE_DEFAULT: u32 = 50;
const VERSIONS_PAGE_MAX: u32 = 200;

/// A prepared version and the sealed source it will make current if the write commits.
struct ReadySource {
    prepared: Prepared,
    origin: SkillOrigin,
    source_secret: Option<Id>,
    staged: Option<SecretName>,
}

impl ReadySource {
    fn plain(prepared: Prepared, origin: SkillOrigin) -> Self {
        Self {
            prepared,
            origin,
            source_secret: None,
            staged: None,
        }
    }
}

/// A failed write leaves no source secret merely staged for its command.
async fn abandon_source(ctx: &Ctx, skill: Id, staged: Option<&SecretName>) {
    let Some(staged) = staged else { return };
    let result = async {
        let mut conn = ctx.app.db.acquire().await?;
        source_url::abandon(&mut conn, skill, staged).await
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(%skill, %error, "a failed skill source could not be forgotten");
    }
}

/// Binds the skills' requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::SkillList, list)
        .on(api::SkillCreate, create)
        .on(api::SkillGet, get)
        .on(api::SkillUpdate, update)
        .on(api::SkillDelete, delete)
        .on(api::SkillVersionList, version_list)
        .on(api::SkillVersionGet, version_get)
        .on_raw(api::SkillVersionFileGet, version_file)
        .on(api::SkillVersionCreate, version_create)
        .on(api::SkillReimport, reimport)
        .on(api::SkillDraftGet, draft_get)
        .on(api::SkillDraftCreate, draft_create)
        .on(api::SkillDraftDelete, draft_delete)
        .on_raw(api::SkillDraftFileGet, draft_file_get)
        .on(api::SkillDraftFilePut, draft_file_put)
        .on(api::SkillDraftFileDelete, draft_file_delete)
        .on(api::SkillDraftPublish, draft_publish);
}

/// Where a skill lies, as the audit row of an act on it needs it.
fn home(row: &SkillRow) -> Home {
    Home {
        node: Some(row.home_id),
        org: Some(row.org_id),
    }
}

/// A version as the wire has it, without its files.
fn version_info(
    row: &VersionRow,
    creators: &HashMap<Id, orch_api_contract::users::UserRef>,
) -> ApiResult<SkillVersionInfo> {
    Ok(SkillVersionInfo {
        id: row.id,
        number: i64::from(row.number),
        name: row.name.clone(),
        description: row.description.clone(),
        size: row.size,
        file_count: row.file_count(),
        archive: SkillArchive {
            blob: row.archive_hash()?,
            size: row.archive_size,
        },
        origin: source_url::public_origin(
            serde_json::from_value(row.origin.clone()).map_err(ApiError::internal)?,
        ),
        created_by: row.created_by.and_then(|id| creators.get(&id).cloned()),
        created_at: Time::new(row.created_at),
    })
}

/// The files a version's manifest lists.
fn files_of(row: &VersionRow) -> ApiResult<Vec<SkillFile>> {
    serde_json::from_value(row.files.clone()).map_err(ApiError::internal)
}

/// The skills that the caller holds what `holdings` say on, as the wire has them: the same few
/// statements for any number of them.
pub(crate) async fn views(
    conn: &mut Conn,
    principal: &Principal,
    holdings: &[Holding<'_>],
) -> ApiResult<Vec<Skill>> {
    let ids: Vec<Id> = holdings.iter().map(|holding| holding.id).collect();
    let skills = rows::skills(conn, &ids).await?;
    let latest: Vec<Id> = skills
        .iter()
        .filter_map(|skill| skill.latest_version_id)
        .collect();
    let latest: HashMap<Id, VersionRow> = rows::versions_by_id(conn, &latest)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let counts: HashMap<Id, i64> = rows::version_counts(conn, &ids)
        .await?
        .into_iter()
        .collect();
    let managed: Vec<Id> = holdings
        .iter()
        .filter(|holding| {
            holding
                .held
                .iter()
                .any(|held| held.permission == Permission::SkillManage)
        })
        .map(|holding| holding.id)
        .collect();
    let drafts: HashMap<Id, rows::DraftInfoRow> = rows::draft_infos(conn, &managed)
        .await?
        .into_iter()
        .map(|row| (row.skill_id, row))
        .collect();
    let mut people: Vec<Id> = skills.iter().map(|skill| skill.owner_id).collect();
    people.extend(latest.values().filter_map(|row| row.created_by));
    people.extend(drafts.values().map(|row| row.updated_by));
    let people = refusals::user_refs(conn, &people).await?;
    let homes = node_view::refs(
        conn,
        principal,
        &skills.iter().map(|skill| skill.home_id).collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let addresses = addresses(conn, &ids).await?;
    let access = accesses(conn, principal, holdings).await?;
    let enabled_places: HashMap<Id, i64> = sqlx::query_as::<_, (Id, i64)>(
        "SELECT wanted.id, count(s.node_id) FROM unnest($1::uuid[]) wanted(id)
         LEFT JOIN settings s ON s.key='agents.skills' AND s.node_id IN (SELECT id FROM nodes WHERE deleted_at IS NULL)
           AND jsonb_typeof(s.value)='object' AND s.value ? wanted.id::text AND s.value->>wanted.id::text <> 'off'
         GROUP BY wanted.id",
    ).bind(&ids).fetch_all(&mut *conn).await.map_err(DbError::from)?.into_iter().collect();
    skills
        .into_iter()
        .map(|skill| {
            let latest = skill
                .latest_version_id
                .and_then(|id| latest.get(&id))
                .map(|row| version_info(row, &people))
                .transpose()?;
            let origin: Option<SkillOrigin> = skill
                .origin
                .clone()
                .map(serde_json::from_value)
                .transpose()
                .map_err(ApiError::internal)?;
            let origin = origin.map(source_url::public_origin);
            let home = homes.get(&skill.home_id).cloned().ok_or_else(|| {
                ApiError::internal("the home of a skill the caller may see was not read")
            })?;
            Ok(Skill {
                enabled_places: Some(if skill.enabled {
                    enabled_places.get(&skill.id).copied().unwrap_or_default()
                } else {
                    0
                }),
                id: skill.id,
                name: skill.name,
                description: skill.description,
                org_id: skill.org_id,
                owner: people.get(&skill.owner_id).cloned(),
                home,
                address: addresses.get(&skill.id).cloned().unwrap_or_default(),
                enabled: skill.enabled,
                latest,
                versions: counts.get(&skill.id).copied().unwrap_or_default(),
                origin,
                draft: drafts.get(&skill.id).map(|row| SkillDraftInfo {
                    based_on: row.based_on,
                    file_count: row.file_count,
                    revision: row.revision,
                    updated_by: people.get(&row.updated_by).cloned(),
                    updated_at: Time::new(row.updated_at),
                }),
                revision: skill.revision,
                access: access.get(&skill.id).cloned().ok_or_else(|| {
                    ApiError::internal("the caller's access to a skill was not read")
                })?,
            })
        })
        .collect()
}

/// The skills a list shows, as [`views`] makes them.
pub(crate) async fn inventory(
    conn: &mut Conn,
    principal: &Principal,
    seen: &[resources::Seen],
) -> ApiResult<Vec<Skill>> {
    let holdings: Vec<Holding<'_>> = seen
        .iter()
        .map(|each| Holding {
            admin_only: &each.admin_only,
            admin_access: each.admin_access,
            admin_place: each.admin_place,
            participating: each.participating,
            id: each.resource.id,
            owner: each.owner,
            administers: each.administers,
            held: &each.held,
        })
        .collect();
    views(conn, principal, &holdings).await
}

/// One skill as the caller may read it, with what they hold on it.
pub(crate) async fn show(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<Skill> {
    require(
        conn,
        principal,
        ResourceKind::Skill,
        id,
        Permission::SkillView,
    )
    .await?;
    source_url::migrate_many(conn, &[id], principal.now().get()).await?;
    let found = resources::effective_among(conn, principal, &[id], Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let holdings: Vec<Holding<'_>> = found
        .iter()
        .map(|each| Holding {
            admin_only: &each.admin_only,
            admin_access: each.admin_access,
            admin_place: each.admin_place,
            participating: each.participating,
            id: each.object.id,
            owner: each.owner,
            administers: each.administers,
            held: &each.held,
        })
        .collect();
    views(conn, principal, &holdings)
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)
}

async fn list(ctx: Ctx, query: api::SkillListQuery, (): ()) -> ApiResult<Vec<Listed<Skill>>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = resources::visible(
            &mut conn,
            &principal,
            ResourceKind::Skill,
            orch_authz::nodes::Page {
                after,
                limit: super::LIST_LIMIT,
            },
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        seen.extend(page.items);
        after = page.next;
        if after.is_none() {
            break;
        }
    }
    if let Some(org) = query.org_id {
        let homes: Vec<_> = seen.iter().filter_map(|each| each.resource.home).collect();
        let inside =
            orch_db::queries::resources::routes::homes_under(&mut conn, org, &homes).await?;
        seen.retain(|each| {
            each.resource
                .home
                .is_some_and(|home| inside.contains(&home))
        });
    }
    let private = super::hidden_items(&mut conn, &principal, &mut seen).await?;
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    source_url::migrate_many(&mut conn, &ids, ctx.now).await?;
    Ok(super::listed(
        inventory(&mut conn, &principal, &seen).await?,
        private,
    ))
}

async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Skill> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    show(&mut conn, &principal, id).await
}

/// The answer to a version that was not made.
fn refused(path: &str, error: SkillError) -> ApiError {
    match error {
        // What the refusal is about (a file's path, a member, a name) goes beside the words, which
        // quote nothing written.
        SkillError::Refused(fault) => {
            let about = fault.about();
            let refusal = refused_value(path, &fault.to_string());
            if about.is_empty() {
                refusal
            } else {
                refusal.with("about", about)
            }
        }
        error @ SkillError::NameChanged { .. } => refused_value(path, &error.to_string()),
        SkillError::NameTaken { name } => ApiError::new(
            ErrorCode::ConflictExists,
            format!("another skill here is named {name:?} already"),
        )
        .with("name", name),
        SkillError::Gone => ApiError::not_found(),
        SkillError::Store(cause) => ApiError::internal(cause),
        SkillError::Engine(error) => ApiError::internal(error),
    }
}

/// A value that cannot be kept, with the reason at its place in the body.
fn refused_value(path: &str, message: &str) -> ApiError {
    ApiError::new(ErrorCode::RequestValueRefused, format!("{path}: {message}")).with(
        "errors",
        [orch_api_contract::common::FieldFault {
            path: path.to_owned(),
            message: message.to_owned(),
        }],
    )
}

/// Reads the uploaded archive `blob`, which the caller must have uploaded, and keeps the version it
/// makes. No transaction is open meanwhile.
async fn prepare_upload(
    ctx: &Ctx,
    caller: Id,
    blob: &orch_types::Hash,
    folder: Option<String>,
    limits: Limits,
) -> ApiResult<Prepared> {
    {
        let mut conn = ctx.app.db.acquire().await?;
        if !orch_db::queries::uploads::owns(&mut conn, caller, blob).await? {
            return Err(invalid(
                "from.blob",
                "this caller has no completed upload of that archive",
            ));
        }
    }
    let limit = i64::try_from(limits.archive_bytes()).unwrap_or(i64::MAX);
    let bytes = ctx
        .app
        .blobs
        .read_whole(blob, limit)
        .await
        .map_err(|error| match error {
            crate::blobs::BlobsError::TooLarge { .. } => refused_value(
                "from.blob",
                &format!(
                    "the archive is larger than a skill may be here ({} bytes of files)",
                    limits.max_bytes
                ),
            ),
            other => ApiError::internal(other),
        })?;
    let store = ctx.app.blobs.store();
    versions::prepare_archive(&store, bytes, folder, limits)
        .await
        .map_err(|error| refused("from", error))
}

/// Makes a version of the source `source` ready, outside any transaction, with the origin it is
/// recorded with. A new skill's draft makes no version.
async fn prepare(
    ctx: &Ctx,
    principal: &Principal,
    skill: Id,
    source: &SkillSource,
    limits: Limits,
    existing_secret: Option<Id>,
) -> ApiResult<ReadySource> {
    match source {
        SkillSource::Upload { blob, name, folder } => Ok(ReadySource::plain(
            prepare_upload(ctx, principal.user(), blob, folder.clone(), limits).await?,
            SkillOrigin::Upload {
                name: name.clone(),
                folder: folder.clone(),
            },
        )),
        SkillSource::Environment {
            environment_id,
            path,
        } => {
            let path = paths::clean("from.path", path, true)?;
            let files = read_directory(ctx, principal, *environment_id, &path, limits).await?;
            let store = ctx.app.blobs.store();
            let prepared = versions::prepare_files(&store, files, limits)
                .await
                .map_err(|error| refused("from", error))?;
            Ok(ReadySource::plain(
                prepared,
                SkillOrigin::Environment {
                    environment_id: *environment_id,
                    path,
                },
            ))
        }
        SkillSource::Git {
            host_id,
            repository,
            reference,
            folder,
        } => {
            if !orch_settings_contract::registry::is_repository_address(repository) {
                return Err(invalid(
                    "from.repository",
                    "a repository is an https, http or git address without a password",
                ));
            }
            let folder = folder
                .as_deref()
                .map(str::trim)
                .filter(|folder| !folder.is_empty())
                .map(|folder| paths::clean("from.folder", folder, false))
                .transpose()?;
            let (bytes, commit) = fetch_on_host(
                ctx,
                principal,
                *host_id,
                SkillFetchSource::Git {
                    repository: repository.clone(),
                    reference: reference.trim().to_owned(),
                    folder: folder.clone(),
                },
                limits,
            )
            .await?;
            let store = ctx.app.blobs.store();
            // Git wrote the folder alone: the archive's top is the skill's.
            let prepared = versions::prepare_archive(&store, bytes, None, limits)
                .await
                .map_err(|error| refused("from", error))?;
            Ok(ReadySource::plain(
                prepared,
                SkillOrigin::Git {
                    repository: repository.clone(),
                    reference: reference.trim().to_owned(),
                    commit: commit.unwrap_or_default(),
                    folder,
                    host_id: *host_id,
                },
            ))
        }
        SkillSource::Url {
            host_id,
            url,
            folder,
        } => {
            let folder = folder
                .as_deref()
                .map(str::trim)
                .filter(|folder| !folder.is_empty())
                .map(str::to_owned);
            let (shown, staged) = if let Some(secret) = existing_secret {
                let mut conn = ctx.app.db.acquire().await?;
                let belongs: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM secrets WHERE id=$1 AND owner_kind='skill' AND owner_id=$2)",
                )
                .bind(secret)
                .bind(skill)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
                if !belongs {
                    return Err(ApiError::internal("the skill's source secret is missing"));
                }
                (source_url::public_url(url).0, None)
            } else {
                let mut conn = ctx.app.db.acquire().await?;
                source_url::stage(&mut conn, skill, url, principal.user(), ctx.now).await?
            };
            let source_secret = existing_secret.or_else(|| staged.as_ref().map(|secret| secret.id));
            let wire_url = match source_secret {
                Some(secret) => SecretOrText::Secret(Secret::Ref(secret)),
                None => SecretOrText::Text(url.clone()),
            };
            let result = async {
                let (bytes, _) = fetch_on_host(
                    ctx,
                    principal,
                    *host_id,
                    SkillFetchSource::Url { url: wire_url },
                    limits,
                )
                .await?;
                let store = ctx.app.blobs.store();
                versions::prepare_archive(&store, bytes, folder.clone(), limits)
                    .await
                    .map_err(|error| refused("from", error))
            }
            .await;
            match result {
                Ok(prepared) => Ok(ReadySource {
                    prepared,
                    origin: SkillOrigin::Url {
                        url: shown,
                        folder,
                        host_id: *host_id,
                    },
                    source_secret,
                    staged,
                }),
                Err(error) => {
                    abandon_source(ctx, skill, staged.as_ref()).await;
                    Err(error)
                }
            }
        }
        SkillSource::New { .. } => Err(invalid(
            "from.kind",
            "a new skill is begun with a draft; a version is made from an archive, a directory, a repository or an address",
        )),
    }
}

/// Has the host `host`, which the caller may use, fetch a skill's files and put them up as one
/// archive: the server fetches no address itself. The archive's bytes are read back, with the
/// commit a repository's reference came to.
async fn fetch_on_host(
    ctx: &Ctx,
    principal: &Principal,
    host: Id,
    source: SkillFetchSource,
    limits: Limits,
) -> ApiResult<(bytes::Bytes, Option<String>)> {
    let (machine, allowed_private_hosts) = {
        let mut conn = ctx.app.db.acquire().await?;
        require(
            &mut conn,
            principal,
            ResourceKind::Host,
            host,
            Permission::HostUse,
        )
        .await?;
        let machine = orch_db::queries::resources::hosting::host(&mut conn, host)
            .await?
            .ok_or_else(ApiError::not_found)?
            .machine_id;
        let allowed_private_hosts = orch_worker_core::channels::outbound_hosts(&mut conn)
            .await
            .map_err(ApiError::internal)?;
        (machine, allowed_private_hosts)
    };
    let limit = i64::try_from(limits.archive_bytes()).unwrap_or(i64::MAX);
    let fetched: NodeSkillFetchResult = ctx
        .app
        .link
        .dispatcher()
        .run(
            machine,
            &SerialKey::None,
            &NodeSkillFetch {
                source,
                max_bytes: limit,
                allowed_private_hosts,
            },
        )
        .await
        .map_err(|error| match live::command_failure(error) {
            // What the host met (Git's or the address's own words) goes beside the sentence.
            crate::routes::workspace::problems::Failure::Machine(error) => {
                refused_value("from", "The host could not fetch it").with("about", [error.detail])
            }
            other => live::refused(other, Doing::Reading),
        })?;
    {
        let mut conn = ctx.app.db.acquire().await?;
        storage::register_blob(
            &mut conn,
            &fetched.archive.hash,
            fetched.archive.size,
            ctx.now,
        )
        .await?;
    }
    let bytes = ctx
        .app
        .blobs
        .read_whole(&fetched.archive.hash, limit)
        .await
        .map_err(ApiError::internal)?;
    Ok((bytes, fetched.commit))
}

/// Whoever reads an environment's directory for a skill manages that environment, and whoever has a
/// host fetch a repository or an address may use that host, now.
async fn may_read_directory(
    conn: &mut Conn,
    principal: &Principal,
    source: &SkillSource,
) -> ApiResult<()> {
    match source {
        SkillSource::Environment { environment_id, .. } => {
            require(
                conn,
                principal,
                ResourceKind::Environment,
                *environment_id,
                Permission::EnvironmentManage,
            )
            .await
        }
        SkillSource::Git { host_id, .. } | SkillSource::Url { host_id, .. } => {
            require(
                conn,
                principal,
                ResourceKind::Host,
                *host_id,
                Permission::HostUse,
            )
            .await
        }
        SkillSource::Upload { .. } | SkillSource::New { .. } => Ok(()),
    }
}

/// The files of the skill in the directory `root` of an environment's workspace, read by its
/// machine: listed folder by folder, then each file uploaded to the store, within what a skill may
/// hold here.
async fn read_directory(
    ctx: &Ctx,
    principal: &Principal,
    environment: Id,
    root: &str,
    limits: Limits,
) -> ApiResult<Vec<File>> {
    let machine = {
        let mut conn = ctx.app.db.acquire().await?;
        require(
            &mut conn,
            principal,
            ResourceKind::Environment,
            environment,
            Permission::EnvironmentManage,
        )
        .await?;
        let env = orch_db::queries::environments::workspace::environment(&mut conn, environment)
            .await?
            .ok_or_else(ApiError::not_found)?;
        live::require_live(ctx, &mut conn, &env).await?
    };
    let fault = |fault: SkillFault| refused("from", SkillError::Refused(fault));
    let prefix = if root.is_empty() {
        String::new()
    } else {
        format!("{root}/")
    };
    let mut pending = vec![root.to_owned()];
    let mut found: Vec<(String, String, bool)> = Vec::new();
    let mut total: u64 = 0;
    while let Some(directory) = pending.pop() {
        let mut cursor = None;
        loop {
            let listed = live::run(
                ctx,
                machine,
                SerialKey::None,
                &FsList {
                    environment_id: environment,
                    mount: None,
                    path: directory.clone(),
                    watch_id: None,
                    watch_instance: None,
                    page: Some(FsListPage {
                        cursor: cursor.take(),
                        limit: None,
                        generation: None,
                    }),
                },
            )
            .await
            .map_err(|failure| live::refused(failure, Doing::Reading))?;
            for entry in listed.entries {
                let Some(relative) = entry.path.strip_prefix(&prefix) else {
                    continue;
                };
                if archive::left_out(relative) {
                    continue;
                }
                match entry.kind {
                    DirEntryKind::Dir => pending.push(entry.path.clone()),
                    DirEntryKind::File => {
                        total = total.saturating_add(u64::try_from(entry.size).unwrap_or(0));
                        if total > limits.max_bytes {
                            return Err(fault(SkillFault::TooLarge {
                                limit: limits.max_bytes,
                            }));
                        }
                        if found.len() >= limits.max_files {
                            return Err(fault(SkillFault::TooManyFiles {
                                limit: limits.max_files,
                            }));
                        }
                        found.push((
                            relative.to_owned(),
                            entry.path.clone(),
                            entry.mode & EXECUTE_BITS != 0,
                        ));
                    }
                    DirEntryKind::Symlink => {
                        return Err(fault(SkillFault::Link(relative.to_owned())));
                    }
                    DirEntryKind::Other => {
                        return Err(fault(SkillFault::NotAFile(relative.to_owned())));
                    }
                }
            }
            cursor = listed.page.and_then(|page| page.next_cursor);
            if cursor.is_none() {
                break;
            }
        }
    }
    if !found.iter().any(|(relative, _, _)| relative == SKILL_FILE) {
        return Err(fault(SkillFault::NoSkillFile));
    }
    let dispatcher = ctx.app.link.dispatcher();
    let mut files = Vec::with_capacity(found.len());
    for (relative, full, executable) in found {
        let fetched: FsFetchResult = dispatcher
            .run(
                machine,
                &SerialKey::None,
                &FsFetch {
                    environment_id: environment,
                    mount: None,
                    path: full,
                    expect: None,
                    max_bytes: Some(i64::try_from(limits.max_bytes).unwrap_or(i64::MAX)),
                },
            )
            .await
            .map_err(|error| live::refused(live::command_failure(error), Doing::Reading))?;
        {
            let mut conn = ctx.app.db.acquire().await?;
            storage::register_blob(&mut conn, &fetched.hash, fetched.size, ctx.now).await?;
        }
        let data = ctx
            .app
            .blobs
            .read_whole(&fetched.hash, fetched.size)
            .await
            .map_err(ApiError::internal)?;
        files.push(File {
            path: relative,
            data: data.to_vec(),
            executable,
        });
    }
    Ok(files)
}

/// Who a new skill's page names, as its maker chose.
fn new_access(choice: Option<NewResourceAccess>) -> NewAccess {
    match choice {
        None | Some(NewResourceAccess::Defaults) => NewAccess::Defaults,
        Some(NewResourceAccess::OnlyMe) => NewAccess::OnlyMe,
        Some(NewResourceAccess::Home) => NewAccess::Home,
    }
}

/// Whether the caller may make a skill at `home`, which must be an org or a project.
async fn may_make_at(conn: &mut Conn, principal: &Principal, home: Id) -> ApiResult<()> {
    let (_, kind) = placed(conn, home).await?;
    if !matches!(kind, NodeKind::Org | NodeKind::Project) {
        return Err(invalid(
            "home_id",
            "a skill is at home in an org or a project",
        ));
    }
    let members = home_policies(conn, home).await?.members_make;
    let policy = policy_at(conn, home).await?;
    match ownership::may_make(
        conn,
        principal,
        ResourceKind::Skill,
        Some(home),
        &members,
        policy,
    )
    .await
    {
        Ok(Making::Allowed(_)) => Ok(()),
        Ok(Making::Refused(why)) => Err(refused_making(conn, home, why).await),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

async fn create(ctx: Ctx, (): (), body: api::SkillCreateBody) -> ApiResult<Skill> {
    if let SkillSource::New { name, description } = &body.from {
        return create_new(&ctx, &body, name, description).await;
    }
    let (principal, limits) = {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        may_make_at(&mut conn, &principal, body.home_id).await?;
        let limits = versions::limits_at(&mut conn, body.home_id)
            .await
            .map_err(ApiError::internal)?;
        (principal, limits)
    };
    let id = orch_types::new_id();
    let ready = prepare(&ctx, &principal, id, &body.from, limits, None).await?;
    let result = async {
        let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
        may_make_at(&mut tx, &principal, body.home_id).await?;
        may_read_directory(&mut tx, &principal, &body.from).await?;
        let Made { skill, version } = versions::record(
            &mut tx,
            Target::NewSkill {
                id,
                home: body.home_id,
                owner: principal.user(),
            },
            &ready.prepared,
            &ready.origin,
            Some(principal.user()),
            Time::new(ctx.now),
        )
        .await
        .map_err(|error| refused("from", error))?;
        if ready.origin.can_be_taken_again() {
            rows::set_source_url_secret(&mut tx, id, ready.source_secret).await?;
        }
        ownership::start_resource(
            &mut tx,
            ObjectRef {
                kind: ResourceKind::Skill.into(),
                id,
            },
            principal.user(),
            new_access(body.access),
            Time::new(ctx.now),
        )
        .await
        .map_err(ApiError::internal)?;
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::SkillCreated),
            id,
            home(&skill),
            "Added a skill",
            json!({"skill": id, "name": skill.name, "version": version.id}),
        )
        .await?;
        let result = show(&mut tx, &principal, id).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(result)
    }
    .await;
    if result.is_err() {
        abandon_source(&ctx, id, ready.staged.as_ref()).await;
    }
    result
}

async fn update(ctx: Ctx, (): (), body: api::SkillUpdateBody) -> ApiResult<Skill> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.revision != body.expected_revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the skill changed since it was read",
        )
        .with("current", show(&mut tx, &principal, id).await?));
    }
    if let Some(enabled) = body.enabled.filter(|enabled| *enabled != row.enabled) {
        rows::set_enabled(&mut tx, id, enabled, ctx.now).await?;
        let (action, summary) = if enabled {
            (
                orch_db::audit::action!(AuditAction::SkillEnabled),
                "Enabled a skill",
            )
        } else {
            (
                orch_db::audit::action!(AuditAction::SkillDisabled),
                "Disabled a skill",
            )
        };
        record(
            &ctx,
            &mut tx,
            action,
            id,
            home(&row),
            summary,
            json!({"skill": id, "name": row.name}),
        )
        .await?;
    }
    let result = show(&mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, _principal) = begin_mutation(&ctx).await?;
    source_url::migrate_many(&mut tx, &[id], ctx.now).await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    rows::delete(&mut tx, id, ctx.now).await?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::SkillDeleted),
        id,
        home(&row),
        "Deleted a skill",
        json!({"skill": id, "name": row.name}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

async fn version_list(
    ctx: Ctx,
    query: api::SkillVersionListQuery,
    (): (),
) -> ApiResult<Page<SkillVersionInfo>> {
    let id = ctx.id("id")?;
    let limit = match query.limit {
        None => VERSIONS_PAGE_DEFAULT,
        Some(0) => return Err(invalid("limit", "must be at least 1")),
        Some(limit) => limit.min(VERSIONS_PAGE_MAX),
    };
    let before = query
        .cursor
        .as_deref()
        .map(|cursor| {
            cursor
                .parse::<i32>()
                .map_err(|_| invalid("cursor", "is not a cursor this list gave"))
        })
        .transpose()?;
    let mut conn = ctx.app.db.acquire().await?;
    source_url::migrate_many(&mut conn, &[id], ctx.now).await?;
    let found = rows::versions(&mut conn, id, before, i64::from(limit) + 1).await?;
    let more = found.len() > limit as usize;
    let found: Vec<VersionRow> = found.into_iter().take(limit as usize).collect();
    let creators = refusals::user_refs(
        &mut conn,
        &found
            .iter()
            .filter_map(|row| row.created_by)
            .collect::<Vec<_>>(),
    )
    .await?;
    let next_cursor = if more {
        found.last().map(|row| row.number.to_string())
    } else {
        None
    };
    Ok(Page {
        items: found
            .iter()
            .map(|row| version_info(row, &creators))
            .collect::<ApiResult<_>>()?,
        next_cursor,
    })
}

/// The version `version` of the skill `skill`; not found where it is another skill's.
async fn version_of(conn: &mut Conn, skill: Id, version: Id) -> ApiResult<VersionRow> {
    rows::versions_by_id(conn, &[version])
        .await?
        .into_iter()
        .find(|row| row.skill_id == skill)
        .ok_or_else(ApiError::not_found)
}

async fn version_get(ctx: Ctx, (): (), (): ()) -> ApiResult<SkillVersion> {
    let (id, version) = (ctx.id("id")?, ctx.id("version_id")?);
    let mut conn = ctx.app.db.acquire().await?;
    source_url::migrate_many(&mut conn, &[id], ctx.now).await?;
    let row = version_of(&mut conn, id, version).await?;
    let creators =
        refusals::user_refs(&mut conn, &row.created_by.into_iter().collect::<Vec<_>>()).await?;
    Ok(SkillVersion {
        version: version_info(&row, &creators)?,
        files: files_of(&row)?,
    })
}

/// How a file of a skill is answered: as text where it reads as UTF-8, otherwise as bytes, and
/// never as something a browser would run.
fn file_reply(data: Vec<u8>) -> ApiResult<Response> {
    let media = if std::str::from_utf8(&data).is_ok() {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, media)
        .header(header::CONTENT_LENGTH, data.len())
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from(data))
        .map_err(ApiError::internal)
}

async fn version_file(ctx: Ctx, query: api::SkillFileQuery, _: Body) -> ApiResult<Response> {
    let (id, version) = (ctx.id("id")?, ctx.id("version_id")?);
    let (row, limits) = {
        let mut conn = ctx.app.db.acquire().await?;
        source_url::migrate_many(&mut conn, &[id], ctx.now).await?;
        let row = version_of(&mut conn, id, version).await?;
        let skill = rows::any_skills(&mut conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        let limits = versions::limits_at(&mut conn, skill.home_id)
            .await
            .map_err(ApiError::internal)?;
        storage::touch_blob(&mut conn, &row.archive_hash()?, ctx.now).await?;
        (row, limits)
    };
    if !files_of(&row)?.iter().any(|file| file.path == query.path) {
        return Err(ApiError::not_found());
    }
    // A version holds what its archive held when it was made, whatever the limits are now.
    let limits = Limits {
        max_bytes: limits
            .max_bytes
            .max(u64::try_from(row.size).unwrap_or(u64::MAX)),
        max_files: limits
            .max_files
            .max(usize::try_from(row.file_count()).unwrap_or(usize::MAX)),
    };
    let bytes = ctx
        .app
        .blobs
        .read_whole(
            &row.archive_hash()?,
            row.archive_size
                .max(i64::try_from(limits.archive_bytes()).unwrap_or(i64::MAX)),
        )
        .await
        .map_err(ApiError::internal)?;
    let path = query.path.clone();
    let data = tokio::task::spawn_blocking(move || {
        archive::unpack(&bytes, limits).map(|files| {
            files
                .into_iter()
                .find(|file| file.path == path)
                .map(|file| file.data)
        })
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?
    .ok_or_else(ApiError::not_found)?;
    file_reply(data)
}

async fn version_create(
    ctx: Ctx,
    (): (),
    body: api::SkillVersionCreateBody,
) -> ApiResult<SkillVersion> {
    let id = ctx.id("id")?;
    let (principal, limits) = {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        source_url::migrate_many(&mut conn, &[id], ctx.now).await?;
        let skill = rows::skills(&mut conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        let limits = versions::limits_at(&mut conn, skill.home_id)
            .await
            .map_err(ApiError::internal)?;
        (principal, limits)
    };
    let ready = prepare(&ctx, &principal, id, &body.from, limits, None).await?;
    let result = async {
        let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
        may_read_directory(&mut tx, &principal, &body.from).await?;
        let Made { skill, version } = versions::record(
            &mut tx,
            Target::Skill(id),
            &ready.prepared,
            &ready.origin,
            Some(principal.user()),
            Time::new(ctx.now),
        )
        .await
        .map_err(|error| refused("from", error))?;
        if ready.origin.can_be_taken_again() {
            rows::set_source_url_secret(&mut tx, id, ready.source_secret).await?;
        }
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::SkillVersionMade),
            id,
            home(&skill),
            "Made a version of a skill",
            json!({"skill": id, "name": skill.name, "version": version.id, "number": version.number, "origin": version.origin}),
        )
        .await?;
        let creators = refusals::user_refs(&mut tx, &[principal.user()]).await?;
        let result = SkillVersion {
            version: version_info(&version, &creators)?,
            files: files_of(&version)?,
        };
        tx.commit().await.map_err(DbError::from)?;
        Ok(result)
    }
    .await;
    if result.is_err() {
        abandon_source(&ctx, id, ready.staged.as_ref()).await;
    }
    result
}

/// The `SKILL.md` a new skill's draft begins with: its name and description, and a heading.
fn new_skill_file(name: &str, description: &str) -> String {
    let quoted = description
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("---\nname: {name}\ndescription: \"{quoted}\"\n---\n\n# {name}\n")
}

/// Makes a skill begun in Orch: no version yet, and a draft whose `SKILL.md` names it.
async fn create_new(
    ctx: &Ctx,
    body: &api::SkillCreateBody,
    name: &str,
    description: &str,
) -> ApiResult<Skill> {
    let name = name.trim();
    if let Some(why) = front_matter::name_fault(name) {
        return Err(invalid("from.name", why));
    }
    let description = description.trim();
    let chars = description.chars().count();
    if chars == 0 || chars > front_matter::DESCRIPTION_MAX_CHARS {
        return Err(invalid(
            "from.description",
            "a description has one to 1,024 characters",
        ));
    }
    {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(ctx, &mut conn).await?;
        may_make_at(&mut conn, &principal, body.home_id).await?;
    }
    let text = new_skill_file(name, description);
    let meta = ctx
        .app
        .blobs
        .put_bytes(bytes::Bytes::from(text))
        .await
        .map_err(ApiError::internal)?;
    let (ctx, mut tx, principal) = begin_mutation(ctx).await?;
    may_make_at(&mut tx, &principal, body.home_id).await?;
    if rows::named(&mut tx, body.home_id, name).await?.is_some() {
        return Err(ApiError::new(
            ErrorCode::ConflictExists,
            format!("another skill here is named {name:?} already"),
        )
        .with("name", name));
    }
    let id = orch_types::new_id();
    rows::insert(
        &mut tx,
        &rows::NewSkill {
            id,
            home_id: body.home_id,
            owner_id: principal.user(),
            name: name.to_owned(),
        },
        ctx.now,
    )
    .await?;
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ResourceKind::Skill.into(),
            id,
        },
        principal.user(),
        new_access(body.access),
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    storage::register_blob(&mut tx, &meta.hash, meta.size, ctx.now).await?;
    rows::start_draft(
        &mut tx,
        id,
        None,
        &[DraftFile {
            path: SKILL_FILE.to_owned(),
            blob: meta.hash,
            size: meta.size,
            executable: false,
        }],
        principal.user(),
        ctx.now,
    )
    .await?;
    let skill = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::SkillCreated),
        id,
        home(&skill),
        "Added a skill",
        json!({"skill": id, "name": skill.name}),
    )
    .await?;
    let result = show(&mut tx, &principal, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn reimport(ctx: Ctx, (): (), (): ()) -> ApiResult<SkillReimportReply> {
    let id = ctx.id("id")?;
    let (principal, limits, source, source_secret) = {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        source_url::migrate_many(&mut conn, &[id], ctx.now).await?;
        let skill = rows::skills(&mut conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        let origin: Option<SkillOrigin> = skill
            .origin
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?;
        let source = match origin {
            Some(SkillOrigin::Environment {
                environment_id,
                path,
            }) => SkillSource::Environment {
                environment_id,
                path,
            },
            Some(SkillOrigin::Git {
                repository,
                reference,
                folder,
                host_id,
                ..
            }) => SkillSource::Git {
                host_id,
                repository,
                reference,
                folder,
            },
            Some(SkillOrigin::Url {
                url,
                folder,
                host_id,
            }) => {
                if url.ends_with("?…") && skill.source_url_secret_id.is_none() {
                    return Err(ApiError::internal("the skill's source secret is missing"));
                }
                SkillSource::Url {
                    host_id,
                    url,
                    folder,
                }
            }
            _ => {
                return Err(refused_value(
                    "origin",
                    "this skill came from no directory, address or repository to take it from again; upload a version or publish its draft",
                ));
            }
        };
        let limits = versions::limits_at(&mut conn, skill.home_id)
            .await
            .map_err(ApiError::internal)?;
        (principal, limits, source, skill.source_url_secret_id)
    };
    let ready = prepare(&ctx, &principal, id, &source, limits, source_secret).await?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    may_read_directory(&mut tx, &principal, &source).await?;
    let skill = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let latest = rows::versions_by_id(
        &mut tx,
        &skill.latest_version_id.into_iter().collect::<Vec<_>>(),
    )
    .await?
    .pop();
    // The same files make the same archive: nothing new is kept for them.
    if let Some(latest) = latest.filter(|row| {
        row.archive_hash()
            .is_ok_and(|hash| hash == ready.prepared.archive)
    }) {
        let creators =
            refusals::user_refs(&mut tx, &latest.created_by.into_iter().collect::<Vec<_>>())
                .await?;
        let version = version_info(&latest, &creators)?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(SkillReimportReply {
            version,
            made: false,
        });
    }
    let Made { skill, version } = versions::record(
        &mut tx,
        Target::Skill(id),
        &ready.prepared,
        &ready.origin,
        Some(principal.user()),
        Time::new(ctx.now),
    )
    .await
    .map_err(|error| refused("origin", error))?;
    if ready.origin.can_be_taken_again() {
        rows::set_source_url_secret(&mut tx, id, ready.source_secret).await?;
    }
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::SkillVersionMade),
        id,
        home(&skill),
        "Updated a skill from where it came from",
        json!({"skill": id, "name": skill.name, "version": version.id, "number": version.number, "origin": version.origin}),
    )
    .await?;
    let creators = refusals::user_refs(&mut tx, &[principal.user()]).await?;
    let version = version_info(&version, &creators)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(SkillReimportReply {
        version,
        made: true,
    })
}

// ---------------------------------------------------------------- drafts

/// A skill's draft as the wire has it.
async fn draft_view(conn: &mut Conn, skill: Id) -> ApiResult<SkillDraft> {
    let info = rows::draft_infos(conn, &[skill])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let files = rows::draft_files(conn, skill).await?;
    let mut people: Vec<Id> = files.iter().map(|file| file.updated_by).collect();
    people.push(info.updated_by);
    let people = refusals::user_refs(conn, &people).await?;
    Ok(SkillDraft {
        info: SkillDraftInfo {
            based_on: info.based_on,
            file_count: info.file_count,
            revision: info.revision,
            updated_by: people.get(&info.updated_by).cloned(),
            updated_at: Time::new(info.updated_at),
        },
        files: files
            .into_iter()
            .map(|file| SkillDraftFile {
                updated_by: people.get(&file.updated_by).cloned(),
                updated_at: Time::new(file.updated_at),
                path: file.path,
                size: file.size,
                executable: file.executable,
            })
            .collect(),
    })
}

/// The refusal of a change made against a draft that changed since it was read.
async fn stale_draft(conn: &mut Conn, skill: Id) -> ApiError {
    match draft_view(conn, skill).await {
        Ok(current) => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the draft changed since it was read",
        )
        .with("current", current),
        Err(error) => error,
    }
}

async fn draft_get(ctx: Ctx, (): (), (): ()) -> ApiResult<SkillDraft> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    draft_view(&mut conn, id).await
}

/// The files of a version, read from its archive.
async fn version_files(ctx: &Ctx, row: &VersionRow, limits: Limits) -> ApiResult<Vec<File>> {
    // A version holds what its archive held when it was made, whatever the limits are now.
    let limits = Limits {
        max_bytes: limits
            .max_bytes
            .max(u64::try_from(row.size).unwrap_or(u64::MAX)),
        max_files: limits
            .max_files
            .max(usize::try_from(row.file_count()).unwrap_or(usize::MAX)),
    };
    let bytes = ctx
        .app
        .blobs
        .read_whole(
            &row.archive_hash()?,
            row.archive_size
                .max(i64::try_from(limits.archive_bytes()).unwrap_or(i64::MAX)),
        )
        .await
        .map_err(ApiError::internal)?;
    tokio::task::spawn_blocking(move || archive::unpack(&bytes, limits))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)
}

async fn draft_create(ctx: Ctx, (): (), body: api::SkillDraftCreateBody) -> ApiResult<SkillDraft> {
    let id = ctx.id("id")?;
    let (version, files) = {
        let mut conn = ctx.app.db.acquire().await?;
        let skill = rows::skills(&mut conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        let wanted = body
            .from_version
            .or(skill.latest_version_id)
            .ok_or_else(|| invalid("from_version", "the skill has no version to begin from"))?;
        let version = version_of(&mut conn, id, wanted).await?;
        let limits = versions::limits_at(&mut conn, skill.home_id)
            .await
            .map_err(ApiError::internal)?;
        let files = version_files(&ctx, &version, limits).await?;
        (version, files)
    };
    // Each file's content is kept as a blob of its own, which its row in the draft pins.
    let mut kept = Vec::with_capacity(files.len());
    for file in files {
        let meta = ctx
            .app
            .blobs
            .put_bytes(bytes::Bytes::from(file.data))
            .await
            .map_err(ApiError::internal)?;
        kept.push((
            DraftFile {
                path: file.path,
                blob: meta.hash,
                size: meta.size,
                executable: file.executable,
            },
            meta,
        ));
    }
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    source_url::migrate_many(&mut tx, &[id], ctx.now).await?;
    rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if rows::lock_draft(&mut tx, id).await?.is_some() {
        return Err(ApiError::new(
            ErrorCode::ConflictExists,
            "the skill has a draft already; publish or discard it first",
        ));
    }
    for (_, meta) in &kept {
        storage::register_blob(&mut tx, &meta.hash, meta.size, ctx.now).await?;
    }
    let files: Vec<DraftFile> = kept.into_iter().map(|(file, _)| file).collect();
    rows::start_draft(
        &mut tx,
        id,
        Some(version.id),
        &files,
        principal.user(),
        ctx.now,
    )
    .await?;
    let result = draft_view(&mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn draft_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (_ctx, mut tx, _principal) = begin_mutation(&ctx).await?;
    rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !rows::discard_draft(&mut tx, id).await? {
        return Err(ApiError::not_found());
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

async fn draft_file_get(ctx: Ctx, query: api::SkillFileQuery, _: Body) -> ApiResult<Response> {
    let id = ctx.id("id")?;
    let file = {
        let mut conn = ctx.app.db.acquire().await?;
        rows::draft_files(&mut conn, id)
            .await?
            .into_iter()
            .find(|file| file.path == query.path)
            .ok_or_else(ApiError::not_found)?
    };
    let data = ctx
        .app
        .blobs
        .read_whole(&file.blob_hash()?, file.size)
        .await
        .map_err(ApiError::internal)?;
    file_reply(data.to_vec())
}

/// A path a draft's file may have: as a skill keeps it, and at most as long as the schema allows.
fn draft_path(path: &str) -> ApiResult<String> {
    /// The longest path the schema keeps for a draft's file.
    const PATH_MAX_CHARS: usize = 4096;
    let normal = archive::normal_path(path)
        .filter(|normal| {
            normal == path && normal.chars().count() <= PATH_MAX_CHARS && !archive::left_out(normal)
        })
        .ok_or_else(|| invalid("path", "is no path a skill's file may have"))?;
    Ok(normal)
}

async fn draft_file_put(
    ctx: Ctx,
    (): (),
    body: api::SkillDraftFilePutBody,
) -> ApiResult<SkillDraft> {
    let id = ctx.id("id")?;
    let path = draft_path(&body.path)?;
    let content: Option<(Hash, i64)> = match (&body.text, &body.blob) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                "blob",
                "a file is given as text or as an upload, not both",
            ));
        }
        (Some(text), None) => {
            let meta = ctx
                .app
                .blobs
                .put_bytes(bytes::Bytes::from(text.clone().into_bytes()))
                .await
                .map_err(ApiError::internal)?;
            Some((meta.hash, meta.size))
        }
        (None, Some(blob)) => {
            let mut conn = ctx.app.db.acquire().await?;
            let principal = authorization::principal(&ctx, &mut conn).await?;
            if !orch_db::queries::uploads::owns(&mut conn, principal.user(), blob).await? {
                return Err(invalid(
                    "blob",
                    "this caller has no completed upload of that file",
                ));
            }
            let size = ctx
                .app
                .blobs
                .store()
                .size(blob)
                .await
                .map_err(ApiError::internal)?
                .ok_or_else(|| invalid("blob", "the uploaded file is not kept any more"))?;
            Some((*blob, size))
        }
        (None, None) => None,
    };
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    let skill = rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let draft = rows::lock_draft(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if draft.revision != body.expected_revision {
        return Err(stale_draft(&mut tx, id).await);
    }
    let files = rows::draft_files(&mut tx, id).await?;
    let before = files.iter().find(|file| file.path == path);
    let limits = versions::limits_at(&mut tx, skill.home_id)
        .await
        .map_err(ApiError::internal)?;
    let (blob, size) = match (content, before) {
        (Some(content), _) => content,
        (None, Some(before)) => (before.blob_hash()?, before.size),
        (None, None) => return Err(invalid("text", "a new file needs its text or an upload")),
    };
    if before.is_none() && files.len() >= limits.max_files {
        return Err(refused_value(
            "path",
            &SkillFault::TooManyFiles {
                limit: limits.max_files,
            }
            .to_string(),
        ));
    }
    let others: u64 = files
        .iter()
        .filter(|file| file.path != path)
        .map(|file| u64::try_from(file.size).unwrap_or(0))
        .sum();
    if others.saturating_add(u64::try_from(size).unwrap_or(0)) > limits.max_bytes {
        return Err(refused_value(
            "text",
            &SkillFault::TooLarge {
                limit: limits.max_bytes,
            }
            .to_string(),
        ));
    }
    let executable = body
        .executable
        .unwrap_or_else(|| before.is_some_and(|before| before.executable));
    storage::register_blob(&mut tx, &blob, size, ctx.now).await?;
    rows::put_draft_file(
        &mut tx,
        id,
        &DraftFile {
            path,
            blob,
            size,
            executable,
        },
        principal.user(),
        ctx.now,
    )
    .await?;
    let result = draft_view(&mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn draft_file_delete(
    ctx: Ctx,
    query: api::SkillDraftFileDeleteQuery,
    (): (),
) -> ApiResult<SkillDraft> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let draft = rows::lock_draft(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if draft.revision != query.expected_revision {
        return Err(stale_draft(&mut tx, id).await);
    }
    if !rows::delete_draft_file(&mut tx, id, &query.path, principal.user(), ctx.now).await? {
        return Err(ApiError::not_found());
    }
    let result = draft_view(&mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}

async fn draft_publish(
    ctx: Ctx,
    (): (),
    body: api::SkillDraftPublishBody,
) -> ApiResult<SkillVersion> {
    let id = ctx.id("id")?;
    let (based_on, files, limits) = {
        let mut conn = ctx.app.db.acquire().await?;
        let skill = rows::skills(&mut conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        let draft = rows::draft_infos(&mut conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        if draft.revision != body.expected_revision {
            return Err(stale_draft(&mut conn, id).await);
        }
        let limits = versions::limits_at(&mut conn, skill.home_id)
            .await
            .map_err(ApiError::internal)?;
        (
            draft.based_on,
            rows::draft_files(&mut conn, id).await?,
            limits,
        )
    };
    let mut read = Vec::with_capacity(files.len());
    for file in files {
        let data = ctx
            .app
            .blobs
            .read_whole(&file.blob_hash()?, file.size)
            .await
            .map_err(ApiError::internal)?;
        read.push(File {
            path: file.path,
            data: data.to_vec(),
            executable: file.executable,
        });
    }
    let store = ctx.app.blobs.store();
    let prepared = versions::prepare_files(&store, read, limits)
        .await
        .map_err(|error| refused("draft", error))?;
    let (ctx, mut tx, principal) = begin_mutation(&ctx).await?;
    source_url::migrate_many(&mut tx, &[id], ctx.now).await?;
    rows::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let draft = rows::lock_draft(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    // What was read is what is published: a change since then refuses the publish.
    if draft.revision != body.expected_revision {
        return Err(stale_draft(&mut tx, id).await);
    }
    let Made { skill, version } = versions::record(
        &mut tx,
        Target::Skill(id),
        &prepared,
        &SkillOrigin::Draft { based_on },
        Some(principal.user()),
        Time::new(ctx.now),
    )
    .await
    .map_err(|error| refused("draft", error))?;
    rows::discard_draft(&mut tx, id).await?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::SkillVersionMade),
        id,
        home(&skill),
        "Published a skill's draft",
        json!({"skill": id, "name": skill.name, "version": version.id, "number": version.number, "origin": version.origin}),
    )
    .await?;
    let creators = refusals::user_refs(&mut tx, &[principal.user()]).await?;
    let result = SkillVersion {
        version: version_info(&version, &creators)?,
        files: files_of(&version)?,
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(result)
}
