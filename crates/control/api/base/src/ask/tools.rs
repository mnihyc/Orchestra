//! Request-local in-process reads. No token is minted, saved or sent to another origin.

use super::*;

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyObjectRef;

use orch_access_contract::key_scope::KeyOperation;

use orch_access_contract::key_scope::KeyReach;

use orch_access_contract::key_scope::KeyRule;

use orch_access_contract::key_scope::KeyScope;

use orch_access_contract::key_scope::KeyTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::ask_tools;

use orch_api_contract::ask_tools::ReadParameters;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::routes::Route;

use orch_model_contract::generation::GenerationTool;

use serde::Deserialize;

use serde::Serialize;

use std::collections::BTreeMap;

use tower::ServiceExt;

use context::ReadContext;

pub(crate) use context::ReadRequest;

pub(crate) use references::Reference;

pub(crate) use references::Target as ReferenceTarget;
include!(concat!(env!("OUT_DIR"), "/ask_help.rs"));

/// Private server extension; no HTTP header or body can construct one.
#[derive(Clone)]
pub struct ReadOrigin {
    pub source: Ctx,
    pub principal: orch_authz::Principal,
    pub root: Id,
    pub conversation: Id,
}

#[derive(Default)]
pub struct ReadSources {
    checked: bool,
    targets: Vec<KeyObjectRef>,
}

struct ReadReply {
    data: Json,
    citations: Vec<Json>,
}

impl std::fmt::Debug for ReadOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReadOrigin([opaque])")
    }
}

/// One retained read witness, re-executed before its result is sent to a model again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub operation: String,
    pub parameters: ReadParameters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    projection: Option<parameters::Projection>,
}

/// Apply the existing operation-scope evaluator to the actual producer's footprint.
/// This is a conversation boundary, separate from and conjunctive with the original credential.
pub async fn check_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    plan: &crate::registry::OperationPlan,
) -> ApiResult<()> {
    let Some(origin) = &ctx.ask_read else {
        return Ok(());
    };
    let requested = origin.selection.as_str();
    if !plan.matches_route(&ctx.route)
        || !ask_tools::matches_selection(&ctx.route, requested, &plan.variant().operation)
            .map_err(ApiError::internal)?
    {
        return Err(denied("Ask only runs read operations"));
    }
    let required = plan
        .shared()
        .required_facets()
        .map_err(ApiError::internal)?;
    let response = plan.shared().response_facets().unwrap_or_default();
    let mut facets = required.clone();
    facets.extend(response.iter().cloned());
    // These IDs come only from the native producer's actual resolved footprint.
    // An Ask's personal storage path is not its fixed knowledge domain.
    let mut nodes: Vec<_> = facets
        .iter()
        .flat_map(|facet| facet.target.node_path().unwrap_or_default())
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    if rows::knowledge_roots(conn, &nodes)
        .await?
        .iter()
        .any(|root| *root != origin.root())
    {
        return Err(denied(
            "This source belongs to another root organization. Ask in that organization.",
        ));
    }
    let allow: Vec<_> = ask_tools::catalogue()
        .map_err(ApiError::internal)?
        .into_iter()
        .flat_map(|operation| {
            operation.variants.into_iter().map(|variant| KeyOperation {
                operation: variant.operation,
                actions: vec![KeyAction::Read],
            })
        })
        .collect();
    let mut scope = KeyScope {
        rules: vec![
            KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Org),
                    id: Some(origin.root()),
                    reach: KeyReach::Subtree,
                },
                resources: None,
                allow: allow.clone(),
            },
            KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
                    id: Some(origin.owner()),
                    reach: KeyReach::SelfOnly,
                },
                resources: None,
                allow: allow.clone(),
            },
        ],
        ..Default::default()
    };
    // Files and saved messages of this conversation remain in its knowledge
    // scope even when its physical storage home is the personal organization.
    {
        let id = origin.conversation();
        scope.rules.push(KeyRule {
            target: KeyTarget {
                kind: KeyTargetKind::Object(ObjectKind::Thread),
                id: Some(id),
                reach: KeyReach::SelfOnly,
            },
            resources: None,
            allow: allow.clone(),
        });
    }
    scope::check(
        &mut scope,
        origin.root(),
        origin.conversation(),
        &required,
        &response,
        &allow,
    )?;
    let mut targets: Vec<_> = required.iter().map(|facet| facet.target.object).collect();
    targets.sort_unstable();
    targets.dedup();
    *origin.sources.lock().map_err(ApiError::internal)? = ReadSources {
        checked: true,
        targets,
    };
    Ok(())
}

/// Current ordinary API handler execution, with typed decoding and its own producer/authz.
async fn read(
    origin: &ReadContext,
    operation: &str,
    parameters: &ReadParameters,
    projection: Option<&parameters::Projection>,
) -> ApiResult<ReadReply> {
    let mut selected = None;
    for route in orch_api_contract::routes::all() {
        if ask_tools::allows_selection(&route, operation).map_err(ApiError::internal)? {
            selected = Some(route);
            break;
        }
    }
    let route = selected.ok_or_else(|| denied("This operation is not in Ask's read catalogue"))?;
    let expected = route.parameters();
    if parameters.path.len() != expected.len()
        || expected
            .iter()
            .any(|name| !parameters.path.contains_key(*name))
    {
        return Err(denied(
            "The operation's path parameters do not match its schema",
        ));
    }
    let mut url = url::Url::parse("http://in-process.invalid").map_err(ApiError::internal)?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|()| ApiError::internal("invalid internal path"))?;
        path.clear();
        for part in route.path.trim_start_matches('/').split('/') {
            let value = part
                .strip_prefix('{')
                .and_then(|part| part.strip_suffix('}'))
                .and_then(|name| parameters.path.get(name))
                .map_or(part, String::as_str);
            if value.contains('/') || value.contains('\\') || value == "." || value == ".." {
                return Err(denied("Invalid path parameter"));
            }
            path.push(value);
        }
    }
    for (name, value) in &parameters.query {
        match value {
            Json::Array(values) => {
                for value in values {
                    append_query(&mut url, name, value);
                }
            }
            value => append_query(&mut url, name, value),
        }
    }
    let body = parameters
        .body
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    let uri = format!(
        "{}{}",
        url.path(),
        url.query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default()
    );
    let mut request = http::Request::builder()
        .method(route.method.as_str())
        .uri(uri)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body))
        .map_err(ApiError::internal)?;
    let sources = Arc::new(std::sync::Mutex::new(ReadSources::default()));
    request.extensions_mut().insert(Arc::new(ReadRequest::new(
        origin.clone(),
        operation.to_owned(),
        Arc::clone(&sources),
    )));
    let mut registry = crate::registry::Registry::new();
    crate::routes::mount(&mut registry);
    let router = origin.router(registry)?.with_state(origin.app().clone());
    let response = router.oneshot(request).await.map_err(ApiError::internal)?;
    if !response.status().is_success() {
        return Err(denied("This source is not available to you"));
    }
    let media_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let body = axum::body::to_bytes(
        response.into_body(),
        orch_api_contract::common::MAX_BODY_BYTES,
    )
    .await
    .map_err(|_| denied("Read a narrower page of this source"))?;
    let mut data = if route.name == "SearchStreamList" {
        super::search::decode(&body)?
    } else if let Ok(value) = serde_json::from_slice::<Json>(&body) {
        if route.name == "ReferencePreviewGet"
            && value
                .get("unavailable")
                .is_some_and(|state| !state.is_null())
        {
            return Err(denied("This source is no longer available to you"));
        }
        value
    } else {
        let address = format!(
            "{}{}",
            orch_api_contract::common::BROWSER_BASE_PATH,
            url.path()
        );
        if let Ok(text) = std::str::from_utf8(&body)
            && !text
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        {
            json!({"text":text,"url":address,"media_type":media_type})
        } else {
            json!({"url":address,"media_type":media_type,"bytes":body.len(),"state":"binary_file"})
        }
    };
    if let Some(projection) = projection {
        projection.apply(&mut data)?;
    }
    let targets = {
        let sources = sources.lock().map_err(ApiError::internal)?;
        if !sources.checked {
            return Err(denied("This read has no checked native source"));
        }
        sources.targets.clone()
    };
    let citations = source_links(&targets, parameters, &data, route.summary);
    Ok(ReadReply { data, citations })
}

fn append_query(url: &mut url::Url, name: &str, value: &Json) {
    let value = match value {
        Json::Null => return,
        Json::String(value) => value.clone(),
        value => value.to_string(),
    };
    // Repeated members and JSON form fields retain the ordinary API decoder.
    url.query_pairs_mut().append_pair(name, &value);
}

fn path(id: Id) -> BTreeMap<String, String> {
    BTreeMap::from([("id".into(), id.to_string())])
}

fn default_operation(operation: &str) -> ApiResult<String> {
    let route = orch_api_contract::routes::all()
        .into_iter()
        .find(|route| route.name == operation)
        .ok_or_else(|| denied("This operation is not in Ask's read catalogue"))?;
    ask_tools::default_selection(&route)
        .map_err(ApiError::internal)?
        .ok_or_else(|| denied("Use find_operation to select a named read variant"))
}

/// Resource references use the declared readers, including the thread engine's
/// own current timeline projection for plain thread references.
fn reference_reader(tool: &str, kind: &str) -> ApiResult<Route> {
    Ok(match (tool, kind) {
        ("history", "thread") => orch_api_contract::threads::ThreadItemList::ROUTE,
        ("history", "chat") => orch_api_contract::chats::ChatMessageList::ROUTE,
        ("history", "doc" | "document") => orch_api_contract::documents::DocumentVersionList::ROUTE,
        (_, "org" | "project" | "topic" | "node") => orch_api_contract::nodes::NodeGet::ROUTE,
        (_, "thread") => orch_api_contract::threads::ThreadSyncGet::ROUTE,
        (_, "doc" | "document") => orch_api_contract::documents::DocumentGet::ROUTE,
        (_, "chat") => orch_api_contract::chats::ChatGet::ROUTE,
        ("open", "provider") => orch_api_contract::resources::ProviderGet::ROUTE,
        ("open", "agent") => orch_api_contract::resources::AgentGet::ROUTE,
        ("open", "environment") => orch_api_contract::environment::EnvironmentGet::ROUTE,
        ("open", "host") => orch_api_contract::resources::HostGet::ROUTE,
        ("open", "machine") => orch_api_contract::resources::MachineGet::ROUTE,
        ("open", "template") => orch_api_contract::resources::TemplateGet::ROUTE,
        ("open", "mcp_server") => orch_api_contract::resources::McpServerGet::ROUTE,
        ("open", "mount") => orch_api_contract::mounts::MountGet::ROUTE,
        ("open", "skill") => orch_api_contract::skills::SkillGet::ROUTE,
        ("open", "document_container") => {
            orch_api_contract::document_tree::DocumentContainerGet::ROUTE
        }
        _ => return Err(denied("Use find_operation for this resource kind")),
    })
}

/// Use exactly the deployment addresses advertised to LINKS and the address of
/// this request. Gateway URLs are converted to their corresponding web origins.
fn own_address(ctx: &Ctx, value: &str) -> ApiResult<url::Url> {
    own_address_with(&ctx.app, Some(&ctx.headers), value)
}

fn own_address_with(
    app: &crate::state::App,
    headers: Option<&http::HeaderMap>,
    value: &str,
) -> ApiResult<url::Url> {
    let public = super::addresses::web(&app.config.system.public_url)
        .ok_or_else(|| ApiError::internal("the public deployment address is invalid"))?;
    let host = headers
        .and_then(|headers| headers.get(http::header::HOST))
        .and_then(|v| v.to_str().ok());
    let origin = headers
        .and_then(|headers| headers.get(http::header::ORIGIN))
        .and_then(|v| v.to_str().ok())
        .and_then(super::addresses::web)
        .filter(|origin| {
            host.and_then(|host| super::addresses::web(&format!("{}://{host}/", origin.scheme())))
                .is_some_and(|request| request.origin() == origin.origin())
        });
    let current = origin
        .or_else(|| {
            host.and_then(|host| super::addresses::web(&format!("{}://{host}/", public.scheme())))
        })
        .unwrap_or_else(|| public.clone());
    let mut declared = vec![public.to_string()];
    declared.extend(app.config.system.alternate_urls.iter().cloned());
    for endpoint in app
        .link
        .certificates()
        .gateway_discovery()
        .gateways
        .into_iter()
        .flat_map(|gateway| gateway.endpoints)
    {
        if let Ok(mut url) = url::Url::parse(&endpoint.url) {
            let scheme = match url.scheme() {
                "wss" => "https",
                "ws" => "http",
                "http" => "http",
                "https" => "https",
                _ => continue,
            };
            if url.set_scheme(scheme).is_ok() {
                declared.push(url.to_string());
            }
        }
    }
    super::addresses::read(value, &current, &declared)
        .ok_or_else(|| denied("Use an Orch address or reference"))
}

pub(crate) fn own_path(ctx: &Ctx, value: &str) -> ApiResult<String> {
    Ok(own_address(ctx, value)?.path().to_owned())
}

/// Parse only relative pages or the current deployment origin. Never fetch an address.
pub(crate) fn reference(ctx: &Ctx, value: &str) -> ApiResult<Reference> {
    reference_with(&ctx.app, Some(&ctx.headers), value)
}

fn reference_with(
    app: &crate::state::App,
    headers: Option<&http::HeaderMap>,
    value: &str,
) -> ApiResult<Reference> {
    let value = value.trim();
    if let Some((kind, id)) = value.split_once(':').filter(|(kind, _)| {
        !kind.contains('/')
            && !kind.eq_ignore_ascii_case("http")
            && !kind.eq_ignore_ascii_case("https")
    }) {
        if let Ok(id) = id.parse::<Id>() {
            return references::typed(kind, id);
        }
        return Err(denied("Use a complete Orch reference"));
    }
    let address = own_address_with(app, headers, value)?;
    references::address(&address)
}

/// Destination/context selection resolves a named organization from the same
/// stored address supplier as its native reader. It never invents an identity.
pub(crate) async fn reference_id(
    ctx: &Ctx,
    conn: &mut Conn,
    value: &str,
) -> ApiResult<(String, Id, String)> {
    let reference = reference(ctx, value)?;
    let (kind, id) = match reference.target {
        ReferenceTarget::Object { kind, id } if kind == "node" => (
            hierarchy::node(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?
                .kind,
            id,
        ),
        ReferenceTarget::Object { kind, id } => (kind, id),
        ReferenceTarget::Organization { address } => (
            "org".into(),
            hierarchy::org_by_address(conn, &address)
                .await?
                .ok_or_else(ApiError::not_found)?,
        ),
    };
    Ok((kind, id, reference.url))
}

/// Public bundled documentation, shared with ordinary native tools without a person credential.
pub(crate) fn documentation(arguments: &Json) -> ApiResult<Json> {
    documentation::search(arguments)
}

/// The sealed native call delegates only the current owner's ordinary reads in its real root.
pub(crate) async fn execute_thread(
    app: &crate::state::App,
    permit: &crate::thread_read_tools::OwnerReadPermit,
    name: &str,
    arguments: &Json,
) -> ApiResult<Json> {
    let declared = orch_agent_contract::read_tools::Kind::ALL
        .iter()
        .any(|kind| kind.name() == name && kind.tool() == permit.call().tool && name != "docs");
    if !declared || arguments != &permit.call().arguments {
        return Err(denied(
            "The read does not match its actual native tool call",
        ));
    }
    let identity = crate::middleware::authentication::NativeReadIdentity::new(permit);
    let context = ReadContext::Owner {
        app: app.clone(),
        identity: identity.clone(),
    };
    orch_authz::with_owner_read_source(identity.owner(), identity.source(), async {
        context.check_owner().await?;
        let (result, _) = execute_for(&context, name, arguments).await?;
        context.check_owner().await?;
        Ok(result)
    })
    .await
}

/// One of the small general tools, all backed by the catalogue's ordinary readers.
pub(crate) async fn execute(
    origin: &Arc<ReadOrigin>,
    name: &str,
    args: &Json,
) -> ApiResult<(Json, Option<Trace>)> {
    execute_for(&ReadContext::Ask(Arc::clone(origin)), name, args).await
}

async fn execute_for(
    origin: &ReadContext,
    name: &str,
    args: &Json,
) -> ApiResult<(Json, Option<Trace>)> {
    let definition = definitions()
        .into_iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| denied("Unknown read tool"))?;
    orch_types::schema::validate(&definition.parameters, args)
        .map_err(|_| denied("The tool's arguments do not match its schema"))?;
    let string = |key: &str| {
        args.get(key)
            .and_then(Json::as_str)
            .ok_or_else(|| denied(format!("Missing {key}")))
    };
    let (operation, parameters) = match name {
        "read" => {
            let operation = string("operation")?.to_owned();
            let parameters = serde_json::from_value(
                args.get("parameters").cloned().unwrap_or_else(|| json!({})),
            )
            .map_err(|_| denied("Invalid operation parameters"))?;
            (operation, parameters)
        }
        "open" | "history" => {
            if name == "open"
                && let Ok(address) =
                    own_address_with(origin.app(), origin.headers(), string("reference")?)
                        .map(|address| address.path().to_owned())
            {
                let parts: Vec<_> = address.trim_matches('/').split('/').collect();
                if parts.len() == 7
                    && matches!(parts[0], "api" | "openapi")
                    && parts[1] == "v1"
                    && parts[2] == "ask"
                    && parts[3] == "conversations"
                    && parts[5] == "attachments"
                {
                    let parameters = ReadParameters {
                        path: BTreeMap::from([
                            ("id".into(), parts[4].into()),
                            ("hash".into(), parts[6].into()),
                        ]),
                        ..Default::default()
                    };
                    let reply = read(origin, "AskAttachmentGet", &parameters, None).await?;
                    return Ok((
                        json!({"data":reply.data,"citations":[{"url":address,"reference":address}]}),
                        Some(Trace {
                            operation: "AskAttachmentGet".into(),
                            parameters,
                            projection: None,
                        }),
                    ));
                }
            }
            let reference = reference_with(origin.app(), origin.headers(), string("reference")?)?;
            let url = reference.url;
            let (kind, id) = match reference.target {
                ReferenceTarget::Object { kind, id } => (kind, id),
                ReferenceTarget::Organization { address } => {
                    if name != "open" {
                        return Err(denied(
                            "Choose a thread, chat or document for paged history",
                        ));
                    }
                    let reader = orch_api_contract::nodes::OrgBySlugGet::ROUTE;
                    let parameters = ReadParameters {
                        path: BTreeMap::from([("slug".into(), address)]),
                        ..Default::default()
                    };
                    let operation = default_operation(reader.name)?;
                    let result = read(origin, &operation, &parameters, None).await?;
                    return Ok((
                        json!({"data":result.data,"citations":result.citations}),
                        Some(Trace {
                            operation,
                            parameters,
                            projection: None,
                        }),
                    ));
                }
            };
            if name == "open" {
                let reference_kind = match kind.as_str() {
                    "doc" | "document" => "document",
                    "chat" => "chat",
                    "org" | "project" | "topic" | "thread" | "node" => "node",
                    _ => {
                        let reader = reference_reader(name, &kind)?;
                        let operation = default_operation(reader.name)?;
                        let parameters = ReadParameters {
                            path: path(id),
                            ..Default::default()
                        };
                        let result = read(origin, &operation, &parameters, None).await?;
                        return Ok((
                            json!({"data":result.data,"citations":result.citations}),
                            Some(Trace {
                                operation,
                                parameters,
                                projection: None,
                            }),
                        ));
                    }
                };
                let mut parameters = ReadParameters {
                    query: BTreeMap::from([
                        ("kind".into(), json!(reference_kind)),
                        ("id".into(), json!(id)),
                    ]),
                    ..Default::default()
                };
                // Select the same document block, thread turn or chat message
                // as the browser's linked reference. The native typed reader
                // validates each selector and its relationship to this target.
                let address = url::Url::parse("https://in-process.invalid/")
                    .and_then(|base| base.join(&url))
                    .map_err(ApiError::internal)?;
                references::selectors(reference_kind, &address, &mut parameters.query)?;
                let result = read(origin, "ReferencePreviewGet", &parameters, None)
                    .await?
                    .data;
                return Ok((
                    json!({"data":result,"citations":[{"reference":format!("{kind}:{id}"),"url":url,"title":result.get("title").cloned().unwrap_or(Json::Null)}]}),
                    Some(Trace {
                        operation: "ReferencePreviewGet".into(),
                        parameters,
                        projection: None,
                    }),
                ));
            }
            let reader = reference_reader(name, &kind)?;
            let (parameters, projection) = parameters::history(&kind, id, args)?;
            let operation = default_operation(reader.name)?;
            let result = read(origin, &operation, &parameters, projection.as_ref()).await?;
            return Ok((
                json!({"data":result.data,"citations":result.citations}),
                Some(Trace {
                    operation,
                    parameters,
                    projection,
                }),
            ));
        }
        "search" => {
            let mut query = BTreeMap::from([
                ("q".into(), json!(string("query")?)),
                ("scope".into(), json!(origin.root())),
                ("immediate".into(), json!(true)),
                ("sort".into(), json!("relevance")),
            ]);
            if let Some(place) = args.get("place") {
                query.insert("scope".into(), place.clone());
            }
            if let Some(kinds) = args.get("kinds").and_then(Json::as_array) {
                query.insert(
                    "kinds".into(),
                    json!(
                        kinds
                            .iter()
                            .filter_map(Json::as_str)
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                );
            }
            (
                "SearchStreamList".into(),
                ReadParameters {
                    query,
                    ..Default::default()
                },
            )
        }
        "list" => {
            let kind = string("kind")?;
            let place = args
                .get("place")
                .and_then(Json::as_str)
                .map(str::parse)
                .transpose()
                .map_err(|_| denied("Invalid place"))?
                .unwrap_or(origin.root());
            parameters::list(kind, place, args.get("filter"))?
        }
        "find_operation" => {
            let query = string("query")?.to_lowercase();
            let mut found = Vec::new();
            let routes = orch_api_contract::routes::all();
            'operations: for operation in ask_tools::catalogue().map_err(ApiError::internal)? {
                let route = routes
                    .iter()
                    .find(|route| route.name == operation.operation_id)
                    .ok_or_else(|| ApiError::internal("a read operation has no route"))?;
                for variant in operation.variants {
                    if !format!("{} {}", variant.operation, route.summary)
                        .to_lowercase()
                        .contains(&query)
                    {
                        continue;
                    }
                    found.push(json!({"operation":variant.operation,"description":route.summary,"selector":variant.selector,"schemas":ask_tools::schemas(route)}));
                    if found.len() == 20 {
                        break 'operations;
                    }
                }
            }
            return Ok((json!({"operations":found,"citations":[]}), None));
        }
        "docs" => return Ok((documentation(args)?, None)),
        _ => return Err(denied("Unknown read tool")),
    };
    let operation = if name == "read" {
        operation
    } else {
        default_operation(&operation)?
    };
    let result = read(origin, &operation, &parameters, None).await?;
    Ok((
        json!({"data":result.data,"citations":result.citations}),
        Some(Trace {
            operation,
            parameters,
            projection: None,
        }),
    ))
}

/// Refresh the actual current projection, not only the success status of a
/// collection query: a successful read may now omit some formerly readable rows.
pub(crate) async fn refresh(origin: &Arc<ReadOrigin>, trace: &Trace) -> ApiResult<Json> {
    let reply = read(
        &ReadContext::Ask(Arc::clone(origin)),
        &trace.operation,
        &trace.parameters,
        trace.projection.as_ref(),
    )
    .await?;
    let data = reply.data;
    let mut citations = reply.citations;
    if trace.operation == "ReferencePreviewGet"
        && let (Some(id), Some(kind)) = (
            data.get("id").and_then(Json::as_str),
            data.get("kind").and_then(Json::as_str),
        )
    {
        let kind = match kind {
            "node" => data
                .get("node_kind")
                .and_then(Json::as_str)
                .unwrap_or("node"),
            "document" => "doc",
            other => other,
        };
        let mut url = format!("/{kind}/{id}");
        for key in ["block", "turn", "message"] {
            if let Some(value) = trace.parameters.query.get(key) {
                let value = value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned);
                url.push_str(&format!(
                    "{}{}={value}",
                    if url.contains('?') { '&' } else { '?' },
                    key
                ));
            }
        }
        citations.push(json!({"reference":url,"url":url,"title":data.get("title")}));
    }
    Ok(json!({"data":data,"citations":citations}))
}

enum SourceName<'a> {
    Named(&'a str),
    Unnamed,
    Unavailable,
}

fn source_name<'a>(data: &'a Json, id: &str) -> Option<SourceName<'a>> {
    match data {
        Json::Object(row) => {
            if row.get("id").and_then(Json::as_str) == Some(id) {
                if ["content_hidden", "redacted"]
                    .iter()
                    .any(|key| row.get(*key) == Some(&Json::Bool(true)))
                {
                    return Some(SourceName::Unavailable);
                }
                return Some(
                    row.get("title")
                        .or_else(|| row.get("name"))
                        .and_then(Json::as_str)
                        .map_or(SourceName::Unnamed, SourceName::Named),
                );
            }
            row.values().find_map(|value| source_name(value, id))
        }
        Json::Array(rows) => rows.iter().find_map(|row| source_name(row, id)),
        _ => None,
    }
}

/// Source identities come from the checked native footprint. JSON in a document
/// or model result cannot declare a new resource identity for this list.
fn source_links(
    targets: &[KeyObjectRef],
    parameters: &ReadParameters,
    data: &Json,
    description: &str,
) -> Vec<Json> {
    let mut links = canonical_links(data);
    for target in targets {
        let Some(id) = target.id else {
            continue;
        };
        let base = match target.kind {
            KeyTargetKind::Object(ObjectKind::Org) => "/org",
            KeyTargetKind::Object(ObjectKind::Project) => "/project",
            KeyTargetKind::Object(ObjectKind::Topic) => "/topic",
            KeyTargetKind::Object(ObjectKind::Thread) => "/thread",
            KeyTargetKind::Object(ObjectKind::Document) => "/doc",
            KeyTargetKind::Object(ObjectKind::DocumentContainer) => "/document-folder",
            KeyTargetKind::Object(ObjectKind::Provider) => "/resources/providers",
            KeyTargetKind::Object(ObjectKind::Agent) => "/resources/agents",
            KeyTargetKind::Object(ObjectKind::Environment) => "/resources/environments",
            KeyTargetKind::Object(ObjectKind::Host) => "/resources/hosts",
            KeyTargetKind::Object(ObjectKind::Machine) => "/resources/machines",
            KeyTargetKind::Object(ObjectKind::Template) => "/resources/templates",
            KeyTargetKind::Object(ObjectKind::McpServer) => "/resources/mcp",
            KeyTargetKind::Object(ObjectKind::Mount) => "/resources/mounts",
            KeyTargetKind::Object(ObjectKind::Skill) => "/resources/skills",
            KeyTargetKind::Identity(KeyIdentityKind::Chat) => "/chat",
            _ => continue,
        };
        let id = id.to_string();
        let name = source_name(data, &id).or_else(|| {
            parameters
                .path
                .values()
                .any(|value| value == &id)
                .then_some(SourceName::Unnamed)
        });
        let title = match name {
            Some(SourceName::Named(name)) => json!(name),
            Some(SourceName::Unnamed) => json!(description.trim()),
            Some(SourceName::Unavailable) => Json::Null,
            None => continue,
        };
        let url = format!("{base}/{id}");
        links.push(json!({"reference":url,"url":url,"title":title}));
    }
    links.sort_by_key(Json::to_string);
    links.dedup();
    links.truncate(50);
    links
}

fn canonical_links(value: &Json) -> Vec<Json> {
    let mut links = Vec::new();
    fn visit(value: &Json, links: &mut Vec<Json>) {
        match value {
            Json::Object(object) => {
                for key in ["url", "address", "href"] {
                    if let Some(url) = object
                        .get(key)
                        .and_then(Json::as_str)
                        .filter(|url| url.starts_with('/') && !url.starts_with("//"))
                    {
                        let title = object
                            .get("title")
                            .or_else(|| object.get("name"))
                            .filter(|value| value.is_string())
                            .cloned()
                            .unwrap_or_else(|| json!(url));
                        links.push(json!({"url":url,"title":title}));
                    }
                }
                for value in object.values() {
                    visit(value, links);
                }
            }
            Json::Array(values) => {
                for value in values {
                    visit(value, links);
                }
            }
            _ => {}
        }
    }
    visit(value, &mut links);
    links.sort_by_key(Json::to_string);
    links.dedup();
    links.truncate(50);
    links
}

pub(crate) fn definitions() -> Vec<GenerationTool> {
    let mut tools = orch_agent_contract::read_tools::definitions()
        .into_iter()
        .map(|definition| GenerationTool {
            name: definition.name.into(),
            description: definition.description.into(),
            parameters: definition.parameters,
        })
        .collect::<Vec<_>>();
    tools.extend([
        ("history","Read a page of thread/chat history by sequence or document history by version. For documents, before accepts the native next_cursor and after is an exclusive version number.",json!({"type":"object","properties":{"reference":{"type":"string"},"before":{},"after":{}},"required":["reference"],"additionalProperties":false})),
        ("list","List readable threads, documents, places, topics, projects or orgs. Place is a native parent ID; threads need a topic and topics need a project. Filter uses that native list's query fields; find_operation shows its schema and discovers other lists.",json!({"type":"object","properties":{"kind":{"type":"string"},"place":{"type":"string"},"filter":{"type":"object"}},"required":["kind"],"additionalProperties":false})),
    ].into_iter().map(|(name,description,parameters)| GenerationTool {name:name.into(),description:description.into(),parameters}));
    tools
}

#[path = "tools/context.rs"]
pub mod context;

#[path = "tools/documentation.rs"]
pub mod documentation;

#[path = "tools/parameters.rs"]
pub mod parameters;

#[path = "tools/references.rs"]
pub mod references;

#[path = "tools/scope.rs"]
pub mod scope;

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn canonical_sources_require_checked_native_identities_and_preserve_redaction() {
        // EN: Ask > read a source > follow its canonical citation; a hidden
        // source stays unavailable and arbitrary returned JSON supplies no grant.
        // 中文：问一问 > 读取来源 > 打开规范引用；隐藏来源保持不可用，返回的 JSON 不能授予权限。
        let actual = new_id();
        let injected = new_id();
        let data = json!({"id":actual,"title":"A current source","extra":{"id":injected,"name":"An unobserved source"}});
        let source = KeyObjectRef {
            kind: KeyTargetKind::Object(ObjectKind::Document),
            id: Some(actual),
        };
        let parameters = ReadParameters {
            path: path(actual),
            ..Default::default()
        };
        assert!(source_links(&[], &parameters, &data, "Read source").is_empty());
        let links = source_links(&[source], &parameters, &data, "Read source");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0]["url"], format!("/doc/{actual}"));
        assert_eq!(links[0]["title"], "A current source");
        let hidden = source_links(
            &[source],
            &parameters,
            &json!({"id":actual,"content_hidden":true,"title":"Private marker"}),
            "Read source",
        );
        assert!(hidden[0]["title"].is_null());
        assert!(
            hidden
                .iter()
                .all(|link| !link.to_string().contains("Private marker"))
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_canonical_sources_require_checked_native_identities_and_preserve_redaction() {
        self::canonical_sources_require_checked_native_identities_and_preserve_redaction()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn repeated_and_json_query_fields_keep_the_native_form_encoding() -> ApiResult<()> {
        let mut url = url::Url::parse("http://in-process.invalid/").map_err(ApiError::internal)?;
        append_query(&mut url, "tag_id", &json!("first&inspect_deleted=true"));
        append_query(&mut url, "tag_id", &json!("second"));
        append_query(&mut url, "thread_access", &json!({"whole":"private"}));
        let fields: Vec<_> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("tag_id".to_owned(), "first&inspect_deleted=true".to_owned()),
                ("tag_id".to_owned(), "second".to_owned()),
                (
                    "thread_access".to_owned(),
                    "{\"whole\":\"private\"}".to_owned()
                ),
            ]
        );
        assert!(!fields.iter().any(|(key, _)| key == "inspect_deleted"));
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_repeated_and_json_query_fields_keep_the_native_form_encoding()
    -> crate::middleware::requests::ApiResult<()> {
        self::repeated_and_json_query_fields_keep_the_native_form_encoding()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn every_reference_reader_resolves_to_a_registered_read_operation() -> ApiResult<()> {
        // EN: Ask > request an org, project, topic or thread reference > open
        // its source. Each mapping resolves to that resource's native reader.
        // 中文：问一问 > 请求组织、项目、话题或会话引用 > 打开来源；每个映射都应解析到
        // 相应资源的原生读取操作。
        let routes = orch_api_contract::routes::all();
        for tool in ["open", "history"] {
            for kind in [
                "org", "project", "topic", "thread", "doc", "document", "chat",
            ] {
                let reader = reference_reader(tool, kind)?;
                let Some(declared) = routes.iter().find(|route| route.name == reader.name) else {
                    panic!(
                        "{tool}({kind}) names an unregistered reader {}",
                        reader.name
                    );
                };
                assert_eq!(declared.path, reader.path);
                assert_eq!(declared.method, reader.method);
                assert_eq!(declared.parameters(), ["id"]);
                let selection = default_operation(reader.name)?;
                assert!(
                    ask_tools::allows_selection(declared, &selection)
                        .map_err(ApiError::internal)?
                );
                let expected = match (tool, kind) {
                    ("history", "thread") => "ThreadItemList.read",
                    ("history", "chat") => "ChatMessageList",
                    ("history", "doc" | "document") => "DocumentVersionList.read",
                    (_, "org" | "project" | "topic") => "NodeGet.read",
                    (_, "thread") => "ThreadSyncGet.read",
                    (_, "doc" | "document") => "DocumentGet.read",
                    (_, "chat") => "ChatGet",
                    _ => panic!("the deciding table covers only supported references"),
                };
                assert_eq!(selection, expected, "{tool}({kind})");
            }
        }
        for kind in [
            "provider",
            "agent",
            "environment",
            "host",
            "machine",
            "template",
            "mcp_server",
            "mount",
            "skill",
            "document_container",
        ] {
            let reader = reference_reader("open", kind)?;
            assert!(
                routes.iter().any(|route| route.name == reader.name),
                "{kind}"
            );
            let selection = default_operation(reader.name)?;
            assert!(ask_tools::allows_selection(&reader, &selection).map_err(ApiError::internal)?);
        }
        assert!(reference_reader("open", "unknown").is_err());
        assert!(reference_reader("history", "unknown").is_err());
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_every_reference_reader_resolves_to_a_registered_read_operation()
    -> crate::middleware::requests::ApiResult<()> {
        self::every_reference_reader_resolves_to_a_registered_read_operation()
    }
}
