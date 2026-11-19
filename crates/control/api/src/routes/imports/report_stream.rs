//! Native report pieces keep their immutable source through the shared bounded delivery cut.

use super::*;
use crate::key_operations::{native, publication, targets};
use bytes::Bytes;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::{
    artifact_sources::{IMPORT_REPORT_DOWNLOAD as DOWNLOAD, Source},
    operation_sources::Declaration,
};
use orch_authz::{Caller, CapturedOperation, Principal, engine::Graph};
use orch_db::queries::imports::ImportRow;
use orch_engine::imports::{PreparedImportViews, PreparedReportPage};
use orch_types::Id;
use std::collections::BTreeSet;

enum Metadata {
    Json(PreparedImportViews),
    Csv(Box<ImportRow>),
}

enum Part {
    Header {
        format: ImportReportFormat,
        metadata: Metadata,
    },
    Rows {
        format: ImportReportFormat,
        page: Box<PreparedReportPage>,
    },
}

pub(super) struct Prepared {
    origin: Option<CapturedOperation>,
    part: Part,
}

impl Prepared {
    fn row(&self) -> ApiResult<&ImportRow> {
        match &self.part {
            Part::Header {
                metadata: Metadata::Json(value),
                ..
            } => {
                let [row] = value.rows() else {
                    return Err(ApiError::internal(
                        "the report opening needs one stored Import",
                    ));
                };
                Ok(row)
            }
            Part::Header {
                metadata: Metadata::Csv(row),
                ..
            } => Ok(row),
            Part::Rows { page, .. } => Ok(page.import()),
        }
    }

    fn names(&self) -> BTreeSet<Id> {
        match &self.part {
            Part::Header {
                metadata: Metadata::Json(value),
                ..
            } => value.named_places(),
            Part::Rows {
                format: ImportReportFormat::Json,
                page,
            } => page.named_places(),
            _ => BTreeSet::new(),
        }
    }

    fn bound_names(&mut self, names: BTreeSet<Id>) {
        match &mut self.part {
            Part::Header {
                metadata: Metadata::Json(value),
                ..
            } => value.bound_names(&names),
            Part::Rows { page, .. } => page.bound_names(names),
            _ => {}
        }
    }

    fn preserves(&self, fresh: &Self) -> ApiResult<bool> {
        if !read_sources::preserves(self.row()?, fresh.row()?)
            || !self.names().is_subset(&fresh.names())
        {
            return Ok(false);
        }
        Ok(match (&self.part, &fresh.part) {
            (
                Part::Header { format, .. },
                Part::Header {
                    format: current, ..
                },
            ) => format == current,
            (
                Part::Rows { format, page },
                Part::Rows {
                    format: current,
                    page: fresh,
                },
            ) => format == current && page.preserves_entries(fresh),
            _ => false,
        })
    }
}

async fn root(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    row: &ImportRow,
    origin: Option<&CapturedOperation>,
) -> ApiResult<(Principal, Vec<native::Facts<Source>>)> {
    let facts = vec![native::Facts::one(
        Source::Artifact,
        KeyTargetKind::Identity(KeyIdentityKind::Import),
        targets::from_import(conn, row).await?,
    )];
    let known = native::bind_known(ctx, &DOWNLOAD, &facts)?;
    let known = match origin {
        Some(origin) => known.with_origin(ctx, conn, caller, origin).await?,
        None => known,
    };
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    Ok((principal, facts))
}

async fn names(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    mut value: Prepared,
    mut facts: Vec<native::Facts<Source>>,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let plan = native::bind_known(ctx, &DOWNLOAD, &facts)?;
    let selected = value.names();
    let graph = Graph::load(conn, &selected.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let (labels, allowed) = native::node_name_facts(
        &plan,
        &DOWNLOAD,
        caller,
        &graph,
        &selected,
        &read_sources::NODE_SOURCES,
    )?;
    value.bound_names(allowed);
    facts.extend(labels);
    Ok((value, facts))
}

async fn own(ctx: &Ctx, conn: &mut Conn, caller: &Caller) -> ApiResult<ImportRow> {
    match ctx
        .app
        .engine
        .import_of(conn, caller.user(), ctx.id("id")?)
        .await
    {
        Ok(row) => Ok(row),
        Err(error) => Err(failed(conn, error).await),
    }
}

async fn header(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    format: ImportReportFormat,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let row = own(ctx, conn, caller).await?;
    let (principal, facts) = root(ctx, conn, caller, &row, None).await?;
    let metadata = match format {
        ImportReportFormat::Csv => Metadata::Csv(Box::new(row)),
        ImportReportFormat::Json => {
            let value = match ctx
                .app
                .engine
                .prepare_import_views(conn, &principal, vec![row])
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            Metadata::Json(value)
        }
    };
    names(
        ctx,
        conn,
        principal.caller(),
        Prepared {
            origin: None,
            part: Part::Header { format, metadata },
        },
        facts,
    )
    .await
}

pub(super) struct Opening;
impl native::Selected for Opening {
    type Source = Source;
    type Query = ImportReportDownloadQuery;
    type Value = Prepared;
    type Reply = ();
    const DECLARATION: &'static Declaration<Source> = &DOWNLOAD;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(header(
            ctx,
            conn,
            caller,
            query.format.unwrap_or(ImportReportFormat::Json),
        ))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(read_sources::require_own(ctx, conn, principal))
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async {
            Err(ApiError::internal(
                "the native report starts outside its source snapshot",
            ))
        })
    }
}

#[derive(Clone)]
struct PageInput {
    format: ImportReportFormat,
    after: Option<i64>,
    origin: Option<CapturedOperation>,
}

struct PageReader;
impl native::Selected for PageReader {
    type Source = Source;
    type Query = PageInput;
    type Value = Prepared;
    type Reply = ();
    const DECLARATION: &'static Declaration<Source> = &DOWNLOAD;
    fn origin<'a>(_query: &PageInput, value: &'a Prepared) -> Option<&'a CapturedOperation> {
        value.origin.as_ref()
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageInput,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = own(ctx, conn, caller).await?;
            let (principal, _) = root(ctx, conn, caller, &row, query.origin.as_ref()).await?;
            let page = match ctx
                .app
                .engine
                .prepare_report_chunk(conn, principal.user(), row.id, query.after)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let (_, facts) = root(
                ctx,
                conn,
                principal.caller(),
                page.import(),
                query.origin.as_ref(),
            )
            .await?;
            names(
                ctx,
                conn,
                principal.caller(),
                Prepared {
                    origin: query.origin.clone(),
                    part: Part::Rows {
                        format: query.format,
                        page: Box::new(page),
                    },
                },
                facts,
            )
            .await
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(read_sources::require_own(ctx, conn, principal))
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async {
            Err(ApiError::internal(
                "native report pieces format once after their snapshot",
            ))
        })
    }
}

async fn check(ctx: &Ctx, conn: &mut Conn, reader: &Principal, opened: &Prepared) -> ApiResult<()> {
    let (fresh, facts) = match &opened.part {
        Part::Header { format, .. } => header(ctx, conn, reader.caller(), *format).await?,
        Part::Rows { format, page } => {
            let row = own(ctx, conn, reader.caller()).await?;
            let (principal, _) =
                root(ctx, conn, reader.caller(), &row, opened.origin.as_ref()).await?;
            let fresh = match ctx
                .app
                .engine
                .refresh_report_page(conn, principal.user(), page)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let (_, facts) = root(
                ctx,
                conn,
                principal.caller(),
                fresh.import(),
                opened.origin.as_ref(),
            )
            .await?;
            names(
                ctx,
                conn,
                principal.caller(),
                Prepared {
                    origin: opened.origin.clone(),
                    part: Part::Rows {
                        format: *format,
                        page: Box::new(fresh),
                    },
                },
                facts,
            )
            .await?
        }
    };
    let same = opened.preserves(&fresh)?;
    let plan = native::bind(ctx, &DOWNLOAD, &facts)?;
    let engine = ctx.app.engine.clone();
    let user = ctx.user()?.id();
    let import = ctx.id("id")?;
    plan.read_operation(
        ctx,
        conn,
        reader.caller(),
        move |conn, principal| -> BoxFuture<'_, ApiResult<()>> {
            Box::pin(async move {
                read_sources::require_own_import(&engine, user, import, conn, principal).await?;
                if !same {
                    return Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the original stored report source changed",
                    ));
                }
                Ok(())
            })
        },
    )
    .await?
    .ok_or_else(|| {
        ApiError::new(
            ErrorCode::AuthzDenied,
            "the stored report is no longer readable",
        )
    })
}

enum Step {
    Head(Bytes),
    Rows { after: Option<i64>, first: bool },
    Done,
}

struct Stream {
    ctx: Ctx,
    header: publication::Frame,
    format: ImportReportFormat,
    origin: Option<CapturedOperation>,
    step: Step,
}

pub(super) async fn download(
    ctx: Ctx,
    query: ImportReportDownloadQuery,
    _body: Body,
) -> ApiResult<Response> {
    native::staged::read::<Opening, _, _, _>(ctx, query, |frame| async move {
        let (ctx, accepted, prepared) = frame.into_parts();
        let Part::Header { format, metadata } = &prepared.part else {
            return Err(ApiError::internal(
                "the report opening selected another native piece",
            ));
        };
        let view = match metadata {
            Metadata::Csv(_) => None,
            Metadata::Json(value) => value.render().map_err(ApiError::internal)?.pop(),
        };
        let file = orch_engine::imports::report_file(prepared.row()?, *format, view.as_ref())
            .map_err(ApiError::internal)?;
        let format = *format;
        let origin = if native::modern(&accepted) {
            Some(CapturedOperation::capture(&accepted).map_err(ApiError::internal)?)
        } else {
            None
        };
        let header = publication::frame(&ctx, &accepted, prepared, |ctx, conn, reader, opened| {
            Box::pin(check(ctx, conn, reader, opened))
        })?;
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, file.media_type)
            .header(header::CONTENT_DISPOSITION, attachment(&file.name))
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(header::CACHE_CONTROL, "private, no-store")
            .body(Body::empty())
            .map_err(ApiError::internal)?;
        let stream = futures::stream::try_unfold(
            Stream {
                ctx: ctx.clone(),
                header: header.clone(),
                format,
                origin,
                step: Step::Head(Bytes::from(file.head)),
            },
            |mut stream| async move {
                let step = std::mem::replace(&mut stream.step, Step::Done);
                match step {
                    Step::Head(bytes) => {
                        stream.step = Step::Rows {
                            after: None,
                            first: true,
                        };
                        Ok(Some((stream.header.data(bytes), stream)))
                    }
                    Step::Rows { after, first } => {
                        let input = PageInput {
                            format: stream.format,
                            after,
                            origin: stream.origin.clone(),
                        };
                        let (ctx, accepted, prepared) =
                            native::staged::read::<PageReader, _, _, _>(
                                stream.ctx.clone(),
                                input,
                                |frame| async move { Ok(frame.into_parts()) },
                            )
                            .await?;
                        let Part::Rows { page, .. } = &prepared.part else {
                            return Err(ApiError::internal(
                                "the report row selector returned another native piece",
                            ));
                        };
                        let Some(last) = page.last_position() else {
                            return match stream.format {
                                ImportReportFormat::Csv => Ok(None),
                                ImportReportFormat::Json => Ok(Some((
                                    stream.header.data(Bytes::from_static(b"]}")),
                                    stream,
                                ))),
                            };
                        };
                        let (values, _) = page.render();
                        let (bytes, first) =
                            orch_engine::imports::report_chunk(stream.format, first, values);
                        let page = publication::frame(
                            &ctx,
                            &accepted,
                            prepared,
                            |ctx, conn, reader, opened| Box::pin(check(ctx, conn, reader, opened)),
                        )?;
                        stream.step = Step::Rows {
                            after: Some(last),
                            first,
                        };
                        Ok(Some((page.data(Bytes::from(bytes)), stream)))
                    }
                    Step::Done => Ok(None),
                }
            },
        );
        publication::streaming_response(&ctx, response, header, stream)
    })
    .await
}
