//! Sync, timeline with the lines of a thread's history and that history as a view, inputs on the sender's own access, side threads, the limits in a turn's way and letting an input past them, draft, pins, binding, fork from any message and exports at three depths.
//!
//! A thread is a node of the tree, made in a topic and read and changed through its own requests. What
//! the engine keeps of it is shown here as the wire has it; who may do what is the access engine's to say,
//! asked of it before a handler runs or by the engine's own functions inside the transaction of the act.

pub mod adopt;
pub(crate) mod approval_items;
pub mod background;
pub mod binding;
pub mod calls;
pub mod context;
pub mod continuations;
pub mod drive;
pub mod export;
pub mod goals;
pub mod instructions;
pub mod make;
pub mod model_inputs;
pub mod native;
mod opening;
pub mod parameters;
pub mod raw;
pub mod read;
mod read_history;
pub mod rules;
pub mod source;
pub mod uploads;
pub mod view;
pub mod written_export;

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::Node;
use orch_api_contract::threads::Thread;
use orch_authz::Policy;
use orch_db::Conn;
use orch_engine::binding::CreateThreadError;
use orch_engine::inputs::InputError;
use orch_engine::views::ReadError;
use orch_types::Id;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    make::mount(registry);
    read::mount(registry);
    read_history::mount(registry);
    opening::mount(registry);
    source::mount(registry);
    drive::mount(registry);
    parameters::mount(registry);
    export::mount(registry);
    binding::mount(registry);
    calls::mount(registry);
    goals::mount(registry);
    uploads::mount(registry);
    adopt::mount(registry);
    raw::mount(registry);
    background::mount(registry);
    rules::mount(registry);
    model_inputs::mount(registry);
    native::mount(registry);
    instructions::mount(registry);
    context::mount(registry);
    continuations::mount(registry);
}

/// The thread `thread` as the caller sees it.
pub async fn one(ctx: &Ctx, conn: &mut Conn, thread: Id) -> ApiResult<Thread> {
    let principal = authorization::principal(ctx, conn).await?;
    match crate::routes::nodes::view::one(conn, &principal, thread, ctx.now, Policy::default())
        .await?
    {
        Some(Node::Thread(thread)) => Ok(thread),
        _ => Err(ApiError::not_found()),
    }
}

/// Why a thread could not be made, as the problem a client acts on.
pub async fn create_failed(conn: &mut Conn, error: CreateThreadError) -> ApiError {
    match error {
        CreateThreadError::NoSuchTopic => ApiError::not_found(),
        CreateThreadError::Denied(refusal) => match refusals::denied(conn, *refusal).await {
            Ok(error) | Err(error) => error,
        },
        CreateThreadError::InvalidName => {
            ApiError::invalid("name", "must be between 1 and 120 characters")
        }
        CreateThreadError::InvalidDirectory => ApiError::invalid(
            "directory",
            "is relative to the workspace root and does not go up",
        ),
        CreateThreadError::AgentUnavailable => ApiError::invalid(
            "binding.agent_id",
            "the agent profile is not there or is switched off",
        ),
        CreateThreadError::ProviderUnavailable => ApiError::invalid(
            "binding.provider_id",
            "the provider is not there or is switched off",
        ),
        CreateThreadError::ProviderNotAllowed(why) => provider_policy_failed(conn, why).await,
        CreateThreadError::EnvironmentUnavailable => {
            ApiError::invalid("environment.use", "the selected environment is unavailable")
        }
        CreateThreadError::AgentNotReported => ApiError::new(
            ErrorCode::BindingIncompatible,
            "the agent profile has not reported what its harness is",
        )
        .with("reason", "harness_unreported"),
        CreateThreadError::ModelNotAllowed(model) => ApiError::new(
            ErrorCode::ModelNotAllowed,
            format!("the model {model} is not available here"),
        )
        .with("reason", "model")
        .with("model", model),
        CreateThreadError::EffortNotListed(effort) => ApiError::new(
            ErrorCode::ModelNotAllowed,
            format!("the model does not take the effort {effort}"),
        )
        .with("reason", "effort")
        .with("effort", effort),
        CreateThreadError::Incompatible(why) => ApiError::new(
            ErrorCode::BindingIncompatible,
            orch_engine::binding::describe(&why),
        )
        .with("reason", why.code()),
        CreateThreadError::InstructionsTooLarge(characters) => {
            let limit = orch_engine::sessions::STANDING_LIMIT;
            ApiError::new(
                ErrorCode::SettingsInstructionsTooLarge,
                format!(
                    "the standing instructions here come to {characters} characters with their headings, and {limit} is the most"
                ),
            )
            .with("limit", limit)
            .with("characters", characters)
            .with("scope", Option::<orch_api_contract::nodes::NodeRef>::None)
        }
        CreateThreadError::Setting(message) | CreateThreadError::Page(message) => {
            ApiError::invalid("", &message)
        }
        CreateThreadError::FirstInput { thread, error } => input_failed(conn, thread, *error).await,
        CreateThreadError::Internal(error) => ApiError::engine(error),
    }
}

/// A provider policy refusal with the ordinary provider reference and the controlling scope.
pub(super) async fn provider_policy_failed(
    conn: &mut Conn,
    why: orch_engine::provider_policy::Refused,
) -> ApiError {
    let provider =
        match orch_db::queries::resources::thread_engine::provider(conn, why.provider).await {
            Ok(Some(provider)) => provider,
            Ok(None) => return ApiError::not_found(),
            Err(error) => return error.into(),
        };
    let reference = match orch_engine::views::provider_ref_json(conn, &provider).await {
        Ok(reference) => reference,
        Err(error) => return ApiError::internal(error),
    };
    ApiError::new(ErrorCode::ProviderNotAllowed, why.to_string())
        .with("provider", reference)
        .with("scope", why.source)
}

/// Why an input could not be taken, changed or cancelled.
pub async fn input_failed(conn: &mut Conn, thread: Id, error: InputError) -> ApiError {
    match error {
        InputError::Native(error) => native_failed(conn, error).await,
        InputError::NotFound => ApiError::not_found(),
        InputError::Denied(refusal) => match refusals::denied(conn, *refusal).await {
            Ok(error) | Err(error) => error,
        },
        InputError::Archived => match refusals::named_node(conn, thread).await {
            Ok(node) => ApiError::new(
                ErrorCode::NodeArchived,
                "this is archived, and so is read-only",
            )
            .with("archived_node", node),
            Err(error) => error,
        },
        InputError::Empty => ApiError::invalid("content", "an input says something"),
        InputError::TooLong => ApiError::invalid(
            "content",
            "an input holds at most 100000 characters of text",
        ),
        InputError::Invalid(message) => ApiError::invalid("", message),
        InputError::SteerUnsupported => ApiError::new(
            ErrorCode::CapabilityUnsupported,
            "the agent does not take a message into a turn that runs",
        )
        .with("capability", "steer")
        .with(
            "fallback_delivery",
            orch_api_contract::threads::Delivery::Interrupt,
        ),
        InputError::PlanUnsupported => ApiError::new(
            ErrorCode::CapabilityUnsupported,
            "the agent program of this thread has no plan mode",
        )
        .with("capability", "plan_mode"),
        // Its own refusal, with the state the input is in: it was taken up, sent or removed meanwhile, which is
        // not a change of somebody's revision.
        InputError::AlreadyDecided(current) => ApiError::new(
            ErrorCode::ApprovalAlreadyDecided,
            "The request was decided already",
        )
        .with("current", current),
        InputError::NotPending { state } => ApiError::new(
            ErrorCode::InputNotPending,
            "the input is not waiting any more",
        )
        .with("state", state),
        InputError::NotTheAuthor => author_only(thread),
        InputError::StaleQueue { revision } => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the queue changed since it was seen",
        )
        .with("current_revision", revision),
        InputError::InactiveContinuation {
            continuation_id,
            active_continuation_id,
        } => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "Select this input's history before changing its delivery or native command.",
        )
        .with("continuation_id", continuation_id)
        .with("active_continuation_id", active_continuation_id),
        InputError::NotHeld { state, detail } => ApiError::new(
            ErrorCode::InputNotHeld,
            "the input is not held for its files",
        )
        .with("state", state)
        .with("state_detail", detail),
        InputError::Internal(error) => ApiError::engine(error),
    }
}

/// Native selector faults carry no supplied arguments or private artifact contents.
pub(crate) async fn native_failed(
    conn: &mut Conn,
    error: orch_engine::native_actions::NativeError,
) -> ApiError {
    use orch_engine::native_actions::NativeError;
    match error {
        NativeError::Unavailable => ApiError::new(
            ErrorCode::NativeActionUnavailable,
            "This command is unavailable for the current session",
        ),
        NativeError::Stale { current_revision } => ApiError::new(
            ErrorCode::NativeActionStale,
            "The session's command list changed",
        )
        .with("current_revision", current_revision),
        NativeError::InvalidArguments => ApiError::invalid(
            "native_action.arguments",
            "the arguments do not meet the selected command's declaration",
        ),
        NativeError::Denied(refusal) => match refusals::denied(conn, *refusal).await {
            Ok(error) | Err(error) => error,
        },
        NativeError::Internal(error) => ApiError::engine(error),
    }
}

/// The refusal to change the words of an input that somebody else sent: what
/// a person says is theirs, and nobody can give the right to change it.
pub fn author_only(thread: Id) -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "only the author changes the words of an input",
    )
    .with("required", "thread.interact")
    .with(
        "object",
        serde_json::json!({ "kind": "thread", "id": thread }),
    )
    .with("requestable", false)
    .with("override", false)
    .with(
        "missing",
        Vec::<orch_api_contract::access::MissingAccess>::new(),
    )
}

/// Why a thread could not be read.
#[must_use]
pub fn read_failed(error: ReadError) -> ApiError {
    match error {
        ReadError::ProjectionChanged(projection) => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The viewed continuation changed; reload this history projection.",
        )
        .with("projection", projection),
        ReadError::NotFound | ReadError::NotACommand => ApiError::not_found(),
        ReadError::BadRequest(message) => ApiError::invalid("", message),
        error @ (ReadError::OutputInBlobStore | ReadError::Internal(_)) => {
            ApiError::internal(error)
        }
    }
}
