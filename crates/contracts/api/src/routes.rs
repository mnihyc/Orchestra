//! Route metadata for OpenAPI and permission declarations.
//!
//! Every request of the API is declared once, as an [`Endpoint`]: its
//! method and path, who may make it, and the types of its query, body and
//! reply. The server binds a handler to each declaration and can bind none
//! to anything else; the OpenAPI document and the client's types are
//! written from the same declarations. A module declares its requests with
//! [`endpoints!`](crate::endpoints).

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Serialize, de::DeserializeOwned};

/// An HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Method(&'static str);

impl Method {
    /// `GET`
    pub const GET: Self = Self("GET");
    /// `POST`
    pub const POST: Self = Self("POST");
    /// `PUT`
    pub const PUT: Self = Self("PUT");
    /// `PATCH`
    pub const PATCH: Self = Self("PATCH");
    /// `DELETE`
    pub const DELETE: Self = Self("DELETE");

    /// The method in capitals.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Who may make a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Anybody, without credentials.
    Public,
    /// The browser that holds the session cookie, with the CSRF header.
    Cookie,
    /// Any signed-in caller: a browser session or an API token.
    User,
    /// A browser session only; an API token is refused.
    Session,
    /// A signed-in caller who holds this permission of the catalogue on
    /// the object the path names. The server checks it before the handler
    /// runs its own rules.
    Permission(&'static str),
    /// A signed-in caller, by a rule that is more than one permission on
    /// one object. The words name the rule; the handler enforces it.
    Rule(&'static str),
}

impl Access {
    /// Whether an API token is accepted.
    #[must_use]
    pub const fn tokens(self) -> bool {
        !matches!(self, Self::Public | Self::Cookie | Self::Session)
    }

    /// Whether the caller has to be signed in.
    #[must_use]
    pub const fn signed_in(self) -> bool {
        !matches!(self, Self::Public | Self::Cookie)
    }
}

/// Writes the schema of a type into a generator and returns a reference
/// to it.
pub type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

/// What a part of a request or of its answer is.
#[derive(Debug, Clone, Copy)]
pub enum Shape {
    /// Nothing.
    None,
    /// JSON of the type whose schema this writes (for a query: its
    /// members are the parameters).
    Json(SchemaFn),
    /// Bytes as they are: a piece of an upload.
    Bytes,
    /// Whatever the handler writes: a file, a redirect, a socket, a stream.
    Raw,
}

/// One request of the API.
#[derive(Debug, Clone, Copy)]
pub struct Route {
    /// A name of its own, as `AuthLogin`: the operation's identifier.
    pub name: &'static str,
    /// What it does, in a sentence.
    pub summary: &'static str,
    /// Its method.
    pub method: Method,
    /// Its path under the base path, with parameters as `{id}`.
    pub path: &'static str,
    /// Who may make it.
    pub access: Access,
    /// Its query parameters.
    pub query: Shape,
    /// Its body.
    pub body: Shape,
    /// Its answer when it succeeds.
    pub reply: Shape,
    /// The status of that answer.
    pub status: u16,
    /// Status-specific JSON successes; empty when the single reply and status apply.
    pub responses: &'static [(u16, SchemaFn)],
}

impl Route {
    /// The names of the parameters in the path, in order.
    #[must_use]
    pub fn parameters(&self) -> Vec<&'static str> {
        self.path
            .split('/')
            .filter_map(|part| {
                part.strip_prefix('{')
                    .and_then(|rest| rest.strip_suffix('}'))
            })
            .collect()
    }
}

/// A request, with the types of its parts. Declared by
/// [`endpoints!`](crate::endpoints); a part that is nothing is `()`, and an
/// answer or a body that is not JSON is [`Raw`].
pub trait Endpoint: 'static {
    /// The query parameters.
    type Query: DeserializeOwned + Send + 'static;
    /// The body.
    type Body: Send + 'static;
    /// The answer.
    type Reply: Send + 'static;
    /// What is declared about it.
    const ROUTE: Route;
}

/// A body or an answer that is not JSON, which the handler reads or writes
/// itself. It has no value.
#[derive(Debug, Clone, Copy)]
pub enum Raw {}

/// The schema of `T`, for a [`Shape::Json`].
pub fn schema_of<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<T>()
}

/// Checks at compile time that the JSON parts of an endpoint can be read
/// and written.
#[doc(hidden)]
pub const fn readable<T: DeserializeOwned + JsonSchema>() {}
#[doc(hidden)]
pub const fn writable<T: Serialize + JsonSchema>() {}

/// Declares the requests of one module: a type for each, which implements
/// [`Endpoint`](crate::routes::Endpoint), and `ROUTES`, the list of them.
///
/// ```
/// use orch_api_contract::routes::Access::{Permission, Public};
/// # #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)] pub struct Login { name: String }
/// # #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)] pub struct Session { token: String }
/// # #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)] pub struct Since { since: Option<i64> }
/// orch_api_contract::endpoints! {
///     /// Signs a person in.
///     AuthLogin: POST "/auth/login", Public, none, json Login, json Session, 200;
///     /// Ends the session.
///     AuthLogout: POST "/auth/logout", Public, none, none, none, 204;
///     /// A thread as a file.
///     ThreadExport: GET "/threads/{id}/export", Permission("thread.view"), query Since, none, raw, 200;
///     /// One piece of an upload.
///     UploadPiece: PUT "/uploads/{id}", Public, none, bytes, json Session, 200;
/// }
/// assert_eq!(ROUTES.len(), 4);
/// ```
///
/// After the name: the method, the path, the [`Access`](crate::routes::Access),
/// then the query (`none` or `query T`), the body (`none`, `json T` or
/// `bytes`), the answer (`none`, `json T` or `raw`) and its status. The
/// doc comment is the summary.
/// An optional `responses [201 => Created, 202 => Pending]` declares each
/// success schema by status while the endpoint's reply stays their Rust union.
#[macro_export]
macro_rules! endpoints {
    ($(
        $(#[doc = $doc:literal])*
        $name:ident : $method:ident $path:literal, $access:expr,
        $qk:ident $($qt:ty)?, $bk:ident $($bt:ty)?, $rk:ident $($rt:ty)?, $status:literal
        $(, responses [$($response_status:literal => $response_type:ty),+ $(,)?])?
    );* $(;)?) => {
        $(
            $(#[doc = $doc])*
            #[derive(Debug, Clone, Copy)]
            pub struct $name;

            impl $crate::routes::Endpoint for $name {
                type Query = $crate::__endpoint_part!(type $qk $($qt)?);
                type Body = $crate::__endpoint_part!(type $bk $($bt)?);
                type Reply = $crate::__endpoint_part!(type $rk $($rt)?);
                const ROUTE: $crate::routes::Route = $crate::routes::Route {
                    name: stringify!($name),
                    summary: concat!($($doc),*),
                    method: $crate::routes::Method::$method,
                    path: $path,
                    access: $access,
                    query: $crate::__endpoint_part!(shape in $qk $($qt)?),
                    body: $crate::__endpoint_part!(shape in $bk $($bt)?),
                    reply: $crate::__endpoint_part!(shape out $rk $($rt)?),
                    status: $status,
                    responses: &[$($(($response_status, $crate::routes::schema_of::<$response_type>)),+)?],
                };
            }
        )*

        /// The requests this module declares.
        pub const ROUTES: &[$crate::routes::Route] = &[
            $( <$name as $crate::routes::Endpoint>::ROUTE, )*
        ];
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __endpoint_part {
    (type none) => {
        ()
    };
    (type bytes) => {
        $crate::routes::Raw
    };
    (type raw) => {
        $crate::routes::Raw
    };
    (type query $t:ty) => {
        $t
    };
    (type json $t:ty) => {
        $t
    };
    (shape $way:ident none) => {
        $crate::routes::Shape::None
    };
    (shape $way:ident bytes) => {
        $crate::routes::Shape::Bytes
    };
    (shape $way:ident raw) => {
        $crate::routes::Shape::Raw
    };
    (shape in query $t:ty) => {{
        const _: () = $crate::routes::readable::<$t>();
        $crate::routes::Shape::Json($crate::routes::schema_of::<$t>)
    }};
    (shape in json $t:ty) => {{
        const _: () = $crate::routes::readable::<$t>();
        $crate::routes::Shape::Json($crate::routes::schema_of::<$t>)
    }};
    (shape out json $t:ty) => {{
        const _: () = $crate::routes::writable::<$t>();
        $crate::routes::Shape::Json($crate::routes::schema_of::<$t>)
    }};
}

/// Every request of the API, module by module.
#[must_use]
pub fn all() -> Vec<Route> {
    let modules: &[&[Route]] = &[
        crate::auth::ROUTES,
        crate::users::ROUTES,
        crate::nodes::ROUTES,
        crate::everything::ROUTES,
        crate::settings::ROUTES,
        crate::access::ROUTES,
        crate::ownership::ROUTES,
        crate::threads::ROUTES,
        crate::calls::ROUTES,
        crate::goals::ROUTES,
        crate::thread_move::ROUTES,
        crate::background::ROUTES,
        crate::native::ROUTES,
        crate::binding::ROUTES,
        crate::forks::ROUTES,
        crate::topics::ROUTES,
        crate::workspace::ROUTES,
        crate::source::ROUTES,
        crate::environment::ROUTES,
        crate::loss::ROUTES,
        crate::trust::ROUTES,
        crate::terminals::ROUTES,
        crate::browsers::ROUTES,
        crate::displays::ROUTES,
        crate::processes::ROUTES,
        crate::ports::ROUTES,
        crate::approvals::ROUTES,
        crate::resources::ROUTES,
        crate::storage::ROUTES,
        crate::resource_facts::ROUTES,
        crate::library::ROUTES,
        crate::model_inputs::ROUTES,
        crate::instructions::ROUTES,
        crate::input_schedules::ROUTES,
        crate::context::ROUTES,
        crate::mounts::ROUTES,
        crate::skills::ROUTES,
        crate::notifications::ROUTES,
        crate::admin::ROUTES,
        crate::program_admin::ROUTES,
        crate::downloads::ROUTES,
        crate::search::ROUTES,
        crate::links::ROUTES,
        crate::usage::ROUTES,
        crate::chats::ROUTES,
        crate::documents::ROUTES,
        crate::document_tree::ROUTES,
        crate::share::ROUTES,
        crate::exports::ROUTES,
        crate::imports::ROUTES,
        crate::personal::ROUTES,
        crate::first_steps::ROUTES,
        crate::directory::ROUTES,
    ];
    modules
        .iter()
        .flat_map(|routes| routes.iter().copied())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_request_is_declared_twice_and_every_path_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for route in all() {
            // Two requests are the same when their paths differ only in the names of parameters.
            let shape: String = route
                .path
                .split('/')
                .map(|part| if part.starts_with('{') { "{}" } else { part })
                .collect::<Vec<_>>()
                .join("/");
            assert!(
                seen.insert((route.method, shape)),
                "{} {} twice",
                route.method.as_str(),
                route.path
            );
            assert!(names.insert(route.name), "{} twice", route.name);
            assert!(
                route.path.starts_with('/') && !route.path.ends_with('/'),
                "{}",
                route.path
            );
            assert!(
                !route.summary.trim().is_empty(),
                "{} has no summary",
                route.name
            );
            if let Access::Permission(key) = route.access {
                assert!(key.contains('.'), "{}: {key}", route.name);
                assert!(
                    !route.parameters().is_empty(),
                    "{} checks a permission on nothing",
                    route.name
                );
            }
        }
    }
}
