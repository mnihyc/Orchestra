//! The history of a document: versions cut by the clock, listed newest first with their authors and what each changed; what differs between two states block by block; a version as it stood; and putting one back.
//!
//! A version is cut by the server, never by a button: while a document is
//! written when the last was cut longer ago than the window, shortly after it
//! was left alone, before a batch that replaces it whole, and before one is
//! put back, so that putting a version back never loses the present state.
//! The first entry of the list is the open span, what was changed since the
//! last version was cut, when something was. A person and an agent that was
//! let write are among the authors alike. Taking a change back is putting a
//! version back.

use super::build;

use super::limits;

use super::record;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::actor_name;

use crate::routes::nodes::invalid;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::permissions::Permission;

use orch_api_contract::documents::Author;

use orch_api_contract::documents::DocumentChangesGet;

use orch_api_contract::documents::DocumentSync;

use orch_api_contract::documents::DocumentVersion;

use orch_api_contract::documents::DocumentVersionGet;

use orch_api_contract::documents::DocumentVersionList;

use orch_api_contract::documents::DocumentVersionRestore;

use orch_api_contract::documents::VersionCause;

use orch_api_contract::documents::VersionChanged;

use orch_authz::Policy;

use orch_db::queries::documents::batch;

use orch_db::queries::documents::batch::Restored;

use orch_db::queries::documents::history;

use orch_db::queries::documents::history::ByAuthor;

use orch_db::queries::documents::history::VersionRow;

use orch_db::queries::documents as rows;

use orch_db::queries::documents::BlockRow;

use orch_db::Conn;

use orch_db::DbError;

use orch_documents::versions::Span;

use orch_documents::versions::Writer;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde_json::json;

use build::Cast;

use build::Wanted;

use limits::DocSettings;

use limits::Limit;
/// The word for the version that was last cut before the caller last had the
/// document open.
const SEEN: &str = "seen";

/// What an author is narrowed to by an agent's thread: the word before the identifier.
const THREAD_PREFIX: &str = "thread:";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_operation(DocumentVersionList, readers::list, readers::list_get)
        .on_snapshot_read_with_operation(DocumentChangesGet, readers::changes, readers::changes_get)
        .on_snapshot_read_with_operation(DocumentVersionGet, readers::version, readers::version_get)
        .on(DocumentVersionRestore, restore);
}

fn cause_of(word: &str) -> VersionCause {
    serde_json::from_value(Json::String(word.to_owned())).unwrap_or(VersionCause::Window)
}

fn changed_of(changed: &Json) -> VersionChanged {
    let number = |member: &str| changed.get(member).and_then(Json::as_i64).unwrap_or(0);
    VersionChanged {
        inserted: number("inserted"),
        updated: number("updated"),
        moved: number("moved"),
        removed: number("removed"),
        files: serde_json::from_value(
            changed
                .get("files")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .unwrap_or_default(),
    }
}

fn writers_of(authors: &Json) -> Vec<Writer> {
    authors
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Writer::from_json)
        .collect()
}

fn want_writers(wanted: &mut Wanted, writers: &[Writer]) {
    for writer in writers {
        match writer {
            Writer::User(user) => wanted.user(*user),
            Writer::Agent { thread, .. } => wanted.node(*thread),
        }
    }
}

fn authors_of(writers: &[Writer], cast: &Cast) -> Vec<Author> {
    writers
        .iter()
        .filter_map(|writer| match writer {
            Writer::User(user) => cast.author(Some(*user), None, None),
            Writer::Agent { thread, agent } => cast.author(None, Some(*thread), Some(agent)),
        })
        .collect()
}

fn entry(row: &VersionRow, cast: &Cast) -> DocumentVersion {
    DocumentVersion {
        version: Some(i64::from(row.version)),
        title: row.title.clone(),
        bytes: row.bytes,
        block_count: row.block_count,
        cause: Some(cause_of(&row.cause)),
        authors: authors_of(&writers_of(&row.authors), cast),
        changed: changed_of(&row.changed),
        from: Time::new(row.from_at),
        to: Time::new(row.to_at),
    }
}

/// The open span as an entry: no number and no cause yet.
fn open_span(
    title: &str,
    bytes: i64,
    block_count: i64,
    span: &Span,
    cast: &Cast,
) -> Option<DocumentVersion> {
    Some(DocumentVersion {
        version: None,
        title: title.to_owned(),
        bytes,
        block_count,
        cause: None,
        authors: authors_of(&span.authors, cast),
        changed: span.changed(),
        from: span.from_at?,
        to: span.to_at?,
    })
}

/// The author a list is narrowed to: a person's identifier, or `thread:` and
/// the identifier of an agent's thread.
fn parse_author(text: Option<&str>) -> ApiResult<Option<ByAuthor>> {
    let Some(text) = text else {
        return Ok(None);
    };
    let bad = || invalid("author", "is a user id, or thread: and the id of a thread");
    if let Some(thread) = text.strip_prefix(THREAD_PREFIX) {
        return Ok(Some(ByAuthor::Thread(thread.parse().map_err(|_| bad())?)));
    }
    Ok(Some(ByAuthor::User(text.parse().map_err(|_| bad())?)))
}

fn wrote_in(span: &Span, author: ByAuthor) -> bool {
    span.authors.iter().any(|writer| match (writer, author) {
        (Writer::User(user), ByAuthor::User(wanted)) => *user == wanted,
        (Writer::Agent { thread, .. }, ByAuthor::Thread(wanted)) => *thread == wanted,
        _ => false,
    })
}

fn version_number(ctx: &Ctx) -> ApiResult<i32> {
    ctx.param("version")?
        .parse()
        .map_err(|_| ApiError::not_found())
}

/// The version a `from` names: a number, `0` for the empty document, or `seen`.
async fn from_version(conn: &mut Conn, user: Id, document: Id, from: &str) -> ApiResult<i32> {
    if from == SEEN {
        let seen = rows::seen_seqs(conn, user, &[document]).await?;
        return Ok(match seen.get(&document) {
            Some((_, at)) => history::last_before(conn, document, *at)
                .await?
                .unwrap_or(0),
            None => 0,
        });
    }
    from.parse::<i32>()
        .ok()
        .filter(|number| *number >= 0)
        .ok_or_else(|| invalid("from", "is a version number, 0 or seen"))
}

/// The live blocks of a version, or of the document as it stands, with its title.
async fn state_at(
    conn: &mut Conn,
    document: Id,
    version: Option<i32>,
    now_title: &str,
) -> ApiResult<(Vec<BlockRow>, Option<String>)> {
    match version {
        None => Ok((
            rows::live_blocks(conn, document).await?,
            Some(now_title.to_owned()),
        )),
        Some(0) => Ok((Vec::new(), None)),
        Some(number) => {
            let found = history::get(conn, document, number)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let held = history::snapshot(conn, document, number)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Ok((held, Some(found.title)))
        }
    }
}

/// What differs between two states of a document, block by block.
/// Puts a version back: the present state is cut as a version first, then the
/// blocks of that version stand again.
async fn restore(mut ctx: Ctx, (): (), (): ()) -> ApiResult<DocumentSync> {
    let id = ctx.id("id")?;
    let number = version_number(&ctx)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let policy = Policy::default();
    let row = super::live_locked(&mut tx, id).await?;
    let settings = DocSettings::at(&mut tx, row.node_id).await?;
    let rules = settings.version_rules(ctx.app.config.documents.version_window_secs);
    let prepared = batch::prepare_restoration(
        &mut tx,
        id,
        number,
        &Writer::User(principal.user()),
        settings.batch_limits(),
        rules,
    )
    .await?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let principal = guard.principal().clone();
    authorization::require_document(&mut tx, &principal, id, Permission::DocManage, policy).await?;
    let restored = batch::apply_restoration(&mut tx, prepared, guard.now().get(), || {
        guard.check_deadline(&ctx)
    })
    .await??;
    match restored {
        Restored::Gone | Restored::NoSuchVersion => return Err(ApiError::not_found()),
        Restored::TooManyBlocks { .. } => {
            return Err(
                limits::too_large(&mut tx, &principal, &settings, Limit::Blocks, policy).await,
            );
        }
        Restored::TooLarge { .. } => {
            return Err(
                limits::too_large(&mut tx, &principal, &settings, Limit::Bytes, policy).await,
            );
        }
        Restored::Done(applied) => {
            guard.check_deadline(&ctx)?;
            let who = actor_name(&ctx)?;
            record(
                &ctx,
                &mut tx,
                orch_db::audit::action!(AuditAction::DocumentVersionRestored),
                &row,
                &format!("{who} put version {number} of \"{}\" back", row.title),
                json!({ "version": number, "kept_as_version": applied.cut.is_some() }),
            )
            .await?;
        }
    }
    let synced = super::read_sync(&mut tx, &principal, id, None, ctx.now).await?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(synced)
}

#[path = "history/readers.rs"]
pub mod readers;
