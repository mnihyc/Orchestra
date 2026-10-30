//! Environment-local MCP execution service: the tools of the session's set.
//!
//! [`OrchExec`] is the executor an environment's exec node starts. Started, it listens on the loopback
//! address on a port the system gives and serves MCP over streamable HTTP at [`MCP_PATH`]. It keeps nothing
//! between requests that belongs to a connection: every request says which session it belongs to (the header
//! of [`crate::scope`]) and carries the executor's token, and what is kept is kept by thread: the directory a
//! `cd` left, the commands that run in the background, the output they wrote.
//!
//! What the node does with it: it starts the executor with [`Executor::start`], and gets back the endpoint:
//! the address of the MCP server, the bearer token its connection has to present, and the tool sets it
//! offers. For each exchange of a harness's `exec` server that its connector relays, it sets the headers
//! `Authorization: Bearer <token>` and `x-orch-session: <the session>` (see [`SessionScope::encode`]) and
//! forwards the request to the address. The tool set of a session is the one in its header; a session of the
//! set `custom` has no `exec` server, and if one asks, it is listed no tool.

use std::net::{Ipv4Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use orch_agent_contract::adapter::AdapterError;
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::descriptor::{ExecutorAvailability, ExecutorDescriptor};
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use orch_plugin_contract::Secret;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::json;
use subtle::ConstantTimeEq as _;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::commands::{Commands, DEFAULT_STOP_GRACE, Host};
use crate::descriptor::{self, KIND};
use crate::output::OutputLimits;
use crate::sandbox::Access;
use crate::scope::{SESSION_HEADER, SessionScope};
use crate::tools::{Call, ToolFault, ToolOutput, ToolSet};
use crate::{claude_code, plain};

/// The path the MCP server answers at.
pub const MCP_PATH: &str = "/mcp";

/// The `_meta` members of a `tools/call` in which a harness names the call: the value is the tool call's id,
/// which becomes `ORCH_ITEM_ID` in the commands of that call. Claude Code names it here.
pub const CALL_ID_META_KEYS: [&str; 1] = ["claudecode/toolUseId"];

/// The largest request the server reads: a file of the largest size an edit works on, written in a JSON
/// string, with room for what escaping adds.
const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

/// How many bytes of randomness the token holds: 256 bits.
const TOKEN_BYTES: usize = 32;

/// How many random ports of a range are tried before the range is given up on.
const PORT_TRIES: usize = 64;

/// How long the server is given to finish what it has begun when the executor stops, once the commands
/// have been ended.
const SERVER_STOP_WAIT: Duration = Duration::from_secs(5);

/// The longest a call id from a harness is taken: it ends up in a variable.
const MAX_CALL_ID_CHARS: usize = 128;

/// How often a call that is still being carried out is written a line, by default. The server learns that a
/// client has gone away by failing to write to it, so this is how soon the command of a call whose client
/// has gone is ended; it also keeps a path through a relay from being closed for being quiet.
pub const DEFAULT_KEEP_ALIVE: Duration = Duration::from_secs(5);

/// What the executor is configured with.
#[derive(Debug, Clone)]
pub struct ExecConfig {
    /// The bounds of a command's output.
    pub limits: OutputLimits,
    /// How long a command has between `TERM` and `KILL`.
    pub stop_grace: Duration,
    /// The ports the server may listen on. `None` lets the system give one, which is what an exec node
    /// does; a range is for a machine that allows a few ports only.
    pub ports: Option<RangeInclusive<u16>>,
    /// How often a call that is still being carried out is written a line.
    pub keep_alive: Duration,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            limits: OutputLimits::default(),
            stop_grace: DEFAULT_STOP_GRACE,
            ports: None,
            keep_alive: DEFAULT_KEEP_ALIVE,
        }
    }
}

/// A started server.
struct Running {
    endpoint: ExecutorEndpoint,
    commands: Commands,
    root: CancellationToken,
    server: JoinHandle<()>,
    /// Ends the server and the commands if the executor is dropped without being stopped.
    _guard: DropGuard,
}

/// Orch's own executor: its tools served through MCP in the tool set a session has.
#[derive(Default)]
pub struct OrchExec {
    config: ExecConfig,
    running: Option<Running>,
}

impl std::fmt::Debug for OrchExec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OrchExec")
            .field("running", &self.running.is_some())
            .finish_non_exhaustive()
    }
}

impl OrchExec {
    /// An executor with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An executor with the configuration given.
    #[must_use]
    pub fn with_config(config: ExecConfig) -> Self {
        Self {
            config,
            running: None,
        }
    }

    /// Where the started executor is reached, while it runs.
    #[must_use]
    pub fn endpoint(&self) -> Option<&ExecutorEndpoint> {
        self.running.as_ref().map(|running| &running.endpoint)
    }

    /// Why the executor cannot be started in this environment, if it cannot.
    fn unavailable(env: &EnvironmentContext) -> Option<String> {
        if env.os != orch_agent_contract::facts::Os::Linux {
            return Some("this executor is declared for Linux; this host's native execution has not been verified".into());
        }
        if env.data_dir.trim().is_empty() {
            return Some("the environment gave the executor no directory of its own".to_owned());
        }
        if !Path::new(&env.shell.path).is_file() {
            return Some(format!(
                "the environment's shell, {}, is not there",
                env.shell.path
            ));
        }
        None
    }
}

#[async_trait]
impl Executor for OrchExec {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn describe(&self) -> ExecutorDescriptor {
        descriptor::descriptor()
    }

    async fn available(&self, env: &EnvironmentContext) -> ExecutorAvailability {
        match Self::unavailable(env) {
            Some(reason) => ExecutorAvailability::unavailable(&reason),
            None => descriptor::availability(env.mcp_servers_enabled),
        }
    }

    async fn start(&mut self, env: &EnvironmentContext) -> Result<ExecutorEndpoint, AdapterError> {
        if let Some(reason) = Self::unavailable(env) {
            return Err(AdapterError::HarnessUnavailable(reason));
        }
        if let Some(running) = &self.running {
            return Ok(running.endpoint.clone());
        }
        let root = CancellationToken::new();
        let outputs_dir = PathBuf::from(&env.data_dir).join("outputs");
        let commands = Commands::new(
            Host {
                shell: env.shell.clone(),
                home: env.home.clone(),
                user: env.user.clone(),
                variables: env.variables.clone(),
                process_launcher: env.process_launcher.clone(),
                outputs_dir,
                limits: self.config.limits,
                stop_grace: self.config.stop_grace,
            },
            root.child_token(),
        );
        let mut raw = [0u8; TOKEN_BYTES];
        rand::fill(&mut raw[..]);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);

        let listener = bind(self.config.ports.as_ref()).await.map_err(|error| {
            AdapterError::Internal(format!("could not listen on the loopback address: {error}"))
        })?;
        let port = listener
            .local_addr()
            .map_err(|error| {
                AdapterError::Internal(format!("the listener has no address: {error}"))
            })?
            .port();
        let app = router(
            commands.clone(),
            token.clone(),
            root.child_token(),
            self.config.keep_alive,
        );
        let shutdown = root.clone();
        let server = tokio::spawn(async move {
            let served = axum::serve(listener, app)
                .with_graceful_shutdown(async move { shutdown.cancelled_owned().await })
                .await;
            if let Err(error) = served {
                tracing::warn!(%error, "the exec server ended with an error");
            }
        });
        let endpoint = ExecutorEndpoint {
            address: format!("http://127.0.0.1:{port}{MCP_PATH}"),
            token: Some(Secret::new(token)),
            version: descriptor::version().to_owned(),
            tool_sets: ToolSet::offered_now(env.mcp_servers_enabled)
                .iter()
                .map(|set| set.name().to_owned())
                .collect(),
            sandbox_modes: descriptor::sandbox_modes(),
        };
        tracing::info!(port, "the exec server listens on the loopback address");
        self.running = Some(Running {
            endpoint: endpoint.clone(),
            commands,
            root: root.clone(),
            server,
            _guard: root.drop_guard(),
        });
        Ok(endpoint)
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        let Some(running) = self.running.take() else {
            return Ok(());
        };
        // The commands are ended first and the server then has nothing left to wait for.
        running.commands.shutdown().await;
        running.root.cancel();
        let mut server = running.server;
        if tokio::time::timeout(SERVER_STOP_WAIT, &mut server)
            .await
            .is_err()
        {
            server.abort();
        }
        Ok(())
    }
}

/// A listener on the loopback address: on a port the system gives, or on a free port of the range.
async fn bind(ports: Option<&RangeInclusive<u16>>) -> std::io::Result<TcpListener> {
    let Some(range) = ports else {
        return TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await;
    };
    let width = u32::from(*range.end()).saturating_sub(u32::from(*range.start())) + 1;
    for _ in 0..PORT_TRIES {
        let offset = rand::random_range(0..width);
        let port = u16::try_from(u32::from(*range.start()) + offset).unwrap_or(*range.start());
        match TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        "no free port in the range",
    ))
}

/// What the gate in front of the MCP service checks against.
#[derive(Clone)]
struct Gate {
    token: Arc<String>,
}

/// The server: the MCP service behind a gate that asks for the token and the session.
fn router(
    commands: Commands,
    token: String,
    stop: CancellationToken,
    keep_alive: Duration,
) -> axum::Router {
    let handler = Handler { commands };
    let mut config = StreamableHttpServerConfig::default()
        // Every request stands alone: the session is in a header and what is kept is kept by thread.
        .with_legacy_session_mode(false)
        .with_sse_keep_alive(Some(keep_alive))
        .with_cancellation_token(stop);
    config.max_request_body_bytes = MAX_REQUEST_BYTES;
    let service: StreamableHttpService<Handler, NeverSessionManager> = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    axum::Router::new()
        .nest_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(
            Gate {
                token: Arc::new(token),
            },
            gate,
        ))
}

/// Lets a request through only with the executor's token and a session it can read, and hands the session
/// on to the tools.
async fn gate(State(gate): State<Gate>, mut request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if !bool::from(presented.as_bytes().ct_eq(gate.token.as_bytes())) {
        let mut refusal = (StatusCode::UNAUTHORIZED, "a bearer token is needed").into_response();
        refusal
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return refusal;
    }
    let Some(value) = request
        .headers()
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "the header {SESSION_HEADER} names the session of the request, and it is missing"
            ),
        )
            .into_response();
    };
    match SessionScope::decode(value) {
        Ok(scope) => {
            request.extensions_mut().insert(Arc::new(scope));
            next.run(request).await
        }
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

/// The MCP side: what the server lists and carries out.
#[derive(Clone)]
struct Handler {
    commands: Commands,
}

/// The session a request belongs to, as the gate read it.
fn scope_of(context: &RequestContext<RoleServer>) -> Result<Arc<SessionScope>, McpError> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Arc<SessionScope>>())
        .cloned()
        .ok_or_else(|| McpError::invalid_request("the request names no session", None))
}

/// The id of the tool call: what the harness named it in `_meta`, else the JSON-RPC id of the request. It
/// ends up in the environment of commands, so it is kept to plain characters and a bounded length.
fn call_id_of(request: &CallToolRequestParams, context: &RequestContext<RoleServer>) -> String {
    let named = CALL_ID_META_KEYS.iter().find_map(|key| {
        let from_params = request.meta.as_ref().and_then(|meta| meta.get(*key));
        from_params
            .or_else(|| context.meta.get(*key))
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    });
    let id = named.unwrap_or_else(|| match &context.id {
        rmcp::model::NumberOrString::Number(number) => format!("rpc-{number}"),
        rmcp::model::NumberOrString::String(text) => format!("rpc-{text}"),
    });
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_CALL_ID_CHARS)
        .collect()
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new(KIND, descriptor::version()).with_title(descriptor::title()),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let scope = scope_of(&context)?;
        Ok(ListToolsResult::with_all_items(
            scope.tool_set.tools().to_vec(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let scope = scope_of(&context)?;
        let call_id = call_id_of(&request, &context);
        let name = request.name.to_string();
        tracing::debug!(thread = %scope.thread_id, tool = %name, "a tool is called");
        let outcome = tokio::select! {
            biased;
            () = context.ct.cancelled() => Err(ToolFault::Cancelled),
            outcome = self.carry_out(&scope, &name, request, &call_id, &context.ct) => outcome,
        };
        Ok(match outcome {
            Ok(output) => output_result(output),
            Err(fault) => fault_result(&fault),
        }
        .into())
    }
}

impl Handler {
    /// Carries out the tool `name` of the session's set.
    async fn carry_out(
        &self,
        scope: &SessionScope,
        name: &str,
        request: CallToolRequestParams,
        call_id: &str,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolFault> {
        let known = scope.tool_set.tool(name).is_some();
        if !known {
            let listed: Vec<String> = scope
                .tool_set
                .tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            return Err(ToolFault::NotFound(format!(
                "the tool set {} has no tool called {name}; its tools are {}",
                scope.tool_set.name(),
                if listed.is_empty() {
                    "none".to_owned()
                } else {
                    listed.join(", ")
                }
            )));
        }
        let kept = self.commands.output_dir(&scope.thread_id);
        std::fs::create_dir_all(&kept)
            .map_err(|error| ToolFault::io("make the output directory", &kept, error))?;
        let access = Access::new(scope, std::slice::from_ref(&kept));
        let call = Call {
            scope,
            access: &access,
            commands: &self.commands,
            call_id,
            cancel,
        };
        let arguments = request.arguments.unwrap_or_default();
        match scope.tool_set {
            ToolSet::Plain => plain::call(name, arguments, &call).await,
            ToolSet::ClaudeCode => claude_code::call(name, arguments, &call).await,
            ToolSet::Custom => unreachable_custom(name),
        }
    }
}

/// A session of the set `custom` lists no tool, so no tool of it is known.
fn unreachable_custom(name: &str) -> Result<ToolOutput, ToolFault> {
    Err(ToolFault::NotFound(format!(
        "the tool set custom has no tool called {name}"
    )))
}

/// A tool's answer as the result of a call. Nothing is sent as an empty text: some interfaces refuse one.
fn output_result(output: ToolOutput) -> CallToolResult {
    match output {
        ToolOutput::Json(value) => CallToolResult::structured(value),
        ToolOutput::Text(text) => {
            let text = if text.is_empty() {
                "(no result)".to_owned()
            } else {
                text
            };
            CallToolResult::success(vec![ContentBlock::text(text)])
        }
        ToolOutput::Image { data, media_type } => {
            CallToolResult::success(vec![ContentBlock::image(data, media_type)])
        }
    }
}

/// A fault as the result of a call, marked as an error, with its code and what to do about it, which the
/// model reads.
fn fault_result(fault: &ToolFault) -> CallToolResult {
    let mut result = CallToolResult::error(vec![ContentBlock::text(format!(
        "{}: {fault}",
        fault.code()
    ))]);
    result.structured_content = Some(json!({
        "error": { "code": fault.code(), "message": fault.to_string() }
    }));
    result
}
